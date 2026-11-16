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
        sign_in_epoch: 0,
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

/// N1 (leftovers 2026-11-14): a provider's extra headers carry a gateway's
/// own auth (`cf-aig-authorization`), and the provider view shows only their
/// names. The `provider.configure` audit entry kept their values, and the
/// audit log is read more widely than the providers are: until item 11
/// (2026-11-15) the default org's Owners and Admins read the hosted master
/// key's entries, and backups and CSV exports keep them. The entry keeps each
/// header's name and never its value, in the JSON list and the CSV export
/// that the master key reads in hosted mode.
#[tokio::test]
async fn the_audit_log_keeps_provider_header_names_but_never_their_values() {
    let fx = fixture().await;
    let token = "cf-aig-token-5521-very-secret";
    let headers = json!({ "cf-aig-authorization": format!("Bearer {token}"), "x-team": "growth" });
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(json!({ "extra_headers": headers, "api_base": "https://gateway.example/v1" })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // The entry is written in the background.
    let mut entry = Value::Null;
    for _ in 0..200 {
        let (s, page) = send(&fx.app, "GET", "/v1/audit?action=provider.configure", Auth::Bearer(MASTER), None, &[]).await;
        assert_eq!(s, StatusCode::OK, "{page}");
        if page["total"] == 1 {
            entry = page["data"][0].clone();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(entry["target_id"], "mock", "the master key reads its entry: {entry}");
    let after: Value = serde_json::from_str(entry["after_json"].as_str().expect("the entry records the settings")).unwrap();
    assert_eq!(after["extra_headers"], json!({ "cf-aig-authorization": "[redacted]", "x-team": "[redacted]" }), "{entry}");
    assert_eq!(after["api_base"], "https://gateway.example/v1", "the rest of the settings stay as they are");

    let (s, csv) = send(&fx.app, "GET", "/v1/audit?format=csv", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    let csv = csv.as_str().expect("a CSV body");
    assert!(csv.contains("provider.configure") && csv.contains("cf-aig-authorization"), "{csv}");
    assert!(!csv.contains(token) && !csv.contains("growth"), "no header value in the export: {csv}");
}

/// N1 follow-up (2026-11-14): an alert webhook URL can carry its secret in
/// the path (Slack, Discord) or in the host (Pipedream, an AWS Lambda
/// function URL). `GET /v1/admin/settings` shows it to the gateway's admins
/// only, but the `settings.update` audit entry is read more widely: until
/// item 11 (2026-11-15) by the default org's Owners and Admins, and by
/// whoever holds a backup or a CSV export. The entry says the URL was set,
/// as `[redacted]`, and keeps none of it.
#[tokio::test]
async fn the_audit_log_never_keeps_the_alert_webhook_url() {
    let fx = fixture().await;
    let secret = "eo3secret77";
    let url = format!("https://{secret}.m.pipedream.net/hook?token=abc");
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "alert_webhook_url": url, "cost_markup_percent": 12.5 })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // The entry is written in the background.
    let mut entry = Value::Null;
    for _ in 0..200 {
        let (s, page) = send(&fx.app, "GET", "/v1/audit?action=settings.update", Auth::Bearer(MASTER), None, &[]).await;
        assert_eq!(s, StatusCode::OK, "{page}");
        if page["total"] == 1 {
            entry = page["data"][0].clone();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let after: Value = serde_json::from_str(entry["after_json"].as_str().expect("the entry records the settings")).unwrap();
    assert_eq!(after["alert_webhook_url"], "[redacted]", "{entry}");
    assert_eq!(after["cost_markup_percent"], 12.5, "the rest of the settings stay as they are");

    let (s, csv) = send(&fx.app, "GET", "/v1/audit?format=csv", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    let csv = csv.as_str().expect("a CSV body");
    assert!(csv.contains("settings.update") && csv.contains("[redacted]"), "{csv}");
    assert!(!csv.contains(secret) && !csv.contains("pipedream"), "no part of the URL in the export: {csv}");
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

/// provider-secrets (2026-11-15): today's PUT rule, held once the header
/// values are kept with the encrypted keys. Each PUT replaces the settings
/// whole: sent, `extra_headers` replaces the whole map; left out, the runtime
/// headers are gone (the env/YAML ones apply again). Only the keys, left out,
/// keep their stored value.
#[tokio::test]
async fn a_put_replaces_the_extra_headers_and_one_without_them_drops_them() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let put = |body: Value| send(&fx.app, "PUT", "/v1/admin/providers/openai", Auth::Bearer(MASTER), Some(body), &[]);
    let (s, v) = put(json!({ "api_key": "sk-provider-key-5521", "extra_headers": { "x-a": "1", "x-b": "2" } })).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    let (s, v) = put(json!({ "extra_headers": { "x-b": "3" } })).await;
    assert_eq!((s, &v["extra_header_names"]), (StatusCode::OK, &json!(["x-b"])), "sent, the map is replaced: {v}");
    let live = fx.state.router.registry.registered_config("openai").await.unwrap();
    assert_eq!(crate::management::runtime::sorted(&live.extra_headers), vec![("x-b".to_string(), "3".to_string())]);
    assert_eq!(live.api_key, "sk-provider-key-5521", "the key, left out, is kept");

    let (s, v) = put(json!({ "api_base": "https://gateway.example/v1" })).await;
    assert_eq!((s, &v["extra_header_names"]), (StatusCode::OK, &json!([])), "left out, the runtime headers are gone: {v}");
    assert!(fx.state.router.registry.registered_config("openai").await.unwrap().extra_headers.is_empty());
}

/// provider-secrets (2026-11-15): an extra header's value can be a gateway's
/// own auth (`cf-aig-authorization`, `Helicone-Auth`), but it was stored in
/// plain text in `provider_configs.settings_json` while the keys beside it
/// were encrypted, so a database dump, a backup or a read replica held it.
/// With a secrets key set and `encrypt_provider_credentials` on (since
/// provider-config, the key alone no longer does it), the row keeps each
/// header's name with `[redacted]`, and the provider still sends the value,
/// after a restart too.
#[tokio::test]
async fn with_encryption_on_extra_header_values_are_stored_encrypted() {
    let fx = fixture_with(Mode::Hosted, true).await;
    assert_eq!(switch_encryption(&fx, true).await.0, StatusCode::OK);
    let token = "cf-aig-token-8842-very-secret";
    let headers = json!({ "cf-aig-authorization": format!("Bearer {token}"), "x-team": "growth" });
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/providers/openai", Auth::Bearer(MASTER), Some(json!({ "api_key": "sk-provider-key-8842", "extra_headers": headers })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["extra_header_names"], json!(["cf-aig-authorization", "x-team"]));

    let row = fx.db.management().unwrap().get_provider_config("openai").await.unwrap().unwrap();
    let stored: Value = serde_json::from_str(&row.settings_json).unwrap();
    assert_eq!(stored["extra_headers"], json!({ "cf-aig-authorization": "[redacted]", "x-team": "[redacted]" }), "{}", row.settings_json);

    let sent = vec![("cf-aig-authorization".to_string(), format!("Bearer {token}")), ("x-team".to_string(), "growth".to_string())];
    let live = fx.state.router.registry.registered_config("openai").await.unwrap();
    assert_eq!(crate::management::runtime::sorted(&live.extra_headers), sent, "the provider sends the values");
    let restarted = build_state(fx.db.clone(), Mode::Hosted, Some([7u8; 32])).await;
    crate::management::runtime::apply_at_boot(&restarted).await.unwrap();
    let live = restarted.router.registry.registered_config("openai").await.unwrap();
    assert_eq!(crate::management::runtime::sorted(&live.extra_headers), sent, "and does after a restart");
}

/// What this thread logs, at every level, while the guard lives (a
/// `#[tokio::test]` runs on one thread), with the process-wide subscriber held
/// first so no callsite a test wants is off (`util::test_logs`).
#[derive(Clone, Default)]
struct Logs(Arc<std::sync::Mutex<Vec<u8>>>);

impl Logs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for Logs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
    type Writer = Logs;
    fn make_writer(&'a self) -> Logs {
        self.clone()
    }
}

fn capture_logs() -> (Logs, tracing::subscriber::DefaultGuard) {
    crate::util::test_logs::hold_process_subscriber();
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt().with_writer(logs.clone()).with_ansi(false).with_max_level(tracing::Level::TRACE).finish();
    (logs, tracing::subscriber::set_default(subscriber))
}

/// provider-secrets (2026-11-15): without a secrets key, the admin API takes
/// extra headers as before and stores their values as set, in plain text,
/// and the provider sends them. Each start names the configs that keep
/// values in plain text and the variable that changes it, never a value.
#[tokio::test]
async fn without_a_secrets_key_header_values_stay_as_set_and_each_start_warns() {
    let fx = fixture().await; // no secrets key
    let headers = json!({ "Helicone-Auth": "Bearer plain-token-3317" });
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(json!({ "extra_headers": headers })), &[]).await;
    assert_eq!((s, &v["extra_header_names"]), (StatusCode::OK, &json!(["Helicone-Auth"])), "{v}");
    let row = fx.db.management().unwrap().get_provider_config("mock").await.unwrap().unwrap();
    assert_eq!((row.ciphertext, serde_json::from_str::<Value>(&row.settings_json).unwrap()), (None, json!({ "extra_headers": headers })));

    let (logs, _guard) = capture_logs();
    let restarted = build_state(fx.db.clone(), Mode::Hosted, None).await;
    crate::management::runtime::apply_at_boot(&restarted).await.unwrap();
    let text = logs.text();
    let warning = text.lines().find(|l| l.contains("WARN") && l.contains("LITEGEN__SECRETS_KEY")).unwrap_or_else(|| panic!("no warning names LITEGEN__SECRETS_KEY: {text}"));
    assert!(warning.contains("mock") && warning.contains("plain text"), "it names the provider: {warning}");
    assert!(!text.contains("plain-token-3317"), "and never a value: {text}");
    let live = restarted.router.registry.registered_config("mock").await.unwrap();
    assert_eq!(crate::management::runtime::sorted(&live.extra_headers), vec![("Helicone-Auth".to_string(), "Bearer plain-token-3317".to_string())]);
}

/// provider-secrets (2026-11-15): an api_base can carry a credential in its
/// user info (`https://user:pass@proxy`, a proxy's basic auth). The provider
/// view showed it whole; it shows `[redacted]` in its place, and the
/// provider still calls the whole URL.
#[tokio::test]
async fn the_provider_view_shows_an_api_bases_user_info_as_redacted() {
    let fx = fixture().await;
    let base = "https://proxy-user:proxy-pass-4410@gateway.example/v1";
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(json!({ "api_base": base })), &[]).await;
    assert_eq!((s, v["api_base"].as_str()), (StatusCode::OK, Some("https://[redacted]@gateway.example/v1")), "{v}");
    let (_, list) = send(&fx.app, "GET", "/v1/admin/providers", Auth::Bearer(MASTER), None, &[]).await;
    let text = list.to_string();
    assert!(!text.contains("proxy-pass-4410") && !text.contains("proxy-user"), "{text}");
    let live = fx.state.router.registry.registered_config("mock").await.unwrap();
    assert_eq!(live.api_base.as_deref(), Some(base), "the provider calls the whole URL");
}

/// provider-secrets (2026-11-15): the dashboard's Configure form fills its
/// API base field from the view and sends it back with every save. Sent back
/// as shown, with `[redacted]` for the user info, the api_base in force stays
/// whole; any other api_base is taken as sent.
#[tokio::test]
async fn an_api_base_sent_back_as_shown_keeps_its_user_info() {
    let fx = fixture().await;
    let put = |body: Value| send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(body), &[]);
    let api_base = || async { fx.state.router.registry.registered_config("mock").await.unwrap().api_base };
    let base = "https://proxy-user:proxy-pass-4410@gateway.example/v1";
    assert_eq!(put(json!({ "api_base": base })).await.0, StatusCode::OK);
    let (s, v) = put(json!({ "api_base": "https://[redacted]@gateway.example/v1", "enabled": true })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(api_base().await.as_deref(), Some(base), "sent back as shown, the URL in force stays");
    assert_eq!(put(json!({ "api_base": "https://gateway.example/v2" })).await.0, StatusCode::OK);
    assert_eq!(api_base().await.as_deref(), Some("https://gateway.example/v2"), "another api_base is taken as sent");
}

/// provider-secrets (2026-11-15): an api_base that keeps the view's
/// `[redacted]` for its user info but isn't the one in force (a path edited
/// on the dashboard) would send `[redacted]` to the proxy as the user name.
/// It is refused, naming the field, and the URL in force stays.
#[tokio::test]
async fn an_edited_api_base_that_keeps_redacted_user_info_is_refused() {
    let fx = fixture().await;
    let put = |body: Value| send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(body), &[]);
    let base = "https://proxy-user:proxy-pass-4410@gateway.example/v1";
    assert_eq!(put(json!({ "api_base": base })).await.0, StatusCode::OK);
    let (s, b) = put(json!({ "api_base": "https://[redacted]@gateway.example/v2" })).await;
    assert_eq!((s, err_type(&b)), (StatusCode::BAD_REQUEST, "invalid_provider_config"), "{b}");
    assert!(b["error"]["message"].as_str().unwrap_or("").contains("api_base"), "{b}");
    assert_eq!(fx.state.router.registry.registered_config("mock").await.unwrap().api_base.as_deref(), Some(base), "the URL in force stays");
}

/// provider-config (2026-11-15): a URL in `options` (fal's
/// `platform_api_base`, Bedrock's `s3_api_base`) can carry a credential in
/// its user info as an api_base can, and the provider view showed it whole.
/// It shows `[redacted]` in its place, and the provider still calls the
/// whole URL.
#[tokio::test]
async fn the_provider_view_shows_the_user_info_of_option_urls_as_redacted() {
    let fx = fixture().await;
    let url = "https://fal-user:fal-pass-5562@proxy.example/fal-platform";
    let body = json!({ "options": { "platform_api_base": url, "retries": 2 } });
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(body), &[]).await;
    assert_eq!((s, &v["options"]), (StatusCode::OK, &json!({ "platform_api_base": "https://[redacted]@proxy.example/fal-platform", "retries": 2 })), "{v}");
    let (_, list) = send(&fx.app, "GET", "/v1/admin/providers", Auth::Bearer(MASTER), None, &[]).await;
    assert!(!list.to_string().contains("fal-pass-5562"), "{list}");
    let live = fx.state.router.registry.registered_config("mock").await.unwrap();
    assert_eq!(live.options.map(|o| o["platform_api_base"].clone()), Some(json!(url)), "the provider calls the whole URL");
}

