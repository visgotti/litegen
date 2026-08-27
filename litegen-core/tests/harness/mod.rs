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

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use std::sync::Arc;
use tower::ServiceExt;

use bytes::Bytes;
use litegen::api::middleware::AppState;
use litegen::capabilities::CapabilityRegistry;
use litegen::config::{AppConfig, CacheGlobalConfig, DevFlags, Mode};
use litegen::db::sqlite::SqliteDatabase;
use litegen::db::DatabaseStore;
use litegen::proxy::cache::GenerationCache;
use litegen::proxy::materializer::{MaterializeError, Materializer, TempStorage};
use litegen::proxy::registry::ProviderRegistry;
use litegen::proxy::router::ProxyRouter;
use litegen::proxy::storage::{ImageStore, LocalStore};

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
pub async fn app_with_mock_3d() -> (Router, Arc<SqliteDatabase>) {
    let db = Arc::new(
        SqliteDatabase::connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite"),
    );

    let mut app_config = AppConfig::default();
    app_config.providers.insert(
        "mock".to_string(),
        serde_json::from_value(serde_json::json!({ "enabled": true }))
            .expect("mock provider config"),
    );
    let provider_registry = Arc::new(ProviderRegistry::new());
    provider_registry.init_from_config(&app_config).await;

    let config = Arc::new(app_config);
    let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
    let image_store: Arc<dyn ImageStore> = Arc::new(LocalStore);
    let router = Arc::new(ProxyRouter::new(provider_registry, cache, config, image_store));

    let materializer = Arc::new(Materializer::new(Arc::new(NoopStorage), reqwest::Client::new()));

    // Shipped capability registry lives at <repo>/models, i.e. CARGO_MANIFEST_DIR
    // (litegen-core) with the last component popped, then `models`.
    let mut models_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    models_dir.pop();
    models_dir.push("models");
    let cap_registry = Arc::new(CapabilityRegistry::from_dir(&models_dir).expect("load shipped models"));

    let state = Arc::new(AppState {
        router,
        db: db.clone() as Arc<dyn DatabaseStore>,
        master_key: None,
        registry: cap_registry,
        materializer,
        rate_limiter: Arc::new(litegen::api::middleware::rate_limit::RateLimiter::new()),
        in_flight: Arc::new(litegen::api::middleware::backpressure::InFlightLimit::new(64)),
        oauth: litegen::auth::oauth::OAuthConfig::default(),
        mode: Mode::SingleTenant,
        secrets_key: None,
        dev: DevFlags::default(),
        allow_password: true,
    });

    let app = litegen::api::create_router(state);
    (app, db)
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
