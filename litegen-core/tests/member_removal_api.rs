//! Removing someone from the single-tenant default org ends what they can do
//! (security fix 4, 2026-11-05). Their sessions end, and a person with no
//! org can't start or change generations. Such a generation would otherwise
//! run on the deployment's own vendor keys, with no budget, model allowlist
//! or rate cap, since those all hang off the org.

mod harness;
mod mcp_support;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use litegen::config::Mode;
use litegen::db::sqlite::SqliteDatabase;
use serde_json::{json, Value};
use tower::ServiceExt;

const MASTER_KEY: &str = "test-master-key";
const PASSWORD: &str = "a-long-test-password-1";
const MEMBER_EMAIL: &str = "member@example.com";

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

async fn send(app: &Router, method: &str, path: &str, body: Option<Value>, headers: &[(&str, &str)]) -> Reply {
    let mut req = Request::builder().method(method).uri(path);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 10 * 1024 * 1024).await.unwrap();
    Reply { status, headers, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null) }
}

/// A signed-in browser session: its cookie and CSRF token.
struct Session {
    cookie: String,
    csrf: String,
}

impl Session {
    async fn from_reply(app: &Router, reply: &Reply) -> Session {
        let cookie = reply
            .headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|s| s.split(';').next())
            .find(|kv| kv.starts_with("litegen_session="))
            .expect("litegen_session cookie")
            .to_string();
        let csrf = send(app, "GET", "/v1/auth/csrf", None, &[("cookie", &cookie)]).await;
        assert_eq!(csrf.status, StatusCode::OK, "csrf: {:?}", csrf.body);
        Session { cookie, csrf: csrf.body["csrf_token"].as_str().unwrap().to_string() }
    }

    async fn call(&self, app: &Router, method: &str, path: &str, body: Option<Value>) -> Reply {
        send(app, method, path, body, &[("cookie", &self.cookie), ("x-csrf-token", &self.csrf)]).await
    }

    async fn generate(&self, app: &Router) -> Reply {
        self.call(app, "POST", "/v1/images/generations", Some(json!({ "model": "mock/image-gen", "prompt": "a fox" }))).await
    }
}

/// Single-tenant app with a master key: the first signup is the owner, and an
/// invited member joins the default org.
async fn app() -> (Router, tempfile::TempDir) {
    let (app, _db, dir) = app_and_db().await;
    (app, dir)
}

async fn app_and_db() -> (Router, Arc<SqliteDatabase>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let app = harness::app_over_with(db.clone(), dir.path(), |c| c.dev.expose_invite_tokens = true, Some(MASTER_KEY), Mode::SingleTenant).await;
    (app, db, dir)
}

/// [`app_and_db`] over a database file, and a second connection to that file
/// to break the database with.
async fn app_on_file() -> (Router, Arc<SqliteDatabase>, sqlx::SqlitePool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}/litegen.db?mode=rwc", dir.path().display());
    let db = Arc::new(SqliteDatabase::connect(&url).await.unwrap());
    let app = harness::app_over_with(db.clone(), dir.path(), |c| c.dev.expose_invite_tokens = true, Some(MASTER_KEY), Mode::SingleTenant).await;
    let raw = sqlx::SqlitePool::connect(&url).await.unwrap();
    (app, db, raw, dir)
}

async fn owner(app: &Router) -> Session {
    let r = send(app, "POST", "/v1/auth/signup", Some(json!({ "email": "owner@example.com", "password": PASSWORD })), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "owner signup: {:?}", r.body);
    Session::from_reply(app, &r).await
}

/// An invited member's session, with the default org's id and the member's id.
async fn member(app: &Router, owner: &Session) -> (Session, String, String) {
    let orgs = owner.call(app, "GET", "/v1/orgs", None).await;
    let org_id = orgs.body[0]["id"].as_str().expect("owner's org").to_string();
    let invite = owner
        .call(app, "POST", &format!("/v1/orgs/{org_id}/members"), Some(json!({ "email": MEMBER_EMAIL, "role": "member" })))
        .await;
    assert_eq!(invite.status, StatusCode::OK, "invite: {:?}", invite.body);
    let token = invite.body["_dev_token"].as_str().expect("dev invite token").to_string();
    let accept = send(app, "POST", &format!("/v1/auth/invitations/{token}/accept"), Some(json!({ "password": PASSWORD })), &[]).await;
    assert_eq!(accept.status, StatusCode::OK, "accept: {:?}", accept.body);
    let session = Session::from_reply(app, &accept).await;
    let me = session.call(app, "GET", "/v1/auth/me", None).await;
    let user_id = me.body["user"]["id"].as_str().expect("member id").to_string();
    (session, org_id, user_id)
}

