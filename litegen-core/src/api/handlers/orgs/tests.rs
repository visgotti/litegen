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
    build_state_full(db, secrets_key, None).await
}

/// Like `build_state` but with an explicit master key, so tests can exercise the
/// platform-admin (master key) auth path in hosted mode.
async fn build_state_full(
    db: Arc<SqliteDatabase>,
    secrets_key: Option<[u8; 32]>,
    master_key: Option<String>,
) -> Arc<AppState> {
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
        master_key,
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
        sign_in_epoch: 0,
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

/// Review A2: changing a member's role takes a role above the member's
/// current one, and never grants one at or above the caller's own; each
/// change is audited with the role before. An Admin could demote a peer
/// Admin, reset them while outranking them, and promote them back, unseen.
#[tokio::test]
async fn a_member_role_change_needs_a_higher_role_and_is_audited() {
    let db = build_db().await;
    let (_, owner_sess, owner_csrf) = seed_user(&db, "owner@test.com").await;
    let (admin, admin_sess, admin_csrf) = seed_user(&db, "admin@test.com").await;
    let (peer, _, _) = seed_user(&db, "peer@test.com").await;
    let (member, _, _) = seed_user(&db, "member@test.com").await;
    let app = create_router(build_state(db.clone(), None).await);
    let org = new_org(&app, &owner_sess, &owner_csrf, "Acme").await;
    db.add_org_member(&org, &admin.id, Role::Admin).await.unwrap();
    db.add_org_member(&org, &peer.id, Role::Admin).await.unwrap();
    db.add_org_member(&org, &member.id, Role::Member).await.unwrap();
    let patch = |id: &str, role: &str, sess: &str, csrf: &str| {
        Request::builder()
            .method("PATCH")
            .uri(format!("/v1/orgs/{org}/members/{id}"))
            .header("cookie", cookie(sess))
            .header("x-csrf-token", csrf)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&json!({ "role": role })).unwrap()))
            .unwrap()
    };
    for (who, req, expected) in [
        ("an admin demoting a peer admin", patch(&peer.id, "member", &admin_sess, &admin_csrf), StatusCode::FORBIDDEN),
        ("an admin promoting a member to admin", patch(&member.id, "admin", &admin_sess, &admin_csrf), StatusCode::FORBIDDEN),
        ("an admin making a member a viewer", patch(&member.id, "viewer", &admin_sess, &admin_csrf), StatusCode::OK),
        ("the owner demoting an admin", patch(&peer.id, "member", &owner_sess, &owner_csrf), StatusCode::OK),
    ] {
        assert_eq!(app.clone().oneshot(req).await.unwrap().status(), expected, "{who}");
    }
    assert_eq!(db.get_membership(&org, &peer.id).await.unwrap(), Some(Role::Member));
    assert_eq!(db.get_membership(&org, &member.id).await.unwrap(), Some(Role::Viewer));
    let mut entries = audit_with(&db, &org, "org.member.update").await;
    for _ in 0..200 {
        if entries.iter().filter(|e| e.action == "org.member.update").count() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        entries = db.list_audit_log_for_tenant(&org, 1, 100).await.unwrap().0;
    }
    let changes: Vec<(String, String, String)> = entries
        .iter()
        .filter(|e| e.action == "org.member.update")
        .map(|e| {
            let after: serde_json::Value = serde_json::from_str(e.after_json.as_deref().unwrap_or("null")).unwrap();
            (e.target_id.clone(), after["previous_role"].as_str().unwrap_or("").to_string(), after["role"].as_str().unwrap_or("").to_string())
        })
        .collect();
    for change in [(member.id.clone(), "member".to_string(), "viewer".to_string()), (peer.id.clone(), "admin".to_string(), "member".to_string())] {
        assert!(changes.contains(&change), "{change:?} in {changes:?}");
    }
}

/// Round 3b, S4: inviting takes the role rule, or "remove a Member, invite
/// them back as Admin" promoted past it: an Admin invites Members and
/// Viewers, an Owner Admins as well; the master key, any role but Owner.
#[tokio::test]
async fn inviting_needs_a_role_above_the_invited_one() {
    let db = build_db().await;
    let (_, owner_sess, owner_csrf) = seed_user(&db, "owner@test.com").await;
    let (admin, admin_sess, admin_csrf) = seed_user(&db, "admin@test.com").await;
    let app = create_router(build_state_full(db.clone(), None, Some("platform-master".into())).await);
    let org = new_org(&app, &owner_sess, &owner_csrf, "Acme").await;
    db.add_org_member(&org, &admin.id, Role::Admin).await.unwrap();
    let invite = |email: &str, role: &str, auth: (&str, &str)| {
        let req = Request::builder().method("POST").uri(format!("/v1/orgs/{org}/members"));
        let req = match auth {
            ("master", _) => req.header("authorization", "Bearer platform-master"),
            (sess, csrf) => req.header("cookie", cookie(sess)).header("x-csrf-token", csrf),
        };
        req.header("content-type", "application/json").body(Body::from(serde_json::to_vec(&json!({ "email": email, "role": role })).unwrap())).unwrap()
    };
    let (as_admin, as_owner, as_master) = ((admin_sess.as_str(), admin_csrf.as_str()), (owner_sess.as_str(), owner_csrf.as_str()), ("master", ""));
    let resp = app.clone().oneshot(invite("new-admin@test.com", "admin", as_admin)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "an admin inviting an admin");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "forbidden", "{body}");
    for (who, req) in [
        ("an admin inviting a member", invite("new-member@test.com", "member", as_admin)),
        ("an admin inviting a viewer", invite("new-viewer@test.com", "viewer", as_admin)),
        ("the owner inviting an admin", invite("new-admin@test.com", "admin", as_owner)),
        ("the master key inviting an admin", invite("other-admin@test.com", "admin", as_master)),
    ] {
        assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::OK, "{who}");
    }
}

