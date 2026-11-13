//! Shared harness for the `model3d` HTTP integration suite (`tests/model3d_api.rs`).
//!
//! Builds the REAL route table (`litegen::api::create_router`) over an
//! in-memory SQLite DB, so generation rows persist across the submit/poll
//! calls a client would actually make. The mock image/video/3d providers are
//! registered the same way `tests/multitenant_api.rs` does: the `register_mock_*`
//! helpers on `ProviderRegistry` are `#[cfg(test)]` and so are invisible to an
//! integration-test crate (which links the lib WITHOUT `--cfg test`). Instead
//! we add a `mock` entry to `AppConfig::providers` and run `init_from_config`,
//! which has a credential-exempt `"mock"` arm and registers the mock across
//! all three modalities (image/video/model3d) in one call.

// Every integration test binary includes this module and uses part of it, so
// the rest is dead code in that binary.
#![allow(dead_code)]

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

use bytes::Bytes;
use litegen::api::middleware::AppState;
use litegen::capabilities::CapabilityRegistry;
use litegen::config::{AppConfig, CacheGlobalConfig, Mode};
use litegen::db::sqlite::SqliteDatabase;
use litegen::db::DatabaseStore;
use litegen::proxy::cache::GenerationCache;
use litegen::proxy::materializer::{MaterializeError, Materializer, TempStorage};
use litegen::proxy::registry::ProviderRegistry;
use litegen::proxy::router::ProxyRouter;
use litegen::proxy::storage::{ImageStore, LocalStore};
use litegen::shared::SharedRedis;

pub async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 10 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

// ─── Materializer temp storage (mirrors tests/multitenant_api.rs) ──────────

struct NoopStorage;

#[async_trait::async_trait]
impl TempStorage for NoopStorage {
    async fn put(&self, key: &str, _bytes: Bytes, _ct: &str) -> Result<String, MaterializeError> {
        Ok(format!("local://{}", key))
    }
    async fn delete(&self, _key: &str) -> Result<(), MaterializeError> {
        Ok(())
    }
}

/// Full app with the mock image/video/3d providers and an in-memory sqlite DB.
/// Assets go to a fresh temporary directory.
pub async fn app_with_mock_3d() -> (Router, Arc<SqliteDatabase>) {
    let dir = tempfile::tempdir().expect("asset dir").keep();
    app_with_mock_3d_assets_in(&dir).await
}

/// Like [`app_with_mock_3d`], with stored assets kept in `asset_dir` — two apps
/// over one directory are a process before and after a restart.
pub async fn app_with_mock_3d_assets_in(asset_dir: &std::path::Path) -> (Router, Arc<SqliteDatabase>) {
    app_with(asset_dir, |_| {}).await
}

/// The full app, with `tweak` applied to its config before anything is built.
pub async fn app_with(asset_dir: &std::path::Path, tweak: impl FnOnce(&mut AppConfig)) -> (Router, Arc<SqliteDatabase>) {
    let db = Arc::new(
        SqliteDatabase::connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite"),
    );
    (app_over(db.clone(), asset_dir, tweak).await, db)
}

/// The full app over a database the test opened itself — a file it can also
/// reach with its own connection, to inject a failure or restart over it.
pub async fn app_over(db: Arc<SqliteDatabase>, asset_dir: &std::path::Path, tweak: impl FnOnce(&mut AppConfig)) -> Router {
    build_app(db, asset_dir, tweak, None, Vec::new()).await
}

/// [`app_over`] with authentication on: a master key (so requests need real
/// credentials) and a tenancy mode. Dev flags come from the tweaked config, as
/// they do in `main`, so a test can expose invitation tokens.
pub async fn app_over_with(
    db: Arc<SqliteDatabase>,
    asset_dir: &std::path::Path,
    tweak: impl FnOnce(&mut AppConfig),
    master_key: Option<&str>,
    mode: Mode,
) -> Router {
    build_app_in(db, asset_dir, tweak, master_key, Vec::new(), Arc::new(LocalStore), mode, None).await.0
}

/// [`app_over_with`], with the state behind it (the router's in-memory jobs).
pub async fn app_and_state_in(
    db: Arc<SqliteDatabase>,
    asset_dir: &std::path::Path,
    tweak: impl FnOnce(&mut AppConfig),
    master_key: Option<&str>,
    mode: Mode,
) -> (Router, Arc<AppState>) {
    build_app_in(db, asset_dir, tweak, master_key, Vec::new(), Arc::new(LocalStore), mode, None).await
}

