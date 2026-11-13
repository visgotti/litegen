//! Gateway settings held in the database over `litegen.yaml`
//! (`docs/gateway-settings.md`), over the real route table: the cache, its
//! per-route and per-request controls, retries, the vendor timeout, the
//! circuit breaker, and replicas kept in step.

mod harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;

/// The full app (no auth: the dev bypass is the gateway admin) with `tweak`.
async fn app(tweak: impl FnOnce(&mut litegen::config::AppConfig)) -> Router {
    let dir = tempfile::tempdir().expect("asset dir").keep();
    harness::app_with(&dir, tweak).await.0
}

async fn send(app: &Router, method: &str, uri: &str, headers: &[(&str, &str)], body: Option<Value>) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// An image answer's `X-Litegen-Cache`.
async fn image(app: &Router, prompt: &str, headers: &[(&str, &str)]) -> String {
    let (status, h, body) = send(app, "POST", "/v1/images/generations", headers, Some(json!({ "model": "mock/image-gen", "prompt": prompt }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.get("x-litegen-cache").map(|v| v.to_str().unwrap().to_string()).unwrap_or_else(|| "absent".into())
}

async fn put_settings(app: &Router, body: Value) -> Value {
    let (status, _, view) = send(app, "PUT", "/v1/admin/settings", &[], Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    view
}

async fn put_routes(app: &Router, routes: Value) {
    let (status, _, body) = send(app, "PUT", "/v1/admin/routes", &[], Some(json!({ "routes": routes }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

// ─── The cache ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_cache_header_says_hit_miss_or_bypass() {
    let app_on = app(|c| c.cache.enabled = true).await;
    assert_eq!(image(&app_on, "a fox", &[]).await, "miss");
    assert_eq!(image(&app_on, "a fox", &[]).await, "hit");
    assert_eq!(image(&app_on, "another fox", &[]).await, "miss", "a different request");
    let app_off = app(|_| {}).await;
    assert_eq!(image(&app_off, "a fox", &[]).await, "bypass");
    assert_eq!(image(&app_off, "a fox", &[]).await, "bypass");
}

#[tokio::test]
async fn cache_control_no_cache_and_no_store_opt_out() {
    let app = app(|c| c.cache.enabled = true).await;
    // no-store: looked up, never kept.
    assert_eq!(image(&app, "a hare", &[("cache-control", "no-store")]).await, "miss");
    assert_eq!(image(&app, "a hare", &[]).await, "miss", "the no-store answer was not kept");
    assert_eq!(image(&app, "a hare", &[]).await, "hit");
    // no-cache: a fresh answer, which is kept.
    assert_eq!(image(&app, "a hare", &[("cache-control", "no-cache")]).await, "bypass");
    assert_eq!(image(&app, "a hare", &[]).await, "hit");
    // Both: the cache is not touched at all.
    assert_eq!(image(&app, "an owl", &[("cache-control", "No-Cache, no-store")]).await, "bypass");
    assert_eq!(image(&app, "an owl", &[]).await, "miss", "nothing was kept");
}

#[tokio::test]
async fn a_route_turns_the_cache_on_or_off_for_its_models() {
    // Off globally, on for the route.
    let app_on = app(|_| {}).await;
    put_routes(&app_on, json!([{ "model": "mock/image-gen", "deployments": [{ "provider": "mock" }], "cache_enabled": true, "cache_ttl_seconds": 60 }])).await;
    assert_eq!(image(&app_on, "a lynx", &[]).await, "miss");
    assert_eq!(image(&app_on, "a lynx", &[]).await, "hit");
    // On globally, off for the route.
    let app_off = app(|c| c.cache.enabled = true).await;
    put_routes(&app_off, json!([{ "model": "mock/image-gen", "deployments": [{ "provider": "mock" }], "cache_enabled": false }])).await;
    assert_eq!(image(&app_off, "a lynx", &[]).await, "bypass");
    assert_eq!(image(&app_off, "a lynx", &[]).await, "bypass");
}

#[tokio::test]
async fn a_routes_ttl_expires_its_entries() {
    let app = app(|c| c.cache.enabled = true).await;
    put_routes(&app, json!([{ "model": "mock/image-gen", "deployments": [{ "provider": "mock" }], "cache_enabled": true, "cache_ttl_seconds": 1 }])).await;
    assert_eq!(image(&app, "a wren", &[]).await, "miss");
    assert_eq!(image(&app, "a wren", &[]).await, "hit");
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
    assert_eq!(image(&app, "a wren", &[]).await, "miss", "expired after the route's 1 s, not the global hour");
}

#[tokio::test]
async fn turning_the_cache_off_live_empties_it_and_resizing_rebuilds_it() {
    let app = app(|_| {}).await;
    let view = put_settings(&app, json!({ "cache_enabled": true })).await;
    assert_eq!(view["effective_cache"], json!({ "enabled": true, "ttl_seconds": 3600, "max_items": 10000, "max_bytes": 268435456 }), "{view}");
    assert_eq!(image(&app, "a newt", &[]).await, "miss");
    assert_eq!(image(&app, "a newt", &[]).await, "hit");

    let view = put_settings(&app, json!({ "cache_enabled": false })).await;
    assert_eq!(view["cache_entries"], 0, "turned off, it is emptied: {view}");
    assert_eq!(image(&app, "a newt", &[]).await, "bypass");

    put_settings(&app, json!({ "cache_enabled": true })).await;
    assert_eq!(image(&app, "a newt", &[]).await, "miss", "nothing survived the switch-off");
    assert_eq!(image(&app, "a newt", &[]).await, "hit");
    let view = put_settings(&app, json!({ "cache_enabled": true, "cache_max_items": 5 })).await;
    assert_eq!(view["effective_cache"]["max_items"], 5, "{view}");
    assert_eq!(image(&app, "a newt", &[]).await, "miss", "a new size is a new, empty cache");
}

/// P7 (code review 2026-11-11): the local cache is bounded in memory by
/// default, not only in answers: a `b64_json` answer carries its image.
#[tokio::test]
async fn the_cache_is_bounded_in_bytes_by_default() {
    let app = app(|_| {}).await;
    let view = put_settings(&app, json!({ "cache_enabled": true })).await;
    assert_eq!(view["effective_cache"]["max_bytes"], 268_435_456, "{view}");
}

/// P7: `cache_max_bytes` is held to its range, 1 MiB to 64 GiB, as the
/// other cache settings are.
#[tokio::test]
async fn a_cache_max_bytes_outside_its_range_is_refused() {
    let app = app(|_| {}).await;
    for bad in [json!(1024), json!(68_719_476_737u64)] {
        let (status, _, body) = send(&app, "PUT", "/v1/admin/settings", &[], Some(json!({ "cache_max_bytes": bad }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "cache_max_bytes {bad}: {body}");
    }
    let view = put_settings(&app, json!({ "cache_enabled": true, "cache_max_bytes": 1_048_576 })).await;
    assert_eq!(view["effective_cache"]["max_bytes"], 1_048_576, "{view}");
}

/// P7: a new `cache_max_bytes` is a new, empty cache, as a new `max_items` is.
#[tokio::test]
async fn changing_cache_max_bytes_live_rebuilds_the_cache() {
    let app = app(|c| c.cache.enabled = true).await;
    assert_eq!(image(&app, "a toad", &[]).await, "miss");
    assert_eq!(image(&app, "a toad", &[]).await, "hit");
    put_settings(&app, json!({ "cache_max_bytes": 8_388_608 })).await;
    assert_eq!(image(&app, "a toad", &[]).await, "miss", "a new byte budget is a new, empty cache");
    assert_eq!(image(&app, "a toad", &[]).await, "hit");
}

#[tokio::test]
async fn the_settings_view_counts_what_the_cache_holds() {
    let app = app(|c| c.cache.enabled = true).await;
    image(&app, "a vole", &[]).await;
    image(&app, "a mole", &[]).await;
    let (_, _, view) = send(&app, "GET", "/v1/admin/settings", &[], None).await;
    assert_eq!(view["cache_entries"], 2, "{view}");
    let (_, _, health) = send(&app, "GET", "/health", &[], None).await;
    assert_eq!(health["cache"], json!({ "enabled": true, "entries": 2 }), "{health}");
}

#[tokio::test]
async fn cache_counters_reach_metrics() {
    litegen::api::metrics::init_prometheus();
    let app = app(|c| c.cache.enabled = true).await;
    image(&app, "a crane", &[]).await;
    image(&app, "a crane", &[]).await;
    let (status, _, _) = send(&app, "GET", "/metrics", &[], None).await;
    assert_eq!(status, StatusCode::OK);
    let resp = app.clone().oneshot(Request::get("/metrics").body(Body::empty()).unwrap()).await.unwrap();
    let text = String::from_utf8(axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
    assert!(text.contains(r#"litegen_cache_requests_total{result="hit"}"#), "{text}");
    assert!(text.contains(r#"litegen_cache_requests_total{result="miss"}"#), "{text}");
}

// ─── The metrics endpoint's settings (S7) ───────────────────────────────────

#[tokio::test]
async fn metrics_can_be_switched_off() {
    let app = app(|c| c.observability.metrics_enabled = false).await;
    let (status, _, _) = send(&app, "GET", "/metrics", &[], None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "observability.metrics_enabled: false still served /metrics");
}

#[tokio::test]
async fn metrics_are_served_at_the_configured_path() {
    litegen::api::metrics::init_prometheus();
    let app = app(|c| c.observability.metrics_path = "/internal/metrics".into()).await;
    let (moved, _, _) = send(&app, "GET", "/internal/metrics", &[], None).await;
    let (old, _, _) = send(&app, "GET", "/metrics", &[], None).await;
    assert_eq!((moved, old), (StatusCode::OK, StatusCode::NOT_FOUND), "GET /internal/metrics, then GET /metrics");
}

/// With `metrics_require_auth`, the metrics need a credential that may read
/// the deployment: in single-tenant mode, any key with `read` acting in the
/// deployment's org, and the master key.
#[tokio::test]
async fn metrics_can_require_a_key() {
    use litegen::db::sqlite::SqliteDatabase;
    let db = std::sync::Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let dir = tempfile::tempdir().unwrap();
    let app = harness::app_over_with(
        db,
        dir.path(),
        |c| c.observability.metrics_require_auth = true,
        Some("test-master"),
        litegen::config::Mode::SingleTenant,
    )
    .await;
    let master = [("authorization", "Bearer test-master")];
    let key = |scopes: &'static str| {
        let app = app.clone();
        async move {
            let (status, _, created) = send(&app, "POST", "/v1/keys", &master, Some(json!({ "name": format!("metrics-{scopes}"), "scopes": scopes }))).await;
            assert_eq!(status, StatusCode::CREATED, "{created}");
            format!("Bearer {}", created["key"].as_str().unwrap())
        }
    };
    let (reader, generator) = (key("read").await, key("generate").await);
    let (anonymous, _, _) = send(&app, "GET", "/metrics", &[], None).await;
    let (as_master, _, _) = send(&app, "GET", "/metrics", &master, None).await;
    let (as_reader, _, _) = send(&app, "GET", "/metrics", &[("authorization", reader.as_str())], None).await;
    let (as_generator, _, _) = send(&app, "GET", "/metrics", &[("authorization", generator.as_str())], None).await;
    assert_eq!(
        (anonymous, as_master, as_reader, as_generator),
        (StatusCode::UNAUTHORIZED, StatusCode::OK, StatusCode::OK, StatusCode::FORBIDDEN),
        "no credential, the master key, a read key, a generate-only key"
    );
}

/// The credential `metrics_require_auth` asks for guards the metrics route
/// only: a path litegen doesn't serve stays a plain 404 for every caller, as
/// with the setting off, whether MCP is on or off (review M1).
#[tokio::test]
async fn requiring_a_key_for_metrics_leaves_unknown_paths_a_plain_404() {
    use litegen::db::sqlite::SqliteDatabase;
    for mcp in [false, true] {
        let db = std::sync::Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let dir = tempfile::tempdir().unwrap();
        let app = harness::app_over_with(
            db,
            dir.path(),
            move |c| {
                c.observability.metrics_require_auth = true;
                c.mcp.enabled = mcp;
            },
            Some("test-master"),
            litegen::config::Mode::SingleTenant,
        )
        .await;
        let master = [("authorization", "Bearer test-master")];
        let (status, _, created) = send(&app, "POST", "/v1/keys", &master, Some(json!({ "name": "unknown-paths", "scopes": "generate" }))).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let generator = format!("Bearer {}", created["key"].as_str().unwrap());
        for path in ["/no-such-path", "/v1/no-such-path"] {
            let (anonymous, _, _) = send(&app, "GET", path, &[], None).await;
            let (as_generator, _, _) = send(&app, "GET", path, &[("authorization", generator.as_str())], None).await;
            assert_eq!(
                (anonymous, as_generator),
                (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND),
                "GET {path} with no credential, then with a generate-only key (mcp.enabled: {mcp}): the metrics route's auth reached a path litegen doesn't serve"
            );
        }
        let (metrics, _, _) = send(&app, "GET", "/metrics", &[], None).await;
        assert_eq!(metrics, StatusCode::UNAUTHORIZED, "the metrics route itself still needs a credential (mcp.enabled: {mcp})");
    }
}

// ─── Retries ────────────────────────────────────────────────────────────────

/// `mock/rate-limited-image` answers its first call per prompt with a 429
/// whose `Retry-After` the prompt names (`wait=<ms>`), then an image.
async fn rate_limited(app: &Router, prompt: &str) -> (StatusCode, std::time::Duration, u32, Value) {
    let before = litegen::providers::image::mock::rate_limited_calls(prompt);
    let started = std::time::Instant::now();
    let (status, _, body) = send(app, "POST", "/v1/images/generations", &[], Some(json!({ "model": "mock/rate-limited-image", "prompt": prompt }))).await;
    (status, started.elapsed(), litegen::providers::image::mock::rate_limited_calls(prompt) - before, body)
}

#[tokio::test]
async fn a_rate_limited_call_is_retried_after_its_retry_after() {
    let app = app(|_| {}).await;
    let (status, took, calls, body) = rate_limited(&app, "wait=300 a heron").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(calls, 2, "refused once, then answered");
    assert!(took >= std::time::Duration::from_millis(300), "waited the vendor's Retry-After: {took:?}");
}

#[tokio::test]
async fn no_retries_allowed_means_the_first_refusal_stands() {
    let app = app(|c| c.retries.max_retries = 0).await;
    let (status, _, calls, body) = rate_limited(&app, "wait=50 a stork").await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(calls, 1);
    assert!(body.to_string().contains("429"), "the vendor's refusal reaches the caller: {body}");
}

#[tokio::test]
async fn a_retry_after_past_the_ceiling_is_not_waited_for() {
    let app = app(|_| {}).await;
    put_settings(&app, json!({ "retry_max_backoff_ms": 1000 })).await;
    let (status, took, calls, body) = rate_limited(&app, "wait=5000 an ibis").await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(calls, 1, "not retried");
    assert!(took < std::time::Duration::from_secs(2), "the request did not hold for 5 s: {took:?}");
}

// ─── The vendor timeout and the breaker, live ───────────────────────────────

#[tokio::test]
async fn the_vendor_timeout_changes_live() {
    let app = app(|_| {}).await;
    let view = put_settings(&app, json!({ "vendor_timeout_seconds": 5, "circuit_breaker_threshold": 9, "circuit_breaker_open_for_seconds": 7 })).await;
    assert_eq!(view["effective_vendor_timeout_seconds"], 5, "{view}");
    assert_eq!(view["effective_circuit_breaker"], json!({ "threshold": 9, "open_for_seconds": 7 }), "{view}");
    // mock/slow-image answers after a day: litegen gives up at the new 5 s.
    let started = std::time::Instant::now();
    let (status, _, body) = send(&app, "POST", "/v1/images/generations", &[], Some(json!({ "model": "mock/slow-image", "prompt": "a sloth, live timeout" }))).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "{body}");
    let took = started.elapsed();
    assert!(took >= std::time::Duration::from_secs(5) && took < std::time::Duration::from_secs(9), "{took:?}");
}

// ─── Every replica in step ──────────────────────────────────────────────────

#[tokio::test]
async fn a_setting_saved_on_one_instance_reaches_another_after_a_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}/litegen.db?mode=rwc", dir.path().display());
    let open = || async { std::sync::Arc::new(litegen::db::sqlite::SqliteDatabase::connect(&url).await.expect("file sqlite")) };
    let (a, _) = harness::app_and_state_over(open().await, &dir.path().join("assets-a")).await;
    let (b, b_state) = harness::app_and_state_over(open().await, &dir.path().join("assets-b")).await;

    put_settings(&a, json!({ "cache_enabled": true, "retry_max_retries": 4 })).await;
    let (_, _, before) = send(&b, "GET", "/v1/admin/settings", &[], None).await;
    assert_eq!(before["effective_cache"]["enabled"], false, "B has not looked yet: {before}");

    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap(), "B found a change");
    let (_, _, after) = send(&b, "GET", "/v1/admin/settings", &[], None).await;
    assert_eq!(after["effective_cache"]["enabled"], true, "{after}");
    assert_eq!(after["effective_retries"]["max_retries"], 4, "{after}");
    assert_eq!(image(&b, "a gull", &[]).await, "miss", "B caches now");
    assert!(!litegen::management::runtime::refresh(&b_state).await.unwrap(), "nothing new the second time");

    // A runtime route saved on A reaches B the same way.
    put_routes(&a, json!([{ "model": "mock/image-gen", "deployments": [{ "provider": "mock" }], "cache_enabled": false }])).await;
    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap());
    assert_eq!(image(&b, "a gull", &[]).await, "bypass", "B follows A's route");
}

// Two instances over one database file: two replicas of one deployment. Both
// hold the same secrets key, so a provider key saved on A decrypts on B.
async fn two_instances() -> (Router, std::sync::Arc<litegen::api::middleware::AppState>, Router, std::sync::Arc<litegen::api::middleware::AppState>) {
    let dir = tempfile::tempdir().unwrap().keep();
    let url = format!("sqlite://{}/litegen.db?mode=rwc", dir.display());
    let open = || async { std::sync::Arc::new(litegen::db::sqlite::SqliteDatabase::connect(&url).await.expect("file sqlite")) };
    let keyed = |c: &mut litegen::config::AppConfig| {
        c.secrets_key = Some(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [5u8; 32]));
    };
    let (a, a_state) = harness::app_and_state_over_with(open().await, &dir.join("assets-a"), keyed).await;
    let (b, b_state) = harness::app_and_state_over_with(open().await, &dir.join("assets-b"), keyed).await;
    (a, a_state, b, b_state)
}

/// A model that passes the runtime-model rules: the mock adapter's image model.
fn runtime_model_json(id: &str) -> Value {
    json!({
        "id": id,
        "provider": "mock",
        "media_type": "image",
        "display_name": "Runtime Mock",
        "description": "Created through the API",
        "pricing": { "base_cost_usd": 2.0 },
        "capabilities": { "text_to_image": true },
        "prompt": { "required": true, "max_length": 500 }
    })
}

#[tokio::test]
async fn a_runtime_model_saved_on_one_instance_reaches_another_after_a_refresh() {
    let (a, _, b, b_state) = two_instances().await;
    let (s, _, body) = send(&a, "POST", "/v1/admin/models", &[], Some(runtime_model_json("mock/runtime-one"))).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    assert_eq!(send(&b, "GET", "/v1/models/mock/runtime-one", &[], None).await.0, StatusCode::NOT_FOUND, "B hasn't looked");
    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap());
    assert_eq!(send(&b, "GET", "/v1/models/mock/runtime-one", &[], None).await.0, StatusCode::OK);
    assert!(!litegen::management::runtime::refresh(&b_state).await.unwrap(), "nothing new the second time");
    send(&a, "DELETE", "/v1/admin/models/mock/runtime-one", &[], None).await;
    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap());
    assert_eq!(send(&b, "GET", "/v1/models/mock/runtime-one", &[], None).await.0, StatusCode::NOT_FOUND, "gone from B too");
}

#[tokio::test]
async fn a_model_disabled_on_one_instance_is_refused_on_another() {
    let (a, _, b, b_state) = two_instances().await;
    send(&a, "PATCH", "/v1/admin/models/mock/image-gen", &[], Some(json!({ "disabled": true }))).await;
    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap());
    let (s, _, _) = send(&b, "POST", "/v1/images/generations", &[], Some(json!({ "model": "mock/image-gen", "prompt": "x" }))).await;
    assert_ne!(s, StatusCode::OK, "disabled on B");
    send(&a, "PATCH", "/v1/admin/models/mock/image-gen", &[], Some(json!({ "disabled": false }))).await;
    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap());
    let (s, _, body) = send(&b, "POST", "/v1/images/generations", &[], Some(json!({ "model": "mock/image-gen", "prompt": "x" }))).await;
    assert_eq!(s, StatusCode::OK, "enabled again on B: {body}");
}

#[tokio::test]
async fn a_provider_config_saved_on_one_instance_reaches_another() {
    let (a, _, b, b_state) = two_instances().await;
    let (s, _, put) = send(&a, "PUT", "/v1/admin/providers/openai", &[], Some(json!({ "api_key": "sk-test-123456" }))).await;
    assert_eq!(s, StatusCode::OK, "{put}");
    let (_, _, before) = send(&b, "GET", "/v1/admin/providers/openai", &[], None).await;
    assert_eq!(before["active"], false, "B hasn't looked: {before}");
    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap());
    let (_, _, view) = send(&b, "GET", "/v1/admin/providers/openai", &[], None).await;
    assert_eq!(view["active"], true, "{view}");
    assert!(!litegen::management::runtime::refresh(&b_state).await.unwrap(), "nothing new the second time");
    // Dropping the config on A takes the provider out of service on B too.
    send(&a, "DELETE", "/v1/admin/providers/openai", &[], None).await;
    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap());
    let (_, _, view) = send(&b, "GET", "/v1/admin/providers/openai", &[], None).await;
    assert_eq!(view["active"], false, "{view}");
}

#[tokio::test]
async fn a_failed_refresh_keeps_the_settings() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}/litegen.db?mode=rwc", dir.path().display());
    let db = std::sync::Arc::new(litegen::db::sqlite::SqliteDatabase::connect(&url).await.expect("file sqlite"));
    let (app, state) = harness::app_and_state_over(db, &dir.path().join("assets")).await;
    put_settings(&app, json!({ "cache_enabled": true })).await;
    // The settings table gone: every read of it fails.
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    sqlx::query("DROP TABLE gateway_settings").execute(&pool).await.unwrap();
    assert!(litegen::management::runtime::refresh(&state).await.is_err());
    let (_, _, view) = send(&app, "GET", "/v1/admin/settings", &[], None).await;
    assert_eq!(view["effective_cache"]["enabled"], true, "kept: {view}");
}