/// The outrank rule (review A2), for removal: removing a member and
/// inviting them back lower is a demotion by another door, so a person
/// removes only a member they outrank in the org. Leaving (removing
/// yourself) stays allowed, and the master key may remove anyone but the
/// Owner.
#[tokio::test]
async fn removing_a_member_needs_a_higher_role_and_leaving_stays_allowed() {
    let db = build_db().await;
    let (_, owner_sess, owner_csrf) = seed_user(&db, "owner@test.com").await;
    let (admin, admin_sess, admin_csrf) = seed_user(&db, "admin@test.com").await;
    let (peer, _, _) = seed_user(&db, "peer@test.com").await;
    let (other, _, _) = seed_user(&db, "other@test.com").await;
    let (member, _, _) = seed_user(&db, "member@test.com").await;
    let app = create_router(build_state_full(db.clone(), None, Some("platform-master".into())).await);
    let org = new_org(&app, &owner_sess, &owner_csrf, "Acme").await;
    for (who, role) in [(&admin, Role::Admin), (&peer, Role::Admin), (&other, Role::Admin), (&member, Role::Member)] {
        db.add_org_member(&org, &who.id, role).await.unwrap();
    }
    let remove = |id: &str, auth: (&str, &str)| {
        let req = Request::builder().method("DELETE").uri(format!("/v1/orgs/{org}/members/{id}"));
        let req = match auth {
            ("master", _) => req.header("authorization", "Bearer platform-master"),
            (sess, csrf) => req.header("cookie", cookie(sess)).header("x-csrf-token", csrf),
        };
        req.body(Body::empty()).unwrap()
    };
    let (as_admin, as_owner, as_master) = ((admin_sess.as_str(), admin_csrf.as_str()), (owner_sess.as_str(), owner_csrf.as_str()), ("master", ""));
    let resp = app.clone().oneshot(remove(&peer.id, as_admin)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "an admin removing a peer admin");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "forbidden", "{body}");
    assert_eq!(db.get_membership(&org, &peer.id).await.unwrap(), Some(Role::Admin), "the peer stays");
    for (who, req) in [
        ("an admin removing a member", remove(&member.id, as_admin)),
        ("an admin leaving", remove(&admin.id, as_admin)),
        ("the owner removing an admin", remove(&peer.id, as_owner)),
        ("the master key removing an admin", remove(&other.id, as_master)),
    ] {
        assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::NO_CONTENT, "{who}");
    }
    for gone in [&member.id, &admin.id, &peer.id, &other.id] {
        assert_eq!(db.get_membership(&org, gone).await.unwrap(), None, "{gone} removed");
    }
}

/// Follow-up item 4: an org, its maker's Owner membership and its first app
/// are made together. One whose app can't be written leaves nothing behind;
/// before, the org and the membership stayed, with no app.
#[tokio::test]
async fn an_org_that_fails_to_be_made_leaves_nothing_behind() {
    let db = build_db().await;
    let (owner, sess, csrf) = seed_user(&db, "owner@test.com").await;
    let app = create_router(build_state(db.clone(), None).await);
    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Acme" })).unwrap()))
        .unwrap();
    sqlx::query("ALTER TABLE applications RENAME TO applications_away").execute(db.pool()).await.unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    sqlx::query("ALTER TABLE applications_away RENAME TO applications").execute(db.pool()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(db.list_orgs_for_user(&owner.id).await.unwrap().is_empty(), "no org or membership left behind");
    assert!(db.get_org_by_slug("acme").await.unwrap().is_none(), "no org left behind");

    let id = new_org(&app, &sess, &csrf, "Acme").await;
    assert_eq!(db.get_organization(&id).await.unwrap().expect("made").slug, "acme", "the slug is free again");
}

/// An org's invitation takes only an address as the invitee's email: the
/// account it makes is labelled by it in the audit log.
#[tokio::test]
async fn an_org_invitation_to_what_is_not_an_address_is_refused() {
    let db = build_db().await;
    let (_, owner_sess, owner_csrf) = seed_user(&db, "owner@test.com").await;
    let app = create_router(build_state(db.clone(), None).await);
    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(&owner_sess))
        .header("x-csrf-token", &owner_csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Acme" })).unwrap()))
        .unwrap();
    let org = body_json(app.clone().oneshot(req).await.unwrap()).await;
    let org_id = org["id"].as_str().unwrap().to_string();

    for email in ["nobody", "first last@test.com"] {
        let req = Request::builder()
            .method("POST")
            .uri(format!("/v1/orgs/{org_id}/members"))
            .header("cookie", cookie(&owner_sess))
            .header("x-csrf-token", &owner_csrf)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&json!({ "email": email, "role": "member" })).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{email}");
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "invalid_email", "{email}: {body}");
    }
    assert!(db.list_invitations().await.unwrap().is_empty(), "no invitation stored");
}

// ─── Invitations: listed and revoked ───────────────────────────────────────────

/// A new org, made by the session `sess`: its id.
async fn new_org(app: &axum::Router, sess: &str, csrf: &str, name: &str) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(sess))
        .header("x-csrf-token", csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": name })).unwrap()))
        .unwrap();
    body_json(app.clone().oneshot(req).await.unwrap()).await["id"].as_str().expect("an org id").to_string()
}

/// An invitation to `org_id`, made `age` ago and good for `ttl` from then.
fn stored_invitation(org_id: &str, email: &str, age: chrono::Duration, ttl: chrono::Duration) -> crate::types::Invitation {
    let made = chrono::Utc::now() - age;
    crate::types::Invitation {
        id: Uuid::new_v4().to_string(),
        email: email.to_string(),
        role: Role::Member,
        token: generate_session_token(),
        invited_by: None,
        org_id: org_id.to_string(),
        expires_at: made + ttl,
        used_at: None,
        created_at: made,
    }
}

