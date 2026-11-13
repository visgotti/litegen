//! The two-replica proof (`docs/superpowers/specs/2026-11-09-horizontal-scaling-design.md`,
//! "Proof"): two full litegen replicas, each an `AppState` and the real route
//! table built the way `main` builds them, over ONE Postgres database and ONE
//! Redis. Each test is one line of the spec's list.
//!
//! **Gated.** A test needs both `LITEGEN_PG_TEST_URL` and
//! `LITEGEN_REDIS_TEST_URL` (throwaway containers, for example
//! `postgres://postgres:pw@127.0.0.1:55441/litegen_test` and
//! `redis://127.0.0.1:56379`). Without either it prints why it skipped and
//! passes.
//!
//! **Isolated.** A test makes its own database, `litegen_scaling_<uuid>`, on
//! the server behind `LITEGEN_PG_TEST_URL`, so both replicas run the
//! migrations fresh, and drops it at the end (also when the test fails). It
//! has its own Redis key prefix too. The replicas' background tasks are
//! stopped before the database goes.
//!
//! **Timing.** "Within about a second" is polled for up to [`SLACK`], since
//! the machines these run on are loaded; each message says so. The spec's
//! target is about a second.

mod harness;

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use harness::ReplicaOptions;
use litegen::api::middleware::{AppState, DEFAULT_APP_ID};
use litegen::config::connections::RedisSettings;
use litegen::config::{AppConfig, Mode};
use litegen::db::DatabaseStore;
use litegen::shared::testing::{addr_of, flush_keys, test_prefix, test_url, CutProxy};
use litegen::shared::SharedRedis;
use serde_json::{json, Value};
use sqlx::Connection;
use tower::ServiceExt;

const MASTER: &str = "sk-scaling-master";

/// What the 503 says when fail-closed refuses a request: the plan's fixed
/// copy, kept here so a test locks it.
const FAIL_CLOSED_MESSAGE: &str = "Rate limit enforcement unavailable: request counters could not be verified \
    against Redis, and LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT is on, so the request was refused to avoid \
    exceeding the configured limit. Retry shortly.";

/// How long a "within about a second" check is given: the spec's target is
/// about a second, and the deadline allows for a busy machine.
const SLACK: Duration = Duration::from_secs(2);

// ─── The world a test runs in ───────────────────────────────────────────────

/// One test's database, Redis prefix and asset directory, and the replicas
/// built over them. Dropping it stops the replicas' tasks and drops the
/// database.
struct World {
    pg_admin_url: String,
    db_name: String,
    db_url: String,
    redis_url: String,
    prefix: String,
    assets: tempfile::TempDir,
    states: Vec<Arc<AppState>>,
}

/// One replica: the route table and the state behind it.
struct Replica {
    app: Router,
    state: Arc<AppState>,
}

/// An answer: status, headers and the JSON body (`Null` when it is not JSON).
struct Reply {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Value,
}

impl World {
    /// A fresh database and prefix, or `None` after saying why the test is
    /// skipped.
    async fn start() -> Option<World> {
        let pg_admin_url = std::env::var("LITEGEN_PG_TEST_URL").ok().filter(|u| !u.trim().is_empty());
        if pg_admin_url.is_none() {
            eprintln!("skipping: LITEGEN_PG_TEST_URL is not set (a throwaway Postgres, for example postgres://postgres:pw@127.0.0.1:55441/litegen_test)");
        }
        // Both skip lines print when both are missing.
        let redis_url = test_url();
        let (pg_admin_url, redis_url) = (pg_admin_url?, redis_url?);

        let db_name = format!("litegen_scaling_{}", uuid::Uuid::new_v4().simple());
        let mut admin = sqlx::PgConnection::connect(&pg_admin_url).await.expect("connect to LITEGEN_PG_TEST_URL");
        sqlx::query(&format!("CREATE DATABASE {db_name}")).execute(&mut admin).await.expect("create the test database");
        Some(World {
            db_url: url_with_database(&pg_admin_url, &db_name),
            pg_admin_url,
            db_name,
            redis_url,
            prefix: test_prefix(),
            assets: tempfile::tempdir().expect("asset dir"),
            states: Vec::new(),
        })
    }