/// provider-config (2026-11-15): options sent back as the view shows them
/// keep each URL in force, user info included, as an api_base does; an
/// option URL that keeps `[redacted]` for its user info but isn't the one in
/// force is refused, naming the option.
#[tokio::test]
async fn an_option_url_sent_back_as_shown_keeps_its_user_info_and_an_edited_one_is_refused() {
    let fx = fixture().await;
    let put = |body: Value| send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(body), &[]);
    let url = "https://fal-user:fal-pass-5562@proxy.example/fal-platform";
    let option = || async { fx.state.router.registry.registered_config("mock").await.unwrap().options.map(|o| o["platform_api_base"].clone()) };
    assert_eq!(put(json!({ "options": { "platform_api_base": url } })).await.0, StatusCode::OK);
    let (s, v) = put(json!({ "options": { "platform_api_base": "https://[redacted]@proxy.example/fal-platform" } })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(option().await, Some(json!(url)), "sent back as shown, the URL in force stays");
    let (s, b) = put(json!({ "options": { "platform_api_base": "https://[redacted]@other.example/fal-platform" } })).await;
    assert_eq!((s, err_type(&b)), (StatusCode::BAD_REQUEST, "invalid_provider_config"), "{b}");
    assert!(b["error"]["message"].as_str().unwrap_or("").contains("options.platform_api_base"), "{b}");
    assert_eq!(option().await, Some(json!(url)), "the URL in force stays");
}