/// The org's audit log once `action` is in it (entries are written in the
/// background).
async fn audit_with(db: &SqliteDatabase, org_id: &str, action: &str) -> Vec<crate::types::AuditLogEntry> {
    for _ in 0..200 {
        let (entries, _) = db.list_audit_log_for_tenant(org_id, 1, 100).await.unwrap();
        if entries.iter().any(|e| e.action == action) {
            return entries;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("no {action} audit entry in {org_id}");
}

/// An org's pending invitations are listed to those who may invite there,
/// and to the master key: newest first and without their tokens (a token is
/// shown only in the answer that makes the invitation). Accepted and expired
/// ones, and another org's, aren't listed.
#[tokio::test]
async fn an_orgs_pending_invitations_are_listed_without_their_tokens() {
    use chrono::Duration;
    let db = build_db().await;
    let (owner, owner_sess, owner_csrf) = seed_user(&db, "owner@test.com").await;
    let (member, member_sess, _) = seed_user(&db, "member@test.com").await;
    let app = create_router(build_state_full(db.clone(), None, Some("mk".into())).await);
    let org_id = new_org(&app, &owner_sess, &owner_csrf, "Acme").await;
    let other_org = new_org(&app, &owner_sess, &owner_csrf, "Other").await;
    db.add_org_member(&org_id, &member.id, Role::Member).await.unwrap();
    let older = crate::types::Invitation {
        invited_by: Some(owner.id.clone()),
        ..stored_invitation(&org_id, "older@test.com", Duration::hours(2), Duration::days(7))
    };
    let newer = stored_invitation(&org_id, "newer@test.com", Duration::hours(1), Duration::days(7));
    let expired = stored_invitation(&org_id, "expired@test.com", Duration::days(8), Duration::days(7));
    let accepted = stored_invitation(&org_id, "accepted@test.com", Duration::hours(3), Duration::days(7));
    let elsewhere = stored_invitation(&other_org, "elsewhere@test.com", Duration::hours(1), Duration::days(7));
    for inv in [&older, &newer, &expired, &accepted, &elsewhere] {
        db.create_invitation(inv).await.unwrap();
    }
    assert!(db.mark_invitation_used(&accepted.token).await.unwrap());

    let list = |auth: (&'static str, String)| {
        Request::builder().uri(format!("/v1/orgs/{org_id}/invitations")).header(auth.0, auth.1).body(Body::empty()).unwrap()
    };
    for (who, auth) in [("the owner", ("cookie", cookie(&owner_sess))), ("the master key", ("authorization", "Bearer mk".to_string()))] {
        let resp = app.clone().oneshot(list(auth)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{who}");
        let listed = body_json(resp).await;
        let items = listed.as_array().unwrap_or_else(|| panic!("{who}: a list: {listed}"));
        let ids: Vec<&str> = items.iter().map(|i| i["id"].as_str().unwrap_or("")).collect();
        assert_eq!(ids, [newer.id.as_str(), older.id.as_str()], "{who}: {listed}");
        assert!(items.iter().all(|i| i.get("token").is_none()), "{who}: no tokens: {listed}");
        assert_eq!(items[1]["email"], "older@test.com");
        assert_eq!(items[1]["role"], "member");
        assert_eq!(items[1]["org_id"], org_id.as_str());
        assert_eq!(items[1]["invited_by"], owner.id.as_str());
        assert_eq!(items[0]["invited_by"], json!(null));
        assert!(items[1]["created_at"].is_string() && items[1]["expires_at"].is_string(), "{listed}");
    }
    let resp = app.clone().oneshot(list(("cookie", cookie(&member_sess)))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "a member may not invite, so lists none");
}

/// A revoked invitation no longer accepts. Through an org's path only that
/// org's invitations not yet accepted can be revoked, and only by those
/// whose role there may (`invitation:revoke`).
#[tokio::test]
async fn a_revoked_invitation_no_longer_accepts() {
    use chrono::Duration;
    let db = build_db().await;
    let (_, owner_sess, owner_csrf) = seed_user(&db, "owner@test.com").await;
    let (member, member_sess, member_csrf) = seed_user(&db, "member@test.com").await;
    let app = create_router(build_state(db.clone(), None).await);
    let org_id = new_org(&app, &owner_sess, &owner_csrf, "Acme").await;
    let other_org = new_org(&app, &owner_sess, &owner_csrf, "Other").await;
    db.add_org_member(&org_id, &member.id, Role::Member).await.unwrap();
    let mine = stored_invitation(&org_id, "mine@test.com", Duration::hours(1), Duration::days(7));
    let theirs = stored_invitation(&other_org, "theirs@test.com", Duration::hours(1), Duration::days(7));
    let accepted = stored_invitation(&org_id, "accepted@test.com", Duration::hours(1), Duration::days(7));
    for inv in [&mine, &theirs, &accepted] {
        db.create_invitation(inv).await.unwrap();
    }
    assert!(db.mark_invitation_used(&accepted.token).await.unwrap());
    let revoke = |id: &str, sess: &str, csrf: &str| {
        Request::builder()
            .method("DELETE")
            .uri(format!("/v1/orgs/{org_id}/invitations/{id}"))
            .header("cookie", cookie(sess))
            .header("x-csrf-token", csrf)
            .body(Body::empty())
            .unwrap()
    };

    let resp = app.clone().oneshot(revoke(&mine.id, &member_sess, &member_csrf)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "a member may not revoke");
    // Review A6: an accepted invitation is a 409 (remove the member instead),
    // told apart from one the org doesn't have.
    for (inv, why, status, code) in [
        (&theirs, "another org's", StatusCode::NOT_FOUND, "invitation_not_found"),
        (&accepted, "an accepted one", StatusCode::CONFLICT, "invitation_accepted"),
    ] {
        let resp = app.clone().oneshot(revoke(&inv.id, &owner_sess, &owner_csrf)).await.unwrap();
        assert_eq!(resp.status(), status, "{why}");
        assert_eq!(body_json(resp).await["error"]["code"], code, "{why}");
        assert!(db.get_invitation(&inv.token).await.unwrap().is_some(), "{why} stays");
    }
    let resp = app.clone().oneshot(revoke(&mine.id, &owner_sess, &owner_csrf)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = app.clone().oneshot(revoke(&mine.id, &owner_sess, &owner_csrf)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "revoked already");

    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/auth/invitations/{}/accept", mine.token))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "password": "a-long-enough-password" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "a revoked token accepts nothing");
    assert!(db.get_user_by_email("mine@test.com").await.unwrap().is_none(), "no account made");
}

/// The org's audit log has an invitation made through the org's route (as
/// `POST /v1/users` already wrote) and its revocation.
#[tokio::test]
async fn an_orgs_invitations_and_their_revocation_are_audited() {
    let db = build_db().await;
    let (_, owner_sess, owner_csrf) = seed_user(&db, "owner@test.com").await;
    let app = create_router(build_state(db.clone(), None).await);
    let org_id = new_org(&app, &owner_sess, &owner_csrf, "Acme").await;
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/orgs/{org_id}/members"))
        .header("cookie", cookie(&owner_sess))
        .header("x-csrf-token", &owner_csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "email": "New@Test.com", "role": "member" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let made = body_json(resp).await;
    let id = made["id"].as_str().expect("an id").to_string();
    let token = made["token"].as_str().unwrap_or_else(|| panic!("the inviter gets the token: {made}"));
    assert!(db.get_invitation(token).await.unwrap().is_some(), "{made}");
    assert_eq!(made["org_id"], org_id.as_str());
    assert!(made.get("invite_url").is_some_and(|u| u.is_null()), "no public URL configured: {made}");

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/orgs/{org_id}/invitations/{id}"))
        .header("cookie", cookie(&owner_sess))
        .header("x-csrf-token", &owner_csrf)
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::NO_CONTENT);

    let entries = audit_with(&db, &org_id, "invitation.revoke").await;
    for action in ["invitation.create", "invitation.revoke"] {
        let entry = entries.iter().find(|e| e.action == action).unwrap_or_else(|| panic!("{action}: {entries:?}"));
        assert_eq!((entry.target_type.as_str(), entry.target_id.as_str()), ("invitation", id.as_str()), "{action}");
        assert_eq!(entry.actor_label, "owner@test.com", "{action}");
        assert!(entry.after_json.as_deref().is_some_and(|a| a.contains("new@test.com")), "{action}: {:?}", entry.after_json);
    }
}

/// Review round 6, finding 6: a demotion to a role that can't invite and a
/// removal revoke invitations (SEC-INVITES), and each revoke is in the org's
/// audit log with its reason, as a deactivation's is.
#[tokio::test]
async fn a_demotion_and_a_removal_audit_the_invitations_they_revoke() {
    use chrono::Duration;
    let db = build_db().await;
    let (_, owner_sess, owner_csrf) = seed_user(&db, "owner@test.com").await;
    let (admin, _, _) = seed_user(&db, "admin@test.com").await;
    let app = create_router(build_state(db.clone(), None).await);
    let org_id = new_org(&app, &owner_sess, &owner_csrf, "Acme").await;
    db.add_org_member(&org_id, &admin.id, Role::Admin).await.unwrap();
    let sent_before_demotion = crate::types::Invitation {
        invited_by: Some(admin.id.clone()),
        ..stored_invitation(&org_id, "first@test.com", Duration::hours(1), Duration::days(7))
    };
    db.create_invitation(&sent_before_demotion).await.unwrap();
    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/v1/orgs/{org_id}/members/{}", admin.id))
        .header("cookie", cookie(&owner_sess))
        .header("x-csrf-token", &owner_csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "role": "member" })).unwrap()))
        .unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::OK, "the owner demotes the admin");

    let sent_before_removal = crate::types::Invitation {
        invited_by: Some(admin.id.clone()),
        ..stored_invitation(&org_id, "second@test.com", Duration::hours(1), Duration::days(7))
    };
    let to_them = stored_invitation(&org_id, "admin@test.com", Duration::hours(1), Duration::days(7));
    for inv in [&sent_before_removal, &to_them] {
        db.create_invitation(inv).await.unwrap();
    }
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/orgs/{org_id}/members/{}", admin.id))
        .header("cookie", cookie(&owner_sess))
        .header("x-csrf-token", &owner_csrf)
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::NO_CONTENT, "the owner removes them");
    assert!(db.list_invitations().await.unwrap().is_empty(), "all three revoked");

    // Written before each answer went out (review round 7, cleanup): one
    // read, no polling.
    let (entries, _) = db.list_audit_log_for_tenant(&org_id, 1, 100).await.unwrap();
    let revokes = entries.into_iter().filter(|e| e.action == "invitation.revoke").collect::<Vec<_>>();
    for (inv, reason) in [(&sent_before_demotion, "inviter_role_changed"), (&sent_before_removal, "member_removed"), (&to_them, "member_removed")] {
        let entry = revokes.iter().find(|e| e.target_id == inv.id).unwrap_or_else(|| panic!("{}: {revokes:?}", inv.email));
        assert!(entry.after_json.as_deref().is_some_and(|a| a.contains(&format!(r#""reason":"{reason}""#))), "{}: {:?}", inv.email, entry.after_json);
        assert_eq!(entry.actor_label, "owner@test.com", "{}", inv.email);
    }
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

// ─── Cross-tenant isolation (BOLA) ─────────────────────────────────────────────
//
// Every org/app/member/credential handler authorizes against the PATH org via
// `require_member_perm`, which checks the caller's membership in that specific
// org (not the active-org header). These tests prove that an authenticated user
// who is NOT a member of the target org is denied (403) on read AND write, and
// that the target survives. Each attacker request carries the attacker's OWN
// valid CSRF token, so a 403 reflects the authorization boundary — not a CSRF
// rejection (which would also be 403, for the wrong reason).

/// Create an org via HTTP (the caller becomes Owner+member) and return
/// `(org_id, default_app_id)`.
async fn create_org_returning_app(
    app: &axum::Router,
    sess: &str,
    csrf: &str,
    name: &str,
) -> (String, String) {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(sess))
        .header("x-csrf-token", csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": name })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "setup: create org");
    let org_id = body_json(resp).await["id"].as_str().unwrap().to_string();

    let req = Request::builder()
        .uri(format!("/v1/orgs/{}/apps", org_id))
        .header("cookie", cookie(sess))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "setup: list apps");
    let apps = body_json(resp).await;
    let app_id = apps.as_array().unwrap()[0]["id"].as_str().unwrap().to_string();
    (org_id, app_id)
}

#[tokio::test]
async fn cross_tenant_cannot_read_or_modify_org() {
    let db = build_db().await;
    let (_owner_a, sess_a, csrf_a) = seed_user(&db, "owner_a@xt-org.com").await;
    let (_attacker_b, sess_b, csrf_b) = seed_user(&db, "attacker_b@xt-org.com").await;
    let app = create_router(build_state(db.clone(), None).await);
    let (org_a, _app_a) = create_org_returning_app(&app, &sess_a, &csrf_a, "OrgA").await;

    // READ
    let req = Request::builder()
        .uri(format!("/v1/orgs/{}", org_a))
        .header("cookie", cookie(&sess_b))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant GET org must be 403"
    );

    // RENAME (valid CSRF for B → denial is authorization, not CSRF)
    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/v1/orgs/{}", org_a))
        .header("cookie", cookie(&sess_b))
        .header("x-csrf-token", &csrf_b)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Pwned" })).unwrap()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant PATCH org must be 403"
    );

    // DELETE
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/orgs/{}", org_a))
        .header("cookie", cookie(&sess_b))
        .header("x-csrf-token", &csrf_b)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant DELETE org must be 403"
    );

    // OrgA survives unchanged.
    let still = db.get_organization(&org_a).await.unwrap();
    assert!(still.is_some(), "OrgA must survive cross-tenant attempts");
    assert_eq!(still.unwrap().name, "OrgA", "OrgA name must be unchanged");
}

