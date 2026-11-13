//! The single-tenant Owner is recorded in two places (security fix 3,
//! 2026-11-05): the account's `users.role`, and the default-org membership
//! that permissions come from. Both transfers move both places together;
//! PATCH never makes an Owner; and an Owner that either place names can't be
//! demoted, deactivated or deleted. Before this, a Members-page transfer left
//! the former Owner able to PATCH their Owner membership back (two Owners),
//! and any Admin could deactivate or delete the real Owner.

mod harness;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use litegen::api::middleware::DEFAULT_ORG_ID;
use litegen::config::Mode;
use litegen::db::sqlite::SqliteDatabase;
use litegen::db::DatabaseStore;
use serde_json::{json, Value};
use tower::ServiceExt;

const MASTER_KEY: &str = "test-master-key";
const PASSWORD: &str = "a-long-test-password-1";

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

/// A signed-in browser session: its cookie, CSRF token and user id.
struct Session {
    cookie: String,
    csrf: String,
    user_id: String,
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
        let csrf = csrf.body["csrf_token"].as_str().unwrap().to_string();
        let me = send(app, "GET", "/v1/auth/me", None, &[("cookie", &cookie)]).await;
        let user_id = me.body["user"]["id"].as_str().expect("user id").to_string();
        Session { cookie, csrf, user_id }
    }

    async fn call(&self, app: &Router, method: &str, path: &str, body: Option<Value>) -> Reply {
        send(app, method, path, body, &[("cookie", &self.cookie), ("x-csrf-token", &self.csrf)]).await
    }
}

/// Single-tenant app with a master key, and its database.
async fn app() -> (Router, Arc<SqliteDatabase>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let app = harness::app_over_with(db.clone(), dir.path(), |c| c.dev.expose_invite_tokens = true, Some(MASTER_KEY), Mode::SingleTenant).await;
    (app, db, dir)
}

async fn owner(app: &Router) -> Session {
    let r = send(app, "POST", "/v1/auth/signup", Some(json!({ "email": "owner@example.com", "password": PASSWORD })), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "owner signup: {:?}", r.body);
    Session::from_reply(app, &r).await
}

async fn invited(app: &Router, owner: &Session, email: &str, role: &str) -> Session {
    let invite = owner
        .call(app, "POST", &format!("/v1/orgs/{DEFAULT_ORG_ID}/members"), Some(json!({ "email": email, "role": role })))
        .await;
    assert_eq!(invite.status, StatusCode::OK, "invite: {:?}", invite.body);
    let token = invite.body["_dev_token"].as_str().expect("dev invite token").to_string();
    let accept = send(app, "POST", &format!("/v1/auth/invitations/{token}/accept"), Some(json!({ "password": PASSWORD })), &[]).await;
    assert_eq!(accept.status, StatusCode::OK, "accept: {:?}", accept.body);
    Session::from_reply(app, &accept).await
}

/// `users.role` of `user_id`, as `GET /v1/users` shows it.
async fn account_role(app: &Router, as_: &Session, user_id: &str) -> String {
    let r = as_.call(app, "GET", "/v1/users", None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let users = r.body.as_array().expect("users");
    let user = users.iter().find(|u| u["id"] == user_id).expect("listed");
    user["role"].as_str().unwrap().to_string()
}

/// The default-org membership roles, as the Members page shows them.
async fn member_roles(app: &Router, as_: &Session) -> Vec<(String, String)> {
    let r = as_.call(app, "GET", &format!("/v1/orgs/{DEFAULT_ORG_ID}/members"), None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body
        .as_array()
        .expect("members")
        .iter()
        .map(|m| (m["user_id"].as_str().unwrap().to_string(), m["role"].as_str().unwrap().to_string()))
        .collect()
}

fn role_of<'a>(roles: &'a [(String, String)], user_id: &str) -> &'a str {
    &roles.iter().find(|(id, _)| id == user_id).expect("a member").1
}

fn owners(roles: &[(String, String)]) -> usize {
    roles.iter().filter(|(_, role)| role == "owner").count()
}

#[tokio::test]
async fn the_members_page_transfer_moves_the_account_role_too() {
    let (app, _db, _dir) = app().await;
    let o = owner(&app).await;
    let a = invited(&app, &o, "admin-a@example.com", "admin").await;

    let r = o.call(&app, "POST", &format!("/v1/orgs/{DEFAULT_ORG_ID}/transfer-owner"), Some(json!({ "new_owner_user_id": a.user_id }))).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);

    let roles = member_roles(&app, &a).await;
    assert_eq!((role_of(&roles, &a.user_id), role_of(&roles, &o.user_id)), ("owner", "admin"));
    assert_eq!(account_role(&app, &a, &a.user_id).await, "owner", "users.role follows the membership");
    assert_eq!(account_role(&app, &a, &o.user_id).await, "admin");
}

#[tokio::test]
async fn the_users_page_transfer_moves_the_default_org_owner_too() {
    let (app, _db, _dir) = app().await;
    let o = owner(&app).await;
    let a = invited(&app, &o, "admin-a@example.com", "admin").await;

    let r = o.call(&app, "POST", "/v1/users/transfer-owner", Some(json!({ "new_owner_id": a.user_id }))).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);

    assert_eq!(account_role(&app, &a, &a.user_id).await, "owner");
    assert_eq!(account_role(&app, &a, &o.user_id).await, "admin");
    let roles = member_roles(&app, &a).await;
    assert_eq!(
        (role_of(&roles, &a.user_id), role_of(&roles, &o.user_id)),
        ("owner", "admin"),
        "the membership, where permissions come from, follows the account"
    );
}