    /// A client of the test Redis on this world's prefix: what one replica's
    /// process holds. Every call makes another, so replicas never share one.
    async fn redis(&self) -> Arc<SharedRedis> {
        self.redis_via(&self.redis_url, |_| {}).await
    }

    /// [`Self::redis`] through `url` (a [`CutProxy`]'s, say), with its settings
    /// adjusted.
    async fn redis_via(&self, url: &str, adjust: impl FnOnce(&mut RedisSettings)) -> Arc<SharedRedis> {
        let mut settings = RedisSettings::from_url(url);
        adjust(&mut settings);
        SharedRedis::connect(&settings, &self.prefix).await.expect("the test Redis URL is usable")
    }

    /// A replica over this world's database (its own connection to it, so the
    /// migrations run for it) and asset directory, with `shared`.
    async fn replica(&mut self, shared: Option<Arc<SharedRedis>>, opts: ReplicaOptions, tweak: impl FnOnce(&mut AppConfig)) -> Replica {
        let db: Arc<dyn DatabaseStore> = Arc::from(litegen::db::connect(&self.db_url).await.expect("connect to the test database"));
        let (app, state) = harness::replica(db, shared, self.assets.path(), opts, tweak).await;
        self.states.push(state.clone());
        Replica { app, state }
    }

    /// Replicas A and B on one Redis, both behind [`MASTER`].
    async fn pair(&mut self, opts: impl Fn() -> ReplicaOptions, tweak: impl Fn(&mut AppConfig)) -> (Replica, Replica) {
        let configured = |c: &mut AppConfig| {
            c.master_key = Some(MASTER.to_string());
            tweak(c);
        };
        let (redis_a, redis_b) = (self.redis().await, self.redis().await);
        let a = self.replica(Some(redis_a), opts(), configured).await;
        let b = self.replica(Some(redis_b), opts(), configured).await;
        (a, b)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        for state in &self.states {
            harness::stop_replica(state);
        }
        // Dropping the database takes a connection, and a drop cannot await:
        // a thread of its own, with its own runtime, does it. FORCE ends what
        // the replicas' pools still hold open.
        // The same thread deletes this world's Redis keys, so a test that
        // fails leaves nothing behind in either.
        let (url, name) = (self.pg_admin_url.clone(), self.db_name.clone());
        let (redis_url, prefix) = (self.redis_url.clone(), self.prefix.clone());
        let _ = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime to drop the database");
            runtime.block_on(async {
                if let Ok(mut admin) = sqlx::PgConnection::connect(&url).await {
                    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")).execute(&mut admin).await;
                }
                // A client of its own: the replicas' are bound to the test's
                // runtime, which may be gone. (`flush_keys` panics if Redis is
                // down; the join below swallows that.)
                if let Ok(shared) = SharedRedis::connect(&RedisSettings::from_url(&redis_url), &prefix).await {
                    flush_keys(&shared).await;
                }
            });
        })
        .join();
    }
}

/// `url` with its database replaced by `database`, or added when `url` names
/// none (`postgres://user:pw@host:port`).
fn url_with_database(url: &str, database: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (url, None),
    };
    // The path starts at the first slash after `scheme://` and the authority
    // (user, password, host and port, none of which holds a raw slash).
    let authority = base.find("://").map(|at| at + "://".len()).expect("LITEGEN_PG_TEST_URL is a URL");
    let path_at = base[authority..].find('/').map_or(base.len(), |slash| authority + slash);
    let mut replaced = format!("{}/{database}", &base[..path_at]);
    if let Some(query) = query {
        replaced.push('?');
        replaced.push_str(query);
    }
    replaced
}