// ─── Gateway admin: provider credentials at rest ────────────────────────────

/// Turn `encrypt_provider_credentials` on or off with a save of the gateway
/// settings, as an operator does.
async fn switch_encryption(fx: &Fx, on: bool) -> (StatusCode, Value) {
    send(&fx.app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "encrypt_provider_credentials": on })), &[]).await
}

/// A row as versions before 2026-11-15 read it (dfdfda7's `decrypt_secrets`
/// and `parse_settings`): its ciphertext decrypted, which they parse as the
/// keys, and `settings_json` as stored.
fn as_older_versions_read(row: &StoredProviderConfig) -> (Option<String>, Value) {
    let keys = row.ciphertext.as_deref().zip(row.nonce.as_deref()).map(|(ct, nonce)| {
        String::from_utf8(crate::auth::secrets::decrypt(&[7u8; 32], ct, nonce).expect("the row's secrets decrypt")).unwrap()
    });
    (keys, serde_json::from_str(&row.settings_json).expect("settings_json is JSON"))
}

/// What a version before 2026-11-15 encrypts for an API key alone.
fn keys_alone(api_key: &str) -> Option<String> {
    Some(serde_json::to_string(&ProviderSecrets { api_key: Some(api_key.to_string()), ..Default::default() }).unwrap())
}

/// Settings with one credential of each kind, which a version before
/// 2026-11-15 reads from `settings_json` and sends: a header's value, an
/// api_base's user info and an option URL's.
fn credentials_6631() -> Value {
    json!({
        "api_base": "https://proxy-user:proxy-pass-6631@gateway.example/v1",
        "extra_headers": { "cf-aig-authorization": "Bearer cf-aig-token-6631" },
        "options": { "platform_api_base": "https://fal-user:fal-pass-6631@proxy.example/fal-platform" },
    })
}

/// A stored provider row's provider, ciphertext, nonce, settings_json and updated_at.
type RowColumns = (String, Option<String>, Option<String>, String, chrono::DateTime<chrono::Utc>);

/// Every stored provider row's columns, by provider, to show what a step rewrote.
async fn provider_rows(fx: &Fx) -> Vec<RowColumns> {
    let rows = fx.db.management().unwrap().list_provider_configs().await.unwrap();
    let mut rows: Vec<_> = rows.into_iter().map(|r| (r.provider, r.ciphertext, r.nonce, r.settings_json, r.updated_at)).collect();
    rows.sort();
    rows
}

/// provider-config (2026-11-15, review #1): with `encrypt_provider_credentials`
/// off, the default, a save stores the row exactly as versions before
/// 2026-11-15 did, so an older replica (a rolling upgrade, a rollback) reads
/// it whole: the ciphertext holds the keys alone and `settings_json` the
/// settings as set. Under provider-secrets' rule a secrets key alone
/// encrypted the header values, and an older replica then sent `[redacted]`.
#[tokio::test]
async fn with_encryption_off_a_save_stores_the_row_as_older_versions_read_it() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let settings = credentials_6631();
    let mut body = settings.clone();
    body["api_key"] = json!("sk-provider-key-6631");
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/providers/openai", Auth::Bearer(MASTER), Some(body), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let row = fx.db.management().unwrap().get_provider_config("openai").await.unwrap().unwrap();
    assert_eq!(as_older_versions_read(&row), (keys_alone("sk-provider-key-6631"), settings));
}

/// provider-config (2026-11-15): turning `encrypt_provider_credentials` on
/// encrypts the credentials of every stored row once, on the replica that
/// takes the save: each header's value and the user info of api_base and of
/// option URLs go into the ciphertext, and the row keeps `[redacted]` in
/// their place and its `updated_at` (the config is the same). The provider
/// still sends the values, after a restart too. Saving the settings again
/// finds nothing to change and rewrites nothing.
#[tokio::test]
async fn turning_encryption_on_encrypts_the_stored_credentials_once() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let mut openai = credentials_6631();
    openai["api_key"] = json!("sk-provider-key-6631");
    for (name, body) in [("openai", openai), ("mock", json!({ "extra_headers": { "Helicone-Auth": "Bearer helicone-token-6631" } }))] {
        let (s, v) = send(&fx.app, "PUT", &format!("/v1/admin/providers/{name}"), Auth::Bearer(MASTER), Some(body), &[]).await;
        assert_eq!(s, StatusCode::OK, "{name}: {v}");
    }
    let plain = provider_rows(&fx).await;

    let (s, v) = switch_encryption(&fx, true).await;
    assert_eq!((s, &v["settings"]["encrypt_provider_credentials"]), (StatusCode::OK, &json!(true)), "{v}");
    let sealed = provider_rows(&fx).await;
    for ((name, _, _, settings_json, updated_at), (_, _, _, _, was)) in sealed.iter().zip(&plain) {
        for secret in ["cf-aig-token-6631", "proxy-pass-6631", "fal-pass-6631", "helicone-token-6631"] {
            assert!(!settings_json.contains(secret), "{name}: {settings_json}");
        }
        assert!(settings_json.contains("[redacted]"), "{name} keeps each name: {settings_json}");
        assert_eq!(updated_at, was, "{name}: the config itself is unchanged");
    }
    let openai_row: Value = serde_json::from_str(&sealed.iter().find(|r| r.0 == "openai").unwrap().3).unwrap();
    assert_eq!(openai_row["api_base"], "https://[redacted]@gateway.example/v1");
    assert_eq!(openai_row["options"]["platform_api_base"], "https://[redacted]@proxy.example/fal-platform");

    let sends_the_values = |state: Arc<AppState>| async move {
        let openai = state.router.registry.registered_config("openai").await.unwrap();
        assert_eq!(crate::management::runtime::sorted(&openai.extra_headers), vec![("cf-aig-authorization".to_string(), "Bearer cf-aig-token-6631".to_string())]);
        assert_eq!(openai.api_base.as_deref(), Some("https://proxy-user:proxy-pass-6631@gateway.example/v1"));
        assert_eq!(openai.options.as_ref().map(|o| o["platform_api_base"].clone()), Some(json!("https://fal-user:fal-pass-6631@proxy.example/fal-platform")));
        assert_eq!(openai.api_key, "sk-provider-key-6631");
        let mock = state.router.registry.registered_config("mock").await.unwrap();
        assert_eq!(crate::management::runtime::sorted(&mock.extra_headers), vec![("Helicone-Auth".to_string(), "Bearer helicone-token-6631".to_string())]);
    };
    sends_the_values(fx.state.clone()).await;
    let restarted = build_state(fx.db.clone(), Mode::Hosted, Some([7u8; 32])).await;
    crate::management::runtime::apply_at_boot(&restarted).await.unwrap();
    sends_the_values(restarted).await;

    assert_eq!(switch_encryption(&fx, true).await.0, StatusCode::OK);
    assert_eq!(provider_rows(&fx).await, sealed, "a second save rewrote a row (encrypting again draws a new nonce)");
}