async fn remove(app: &Router, owner: &Session, org_id: &str, user_id: &str) {
    let r = owner.call(app, "DELETE", &format!("/v1/orgs/{org_id}/members/{user_id}"), None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "remove member: {:?}", r.body);
}

async fn sign_in_again(app: &Router) -> Session {
    let r = send(app, "POST", "/v1/auth/login", Some(json!({ "email": MEMBER_EMAIL, "password": PASSWORD })), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "the account is still active, so it signs in: {:?}", r.body);
    Session::from_reply(app, &r).await
}

#[tokio::test]
async fn removing_a_member_ends_their_sessions() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (member, org_id, user_id) = member(&app, &owner).await;
    let before = member.generate(&app).await;
    assert_eq!(before.status, StatusCode::OK, "a member generates: {:?}", before.body);

    remove(&app, &owner, &org_id, &user_id).await;

    let after = member.generate(&app).await;
    assert_eq!(after.status, StatusCode::UNAUTHORIZED, "the removed member's session is gone: {:?}", after.body);
    // The owner's own session is untouched.
    assert_eq!(owner.generate(&app).await.status, StatusCode::OK);
}

#[tokio::test]
async fn a_removed_member_who_signs_in_again_cannot_generate() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (member, org_id, user_id) = member(&app, &owner).await;
    let made = member.generate(&app).await;
    assert_eq!(made.status, StatusCode::OK, "{:?}", made.body);
    let id = made.body["id"].as_str().expect("generation id").to_string();

    remove(&app, &owner, &org_id, &user_id).await;
    let again = sign_in_again(&app).await;

    let r = again.generate(&app).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "no org, so no generation on the deployment's keys: {:?}", r.body);
    assert_eq!(r.body["error"]["code"], "no_active_org", "{:?}", r.body);
    for (method, path) in [
        ("POST", "/v1/videos/generations".to_string()),
        ("POST", "/v1/models3d/generations".to_string()),
        ("POST", "/v1/images/cost".to_string()),
        ("POST", "/v1/utils/models3d/convert".to_string()),
        ("GET", "/v1/videos/litegen-vid-x".to_string()),
        ("POST", format!("/v1/generations/{id}/revoke-links")),
        ("DELETE", format!("/v1/generations/{id}")),
    ] {
        let r = again.call(&app, method, &path, Some(json!({ "model": "mock/video-gen", "prompt": "a wave" }))).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{method} {path}: {:?}", r.body);
        assert_eq!(r.body["error"]["code"], "no_active_org", "{method} {path}: {:?}", r.body);
    }
}