#[test]
fn a_database_url_gets_its_database_replaced_or_added() {
    let cases = [
        // A path: the database it names is replaced.
        ("postgres://postgres:pw@127.0.0.1:55441/litegen_test", "postgres://postgres:pw@127.0.0.1:55441/scratch"),
        // No path: the slashes after the scheme are not one.
        ("postgres://postgres:pw@127.0.0.1:55441", "postgres://postgres:pw@127.0.0.1:55441/scratch"),
        ("postgres://postgres:pw@127.0.0.1:55441/", "postgres://postgres:pw@127.0.0.1:55441/scratch"),
        ("postgres://127.0.0.1", "postgres://127.0.0.1/scratch"),
        // A query stays, whether or not there was a path before it.
        ("postgres://u:pw@db:5432/litegen?sslmode=require", "postgres://u:pw@db:5432/scratch?sslmode=require"),
        ("postgres://u:pw@db:5432?sslmode=require", "postgres://u:pw@db:5432/scratch?sslmode=require"),
        ("postgres://u:pw@db:5432/x?sslrootcert=/etc/ca.pem", "postgres://u:pw@db:5432/scratch?sslrootcert=/etc/ca.pem"),
    ];
    for (url, expected) in cases {
        assert_eq!(url_with_database(url, "scratch"), expected, "{url}");
    }
}

impl Replica {
    async fn call(&self, method: &str, uri: &str, auth: Option<&str>, body: Option<Value>) -> Reply {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(token) = auth {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let (status, headers) = (resp.status(), resp.headers().clone());
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
        Reply { status, headers, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null) }
    }

    /// A new API key made with the master key: its id and its secret.
    async fn create_key(&self, body: Value) -> (String, String) {
        let made = self.call("POST", "/v1/keys", Some(MASTER), Some(body)).await;
        assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
        (made.body["id"].as_str().unwrap().to_string(), made.body["key"].as_str().unwrap().to_string())
    }

    /// `GET /v1/models` as `key`: a request that costs nothing and passes the
    /// key's rate limit.
    async fn list_models(&self, key: &str) -> Reply {
        self.call("GET", "/v1/models", Some(key), None).await
    }

    /// An image generation of `model`, as the holder of `token`.
    async fn generate(&self, model: &str, prompt: &str, token: &str) -> Reply {
        self.call("POST", "/v1/images/generations", Some(token), Some(json!({ "model": model, "prompt": prompt }))).await
    }

    /// What `X-Litegen-Cache` says of a `mock/image-gen` answer for `prompt`.
    async fn cache_header(&self, prompt: &str) -> String {
        let answer = self.generate("mock/image-gen", prompt, MASTER).await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
        answer.headers.get("x-litegen-cache").map(|v| v.to_str().unwrap().to_string()).unwrap_or_else(|| "absent".into())
    }

    async fn put_settings(&self, body: Value) {
        let put = self.call("PUT", "/v1/admin/settings", Some(MASTER), Some(body)).await;
        assert_eq!(put.status, StatusCode::OK, "{}", put.body);
    }

    /// The code of the error an answer carries.
    fn code(reply: &Reply) -> &str {
        reply.body["error"]["code"].as_str().unwrap_or_default()
    }
}

