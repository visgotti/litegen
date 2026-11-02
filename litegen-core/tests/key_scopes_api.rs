//! API-key scopes over the real route table: who may grant which scope, on
//! create and on edit.

mod harness;

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

fn session_cookie(headers: &HeaderMap) -> String {
    headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|s| s.split(';').next())
        .find(|kv| kv.starts_with("litegen_session="))
        .expect("litegen_session cookie")
        .to_string()
}

impl Session {
    async fn from_reply(app: &Router, reply: &Reply) -> Session {
        let cookie = session_cookie(&reply.headers);
        let csrf = send(app, "GET", "/v1/auth/csrf", None, &[("cookie", &cookie)]).await;
        assert_eq!(csrf.status, StatusCode::OK, "csrf: {:?}", csrf.body);
        Session { cookie, csrf: csrf.body["csrf_token"].as_str().unwrap().to_string() }
    }

    async fn call(&self, app: &Router, method: &str, path: &str, body: Option<Value>) -> Reply {
        send(app, method, path, body, &[("cookie", &self.cookie), ("x-csrf-token", &self.csrf)]).await
    }

    /// Create a key through the session; returns `(id, secret)`.
    async fn create_key(&self, app: &Router, scopes: &str) -> (String, String) {
        let r = self.call(app, "POST", "/v1/keys", Some(json!({ "name": format!("k-{scopes}"), "scopes": scopes }))).await;
        assert_eq!(r.status, StatusCode::CREATED, "create key {scopes}: {:?}", r.body);
        (r.body["id"].as_str().unwrap().to_string(), r.body["key"].as_str().unwrap().to_string())
    }
}

/// Single-tenant app with a master key: the first signup is the owner, and an
/// invited member joins the default org.
async fn app() -> (Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let app = harness::app_over_with(
        db,
        dir.path(),
        |c| c.dev.expose_invite_tokens = true,
        Some(MASTER_KEY),
        Mode::SingleTenant,
    )
    .await;
    (app, dir)
}

async fn owner(app: &Router) -> Session {
    let r = send(app, "POST", "/v1/auth/signup", Some(json!({ "email": "owner@example.com", "password": PASSWORD })), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "owner signup: {:?}", r.body);
    Session::from_reply(app, &r).await
}

async fn member(app: &Router, owner: &Session) -> Session {
    let orgs = owner.call(app, "GET", "/v1/orgs", None).await;
    let org_id = orgs.body[0]["id"].as_str().expect("owner's org").to_string();
    let invite = owner
        .call(app, "POST", &format!("/v1/orgs/{org_id}/members"), Some(json!({ "email": "member@example.com", "role": "member" })))
        .await;
    assert_eq!(invite.status, StatusCode::OK, "invite: {:?}", invite.body);
    let token = invite.body["_dev_token"].as_str().expect("dev invite token").to_string();
    let accept = send(app, "POST", &format!("/v1/auth/invitations/{token}/accept"), Some(json!({ "password": PASSWORD })), &[]).await;
    assert_eq!(accept.status, StatusCode::OK, "accept: {:?}", accept.body);
    Session::from_reply(app, &accept).await
}

fn message(r: &Reply) -> &str {
    r.body["error"]["message"].as_str().unwrap_or("")
}

// ─── (a) edits are privilege-checked like creation ────────────────────────────

#[tokio::test]
async fn member_cannot_edit_own_key_to_admin() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let member = member(&app, &owner).await;
    let (id, _) = member.create_key(&app, "generate,read").await;

    let r = member.call(&app, "PATCH", &format!("/v1/keys/{id}"), Some(json!({ "scopes": "admin" }))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{:?}", r.body);
    assert_eq!(message(&r), "insufficient privilege to grant scope 'admin'");

    let after = member.call(&app, "GET", &format!("/v1/keys/{id}"), None).await;
    assert_eq!(after.status, StatusCode::OK, "{:?}", after.body);
    assert_eq!(after.body["scopes"], "generate,read");
}

#[tokio::test]
async fn edit_rejects_unknown_scope() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (id, _) = owner.create_key(&app, "generate").await;

    let r = owner.call(&app, "PATCH", &format!("/v1/keys/{id}"), Some(json!({ "scopes": "generate,admn" }))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.body);
    assert_eq!(message(&r), "unknown scope 'admn'");
}

#[tokio::test]
async fn owner_can_edit_to_admin() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (id, _) = owner.create_key(&app, "generate").await;

    let r = owner.call(&app, "PATCH", &format!("/v1/keys/{id}"), Some(json!({ "scopes": "admin" }))).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["scopes"], "admin");
}