/// provider-config (2026-11-15): turning `encrypt_provider_credentials` off,
/// here with a save of the settings that leaves it out, stores every row as
/// versions before 2026-11-15 read them, the step before a rollback: the
/// values back in `settings_json`, the keys alone in the ciphertext,
/// `updated_at` kept. The provider goes on sending the same values.
#[tokio::test]
async fn turning_encryption_off_stores_the_rows_as_older_versions_read_them() {
    let fx = fixture_with(Mode::Hosted, true).await;
    assert_eq!(switch_encryption(&fx, true).await.0, StatusCode::OK);
    let settings = credentials_6631();
    let mut body = settings.clone();
    body["api_key"] = json!("sk-provider-key-6631");
    assert_eq!(send(&fx.app, "PUT", "/v1/admin/providers/openai", Auth::Bearer(MASTER), Some(body), &[]).await.0, StatusCode::OK);
    let sealed = fx.db.management().unwrap().get_provider_config("openai").await.unwrap().unwrap();
    assert!(!sealed.settings_json.contains("cf-aig-token-6631"), "stored encrypted first: {}", sealed.settings_json);

    // As the dashboard turns it off: `null`.
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "cost_markup_percent": 5.0, "encrypt_provider_credentials": null })), &[]).await;
    assert_eq!((s, &v["settings"]["encrypt_provider_credentials"]), (StatusCode::OK, &Value::Null), "{v}");
    let row = fx.db.management().unwrap().get_provider_config("openai").await.unwrap().unwrap();
    assert_eq!(as_older_versions_read(&row), (keys_alone("sk-provider-key-6631"), settings));
    assert_eq!(row.updated_at, sealed.updated_at, "the config itself is unchanged");
    let live = fx.state.router.registry.registered_config("openai").await.unwrap();
    assert_eq!(crate::management::runtime::sorted(&live.extra_headers), vec![("cf-aig-authorization".to_string(), "Bearer cf-aig-token-6631".to_string())]);
}

/// provider-config follow-up (2026-11-15): unlike the other gateway settings,
/// `encrypt_provider_credentials` left out of a save keeps its stored value.
/// A client written before the field existed saves the settings without it,
/// and that save turned the encryption off and stored every credential in
/// plain text again, without a word. Only `null` or `false` turns it off.
#[tokio::test]
async fn a_settings_save_that_leaves_the_encryption_out_keeps_it() {
    let fx = fixture_with(Mode::Hosted, true).await;
    assert_eq!(switch_encryption(&fx, true).await.0, StatusCode::OK);
    let body = json!({ "extra_headers": { "Helicone-Auth": "Bearer helicone-token-7302" } });
    assert_eq!(send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(body), &[]).await.0, StatusCode::OK);
    let sealed = provider_rows(&fx).await;

    let (s, v) = send(&fx.app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "cost_markup_percent": 5.0 })), &[]).await;
    assert_eq!((s, &v["settings"]["encrypt_provider_credentials"]), (StatusCode::OK, &json!(true)), "{v}");
    assert_eq!(provider_rows(&fx).await, sealed, "the rows stay encrypted");
}

/// provider-config (2026-11-15): with the credentials encrypted, the row
/// keeps `https://[redacted]@host/v1` for an api_base with user info, so the
/// URL in force is the one the ciphertext holds. A dashboard save that sends
/// the shown api_base back keeps that URL whole, and the provider goes on
/// calling it.
#[tokio::test]
async fn with_encryption_on_an_api_base_sent_back_as_shown_keeps_its_user_info() {
    let fx = fixture_with(Mode::Hosted, true).await;
    assert_eq!(switch_encryption(&fx, true).await.0, StatusCode::OK);
    let put = |body: Value| send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(body), &[]);
    let base = "https://proxy-user:proxy-pass-2290@gateway.example/v1";
    assert_eq!(put(json!({ "api_base": base })).await.0, StatusCode::OK);
    let (s, v) = put(json!({ "api_base": "https://[redacted]@gateway.example/v1", "enabled": true })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(fx.state.router.registry.registered_config("mock").await.unwrap().api_base.as_deref(), Some(base), "the URL in force stays");
    let row = fx.db.management().unwrap().get_provider_config("mock").await.unwrap().unwrap();
    assert!(!row.settings_json.contains("proxy-pass-2290"), "and stays encrypted: {}", row.settings_json);
}

/// provider-config (2026-11-15): a save on a replica that hasn't heard of a
/// switch yet still ends as the stored setting says. The switch's pass may
/// have read the rows before this save wrote its own, so the save reads the
/// setting again after its write and stores its row again when the setting
/// changed meanwhile. Otherwise a save racing a switch to off would leave an
/// encrypted row behind for a rollback.
#[tokio::test]
async fn a_save_that_races_the_switch_ends_as_the_stored_setting_says() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let store = fx.db.management().unwrap();
    let stored_headers = || async { serde_json::from_str::<Value>(&store.get_provider_config("mock").await.unwrap().unwrap().settings_json).unwrap()["extra_headers"].clone() };
    let headers = json!({ "Helicone-Auth": "Bearer race-token-3301" });
    let put = || send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(json!({ "extra_headers": headers })), &[]);

    // This replica still holds "on"; another one has just stored "off".
    fx.state.router.update_overrides(|o| o.gateway.encrypt_provider_credentials = Some(Some(true)));
    store.set_setting(crate::management::runtime::SETTINGS_KEY, "{}").await.unwrap();
    assert_eq!(put().await.0, StatusCode::OK);
    assert_eq!(stored_headers().await, headers, "stored as older versions read it");

    // The other way round: this replica still holds "off"; the stored setting is "on".
    fx.state.router.update_overrides(|o| o.gateway.encrypt_provider_credentials = None);
    store.set_setting(crate::management::runtime::SETTINGS_KEY, r#"{"encrypt_provider_credentials":true}"#).await.unwrap();
    assert_eq!(put().await.0, StatusCode::OK);
    assert_eq!(stored_headers().await, json!({ "Helicone-Auth": "[redacted]" }), "stored encrypted");
    let live = fx.state.router.registry.registered_config("mock").await.unwrap();
    assert_eq!(crate::management::runtime::sorted(&live.extra_headers), vec![("Helicone-Auth".to_string(), "Bearer race-token-3301".to_string())]);
}