#[tokio::test]
async fn ownership_goes_only_to_a_member_of_the_default_org() {
    let (app, _db, _dir) = app().await;
    let o = owner(&app).await;
    let m = invited(&app, &o, "gone@example.com", "member").await;
    let removed = o.call(&app, "DELETE", &format!("/v1/orgs/{DEFAULT_ORG_ID}/members/{}", m.user_id), None).await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{:?}", removed.body);

    let r = o.call(&app, "POST", "/v1/users/transfer-owner", Some(json!({ "new_owner_id": m.user_id }))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "an account with no membership can't be the Owner: {:?}", r.body);
    assert_eq!(r.body["error"]["code"], "not_a_member", "{:?}", r.body);
    assert_eq!(account_role(&app, &o, &o.user_id).await, "owner", "nothing moved");
    assert_eq!(role_of(&member_roles(&app, &o).await, &o.user_id), "owner");
}

/// Review of security fix 3: both transfer routes refuse an inactive account,
/// which no one could then sign in as.
#[tokio::test]
async fn the_members_page_transfer_refuses_an_inactive_member() {
    let (app, _db, _dir) = app().await;
    let o = owner(&app).await;
    let b = invited(&app, &o, "gone-b@example.com", "admin").await;
    let off = o.call(&app, "PATCH", &format!("/v1/users/{}", b.user_id), Some(json!({ "is_active": false }))).await;
    assert_eq!(off.status, StatusCode::OK, "{:?}", off.body);

    let r = o.call(&app, "POST", &format!("/v1/orgs/{DEFAULT_ORG_ID}/transfer-owner"), Some(json!({ "new_owner_user_id": b.user_id }))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.body);
    assert_eq!(r.body["error"]["code"], "user_inactive", "{:?}", r.body);
    assert_eq!(role_of(&member_roles(&app, &o).await, &o.user_id), "owner", "nothing moved");
}

/// Another org's Owner is just that: its transfer leaves the deployment's
/// Owner (account role and default membership) alone.
#[tokio::test]
async fn another_orgs_transfer_leaves_the_deployment_owner_alone() {
    let (app, db, _dir) = app().await;
    let o = owner(&app).await;
    let a = invited(&app, &o, "admin-a@example.com", "admin").await;
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
    db.create_organization(&side).await.unwrap();
    db.add_org_member(&side.id, &o.user_id, litegen::types::Role::Owner).await.unwrap();
    db.add_org_member(&side.id, &a.user_id, litegen::types::Role::Admin).await.unwrap();

    let r = o.call(&app, "POST", &format!("/v1/orgs/{}/transfer-owner", side.id), Some(json!({ "new_owner_user_id": a.user_id }))).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
    assert_eq!(db.get_membership(&side.id, &a.user_id).await.unwrap(), Some(litegen::types::Role::Owner));
    assert_eq!(account_role(&app, &o, &o.user_id).await, "owner", "still the deployment's Owner");
    assert_eq!(account_role(&app, &o, &a.user_id).await, "admin");
    assert_eq!(role_of(&member_roles(&app, &o).await, &o.user_id), "owner");
}

#[tokio::test]
async fn a_former_owner_cannot_patch_back_to_owner() {
    let (app, db, _dir) = app().await;
    let o = owner(&app).await;
    let a = invited(&app, &o, "admin-a@example.com", "admin").await;
    let b = invited(&app, &o, "admin-b@example.com", "admin").await;
    // What a Members-page transfer left before the fix: the membership moved,
    // the account role didn't.
    db.transfer_org_owner(DEFAULT_ORG_ID, &a.user_id).await.unwrap();

    let r = o.call(&app, "PATCH", &format!("/v1/users/{}", o.user_id), Some(json!({ "role": "owner" }))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "PATCH never makes an Owner: {:?}", r.body);
    assert_eq!(r.body["error"]["code"], "cannot_assign_owner", "{:?}", r.body);
    let r = b.call(&app, "PATCH", &format!("/v1/users/{}", b.user_id), Some(json!({ "role": "owner" }))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.body);

    let roles = member_roles(&app, &a).await;
    assert_eq!(owners(&roles), 1, "exactly one Owner: {roles:?}");
    assert_eq!(role_of(&roles, &a.user_id), "owner");
}

#[tokio::test]
async fn an_admin_cannot_deactivate_or_delete_the_owner_the_membership_names() {
    let (app, db, _dir) = app().await;
    let o = owner(&app).await;
    let a = invited(&app, &o, "admin-a@example.com", "admin").await;
    let b = invited(&app, &o, "admin-b@example.com", "admin").await;
    // A is the Owner by membership, an Admin by account role.
    db.transfer_org_owner(DEFAULT_ORG_ID, &a.user_id).await.unwrap();

    let r = b.call(&app, "PATCH", &format!("/v1/users/{}", a.user_id), Some(json!({ "is_active": false }))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.body);
    assert_eq!(r.body["error"]["code"], "cannot_deactivate_owner", "{:?}", r.body);
    let r = b.call(&app, "PATCH", &format!("/v1/users/{}", a.user_id), Some(json!({ "role": "member" }))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.body);
    assert_eq!(r.body["error"]["code"], "cannot_change_owner_role", "{:?}", r.body);
    let r = b.call(&app, "DELETE", &format!("/v1/users/{}", a.user_id), None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.body);
    assert_eq!(r.body["error"]["code"], "cannot_delete_owner", "{:?}", r.body);

    // A is still there, active, and the Owner.
    let me = a.call(&app, "GET", "/v1/auth/me", None).await;
    assert_eq!(me.status, StatusCode::OK, "A's session still works: {:?}", me.body);
    assert_eq!(role_of(&member_roles(&app, &a).await, &a.user_id), "owner");
}