#[tokio::test]
async fn cross_tenant_cannot_touch_apps() {
    let db = build_db().await;
    let (_owner_a, sess_a, csrf_a) = seed_user(&db, "owner_a@xt-app.com").await;
    let (_attacker_b, sess_b, csrf_b) = seed_user(&db, "attacker_b@xt-app.com").await;
    let app = create_router(build_state(db.clone(), None).await);
    let (_org_a, app_a) = create_org_returning_app(&app, &sess_a, &csrf_a, "OrgA").await;

    let req = Request::builder()
        .uri(format!("/v1/apps/{}", app_a))
        .header("cookie", cookie(&sess_b))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant GET app must be 403"
    );

    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/v1/apps/{}", app_a))
        .header("cookie", cookie(&sess_b))
        .header("x-csrf-token", &csrf_b)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Pwned" })).unwrap()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant PATCH app must be 403"
    );

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/apps/{}", app_a))
        .header("cookie", cookie(&sess_b))
        .header("x-csrf-token", &csrf_b)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant DELETE app must be 403"
    );

    assert!(
        db.get_application(&app_a).await.unwrap().is_some(),
        "app A must survive cross-tenant attempts"
    );
}

#[tokio::test]
async fn cross_tenant_cannot_manage_members() {
    let db = build_db().await;
    let (owner_a, sess_a, csrf_a) = seed_user(&db, "owner_a@xt-mem.com").await;
    let (_attacker_b, sess_b, csrf_b) = seed_user(&db, "attacker_b@xt-mem.com").await;
    let app = create_router(build_state(db.clone(), None).await);
    let (org_a, _app_a) = create_org_returning_app(&app, &sess_a, &csrf_a, "OrgA").await;

    // INVITE a member into OrgA.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/orgs/{}/members", org_a))
        .header("cookie", cookie(&sess_b))
        .header("x-csrf-token", &csrf_b)
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({ "email": "mole@evil.com", "role": "admin" })).unwrap(),
        ))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant invite member must be 403"
    );

    // CHANGE owner A's role.
    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/v1/orgs/{}/members/{}", org_a, owner_a.id))
        .header("cookie", cookie(&sess_b))
        .header("x-csrf-token", &csrf_b)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "role": "viewer" })).unwrap()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant patch member must be 403"
    );

    // REMOVE owner A.
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/orgs/{}/members/{}", org_a, owner_a.id))
        .header("cookie", cookie(&sess_b))
        .header("x-csrf-token", &csrf_b)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant remove member must be 403"
    );

    // TRANSFER ownership of OrgA.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/orgs/{}/transfer-owner", org_a))
        .header("cookie", cookie(&sess_b))
        .header("x-csrf-token", &csrf_b)
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({ "new_owner_user_id": owner_a.id })).unwrap(),
        ))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant transfer-owner must be 403"
    );

    // Owner A remains the Owner of OrgA.
    assert_eq!(
        db.get_membership(&org_a, &owner_a.id).await.unwrap(),
        Some(Role::Owner),
        "owner A must remain Owner after cross-tenant attempts"
    );
}

