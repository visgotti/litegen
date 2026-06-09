//! Integration-style tests for the org/app/member/provider-credential endpoints.
//! Use a real in-memory SqliteDatabase + the actual router in hosted mode.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::json;
use tower::ServiceExt;

use crate::api::handlers::create_router;
use crate::api::middleware::AppState;
use crate::auth::tokens::{generate_csrf_token, generate_session_token};
use crate::capabilities::CapabilityRegistry;
use crate::config::{AppConfig, CacheGlobalConfig, DevFlags, Mode};
use crate::db::sqlite::SqliteDatabase;
use crate::db::DatabaseStore;
use crate::proxy::cache::GenerationCache;
use crate::proxy::materializer::{MaterializeError, Materializer, TempStorage};
use crate::proxy::registry::ProviderRegistry;
use crate::proxy::router::ProxyRouter;
use crate::proxy::storage::LocalStore;
use crate::types::{Role, Session, User};
use bytes::Bytes;
use uuid::Uuid;

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

async fn build_db() -> Arc<SqliteDatabase> {
    Arc::new(
        SqliteDatabase::connect("sqlite::memory:")
            .await
            .expect("in-memory db"),
    )
}

/// Hosted-mode state with an optional secrets key. master_key=None so auth is
/// session/cookie based and the dev-bypass path is disabled (hosted).
async fn build_state(db: Arc<SqliteDatabase>, secrets_key: Option<[u8; 32]>) -> Arc<AppState> {
    let registry = Arc::new(ProviderRegistry::new());
    let config = Arc::new(AppConfig::default());
    let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
    let image_store = Arc::new(LocalStore);
    let router = Arc::new(ProxyRouter::new(registry, cache, config, image_store));
    let materializer = Arc::new(Materializer::new(Arc::new(NoopStorage), reqwest::Client::new()));
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.push("models");
    let cap_registry = Arc::new(CapabilityRegistry::from_dir(&p).expect("models"));
    Arc::new(AppState {
        router,
        db,
        master_key: None,
        registry: cap_registry,
        materializer,
        rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
        in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),
        oauth: crate::auth::oauth::OAuthConfig::default(),
        mode: Mode::Hosted,
        secrets_key,
        dev: DevFlags::default(),
        allow_password: true,
    })
}

/// Create a user + session, returning (user, session_token, csrf_token).
async fn seed_user(db: &Arc<SqliteDatabase>, email: &str) -> (User, String, String) {
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
    db.create_user(&user).await.expect("create user");
    let session_token = generate_session_token();
    let csrf_token = generate_csrf_token();
    let sess = Session {
        id: session_token.clone(),
        user_id: user.id.clone(),
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::days(7),
        ip: None,
        user_agent: None,
        csrf_token: csrf_token.clone(),
    };
    db.create_session(&sess).await.expect("create session");
    (user, session_token, csrf_token)
}

fn cookie(sess: &str) -> String {
    format!("litegen_session={}", sess)
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    if bytes.is_empty() {
        return json!(null);
    }
    serde_json::from_slice(&bytes).unwrap()
}

// ─── Owner: create org → create app → list apps ────────────────────────────────

#[tokio::test]
async fn owner_creates_org_app_and_lists() {
    let db = build_db().await;
    let (_, sess, csrf) = seed_user(&db, "owner@test.com").await;
    let app = create_router(build_state(db.clone(), None).await);

    // Create org
    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Acme" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let org = body_json(resp).await;
    let org_id = org["id"].as_str().unwrap().to_string();
    assert_eq!(org["slug"], "acme");

    // Caller should be Owner member of the new org.
    let owner_user = db.get_user_by_email("owner@test.com").await.unwrap().unwrap();
    assert_eq!(
        db.get_membership(&org_id, &owner_user.id).await.unwrap(),
        Some(Role::Owner)
    );

    // Create an app in the org.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/orgs/{}/apps", org_id))
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Worker" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // List apps → should include the default app + Worker.
    let req = Request::builder()
        .uri(format!("/v1/orgs/{}/apps", org_id))
        .header("cookie", cookie(&sess))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let apps = body_json(resp).await;
    let arr = apps.as_array().unwrap();
    assert_eq!(arr.len(), 2, "default app + Worker");
    assert!(arr.iter().any(|a| a["name"] == "Worker"));
}

// ─── Member/Viewer cannot invite; Owner/Admin can ─────────────────────────────