/// Only the last membership ends the sessions: leaving another org while
/// still in the default one changes nothing for them.
#[tokio::test]
async fn removal_from_another_org_keeps_the_sessions() {
    let (app, db, _dir) = app_and_db().await;
    let owner = owner(&app).await;
    let (member, _default_org, user_id) = member(&app, &owner).await;
    let owner_id = owner.call(&app, "GET", "/v1/auth/me", None).await.body["user"]["id"].as_str().unwrap().to_string();
    let now = chrono::Utc::now();
    let side = litegen::types::Organization {
        id: uuid::Uuid::new_v4().to_string(),
        name: "Side".into(),
        slug: "side".into(),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    use litegen::db::DatabaseStore;
    db.create_organization(&side).await.unwrap();
    db.add_org_member(&side.id, &owner_id, litegen::types::Role::Owner).await.unwrap();
    db.add_org_member(&side.id, &user_id, litegen::types::Role::Member).await.unwrap();

    remove(&app, &owner, &side.id, &user_id).await;

    let r = member.generate(&app).await;
    assert_eq!(r.status, StatusCode::OK, "still a member of the default org: {:?}", r.body);
}

// ─── A person's API keys end with them (2026-11-06) ─────────────────────────

/// A person `owner` invited to the default org with `role`, signed in: their
/// session, the org's id and their id.
async fn invited(app: &Router, owner: &Session, email: &str, role: &str) -> (Session, String, String) {
    let orgs = owner.call(app, "GET", "/v1/orgs", None).await;
    let org_id = orgs.body[0]["id"].as_str().expect("owner's org").to_string();
    let invite = owner.call(app, "POST", &format!("/v1/orgs/{org_id}/members"), Some(json!({ "email": email, "role": role }))).await;
    assert_eq!(invite.status, StatusCode::OK, "invite: {:?}", invite.body);
    let token = invite.body["_dev_token"].as_str().expect("dev invite token").to_string();
    let accept = send(app, "POST", &format!("/v1/auth/invitations/{token}/accept"), Some(json!({ "password": PASSWORD })), &[]).await;
    assert_eq!(accept.status, StatusCode::OK, "accept: {:?}", accept.body);
    let session = Session::from_reply(app, &accept).await;
    let me = session.call(app, "GET", "/v1/auth/me", None).await;
    let user_id = me.body["user"]["id"].as_str().expect("user id").to_string();
    (session, org_id, user_id)
}

/// A key with `scopes` made by whoever `auth` signs in as: its secret.
async fn key_made_with(app: &Router, auth: &[(&str, &str)], scopes: &str) -> String {
    let r = send(app, "POST", "/v1/keys", Some(json!({ "name": "a key", "scopes": scopes })), auth).await;
    assert_eq!(r.status, StatusCode::CREATED, "create key: {:?}", r.body);
    r.body["key"].as_str().expect("key secret").to_string()
}

async fn key_made_by(app: &Router, session: &Session, scopes: &str) -> String {
    key_made_with(app, &[("cookie", &session.cookie), ("x-csrf-token", &session.csrf)], scopes).await
}

fn bearer(key: &str) -> String {
    format!("Bearer {key}")
}

/// What `key` gets for a generation, and for a gateway-admin read.
async fn key_can(app: &Router, key: &str) -> (StatusCode, StatusCode) {
    let auth = bearer(key);
    let made = send(app, "POST", "/v1/images/generations", Some(json!({ "model": "mock/image-gen", "prompt": "a fox" })), &[("authorization", &auth)]).await;
    let admin = send(app, "GET", "/v1/admin/settings", None, &[("authorization", &auth)]).await;
    (made.status, admin.status)
}

const ALL: &str = "generate,read,admin";

/// A key acts for the person who made it. Otherwise an Admin removed from the
/// deployment keeps generating on its vendor keys and keeps running the
/// gateway through an admin-scope key made before.
#[tokio::test]
async fn a_removed_members_api_keys_stop_working() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, org_id, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let key = key_made_by(&app, &admin, ALL).await;
    assert_eq!(key_can(&app, &key).await, (StatusCode::OK, StatusCode::OK), "an admin-scope key of the default org is a gateway admin");

    remove(&app, &owner, &org_id, &admin_id).await;

    assert_eq!(key_can(&app, &key).await, (StatusCode::UNAUTHORIZED, StatusCode::UNAUTHORIZED));
}

#[tokio::test]
async fn a_deactivated_persons_api_keys_stop_working() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, _org_id, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let key = key_made_by(&app, &admin, ALL).await;

    let r = owner.call(&app, "DELETE", &format!("/v1/users/{admin_id}"), None).await;
    assert!(r.status.is_success(), "deactivate: {:?}", r.body);

    assert_eq!(key_can(&app, &key).await, (StatusCode::UNAUTHORIZED, StatusCode::UNAUTHORIZED));
}

/// A key never carries more authority than its maker could grant now: an Admin
/// demoted to Member keeps a working key, but not its `admin` scope.
#[tokio::test]
async fn a_demoted_admins_key_loses_its_admin_scope() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, org_id, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let key = key_made_by(&app, &admin, ALL).await;

    let r = owner.call(&app, "PATCH", &format!("/v1/orgs/{org_id}/members/{admin_id}"), Some(json!({ "role": "member" }))).await;
    assert!(r.status.is_success(), "demote: {:?}", r.body);

    assert_eq!(key_can(&app, &key).await, (StatusCode::OK, StatusCode::FORBIDDEN), "a Member generates, but isn't a gateway admin");
}

