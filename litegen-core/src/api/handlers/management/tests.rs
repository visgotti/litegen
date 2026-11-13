//! Management API handler tests: the real router over an in-memory SQLite
//! database, hosted mode, with the mock provider registered. Each test drives
//! the HTTP surface end to end — including the generation endpoints, so the
//! enforcement hooks in `generate_*` are covered too.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

use crate::api::handlers::create_router;
use crate::api::middleware::{AppState, DEFAULT_APP_ID, DEFAULT_ORG_ID};
use crate::auth::tokens::{generate_csrf_token, generate_session_token};
use crate::capabilities::CapabilityRegistry;
use crate::config::{AppConfig, CacheGlobalConfig, DevFlags, Mode};
use crate::db::sqlite::SqliteDatabase;
use crate::db::DatabaseStore;
use crate::management::enforce::ParallelSlot;
use crate::management::types::*;
use crate::proxy::cache::GenerationCache;
use crate::proxy::materializer::{MaterializeError, Materializer, TempStorage};
use crate::proxy::registry::ProviderRegistry;
use crate::proxy::router::ProxyRouter;
use crate::proxy::storage::LocalStore;
use crate::types::{Application, Organization, RequestArtifact, Role, Session, User};

const MASTER: &str = "master-secret-for-tests";
const EXPENSIVE: &str = "mock/expensive-image"; // $5.00 per request
const FREE: &str = "mock/image-gen"; // $0.00

struct NoopStorage;
#[async_trait::async_trait]
impl TempStorage for NoopStorage {
    async fn put(&self, key: &str, _bytes: bytes::Bytes, _ct: &str) -> Result<String, MaterializeError> {
        Ok(format!("local://{key}"))
    }
    async fn delete(&self, _key: &str) -> Result<(), MaterializeError> {
        Ok(())
    }
}

async fn build_state(db: Arc<SqliteDatabase>, mode: Mode, secrets_key: Option<[u8; 32]>) -> Arc<AppState> {
    build_state_with(db, mode, secrets_key, None).await
}

/// [`build_state`] for a replica that shares a Redis with the others.
async fn build_state_with(
    db: Arc<SqliteDatabase>,
    mode: Mode,
    secrets_key: Option<[u8; 32]>,
    shared: Option<Arc<crate::shared::SharedRedis>>,
) -> Arc<AppState> {
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .configure_provider("mock", crate::providers::ProviderInstanceConfig::default())
        .await;
    // As `tests/harness/mod.rs` does: the mock provider is configured, not only
    // registered, so a refresh has nothing to say about it.
    let mut app_config = AppConfig::default();
    app_config.providers.insert("mock".to_string(), serde_json::from_value(json!({ "enabled": true })).expect("mock provider config"));
    let config = Arc::new(app_config);
    let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
    let router = ProxyRouter::new(registry, cache, config, Arc::new(LocalStore));
    let router = Arc::new(match shared {
        Some(shared) => router.with_shared(shared),
        None => router,
    });
    let materializer = Arc::new(Materializer::new(Arc::new(NoopStorage), reqwest::Client::new()));
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.push("models");
    Arc::new(AppState {
        router,
        db,
        master_key: Some(MASTER.to_string()),
        registry: Arc::new(CapabilityRegistry::from_dir(&p).expect("models")),
        materializer,
        rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
        in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),
        oauth: crate::auth::oauth::OAuthConfig::default(),
        mode,
        secrets_key,
        dev: DevFlags::default(),
        allow_password: true,
    })
}

struct Sess {
    user: User,
    token: String,
    csrf: String,
}

async fn seed_user(db: &SqliteDatabase, email: &str) -> Sess {
    let user = User {
        id: format!("user-{}", Uuid::new_v4()),
        email: email.to_string(),
        password_hash: None,
        role: Role::Member,
        oauth_github_id: None,
        oauth_google_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        last_login_at: None,
        is_active: true,
    };
    db.create_user(&user).await.unwrap();
    let token = generate_session_token();
    let csrf = generate_csrf_token();
    db.create_session(&Session {
        id: token.clone(),
        user_id: user.id.clone(),
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::days(1),
        ip: None,
        user_agent: None,
        csrf_token: csrf.clone(),
    })
    .await
    .unwrap();
    Sess { user, token, csrf }
}

struct Key {
    raw: String,
    id: Uuid,
}

async fn seed_key(db: &SqliteDatabase, org: &str, app: &str, scopes: &str, owner: Option<&str>) -> Key {
    use sha2::{Digest, Sha256};
    let raw = format!("sk_test_{}", Uuid::new_v4().simple());
    let hash = hex::encode(Sha256::digest(raw.as_bytes()));
    let k = db
        .create_api_key_scoped(org, app, &format!("pk_{}", Uuid::new_v4().simple()), "test key", &hash, &raw[..12], None, None, scopes, None, None)
        .await
        .unwrap();
    if let Some(o) = owner {
        db.set_api_key_owner(&k.id, o).await.unwrap();
    }
    Key { raw, id: k.id }
}

async fn seed_org(db: &SqliteDatabase, name: &str) -> (String, String) {
    let now = chrono::Utc::now();
    let org = Organization {
        id: format!("org-{}", Uuid::new_v4()),
        name: name.into(),
        slug: format!("{}-{}", name.to_lowercase(), Uuid::new_v4().simple()),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_organization(&org).await.unwrap();
    let app = Application {
        id: format!("app-{}", Uuid::new_v4()),
        org_id: org.id.clone(),
        name: "Default".into(),
        slug: format!("default-{}", Uuid::new_v4().simple()),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_application(&app).await.unwrap();
    (org.id, app.id)
}

/// Org + app, an owner session, an admin-scope key owned by the owner, and a
/// generate/read key with no owner.
struct Fx {
    app: Router,
    db: Arc<SqliteDatabase>,
    state: Arc<AppState>,
    org: String,
    app_id: String,
    owner: Sess,
    admin_key: Key,
    gen_key: Key,
}

async fn fixture_with(mode: Mode, secrets: bool) -> Fx {
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let state = build_state(db.clone(), mode, secrets.then_some([7u8; 32])).await;
    let (org, app_id) = seed_org(&db, "Acme").await;
    let owner = seed_user(&db, "owner@acme.test").await;
    db.add_org_member(&org, &owner.user.id, Role::Owner).await.unwrap();
    let admin_key = seed_key(&db, &org, &app_id, "generate,read,admin", Some(&owner.user.id)).await;
    let gen_key = seed_key(&db, &org, &app_id, "generate,read", None).await;
    Fx { app: create_router(state.clone()), db, state, org, app_id, owner, admin_key, gen_key }
}

async fn fixture() -> Fx {
    fixture_with(Mode::Hosted, false).await
}

enum Auth<'a> {
    Bearer(&'a str),
    Session(&'a Sess, Option<&'a str>),
    Anon,
}

async fn send(app: &Router, method: &str, uri: &str, auth: Auth<'_>, body: Option<Value>, headers: &[(&str, &str)]) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    match auth {
        Auth::Bearer(k) => b = b.header("authorization", format!("Bearer {k}")),
        Auth::Session(s, org) => {
            b = b.header("cookie", format!("litegen_session={}", s.token)).header("x-csrf-token", &s.csrf);
            if let Some(o) = org {
                b = b.header("x-litegen-org-id", o);
            }
        }
        Auth::Anon => {}
    }
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let req = match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into())) };
    (status, v)
}

async fn generate(app: &Router, key: &str, model: &str, headers: &[(&str, &str)]) -> (StatusCode, Value) {
    send(app, "POST", "/v1/images/generations", Auth::Bearer(key), Some(json!({ "model": model, "prompt": "a cat" })), headers).await
}