/// [`app_over`], with the state behind it: what a background task (the
/// settings refresher, the poller) is handed.
pub async fn app_and_state_over(db: Arc<SqliteDatabase>, asset_dir: &std::path::Path) -> (Router, Arc<AppState>) {
    build_app_and_state(db, asset_dir, |_| {}, None, Vec::new()).await
}

/// [`app_and_state_over`], with the config tweaked first.
pub async fn app_and_state_over_with(
    db: Arc<SqliteDatabase>,
    asset_dir: &std::path::Path,
    tweak: impl FnOnce(&mut AppConfig),
) -> (Router, Arc<AppState>) {
    build_app_and_state(db, asset_dir, tweak, None, Vec::new()).await
}

/// The full app behind `master_key` whose image store (the S3 image store's
/// seat) is `image_store`, with the config adjusted first.
pub async fn app_with_image_store(
    asset_dir: &std::path::Path,
    master_key: Option<&str>,
    image_store: Arc<dyn ImageStore>,
    tweak: impl FnOnce(&mut AppConfig),
) -> (Router, Arc<SqliteDatabase>) {
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory sqlite"));
    let app = build_app_and_state_with(db.clone(), asset_dir, tweak, master_key, Vec::new(), image_store).await.0;
    (app, db)
}

/// The full app behind `master_key`, with these generation call hooks. Hook
/// paths resolve against `tests/fixtures/hooks`.
pub async fn app_with_hooks(
    asset_dir: &std::path::Path,
    master_key: Option<&str>,
    hooks: Vec<litegen::hooks::HookConfig>,
) -> (Router, Arc<SqliteDatabase>) {
    app_with_hooks_tweaked(asset_dir, master_key, hooks, |_| {}).await
}

/// [`app_with_hooks`], with the config adjusted first.
pub async fn app_with_hooks_tweaked(
    asset_dir: &std::path::Path,
    master_key: Option<&str>,
    hooks: Vec<litegen::hooks::HookConfig>,
    tweak: impl FnOnce(&mut AppConfig),
) -> (Router, Arc<SqliteDatabase>) {
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.expect("in-memory sqlite"));
    (build_app(db.clone(), asset_dir, tweak, master_key, hooks).await, db)
}

async fn build_app(
    db: Arc<SqliteDatabase>,
    asset_dir: &std::path::Path,
    tweak: impl FnOnce(&mut AppConfig),
    master_key: Option<&str>,
    hooks: Vec<litegen::hooks::HookConfig>,
) -> Router {
    build_app_and_state(db, asset_dir, tweak, master_key, hooks).await.0
}

async fn build_app_and_state(
    db: Arc<SqliteDatabase>,
    asset_dir: &std::path::Path,
    tweak: impl FnOnce(&mut AppConfig),
    master_key: Option<&str>,
    hooks: Vec<litegen::hooks::HookConfig>,
) -> (Router, Arc<AppState>) {
    build_app_and_state_with(db, asset_dir, tweak, master_key, hooks, Arc::new(LocalStore)).await
}

async fn build_app_and_state_with(
    db: Arc<SqliteDatabase>,
    asset_dir: &std::path::Path,
    tweak: impl FnOnce(&mut AppConfig),
    master_key: Option<&str>,
    hooks: Vec<litegen::hooks::HookConfig>,
    image_store: Arc<dyn ImageStore>,
) -> (Router, Arc<AppState>) {
    build_app_in(db, asset_dir, tweak, master_key, hooks, image_store, Mode::SingleTenant, None).await
}

/// What a replica of a deployment is wired with beyond the plain harness app:
/// the pieces `main` adds when it runs one of several litegen servers.
struct ReplicaWiring {
    shared: Option<Arc<SharedRedis>>,
    subscribe: bool,
}