/// A key made by a key belongs to the same person: otherwise an Admin's key
/// could mint keys that outlive the Admin.
#[tokio::test]
async fn a_key_made_by_a_key_ends_with_the_same_person() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, org_id, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let parent = key_made_by(&app, &admin, ALL).await;
    let child = key_made_with(&app, &[("authorization", &bearer(&parent))], ALL).await;
    assert_eq!(key_can(&app, &child).await, (StatusCode::OK, StatusCode::OK));

    remove(&app, &owner, &org_id, &admin_id).await;

    assert_eq!(key_can(&app, &child).await, (StatusCode::UNAUTHORIZED, StatusCode::UNAUTHORIZED));
}

/// Keys made with the master key belong to the deployment rather than to a
/// person, so nobody's removal ends them.
#[tokio::test]
async fn keys_made_with_the_master_key_outlive_every_member() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, org_id, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let service = key_made_with(&app, &[("authorization", &bearer(MASTER_KEY))], ALL).await;
    let own = key_made_by(&app, &admin, "generate,read").await;

    remove(&app, &owner, &org_id, &admin_id).await;

    assert_eq!(key_can(&app, &service).await, (StatusCode::OK, StatusCode::OK));
    assert_eq!(key_can(&app, &own).await.0, StatusCode::UNAUTHORIZED);
}

// ─── A rotated secret acts for whoever rotated it (2026-11-11) ──────────────

/// A key with `scopes` made by whoever `auth` signs in as: its id and secret.
async fn new_key(app: &Router, auth: &[(&str, &str)], scopes: &str) -> (String, String) {
    let r = send(app, "POST", "/v1/keys", Some(json!({ "name": "a key", "scopes": scopes })), auth).await;
    assert_eq!(r.status, StatusCode::CREATED, "create key: {:?}", r.body);
    (r.body["id"].as_str().expect("key id").to_string(), r.body["key"].as_str().expect("key secret").to_string())
}

/// Rotate key `id` as whoever `auth` signs in as: the new secret.
async fn rotate(app: &Router, auth: &[(&str, &str)], id: &str) -> String {
    let r = send(app, "POST", &format!("/v1/keys/{id}/rotate"), None, auth).await;
    assert_eq!(r.status, StatusCode::OK, "rotate: {:?}", r.body);
    r.body["key"].as_str().expect("new secret").to_string()
}

fn signed_in(session: &Session) -> [(&'static str, &str); 2] {
    [("cookie", &session.cookie), ("x-csrf-token", &session.csrf)]
}

/// Rotating a key hands its only working secret to whoever rotates it, so the
/// key then acts for them. Otherwise an Admin rotates the deployment's service
/// key, or anyone's, before leaving, and keeps a gateway-admin secret.
#[tokio::test]
async fn a_secret_an_admin_rotated_ends_with_them() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, org_id, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let (service, _) = new_key(&app, &[("authorization", &bearer(MASTER_KEY))], ALL).await;
    let (owners_key, _) = new_key(&app, &signed_in(&owner), ALL).await;
    let rotated_service = rotate(&app, &signed_in(&admin), &service).await;
    let rotated_owners = rotate(&app, &signed_in(&admin), &owners_key).await;
    assert_eq!(key_can(&app, &rotated_service).await, (StatusCode::OK, StatusCode::OK));

    remove(&app, &owner, &org_id, &admin_id).await;

    assert_eq!(
        key_can(&app, &rotated_service).await,
        (StatusCode::UNAUTHORIZED, StatusCode::UNAUTHORIZED),
        "the deployment's key, rotated by the Admin"
    );
    assert_eq!(
        key_can(&app, &rotated_owners).await,
        (StatusCode::UNAUTHORIZED, StatusCode::UNAUTHORIZED),
        "the Owner's key, rotated by the Admin"
    );
}

/// The same through a key: a rotation by a person's key counts as theirs.
#[tokio::test]
async fn a_secret_rotated_with_a_persons_key_ends_with_that_person() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, org_id, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let (_, admins_key) = new_key(&app, &signed_in(&admin), ALL).await;
    let (service, _) = new_key(&app, &[("authorization", &bearer(MASTER_KEY))], ALL).await;
    let rotated = rotate(&app, &[("authorization", &bearer(&admins_key))], &service).await;

    remove(&app, &owner, &org_id, &admin_id).await;

    assert_eq!(key_can(&app, &rotated).await, (StatusCode::UNAUTHORIZED, StatusCode::UNAUTHORIZED));
}