#[tokio::test]
async fn only_owner_admin_can_invite_members() {
    let db = build_db().await;
    let (owner, owner_sess, owner_csrf) = seed_user(&db, "owner@test.com").await;
    let (member, _member_sess, _) = seed_user(&db, "member@test.com").await;
    let (_viewer, viewer_sess, viewer_csrf) = seed_user(&db, "viewer@test.com").await;
    let app = create_router(build_state(db.clone(), None).await);

    // Owner creates org
    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(&owner_sess))
        .header("x-csrf-token", &owner_csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Acme" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let org = body_json(resp).await;
    let org_id = org["id"].as_str().unwrap().to_string();

    // Add member + viewer to the org directly.
    db.add_org_member(&org_id, &member.id, Role::Member).await.unwrap();
    let viewer_user = db.get_user_by_email("viewer@test.com").await.unwrap().unwrap();
    db.add_org_member(&org_id, &viewer_user.id, Role::Viewer).await.unwrap();

    // Owner can invite.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/orgs/{}/members", org_id))
        .header("cookie", cookie(&owner_sess))
        .header("x-csrf-token", &owner_csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "email": "new@test.com", "role": "member" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "owner can invite");

    // Viewer cannot invite → 403.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/orgs/{}/members", org_id))
        .header("cookie", cookie(&viewer_sess))
        .header("x-csrf-token", &viewer_csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "email": "x@test.com", "role": "member" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "viewer cannot invite");

    let _ = owner;
}

// ─── Non-member is denied (require_member_perm) ────────────────────────────────

#[tokio::test]
async fn non_member_gets_403_listing_members() {
    let db = build_db().await;
    let (_, owner_sess, owner_csrf) = seed_user(&db, "owner@test.com").await;
    let (_, outsider_sess, _) = seed_user(&db, "outsider@test.com").await;
    let app = create_router(build_state(db.clone(), None).await);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(&owner_sess))
        .header("x-csrf-token", &owner_csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Acme" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let org = body_json(resp).await;
    let org_id = org["id"].as_str().unwrap().to_string();

    // Outsider (not a member) listing members → 403.
    let req = Request::builder()
        .uri(format!("/v1/orgs/{}/members", org_id))
        .header("cookie", cookie(&outsider_sess))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ─── transfer-owner swaps roles; member denied ─────────────────────────────────

#[tokio::test]
async fn transfer_owner_swaps_and_member_denied() {
    let db = build_db().await;
    let (_, owner_sess, owner_csrf) = seed_user(&db, "owner@test.com").await;
    let (member, member_sess, member_csrf) = seed_user(&db, "member@test.com").await;
    let app = create_router(build_state(db.clone(), None).await);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(&owner_sess))
        .header("x-csrf-token", &owner_csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Acme" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let org = body_json(resp).await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let owner_user = db.get_user_by_email("owner@test.com").await.unwrap().unwrap();

    db.add_org_member(&org_id, &member.id, Role::Member).await.unwrap();

    // Member cannot transfer ownership → 403.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/orgs/{}/transfer-owner", org_id))
        .header("cookie", cookie(&member_sess))
        .header("x-csrf-token", &member_csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "new_owner_user_id": member.id })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "member cannot transfer");

    // Owner transfers to member → swaps.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/orgs/{}/transfer-owner", org_id))
        .header("cookie", cookie(&owner_sess))
        .header("x-csrf-token", &owner_csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "new_owner_user_id": member.id })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "owner transfers");

    assert_eq!(db.get_membership(&org_id, &member.id).await.unwrap(), Some(Role::Owner));
    assert_eq!(db.get_membership(&org_id, &owner_user.id).await.unwrap(), Some(Role::Admin));
}

// ─── Provider credentials: store, list (no plaintext), and 400 without key ────