#[tokio::test]
async fn cross_tenant_cannot_touch_app_storage() {
    let db = build_db().await;
    let (_owner_a, sess_a, csrf_a) = seed_user(&db, "owner_a@xt-store.com").await;
    let (_attacker_b, sess_b, csrf_b) = seed_user(&db, "attacker_b@xt-store.com").await;
    let app = create_router(build_state(db.clone(), Some([9u8; 32])).await);
    let (_org_a, app_a) = create_org_returning_app(&app, &sess_a, &csrf_a, "OrgA").await;

    // READ another tenant's storage config.
    let req = Request::builder()
        .uri(format!("/v1/apps/{}/storage", app_a))
        .header("cookie", cookie(&sess_b))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant GET app storage must be 403"
    );

    // DELETE another tenant's storage config.
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/apps/{}/storage", app_a))
        .header("cookie", cookie(&sess_b))
        .header("x-csrf-token", &csrf_b)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant DELETE app storage must be 403"
    );
}

/// CSRF is enforced on mutating cookie-session requests. Each mutation below is
/// sent WITHOUT an `x-csrf-token` and must be rejected (403) by `csrf_middleware`
/// before the handler runs; a final request WITH the valid token succeeds.
#[tokio::test]
async fn session_mutations_require_csrf_token() {
    let db = build_db().await;
    let (_owner, sess, csrf) = seed_user(&db, "owner@csrf.com").await;
    let app = create_router(build_state(db.clone(), Some([3u8; 32])).await);
    let (org_id, app_id) = create_org_returning_app(&app, &sess, &csrf, "OrgC").await;

    let cases: Vec<(&str, String)> = vec![
        ("POST", format!("/v1/orgs/{}/members", org_id)),
        ("POST", format!("/v1/orgs/{}/apps", org_id)),
        ("PUT", format!("/v1/apps/{}/storage", app_id)),
        ("PATCH", "/v1/account".to_string()),
    ];
    for (method, uri) in cases {
        let req = Request::builder()
            .method(method)
            .uri(uri.as_str())
            .header("cookie", cookie(&sess))
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "{} {} without CSRF token must be 403",
            method,
            uri
        );
    }

    // Sanity: the SAME mutation WITH a valid CSRF token is not blocked by CSRF.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/orgs/{}/apps", org_id))
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "WithCsrf" })).unwrap()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK,
        "a valid CSRF token must allow the mutation"
    );
}