#[allow(clippy::too_many_arguments)]
async fn build_app_in(
    db: Arc<dyn DatabaseStore>,
    asset_dir: &std::path::Path,
    tweak: impl FnOnce(&mut AppConfig),
    master_key: Option<&str>,
    hooks: Vec<litegen::hooks::HookConfig>,
    image_store: Arc<dyn ImageStore>,
    mode: Mode,
    replica: Option<ReplicaWiring>,
) -> (Router, Arc<AppState>) {
    let mut app_config = AppConfig::default();
    tweak(&mut app_config);
    let dev = app_config.dev.clone();
    app_config.providers.insert(
        "mock".to_string(),
        serde_json::from_value(serde_json::json!({ "enabled": true }))
            .expect("mock provider config"),
    );
    app_config.image_storage.local_dir = Some(asset_dir.display().to_string());
    let provider_registry = Arc::new(ProviderRegistry::new());
    provider_registry.init_from_config(&app_config).await;

    let max_in_flight = app_config.backpressure.max_in_flight;
    // As main.rs decodes it: app buckets and stored credentials need it.
    let secrets_key = app_config
        .secrets_key
        .as_deref()
        .and_then(|k| base64::Engine::decode(&base64::engine::general_purpose::STANDARD, k.trim()).ok())
        .and_then(|b| <[u8; 32]>::try_from(b).ok());
    let config = Arc::new(app_config);
    let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
    let public_base_url = config.server.public_base_url();
    let router = ProxyRouter::new(provider_registry, cache, config.clone(), image_store);
    // A replica is wired in main's order: the signing key every replica reads
    // from the database, then the Redis they share.
    let router = match &replica {
        Some(wiring) => {
            let signer = litegen::access::signing::link_signer_for(config.secrets_key.as_deref(), &*db).await;
            let router = router.with_link_signer(Arc::new(signer));
            match &wiring.shared {
                Some(shared) => router.with_shared(shared.clone()),
                None => router,
            }
        }
        None => router,
    };
    let router = Arc::new(router);

    let materializer = Arc::new(
        Materializer::new(Arc::new(NoopStorage), reqwest::Client::new()).with_publisher(Arc::new(
            litegen::proxy::ref_publish::RefPublisher::new(router.model3d_store.clone(), public_base_url),
        )),
    );

    // Shipped capability registry lives at <repo>/models, i.e. CARGO_MANIFEST_DIR
    // (litegen-core) with the last component popped, then `models`.
    let mut models_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    models_dir.pop();
    models_dir.push("models");
    let cap_registry = Arc::new(CapabilityRegistry::from_dir(&models_dir).expect("load shipped models"));

    let rate_limiter = match replica.as_ref().and_then(|w| w.shared.clone()) {
        Some(shared) => litegen::api::middleware::rate_limit::RateLimiter::with_shared(shared),
        None => litegen::api::middleware::rate_limit::RateLimiter::new(),
    };
    let state = Arc::new(AppState {
        router,
        db: db.clone(),
        master_key: master_key.map(str::to_string),
        registry: cap_registry,
        materializer,
        rate_limiter: Arc::new(rate_limiter),
        in_flight: Arc::new(litegen::api::middleware::backpressure::InFlightLimit::with_body_timeout(
            max_in_flight,
            config.server.request_body_timeout(),
        )),
        oauth: litegen::auth::oauth::OAuthConfig::default(),
        mode,
        secrets_key,
        dev,
        allow_password: true,
    });

    if let Some(wiring) = &replica {
        // As main does, once the state exists: what the management API stored,
        // then the loop that follows what other replicas save. Its handle is
        // kept for `stop_replica`.
        litegen::management::runtime::apply_at_boot(&state).await.expect("management runtime config applied");
        let sync = litegen::management::runtime::spawn_runtime_sync(state.clone(), std::time::Duration::from_secs(15), wiring.subscribe);
        replica_tasks().lock().unwrap_or_else(|p| p.into_inner()).push((Arc::as_ptr(&state) as usize, sync));
    }

    let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hooks");
    let hooks = litegen::hooks::HookSet::start(&hooks, &fixtures, max_in_flight, state.router.config.server.request_body_timeout())
        .await
        .expect("hooks start");
    (litegen::api::create_router_with_hooks(state.clone(), hooks), state)
}

// ─── Replicas ──────────────────────────────────────────────────────────────

/// How a replica differs from the next one in a test. A `Default` is a replica
/// as `main` runs it.
pub struct ReplicaOptions {
    /// Whether the replica listens for the other replicas' events on Redis.
    /// `false` is the switch the spec names for tests: changes then reach it
    /// only at its next refresh (`management::runtime::refresh`), which a test
    /// calls by hand.
    pub subscribe: bool,
}