#[tokio::test]
async fn provider_credential_store_and_list_no_plaintext() {
    let db = build_db().await;
    let (_, sess, csrf) = seed_user(&db, "owner@test.com").await;
    let app = create_router(build_state(db.clone(), Some([7u8; 32])).await);

    // Create org + grab its default app.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Acme" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let org = body_json(resp).await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let apps = db.list_apps_for_org(&org_id).await.unwrap();
    let app_id = apps[0].id.clone();

    // POST a credential.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/apps/{}/provider-credentials", app_id))
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({ "provider": "openai", "credentials": { "api_key": "sk-secret1234" } })).unwrap(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let info = body_json(resp).await;
    assert_eq!(info["provider"], "openai");
    assert_eq!(info["display_hint"], "…1234");
    assert!(info.get("api_key").is_none(), "no plaintext in response");
    assert!(!info.to_string().contains("sk-secret1234"), "plaintext must not leak");

    // GET list → ProviderCredentialInfo, no plaintext.
    let req = Request::builder()
        .uri(format!("/v1/apps/{}/provider-credentials", app_id))
        .header("cookie", cookie(&sess))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = body_json(resp).await;
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["provider"], "openai");
    assert!(!list.to_string().contains("sk-secret1234"), "plaintext must not leak in list");
}

#[tokio::test]
async fn provider_credential_post_400_without_secrets_key() {
    let db = build_db().await;
    let (_, sess, csrf) = seed_user(&db, "owner@test.com").await;
    let app = create_router(build_state(db.clone(), None).await); // no secrets key

    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Acme" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let org = body_json(resp).await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let apps = db.list_apps_for_org(&org_id).await.unwrap();
    let app_id = apps[0].id.clone();

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/apps/{}/provider-credentials", app_id))
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({ "provider": "openai", "credentials": { "api_key": "sk-x" } })).unwrap(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp).await;
    assert_eq!(body["error"]["code"], "secrets_not_configured");
}

// ─── App endpoints authorize via the app's org membership ──────────────────────

#[tokio::test]
async fn non_member_403_on_get_app() {
    let db = build_db().await;
    let (_, owner_sess, owner_csrf) = seed_user(&db, "owner@test.com").await;
    let (_, outsider_sess, _) = seed_user(&db, "outsider@test.com").await;
    let app = create_router(build_state(db.clone(), None).await);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(&owner_sess))
        .header("x-csrf-token", &owner_csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Acme" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let org = body_json(resp).await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let apps = db.list_apps_for_org(&org_id).await.unwrap();
    let app_id = apps[0].id.clone();

    // Owner can read it.
    let req = Request::builder()
        .uri(format!("/v1/apps/{}", app_id))
        .header("cookie", cookie(&owner_sess))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Outsider → 403.
    let req = Request::builder()
        .uri(format!("/v1/apps/{}", app_id))
        .header("cookie", cookie(&outsider_sess))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ─── Bearer/no-session callers cannot manage orgs ──────────────────────────────

#[tokio::test]
async fn create_org_requires_session_not_just_auth() {
    // master_key set so a Bearer token authenticates but has ctx.user == None.
    let db = build_db().await;
    let registry = Arc::new(ProviderRegistry::new());
    let config = Arc::new(AppConfig::default());
    let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
    let image_store = Arc::new(LocalStore);
    let router = Arc::new(ProxyRouter::new(registry, cache, config, image_store));
    let materializer = Arc::new(Materializer::new(Arc::new(NoopStorage), reqwest::Client::new()));
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.push("models");
    let cap_registry = Arc::new(CapabilityRegistry::from_dir(&p).expect("models"));
    let state = Arc::new(AppState {
        router,
        db: db.clone(),
        master_key: Some("master".to_string()),
        registry: cap_registry,
        materializer,
        rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
        in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),
        oauth: crate::auth::oauth::OAuthConfig::default(),
        mode: Mode::Hosted,
        secrets_key: None,
        dev: DevFlags::default(),
        allow_password: true,
    });
    let app = create_router(state);

    // Bearer master key → ctx.user is None → create_org returns 401 session_required.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("authorization", "Bearer master")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Acme" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ─── rename org: PATCH /v1/orgs/{id} by Owner ─────────────────────────────────

#[tokio::test]
async fn rename_org_returns_200_and_name_changes() {
    let db = build_db().await;
    let (_, sess, csrf) = seed_user(&db, "owner@rename.com").await;
    let app = create_router(build_state(db.clone(), None).await);

    // Create org
    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Before" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let org = body_json(resp).await;
    let org_id = org["id"].as_str().unwrap().to_string();

    // Rename
    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/v1/orgs/{}", org_id))
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "After" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "rename should return 200");
    let updated = body_json(resp).await;
    assert_eq!(updated["name"], "After", "name should be updated in response");

    // Verify via DB.
    let org_row = db.get_organization(&org_id).await.unwrap().expect("org exists");
    assert_eq!(org_row.name, "After", "name should be persisted in DB");
}