#[tokio::test]
async fn viewer_denied_app_writes() {
    let db = build_db().await;
    let (_owner, sess_o, csrf_o) = seed_user(&db, "owner@vw.com").await;
    let (viewer, sess_v, csrf_v) = seed_user(&db, "viewer@vw.com").await;
    let app = create_router(build_state(db.clone(), None).await);
    let (org_id, app_id) = create_org_returning_app(&app, &sess_o, &csrf_o, "OrgV").await;
    db.add_org_member(&org_id, &viewer.id, Role::Viewer).await.unwrap();

    // CREATE app.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/orgs/{}/apps", org_id))
        .header("cookie", cookie(&sess_v))
        .header("x-csrf-token", &csrf_v)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Nope" })).unwrap()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "viewer must not create apps"
    );

    // PATCH app.
    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/v1/apps/{}", app_id))
        .header("cookie", cookie(&sess_v))
        .header("x-csrf-token", &csrf_v)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Nope" })).unwrap()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "viewer must not modify apps"
    );

    // DELETE app.
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/apps/{}", app_id))
        .header("cookie", cookie(&sess_v))
        .header("x-csrf-token", &csrf_v)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "viewer must not delete apps"
    );
}

#[tokio::test]
async fn member_denied_member_management() {
    let db = build_db().await;
    let (owner, sess_o, csrf_o) = seed_user(&db, "owner@mm.com").await;
    let (member, sess_m, csrf_m) = seed_user(&db, "member@mm.com").await;
    let app = create_router(build_state(db.clone(), None).await);
    let (org_id, _app_id) = create_org_returning_app(&app, &sess_o, &csrf_o, "OrgM").await;
    db.add_org_member(&org_id, &member.id, Role::Member).await.unwrap();

    // A plain Member cannot change another member's role.
    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/v1/orgs/{}/members/{}", org_id, owner.id))
        .header("cookie", cookie(&sess_m))
        .header("x-csrf-token", &csrf_m)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "role": "viewer" })).unwrap()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "member must not change member roles"
    );

    // A plain Member cannot remove a member.
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/orgs/{}/members/{}", org_id, owner.id))
        .header("cookie", cookie(&sess_m))
        .header("x-csrf-token", &csrf_m)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "member must not remove members"
    );

    // Owner is untouched.
    assert_eq!(
        db.get_membership(&org_id, &owner.id).await.unwrap(),
        Some(Role::Owner),
        "owner membership must be unchanged"
    );
}

// ─── Org provider credentials: store, list, delete ───────────────────────────