/// The master key acts for the deployment, not a person: what it rotates
/// keeps the owner it had.
#[tokio::test]
async fn a_secret_the_master_key_rotated_keeps_its_owner() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, org_id, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let master_auth = bearer(MASTER_KEY);
    let master = [("authorization", master_auth.as_str())];
    let (admins_key, _) = new_key(&app, &signed_in(&admin), ALL).await;
    let (service, _) = new_key(&app, &master, ALL).await;
    let admins_rotated = rotate(&app, &master, &admins_key).await;
    let service_rotated = rotate(&app, &master, &service).await;

    remove(&app, &owner, &org_id, &admin_id).await;

    assert_eq!(key_can(&app, &admins_rotated).await, (StatusCode::UNAUTHORIZED, StatusCode::UNAUTHORIZED), "still the Admin's");
    assert_eq!(key_can(&app, &service_rotated).await, (StatusCode::OK, StatusCode::OK), "still the deployment's");
}

/// The audit log names the person behind a rotation, so an operator can find
/// the secrets someone who left was handed.
#[tokio::test]
async fn the_audit_log_names_the_person_who_rotated_a_key() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, _org_id, _admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let (service, _) = new_key(&app, &[("authorization", &bearer(MASTER_KEY))], ALL).await;
    rotate(&app, &signed_in(&admin), &service).await;

    // The entry is written in the background.
    let mut entry = Value::Null;
    for _ in 0..100 {
        let r = send(&app, "GET", "/v1/audit?action=key.rotate", None, &[("authorization", &bearer(MASTER_KEY))]).await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        if r.body["total"] == 1 {
            entry = r.body["data"][0].clone();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(entry["target_id"], service.as_str(), "{entry}");
    assert_eq!(entry["actor_label"], "admin@example.com", "{entry}");
}

/// A key holds `admin` only while its owner could grant it, so giving a
/// Member's key `admin` would do nothing; it is refused instead.
#[tokio::test]
async fn admin_scope_is_refused_for_a_key_whose_owner_cannot_hold_it() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (member, _org_id, _member_id) = member(&app, &owner).await;
    let (members_key, _) = new_key(&app, &signed_in(&member), "generate,read").await;

    let r = owner.call(&app, "PATCH", &format!("/v1/keys/{members_key}"), Some(json!({ "scopes": ALL }))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{:?}", r.body);
    assert!(r.body["error"]["message"].as_str().unwrap_or_default().contains("admin"), "{:?}", r.body);

    // The Owner's own key, and the deployment's, can still be given `admin`.
    let (owners_key, _) = new_key(&app, &signed_in(&owner), "generate,read").await;
    let r = owner.call(&app, "PATCH", &format!("/v1/keys/{owners_key}"), Some(json!({ "scopes": ALL }))).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let (service, _) = new_key(&app, &[("authorization", &bearer(MASTER_KEY))], "generate,read").await;
    let r = owner.call(&app, "PATCH", &format!("/v1/keys/{service}"), Some(json!({ "scopes": ALL }))).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
}

/// A key holds `generate` only while its owner may start generations, so
/// giving a Viewer's key `generate` would answer 200 and do nothing: the
/// scope is dropped at every request. It is refused instead, as `admin` is
/// for a Member's key, whoever edits it, and the key keeps its scopes.
#[tokio::test]
async fn generate_scope_is_refused_for_a_key_whose_owner_is_a_viewer() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (member, org_id, member_id) = member(&app, &owner).await;
    let (key_id, _) = new_key(&app, &signed_in(&member), "read").await;
    let r = owner.call(&app, "PATCH", &format!("/v1/orgs/{org_id}/members/{member_id}"), Some(json!({ "role": "viewer" }))).await;
    assert_eq!(r.status, StatusCode::OK, "make them a viewer: {:?}", r.body);

    let master = bearer(MASTER_KEY);
    for (who, auth) in [("the owner", signed_in(&owner).to_vec()), ("the master key", vec![("authorization", master.as_str())])] {
        let r = send(&app, "PATCH", &format!("/v1/keys/{key_id}"), Some(json!({ "scopes": "generate,read" })), &auth).await;
        assert_eq!((r.status, r.body["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")), "{who}: {:?}", r.body);
        assert!(r.body["error"]["message"].as_str().unwrap_or_default().contains("'generate'"), "{who}: {:?}", r.body);
    }
    let key = owner.call(&app, "GET", &format!("/v1/keys/{key_id}"), None).await;
    assert_eq!(key.body["scopes"], "read", "the key is unchanged: {:?}", key.body);

    // What the owner may hold is still accepted.
    let r = owner.call(&app, "PATCH", &format!("/v1/keys/{key_id}"), Some(json!({ "scopes": "read" }))).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
}