/// provider-config (2026-11-15): two saves of the settings that switch the
/// encryption opposite ways at once. Each stores the rows after it stored its
/// setting, and the later pass may be the one whose setting lost, which left
/// the rows otherwise than the setting that stands, encrypted while it is off
/// in the worst case, which an older version can't read. So a pass reads the
/// setting again when it ends and passes again for the one that stands. Here
/// this save stored "on" and another one has since stored "off": the rows end
/// as older versions read them.
#[tokio::test]
async fn after_two_saves_at_once_the_rows_follow_the_setting_that_stands() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let headers = json!({ "Helicone-Auth": "Bearer helicone-token-6120" });
    assert_eq!(send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(json!({ "extra_headers": headers })), &[]).await.0, StatusCode::OK);
    let store = fx.db.management().unwrap();
    store.set_setting(crate::management::runtime::SETTINGS_KEY, "{}").await.unwrap();

    let (stored, _) = crate::management::runtime::store_configs_after_save(&fx.state, store, true).await.unwrap();
    let row = store.get_provider_config("mock").await.unwrap().unwrap();
    assert_eq!((stored, serde_json::from_str::<Value>(&row.settings_json).unwrap()["extra_headers"].clone()), (false, headers));
}

/// provider-config (2026-11-15): encrypting provider credentials needs the
/// secrets key, so turning it on without one is refused and nothing is
/// stored.
#[tokio::test]
async fn encrypting_provider_credentials_needs_a_secrets_key() {
    let fx = fixture().await; // no secrets key
    let (s, b) = switch_encryption(&fx, true).await;
    assert_eq!((s, err_type(&b)), (StatusCode::BAD_REQUEST, "secrets_key_missing"), "{b}");
    assert!(b["error"]["message"].as_str().unwrap_or("").contains("LITEGEN__SECRETS_KEY"), "{b}");
    let (_, v) = send(&fx.app, "GET", "/v1/admin/settings", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(v["settings"]["encrypt_provider_credentials"], Value::Null, "{v}");
}

/// provider-config (2026-11-15): a save that switches the encryption logs one
/// line and writes one audit entry saying how many configs it stored again,
/// and which, never a value.
#[tokio::test]
async fn switching_encryption_logs_and_audits_how_many_configs_were_stored_again() {
    let fx = fixture_with(Mode::Hosted, true).await;
    for name in ["mock", "openai"] {
        let body = json!({ "api_key": "sk-provider-key-4471", "extra_headers": { "x-gateway-token": "token-4471" } });
        assert_eq!(send(&fx.app, "PUT", &format!("/v1/admin/providers/{name}"), Auth::Bearer(MASTER), Some(body), &[]).await.0, StatusCode::OK);
    }
    let (logs, _guard) = capture_logs();
    assert_eq!(switch_encryption(&fx, true).await.0, StatusCode::OK);
    let entry = wait_for_audit(&fx.db, "provider.encrypt_credentials").await;
    let after: Value = serde_json::from_str(entry.after_json.as_deref().expect("the entry says what was done")).unwrap();
    assert_eq!(after, json!({ "encrypt_provider_credentials": true, "configs_stored_again": 2, "providers": ["mock", "openai"] }));
    let text = logs.text();
    let line = text.lines().find(|l| l.contains("encrypt_provider_credentials") && l.contains("stored again")).unwrap_or_else(|| panic!("no line says what the switch did: {text}"));
    assert!(line.contains("configs_stored_again=2"), "{line}");
    assert!(!text.contains("token-4471"), "never a value: {text}");

    assert_eq!(switch_encryption(&fx, false).await.0, StatusCode::OK);
    let entry = wait_for_audit(&fx.db, "provider.decrypt_credentials").await;
    let after: Value = serde_json::from_str(entry.after_json.as_deref().expect("the entry says what was done")).unwrap();
    assert_eq!(after, json!({ "encrypt_provider_credentials": false, "configs_stored_again": 2, "providers": ["mock", "openai"] }));
}

// ─── Gateway admin: PATCH a provider ────────────────────────────────────────

async fn patch_provider(fx: &Fx, name: &str, body: Value) -> (StatusCode, Value) {
    send(&fx.app, "PATCH", &format!("/v1/admin/providers/{name}"), Auth::Bearer(MASTER), Some(body), &[]).await
}

/// provider-config (2026-11-15, a customer's bug): the dashboard's Configure
/// form sends only the key, the api_base and the enabled flag, and a PUT
/// replaces the settings whole, so each dashboard save dropped the
/// extra_headers, model_mapping, options and region set through the API.
/// A PATCH sets what it sends and keeps every field it leaves out, and its
/// audit entry records the config it leaves, header values redacted.
#[tokio::test]
async fn a_patch_keeps_what_it_does_not_send() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let full = json!({
        "api_key": "sk-provider-key-1180",
        "api_base": "https://gateway.example/v1",
        "model_mapping": { "openai/gpt-image-2": "my-deployment" },
        "extra_headers": { "cf-aig-authorization": "Bearer cf-aig-token-1180" },
        "options": { "platform_api_base": "https://gateway.example/platform" },
        "region": "eu-west-1",
    });
    assert_eq!(send(&fx.app, "PUT", "/v1/admin/providers/openai", Auth::Bearer(MASTER), Some(full), &[]).await.0, StatusCode::OK);

    // As the dashboard's Configure form saves: a new key, the api_base, enabled.
    let (s, v) = patch_provider(&fx, "openai", json!({ "api_key": "sk-provider-key-2290", "api_base": "https://gateway.example/v2", "enabled": true })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        (v["api_base"].as_str(), v["region"].as_str(), &v["extra_header_names"], &v["model_mapping"], &v["options"]),
        (
            Some("https://gateway.example/v2"),
            Some("eu-west-1"),
            &json!(["cf-aig-authorization"]),
            &json!({ "openai/gpt-image-2": "my-deployment" }),
            &json!({ "platform_api_base": "https://gateway.example/platform" })
        ),
        "{v}"
    );
    let live = fx.state.router.registry.registered_config("openai").await.unwrap();
    assert_eq!(live.api_key, "sk-provider-key-2290");
    assert_eq!(crate::management::runtime::sorted(&live.extra_headers), vec![("cf-aig-authorization".to_string(), "Bearer cf-aig-token-1180".to_string())]);
    assert_eq!(live.credentials.region.as_deref(), Some("eu-west-1"));

    let mut audited = Vec::new();
    for _ in 0..200 {
        let (_, page) = send(&fx.app, "GET", "/v1/audit?action=provider.configure", Auth::Bearer(MASTER), None, &[]).await;
        audited = page["data"].as_array().cloned().unwrap_or_default();
        if audited.len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let after: Value = audited
        .iter()
        .filter_map(|e| serde_json::from_str::<Value>(e["after_json"].as_str()?).ok())
        .find(|a| a["api_base"] == "https://gateway.example/v2")
        .unwrap_or_else(|| panic!("no entry for the PATCH: {audited:?}"));
    assert_eq!((&after["region"], &after["extra_headers"]), (&json!("eu-west-1"), &json!({ "cf-aig-authorization": "[redacted]" })), "{after}");
}

/// provider-config (2026-11-15): in a PATCH, `null` clears a field, so it
/// falls back to the env/YAML config, and a field sent with a value replaces
/// the stored one whole, a map too. A key sent as `null` keeps its stored
/// value.
#[tokio::test]
async fn a_patch_clears_a_field_sent_as_null_and_keeps_a_key_sent_null() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let full = json!({
        "api_key": "sk-provider-key-3370",
        "region": "eu-west-1",
        "model_mapping": { "openai/gpt-image-2": "my-deployment" },
        "extra_headers": { "x-a": "1", "x-b": "2" },
    });
    assert_eq!(send(&fx.app, "PUT", "/v1/admin/providers/openai", Auth::Bearer(MASTER), Some(full), &[]).await.0, StatusCode::OK);
    let (s, v) = patch_provider(&fx, "openai", json!({ "region": null, "extra_headers": { "x-c": "3" }, "api_key": null })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((&v["region"], &v["extra_header_names"], &v["model_mapping"]), (&Value::Null, &json!(["x-c"]), &json!({ "openai/gpt-image-2": "my-deployment" })), "{v}");
    let live = fx.state.router.registry.registered_config("openai").await.unwrap();
    assert_eq!(live.api_key, "sk-provider-key-3370", "a key sent null keeps its value");
    assert_eq!(crate::management::runtime::sorted(&live.extra_headers), vec![("x-c".to_string(), "3".to_string())]);
    assert_eq!(live.credentials.region, None);
}

/// provider-config follow-up (2026-11-15): a stored key could only go with the
/// whole runtime config (DELETE). In a PATCH, a key sent as `""` removes that
/// key alone, so the env/YAML one applies again (here there is none, so the
/// provider has no key and is out of service); the row keeps no ciphertext
/// once no key is left. A PUT keeps a key sent as `""`, as before.
#[tokio::test]
async fn a_patch_removes_a_key_sent_as_an_empty_string_and_a_put_keeps_it() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let put = |body: Value| send(&fx.app, "PUT", "/v1/admin/providers/openai", Auth::Bearer(MASTER), Some(body), &[]);
    assert_eq!(put(json!({ "api_key": "sk-provider-key-4410" })).await.0, StatusCode::OK);
    let (s, v) = put(json!({ "api_key": "", "region": "eu-west-1" })).await;
    assert_eq!((s, v["key_hint"].as_str()), (StatusCode::OK, Some("…4410")), "a PUT keeps a key sent as \"\": {v}");

    let (s, v) = patch_provider(&fx, "openai", json!({ "api_key": "" })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["has_secret"].as_bool(), &v["key_hint"], v["active"].as_bool(), v["region"].as_str()), (Some(false), &Value::Null, Some(false), Some("eu-west-1")), "{v}");
    let row = fx.db.management().unwrap().get_provider_config("openai").await.unwrap().unwrap();
    assert_eq!((row.ciphertext, row.display_hint), (None, None), "no key is left");
    assert!(fx.state.router.registry.registered_config("openai").await.is_none(), "no key, no provider");
}