/// Probe until `done` accepts what the probe returned, or `within` passes;
/// returns the last answer either way, so an assertion can show it.
async fn until<T, Fut: Future<Output = T>>(within: Duration, mut probe: impl FnMut() -> Fut, done: impl Fn(&T) -> bool) -> T {
    let deadline = Instant::now() + within;
    loop {
        let value = probe().await;
        if done(&value) || Instant::now() >= deadline {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Wait until `listeners` listeners are subscribed to this world's event
/// channel. An event published before a listener has subscribed is lost, which
/// is how Redis pub/sub works, so a test that depends on an event being heard
/// starts only once both replicas listen. (The reload a listener runs when it
/// subscribes catches up on the settings and, because `refresh` runs
/// `sync_epoch` first, on the cache epoch too. So it can hide a lost event of
/// either kind, a lost cache-clear included.)
async fn wait_for_listeners(world: &World, listeners: i64) {
    let shared = world.redis().await;
    let channel = shared.key(&["events"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let channel = channel.clone();
        let counted: Result<(String, i64), _> =
            shared.run(|mut c| async move { redis::cmd("PUBSUB").arg("NUMSUB").arg(channel).query_async(&mut c).await }).await;
        let subscribed = counted.as_ref().map_or(0, |(_, n)| *n);
        if subscribed >= listeners {
            return;
        }
        assert!(Instant::now() < deadline, "{subscribed} of {listeners} listeners subscribed within 10 s ({counted:?})");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// How many answers the shared cache tier holds under this client's prefix.
async fn shared_cache_entries(shared: &SharedRedis) -> usize {
    let pattern = shared.key(&["cache", "e", "*"]);
    let keys: Vec<String> = shared.run(|mut c| async move { redis::cmd("KEYS").arg(pattern).query_async(&mut c).await }).await.expect("list the shared tier");
    keys.len()
}

// ─── 1. A key's per-minute limit is shared ──────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keys_rpm_is_shared_by_both_replicas() {
    let Some(mut world) = World::start().await else { return };
    let (a, b) = world.pair(ReplicaOptions::default, |_| {}).await;
    // B reads the key from the one database, so a key made on A works on B.
    let (_, key) = a.create_key(json!({ "name": "four a minute", "scopes": "generate,read", "rpm_limit": 4 })).await;

    for i in 0..4 {
        let (replica, name) = if i % 2 == 0 { (&a, "A") } else { (&b, "B") };
        let reply = replica.list_models(&key).await;
        assert_eq!(reply.status, StatusCode::OK, "request {} (on {name}) is within the limit: {}", i + 1, reply.body);
    }
    // Two on each: alone, a replica would still have two to give.
    let fifth = a.list_models(&key).await;
    assert_eq!(fifth.status, StatusCode::TOO_MANY_REQUESTS, "the fifth is over the key's 4, whichever replica it reaches: {}", fifth.body);
    let wait: u64 = fifth.headers.get("retry-after").expect("a refusal says when to retry").to_str().unwrap().parse().unwrap();
    assert!(wait >= 1, "retry-after is a whole number of seconds from now: {wait}");
    assert_eq!(Replica::code(&fifth), "rate_limit_exceeded");
    let sixth = b.list_models(&key).await;
    assert_eq!(sixth.status, StatusCode::TOO_MANY_REQUESTS, "and the other replica refuses it too: {}", sixth.body);
    assert!(sixth.headers.contains_key("retry-after"));
}

// ─── 2. A parallel cap is shared ────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_parallel_cap_holds_across_replicas() {
    let Some(mut world) = World::start().await else { return };
    // A request runs to its end even when its client hangs up (E12): A's
    // slow request ends when the vendor timeout cuts it.
    let (a, b) = world.pair(ReplicaOptions::default, |c| c.server.vendor_timeout_secs = Some(3)).await;
    let (key_id, key) = a.create_key(json!({ "name": "one at a time", "scopes": "generate,read" })).await;
    let limits = a.call("PUT", &format!("/v1/keys/{key_id}/limits"), Some(MASTER), Some(json!({ "max_parallel_requests": 1 }))).await;
    assert_eq!(limits.status, StatusCode::OK, "{}", limits.body);

    // A request that stays in flight on A: mock/slow-image answers after a day.
    let prompt = format!("a slow lighthouse {}", uuid::Uuid::new_v4());
    let slow = tokio::spawn({
        let app = a.app.clone();
        let request = Request::post("/v1/images/generations")
            .header("authorization", format!("Bearer {key}"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "model": "mock/slow-image", "prompt": prompt }).to_string()))
            .unwrap();
        async move { app.oneshot(request).await }
    });
    // The slot is taken before the vendor is called, so once the vendor has
    // the request, A holds the key's only slot.
    let reached = until(Duration::from_secs(10), || async { litegen::providers::image::mock::slow_image_calls(&prompt) }, |calls| *calls >= 1).await;
    assert_eq!(reached, 1, "the slow request reached the vendor on A");

    let refused = b.generate("mock/image-gen", "while A holds the slot", &key).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS, "B counts A's request against the cap: {}", refused.body);
    assert_eq!(Replica::code(&refused), "parallel_limit_exceeded", "{}", refused.body);

    // A's request ends (the vendor timed out): its slot goes back.
    let ended = slow.await.expect("A's request ran to its end").unwrap();
    assert_eq!(ended.status(), StatusCode::GATEWAY_TIMEOUT);
    let freed = until(SLACK, || b.generate("mock/image-gen", "after A's request ended", &key), |r| r.status == StatusCode::OK).await;
    assert_eq!(freed.status, StatusCode::OK, "B gets the slot once A's request has ended (waited up to {SLACK:?} for the release): {}", freed.body);
}

// ─── 3. Turning the cache off empties the shared tier ───────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_the_cache_off_on_a_empties_the_shared_tier() {
    let Some(mut world) = World::start().await else { return };
    // No pub/sub: B reads the change when the test says, so only the shared
    // tier decides what B answers.
    let (a, b) = world.pair(|| ReplicaOptions { subscribe: false }, |c| c.cache.enabled = true).await;
    let tier = world.redis().await;

    assert_eq!(a.cache_header("a heron").await, "miss");
    a.state.router.cache.flush_shared_writes().await;
    assert_eq!(shared_cache_entries(&tier).await, 1, "the answer is in the shared tier");

    a.put_settings(json!({ "cache_enabled": false })).await;
    assert_eq!(shared_cache_entries(&tier).await, 0, "turning the cache off emptied the shared tier");
    a.put_settings(json!({ "cache_enabled": true })).await;

    assert!(litegen::management::runtime::refresh(&b.state).await.unwrap(), "B applies the settings A saved");
    assert_eq!(b.cache_header("a heron").await, "miss", "the answer is gone from the shared tier, so B has none to hit");
    b.state.router.cache.flush_shared_writes().await;
    assert_eq!(a.cache_header("a heron").await, "hit", "and B's new answer is shared again");
}