impl Default for ReplicaOptions {
    fn default() -> Self {
        Self { subscribe: true }
    }
}

/// The background loops of the replicas built so far, by their state's address:
/// the runtime sync, which holds the event listener.
fn replica_tasks() -> &'static Mutex<Vec<(usize, tokio::task::JoinHandle<()>)>> {
    static TASKS: Mutex<Vec<(usize, tokio::task::JoinHandle<()>)>> = Mutex::new(Vec::new());
    &TASKS
}

/// Stop what [`replica`] started for `state`: its runtime sync and, with it,
/// its event listener. A test calls it before it drops the database the
/// replica reads.
pub fn stop_replica(state: &Arc<AppState>) {
    let key = Arc::as_ptr(state) as usize;
    let mut tasks = replica_tasks().lock().unwrap_or_else(|p| p.into_inner());
    tasks.retain(|(owner, handle)| {
        if *owner == key {
            handle.abort();
        }
        *owner != key
    });
}

/// One replica of a deployment over `db`: the full route table and its
/// state, built the way `main` builds the production app.
///
/// - **Wiring:** `ProxyRouter::new`, then `with_link_signer` (the key every
///   replica reads from the database), then `with_shared` when there is a
///   Redis, which also moves the provider breaker and the cache's second
///   tier onto it. The limiter shares the Redis too. Then what the management
///   API stored is applied, and the runtime sync starts
///   (`opts.subscribe` says whether it listens for events).
/// - **Config:** `tweak` edits the default config first, so it sets the master
///   key, the mode, the secrets key and the rest, as `litegen.yaml` would. The
///   mock providers are registered as in every harness app.
/// - **Left out:** the poller, which would race a test that drives its ticks
///   by hand (`poll_once_as`), and the retention sweeps.
/// - **Stopping:** [`stop_replica`].
pub async fn replica(
    db: Arc<dyn DatabaseStore>,
    shared: Option<Arc<SharedRedis>>,
    asset_dir: &Path,
    opts: ReplicaOptions,
    tweak: impl FnOnce(&mut AppConfig),
) -> (Router, Arc<AppState>) {
    // `main` reads the master key and the mode from the config, so the tweak
    // is applied once, here, and handed on as the config itself.
    let mut config = AppConfig::default();
    tweak(&mut config);
    let (master_key, mode) = (config.master_key.clone(), config.mode);
    let wiring = ReplicaWiring { shared, subscribe: opts.subscribe };
    build_app_in(db, asset_dir, move |c| *c = config, master_key.as_deref(), Vec::new(), Arc::new(LocalStore), mode, Some(wiring)).await
}

pub async fn submit(app: &Router, model: &str, prompt: &str) -> String {
    let body = serde_json::json!({ "model": model, "prompt": prompt }).to_string();
    let resp = app.clone().oneshot(
        Request::post("/v1/models3d/generations")
            .header("content-type", "application/json")
            .body(Body::from(body)).unwrap(),
    ).await.unwrap();
    json_body(resp).await["id"].as_str().unwrap().to_string()
}

/// Submit with an explicit body, for the params `submit` does not take.
pub async fn submit_with(app: &Router, body: serde_json::Value) -> axum::response::Response {
    app.clone().oneshot(
        Request::post("/v1/models3d/generations")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())).unwrap(),
    ).await.unwrap()
}

pub async fn poll_until_terminal(app: &Router, id: &str) -> serde_json::Value {
    for _ in 0..10 {
        let resp = app.clone().oneshot(
            Request::get(format!("/v1/models3d/{id}")).body(Body::empty()).unwrap(),
        ).await.unwrap();
        let v = json_body(resp).await;
        if v["status"] == "completed" || v["status"] == "failed" || v["status"] == "cancelled" {
            return v;
        }
    }
    panic!("3d job {id} never reached a terminal status");
}

pub async fn run_to_completion(app: &Router, model: &str, prompt: &str) -> String {
    let id = submit(app, model, prompt).await;
    let last = poll_until_terminal(app, &id).await;
    last["assets"].as_array().unwrap().iter()
        .find(|a| a["kind"] == "mesh").unwrap()["url"].as_str().unwrap().to_string()
}