/// `/mcp` authenticates keys the same way, so a removed Admin's key ends there
/// too.
#[tokio::test]
async fn a_removed_admins_key_is_refused_on_mcp() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, org_id, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let (_, key) = new_key(&app, &signed_in(&admin), ALL).await;
    assert_ne!(mcp_status(&app, &key).await, StatusCode::UNAUTHORIZED);

    remove(&app, &owner, &org_id, &admin_id).await;

    assert_eq!(mcp_status(&app, &key).await, StatusCode::UNAUTHORIZED);
}

/// What `key` gets for a `tools/list` call on `/mcp`.
async fn mcp_status(app: &Router, key: &str) -> StatusCode {
    let auth = bearer(key);
    let headers = [("authorization", auth.as_str()), ("host", "localhost"), ("accept", "application/json, text/event-stream")];
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} });
    send(app, "POST", "/mcp", Some(body), &headers).await.status
}

// ─── A Viewer's keys only read (review 2026-11-12, security M2) ─────────────

/// What `key` gets for an image generation, for its quote and for
/// `GET /v1/generations`: each status, with a refusal's error code.
async fn key_answers(app: &Router, key: &str) -> [(StatusCode, String); 3] {
    let auth = bearer(key);
    let body = json!({ "model": "mock/image-gen", "prompt": "a fox" });
    let made = send(app, "POST", "/v1/images/generations", Some(body.clone()), &[("authorization", &auth)]).await;
    let quoted = send(app, "POST", "/v1/images/cost", Some(body), &[("authorization", &auth)]).await;
    let listed = send(app, "GET", "/v1/generations", None, &[("authorization", &auth)]).await;
    [made, quoted, listed].map(|r| (r.status, r.body["error"]["code"].as_str().unwrap_or_default().to_string()))
}

fn served() -> (StatusCode, String) {
    (StatusCode::OK, String::new())
}

fn no_scope() -> (StatusCode, String) {
    (StatusCode::FORBIDDEN, "forbidden_scope".to_string())
}

/// Some of the tools that start or change a generation.
const GENERATION_TOOLS: [&str; 6] = ["generate_image", "estimate_image_cost", "generate_video", "generate_3d_model", "cancel_generation", "delete_generation"];

/// The tools `key` lists on `/mcp`.
async fn ai_tools(app: &Router, key: &str) -> Vec<String> {
    mcp_support::Mcp::new(app, "/mcp").bearer(key).tool_names().await
}

/// `key` reads, and starts or changes no generation, over HTTP and on `/mcp`.
async fn assert_only_reads(app: &Router, key: &str, how: &str) {
    assert_eq!(key_answers(app, key).await, [no_scope(), no_scope(), served()], "{how}: a viewer's key reads and starts nothing");
    let listed = ai_tools(app, key).await;
    for tool in GENERATION_TOOLS {
        assert!(!listed.contains(&tool.to_string()), "{how}: a viewer's key still lists {tool}: {listed:?}");
    }
    assert!(listed.contains(&"get_generation".to_string()), "{how}: a viewer's key still reads generations on /mcp: {listed:?}");
}