/// provider-config (2026-11-15): a PATCH of a provider with no runtime config
/// creates one from the fields it sends, and makes PUT's checks: an unknown
/// provider is a 404; an api_base that isn't http(s), a header litegen can't
/// send, or an edited api_base that keeps `[redacted]` for its user info is a
/// 400; a key with no secrets key to encrypt it is refused.
#[tokio::test]
async fn a_patch_creates_a_runtime_config_and_makes_the_checks_a_put_makes() {
    let fx = fixture().await; // no secrets key
    let (s, v) = patch_provider(&fx, "mock", json!({ "extra_headers": { "x-team": "growth" } })).await;
    assert_eq!((s, v["source"].as_str(), &v["extra_header_names"]), (StatusCode::OK, Some("runtime"), &json!(["x-team"])), "{v}");
    assert_eq!(patch_provider(&fx, "acme-ai", json!({})).await.0, StatusCode::NOT_FOUND);
    for body in [
        json!({ "api_base": "ftp://nope" }),
        json!({ "extra_headers": { "Host": "evil.example" } }),
        json!({ "api_base": "https://[redacted]@gateway.example/v2" }),
    ] {
        let (s, b) = patch_provider(&fx, "mock", body.clone()).await;
        assert_eq!((s, err_type(&b)), (StatusCode::BAD_REQUEST, "invalid_provider_config"), "{body}: {b}");
    }
    let (s, b) = patch_provider(&fx, "openai", json!({ "api_key": "sk-123456789" })).await;
    assert_eq!((s, err_type(&b)), (StatusCode::BAD_REQUEST, "secrets_key_missing"), "{b}");
    assert_eq!(fx.state.router.registry.registered_config("mock").await.unwrap().extra_headers.get("x-team").map(String::as_str), Some("growth"), "the refused PATCHes changed nothing");
}