// ─── 4. A model disabled on A is refused on B ───────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_model_disabled_on_a_is_refused_on_b_within_a_second() {
    // With pub/sub: an event wakes B's refresh.
    {
        let Some(mut world) = World::start().await else { return };
        let (a, b) = world.pair(ReplicaOptions::default, |_| {}).await;
        // So B hears A's event, not only the reload it runs on subscribing.
        wait_for_listeners(&world, 2).await;
        assert_eq!(b.generate("mock/image-gen", "before", MASTER).await.status, StatusCode::OK, "B serves the model");

        let patch = a.call("PATCH", "/v1/admin/models/mock/image-gen", Some(MASTER), Some(json!({ "disabled": true }))).await;
        assert_eq!(patch.status, StatusCode::OK, "{}", patch.body);
        let refused = until(SLACK, || b.generate("mock/image-gen", "after", MASTER), |r| r.status != StatusCode::OK).await;
        assert_eq!(refused.status, StatusCode::NOT_FOUND, "B refuses the model within about a second (polled up to {SLACK:?}): {}", refused.body);
        assert_eq!(Replica::code(&refused), "model_not_found");

        let patch = a.call("PATCH", "/v1/admin/models/mock/image-gen", Some(MASTER), Some(json!({ "disabled": false }))).await;
        assert_eq!(patch.status, StatusCode::OK, "{}", patch.body);
        let served = until(SLACK, || b.generate("mock/image-gen", "again", MASTER), |r| r.status == StatusCode::OK).await;
        assert_eq!(served.status, StatusCode::OK, "and serves it again within about a second (polled up to {SLACK:?}): {}", served.body);
    }

    // With pub/sub off: nothing reaches B until it refreshes, which the test
    // calls by hand (the loop's own tick is 15 s).
    {
        let Some(mut world) = World::start().await else { return };
        let (a, b) = world.pair(|| ReplicaOptions { subscribe: false }, |_| {}).await;
        let patch = a.call("PATCH", "/v1/admin/models/mock/image-gen", Some(MASTER), Some(json!({ "disabled": true }))).await;
        assert_eq!(patch.status, StatusCode::OK, "{}", patch.body);
        assert_eq!(b.generate("mock/image-gen", "not yet", MASTER).await.status, StatusCode::OK, "B has not heard: only its refresh will tell it");

        assert!(litegen::management::runtime::refresh(&b.state).await.unwrap(), "the refresh applies A's change");
        let refused = b.generate("mock/image-gen", "after the refresh", MASTER).await;
        assert_eq!((refused.status, Replica::code(&refused)), (StatusCode::NOT_FOUND, "model_not_found"), "{}", refused.body);
    }
}