/// Viewers only read (README's roles), and a key acts for its owner: once the
/// owner is a Viewer, the key loses `generate` and keeps `read`, as it loses
/// `admin` once its owner can't hold that. Otherwise a Member made a Viewer
/// keeps generating, quoting, polling, cancelling and deleting through a key
/// they made before.
#[tokio::test]
async fn a_member_made_a_viewer_keeps_reading_but_their_keys_stop_generating() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (member, org_id, member_id) = member(&app, &owner).await;
    let key = key_made_by(&app, &member, "generate,read").await;
    let generate_only = key_made_by(&app, &member, "generate").await;
    assert_eq!(key_answers(&app, &key).await, [served(), served(), served()], "a member's key generates, quotes and reads");
    assert_eq!(key_answers(&app, &generate_only).await, [served(), served(), served()], "`generate` reads generations too");
    let listed = ai_tools(&app, &key).await;
    for tool in GENERATION_TOOLS {
        assert!(listed.contains(&tool.to_string()), "a member's key lists {tool}: {listed:?}");
    }

    let r = owner.call(&app, "PATCH", &format!("/v1/orgs/{org_id}/members/{member_id}"), Some(json!({ "role": "viewer" }))).await;
    assert_eq!(r.status, StatusCode::OK, "make them a viewer: {:?}", r.body);

    assert_only_reads(&app, &key, "members page").await;
    assert_eq!(key_answers(&app, &generate_only).await, [no_scope(), no_scope(), no_scope()], "a viewer's generate-only key is left with nothing");
}

/// The same when the Owner makes someone a Viewer on the users page
/// (`PATCH /v1/users/{id}`), which in single-tenant mode sets their role in
/// the deployment's org.
#[tokio::test]
async fn a_person_made_a_viewer_on_the_users_page_loses_generate_on_their_keys() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (member, _org_id, member_id) = member(&app, &owner).await;
    let key = key_made_by(&app, &member, "generate,read").await;
    assert_eq!(key_answers(&app, &key).await, [served(), served(), served()], "a member's key generates, quotes and reads");

    let r = owner.call(&app, "PATCH", &format!("/v1/users/{member_id}"), Some(json!({ "role": "viewer" }))).await;
    assert_eq!(r.status, StatusCode::OK, "make them a viewer: {:?}", r.body);

    assert_only_reads(&app, &key, "users page").await;
}

// ─── Pending invitations end with the person (review 2026-11-11 SEC-INVITES) ──

/// An invitation `by` sends into `org_id`: its token.
async fn invitation(app: &Router, by: &Session, org_id: &str, email: &str, role: &str) -> String {
    let r = by.call(app, "POST", &format!("/v1/orgs/{org_id}/members"), Some(json!({ "email": email, "role": role }))).await;
    assert_eq!(r.status, StatusCode::OK, "invite: {:?}", r.body);
    r.body["_dev_token"].as_str().expect("dev invite token").to_string()
}

async fn accept(app: &Router, token: &str) -> Reply {
    send(app, "POST", &format!("/v1/auth/invitations/{token}/accept"), Some(json!({ "password": PASSWORD })), &[]).await
}

fn no_such_invitation(r: &Reply) -> bool {
    r.status == StatusCode::NOT_FOUND && r.body["error"]["code"] == "invitation_not_found"
}

async fn members_of(app: &Router, owner: &Session, org_id: &str) -> Vec<String> {
    let r = owner.call(app, "GET", &format!("/v1/orgs/{org_id}/members"), None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body.as_array().unwrap().iter().filter_map(|m| m["email"].as_str().map(str::to_string)).collect()
}

#[tokio::test]
async fn a_removed_member_cannot_rejoin_with_an_unused_invitation() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let org_id = owner.call(&app, "GET", "/v1/orgs", None).await.body[0]["id"].as_str().unwrap().to_string();
    let first = invitation(&app, &owner, &org_id, MEMBER_EMAIL, "member").await;
    let second = invitation(&app, &owner, &org_id, MEMBER_EMAIL, "member").await;
    let joined = accept(&app, &first).await;
    assert_eq!(joined.status, StatusCode::OK, "{:?}", joined.body);
    let me = Session::from_reply(&app, &joined).await.call(&app, "GET", "/v1/auth/me", None).await;
    let user_id = me.body["user"]["id"].as_str().expect("member id").to_string();
    remove(&app, &owner, &org_id, &user_id).await;
    let again = accept(&app, &second).await;
    assert!(no_such_invitation(&again), "a removed member's second invitation still admits them: {} {:?}", again.status, again.body);
    assert!(!members_of(&app, &owner, &org_id).await.contains(&MEMBER_EMAIL.to_string()), "a member again");
}