/// provider-config (2026-11-15): a PATCH stores its row as
/// `encrypt_provider_credentials` says, like a PUT, the credentials it keeps
/// included, and takes an api_base sent back as the view shows it as the
/// one in force: the dashboard's save with the setting on.
#[tokio::test]
async fn a_patch_with_encryption_on_encrypts_what_it_stores() {
    let fx = fixture_with(Mode::Hosted, true).await;
    assert_eq!(switch_encryption(&fx, true).await.0, StatusCode::OK);
    let base = "https://proxy-user:proxy-pass-8813@gateway.example/v1";
    let body = json!({ "api_base": base, "extra_headers": { "Helicone-Auth": "Bearer helicone-token-8813" } });
    assert_eq!(send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(body), &[]).await.0, StatusCode::OK);
    let (s, v) = patch_provider(&fx, "mock", json!({ "api_base": "https://[redacted]@gateway.example/v1", "enabled": true })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let row = fx.db.management().unwrap().get_provider_config("mock").await.unwrap().unwrap();
    assert!(!row.settings_json.contains("helicone-token-8813") && !row.settings_json.contains("proxy-pass-8813"), "{}", row.settings_json);
    let live = fx.state.router.registry.registered_config("mock").await.unwrap();
    assert_eq!(live.api_base.as_deref(), Some(base));
    assert_eq!(crate::management::runtime::sorted(&live.extra_headers), vec![("Helicone-Auth".to_string(), "Bearer helicone-token-8813".to_string())]);
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

/// provider-config (2026-11-15, review #1): a start neither encrypts nor
/// decrypts the stored rows; it follows the stored setting. This inverts
/// provider-secrets' `boot_moves_header_values_an_older_version_stored_in_plain_text`:
/// each start with a secrets key encrypted the header values stored in plain
/// text, so an older replica that reloaded afterwards (a rolling upgrade, a
/// rollback) sent `[redacted]` to the gateway. With the key set and
/// `encrypt_provider_credentials` off, a start says to turn it on once every
/// replica runs this version, naming the configs that keep credentials in
/// plain text, never a value.
#[tokio::test]
async fn a_start_leaves_the_stored_rows_as_they_are_and_says_how_to_encrypt_them() {
    let fx = fixture_with(Mode::Hosted, true).await;
    let store = fx.db.management().unwrap();
    let key = [7u8; 32];
    let at = chrono::Utc::now() - chrono::Duration::days(3);
    // As a version before 2026-11-15 stored them: the keys encrypted, the headers as set.
    let (ciphertext, nonce) = crate::auth::secrets::encrypt(&key, br#"{"api_key":"sk-legacy-key-7781"}"#).unwrap();
    for (provider, ciphertext, nonce, settings) in [
        ("openai", Some(ciphertext), Some(nonce), json!({ "api_base": "https://gateway.example/v1", "extra_headers": { "cf-aig-authorization": "Bearer legacy-token-openai" } })),
        // Headers and no key: the mock adapter needs none.
        ("mock", None, None, json!({ "extra_headers": { "Helicone-Auth": "Bearer legacy-token-mock" } })),
    ] {
        let row = StoredProviderConfig { provider: provider.into(), ciphertext, nonce, display_hint: None, settings_json: settings.to_string(), created_at: at, updated_at: at };
        store.upsert_provider_config(&row).await.unwrap();
    }
    let before = provider_rows(&fx).await;

    let (logs, _guard) = capture_logs();
    let booted = build_state(fx.db.clone(), Mode::Hosted, Some(key)).await;
    crate::management::runtime::apply_at_boot(&booted).await.unwrap();
    assert_eq!(provider_rows(&fx).await, before, "the start rewrote a row");
    let headers = |c: crate::providers::ProviderInstanceConfig| crate::management::runtime::sorted(&c.extra_headers);
    assert_eq!(headers(booted.router.registry.registered_config("openai").await.unwrap()), vec![("cf-aig-authorization".to_string(), "Bearer legacy-token-openai".to_string())]);
    assert_eq!(booted.router.registry.registered_config("openai").await.unwrap().api_key, "sk-legacy-key-7781");
    assert_eq!(headers(booted.router.registry.registered_config("mock").await.unwrap()), vec![("Helicone-Auth".to_string(), "Bearer legacy-token-mock".to_string())]);

    let text = logs.text();
    let notice = text.lines().find(|l| l.contains("encrypt_provider_credentials")).unwrap_or_else(|| panic!("no notice names encrypt_provider_credentials: {text}"));
    assert!(notice.contains("every replica") && notice.contains("openai") && notice.contains("mock"), "{notice}");
    assert!(!text.contains("legacy-token"), "and never a value: {text}");
}

/// provider-config (2026-11-15): rows that keep encrypted credentials while
/// `encrypt_provider_credentials` is off (it went off without the save that
/// stores the rows again, say a row written directly) are rows an older
/// version can't read: it would send `[redacted]`. A start names them and
/// says how to store them as older versions read them before a rollback,
/// and rewrites nothing itself.
#[tokio::test]
async fn a_start_names_the_configs_an_older_version_could_not_read() {
    let fx = fixture_with(Mode::Hosted, true).await;
    assert_eq!(switch_encryption(&fx, true).await.0, StatusCode::OK);
    let body = json!({ "extra_headers": { "Helicone-Auth": "Bearer sealed-token-5150" } });
    assert_eq!(send(&fx.app, "PUT", "/v1/admin/providers/mock", Auth::Bearer(MASTER), Some(body), &[]).await.0, StatusCode::OK);
    let store = fx.db.management().unwrap();
    store.set_setting(crate::management::runtime::SETTINGS_KEY, "{}").await.unwrap();
    let before = provider_rows(&fx).await;
    assert!(before[0].3.contains("[redacted]"), "stored encrypted: {}", before[0].3);

    let (logs, _guard) = capture_logs();
    let booted = build_state(fx.db.clone(), Mode::Hosted, Some([7u8; 32])).await;
    crate::management::runtime::apply_at_boot(&booted).await.unwrap();
    assert_eq!(provider_rows(&fx).await, before, "the start rewrote a row");
    let text = logs.text();
    let warning = text
        .lines()
        .find(|l| l.contains("WARN") && l.contains("mock") && l.contains("rollback"))
        .unwrap_or_else(|| panic!("no warning names the config an older version can't read: {text}"));
    assert!(warning.contains("PUT /v1/admin/settings"), "it says how to store them again: {warning}");
    assert!(!text.contains("sealed-token-5150"), "never a value: {text}");
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

// ─── Audit log: where an entry is filed, who reads it ───────────────────────
//
// Item 11 (2026-11-15). An org's Owners and Admins read the entries filed
// under it. In hosted mode the master key acts in no org, and
// `insert_audit_log` filed what it did there under the default org, whose
// members read it: the gateway's providers, settings, routes and models, and
// the accounts it changed. Those entries concern no org: they are filed
// under none, and only the master key reads them.

/// Audit entries are written in the background: wait until one for `action`
/// is stored, wherever it is filed.
async fn wait_for_audit(db: &SqliteDatabase, action: &str) -> crate::types::AuditLogEntry {
    let filter = crate::types::AuditLogFilter { action: Some(action.to_string()), ..Default::default() };
    for _ in 0..200 {
        if let Some(entry) = db.list_audit_log(&filter, 1, 1).await.unwrap().0.pop() {
            return entry;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("no {action} audit entry was written");
}

#[tokio::test]
async fn hosted_the_master_keys_gateway_and_account_changes_are_not_in_the_default_orgs_log() {
    let fx = fixture().await;
    let reader = seed_user(&fx.db, "owner@default.test").await;
    fx.db.add_org_member(DEFAULT_ORG_ID, &reader.user.id, Role::Owner).await.unwrap();
    let someone = seed_user(&fx.db, "someone@else.test").await;
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "cost_markup_percent": 3.0 })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = send(&fx.app, "PATCH", &format!("/v1/users/{}", someone.user.id), Auth::Bearer(MASTER), Some(json!({ "role": "viewer" })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let written = [wait_for_audit(&fx.db, "settings.update").await, wait_for_audit(&fx.db, "user.update").await];

    let (s, page) = send(&fx.app, "GET", "/v1/audit", Auth::Session(&reader, Some(DEFAULT_ORG_ID)), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "{page}");
    assert_eq!(page["total"], 0, "the default org's Owner reads none of them: {page}");
    let (s, csv) = send(&fx.app, "GET", "/v1/audit?format=csv", Auth::Session(&reader, Some(DEFAULT_ORG_ID)), None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    let csv = csv.as_str().expect("a CSV body");
    assert!(!csv.contains("settings.update") && !csv.contains("user.update"), "nor does the export: {csv}");
    for entry in written {
        assert_eq!(entry.org_id, None, "{}: filed under no org", entry.action);
    }
}

/// A hosted account that belongs to no org acts in none: its password
/// change was filed under the default org too, naming it to that org's
/// Owners and Admins. It is filed under no org.
#[tokio::test]
async fn hosted_an_account_change_by_a_person_in_no_org_is_not_in_the_default_orgs_log() {
    let fx = fixture().await;
    let reader = seed_user(&fx.db, "owner@default.test").await;
    fx.db.add_org_member(DEFAULT_ORG_ID, &reader.user.id, Role::Owner).await.unwrap();
    let loner = seed_user(&fx.db, "loner@nowhere.test").await;
    let hash = crate::auth::password::hash_password("old-password-123").unwrap();
    fx.db.update_user(&loner.user.id, None, None, Some(&hash)).await.unwrap();
    let body = json!({ "current_password": "old-password-123", "new_password": "new-password-456" });
    let (s, v) = send(&fx.app, "PATCH", "/v1/account", Auth::Session(&loner, None), Some(body), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let written = wait_for_audit(&fx.db, "account.change_password").await;

    let (s, page) = send(&fx.app, "GET", "/v1/audit", Auth::Session(&reader, Some(DEFAULT_ORG_ID)), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "{page}");
    assert_eq!(page["total"], 0, "the default org's Owner reads none of it: {page}");
    assert_eq!(written.org_id, None, "filed under no org");
}

/// Single-tenant: the default org is the deployment, and its Owners and
/// Admins are the operator's people, so the master key's entries stay in its
/// log, and the master key reads them there.
#[tokio::test]
async fn single_tenant_the_master_keys_gateway_changes_stay_in_the_default_orgs_log() {
    let d = deployment().await;
    let (s, v) = send(&d.app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "cost_markup_percent": 3.0 })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let written = wait_for_audit(&d.db, "settings.update").await;
    assert_eq!(written.org_id.as_deref(), Some(DEFAULT_ORG_ID));
    for (who, auth) in [("the owner", Auth::Session(&d.owner, Some(DEFAULT_ORG_ID))), ("the master key", Auth::Bearer(MASTER))] {
        let (s, page) = send(&d.app, "GET", "/v1/audit?action=settings.update", auth, None, &[]).await;
        assert_eq!(s, StatusCode::OK, "{who}: {page}");
        assert_eq!(page["total"], 1, "{who}: {page}");
    }
    let (s, csv) = send(&d.app, "GET", "/v1/audit?format=csv", Auth::Session(&d.owner, Some(DEFAULT_ORG_ID)), None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert!(csv.as_str().expect("a CSV body").contains("settings.update"), "{csv}");
}

/// The master key reads what is filed under no org, in the JSON list and the
/// CSV export, and never an org's own entries. Anyone else who acts in no
/// org reads none of it.
#[tokio::test]
async fn hosted_the_master_key_reads_the_entries_filed_under_no_org() {
    let fx = fixture().await;
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "cost_markup_percent": 3.0 })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // An entry of the org's own: its owner sets its budget.
    let (s, v) = send(&fx.app, "PUT", &format!("/v1/orgs/{}/limits", fx.org), Auth::Session(&fx.owner, Some(&fx.org)), Some(json!({ "max_budget_usd": 7.0 })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    wait_for_audit(&fx.db, "settings.update").await;
    wait_for_audit(&fx.db, "limits.update").await;

    let (s, page) = send(&fx.app, "GET", "/v1/audit", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "{page}");
    let actions: Vec<&str> = page["data"].as_array().expect("entries").iter().map(|e| e["action"].as_str().unwrap_or("")).collect();
    assert_eq!(actions, ["settings.update"], "only what is filed under no org: {page}");
    assert!(page["data"][0].get("org_id").is_none(), "{page}");
    let (s, csv) = send(&fx.app, "GET", "/v1/audit?format=csv", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    let csv = csv.as_str().expect("a CSV body");
    assert!(csv.contains("settings.update") && !csv.contains("limits.update"), "{csv}");

    let loner = seed_user(&fx.db, "loner@nowhere.test").await;
    let (s, v) = send(&fx.app, "GET", "/v1/audit", Auth::Session(&loner, None), None, &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "a person in no org: {v}");
}

/// An entry about an org's object is filed under that org, whoever made the
/// change: its Owners read the master key's change to their budget next to
/// their own key's changes, and the default org's Owner reads neither.
#[tokio::test]
async fn hosted_an_orgs_owner_reads_the_master_keys_changes_to_its_objects() {
    let fx = fixture().await;
    let reader = seed_user(&fx.db, "owner@default.test").await;
    fx.db.add_org_member(DEFAULT_ORG_ID, &reader.user.id, Role::Owner).await.unwrap();
    let (s, v) = send(&fx.app, "PUT", &format!("/v1/orgs/{}/limits", fx.org), Auth::Bearer(MASTER), Some(json!({ "max_budget_usd": 7.0 })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = send(&fx.app, "POST", &format!("/v1/keys/{}/block", fx.gen_key.id), Auth::Bearer(&fx.admin_key.raw), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(wait_for_audit(&fx.db, "limits.update").await.org_id.as_deref(), Some(fx.org.as_str()));
    wait_for_audit(&fx.db, "key.block").await;

    let (s, page) = send(&fx.app, "GET", "/v1/audit", Auth::Session(&fx.owner, Some(&fx.org)), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "{page}");
    let mut actions: Vec<&str> = page["data"].as_array().expect("entries").iter().map(|e| e["action"].as_str().unwrap_or("")).collect();
    actions.sort();
    assert_eq!(actions, ["key.block", "limits.update"], "{page}");
    let by_master = page["data"].as_array().unwrap().iter().find(|e| e["action"] == "limits.update").unwrap();
    assert_eq!(by_master["actor_label"], "master-key", "{page}");
    let (s, page) = send(&fx.app, "GET", "/v1/audit", Auth::Session(&reader, Some(DEFAULT_ORG_ID)), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "{page}");
    assert_eq!(page["total"], 0, "{page}");
}

/// The audit actions on a page of `GET /v1/audit`.
fn audit_actions(page: &Value) -> Vec<String> {
    page["data"].as_array().expect("entries").iter().map(|e| e["action"].as_str().unwrap_or("").to_string()).collect()
}

/// Hosted: the master key reads one org's log with `?org_id=` (the spend
/// endpoints' rule), in JSON and CSV; without it, what is filed under no
/// org. Anyone else may name only the org they act in.
#[tokio::test]
async fn hosted_the_master_key_reads_one_orgs_log_with_org_id() {
    let fx = fixture().await;
    let (other_org, _) = seed_org(&fx.db, "Other").await;
    let (s, v) = send(&fx.app, "PUT", &format!("/v1/orgs/{}/limits", fx.org), Auth::Session(&fx.owner, Some(&fx.org)), Some(json!({ "max_budget_usd": 7.0 })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = send(&fx.app, "PUT", "/v1/admin/settings", Auth::Bearer(MASTER), Some(json!({ "cost_markup_percent": 3.0 })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    wait_for_audit(&fx.db, "limits.update").await;
    wait_for_audit(&fx.db, "settings.update").await;

    let (s, page) = send(&fx.app, "GET", &format!("/v1/audit?org_id={}", fx.org), Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "{page}");
    assert_eq!(audit_actions(&page), ["limits.update"], "the org's own: {page}");
    let (s, csv) = send(&fx.app, "GET", &format!("/v1/audit?org_id={}&format=csv", fx.org), Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    let csv = csv.as_str().expect("a CSV body");
    assert!(csv.contains("limits.update") && !csv.contains("settings.update"), "{csv}");
    let (s, page) = send(&fx.app, "GET", &format!("/v1/audit?org_id={other_org}"), Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!((s, page["total"].clone()), (StatusCode::OK, json!(0)), "{page}");
    let (_, page) = send(&fx.app, "GET", "/v1/audit", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(audit_actions(&page), ["settings.update"], "without org_id, what is filed under no org: {page}");

    let (s, page) = send(&fx.app, "GET", &format!("/v1/audit?org_id={}", fx.org), Auth::Session(&fx.owner, Some(&fx.org)), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "naming one's own org: {page}");
    assert_eq!(audit_actions(&page), ["limits.update"]);
    for (who, auth) in [("the org's owner", Auth::Session(&fx.owner, Some(&fx.org))), ("its admin key", Auth::Bearer(&fx.admin_key.raw))] {
        let (s, v) = send(&fx.app, "GET", &format!("/v1/audit?org_id={other_org}"), auth, None, &[]).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{who} names another org: {v}");
    }
}

/// Single-tenant: the master key acts in the default org, and reads any
/// other org's log with `?org_id=`.
#[tokio::test]
async fn single_tenant_the_master_key_reads_another_orgs_log_with_org_id() {
    let fx = fixture_with(Mode::SingleTenant, false).await;
    let (s, v) = send(&fx.app, "PUT", &format!("/v1/orgs/{}/limits", fx.org), Auth::Session(&fx.owner, Some(&fx.org)), Some(json!({ "max_budget_usd": 7.0 })), &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    wait_for_audit(&fx.db, "limits.update").await;
    let (s, page) = send(&fx.app, "GET", &format!("/v1/audit?org_id={}", fx.org), Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "{page}");
    assert_eq!(audit_actions(&page), ["limits.update"], "{page}");
    let (_, page) = send(&fx.app, "GET", "/v1/audit", Auth::Bearer(MASTER), None, &[]).await;
    assert_eq!(page["total"], 0, "without org_id, the default org's: {page}");
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