/// Request logs are written off the response path; wait for `n` attributed rows.
async fn wait_for_logs(fx: &Fx, n: u64) {
    let store = fx.db.management().unwrap();
    let filter = SpendFilter { org_id: Some(fx.org.clone()), ..Default::default() };
    for _ in 0..200 {
        let (rows, total) = store.spend_logs(&filter, 1, 500).await.unwrap();
        let attributed = rows.iter().filter(|r| r.key_id.is_some() || r.user_id.is_some()).count() as u64;
        if total >= n && attributed >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("request logs never reached {n}");
}

fn err_type(v: &Value) -> &str {
    v["error"]["type"].as_str().unwrap_or("")
}

// ─── Budgets: enforcement on every principal ────────────────────────────────

#[tokio::test]
async fn org_budget_refuses_the_request_that_would_exceed_it() {
    let fx = fixture().await;
    let (s, l) = send(&fx.app, "PUT", &format!("/v1/orgs/{}/limits", fx.org), Auth::Session(&fx.owner, Some(&fx.org)), Some(json!({ "max_budget_usd": 7.0 })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{l}");
    assert_eq!(l["entity_type"], "org");
    assert_eq!(l["spend_usd"], 0.0);

    let (s, body) = generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (s, body) = generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await;
    assert_eq!(s, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(err_type(&body), "budget_exceeded");
    assert!(body["error"]["message"].as_str().unwrap().contains("org budget exceeded"));

    // The refused request was rolled back: spend is exactly the first request.
    let (_, l) = send(&fx.app, "GET", &format!("/v1/orgs/{}/limits", fx.org), Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!(l["spend_usd"], 5.0);
    // Free models still go through.
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, FREE, &[]).await.0, StatusCode::OK);
}

#[tokio::test]
async fn app_key_and_member_budgets_each_apply() {
    let fx = fixture().await;
    // App budget.
    send(&fx.app, "PUT", &format!("/v1/apps/{}/limits", fx.app_id), Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_budget_usd": 4.0 })), &[]).await;
    let (s, b) = generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await;
    assert_eq!((s, err_type(&b)), (StatusCode::PAYMENT_REQUIRED, "budget_exceeded"));
    assert!(b["error"]["message"].as_str().unwrap().contains("app"));
    send(&fx.app, "DELETE", &format!("/v1/apps/{}/limits", fx.app_id), Auth::Bearer(&fx.admin_key.raw), None, &[]).await;

    // Key budget.
    let (s, l) = send(&fx.app, "PUT", &format!("/v1/keys/{}/limits", fx.gen_key.id), Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_budget_usd": 1.0 })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{l}");
    let (s, b) = generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await;
    assert_eq!((s, err_type(&b)), (StatusCode::PAYMENT_REQUIRED, "budget_exceeded"));
    assert!(b["error"]["message"].as_str().unwrap().contains("key"));

    // Member budget covers the keys the member owns.
    let (s, _) = send(
        &fx.app, "PUT", &format!("/v1/orgs/{}/members/{}/limits", fx.org, fx.owner.user.id),
        Auth::Session(&fx.owner, Some(&fx.org)), Some(json!({ "max_budget_usd": 2.0 })), &[],
    ).await;
    assert_eq!(s, StatusCode::OK);
    let (s, b) = generate(&fx.app, &fx.admin_key.raw, EXPENSIVE, &[]).await;
    assert_eq!((s, err_type(&b)), (StatusCode::PAYMENT_REQUIRED, "budget_exceeded"));
    assert!(b["error"]["message"].as_str().unwrap().contains("member"));
}

#[tokio::test]
async fn a_refusal_by_one_principal_releases_every_other_reservation() {
    let fx = fixture().await;
    // Org tracks spend (no cap); the app refuses.
    send(&fx.app, "PUT", &format!("/v1/orgs/{}/limits", fx.org), Auth::Bearer(&fx.admin_key.raw), Some(json!({ "soft_budget_usd": 100.0 })), &[]).await;
    send(&fx.app, "PUT", &format!("/v1/apps/{}/limits", fx.app_id), Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_budget_usd": 1.0 })), &[]).await;
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await.0, StatusCode::PAYMENT_REQUIRED);
    let (_, l) = send(&fx.app, "GET", &format!("/v1/orgs/{}/limits", fx.org), Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!(l["spend_usd"], 0.0, "org reservation must be refunded: {l}");
}

/// L18: a cancelled async job gives what it was charged back to every
/// principal it was charged against, long after the request's guard is gone.
#[tokio::test]
async fn a_cancelled_job_refunds_every_principal_it_was_charged_against() {
    let fx = fixture().await;
    let org_limits = format!("/v1/orgs/{}/limits", fx.org);
    let key_limits = format!("/v1/keys/{}/limits", fx.gen_key.id);
    for path in [&org_limits, &key_limits] {
        let (s, l) = send(&fx.app, "PUT", path, Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_budget_usd": 10.0 })), &[]).await;
        assert_eq!(s, StatusCode::OK, "{l}");
    }
    let (s, job) = send(&fx.app, "POST", "/v1/videos/generations", Auth::Bearer(&fx.gen_key.raw), Some(json!({ "model": "mock/priced-video", "prompt": "a cat" })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{job}");
    let spend = |l: Value| l["spend_usd"].as_f64().unwrap();
    for path in [&org_limits, &key_limits] {
        let (_, l) = send(&fx.app, "GET", path, Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
        assert_eq!(spend(l), 0.5, "charged at submit: {path}");
    }

    let id = job["id"].as_str().unwrap();
    let (s, cancelled) = send(&fx.app, "PATCH", &format!("/v1/generations/{id}"), Auth::Bearer(&fx.gen_key.raw), Some(json!({ "status": "cancelled" })), &[]).await;
    assert_eq!((s, cancelled["vendor_cancelled"].as_bool()), (StatusCode::OK, Some(true)), "{cancelled}");
    for path in [&org_limits, &key_limits] {
        let (_, l) = send(&fx.app, "GET", path, Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
        assert_eq!(spend(l), 0.0, "refunded: {path}");
    }
}

#[tokio::test]
async fn lapsed_budget_window_resets_spend() {
    let fx = fixture().await;
    let (s, l) = send(&fx.app, "PUT", &format!("/v1/orgs/{}/limits", fx.org), Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_budget_usd": 6.0, "budget_duration": "1d" })), &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert!(l["budget_reset_at"].is_string());
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await.0, StatusCode::OK);
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await.0, StatusCode::PAYMENT_REQUIRED);

    // Move the window end into the past, as if a day went by.
    let store = fx.db.management().unwrap();
    let mut row = store.get_limits(EntityType::Org, &fx.org).await.unwrap().unwrap();
    let lapsed = chrono::Utc::now() - chrono::Duration::hours(1);
    row.budget_reset_at = Some(lapsed);
    store.upsert_limits(&row).await.unwrap();

    let (_, l) = send(&fx.app, "GET", &format!("/v1/orgs/{}/limits", fx.org), Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!(l["spend_usd"], 0.0);
    let next: chrono::DateTime<chrono::Utc> = l["budget_reset_at"].as_str().unwrap().parse().unwrap();
    assert_eq!(next, lapsed + chrono::Duration::days(1), "window rolls forward from its anchor");
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await.0, StatusCode::OK);
}

#[tokio::test]
async fn reset_spend_endpoints_zero_the_window() {
    let fx = fixture().await;
    let base = format!("/v1/keys/{}/limits", fx.gen_key.id);
    send(&fx.app, "PUT", &base, Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_budget_usd": 5.0 })), &[]).await;
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await.0, StatusCode::OK);
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await.0, StatusCode::PAYMENT_REQUIRED);
    let (s, l) = send(&fx.app, "POST", &format!("{base}/reset"), Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!((s, l["spend_usd"].as_f64()), (StatusCode::OK, Some(0.0)));
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await.0, StatusCode::OK);

    for uri in [
        format!("/v1/orgs/{}/limits/reset", fx.org),
        format!("/v1/apps/{}/limits/reset", fx.app_id),
        format!("/v1/orgs/{}/members/{}/limits/reset", fx.org, fx.owner.user.id),
        "/v1/end-users/nobody/limits/reset".to_string(),
    ] {
        let (s, b) = send(&fx.app, "POST", &uri, Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}: {b}");
        assert_eq!(b["error"]["type"], "limits_not_found", "{uri}");
    }
}

#[tokio::test]
async fn soft_budget_raises_one_alert_per_window() {
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("x-litegen-event", "budget.soft_limit_reached"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let fx = fixture().await;
    let (s, _) = send(&fx.app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "alert_webhook_url": server.uri() })), &[]).await;
    assert_eq!(s, StatusCode::OK);
    send(&fx.app, "PUT", &format!("/v1/orgs/{}/limits", fx.org), Auth::Bearer(&fx.admin_key.raw), Some(json!({ "soft_budget_usd": 4.0 })), &[]).await;
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await.0, StatusCode::OK);
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await.0, StatusCode::OK);

    let (_, l) = send(&fx.app, "GET", &format!("/v1/orgs/{}/limits", fx.org), Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert!(l["soft_budget_alerted_at"].is_string(), "{l}");
    assert_eq!(l["spend_usd"], 10.0);

    let (entries, _) = fx.db.list_audit_log_for_tenant(&fx.org, 1, 100).await.unwrap();
    let alerts: Vec<_> = entries.iter().filter(|e| e.action == "budget.soft_limit_reached").collect();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].target_id, fx.org);

    for _ in 0..100 {
        if !server.received_requests().await.unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let body: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    assert_eq!(body["entity_type"], "org");
    assert_eq!(body["soft_budget_usd"], 4.0);
}

#[tokio::test]
async fn limits_validation_rejects_bad_input() {
    let fx = fixture().await;
    let uri = format!("/v1/orgs/{}/limits", fx.org);
    for (body, needle) in [
        (json!({ "max_budget_usd": -1 }), "max_budget_usd"),
        (json!({ "max_budget_usd": 1, "soft_budget_usd": 2 }), "soft_budget_usd must not exceed"),
        (json!({ "budget_duration": "fortnight" }), "budget_duration"),
        (json!({ "rpm_limit": 0 }), "rpm_limit"),
        (json!({ "max_parallel_requests": 0 }), "max_parallel_requests"),
        (json!({ "metadata": [1, 2] }), "metadata"),
        (json!({ "allowed_models": ["mock/does-not-exist"] }), "unknown model"),
    ] {
        let (s, b) = send(&fx.app, "PUT", &uri, Auth::Bearer(&fx.admin_key.raw), Some(body.clone()), &[]).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}: {b}");
        assert!(b["error"]["message"].as_str().unwrap().contains(needle), "{body}: {b}");
    }
}

#[tokio::test]
async fn put_preserves_spend_and_restarts_the_window_on_duration_change() {
    let fx = fixture().await;
    let uri = format!("/v1/orgs/{}/limits", fx.org);
    send(&fx.app, "PUT", &uri, Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_budget_usd": 100.0, "budget_duration": "30d", "metadata": { "team": "growth" } })), &[]).await;
    generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await;
    let (_, before) = send(&fx.app, "GET", &uri, Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    let (_, same) = send(&fx.app, "PUT", &uri, Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_budget_usd": 50.0, "budget_duration": "30d" })), &[]).await;
    assert_eq!(same["spend_usd"], 5.0);
    assert_eq!(same["budget_reset_at"], before["budget_reset_at"], "same duration keeps the window");
    assert_eq!(same["metadata"], Value::Null, "PUT replaces every configurable field");
    let (_, changed) = send(&fx.app, "PUT", &uri, Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_budget_usd": 50.0, "budget_duration": "1w" })), &[]).await;
    assert_eq!(changed["spend_usd"], 5.0);
    assert_ne!(changed["budget_reset_at"], before["budget_reset_at"]);
}

// ─── Model access, rate and parallel limits ─────────────────────────────────

#[tokio::test]
async fn key_model_allowlist_is_enforced() {
    let fx = fixture().await;
    let (s, _) = send(&fx.app, "PUT", &format!("/v1/keys/{}/limits", fx.gen_key.id), Auth::Bearer(&fx.admin_key.raw), Some(json!({ "allowed_models": [FREE] })), &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, FREE, &[]).await.0, StatusCode::OK);
    let (s, b) = generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await;
    assert_eq!((s, err_type(&b)), (StatusCode::FORBIDDEN, "model_not_allowed"));
}

#[tokio::test]
async fn org_pool_and_app_access_are_enforced_and_filter_the_model_list() {
    let fx = fixture().await;
    let (s, _) = send(&fx.app, "PUT", &format!("/v1/orgs/{}/allowed-models", fx.org), Auth::Session(&fx.owner, Some(&fx.org)), Some(json!({ "models": [FREE, EXPENSIVE] })), &[]).await;
    assert_eq!(s, StatusCode::OK);
    let (s, b) = generate(&fx.app, &fx.gen_key.raw, "mock/all-params-image", &[]).await;
    assert_eq!((s, err_type(&b)), (StatusCode::FORBIDDEN, "model_not_allowed"));
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, FREE, &[]).await.0, StatusCode::OK);

    let (_, list) = send(&fx.app, "GET", "/v1/models", Auth::Bearer(&fx.gen_key.raw), None, &[]).await;
    let ids: Vec<&str> = list["data"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![FREE, EXPENSIVE].into_iter().collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>());

    // App narrowed to one model.
    let (s, _) = send(&fx.app, "PUT", &format!("/v1/apps/{}/allowed-models", fx.app_id), Auth::Session(&fx.owner, Some(&fx.org)), Some(json!({ "mode": "select", "models": [FREE] })), &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await.0, StatusCode::FORBIDDEN);
    let (_, list) = send(&fx.app, "GET", "/v1/models", Auth::Bearer(&fx.gen_key.raw), None, &[]).await;
    assert_eq!(list["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn unrestricted_tenants_see_every_model_sorted() {
    let fx = fixture().await;
    let (_, list) = send(&fx.app, "GET", "/v1/models", Auth::Bearer(&fx.gen_key.raw), None, &[]).await;
    let ids: Vec<String> = list["data"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids.len(), fx.state.registry.len());
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
}

#[tokio::test]
async fn rpm_limit_on_an_app_returns_429_with_retry_after() {
    let fx = fixture().await;
    send(&fx.app, "PUT", &format!("/v1/apps/{}/limits", fx.app_id), Auth::Bearer(&fx.admin_key.raw), Some(json!({ "rpm_limit": 1 })), &[]).await;
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, FREE, &[]).await.0, StatusCode::OK);
    let req = Request::builder()
        .method("POST")
        .uri("/v1/images/generations")
        .header("authorization", format!("Bearer {}", fx.gen_key.raw))
        .header("content-type", "application/json")
        .body(Body::from(json!({ "model": FREE, "prompt": "a cat" }).to_string()))
        .unwrap();
    let resp = fx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(resp.headers().get("retry-after").is_some());
}

#[tokio::test]
async fn parallel_cap_refuses_while_slots_are_taken() {
    let fx = fixture().await;
    send(&fx.app, "PUT", &format!("/v1/keys/{}/limits", fx.gen_key.id), Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_parallel_requests": 1 })), &[]).await;
    let held = ParallelSlot::try_acquire(EntityType::Key, &fx.gen_key.id.to_string(), 1).unwrap();
    let (s, b) = generate(&fx.app, &fx.gen_key.raw, FREE, &[]).await;
    assert_eq!((s, err_type(&b)), (StatusCode::TOO_MANY_REQUESTS, "parallel_limit_exceeded"));
    drop(held);
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, FREE, &[]).await.0, StatusCode::OK);
    assert_eq!(crate::management::enforce::in_flight(EntityType::Key, &fx.gen_key.id.to_string()), 0, "slot freed after the request");
}

// ─── Keys: block/unblock, listing, /limits/me ───────────────────────────────

#[tokio::test]
async fn blocked_keys_are_refused_until_unblocked() {
    let fx = fixture().await;
    let (s, b) = send(&fx.app, "POST", &format!("/v1/keys/{}/block", fx.gen_key.id), Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!((s, b["is_active"].as_bool()), (StatusCode::OK, Some(false)));
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, FREE, &[]).await.0, StatusCode::UNAUTHORIZED);
    let (s, b) = send(&fx.app, "POST", &format!("/v1/keys/{}/unblock", fx.gen_key.id), Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!((s, b["is_active"].as_bool()), (StatusCode::OK, Some(true)));
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, FREE, &[]).await.0, StatusCode::OK);
}

#[tokio::test]
async fn key_endpoints_authorize_by_scope_org_and_ownership() {
    let fx = fixture().await;
    let uri = format!("/v1/keys/{}/limits", fx.gen_key.id);
    // A generate/read key cannot manage keys.
    let (s, _) = send(&fx.app, "PUT", &uri, Auth::Bearer(&fx.gen_key.raw), Some(json!({})), &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // A key from another org sees nothing.
    let (org2, app2) = seed_org(&fx.db, "Other").await;
    let other = seed_key(&fx.db, &org2, &app2, "generate,read,admin", None).await;
    let (s, _) = send(&fx.app, "GET", &uri, Auth::Bearer(&other.raw), None, &[]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // A viewer session cannot write; the owner can.
    let viewer = seed_user(&fx.db, "viewer@acme.test").await;
    fx.db.add_org_member(&fx.org, &viewer.user.id, Role::Viewer).await.unwrap();
    let (s, _) = send(&fx.app, "PUT", &uri, Auth::Session(&viewer, Some(&fx.org)), Some(json!({})), &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = send(&fx.app, "PUT", &uri, Auth::Session(&fx.owner, Some(&fx.org)), Some(json!({})), &[]).await;
    assert_eq!(s, StatusCode::OK);
    // Unknown and malformed ids.
    for id in [Uuid::new_v4().to_string(), "not-a-uuid".to_string()] {
        let (s, _) = send(&fx.app, "GET", &format!("/v1/keys/{id}/limits"), Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }
    // No limits yet on the admin key.
    let (s, b) = send(&fx.app, "GET", &format!("/v1/keys/{}/limits", fx.admin_key.id), Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!((s, err_type(&b)), (StatusCode::NOT_FOUND, "limits_not_found"));
    // Delete, then delete again.
    let (s, b) = send(&fx.app, "DELETE", &uri, Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!((s, b["deleted"].as_bool()), (StatusCode::OK, Some(true)));
    assert_eq!(send(&fx.app, "DELETE", &uri, Auth::Bearer(&fx.admin_key.raw), None, &[]).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn org_and_member_endpoints_check_membership_and_existence() {
    let fx = fixture().await;
    let outsider = seed_user(&fx.db, "outsider@x.test").await;
    let (s, _) = send(&fx.app, "GET", &format!("/v1/orgs/{}/limits", fx.org), Auth::Session(&outsider, None), None, &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = send(&fx.app, "PUT", "/v1/orgs/org-missing/limits", Auth::Bearer(MASTER), Some(json!({})), &[]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, b) = send(&fx.app, "PUT", &format!("/v1/orgs/{}/members/nobody/limits", fx.org), Auth::Bearer(MASTER), Some(json!({})), &[]).await;
    assert_eq!((s, err_type(&b)), (StatusCode::NOT_FOUND, "member_not_found"));
    let (s, _) = send(&fx.app, "GET", "/v1/apps/app-missing/limits", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // A read-only key reads its own org but cannot write it.
    let (s, _) = send(&fx.app, "PUT", &format!("/v1/orgs/{}/limits", fx.org), Auth::Bearer(&fx.gen_key.raw), Some(json!({})), &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    send(&fx.app, "PUT", &format!("/v1/orgs/{}/limits", fx.org), Auth::Bearer(MASTER), Some(json!({ "max_budget_usd": 9.0 })), &[]).await;
    let (s, l) = send(&fx.app, "GET", &format!("/v1/orgs/{}/limits", fx.org), Auth::Bearer(&fx.gen_key.raw), None, &[]).await;
    assert_eq!((s, l["max_budget_usd"].as_f64()), (StatusCode::OK, Some(9.0)));
    // Unauthenticated requests never reach the handlers.
    assert_eq!(send(&fx.app, "GET", "/v1/limits", Auth::Anon, None, &[]).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn list_limits_and_my_limits() {
    let fx = fixture().await;
    let admin = Auth::Bearer(&fx.admin_key.raw);
    send(&fx.app, "PUT", &format!("/v1/orgs/{}/limits", fx.org), Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_budget_usd": 50 })), &[]).await;
    send(&fx.app, "PUT", &format!("/v1/apps/{}/limits", fx.app_id), Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_budget_usd": 20 })), &[]).await;
    send(&fx.app, "PUT", &format!("/v1/keys/{}/limits", fx.admin_key.id), Auth::Bearer(&fx.admin_key.raw), Some(json!({ "rpm_limit": 100 })), &[]).await;
    send(&fx.app, "PUT", &format!("/v1/orgs/{}/members/{}/limits", fx.org, fx.owner.user.id), Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_budget_usd": 10 })), &[]).await;
    // Another org's row must not leak into this listing.
    let (org2, _) = seed_org(&fx.db, "Other").await;
    send(&fx.app, "PUT", &format!("/v1/orgs/{org2}/limits"), Auth::Bearer(MASTER), Some(json!({ "max_budget_usd": 1 })), &[]).await;

    let (s, all) = send(&fx.app, "GET", "/v1/limits", admin, None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(all["data"].as_array().unwrap().len(), 4, "{all}");
    let (_, apps) = send(&fx.app, "GET", "/v1/limits?entity_type=app", Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!(apps["data"].as_array().unwrap().len(), 1);
    let (s, _) = send(&fx.app, "GET", "/v1/limits?entity_type=team", Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = send(&fx.app, "GET", &format!("/v1/limits?org_id={org2}"), Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "only the platform admin may name another org");
    let (_, other) = send(&fx.app, "GET", &format!("/v1/limits?org_id={org2}"), Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(other["data"].as_array().unwrap().len(), 1);

    // The admin key is owned by the owner: key, org, member and app rows apply.
    let (_, me) = send(&fx.app, "GET", "/v1/limits/me", Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    let mut types: Vec<&str> = me["data"].as_array().unwrap().iter().map(|l| l["entity_type"].as_str().unwrap()).collect();
    types.sort();
    assert_eq!(types, vec!["app", "key", "member", "org"]);
    // The unowned key sees org + app only.
    let (_, me) = send(&fx.app, "GET", "/v1/limits/me", Auth::Bearer(&fx.gen_key.raw), None, &[]).await;
    assert_eq!(me["data"].as_array().unwrap().len(), 2);
}

// ─── End users ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn end_user_crud() {
    let fx = fixture().await;
    let admin = || Auth::Bearer(&fx.admin_key.raw);
    let (s, u) = send(&fx.app, "POST", "/v1/end-users", admin(), Some(json!({ "id": "cust-1", "alias": "Jane", "metadata": { "plan": "pro" }, "limits": { "max_budget_usd": 3 } })), &[]).await;
    assert_eq!(s, StatusCode::CREATED, "{u}");
    assert_eq!(u["id"], "cust-1");
    assert_eq!(u["alias"], "Jane");
    assert_eq!(u["limits"]["max_budget_usd"], 3.0);
    let (s, b) = send(&fx.app, "POST", "/v1/end-users", admin(), Some(json!({ "id": "cust-1" })), &[]).await;
    assert_eq!((s, err_type(&b)), (StatusCode::CONFLICT, "end_user_exists"));
    for bad in [json!({ "id": "  " }), json!({ "id": "x".repeat(300) }), json!({ "id": "ok", "metadata": "nope" }), json!({ "id": "ok", "limits": { "rpm_limit": 0 } })] {
        assert_eq!(send(&fx.app, "POST", "/v1/end-users", admin(), Some(bad), &[]).await.0, StatusCode::BAD_REQUEST);
    }

    let (s, u) = send(&fx.app, "PATCH", "/v1/end-users/cust-1", admin(), Some(json!({ "alias": "Janet", "blocked": true })), &[]).await;
    assert_eq!((s, u["alias"].as_str(), u["blocked"].as_bool()), (StatusCode::OK, Some("Janet"), Some(true)));
    let (_, u) = send(&fx.app, "GET", "/v1/end-users/cust-1", admin(), None, &[]).await;
    assert_eq!(u["metadata"]["plan"], "pro", "PATCH leaves omitted fields alone");
    let (_, list) = send(&fx.app, "GET", "/v1/end-users", admin(), None, &[]).await;
    assert_eq!(list["data"].as_array().unwrap().len(), 1);

    let (s, _) = send(&fx.app, "GET", "/v1/end-users/ghost", admin(), None, &[]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = send(&fx.app, "PATCH", "/v1/end-users/ghost", admin(), Some(json!({})), &[]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, d) = send(&fx.app, "DELETE", "/v1/end-users/cust-1", admin(), None, &[]).await;
    assert_eq!((s, d["deleted"].as_bool()), (StatusCode::OK, Some(true)));
    assert!(fx.db.management().unwrap().get_limits(EntityType::EndUser, &end_user_entity_id(&fx.org, "cust-1")).await.unwrap().is_none(), "limits go with it");
    assert_eq!(send(&fx.app, "DELETE", "/v1/end-users/cust-1", admin(), None, &[]).await.0, StatusCode::NOT_FOUND);
    // Read-only keys can list but not create.
    assert_eq!(send(&fx.app, "GET", "/v1/end-users", Auth::Bearer(&fx.gen_key.raw), None, &[]).await.0, StatusCode::OK);
    assert_eq!(send(&fx.app, "POST", "/v1/end-users", Auth::Bearer(&fx.gen_key.raw), Some(json!({ "id": "x" })), &[]).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn end_users_are_tracked_blocked_and_budgeted_per_request() {
    let fx = fixture().await;
    let eu = [("x-litegen-end-user", "cust-9")];
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, FREE, &eu).await.0, StatusCode::OK);
    let (_, u) = send(&fx.app, "GET", "/v1/end-users/cust-9", Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert!(u["last_seen_at"].is_string(), "first sight registers the end user: {u}");

    // Blocking an id never seen before registers it as blocked.
    let (s, u) = send(&fx.app, "POST", "/v1/end-users/fresh/block", Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!((s, u["blocked"].as_bool()), (StatusCode::OK, Some(true)));
    let (s, b) = generate(&fx.app, &fx.gen_key.raw, FREE, &[("x-litegen-end-user", "fresh")]).await;
    assert_eq!((s, err_type(&b)), (StatusCode::FORBIDDEN, "end_user_blocked"));
    send(&fx.app, "POST", "/v1/end-users/fresh/unblock", Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, FREE, &[("x-litegen-end-user", "fresh")]).await.0, StatusCode::OK);

    // Per-end-user budget via the limits sub-resource.
    let (s, l) = send(&fx.app, "PUT", "/v1/end-users/cust-9/limits", Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_budget_usd": 5.0 })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{l}");
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &eu).await.0, StatusCode::OK);
    let (s, b) = generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &eu).await;
    assert_eq!((s, err_type(&b)), (StatusCode::PAYMENT_REQUIRED, "budget_exceeded"));
    assert!(b["error"]["message"].as_str().unwrap().contains("end_user"));
    // Other end users and anonymous requests are unaffected.
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[("x-litegen-end-user", "cust-10")]).await.0, StatusCode::OK);
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await.0, StatusCode::OK);

    let (_, l) = send(&fx.app, "GET", "/v1/end-users/cust-9/limits", Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!(l["spend_usd"], 5.0);
    let (s, _) = send(&fx.app, "DELETE", "/v1/end-users/cust-9/limits", Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &eu).await.0, StatusCode::OK);
}

#[tokio::test]
async fn default_end_user_budget_applies_to_new_end_users() {
    let fx = fixture().await;
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "default_end_user_max_budget_usd": 6.0, "default_end_user_budget_duration": "1d" })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let eu = [("x-litegen-end-user", "newbie")];
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &eu).await.0, StatusCode::OK);
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &eu).await.0, StatusCode::PAYMENT_REQUIRED);
    let (_, l) = send(&fx.app, "GET", "/v1/end-users/newbie/limits", Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!((l["max_budget_usd"].as_f64(), l["budget_duration"].as_str()), (Some(6.0), Some("1d")));
}

// ─── Spend ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn spend_report_groups_by_every_dimension() {
    let fx = fixture().await;
    generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[("x-litegen-end-user", "a"), ("x-litegen-tags", "campaign-x, beta")]).await;
    generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[("x-litegen-end-user", "b"), ("x-litegen-tags", "beta")]).await;
    generate(&fx.app, &fx.admin_key.raw, FREE, &[]).await;
    wait_for_logs(&fx, 3).await;
    let get = |q: &str| {
        let uri = format!("/v1/spend/report?{q}");
        let app = fx.app.clone();
        let key = fx.admin_key.raw.clone();
        async move { send(&app, "GET", &uri, Auth::Bearer(&key), None, &[]).await }
    };

    let (s, r) = get("group_by=key").await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["total_requests"], 3);
    assert_eq!(r["total_cost_usd"], 10.0);
    let first = &r["data"][0];
    assert_eq!((first["group"].as_str(), first["cost_usd"].as_f64(), first["requests"].as_u64()), (Some(fx.gen_key.id.to_string().as_str()), Some(10.0), Some(2)));

    let (_, r) = get("group_by=end_user").await;
    let groups: Vec<Value> = r["data"].as_array().unwrap().iter().map(|g| g["group"].clone()).collect();
    assert!(groups.contains(&json!("a")) && groups.contains(&json!("b")) && groups.contains(&Value::Null), "{r}");

    let (_, r) = get("group_by=tag").await;
    assert_eq!(r["data"][0], json!({ "group": "beta", "requests": 2, "failed_requests": 0, "cost_usd": 10.0 }));
    assert_eq!(r["total_requests"], 3, "tag totals count each request once");

    let (_, r) = get("group_by=user").await;
    assert!(r["data"].as_array().unwrap().iter().any(|g| g["group"] == json!(fx.owner.user.id)), "key owner is the user: {r}");

    let (_, r) = get("group_by=model").await;
    assert_eq!(r["data"][0]["group"], EXPENSIVE);
    let (_, r) = get("group_by=day").await;
    assert_eq!(r["data"][0]["group"], chrono::Utc::now().format("%Y-%m-%d").to_string());
    for g in ["provider", "media_type", "status", "org", "app"] {
        let (s, r) = get(&format!("group_by={g}")).await;
        assert_eq!(s, StatusCode::OK, "{g}");
        assert_eq!(r["total_requests"], 3, "{g}");
    }

    // Filters.
    let (_, r) = get("group_by=model&end_user=a").await;
    assert_eq!(r["total_cost_usd"], 5.0);
    let (_, r) = get("group_by=model&tag=campaign-x").await;
    assert_eq!(r["total_requests"], 1);
    let (_, r) = get(&format!("group_by=model&key_id={}", fx.admin_key.id)).await;
    assert_eq!(r["total_cost_usd"], 0.0);
    let (_, r) = get("group_by=model&from=2000-01-01&to=2000-12-31").await;
    assert_eq!(r["total_requests"], 0);
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let (_, r) = get(&format!("group_by=model&from={today}&to={today}")).await;
    assert_eq!(r["total_requests"], 3);

    // Bad input.
    assert_eq!(get("group_by=team").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(get("from=yesterday").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(get("from=2026-02-01&to=2026-01-01").await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn spend_is_tenant_scoped() {
    let fx = fixture().await;
    generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await;
    let (org2, app2) = seed_org(&fx.db, "Other").await;
    let other = seed_key(&fx.db, &org2, &app2, "generate,read", None).await;
    generate(&fx.app, &other.raw, EXPENSIVE, &[]).await;
    wait_for_logs(&fx, 1).await;

    let (_, mine) = send(&fx.app, "GET", "/v1/spend/report?group_by=org", Auth::Bearer(&fx.gen_key.raw), None, &[]).await;
    assert_eq!(mine["total_cost_usd"], 5.0);
    let (s, _) = send(&fx.app, "GET", &format!("/v1/spend/report?org_id={org2}"), Auth::Bearer(&fx.gen_key.raw), None, &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // The hosted platform admin sees every org, or one.
    for _ in 0..200 {
        let (_, all) = send(&fx.app, "GET", "/v1/spend/report?group_by=org", Auth::Bearer(MASTER), None, &[]).await;
        if all["total_cost_usd"] == 10.0 {
            assert_eq!(all["data"].as_array().unwrap().len(), 2);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let (_, one) = send(&fx.app, "GET", &format!("/v1/spend/report?group_by=org&org_id={org2}"), Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(one["total_cost_usd"], 5.0);
    // An app-bound key cannot widen to another app.
    let (s, _) = send(&fx.app, "GET", "/v1/spend/report?app_id=app-elsewhere", Auth::Bearer(&fx.gen_key.raw), None, &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn spend_logs_carry_attribution_and_paginate() {
    let fx = fixture().await;
    for i in 0..3 {
        let eu = format!("cust-{i}");
        generate(&fx.app, &fx.gen_key.raw, FREE, &[("x-litegen-end-user", &eu), ("x-litegen-tags", "t1,t2")]).await;
    }
    wait_for_logs(&fx, 3).await;
    let (s, page) = send(&fx.app, "GET", "/v1/spend/logs?per_page=2", Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((page["total"].as_u64(), page["per_page"].as_u64(), page["data"].as_array().unwrap().len()), (Some(3), Some(2), 2));
    let row = &page["data"][0];
    assert_eq!(row["key_id"], json!(fx.gen_key.id.to_string()));
    assert_eq!(row["tags"], json!(["t1", "t2"]));
    assert!(row["end_user"].as_str().unwrap().starts_with("cust-"));
    let (_, p2) = send(&fx.app, "GET", "/v1/spend/logs?per_page=2&page=2", Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!(p2["data"].as_array().unwrap().len(), 1);
    let (_, one) = send(&fx.app, "GET", "/v1/spend/logs?end_user=cust-1", Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!(one["total"], 1);
    // Session requests are attributed to the user.
    let (s, _) = send(&fx.app, "POST", "/v1/images/generations", Auth::Session(&fx.owner, Some(&fx.org)), Some(json!({ "model": FREE, "prompt": "x" })), &[]).await;
    assert_eq!(s, StatusCode::OK);
    for _ in 0..200 {
        let (_, mine) = send(&fx.app, "GET", &format!("/v1/spend/logs?user_id={}", fx.owner.user.id), Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
        if mine["total"] == 1 {
            assert!(mine["data"][0]["key_id"].is_null());
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("session generation was never attributed to its user");
}

// ─── Gateway admin: models ──────────────────────────────────────────────────

fn runtime_model(id: &str) -> Value {
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
async fn runtime_models_are_created_served_generated_and_deleted() {
    let fx = fixture().await;
    let id = "mock/runtime-image";
    let (s, v) = send(&fx.app, "POST", "/v1/admin/models", Auth::Bearer(MASTER), Some(runtime_model(id)), &[]).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!((v["source"].as_str(), v["disabled"].as_bool()), (Some("runtime"), Some(false)));

    let (s, m) = send(&fx.app, "GET", &format!("/v1/models/{id}"), Auth::Bearer(&fx.gen_key.raw), None, &[]).await;
    assert_eq!((s, m["display_name"].as_str()), (StatusCode::OK, Some("Runtime Mock")));
    let (_, list) = send(&fx.app, "GET", "/v1/models", Auth::Bearer(&fx.gen_key.raw), None, &[]).await;
    assert!(list["data"].as_array().unwrap().iter().any(|m| m["id"] == id));
    let (s, g) = generate(&fx.app, &fx.gen_key.raw, id, &[]).await;
    assert_eq!(s, StatusCode::OK, "{g}");

    // Persisted for the next boot.
    let stored = fx.db.management().unwrap().list_runtime_models().await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].updated_by.as_deref(), Some("master-key"));

    // Replace.
    let mut changed = runtime_model(id);
    changed["display_name"] = json!("Renamed");
    let (s, v) = send(&fx.app, "PUT", &format!("/v1/admin/models/{id}"), Auth::Bearer(MASTER), Some(changed.clone()), &[]).await;
    assert_eq!((s, v["schema"]["display_name"].as_str()), (StatusCode::OK, Some("Renamed")));
    let (s, _) = send(&fx.app, "PUT", "/v1/admin/models/mock/other", Auth::Bearer(MASTER), Some(changed), &[]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, b) = send(&fx.app, "PUT", &format!("/v1/admin/models/{id}"), Auth::Bearer(MASTER), Some(runtime_model("mock/renamed")), &[]).await;
    assert_eq!((s, err_type(&b)), (StatusCode::BAD_REQUEST, "invalid_model"));

    // Delete.
    let (s, d) = send(&fx.app, "DELETE", &format!("/v1/admin/models/{id}"), Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!((s, d["deleted"].as_bool()), (StatusCode::OK, Some(true)));
    assert_eq!(send(&fx.app, "GET", &format!("/v1/models/{id}"), Auth::Bearer(&fx.gen_key.raw), None, &[]).await.0, StatusCode::NOT_FOUND);
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, id, &[]).await.0, StatusCode::NOT_FOUND);
    assert_eq!(send(&fx.app, "DELETE", &format!("/v1/admin/models/{id}"), Auth::Bearer(MASTER), None, &[]).await.0, StatusCode::NOT_FOUND);
}

/// E4a: a runtime model cannot allowlist a field litegen owns (where the
/// vendor sends results), as a catalog row cannot.
#[tokio::test]
async fn a_runtime_model_cannot_allowlist_a_field_litegen_owns() {
    let fx = fixture().await;
    let mut m = runtime_model("mock/cb");
    m["extra_allowlist"] = json!(["callback_url"]);
    let (s, b) = send(&fx.app, "POST", "/v1/admin/models", Auth::Bearer(MASTER), Some(m), &[]).await;
    assert_eq!((s, err_type(&b)), (StatusCode::BAD_REQUEST, "invalid_model"), "{b}");
    assert!(b["error"]["message"].as_str().unwrap().contains("callback_url"), "{b}");
}

/// FU-LEO2: a runtime Leonardo video model cannot allowlist `imageId` or
/// `imageType`, as a catalog row cannot: through the gateway's own key, any
/// tenant could animate any image on its Leonardo account.
#[tokio::test]
async fn a_runtime_leonardo_video_model_cannot_take_an_image_id_in_extra() {
    let fx = fixture().await;
    let video = |id: &str, allowlist: Value| {
        let mut m = runtime_model(id);
        m["provider"] = json!("leonardo");
        m["media_type"] = json!("video");
        m["capabilities"] = json!({ "text_to_video": true });
        m["extra_allowlist"] = allowlist;
        m
    };
    let (s, b) = send(&fx.app, "POST", "/v1/admin/models", Auth::Bearer(MASTER), Some(video("leonardo/motion-2", json!(["motionStrength"]))), &[]).await;
    assert_eq!(s, StatusCode::CREATED, "the same model with a vendor knob: {b}");
    for key in ["imageId", "imageType"] {
        let id = format!("leonardo/motion-2-{}", key.to_lowercase());
        let (s, b) = send(&fx.app, "POST", "/v1/admin/models", Auth::Bearer(MASTER), Some(video(&id, json!([key]))), &[]).await;
        assert_eq!((s, err_type(&b)), (StatusCode::BAD_REQUEST, "invalid_model"), "{key}: {b}");
        let message = b["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains(&format!("'{id}'")) && message.contains(&format!("'{key}'")), "{key}: {b}");
    }
}

#[tokio::test]
async fn runtime_models_get_the_catalog_validation() {
    let fx = fixture().await;
    let post = |body: Value| send(&fx.app, "POST", "/v1/admin/models", Auth::Bearer(MASTER), Some(body), &[]);
    // Shadowing a catalog id.
    let (s, b) = post(runtime_model(FREE)).await;
    assert_eq!((s, err_type(&b)), (StatusCode::CONFLICT, "model_exists"));
    // Prefix must match the provider (validate_model).
    let mut m = runtime_model("openai/whatever");
    m["provider"] = json!("mock");
    let (s, b) = post(m).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    // Unknown param name (validate_model).
    let mut m = runtime_model("mock/bad-param");
    m["params"] = json!({ "frobnicate": { "kind": "seed", "min": 0, "max": 1 } });
    assert_eq!(post(m).await.0, StatusCode::BAD_REQUEST);
    // A provider with no adapter for the media type.
    let mut m = runtime_model("vidu/still");
    m["provider"] = json!("vidu");
    let (s, b) = post(m).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(b["error"]["message"].as_str().unwrap().contains("no image adapter"), "{b}");
    // Table-routed providers are refused outright.
    let mut m = runtime_model("fal/brand-new");
    m["provider"] = json!("fal");
    let (s, b) = post(m).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(b["error"]["message"].as_str().unwrap().contains("fixed endpoint table"), "{b}");
    // Not a schema at all.
    assert_eq!(post(json!({ "id": 5 })).await.0, StatusCode::BAD_REQUEST);
    // Catalog models are read-only.
    let (s, _) = send(&fx.app, "PUT", &format!("/v1/admin/models/{FREE}"), Auth::Bearer(MASTER), Some(runtime_model(FREE)), &[]).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = send(&fx.app, "DELETE", &format!("/v1/admin/models/{FREE}"), Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(s, StatusCode::CONFLICT);
}

/// Dashboard-server review #2 (orchestrator's ruling): `GET /v1/models/{id}`
/// serves `litegen_params`, which the server computes. A served schema posted
/// back to the admin API (or MCP `create_gateway_model`) is taken, with
/// `litegen_params` read and ignored as `status` is: the copy is served the
/// server's own bounds, never the posted ones, and nothing of it is stored.
#[tokio::test]
async fn a_served_schema_posted_back_is_taken_with_its_litegen_params_ignored() {
    let fx = fixture().await;
    let (s, served) = send(&fx.app, "GET", "/v1/models/mock/region-image", Auth::Bearer(&fx.gen_key.raw), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "{served}");
    let theirs = served["litegen_params"]["mask_grow_px"].clone();
    assert!(theirs.is_object(), "the served schema carries litegen_params: {served}");
    let id = "mock/region-image-copy";
    let mut copy = served.clone();
    copy["id"] = json!(id);
    copy["display_name"] = json!("Region copy");
    // A bound the server would never serve: posted, it must not be trusted.
    copy["litegen_params"]["mask_grow_px"]["max"] = json!(9999);

    let (s, created) = send(&fx.app, "POST", "/v1/admin/models", Auth::Bearer(MASTER), Some(copy.clone()), &[]).await;
    assert_eq!(s, StatusCode::CREATED, "a served schema posted back: {created}");
    let (s, again) = send(&fx.app, "GET", &format!("/v1/models/{id}"), Auth::Bearer(&fx.gen_key.raw), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "{again}");
    assert_eq!(again["litegen_params"]["mask_grow_px"], theirs, "the copy is served the server's own bounds, not the posted ones");
    let (s, replaced) = send(&fx.app, "PUT", &format!("/v1/admin/models/{id}"), Auth::Bearer(MASTER), Some(copy), &[]).await;
    assert_eq!(s, StatusCode::OK, "the same body sent to PUT: {replaced}");
    let stored = fx.db.management().unwrap().list_runtime_models().await.unwrap();
    assert!(stored.iter().all(|m| !m.schema_json.contains("litegen_params")), "the posted map was stored: {:?}", stored.iter().map(|m| &m.schema_json).collect::<Vec<_>>());
}

#[tokio::test]
async fn disabling_a_model_hides_and_refuses_it() {
    let fx = fixture().await;
    let uri = format!("/v1/admin/models/{FREE}");
    let (s, v) = send(&fx.app, "PATCH", &uri, Auth::Bearer(MASTER), Some(json!({ "disabled": true })), &[]).await;
    assert_eq!((s, v["disabled"].as_bool(), v["source"].as_str()), (StatusCode::OK, Some(true), Some("catalog")));
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, FREE, &[]).await.0, StatusCode::NOT_FOUND);
    let (_, list) = send(&fx.app, "GET", "/v1/models", Auth::Bearer(&fx.gen_key.raw), None, &[]).await;
    assert!(!list["data"].as_array().unwrap().iter().any(|m| m["id"] == FREE));
    // Still visible to the admin.
    let (_, all) = send(&fx.app, "GET", "/v1/admin/models", Auth::Bearer(MASTER), None, &[]).await;
    assert!(all["data"].as_array().unwrap().iter().any(|m| m["id"] == FREE && m["disabled"] == true));
    let (s, v) = send(&fx.app, "GET", &uri, Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!((s, v["disabled"].as_bool()), (StatusCode::OK, Some(true)));
    send(&fx.app, "PATCH", &uri, Auth::Bearer(MASTER), Some(json!({ "disabled": false })), &[]).await;
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, FREE, &[]).await.0, StatusCode::OK);
    let (s, _) = send(&fx.app, "PATCH", "/v1/admin/models/mock/nope", Auth::Bearer(MASTER), Some(json!({ "disabled": true })), &[]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(send(&fx.app, "GET", "/v1/admin/models/mock/nope", Auth::Bearer(MASTER), None, &[]).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_routes_are_platform_admin_only_in_hosted_mode() {
    let fx = fixture().await;
    for (method, uri) in [
        ("GET", "/v1/admin/models"),
        ("GET", "/v1/admin/providers"),
        ("GET", "/v1/admin/routes"),
        ("GET", "/v1/admin/settings"),
    ] {
        assert_eq!(send(&fx.app, method, uri, Auth::Bearer(&fx.admin_key.raw), None, &[]).await.0, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(send(&fx.app, method, uri, Auth::Session(&fx.owner, Some(&fx.org)), None, &[]).await.0, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(send(&fx.app, method, uri, Auth::Bearer(MASTER), None, &[]).await.0, StatusCode::OK, "{uri}");
    }
}

#[tokio::test]
async fn single_tenant_admins_manage_the_gateway() {
    let d = deployment().await;
    // An admin-scope key of the deployment and its owner's session both
    // qualify; a generate key and a member do not.
    let admin_key = seed_key(&d.db, DEFAULT_ORG_ID, DEFAULT_APP_ID, "generate,read,admin", Some(&d.owner.user.id)).await;
    let gen_key = seed_key(&d.db, DEFAULT_ORG_ID, DEFAULT_APP_ID, "generate,read", None).await;
    assert_eq!(send(&d.app, "GET", "/v1/admin/models", Auth::Bearer(&admin_key.raw), None, &[]).await.0, StatusCode::OK);
    assert_eq!(send(&d.app, "GET", "/v1/admin/settings", Auth::Session(&d.owner, Some(DEFAULT_ORG_ID)), None, &[]).await.0, StatusCode::OK);
    assert_eq!(send(&d.app, "GET", "/v1/admin/models", Auth::Bearer(&gen_key.raw), None, &[]).await.0, StatusCode::FORBIDDEN);
    let member = seed_deployment_user(&d.db, "member@deploy.test", Role::Member).await;
    assert_eq!(send(&d.app, "GET", "/v1/admin/routes", Auth::Session(&member, Some(DEFAULT_ORG_ID)), None, &[]).await.0, StatusCode::FORBIDDEN);
}

// ─── Gateway admin: providers ───────────────────────────────────────────────

#[tokio::test]
async fn provider_configs_store_encrypted_secrets_and_never_return_them() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let secret = "sk-provider-very-secret-9876";
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/providers/openai", Auth::Bearer(MASTER), Some(json!({ "api_key": secret, "api_base": "https://proxy.example/v1", "extra_headers": { "x-team": "t" } })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(!v.to_string().contains(secret));
    assert_eq!(v["key_hint"], "…9876");
    assert_eq!((v["active"].as_bool(), v["source"].as_str(), v["has_secret"].as_bool()), (Some(true), Some("runtime"), Some(true)));
    assert_eq!(v["api_base"], "https://proxy.example/v1");
    assert_eq!(v["extra_header_names"], json!(["x-team"]));
    assert!(fx.state.router.has_image_provider("openai").await);
    let stored = fx.db.management().unwrap().get_provider_config("openai").await.unwrap().unwrap();
    assert!(!stored.ciphertext.as_deref().unwrap().contains(secret), "stored encrypted");

    // An api_base-only update keeps the stored secret.
    let (_, v) = send(&fx.app, "PUT", "/v1/admin/providers/openai", Auth::Bearer(MASTER), Some(json!({ "api_base": "https://other.example/v1" })), &[]).await;
    assert_eq!((v["key_hint"].as_str(), v["api_base"].as_str()), (Some("…9876"), Some("https://other.example/v1")));
    let cfg = fx.state.router.registry.registered_config("openai").await.unwrap();
    assert_eq!(cfg.api_key, secret, "the live instance has the key");

    let (_, list) = send(&fx.app, "GET", "/v1/admin/providers", Auth::Bearer(MASTER), None, &[]).await;
    assert!(!list.to_string().contains(secret));
    let names: Vec<&str> = list["data"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"openai") && names.contains(&"mock"));
    let (_, one) = send(&fx.app, "GET", "/v1/admin/providers/openai", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(one["source"], "runtime");

    // Dropping the runtime config takes it out of service (no env config here).
    let (s, v) = send(&fx.app, "DELETE", "/v1/admin/providers/openai", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!((s, v["active"].as_bool(), v["source"].as_str()), (StatusCode::OK, Some(false), Some("none")));
    assert!(!fx.state.router.has_image_provider("openai").await);
    assert_eq!(send(&fx.app, "DELETE", "/v1/admin/providers/openai", Auth::Bearer(MASTER), None, &[]).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn provider_config_validation_and_enable_switch() {
    let fx = fixture().await; // no secrets key
    let (s, b) = send(&fx.app, "PUT", "/v1/admin/providers/openai", Auth::Bearer(MASTER), Some(json!({ "api_key": "sk-123456789" })), &[]).await;
    assert_eq!((s, err_type(&b)), (StatusCode::BAD_REQUEST, "secrets_key_missing"));
    let (s, _) = send(&fx.app, "PUT", "/v1/admin/providers/openai", Auth::Bearer(MASTER), Some(json!({ "api_base": "ftp://nope" })), &[]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // A header a default would break, or one that isn't HTTP, is refused by name.
    for headers in [json!({ "Host": "evil.example" }), json!({ "bad header": "x" }), json!({ "x-ok": "a\r\nb" })] {
        let (s, b) = send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(json!({ "extra_headers": headers })), &[]).await;
        assert_eq!((s, err_type(&b)), (StatusCode::BAD_REQUEST, "invalid_provider_config"), "{headers}: {b}");
    }
    assert_eq!(send(&fx.app, "PUT", "/v1/admin/providers/acme-ai", Auth::Bearer(MASTER), Some(json!({})), &[]).await.0, StatusCode::NOT_FOUND);
    assert_eq!(send(&fx.app, "GET", "/v1/admin/providers/acme-ai", Auth::Bearer(MASTER), None, &[]).await.0, StatusCode::NOT_FOUND);

    // Disable the mock provider: requests fail as unconfigured; re-enable restores it.
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(json!({ "enabled": false })), &[]).await;
    assert_eq!((s, v["active"].as_bool(), v["enabled"].as_bool()), (StatusCode::OK, Some(false), Some(false)));
    assert_ne!(generate(&fx.app, &fx.gen_key.raw, FREE, &[]).await.0, StatusCode::OK);
    let (_, v) = send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(json!({ "enabled": true })), &[]).await;
    assert_eq!(v["active"], true);
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, FREE, &[]).await.0, StatusCode::OK);
}

// ─── Gateway admin: routes and settings ─────────────────────────────────────

#[tokio::test]
async fn routes_can_be_replaced_and_reset() {
    let fx = fixture().await;
    let (_, v) = send(&fx.app, "GET", "/v1/admin/routes", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!((v["source"].as_str(), v["routes"].as_array().map(|r| r.len())), (Some("config"), Some(0)));
    let doc = json!({ "routes": [{ "model": "mock/*", "strategy": "fallback", "deployments": [{ "provider": "mock" }] }] });
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/routes", Auth::Bearer(MASTER), Some(doc), &[]).await;
    assert_eq!((s, v["source"].as_str()), (StatusCode::OK, Some("runtime")));
    assert_eq!(v["routes"][0]["deployments"][0]["weight"], 1, "defaults filled in");
    assert_eq!(fx.state.router.model_routes().len(), 1);
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, FREE, &[]).await.0, StatusCode::OK, "routed generation still works");
    let bad = json!({ "routes": [{ "model": "x", "deployments": [{ "provider": "nope" }] }] });
    assert_eq!(send(&fx.app, "PUT", "/v1/admin/routes", Auth::Bearer(MASTER), Some(bad), &[]).await.0, StatusCode::BAD_REQUEST);
    let (_, v) = send(&fx.app, "DELETE", "/v1/admin/routes", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(v["source"], "config");
    assert!(fx.state.router.model_routes().is_empty());
}

#[tokio::test]
async fn markup_setting_changes_what_requests_cost() {
    let fx = fixture().await;
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "cost_markup_percent": 10.0 })), &[]).await;
    assert_eq!((s, v["effective_cost_markup_percent"].as_f64()), (StatusCode::OK, Some(10.0)));
    send(&fx.app, "PUT", &format!("/v1/orgs/{}/limits", fx.org), Auth::Bearer(&fx.admin_key.raw), Some(json!({ "max_budget_usd": 100 })), &[]).await;
    assert_eq!(generate(&fx.app, &fx.gen_key.raw, EXPENSIVE, &[]).await.0, StatusCode::OK);
    let (_, l) = send(&fx.app, "GET", &format!("/v1/orgs/{}/limits", fx.org), Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert!((l["spend_usd"].as_f64().unwrap() - 5.5).abs() < 1e-9, "{l}");
    let (s, _) = send(&fx.app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "cost_markup_percent": -3 })), &[]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (_, v) = send(&fx.app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({})), &[]).await;
    assert_eq!(v["effective_cost_markup_percent"], 0.0, "clearing falls back to the config value");
}

// ─── Boot ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn everything_stored_is_reapplied_at_boot() {
    let fx = fixture_with(Mode::Hosted, true).await;
    send(&fx.app, "POST", "/v1/admin/models", Auth::Bearer(MASTER), Some(runtime_model("mock/booted")), &[]).await;
    send(&fx.app, "PATCH", &format!("/v1/admin/models/{EXPENSIVE}"), Auth::Bearer(MASTER), Some(json!({ "disabled": true })), &[]).await;
    send(&fx.app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "cost_markup_percent": 25.0 })), &[]).await;
    send(&fx.app, "PUT", "/v1/admin/routes", Auth::Bearer(MASTER), Some(json!({ "routes": [{ "model": "*", "deployments": [{ "provider": "mock" }] }] })), &[]).await;
    send(&fx.app, "PUT", "/v1/admin/providers/openai", Auth::Bearer(MASTER), Some(json!({ "api_key": "sk-boot-123456" })), &[]).await;
    // A row that no longer validates is skipped, not fatal.
    fx.db.management().unwrap().upsert_runtime_model("mock/broken", "{\"id\": \"mock/broken\"}", None).await.unwrap();

    let fresh = build_state(fx.db.clone(), Mode::Hosted, Some([7u8; 32])).await;
    assert!(fresh.registry.resolve("mock/booted").is_none());
    let report = crate::management::runtime::apply_at_boot(&fresh).await.unwrap();
    assert_eq!(
        report,
        crate::management::runtime::BootReport { runtime_models: 1, skipped_models: 1, disabled_models: 1, provider_configs: 1, routes_overridden: true }
    );
    assert!(fresh.registry.resolve("mock/booted").is_some());
    assert!(fresh.registry.resolve(EXPENSIVE).is_none());
    assert_eq!(fresh.router.cost_markup_percent(), 25.0);
    assert_eq!(fresh.router.model_routes().len(), 1);
    assert_eq!(fresh.router.registry.registered_config("openai").await.unwrap().api_key, "sk-boot-123456");
}

// ─── Refresh: a second replica over the same database ───────────────────────

/// A second replica of the fixture's deployment: its own state over the same
/// database, with nothing stored yet for a refresh to find.
async fn second_replica(fx: &Fx) -> (Router, Arc<AppState>) {
    let state = build_state(fx.db.clone(), Mode::Hosted, Some([7u8; 32])).await;
    assert!(!crate::management::runtime::refresh(&state).await.unwrap(), "a new replica has nothing to refresh");
    (create_router(state.clone()), state)
}

async fn refresh(state: &AppState) -> bool {
    crate::management::runtime::refresh(state).await.unwrap()
}

/// A refresh that read the model states, then a PATCH on this very replica,
/// then the refresh applying what it read: the PATCH must stand. Both ways.
#[tokio::test]
async fn a_refresh_that_read_before_a_patch_here_does_not_undo_it() {
    let fx = fixture_with(Mode::Hosted, true).await;
    for disabled in [true, false] {
        let pause = fx.state.router.pause_next_sync("model_states");
        let refreshing = tokio::spawn({
            let s = fx.state.clone();
            async move { crate::management::runtime::refresh(&s).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), pause.reached.notified()).await.expect("the refresh read the model states");
        let (s, _) = send(&fx.app, "PATCH", &format!("/v1/admin/models/{EXPENSIVE}"), Auth::Bearer(MASTER), Some(json!({ "disabled": disabled })), &[]).await;
        assert_eq!(s, StatusCode::OK);
        pause.resume.notify_one();
        refreshing.await.unwrap().unwrap();
        assert_eq!(
            fx.state.registry.resolve(EXPENSIVE).is_none(),
            disabled,
            "a refresh that read the database before the PATCH (disabled: {disabled}) undid it on the replica that answered 200"
        );
    }
}

/// NEW 1, the runtime-model twin: a model created on this replica, then a
/// refresh that read the rows, then a DELETE here, then the refresh applying
/// what it read. The deleted model must not be served again.
#[tokio::test]
async fn a_refresh_that_read_before_a_delete_here_does_not_serve_the_model_again() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let (s, _) = send(&fx.app, "POST", "/v1/admin/models", Auth::Bearer(MASTER), Some(runtime_model("mock/fleeting")), &[]).await;
    assert_eq!(s, StatusCode::CREATED);
    let pause = fx.state.router.pause_next_sync("runtime_models");
    let refreshing = tokio::spawn({
        let s = fx.state.clone();
        async move { crate::management::runtime::refresh(&s).await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), pause.reached.notified()).await.expect("the refresh read the runtime models");
    let (s, _) = send(&fx.app, "DELETE", "/v1/admin/models/mock/fleeting", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    pause.resume.notify_one();
    refreshing.await.unwrap().unwrap();
    assert!(
        fx.state.registry.resolve("mock/fleeting").is_none(),
        "a refresh that read the database before the DELETE served the deleted model again on the replica that answered 200"
    );
    refresh(&fx.state).await;
    assert!(fx.state.registry.resolve("mock/fleeting").is_none(), "and the next refresh keeps it gone");
}

#[tokio::test]
async fn a_refresh_applies_what_was_stored_once_and_follows_deletions() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let (_, b) = second_replica(&fx).await;
    send(&fx.app, "POST", "/v1/admin/models", Auth::Bearer(MASTER), Some(runtime_model("mock/refreshed")), &[]).await;
    send(&fx.app, "PATCH", &format!("/v1/admin/models/{EXPENSIVE}"), Auth::Bearer(MASTER), Some(json!({ "disabled": true })), &[]).await;
    send(&fx.app, "PUT", "/v1/admin/providers/openai", Auth::Bearer(MASTER), Some(json!({ "api_key": "sk-refresh-123456" })), &[]).await;
    assert!(b.registry.resolve("mock/refreshed").is_none());
    assert!(b.registry.resolve(EXPENSIVE).is_some());
    assert!(!b.router.has_image_provider("openai").await);

    assert!(refresh(&b).await, "three kinds of change found");
    assert!(b.registry.resolve("mock/refreshed").is_some());
    assert!(b.registry.resolve(EXPENSIVE).is_none(), "disabled");
    assert_eq!(b.router.registry.registered_config("openai").await.unwrap().api_key, "sk-refresh-123456");
    assert!(!refresh(&b).await, "nothing new the second time");

    // Every kind leaves again with its row.
    let store = fx.db.management().unwrap();
    store.delete_runtime_model("mock/refreshed").await.unwrap();
    store.set_model_state(EXPENSIVE, false).await.unwrap();
    store.delete_provider_config("openai").await.unwrap();
    assert!(refresh(&b).await);
    assert!(b.registry.resolve("mock/refreshed").is_none(), "the model left the runtime layer");
    assert!(b.registry.resolve(EXPENSIVE).is_some(), "enabled again");
    assert!(!b.router.has_image_provider("openai").await, "no row and no env config: out of service");
    assert!(!refresh(&b).await);
}

#[tokio::test]
async fn a_refresh_re_applies_a_row_it_applied_an_older_version_of() {
    // The race: a save made through this replica's API, and a refresh that
    // read the row before it and applied it after. The registry then holds the
    // older model, the record holds the older `updated_at`, and the database
    // the newer one.
    let fx = fixture_with(Mode::Hosted, true).await;
    let (b_app, b) = second_replica(&fx).await;
    let named = |name: &str| {
        let mut m = runtime_model("mock/raced");
        m["display_name"] = json!(name);
        m
    };
    send(&fx.app, "POST", "/v1/admin/models", Auth::Bearer(MASTER), Some(named("one")), &[]).await;
    assert!(refresh(&b).await);
    let store = fx.db.management().unwrap();
    let older = store.list_runtime_models().await.unwrap()[0].updated_at;
    assert_eq!(b.router.applied_runtime().models.get("mock/raced"), Some(&older));
    let stale = b.registry.runtime_model("mock/raced").unwrap();

    let (s, _) = send(&b_app, "PUT", "/v1/admin/models/mock/raced", Auth::Bearer(MASTER), Some(named("two")), &[]).await;
    assert_eq!(s, StatusCode::OK);
    b.registry.upsert_runtime(stale); // the refresh that read the old row lands last
    assert_eq!(b.registry.runtime_model("mock/raced").unwrap().display_name, "one");

    assert!(refresh(&b).await, "the record is older than the row");
    assert_eq!(b.registry.runtime_model("mock/raced").unwrap().display_name, "two", "healed");
    let newer = store.list_runtime_models().await.unwrap()[0].updated_at;
    assert!(newer > older);
    assert_eq!(b.router.applied_runtime().models.get("mock/raced"), Some(&newer));
    assert!(!refresh(&b).await, "and settled");
}

#[tokio::test]
async fn a_refresh_removes_what_this_replica_saved_and_another_deleted() {
    // Saved through this replica's API, so the record does not know it; deleted
    // through another replica before this one refreshed.
    let fx = fixture_with(Mode::Hosted, true).await;
    let (b_app, b) = second_replica(&fx).await;
    send(&b_app, "POST", "/v1/admin/models", Auth::Bearer(MASTER), Some(runtime_model("mock/local")), &[]).await;
    send(&b_app, "PUT", "/v1/admin/providers/openai", Auth::Bearer(MASTER), Some(json!({ "api_key": "sk-local-123456" })), &[]).await;
    assert!(b.registry.resolve("mock/local").is_some() && b.router.has_image_provider("openai").await);

    send(&fx.app, "DELETE", "/v1/admin/models/mock/local", Auth::Bearer(MASTER), None, &[]).await;
    send(&fx.app, "DELETE", "/v1/admin/providers/openai", Auth::Bearer(MASTER), None, &[]).await;
    assert!(refresh(&b).await);
    assert!(b.registry.resolve("mock/local").is_none(), "no longer served here");
    assert!(!b.router.has_image_provider("openai").await);
    assert!(!refresh(&b).await);
}

#[tokio::test]
async fn the_sync_task_applies_a_change_at_its_next_tick_without_redis() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let (_, b) = second_replica(&fx).await;
    let task = crate::management::runtime::spawn_runtime_sync(b.clone(), std::time::Duration::from_millis(50), false);
    send(&fx.app, "POST", "/v1/admin/models", Auth::Bearer(MASTER), Some(runtime_model("mock/ticked")), &[]).await;
    let mut arrived = false;
    for _ in 0..100 {
        if b.registry.resolve("mock/ticked").is_some() {
            arrived = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    task.abort();
    assert!(arrived, "no Redis here, so only the tick can have brought it");
}

/// Wait until a listener is subscribed to the replicas' event channel: an
/// event sent before is lost, which is how Redis pub/sub works.
async fn wait_until_subscribed(shared: &crate::shared::SharedRedis) {
    let channel = shared.key(&["events"]);
    for _ in 0..200 {
        let channel = channel.clone();
        let (_, subscribers): (String, i64) = shared
            .run(|mut c| async move { redis::cmd("PUBSUB").arg("NUMSUB").arg(channel).query_async(&mut c).await })
            .await
            .unwrap();
        if subscribers > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the listener never subscribed");
}

/// `DELETE /v1/cache` on one replica announces a `cache-clear` event, and a
/// replica that hears one from another replica empties its memory at once,
/// not at its next refresh: B's refresh is an hour away.
///
/// A and B are two states in this process, but each has a Redis client of its
/// own, and so an id of its own on the event channel (`SharedRedis::id`): B
/// hears A's announcement as another replica's, and skips only its own. The
/// test watches A's announcement on a subscription of A's client, then sends B
/// the same event by hand, from an id that is neither state's.
#[tokio::test]
async fn clearing_the_cache_announces_it_and_a_heard_clear_empties_memory_at_once() {
    use crate::shared::cache_tier::tests::{cache_get, cache_put, small_response};
    use futures::StreamExt;
    let Some((shared_a, shared_b)) = crate::shared::testing::two_shared().await else { return };
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let a = build_state_with(db.clone(), Mode::Hosted, None, Some(shared_a.clone())).await;
    let b = build_state_with(db.clone(), Mode::Hosted, None, Some(shared_b.clone())).await;
    let sync = crate::management::runtime::spawn_runtime_sync(b.clone(), std::time::Duration::from_secs(3600), true);
    wait_until_subscribed(&shared_b).await;
    let channel = shared_a.key(&["events"]);
    let mut watcher = shared_a.subscribe(&channel).await.unwrap();
    let mut heard = std::pin::pin!(watcher.on_message());

    cache_put(&a.router.cache, "p", small_response(), std::time::Duration::from_secs(60)).await;
    assert!(cache_get(&b.router.cache, "p").await.is_some(), "B has it, copied from the shared tier");
    assert_eq!(b.router.cache.entry_count().await, 1);

    let (status, _) = send(&create_router(a.clone()), "DELETE", "/v1/cache", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), heard.next()).await.expect("an event within 5 s").expect("a message");
    let announced: serde_json::Value = serde_json::from_str(&message.get_payload::<String>().unwrap()).unwrap();
    assert_eq!(announced["event"], "cache-clear", "{announced}");

    // The same event, from another replica.
    shared_a.publish(&channel, r#"{"event":"cache-clear","from":"another-replica"}"#).await.unwrap();
    let mut emptied = false;
    for _ in 0..200 {
        if b.router.cache.entry_count().await == 0 {
            emptied = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    sync.abort();
    assert!(emptied, "B emptied its memory on hearing it");
    assert!(cache_get(&b.router.cache, "p").await.is_none(), "and Redis holds nothing");
}

/// The event can be lost; the refresh every 15 s is the backstop.
#[tokio::test]
async fn a_refresh_empties_the_memory_of_a_cache_cleared_elsewhere() {
    use crate::shared::cache_tier::tests::{cache_get, cache_put, small_response};
    let Some((shared_a, shared_b)) = crate::shared::testing::two_shared().await else { return };
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let a = build_state_with(db.clone(), Mode::Hosted, None, Some(shared_a)).await;
    let b = build_state_with(db.clone(), Mode::Hosted, None, Some(shared_b)).await;
    cache_put(&a.router.cache, "p", small_response(), std::time::Duration::from_secs(60)).await;
    assert!(cache_get(&b.router.cache, "p").await.is_some());

    a.router.cache.clear().await.unwrap();
    assert_eq!(b.router.cache.entry_count().await, 1, "B has heard nothing");
    refresh(&b).await;
    assert_eq!(b.router.cache.entry_count().await, 0, "its refresh saw the epoch move");
}

/// A client that goes away mid-request, at the same point every time: the
/// clear is held once this replica's memory is empty and before it has touched
/// Redis, `request` is dropped there, and only then is the clear let go. The
/// request cannot have finished before it was dropped, however quick Redis is,
/// so what the caller checks next is whether the clear finished on its own.
async fn walk_away_during_the_clear(app: &Router, request: Request<Body>, state: &AppState) {
    let pause = state.router.cache.pause_next_clear();
    let mut call = Box::pin(app.clone().oneshot(request));
    tokio::select! {
        _ = &mut call => panic!("the request finished while its clear was held"),
        reached = tokio::time::timeout(std::time::Duration::from_secs(10), pause.reached.notified()) => {
            reached.expect("the handler never reached the clear");
        }
    }
    drop(call);
    pause.resume.notify_one();
}

/// What a clear that finished leaves: the epoch raised to `epoch`, no shared
/// answer, and the event on the channel (read from a subscription on a second
/// client, which hears A's event as another replica's).
async fn assert_clear_finished(shared: &crate::shared::SharedRedis, heard: &mut (impl futures::Stream<Item = redis::Msg> + Unpin), event: &str, epoch: i64) {
    use futures::StreamExt;
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), heard.next())
        .await
        .expect("the event still went out")
        .expect("a message");
    let announced: serde_json::Value = serde_json::from_str(&message.get_payload::<String>().unwrap()).unwrap();
    assert_eq!(announced["event"], event, "{announced}");
    assert_eq!(crate::shared::cache_tier::epoch(shared).await.unwrap(), epoch, "the epoch was raised");
    assert!(crate::shared::cache_tier::tests::entry_keys(shared).await.is_empty(), "the shared answers are gone");
}

#[tokio::test]
async fn a_cache_clear_goes_on_when_the_client_walks_away() {
    use crate::shared::cache_tier::tests::{cache_put, small_response};
    let Some((shared_a, shared_b)) = crate::shared::testing::two_shared().await else { return };
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let a = build_state_with(db.clone(), Mode::Hosted, None, Some(shared_a.clone())).await;
    let mut watcher = shared_b.subscribe(&shared_b.key(&["events"])).await.unwrap();
    let mut heard = std::pin::pin!(watcher.on_message());
    cache_put(&a.router.cache, "p", small_response(), std::time::Duration::from_secs(60)).await;

    let request = Request::builder().method("DELETE").uri("/v1/cache").header("authorization", format!("Bearer {MASTER}")).body(Body::empty()).unwrap();
    walk_away_during_the_clear(&create_router(a.clone()), request, &a).await;
    assert_clear_finished(&shared_a, &mut heard, "cache-clear", 1).await;
}

#[tokio::test]
async fn turning_the_cache_off_goes_on_when_the_client_walks_away() {
    use crate::shared::cache_tier::tests::{cache_put, small_response};
    let Some((shared_a, shared_b)) = crate::shared::testing::two_shared().await else { return };
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let a = build_state_with(db.clone(), Mode::Hosted, None, Some(shared_a.clone())).await;
    let app = create_router(a.clone());
    let (status, _) = send(&app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "cache_enabled": true })), &[]).await;
    assert_eq!(status, StatusCode::OK);
    // Turning it on cleared it too: the switch-off's clear is one more.
    let before = crate::shared::cache_tier::epoch(&shared_a).await.unwrap();
    let mut watcher = shared_b.subscribe(&shared_b.key(&["events"])).await.unwrap();
    let mut heard = std::pin::pin!(watcher.on_message());
    cache_put(&a.router.cache, "p", small_response(), std::time::Duration::from_secs(60)).await;

    let request = Request::builder()
        .method("PUT")
        .uri("/v1/admin/settings")
        .header("authorization", format!("Bearer {MASTER}"))
        .header("content-type", "application/json")
        .body(Body::from(json!({ "cache_enabled": false }).to_string()))
        .unwrap();
    walk_away_during_the_clear(&app, request, &a).await;
    assert_clear_finished(&shared_a, &mut heard, "reload", before + 1).await;
}

/// A clear that emptied this replica but not the shared tier (Redis out, or a
/// page that failed) must not say it cleared: the other replicas keep their
/// answers. The dashboard shows "Cache cleared" on any 2xx.
#[tokio::test]
async fn a_cache_clear_that_cannot_reach_the_shared_tier_says_so() {
    use crate::config::connections::RedisSettings;
    use crate::shared::cache_tier::tests::{entry_keys, small_response};
    use crate::shared::testing::{addr_of, flush_keys, test_prefix, test_url, CutProxy};
    let Some(url) = test_url() else { return };
    let proxy = CutProxy::start(addr_of(&url)).await;
    let prefix = test_prefix();
    let behind = crate::shared::SharedRedis::connect(&RedisSettings::from_url(&proxy.url()), &prefix).await.unwrap();
    let direct = crate::shared::SharedRedis::connect(&RedisSettings::from_url(&url), &prefix).await.unwrap();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let a = build_state_with(db, Mode::Hosted, None, Some(behind)).await;
    crate::shared::cache_tier::put(&direct, "p", &small_response(), std::time::Duration::from_secs(60)).await.unwrap();

    proxy.cut();
    let request = Request::builder().method("DELETE").uri("/v1/cache").header("authorization", format!("Bearer {MASTER}")).body(Body::empty()).unwrap();
    let response = create_router(a.clone()).oneshot(request).await.unwrap();
    let (status, retry_after) = (response.status(), response.headers().get("retry-after").cloned());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into()));
    assert_eq!(entry_keys(&direct).await.len(), 1, "precondition: the shared answer is still there");
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "the shared tier was not cleared, so the clear must not report success: {body}");
    assert_eq!(body["error"]["code"], "cache_clear_incomplete", "{body}");
    assert_eq!(retry_after.as_ref().and_then(|v| v.to_str().ok()), Some("1"), "a retryable 503");
    flush_keys(&direct).await;
}

/// Answers left in the shared tier while the cache was off (a switch-off
/// whose clear could not reach Redis, or answers stored around it) must not
/// be served once it is on again.
#[tokio::test]
async fn turning_the_cache_on_empties_what_the_shared_tier_kept_while_it_was_off() {
    use crate::shared::cache_tier::tests::{entry_keys, small_response};
    let Some((shared_a, _)) = crate::shared::testing::two_shared().await else { return };
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let a = build_state_with(db, Mode::Hosted, None, Some(shared_a.clone())).await; // the cache is off by default
    crate::shared::cache_tier::put(&shared_a, "leftover", &small_response(), std::time::Duration::from_secs(60)).await.unwrap();
    let (status, _) = send(&create_router(a.clone()), "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "cache_enabled": true })), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(entry_keys(&shared_a).await.is_empty(), "a switch-on must not serve what an earlier switch-off failed to remove");
    crate::shared::testing::flush_keys(&shared_a).await;
}

/// Turning the cache on clears it before the new settings apply. A route
/// saved on this replica during that clear (it answered 200) must still be in
/// force once they do. No Redis needed: the clear stops before it would look
/// for one.
#[tokio::test]
async fn a_route_saved_here_while_the_cache_is_switched_on_stays() {
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let a = build_state_with(db, Mode::Hosted, None, None).await; // the cache is off by default
    let app = create_router(a.clone());
    let pause = a.router.cache.pause_next_clear();
    let switching = switch_the_cache_on(&app);
    tokio::time::timeout(std::time::Duration::from_secs(5), pause.reached.notified()).await.expect("the switch-on reached its clear");
    assert_eq!(send(&app, "PUT", "/v1/admin/routes", Auth::Bearer(MASTER), Some(one_route()), &[]).await.0, StatusCode::OK);
    pause.resume.notify_one();
    assert_eq!(switching.await.unwrap(), StatusCode::OK);
    assert_the_route_stands(&app, &a).await;
}

/// The same for a route saved on another replica, which a refresh here
/// applies during the clear.
#[tokio::test]
async fn a_route_a_refresh_applies_while_the_cache_is_switched_on_stays() {
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let a = build_state(db.clone(), Mode::Hosted, Some([7u8; 32])).await;
    let b = build_state(db, Mode::Hosted, Some([7u8; 32])).await;
    let app = create_router(a.clone());
    let pause = a.router.cache.pause_next_clear();
    let switching = switch_the_cache_on(&app);
    tokio::time::timeout(std::time::Duration::from_secs(5), pause.reached.notified()).await.expect("the switch-on reached its clear");
    assert_eq!(send(&create_router(b), "PUT", "/v1/admin/routes", Auth::Bearer(MASTER), Some(one_route()), &[]).await.0, StatusCode::OK);
    assert!(refresh(&a).await, "this replica's refresh applied the other replica's route");
    pause.resume.notify_one();
    assert_eq!(switching.await.unwrap(), StatusCode::OK);
    assert_the_route_stands(&app, &a).await;
}

/// `PUT /v1/admin/settings {"cache_enabled": true}`, as a request of its own.
fn switch_the_cache_on(app: &Router) -> tokio::task::JoinHandle<StatusCode> {
    let app = app.clone();
    tokio::spawn(async move { send(&app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "cache_enabled": true })), &[]).await.0 })
}

fn one_route() -> Value {
    json!({ "routes": [{ "model": "mock/*", "deployments": [{ "provider": "mock" }] }] })
}

/// After the switch-on: the cache is on, and [`one_route`], saved during its
/// clear, is the route in force.
async fn assert_the_route_stands(app: &Router, a: &AppState) {
    assert!(a.router.settings().cache.enabled, "the switch-on applied");
    let (_, v) = send(app, "GET", "/v1/admin/routes", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(
        (v["source"].as_str(), v["routes"][0]["model"].as_str()),
        (Some("runtime"), Some("mock/*")),
        "a route saved while the cache was being switched on was undone when the switch applied: {v}"
    );
}

#[tokio::test]
async fn a_row_that_does_not_validate_is_left_out_and_not_retried() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let (_, b) = second_replica(&fx).await;
    let store = fx.db.management().unwrap();
    store.upsert_runtime_model("mock/broken", "{\"id\": \"mock/broken\"}", None).await.unwrap();
    assert!(!refresh(&b).await, "nothing served changed: the row is only recorded as seen");
    assert!(b.registry.runtime_model("mock/broken").is_none());
    let seen = store.list_runtime_models().await.unwrap()[0].updated_at;
    assert_eq!(b.router.applied_runtime().models.get("mock/broken"), Some(&seen), "recorded, so not parsed and warned about again");
    assert!(!refresh(&b).await);
}

#[tokio::test]
async fn a_valid_model_replaced_by_an_invalid_version_stops_being_served() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let (_, b) = second_replica(&fx).await;
    send(&fx.app, "POST", "/v1/admin/models", Auth::Bearer(MASTER), Some(runtime_model("mock/degraded")), &[]).await;
    assert!(refresh(&b).await);
    assert!(b.registry.resolve("mock/degraded").is_some());

    // As a restart would find it: the newest row does not validate here.
    fx.db.management().unwrap().upsert_runtime_model("mock/degraded", "{\"id\": \"mock/degraded\"}", None).await.unwrap();
    assert!(refresh(&b).await, "an older valid version left the runtime layer");
    assert!(b.registry.resolve("mock/degraded").is_none());
    assert!(!refresh(&b).await);
}

// ─── Single-tenant: who runs the deployment ─────────────────────────────────
//
// In single-tenant mode the default org is the deployment: its owners and
// admins run the gateway and its users. A session's permissions are its role
// in the org it acts in, so a role in any other org must carry none of that.

/// A signed-in user of a single-tenant deployment, holding `role` both on
/// their account and in the default org, as signup and invitations leave them.
async fn seed_deployment_user(db: &SqliteDatabase, email: &str, role: Role) -> Sess {
    let sess = seed_user(db, email).await;
    db.update_user(&sess.user.id, Some(role), None, None).await.unwrap();
    db.add_org_member(DEFAULT_ORG_ID, &sess.user.id, role).await.unwrap();
    sess
}

/// A single-tenant deployment with its owner and a viewer.
struct Deployment {
    app: Router,
    db: Arc<SqliteDatabase>,
    owner: Sess,
    viewer: Sess,
}

async fn deployment() -> Deployment {
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let state = build_state(db.clone(), Mode::SingleTenant, None).await;
    let owner = seed_deployment_user(&db, "owner@deploy.test", Role::Owner).await;
    let viewer = seed_deployment_user(&db, "viewer@deploy.test", Role::Viewer).await;
    Deployment { app: create_router(state), db, owner, viewer }
}

#[tokio::test]
async fn single_tenant_a_role_in_another_org_runs_nothing_of_the_deployment() {
    let d = deployment().await;
    // An org the viewer owns, as one they could make for themselves.
    let (side, side_app) = seed_org(&d.db, "Side").await;
    d.db.add_org_member(&side, &d.viewer.user.id, Role::Owner).await.unwrap();
    for (method, uri, body) in [
        ("GET", "/v1/admin/settings", None),
        ("GET", "/v1/admin/providers", None),
        ("GET", "/v1/users", None),
        ("DELETE", "/v1/cache", None),
        ("POST", "/v1/users/transfer-owner", Some(json!({ "new_owner_id": d.viewer.user.id }))),
    ] {
        let (s, v) = send(&d.app, method, uri, Auth::Session(&d.viewer, Some(&side)), body, &[]).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{method} {uri} acting in the side org: {v}");
    }
    // Nor does an admin-scope key of that org.
    let side_key = seed_key(&d.db, &side, &side_app, "generate,read,admin", Some(&d.viewer.user.id)).await;
    for (method, uri) in [("GET", "/v1/admin/models"), ("DELETE", "/v1/cache")] {
        let (s, v) = send(&d.app, method, uri, Auth::Bearer(&side_key.raw), None, &[]).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{method} {uri} with the side org's admin key: {v}");
    }
    // The deployment's owner keeps all of it.
    assert_eq!(d.db.get_user_by_id(&d.owner.user.id).await.unwrap().unwrap().role, Role::Owner);
    for (method, uri) in [("GET", "/v1/admin/settings"), ("GET", "/v1/users"), ("DELETE", "/v1/cache")] {
        let (s, v) = send(&d.app, method, uri, Auth::Session(&d.owner, None), None, &[]).await;
        assert_eq!(s, StatusCode::OK, "{method} {uri} as the owner: {v}");
    }
}

/// P1 (code review 2026-11-11): `GET /health` checks the gateway's own vendor
/// keys and shows what each vendor answered about them, so in hosted mode it
/// is the platform admin's alone. A tenant's key or session gets 403.
#[tokio::test]
async fn provider_health_is_the_platforms_in_hosted_mode() {
    let fx = fixture().await;
    for (who, auth) in [
        ("a tenant's generate,read key", Auth::Bearer(&fx.gen_key.raw)),
        ("a tenant's admin key", Auth::Bearer(&fx.admin_key.raw)),
        ("a tenant owner's session", Auth::Session(&fx.owner, Some(&fx.org))),
    ] {
        let (s, v) = send(&fx.app, "GET", "/health", auth, None, &[]).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{who} read the platform's provider health: {v}");
    }
    let (s, v) = send(&fx.app, "GET", "/health", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "the platform admin reads it: {v}");
}

/// P1: in single-tenant mode `/health` is the deployment's: a caller acting in
/// another org (a session there, or that org's key) gets 403, and a reader of
/// the default org keeps it.
#[tokio::test]
async fn single_tenant_provider_health_is_the_deployments() {
    let d = deployment().await;
    let (side, side_app) = seed_org(&d.db, "Side").await;
    d.db.add_org_member(&side, &d.viewer.user.id, Role::Owner).await.unwrap();
    let side_key = seed_key(&d.db, &side, &side_app, "generate,read", None).await;
    let (s, v) = send(&d.app, "GET", "/health", Auth::Session(&d.viewer, Some(&side)), None, &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "a session acting in the side org: {v}");
    let (s, v) = send(&d.app, "GET", "/health", Auth::Bearer(&side_key.raw), None, &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "the side org's read key: {v}");
    // The org is pinned: the viewer also owns the side org.
    let (s, v) = send(&d.app, "GET", "/health", Auth::Session(&d.viewer, Some(DEFAULT_ORG_ID)), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "the deployment's viewer: {v}");
    let (s, v) = send(&d.app, "GET", "/health", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "the master key: {v}");
}

#[tokio::test]
async fn single_tenant_orgs_are_made_by_the_deployments_admins() {
    let d = deployment().await;
    let member = seed_deployment_user(&d.db, "member@deploy.test", Role::Member).await;
    for who in [&d.viewer, &member] {
        let (s, v) = send(&d.app, "POST", "/v1/orgs", Auth::Session(who, None), Some(json!({ "name": "Mine" })), &[]).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{} made an org: {v}", who.user.email);
    }
    let (s, v) = send(&d.app, "POST", "/v1/orgs", Auth::Session(&d.owner, None), Some(json!({ "name": "Team" })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn single_tenant_removal_from_the_default_org_ends_access() {
    let d = deployment().await;
    let admin = seed_deployment_user(&d.db, "admin@deploy.test", Role::Admin).await;
    assert_eq!(send(&d.app, "GET", "/v1/admin/settings", Auth::Session(&admin, None), None, &[]).await.0, StatusCode::OK, "an admin runs the gateway");
    let (s, v) = send(&d.app, "DELETE", &format!("/v1/orgs/{DEFAULT_ORG_ID}/members/{}", admin.user.id), Auth::Session(&d.owner, None), None, &[]).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    let routes = [("GET", "/v1/admin/settings"), ("GET", "/v1/users"), ("GET", "/v1/keys"), ("GET", "/v1/generations")];
    // Its sessions end with its last membership (security fix 4, 2026-11-05)...
    for (method, uri) in routes {
        let (s, v) = send(&d.app, method, uri, Auth::Session(&admin, None), None, &[]).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{method} {uri} with the old session: {v}");
    }
    // ...and a new one, as signing in again gives, holds nothing.
    let (token, csrf) = (generate_session_token(), generate_csrf_token());
    d.db.create_session(&Session {
        id: token.clone(),
        user_id: admin.user.id.clone(),
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::days(1),
        ip: None,
        user_agent: None,
        csrf_token: csrf.clone(),
    })
    .await
    .unwrap();
    let again = Sess { user: admin.user.clone(), token, csrf };
    for (method, uri) in routes {
        let (s, v) = send(&d.app, method, uri, Auth::Session(&again, None), None, &[]).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{method} {uri} after signing in again: {v}");
    }
}

#[tokio::test]
async fn single_tenant_a_role_set_on_the_users_page_takes_effect() {
    let d = deployment().await;
    let admin = seed_deployment_user(&d.db, "admin@deploy.test", Role::Admin).await;
    let (s, v) = send(&d.app, "PATCH", &format!("/v1/users/{}", admin.user.id), Auth::Session(&d.owner, None), Some(json!({ "role": "viewer" })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = send(&d.app, "GET", "/v1/admin/settings", Auth::Session(&admin, None), None, &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "a viewer now: {v}");
    assert_eq!(d.db.get_membership(DEFAULT_ORG_ID, &admin.user.id).await.unwrap(), Some(Role::Viewer));
}

#[tokio::test]
async fn single_tenant_the_users_page_never_hands_out_owner() {
    let d = deployment().await;
    let admin = seed_deployment_user(&d.db, "admin@deploy.test", Role::Admin).await;
    for who in [&admin, &d.owner] {
        let (s, v) = send(&d.app, "PATCH", &format!("/v1/users/{}", admin.user.id), Auth::Session(who, None), Some(json!({ "role": "owner" })), &[]).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{} gave out owner: {v}", who.user.email);
    }
    assert_eq!(d.db.get_user_by_id(&admin.user.id).await.unwrap().unwrap().role, Role::Admin);
}

#[tokio::test]
async fn single_tenant_the_users_page_leaves_the_default_orgs_owner_alone() {
    let d = deployment().await;
    let admin = seed_deployment_user(&d.db, "admin@deploy.test", Role::Admin).await;
    // An account whose own role says admin while it owns the default org, as
    // `POST /v1/users/transfer-owner` leaves the previous owner.
    d.db.update_user(&d.owner.user.id, Some(Role::Admin), None, None).await.unwrap();
    let (s, v) = send(&d.app, "PATCH", &format!("/v1/users/{}", d.owner.user.id), Auth::Session(&admin, None), Some(json!({ "role": "viewer" })), &[]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(d.db.get_membership(DEFAULT_ORG_ID, &d.owner.user.id).await.unwrap(), Some(Role::Owner));
}

// ─── Request artifacts follow generation read rights ────────────────────────

/// Artifacts are written off the response path; wait until `id`'s lands.
async fn wait_for_artifact(db: &SqliteDatabase, id: &str) {
    for _ in 0..200 {
        if db.get_request_artifact(id).await.unwrap().is_some() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("no request artifact for {id}");
}

#[tokio::test]
async fn request_artifacts_follow_generation_read_own() {
    let fx = fixture().await;
    let member = seed_user(&fx.db, "member@acme.test").await;
    fx.db.add_org_member(&fx.org, &member.user.id, Role::Member).await.unwrap();
    let mine = seed_key(&fx.db, &fx.org, &fx.app_id, "generate,read", Some(&member.user.id)).await;
    let (s, v) = generate(&fx.app, &fx.admin_key.raw, FREE, &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let theirs = v["id"].as_str().unwrap().to_string();
    let (s, v) = generate(&fx.app, &mine.raw, FREE, &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let own = v["id"].as_str().unwrap().to_string();
    // A failed request leaves an artifact and no generation row.
    let failed = format!("litegen-img-{}", Uuid::new_v4());
    fx.db
        .insert_request_artifact(&RequestArtifact {
            request_id: failed.clone(),
            media_type: "image".into(),
            prompt: Some("a private prompt".into()),
            negative_prompt: None,
            params_json: None,
            refs_meta_json: None,
            output_kind: "error".into(),
            output_value: None,
            output_mime: None,
            output_truncated: false,
            error_message: Some("vendor refused".into()),
            created_at: chrono::Utc::now(),
            org_id: Some(fx.org.clone()),
            app_id: Some(fx.app_id.clone()),
        })
        .await
        .unwrap();
    wait_for_artifact(&fx.db, &theirs).await;
    wait_for_artifact(&fx.db, &own).await;

    // A member reads the artifacts of their own keys' generations, as
    // `GET /v1/generations/{id}` lets them, and no one else's.
    for (id, want) in [(&theirs, StatusCode::FORBIDDEN), (&failed, StatusCode::FORBIDDEN), (&own, StatusCode::OK)] {
        let (s, v) = send(&fx.app, "GET", &format!("/v1/logs/{id}/artifact"), Auth::Session(&member, Some(&fx.org)), None, &[]).await;
        assert_eq!(s, want, "the member reading {id}: {v}");
    }
    let (s, v) = send(&fx.app, "GET", &format!("/v1/generations/{theirs}"), Auth::Session(&member, Some(&fx.org)), None, &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "the same rule as the generation itself: {v}");
    for id in [&theirs, &failed, &own] {
        let (s, v) = send(&fx.app, "GET", &format!("/v1/logs/{id}/artifact"), Auth::Session(&fx.owner, Some(&fx.org)), None, &[]).await;
        assert_eq!(s, StatusCode::OK, "the owner reads any, {id}: {v}");
    }
}