// ─── 5. Each active job is polled by one replica per tick ───────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_active_job_is_polled_by_one_replica_per_tick() {
    use litegen::proxy::poller::poll_once_as;

    let Some(mut world) = World::start().await else { return };
    let (a, b) = world.pair(|| ReplicaOptions { subscribe: false }, |_| {}).await;

    // The mock 3D vendor counts its polls: a job's progress is a third of the
    // way per poll (33, then 66), and the third poll finishes it.
    let mut jobs = Vec::new();
    for n in 0..4 {
        let submitted = a.call("POST", "/v1/models3d/generations", Some(MASTER), Some(json!({ "model": "mock/mesh-3d", "prompt": format!("a teapot {n}") }))).await;
        assert!(submitted.status.is_success(), "{}", submitted.body);
        jobs.push(submitted.body["id"].as_str().unwrap().to_string());
    }
    let progress = |jobs: &[String]| {
        let (db, jobs) = (a.state.db.clone(), jobs.to_vec());
        async move {
            let mut seen = Vec::new();
            for id in &jobs {
                let row = db.get_generation(id).await.unwrap().expect("the job's row");
                seen.push((row.status.to_string(), row.progress));
            }
            seen
        }
    };
    let before = progress(&jobs).await;
    assert!(before.iter().all(|(_, p)| *p == 0), "nothing has polled them yet: {before:?}");

    // Both replicas run their tick at the same moment.
    for (tick, expected) in [(1, 33), (2, 66)] {
        tokio::join!(
            poll_once_as("replica-a", &a.state.db, &a.state.router.registry, None, Mode::SingleTenant, &a.state.router.model3d_store, None),
            poll_once_as("replica-b", &b.state.db, &b.state.router.registry, None, Mode::SingleTenant, &b.state.router.model3d_store, None),
        );
        let after = progress(&jobs).await;
        assert!(
            after.iter().all(|(_, p)| *p == expected),
            "tick {tick}: every job was polled exactly once (progress {expected}); a job both replicas polled would be further along: {after:?}"
        );
    }
}

// ─── 6. A link signed on A opens on B ───────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_link_signed_on_a_opens_on_b() {
    let Some(mut world) = World::start().await else { return };
    // No LITEGEN__SECRETS_KEY: each replica takes the signing key the database
    // holds, which the first to ask made.
    let (a, b) = world.pair(ReplicaOptions::default, |_| {}).await;
    a.state
        .db
        .put_app_file_access(&litegen::types::AppFileAccessUpsert {
            app_id: DEFAULT_APP_ID.into(),
            mode: Some("signed".into()),
            owner_scope: None,
            signed_url_ttl_seconds: Some(600),
            keep_outputs: Some(true),
            retention_days: None,
            signed_links: None,
        })
        .await
        .unwrap();

    // A generates and keeps a 3D model: a mock job needs three reads to finish.
    let submitted = a.call("POST", "/v1/models3d/generations", Some(MASTER), Some(json!({ "model": "mock/mesh-3d", "prompt": "a chair" }))).await;
    assert!(submitted.status.is_success(), "{}", submitted.body);
    let id = submitted.body["id"].as_str().unwrap().to_string();
    let mut done = Value::Null;
    for _ in 0..20 {
        done = a.call("GET", &format!("/v1/models3d/{id}"), Some(MASTER), None).await.body;
        if done["status"] == "completed" {
            break;
        }
        assert_ne!(done["status"], "failed", "{done}");
    }
    assert_eq!(done["status"], "completed", "{done}");
    let url = done["assets"].as_array().unwrap().iter().find(|a| a["kind"] == "mesh").expect("a mesh")["url"].as_str().unwrap().to_string();
    assert!(url.contains("signature="), "A hands out a signed link: {url}");

    // Through B, with no credentials at all: the signature is the only proof.
    let path = path_of(&url);
    let opened = b.call("GET", path, None, None).await;
    assert_eq!(opened.status, StatusCode::OK, "B verifies the link A signed: {path}");
    // And B is checking it: the stored URL without the signature is not served.
    let bare = path.split('?').next().unwrap();
    assert_eq!(b.call("GET", bare, None, None).await.status, StatusCode::NOT_FOUND, "an unsigned request is refused on B");
}