/// An outsider (a user NOT a member of org A) must receive 403 on GET, POST,
/// and DELETE against `/v1/orgs/{org_a}/provider-credentials`.
#[tokio::test]
async fn org_provider_credential_cross_tenant_403() {
    let db = build_db().await;
    let (_owner, sess_a, csrf_a) = seed_user(&db, "owner@cred-bola.com").await;
    let (_outsider, sess_b, csrf_b) = seed_user(&db, "outsider@cred-bola.com").await;
    let app = create_router(build_state(db.clone(), Some([7u8; 32])).await);

    // Owner creates the org (becomes member).
    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(&sess_a))
        .header("x-csrf-token", &csrf_a)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "OrgA" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "setup: create org");
    let org_a = body_json(resp).await["id"].as_str().unwrap().to_string();

    // Outsider: GET → 403.
    let req = Request::builder()
        .uri(format!("/v1/orgs/{}/provider-credentials", org_a))
        .header("cookie", cookie(&sess_b))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant GET provider-credentials must be 403"
    );

    // Outsider: POST → 403.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/orgs/{}/provider-credentials", org_a))
        .header("cookie", cookie(&sess_b))
        .header("x-csrf-token", &csrf_b)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({
            "provider": "openai",
            "credentials": { "api_key": "sk-hax0r" }
        })).unwrap()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant POST provider-credentials must be 403"
    );

    // Outsider: DELETE → 403.
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/orgs/{}/provider-credentials/openai", org_a))
        .header("cookie", cookie(&sess_b))
        .header("x-csrf-token", &csrf_b)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "cross-tenant DELETE provider-credentials must be 403"
    );
}

/// Without a secrets key (`build_state(db, None)`), POST to
/// `/v1/orgs/{org_id}/provider-credentials` returns 400 with
/// `code = secrets_not_configured`.
#[tokio::test]
async fn org_provider_credential_post_400_without_secrets_key() {
    let db = build_db().await;
    let (_owner, sess, csrf) = seed_user(&db, "owner@cred-nokey.com").await;
    // No secrets key → state.secrets_key = None.
    let app = create_router(build_state(db.clone(), None).await);

    // Create org.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/orgs")
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "NoKeyOrg" })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "setup: create org");
    let org_id = body_json(resp).await["id"].as_str().unwrap().to_string();

    // POST credential → 400 secrets_not_configured.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/orgs/{}/provider-credentials", org_id))
        .header("cookie", cookie(&sess))
        .header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({
            "provider": "openai",
            "credentials": { "api_key": "sk-test" }
        })).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "must be 400 when secrets key absent");
    let body = body_json(resp).await;
    assert_eq!(
        body["error"]["code"].as_str().unwrap_or(""),
        "secrets_not_configured",
        "error code must be secrets_not_configured"
    );
}

#[tokio::test]
async fn org_provider_credential_store_list_delete() {
    let db = build_db().await;
    let (_, sess, csrf) = seed_user(&db, "owner@test.com").await;
    let app = create_router(build_state(db.clone(), Some([7u8; 32])).await);

    // Create org → org_id.
    let req = Request::builder().method("POST").uri("/v1/orgs")
        .header("cookie", cookie(&sess)).header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Acme" })).unwrap())).unwrap();
    let org_id = body_json(app.clone().oneshot(req).await.unwrap()).await["id"].as_str().unwrap().to_string();

    // POST a credential → masked hint, no plaintext.
    let req = Request::builder().method("POST")
        .uri(format!("/v1/orgs/{}/provider-credentials", org_id))
        .header("cookie", cookie(&sess)).header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "provider": "openai", "credentials": { "api_key": "sk-secret1234" } })).unwrap())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let info = body_json(resp).await;
    assert_eq!(info["provider"], "openai");
    assert_eq!(info["display_hint"], "…1234");
    assert!(!info.to_string().contains("sk-secret1234"), "plaintext must not leak");

    // GET list.
    let req = Request::builder().uri(format!("/v1/orgs/{}/provider-credentials", org_id))
        .header("cookie", cookie(&sess)).body(Body::empty()).unwrap();
    let list = body_json(app.clone().oneshot(req).await.unwrap()).await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    // DELETE → 204, then empty.
    let req = Request::builder().method("DELETE")
        .uri(format!("/v1/orgs/{}/provider-credentials/openai", org_id))
        .header("cookie", cookie(&sess)).header("x-csrf-token", &csrf).body(Body::empty()).unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::NO_CONTENT);
}

/// The platform admin (master key) is bound to no tenant (no session user, no DB
/// key, no session id), so it is not a "member" of any org. It must still be able
/// to manage ANY org's provider credentials — this is the BYO-key admin path used
/// by hosted ops to set keys without an env file or a browser session. A logged-in
/// NON-member session must remain 403, proving the bypass is platform-admin only.
#[tokio::test]
async fn platform_admin_master_key_manages_any_org_provider_credentials() {
    let db = build_db().await;
    let (_, owner_sess, owner_csrf) = seed_user(&db, "owner@admincred.com").await;
    let (_, outsider_sess, outsider_csrf) = seed_user(&db, "outsider@admincred.com").await;
    // master_key set + secrets key configured.
    let app = create_router(build_state_full(db.clone(), Some([7u8; 32]), Some("master".to_string())).await);

    // A tenant creates an org. The master key is NOT a member of it.
    let req = Request::builder().method("POST").uri("/v1/orgs")
        .header("cookie", cookie(&owner_sess)).header("x-csrf-token", &owner_csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "TenantOrg" })).unwrap())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "setup: create org");
    let org_id = body_json(resp).await["id"].as_str().unwrap().to_string();

    // Master key (Bearer, no session cookie) stores a fal credential → 200.
    let req = Request::builder().method("POST")
        .uri(format!("/v1/orgs/{}/provider-credentials", org_id))
        .header("authorization", "Bearer master")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({
            "provider": "fal", "credentials": { "api_key": "fal-secret-9999" }
        })).unwrap())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "platform admin must store ANY org's credential");
    let info = body_json(resp).await;
    assert_eq!(info["provider"], "fal");
    assert!(!info.to_string().contains("fal-secret-9999"), "plaintext must not leak");

    // Master key lists → sees exactly the stored credential.
    let req = Request::builder().uri(format!("/v1/orgs/{}/provider-credentials", org_id))
        .header("authorization", "Bearer master").body(Body::empty()).unwrap();
    let list = body_json(app.clone().oneshot(req).await.unwrap()).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "platform admin must list ANY org's credentials");

    // Boundary: a logged-in NON-member session is still 403 (bypass is admin-only).
    let req = Request::builder().method("POST")
        .uri(format!("/v1/orgs/{}/provider-credentials", org_id))
        .header("cookie", cookie(&outsider_sess)).header("x-csrf-token", &outsider_csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({
            "provider": "fal", "credentials": { "api_key": "x" }
        })).unwrap())).unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "non-member session must remain 403 — bypass must not apply to ordinary sessions"
    );

    // Master key deletes → 204.
    let req = Request::builder().method("DELETE")
        .uri(format!("/v1/orgs/{}/provider-credentials/fal", org_id))
        .header("authorization", "Bearer master").body(Body::empty()).unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::NO_CONTENT,
        "platform admin must delete ANY org's credential"
    );
}