#[tokio::test]
async fn invitations_a_removed_admin_sent_end_with_them() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, org_id, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let sock = invitation(&app, &admin, &org_id, "sock@example.com", "admin").await;
    remove(&app, &owner, &org_id, &admin_id).await;
    let r = accept(&app, &sock).await;
    assert!(no_such_invitation(&r), "a removed admin's invitation still admits an admin: {} {:?}", r.status, r.body);
}

#[tokio::test]
async fn invitations_a_deactivated_admin_sent_end_with_them() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, org_id, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let sock = invitation(&app, &admin, &org_id, "sock@example.com", "admin").await;
    let r = owner.call(&app, "DELETE", &format!("/v1/users/{admin_id}"), None).await;
    assert!(r.status.is_success(), "deactivate: {:?}", r.body);
    let r = accept(&app, &sock).await;
    assert!(no_such_invitation(&r), "a deactivated admin's invitation still admits an admin: {} {:?}", r.status, r.body);
}

#[tokio::test]
async fn invitations_a_demoted_admin_sent_end_with_their_admin_role() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, org_id, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let sock = invitation(&app, &admin, &org_id, "sock@example.com", "admin").await;
    let r = owner.call(&app, "PATCH", &format!("/v1/orgs/{org_id}/members/{admin_id}"), Some(json!({ "role": "member" }))).await;
    assert_eq!(r.status, StatusCode::OK, "demote: {:?}", r.body);
    let r = accept(&app, &sock).await;
    assert!(no_such_invitation(&r), "a demoted admin's invitation still admits an admin: {} {:?}", r.status, r.body);
}

/// The users page deactivates (`PATCH /v1/users/{id}` with `is_active: false`)
/// as `DELETE /v1/users/{id}` does: the invitations they sent go too.
#[tokio::test]
async fn invitations_an_admin_deactivated_on_the_users_page_sent_end_with_them() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, org_id, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let sock = invitation(&app, &admin, &org_id, "sock@example.com", "admin").await;
    let r = owner.call(&app, "PATCH", &format!("/v1/users/{admin_id}"), Some(json!({ "is_active": false }))).await;
    assert_eq!(r.status, StatusCode::OK, "deactivate: {:?}", r.body);
    let r = accept(&app, &sock).await;
    assert!(no_such_invitation(&r), "an admin deactivated on the users page still admits an admin: {} {:?}", r.status, r.body);
}

/// In single-tenant mode the users page sets the role in the deployment's
/// org, so a demotion there ends the invitations they sent, as on the
/// members page.
#[tokio::test]
async fn invitations_an_admin_demoted_on_the_users_page_sent_end_with_their_admin_role() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, org_id, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let sock = invitation(&app, &admin, &org_id, "sock@example.com", "admin").await;
    let r = owner.call(&app, "PATCH", &format!("/v1/users/{admin_id}"), Some(json!({ "role": "member" }))).await;
    assert_eq!(r.status, StatusCode::OK, "demote: {:?}", r.body);
    let r = accept(&app, &sock).await;
    assert!(no_such_invitation(&r), "an admin demoted on the users page still admits an admin: {} {:?}", r.status, r.body);
}

/// A PATCH that deactivates someone revokes the invitations they sent before
/// it changes anything: when that fails (a 500), their role is as it was
/// too, so the PATCH can be sent again whole.
#[tokio::test]
async fn a_deactivation_whose_invitations_cannot_be_revoked_changes_nothing() {
    use litegen::db::DatabaseStore;
    let (app, db, raw, _dir) = app_on_file().await;
    let owner = owner(&app).await;
    let (_member, org_id, member_id) = member(&app, &owner).await;
    // A storage failure at the revoke: the table it deletes from is gone.
    sqlx::query("ALTER TABLE invitations RENAME TO invitations_away").execute(&raw).await.unwrap();

    let r = owner.call(&app, "PATCH", &format!("/v1/users/{member_id}"), Some(json!({ "role": "viewer", "is_active": false }))).await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR, "{:?}", r.body);
    assert_eq!(
        db.get_membership(&org_id, &member_id).await.unwrap(),
        Some(litegen::types::Role::Member),
        "the PATCH failed, but their role changed"
    );
    assert!(db.get_user_by_id(&member_id).await.unwrap().expect("the account").is_active, "the PATCH failed, but the account was deactivated");
}