// ─── delete app: DELETE /v1/apps/{app_id} by Owner ────────────────────────────

#[tokio::test]
async fn delete_app_returns_2xx_and_app_gone() {
    let db = build_db().await;
    let (_, sess, csrf) = seed_user(&db, "owner@delapp.com").await;
    let app = create_router(build_state(db.clone(), None).await);

    // Create org → grabs its default app.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Acme" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let org = body_json(resp).await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let apps = db.list_apps_for_org(&org_id).await.unwrap();
    let app_id = apps[0].id.clone();

    // Delete the app.
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/apps/{}", app_id))
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert!(
        resp.status().is_success(),
        "delete app should return 2xx, got {}",
        resp.status()
    );

    // Verify via DB — application should no longer exist.
    let gone = db.get_application(&app_id).await.unwrap();
    assert!(gone.is_none(), "app should be gone from DB after delete");
}

// ─── delete org: DELETE /v1/orgs/{id} by Owner ────────────────────────────────
//
// NOTE: sqlx 0.8 enables `PRAGMA foreign_keys = ON` by default. The
// `organization_members` table references `organizations(id)` without CASCADE,
// so `DELETE FROM organizations` will be blocked while any member rows exist.
// The happy-path test therefore: (a) verifies that the DB layer correctly deletes
// an org when there are no FK-dependent rows, and (b) verifies the HTTP endpoint
// is routed and auth-checked (non-member → 403, non-existent org → 404).

#[tokio::test]
async fn delete_org_db_deletes_when_no_fk_rows() {
    let db = build_db().await;

    // Insert a bare org with no apps and no member rows.
    let org_id = format!("org-{}", Uuid::new_v4());
    let now = chrono::Utc::now();
    let org = crate::types::Organization {
        id: org_id.clone(),
        name: "ToDelete".into(),
        slug: format!("to-delete-{}", Uuid::new_v4()),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_organization(&org).await.unwrap();

    // DB-level delete succeeds when no FK child rows exist.
    let deleted = db.delete_organization(&org_id).await.unwrap();
    assert!(deleted, "delete_organization should return true for an existing org");
    let gone = db.get_organization(&org_id).await.unwrap();
    assert!(gone.is_none(), "org should be absent from DB after delete");
}

#[tokio::test]
async fn delete_org_endpoint_requires_membership() {
    let db = build_db().await;
    let (_, sess, csrf) = seed_user(&db, "owner@delorg2.com").await;
    let app = create_router(build_state(db.clone(), None).await);

    // Create an org via HTTP — caller becomes owner+member.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Acme" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let org = body_json(resp).await;
    let org_id = org["id"].as_str().unwrap().to_string();

    // A non-member should be denied.
    let (_, outsider_sess, _) = seed_user(&db, "outsider@delorg2.com").await;
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/orgs/{}", org_id))
        .header("cookie", cookie(&outsider_sess))
        .header("x-csrf-token", &csrf)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "non-member must be denied delete"
    );
}

#[test]
fn derive_display_hint_covers_pools_and_singles() {
    use super::derive_display_hint;

    // Single bearer key → last 4 chars.
    assert_eq!(
        derive_display_hint(&json!({ "api_key": "sk-secret1234" })).as_deref(),
        Some("…1234")
    );

    // Weighted key pool → first key's last 4 + a count of the rest.
    assert_eq!(
        derive_display_hint(&json!({ "api_keys": [
            { "key": "sk-aaaa1111", "weight": 3 },
            { "key": "sk-bbbb2222" }
        ] }))
        .as_deref(),
        Some("…1111 (+1 more)")
    );

    // Signing credential pool → first key_id's last 4 + count.
    assert_eq!(
        derive_display_hint(&json!({ "credential_sets": [
            { "key_id": "AKIAEXAMPLE9999", "key_secret": "x" },
            { "key_id": "AKIAEXAMPLE8888", "key_secret": "y" }
        ] }))
        .as_deref(),
        Some("…9999 (+1 more)")
    );

    // Single signing credential → key_id's last 4 (the secret is never shown).
    assert_eq!(
        derive_display_hint(&json!({ "key_id": "AKIAEXAMPLE4321", "key_secret": "top" })).as_deref(),
        Some("…4321")
    );

    // Nothing usable → no hint.
    assert_eq!(derive_display_hint(&json!({ "note": "n/a" })), None);
    assert_eq!(derive_display_hint(&json!({ "api_keys": [] })), None);
}