#[tokio::test]
async fn internal_error_does_not_leak_the_raw_detail() {
    // A raw DB/sqlx error string (table/column/constraint names) must never
    // reach an untrusted caller — it is logged server-side and the response
    // body carries only a generic message.
    let secret_detail =
        "error returned from database: value too long for type character varying(255) in table users";
    let resp = super::internal_error(secret_detail);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        !text.contains("character varying")
            && !text.contains("table users")
            && !text.contains(secret_detail),
        "500 body must not echo the raw DB error detail, got: {text}"
    );
    // But it must still be a well-formed error envelope.
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "internal_error");
}

#[tokio::test]
async fn deleting_a_provider_credential_writes_an_audit_entry() {
    // Privileged, billing-sensitive mutations must be attributable after the
    // fact. Deleting a provider credential must leave an audit_log entry.
    let db = build_db().await;
    let (_, sess, csrf) = seed_user(&db, "owner@test.com").await;
    let app = create_router(build_state(db.clone(), Some([7u8; 32])).await);

    let req = Request::builder().method("POST").uri("/v1/orgs")
        .header("cookie", cookie(&sess)).header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "name": "Audited" })).unwrap())).unwrap();
    let org_id = body_json(app.clone().oneshot(req).await.unwrap()).await["id"].as_str().unwrap().to_string();

    let req = Request::builder().method("POST")
        .uri(format!("/v1/orgs/{}/provider-credentials", org_id))
        .header("cookie", cookie(&sess)).header("x-csrf-token", &csrf)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&json!({ "provider": "openai", "credentials": { "api_key": "sk-secret1234" } })).unwrap())).unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::OK);

    let req = Request::builder().method("DELETE")
        .uri(format!("/v1/orgs/{}/provider-credentials/openai", org_id))
        .header("cookie", cookie(&sess)).header("x-csrf-token", &csrf).body(Body::empty()).unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::NO_CONTENT);

    // log_audit is fire-and-forget (tokio::spawn); let it land.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let (entries, _) = db.list_audit_log_for_tenant(&org_id, 1, 100).await.unwrap();
    assert!(
        entries.iter().any(|e| e.action == "org.provider_credential.delete"),
        "deleting a provider credential must write an audit entry, got: {:?}",
        entries.iter().map(|e| e.action.clone()).collect::<Vec<_>>()
    );
}

/// Security fix 1 (2026-11-05): in hosted mode `app_bucket` checks a bucket
/// at every use, so one whose requests would go inside the deployment is
/// unavailable however its row was saved. The same row is a usable bucket
/// where the operator may name internal stores (single-tenant).
#[tokio::test]
async fn an_app_bucket_inside_the_deployment_is_unavailable_when_checked() {
    let db = build_db().await;
    let (_owner, sess, csrf) = seed_user(&db, "owner@bucket-use.com").await;
    let key = [9u8; 32];
    let app = create_router(build_state(db.clone(), Some(key)).await);
    let (_org, app_id) = create_org_returning_app(&app, &sess, &csrf, "BucketUse").await;
    let (secret_ciphertext, secret_nonce) =
        crate::auth::secrets::encrypt(&key, br#"{"access_key_id":"AKIATEST","secret_access_key":"secret"}"#).unwrap();
    db.upsert_app_storage(&crate::types::AppStorageUpsert {
        app_id: app_id.clone(),
        backend: "s3".into(),
        bucket_name: "b".into(),
        region: "us-east-1".into(),
        endpoint_url: Some("http://169.254.169.254".into()),
        custom_public_url: None,
        path_prefix: None,
        access_key_id_hint: Some("AKIA…TEST".into()),
        secret_ciphertext,
        secret_nonce,
        credential_mode: None,
        role_arn: None,
        fallback_to_litegen_store: Some(true),
    })
    .await
    .unwrap();
    let db: Arc<dyn DatabaseStore> = db;
    let aws = crate::config::AwsConfig::default();

    match crate::api::handlers::app_bucket(&db, Some(key), &aws, crate::api::handlers::TenantBucket::Hosted, &app_id).await {
        crate::api::handlers::AppBucket::Unavailable { reason, fallback } => {
            assert!(reason.contains("may not be used"), "{reason}");
            assert!(fallback, "litegen's own store may still stand in");
        }
        _ => panic!("a bucket on the metadata address must be unavailable in hosted mode"),
    }
    assert!(
        matches!(
            crate::api::handlers::app_bucket(&db, Some(key), &aws, crate::api::handlers::TenantBucket::Operator, &app_id).await,
            crate::api::handlers::AppBucket::Ready { .. }
        ),
        "without the hosted check the same row is a usable bucket"
    );
}