/// The path (and query) of an absolute URL litegen minted.
fn path_of(url: &str) -> &str {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    &after_scheme[after_scheme.find('/').unwrap_or(after_scheme.len())..]
}

// ─── 7. With Redis cut, requests keep going ─────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_redis_cut_requests_succeed_on_local_limits_and_fail_closed_refuses() {
    let Some(mut world) = World::start().await else { return };
    let proxy = CutProxy::start(addr_of(&world.redis_url)).await;
    // A falls back to its own counters; B refuses what it cannot verify. Both
    // reach Redis through the proxy, and try it again after one second.
    let recovery = |s: &mut RedisSettings| s.breaker_recovery = Duration::from_secs(1);
    let redis_a = world.redis_via(&proxy.url(), recovery).await;
    let redis_b = world.redis_via(&proxy.url(), |s| {
        recovery(s);
        s.fail_closed = true;
    })
    .await;
    let master = |c: &mut AppConfig| c.master_key = Some(MASTER.to_string());
    let a = world.replica(Some(redis_a.clone()), ReplicaOptions { subscribe: false }, master).await;
    let b = world.replica(Some(redis_b.clone()), ReplicaOptions { subscribe: false }, master).await;
    let (_, key) = a.create_key(json!({ "name": "four a minute", "scopes": "generate,read", "rpm_limit": 4 })).await;
    redis_a.ping().await.expect("Redis answers through the proxy");

    proxy.cut();
    // A: requests succeed, under its own limit of 4.
    for i in 0..4 {
        let reply = a.list_models(&key).await;
        assert_eq!(reply.status, StatusCode::OK, "request {} with Redis cut is served on A's local limits: {}", i + 1, reply.body);
    }
    let over = a.list_models(&key).await;
    assert_eq!(over.status, StatusCode::TOO_MANY_REQUESTS, "the local limit still holds: {}", over.body);
    assert!(over.headers.contains_key("retry-after"));
    // B: fail-closed refuses, with the fixed 503.
    let refused = b.list_models(&key).await;
    assert_eq!(refused.status, StatusCode::SERVICE_UNAVAILABLE, "{}", refused.body);
    assert_eq!(refused.headers.get("retry-after").map(|v| v.to_str().unwrap()), Some("1"));
    assert_eq!(refused.body["error"]["type"], "rate_limit_unverifiable", "{}", refused.body);
    assert_eq!(Replica::code(&refused), "rate_limit_unverifiable");
    assert_eq!(refused.body["error"]["message"], FAIL_CLOSED_MESSAGE, "the copy is fixed");

    // Redis returns; once each breaker has tried it again (a ping is that
    // try), the limits are shared once more. Only the shared bucket can make
    // this come out right: A's own is spent, and B has taken nothing locally.
    proxy.restore();
    for (name, redis) in [("A", &redis_a), ("B", &redis_b)] {
        let back = until(Duration::from_secs(10), || redis.ping(), |r| r.is_ok()).await;
        assert!(back.is_ok(), "{name}'s client reaches Redis again once its breaker's recovery has passed: {back:?}");
    }
    for i in 0..4 {
        let (replica, name) = if i % 2 == 0 { (&a, "A") } else { (&b, "B") };
        let reply = replica.list_models(&key).await;
        assert_eq!(reply.status, StatusCode::OK, "request {} (on {name}) is within the shared limit of 4: {}", i + 1, reply.body);
    }
    let fifth = a.list_models(&key).await;
    assert_eq!(fifth.status, StatusCode::TOO_MANY_REQUESTS, "the fifth is over the limit the two replicas share again: {}", fifth.body);
    assert_eq!(b.list_models(&key).await.status, StatusCode::TOO_MANY_REQUESTS);
}

