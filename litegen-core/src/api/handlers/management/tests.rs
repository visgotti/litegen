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
use crate::api::middleware::AppState;
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
use crate::types::{Application, Organization, Role, Session, User};

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
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .configure_provider("mock", crate::providers::ProviderInstanceConfig::default())
        .await;
    let config = Arc::new(AppConfig::default());
    let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
    let router = Arc::new(ProxyRouter::new(registry, cache, config, Arc::new(LocalStore)));
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
    let fx = fixture_with(Mode::SingleTenant, false).await;
    // An admin-scope key and an owner session both qualify; a generate key does not.
    assert_eq!(send(&fx.app, "GET", "/v1/admin/models", Auth::Bearer(&fx.admin_key.raw), None, &[]).await.0, StatusCode::OK);
    assert_eq!(send(&fx.app, "GET", "/v1/admin/settings", Auth::Session(&fx.owner, Some(&fx.org)), None, &[]).await.0, StatusCode::OK);
    assert_eq!(send(&fx.app, "GET", "/v1/admin/models", Auth::Bearer(&fx.gen_key.raw), None, &[]).await.0, StatusCode::FORBIDDEN);
    let member = seed_user(&fx.db, "member@acme.test").await;
    fx.db.add_org_member(&fx.org, &member.user.id, Role::Member).await.unwrap();
    assert_eq!(send(&fx.app, "GET", "/v1/admin/routes", Auth::Session(&member, Some(&fx.org)), None, &[]).await.0, StatusCode::FORBIDDEN);
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