// ─── The cache's shared tier ────────────────────────────────────────────────

/// Two replicas over one database whose caches share a Redis (the harness's
/// routers have none of their own, so the tier goes onto each cache). Skipped
/// without `LITEGEN_REDIS_TEST_URL`.
#[tokio::test]
async fn turning_the_cache_off_empties_the_shared_tier_and_every_replicas_memory() {
    let Some((shared_a, shared_b)) = litegen::shared::testing::two_shared().await else { return };
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}/litegen.db?mode=rwc", dir.path().display());
    let open = || async { std::sync::Arc::new(litegen::db::sqlite::SqliteDatabase::connect(&url).await.expect("file sqlite")) };
    let cached = |c: &mut litegen::config::AppConfig| c.cache.enabled = true;
    let (a, a_state) = harness::app_and_state_over_with(open().await, &dir.path().join("assets-a"), cached).await;
    let (b, b_state) = harness::app_and_state_over_with(open().await, &dir.path().join("assets-b"), cached).await;
    a_state.router.cache.attach_shared(shared_a);
    b_state.router.cache.attach_shared(shared_b);

    assert_eq!(image(&a, "a heron", &[]).await, "miss");
    a_state.router.cache.flush_shared_writes().await;
    assert_eq!(image(&b, "a heron", &[]).await, "hit", "B reads A's answer from the shared tier");
    assert_eq!(b_state.router.cache.entry_count().await, 1, "and keeps a copy");

    put_settings(&a, json!({ "cache_enabled": false })).await;
    // Before B has applied the change, only the epoch can tell it.
    b_state.router.cache.sync_epoch().await;
    assert_eq!(b_state.router.cache.entry_count().await, 0, "B's memory is emptied by the epoch");
    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap(), "B applies the change");
    assert_eq!(image(&b, "a heron", &[]).await, "bypass");

    // Back on: nothing the shared tier held before the switch-off is left.
    put_settings(&a, json!({ "cache_enabled": true })).await;
    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap());
    assert_eq!(image(&b, "a heron", &[]).await, "miss", "the shared tier was emptied with the cache");
    b_state.router.cache.flush_shared_writes().await;
    assert_eq!(image(&a, "a heron", &[]).await, "hit", "and B's new answer is shared again");
}