// ─── 8. Clearing the cache clears every replica ─────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clearing_the_cache_on_a_clears_b_and_an_answer_cached_on_a_hits_on_b() {
    let Some(mut world) = World::start().await else { return };
    let (a, b) = world.pair(ReplicaOptions::default, |c| c.cache.enabled = true).await;
    // A clear announced before B has subscribed is lost to B.
    wait_for_listeners(&world, 2).await;

    assert_eq!(a.cache_header("a fox").await, "miss");
    a.state.router.cache.flush_shared_writes().await;
    assert_eq!(b.cache_header("a fox").await, "hit", "an answer cached on A is a hit on B");
    assert_eq!(b.state.router.cache.entry_count().await, 1, "B kept a copy in its own memory");

    let cleared = a.call("DELETE", "/v1/cache", Some(MASTER), None).await;
    assert_eq!(cleared.status, StatusCode::OK, "{}", cleared.body);
    // B's memory is emptied by the event, not by a request to it.
    let left = until(SLACK, || b.state.router.cache.entry_count(), |n| *n == 0).await;
    assert_eq!(left, 0, "B's own copy is dropped within about a second (polled up to {SLACK:?})");
    assert_eq!(b.cache_header("a fox").await, "miss", "and the shared tier is empty too: B has nothing to hit");
    b.state.router.cache.flush_shared_writes().await;
    assert_eq!(a.cache_header("a fox").await, "hit", "B's new answer is shared again");
}

// ─── 9. A failing provider stays cut off on both replicas ───────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_provider_that_keeps_failing_stays_cut_off_on_both_replicas() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let Some(mut world) = World::start().await else { return };
    // fal's endpoint, which always answers 503.
    let vendor = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(503).set_body_json(json!({ "detail": "upstream overloaded" }))).mount(&vendor).await;
    let hits = || async { vendor.received_requests().await.expect("the vendor records its requests").len() };

    let uri = vendor.uri();
    let configured = move |c: &mut AppConfig| {
        c.providers.insert("fal".into(), serde_json::from_value(json!({ "enabled": true, "api_key": "k", "api_base": uri })).unwrap());
        // The breaker is only in the path of a model route. Two failures open
        // it, for three seconds: long enough that a loaded machine does not
        // end the window between the checks below.
        c.model_routes = vec![serde_json::from_value(json!({
            "model": "fal/flux-dev",
            "deployments": [{ "provider": "fal", "max_retries": 0 }],
            "strategy": "fallback",
        }))
        .unwrap()];
        c.circuit_breaker.threshold = 2;
        c.circuit_breaker.open_for_seconds = 3;
    };
    let (a, b) = world.pair(|| ReplicaOptions { subscribe: false }, configured).await;
    async fn ask(replica: &Replica) -> Reply {
        replica.generate("fal/flux-dev", "a lighthouse", MASTER).await
    }

    // One failure on each replica: neither has two, the pair has.
    assert_eq!(ask(&a).await.status, StatusCode::BAD_GATEWAY, "A's first call reaches the failing vendor");
    assert_eq!(ask(&b).await.status, StatusCode::BAD_GATEWAY, "B's first call reaches it too");
    assert_eq!(hits().await, 2);
    assert!(a.state.router.circuit_breaker.is_open("fal").await && b.state.router.circuit_breaker.is_open("fal").await, "two failures between them opened the breaker on both");
    for (name, replica) in [("A", &a), ("B", &b)] {
        let skipped = ask(replica).await;
        assert!(!skipped.status.is_success(), "{name}: {}", skipped.body);
    }
    assert_eq!(hits().await, 2, "with the breaker open neither replica calls the vendor");

    // The window ends, and the next call is a probe.
    let closed = until(Duration::from_secs(10), || async { a.state.router.circuit_breaker.is_open("fal").await }, |open| !*open).await;
    assert!(!closed, "the breaker lets a probe through once its window is over");
    assert_eq!(ask(&a).await.status, StatusCode::BAD_GATEWAY, "A's probe reaches the vendor and fails");
    assert_eq!(hits().await, 3, "the probe was the only call");
    // The failed probe opened it again, for B too: B never calls the vendor.
    assert!(b.state.router.circuit_breaker.is_open("fal").await, "A's failed probe opened the breaker again for B");
    let skipped = ask(&b).await;
    assert!(!skipped.status.is_success(), "{}", skipped.body);
    assert_eq!(hits().await, 3, "B stays cut off: its own count was one failure, and it never called the vendor");
}
