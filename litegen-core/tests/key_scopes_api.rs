//! API-key scopes over the real route table: who may grant which scope (on
//! create and on edit), and what each scope opens. A `generate` key is an
//! AI-only key; `read` adds the management reads; `admin` the management writes.
//! And what a session's role opens in each organization, as `GET /v1/auth/me`
//! serves it (section f).

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

async fn get_with_key(app: &Router, key: &str, path: &str) -> Reply {
    send(app, "GET", path, None, &[("authorization", &format!("Bearer {key}"))]).await
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

// ─── (b) what each scope opens ────────────────────────────────────────────────

const AI_READS: [&str; 4] = ["/v1/models", "/v1/providers", "/v1/generations", "/v1/utils/models3d/formats"];
const MANAGEMENT_READS: [&str; 4] = ["/v1/logs", "/v1/stats", "/v1/spend/report?group_by=day", "/v1/limits"];

#[tokio::test]
async fn generate_only_key_reads_ai_routes() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (_, key) = owner.create_key(&app, "generate").await;

    for path in AI_READS {
        let r = get_with_key(&app, &key, path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}: {:?}", r.body);
    }
}

#[tokio::test]
async fn generate_only_key_has_no_management_reads() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (_, key) = owner.create_key(&app, "generate").await;

    for path in MANAGEMENT_READS {
        let r = get_with_key(&app, &key, path).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}: {:?}", r.body);
    }
    for path in ["/v1/logs", "/v1/stats"] {
        assert_eq!(get_with_key(&app, &key, path).await.body["error"]["code"], "forbidden_scope", "{path}");
    }
}

#[tokio::test]
async fn read_only_key_unchanged() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (_, key) = owner.create_key(&app, "read").await;

    for path in AI_READS.iter().chain(MANAGEMENT_READS.iter()) {
        let r = get_with_key(&app, &key, path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}: {:?}", r.body);
    }
    let generate = send(
        &app,
        "POST",
        "/v1/images/generations",
        Some(json!({ "model": "mock/image", "prompt": "x" })),
        &[("authorization", &format!("Bearer {key}"))],
    )
    .await;
    assert_eq!(generate.status, StatusCode::FORBIDDEN, "{:?}", generate.body);
}

// ─── (d) cancelling a job is a change, like deleting it (review 2026-11-11 SEC-CANCEL) ──

/// A `mock/held-video` job made with `key`: it stays `processing` until cancelled.
async fn held_job(app: &Router, key: &str) -> String {
    let r = send(app, "POST", "/v1/videos/generations", Some(json!({ "model": "mock/held-video", "prompt": "a wave" })), &[("authorization", &format!("Bearer {key}"))]).await;
    assert_eq!(r.status, StatusCode::OK, "submit: {:?}", r.body);
    r.body["id"].as_str().expect("job id").to_string()
}

async fn cancel_with(app: &Router, bearer: &str, id: &str) -> Reply {
    send(app, "PATCH", &format!("/v1/generations/{id}"), Some(json!({ "status": "cancelled" })), &[("authorization", &format!("Bearer {bearer}"))]).await
}

async fn status_of(app: &Router, key: &str, id: &str) -> String {
    let r = get_with_key(app, key, &format!("/v1/generations/{id}")).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    r.body["status"].as_str().unwrap_or_default().to_string()
}

#[tokio::test]
async fn a_read_only_key_cannot_cancel_a_job() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (_, maker) = owner.create_key(&app, "generate").await;
    let (_, reader) = owner.create_key(&app, "read").await;
    let id = held_job(&app, &maker).await;
    let r = cancel_with(&app, &reader, &id).await;
    assert_eq!((r.status, r.body["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden_scope")), "{:?}", r.body);
    assert_ne!(status_of(&app, &maker, &id).await, "cancelled", "a read-only key's cancel stopped the job");
}

/// What the SDKs' READMEs tell a read-only key (a Viewer's key keeps only
/// `read`): a job's status route takes `generate`, since a status read can
/// move the job on, so its wait helpers get 403 `forbidden_scope` at the first
/// poll; the job's row (`generations.waitForCompletion`) is a read.
#[tokio::test]
async fn a_read_only_key_reads_a_jobs_row_but_not_its_status_route() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (_, maker) = owner.create_key(&app, "generate").await;
    let (_, reader) = owner.create_key(&app, "read").await;
    let video = held_job(&app, &maker).await;
    let mesh = send(&app, "POST", "/v1/models3d/generations", Some(json!({ "model": "mock/mesh-3d", "prompt": "a cube" })), &[("authorization", &format!("Bearer {maker}"))]).await;
    assert_eq!(mesh.status, StatusCode::OK, "submit: {:?}", mesh.body);
    let mesh = mesh.body["id"].as_str().expect("job id").to_string();
    for path in [format!("/v1/videos/{video}"), format!("/v1/models3d/{mesh}")] {
        let r = get_with_key(&app, &reader, &path).await;
        assert_eq!((r.status, r.body["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden_scope")), "{path}: {:?}", r.body);
    }
    for id in [&video, &mesh] {
        assert!(!status_of(&app, &reader, id).await.is_empty(), "{id}: the row reads with `read`");
    }
}

#[tokio::test]
async fn a_key_cannot_cancel_a_job_the_apps_owner_scope_keeps_from_it() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (_, a) = owner.create_key(&app, "generate").await;
    let (_, b) = owner.create_key(&app, "generate").await;
    // The default owner scope is `key`: B may not change A's job.
    let first = held_job(&app, &a).await;
    let r = cancel_with(&app, &b, &first).await;
    assert_eq!((r.status, r.body["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("not_file_owner")), "{:?}", r.body);
    assert_ne!(status_of(&app, &a, &first).await, "cancelled", "another key's cancel stopped the job");
    let own = cancel_with(&app, &a, &first).await;
    assert_eq!(own.status, StatusCode::OK, "a key cancels its own job: {:?}", own.body);
    let second = held_job(&app, &a).await;
    let master = cancel_with(&app, MASTER_KEY, &second).await;
    assert_eq!(master.status, StatusCode::OK, "the master key cancels any job: {:?}", master.body);
}

// ─── (e) who may point a webhook inside the deployment (review 2026-11-11 SEC-STWEBHOOK) ──

/// Someone the owner invited into the default org with `role`, signed in,
/// and their user id.
async fn invited(app: &Router, owner: &Session, email: &str, role: &str) -> (Session, String) {
    let orgs = owner.call(app, "GET", "/v1/orgs", None).await;
    let org_id = orgs.body[0]["id"].as_str().expect("owner's org").to_string();
    let invite = owner.call(app, "POST", &format!("/v1/orgs/{org_id}/members"), Some(json!({ "email": email, "role": role }))).await;
    assert_eq!(invite.status, StatusCode::OK, "invite: {:?}", invite.body);
    let token = invite.body["_dev_token"].as_str().expect("dev invite token").to_string();
    let accept = send(app, "POST", &format!("/v1/auth/invitations/{token}/accept"), Some(json!({ "password": PASSWORD })), &[]).await;
    assert_eq!(accept.status, StatusCode::OK, "accept: {:?}", accept.body);
    let session = Session::from_reply(app, &accept).await;
    let me = session.call(app, "GET", "/v1/auth/me", None).await;
    let id = me.body["id"].as_str().or_else(|| me.body["user"]["id"].as_str()).expect("user id").to_string();
    (session, id)
}

/// An endpoint on this machine, as a webhook target inside the deployment.
async fn internal_hook() -> wiremock::MockServer {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST")).respond_with(wiremock::ResponseTemplate::new(200)).mount(&server).await;
    server
}

fn refused_as_internal(r: &Reply) -> bool {
    r.status == StatusCode::BAD_REQUEST && r.body["error"]["code"] == "invalid_webhook_url"
}

/// How many requests `server` has had at `at`.
async fn arrivals(server: &wiremock::MockServer, at: &str) -> usize {
    server.received_requests().await.unwrap_or_default().iter().filter(|r| r.url.path() == at).count()
}

/// A `mock/video-gen` job made with `key`, polled until it completes (its
/// first poll ends it): its id. The poll that ends it hands the job's webhook
/// to the process's dispatcher.
async fn finished_video(app: &Router, key: &str) -> String {
    let job = send(app, "POST", "/v1/videos/generations", Some(json!({ "model": "mock/video-gen", "prompt": "a wave" })), &[("authorization", &format!("Bearer {key}"))]).await;
    assert_eq!(job.status, StatusCode::OK, "{:?}", job.body);
    let id = job.body["id"].as_str().unwrap().to_string();
    for _ in 0..20 {
        let polled = get_with_key(app, key, &format!("/v1/videos/{id}")).await;
        if polled.body["status"] == "completed" {
            return id;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("job {id} never completed");
}

#[tokio::test]
async fn a_members_key_webhook_may_not_reach_the_deployments_network() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let member = member(&app, &owner).await;
    let hook = internal_hook().await;
    let body = json!({ "name": "hooked", "scopes": "generate,read", "webhook_url": format!("{}/member-hook", hook.uri()) });
    let r = member.call(&app, "POST", "/v1/keys", Some(body.clone())).await;
    assert!(refused_as_internal(&r), "a member's key may not post to the deployment's own network: {} {:?}", r.status, r.body);
    let r = owner.call(&app, "POST", "/v1/keys", Some(body)).await;
    assert_eq!(r.status, StatusCode::CREATED, "the owner's may: {:?}", r.body);
}

#[tokio::test]
async fn a_demoted_admins_internal_webhook_stops_at_dispatch() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, admin_id) = invited(&app, &owner, "admin@example.com", "admin").await;
    let hook = internal_hook().await;
    let made = admin.call(&app, "POST", "/v1/keys", Some(json!({ "name": "ops", "scopes": "generate,read", "webhook_url": format!("{}/ops-hook", hook.uri()) }))).await;
    assert_eq!(made.status, StatusCode::CREATED, "an admin's key may: {:?}", made.body);
    let (key_id, secret) = (made.body["id"].as_str().unwrap().to_string(), made.body["key"].as_str().unwrap().to_string());
    // The control: the Owner's key, with a webhook to the same receiver.
    let control = owner.call(&app, "POST", "/v1/keys", Some(json!({ "name": "owner-ops", "scopes": "generate,read", "webhook_url": format!("{}/owner-hook", hook.uri()) }))).await;
    assert_eq!(control.status, StatusCode::CREATED, "the owner's key may: {:?}", control.body);
    let control_secret = control.body["key"].as_str().unwrap().to_string();
    let org = owner.call(&app, "GET", "/v1/orgs", None).await.body[0]["id"].as_str().unwrap().to_string();
    let demoted = owner.call(&app, "PATCH", &format!("/v1/orgs/{org}/members/{admin_id}"), Some(json!({ "role": "member" }))).await;
    assert_eq!(demoted.status, StatusCode::OK, "demote: {:?}", demoted.body);

    let r = owner.call(&app, "POST", &format!("/v1/keys/{key_id}/test-webhook"), None).await;
    assert!(refused_as_internal(&r), "a member's key now: {} {:?}", r.status, r.body);
    // A job's own webhook is held to the same rule when it is sent: the
    // member's job ends first, then the Owner's.
    let members_job = finished_video(&app, &secret).await;
    finished_video(&app, &control_secret).await;
    // The Owner's webhook arriving shows that the dispatcher sends to this
    // receiver (30 s at most, done as soon as it arrives).
    let mut controls = 0;
    for _ in 0..300 {
        controls = arrivals(&hook, "/owner-hook").await;
        if controls > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(controls > 0, "the owner's key's webhook never arrived: no delivery was sent at all");
    // Once every delivery the dispatcher holds has ended, the member's has
    // had its turn too.
    let left = litegen::proxy::webhook::deliveries().drain(tokio::time::Instant::now() + std::time::Duration::from_secs(30)).await;
    assert!(!left.contains(&members_job), "the member's delivery is still running after 30 s");
    assert_eq!(arrivals(&hook, "/ops-hook").await, 0, "the deployment's network got a webhook for a member's key");
}

#[tokio::test]
async fn a_side_orgs_key_keeps_no_internal_webhook_once_its_owner_leaves_the_deployments_org() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, admin_id) = invited(&app, &owner, "side-admin@example.com", "admin").await;
    let side = admin.call(&app, "POST", "/v1/orgs", Some(json!({ "name": "Side" }))).await;
    assert!(side.status.is_success(), "a gateway admin makes a side org: {:?}", side.body);
    let side_id = side.body["id"].as_str().expect("side org id").to_string();
    let in_side = |method: &'static str, path: String, body: Option<Value>| {
        let (app, cookie, csrf, side_id) = (app.clone(), admin.cookie.clone(), admin.csrf.clone(), side_id.clone());
        async move { send(&app, method, &path, body, &[("cookie", &cookie), ("x-csrf-token", &csrf), ("x-litegen-org-id", &side_id)]).await }
    };
    let hook = internal_hook().await;
    let url = format!("{}/side-hook", hook.uri());
    let made = in_side("POST", "/v1/keys".into(), Some(json!({ "name": "side", "scopes": "generate,read", "webhook_url": url }))).await;
    assert_eq!(made.status, StatusCode::CREATED, "while a gateway admin: {:?}", made.body);
    let key_id = made.body["id"].as_str().unwrap().to_string();
    let org = owner.call(&app, "GET", "/v1/orgs", None).await.body[0]["id"].as_str().unwrap().to_string();
    let removed = owner.call(&app, "DELETE", &format!("/v1/orgs/{org}/members/{admin_id}"), None).await;
    assert!(removed.status.is_success(), "remove from the deployment's org: {:?}", removed.body);

    // Still the side org's owner, and still signed in through it.
    let saved = in_side("PATCH", format!("/v1/keys/{key_id}"), Some(json!({ "webhook_url": url }))).await;
    assert!(refused_as_internal(&saved), "on save: {} {:?}", saved.status, saved.body);
    let tested = in_side("POST", format!("/v1/keys/{key_id}/test-webhook"), None).await;
    assert!(refused_as_internal(&tested), "at test-webhook: {} {:?}", tested.status, tested.body);
    assert!(hook.received_requests().await.unwrap_or_default().is_empty(), "the deployment's network was reached");
}

/// `webhooks.internal_targets: everyone` is the opt-in back to the old rule:
/// any key's webhook may reach the deployment's network.
#[tokio::test]
async fn with_internal_targets_everyone_a_members_webhook_reaches_inside_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let app = harness::app_over_with(
        db,
        dir.path(),
        |c| {
            c.dev.expose_invite_tokens = true;
            c.webhooks.internal_targets = litegen::config::InternalTargets::Everyone;
        },
        Some(MASTER_KEY),
        Mode::SingleTenant,
    )
    .await;
    let owner = owner(&app).await;
    let member = member(&app, &owner).await;
    let hook = internal_hook().await;
    let made = member.call(&app, "POST", "/v1/keys", Some(json!({ "name": "hooked", "scopes": "generate,read", "webhook_url": format!("{}/member-hook", hook.uri()) }))).await;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    let tested = member.call(&app, "POST", &format!("/v1/keys/{}/test-webhook", made.body["id"].as_str().unwrap()), None).await;
    assert_eq!((tested.status, tested.body["delivered"].as_bool()), (StatusCode::OK, Some(true)), "{:?}", tested.body);
}

// ─── (f) what `GET /v1/auth/me` says a role may do (follow-up FU-ME) ─────────

/// The deployment's own permissions: the gateway's settings, the users table,
/// handing the deployment over, the cache. A role holds them only acting in
/// a single-tenant deployment's own org; in hosted mode they are the master
/// key's alone.
const DEPLOYMENT: [&str; 6] = ["system:config", "user:read:any", "user:write:any", "user:delete:any", "system:transfer_owner", "cache:clear"];

/// The permissions [`probe`] has a request for.
const PROBED: [&str; 18] = [
    "system:config", "user:read:any", "user:write:any", "user:delete:any", "system:transfer_owner", "cache:clear",
    "audit:read", "generation:create", "invitation:send", "key:write:own", "key:write:any",
    "org:write", "org:transfer_owner", "member:invite", "member:write", "member:remove", "app:write", "provider_cred:write",
];

/// A request the check for `perm` refuses with 403 before anything else, and
/// otherwise answers without changing anything a later probe reads: a read,
/// a refusal of the body (an Owner can't be invited or assigned, a blank
/// name), or a 404 for a user or member that doesn't exist. `org` is where
/// it acts (path, or the `x-litegen-org-id` header), `app` an app of it.
fn probe(perm: &str, org: &str, app: &str) -> (&'static str, String, Option<Value>) {
    match perm {
        "system:config" => ("GET", "/v1/admin/settings".into(), None),
        "user:read:any" => ("GET", "/v1/users".into(), None),
        "user:write:any" => ("PATCH", "/v1/users/nobody".into(), Some(json!({ "role": "viewer" }))),
        "user:delete:any" => ("DELETE", "/v1/users/nobody".into(), None),
        "system:transfer_owner" => ("POST", "/v1/users/transfer-owner".into(), Some(json!({ "new_owner_id": "nobody" }))),
        "cache:clear" => ("DELETE", "/v1/cache".into(), None),
        "audit:read" => ("GET", "/v1/audit".into(), None),
        "generation:create" => ("POST", "/v1/images/cost".into(), Some(json!({ "model": "mock/image-gen", "prompt": "a fox" }))),
        "invitation:send" => ("POST", "/v1/users".into(), Some(json!({ "email": "probe@example.com", "role": "owner" }))),
        "key:write:own" => ("POST", "/v1/keys".into(), Some(json!({ "name": "probe", "scopes": "read" }))),
        "key:write:any" => ("POST", "/v1/keys".into(), Some(json!({ "name": "probe", "scopes": "admin" }))),
        "org:write" => ("PATCH", format!("/v1/orgs/{org}"), Some(json!({ "name": " " }))),
        "org:transfer_owner" => ("POST", format!("/v1/orgs/{org}/transfer-owner"), Some(json!({ "new_owner_user_id": "nobody" }))),
        "member:invite" => ("POST", format!("/v1/orgs/{org}/members"), Some(json!({ "email": "probe@example.com", "role": "owner" }))),
        "member:write" => ("PATCH", format!("/v1/orgs/{org}/members/nobody"), Some(json!({ "role": "owner" }))),
        "member:remove" => ("DELETE", format!("/v1/orgs/{org}/members/nobody"), None),
        "app:write" => ("PATCH", format!("/v1/apps/{app}"), Some(json!({ "name": " " }))),
        "provider_cred:write" => ("POST", format!("/v1/orgs/{org}/provider-credentials"), Some(json!({ "provider": " ", "credentials": {} }))),
        other => panic!("no probe for {other}"),
    }
}

/// What `GET /v1/auth/me` serves `session` for `org` is what the server
/// grants there: exactly the role's permissions (`permissions_for`, what a
/// session acting there carries), less the deployment's own unless
/// `deployment`; and every probed route lets the session through exactly
/// when the list has its permission. `who` names the case in a failure.
async fn assert_served_as_checked(app: &Router, who: &str, session: &Session, org: &str, role: litegen::types::Role, deployment: bool) {
    use litegen::auth::permissions::permissions_for;
    let me = session.call(app, "GET", "/v1/auth/me", None).await;
    assert_eq!(me.status, StatusCode::OK, "{who}: {:?}", me.body);
    let entry = me.body["orgs"]
        .as_array()
        .and_then(|orgs| orgs.iter().find(|o| o["id"] == org))
        .unwrap_or_else(|| panic!("{who}: {org} is not in orgs: {:?}", me.body))
        .clone();
    assert_eq!(entry["role"], role.as_str(), "{who}: {entry}");
    let served: std::collections::BTreeSet<String> = entry["permissions"]
        .as_array()
        .unwrap_or_else(|| panic!("{who}: no permissions served for {org}: {entry}"))
        .iter()
        .map(|p| p.as_str().expect("a permission name").to_string())
        .collect();
    let granted: std::collections::BTreeSet<String> = permissions_for(role)
        .iter()
        .map(|p| p.as_str().to_string())
        .filter(|p| deployment || !DEPLOYMENT.contains(&p.as_str()))
        .collect();
    assert_eq!(served, granted, "{who}: served in {org}");
    let apps = send(app, "GET", &format!("/v1/orgs/{org}/apps"), None, &[("cookie", &session.cookie)]).await;
    let app_id = apps.body[0]["id"].as_str().unwrap_or_else(|| panic!("{who}: an app of {org}: {:?}", apps.body)).to_string();
    for perm in PROBED {
        let (method, path, body) = probe(perm, org, &app_id);
        let r = send(app, method, &path, body, &[("cookie", &session.cookie), ("x-csrf-token", &session.csrf), ("x-litegen-org-id", org)]).await;
        let let_through = r.status != StatusCode::FORBIDDEN;
        assert_eq!(
            let_through,
            served.contains(perm),
            "{who} in {org}: {perm} {} served, and {method} {path} answered {} {:?}",
            if served.contains(perm) { "is" } else { "is not" },
            r.status,
            r.body
        );
    }
}

/// FU-ME: `GET /v1/auth/me` serves, for each org, the permissions its role
/// holds there as the server checks them, so a client offers what will work
/// without a copy of the role map. In a single-tenant deployment the
/// deployment's own permissions (the gateway, the users table, the cache)
/// come only with a role in its own org: an Owner of a side org holds none
/// of them there.
#[tokio::test]
async fn me_serves_each_roles_permissions_as_the_server_checks_them() {
    use litegen::types::Role;
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (admin, _) = invited(&app, &owner, "perm-admin@example.com", "admin").await;
    let (member, _) = invited(&app, &owner, "perm-member@example.com", "member").await;
    let (viewer, _) = invited(&app, &owner, "perm-viewer@example.com", "viewer").await;
    let org = litegen::api::middleware::DEFAULT_ORG_ID;
    for (who, session, role) in [("owner", &owner, Role::Owner), ("admin", &admin, Role::Admin), ("member", &member, Role::Member), ("viewer", &viewer, Role::Viewer)] {
        assert_served_as_checked(&app, who, session, org, role, true).await;
    }
    let side = owner.call(&app, "POST", "/v1/orgs", Some(json!({ "name": "Side" }))).await;
    assert!(side.status.is_success(), "the deployment's owner makes a side org: {:?}", side.body);
    let side_id = side.body["id"].as_str().expect("side org id").to_string();
    assert_served_as_checked(&app, "the owner of a side org", &owner, &side_id, Role::Owner, false).await;
}

/// FU-ME, hosted: every org is a tenant's, so no role holds the deployment's
/// own permissions anywhere; the rest are the role's, as in single-tenant.
#[tokio::test]
async fn me_serves_no_deployment_permission_to_a_hosted_role() {
    use litegen::types::Role;
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let app = harness::app_over_with(db, dir.path(), |c| c.dev.expose_invite_tokens = true, Some(MASTER_KEY), Mode::Hosted).await;
    let owner = owner(&app).await;
    let org = owner.call(&app, "GET", "/v1/orgs", None).await.body[0]["id"].as_str().expect("the owner's org").to_string();
    let (admin, _) = invited(&app, &owner, "hosted-admin@example.com", "admin").await;
    let (member, _) = invited(&app, &owner, "hosted-member@example.com", "member").await;
    let (viewer, _) = invited(&app, &owner, "hosted-viewer@example.com", "viewer").await;
    for (who, session, role) in [("owner", &owner, Role::Owner), ("admin", &admin, Role::Admin), ("member", &member, Role::Member), ("viewer", &viewer, Role::Viewer)] {
        assert_served_as_checked(&app, who, session, &org, role, false).await;
    }
}

// ─── (g) a key's PATCH is a JSON merge patch ──────────────────────────────────

/// The key `id` as the API shows it.
async fn key_of(app: &Router, owner: &Session, id: &str) -> Value {
    let r = owner.call(app, "GET", &format!("/v1/keys/{id}"), None).await;
    assert_eq!(r.status, StatusCode::OK, "get key: {:?}", r.body);
    r.body
}

/// `PATCH /v1/keys/{id}` with `body`, which must be answered 200: the key
/// as the answer shows it.
async fn patched(app: &Router, owner: &Session, id: &str, body: Value) -> Value {
    let r = owner.call(app, "PATCH", &format!("/v1/keys/{id}"), Some(body.clone())).await;
    assert_eq!(r.status, StatusCode::OK, "PATCH {body}: {:?}", r.body);
    r.body
}

/// `null` clears a key's budget, its rate limit, its webhook and its expiry
/// (the dashboard's Keys page clears them that way; before, `null` read as
/// "unchanged", so nothing could clear a quota or a limit), and a field left
/// out stays as it is.
#[tokio::test]
async fn null_clears_a_keys_quota_limit_webhook_and_expiry_and_absent_keeps_them() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (id, _) = owner.create_key(&app, "generate,read").await;
    let set = json!({
        "token_quota": 5.0,
        "rpm_limit": 30,
        "webhook_url": "https://8.8.8.8/hook",
        "expires_at": "2099-01-01T00:00:00Z",
    });
    let all_set = patched(&app, &owner, &id, set).await;
    assert_eq!(
        (&all_set["token_quota"], &all_set["rpm_limit"], &all_set["webhook_url"], all_set["expires_at"].is_string()),
        (&json!(5.0), &json!(30), &json!("https://8.8.8.8/hook"), true),
        "{all_set}"
    );

    let renamed = patched(&app, &owner, &id, json!({ "name": "renamed" })).await;
    assert_eq!((&renamed["token_quota"], &renamed["rpm_limit"], &renamed["webhook_url"]), (&json!(5.0), &json!(30), &json!("https://8.8.8.8/hook")), "absent fields stay: {renamed}");
    assert!(renamed["expires_at"].is_string(), "absent fields stay: {renamed}");

    patched(&app, &owner, &id, json!({ "token_quota": null, "rpm_limit": null, "webhook_url": null, "expires_at": null })).await;
    let cleared = key_of(&app, &owner, &id).await;
    assert_eq!((&cleared["token_quota"], &cleared["rpm_limit"], &cleared["webhook_url"]), (&Value::Null, &Value::Null, &Value::Null), "{cleared}");
    assert!(cleared.get("expires_at").is_none_or(Value::is_null), "the expiry is removed: {cleared}");
    assert_eq!(cleared["name"], "renamed");
}

/// A rate limit of 0 is refused, as the limits API refuses it: a key with
/// `rpm_limit: 0` answered every request 429 forever, and many APIs read 0 as
/// "unlimited". To stop a key, block it. Creating and patching both answer
/// `400 invalid_limits` and change nothing.
#[tokio::test]
async fn a_zero_rate_limit_is_refused_on_create_and_patch() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let created = owner.call(&app, "POST", "/v1/keys", Some(json!({ "name": "zero", "rpm_limit": 0 }))).await;
    assert_eq!((created.status, created.body["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_limits")), "{:?}", created.body);
    let (id, _) = owner.create_key(&app, "generate,read").await;
    let r = owner.call(&app, "PATCH", &format!("/v1/keys/{id}"), Some(json!({ "rpm_limit": 0 }))).await;
    assert_eq!((r.status, r.body["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_limits")), "{:?}", r.body);
    assert_eq!(key_of(&app, &owner, &id).await["rpm_limit"], Value::Null, "nothing changed");
}

/// `""` still clears a webhook, as it always has, and is stored as no
/// webhook (`null`), on create and on edit: the outbox's trigger took a
/// stored `""` for a webhook (review 2026-11-15 round 2, finding 4).
#[tokio::test]
async fn an_empty_webhook_url_is_stored_as_none() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let made = owner.call(&app, "POST", "/v1/keys", Some(json!({ "name": "blank", "webhook_url": "" }))).await;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    let blank = made.body["id"].as_str().unwrap().to_string();
    assert_eq!(key_of(&app, &owner, &blank).await["webhook_url"], Value::Null, "created with \"\": none");

    let (id, _) = owner.create_key(&app, "generate,read").await;
    patched(&app, &owner, &id, json!({ "webhook_url": "https://8.8.8.8/hook" })).await;
    let cleared = patched(&app, &owner, &id, json!({ "webhook_url": "" })).await;
    assert_eq!(cleared["webhook_url"], Value::Null, "the answer shows none: {cleared}");
    assert_eq!(key_of(&app, &owner, &id).await["webhook_url"], Value::Null, "and none is stored");
}

/// `name`, `scopes` and `is_active` can't be cleared: `null` leaves them as
/// they are.
#[tokio::test]
async fn null_leaves_a_keys_name_scopes_and_state_as_they_are() {
    let (app, _dir) = app().await;
    let owner = owner(&app).await;
    let (id, _) = owner.create_key(&app, "generate,read").await;
    let before = key_of(&app, &owner, &id).await;
    let after = patched(&app, &owner, &id, json!({ "name": null, "scopes": null, "is_active": null })).await;
    for field in ["name", "scopes", "is_active"] {
        assert_eq!(after[field], before[field], "{field}: {after}");
    }
}

// ─── (h) sending a webhook delivery again ─────────────────────────────────────

/// The single-tenant app over `db`, which a test reads and writes beside it.
async fn app_over_db(db: &Arc<SqliteDatabase>, dir: &std::path::Path) -> Router {
    harness::app_over_with(db.clone(), dir, |c| c.dev.expose_invite_tokens = true, Some(MASTER_KEY), Mode::SingleTenant).await
}

/// A job of key `key_id` whose webhook delivery has ended as `outcome` (no
/// dispatcher runs here: the test plays the one that sent it): the
/// delivery's id.
async fn ended_delivery(db: &SqliteDatabase, key_id: &str, outcome: litegen::db::OutboxOutcome) -> String {
    use litegen::api::middleware::{DEFAULT_APP_ID, DEFAULT_ORG_ID};
    use litegen::db::DatabaseStore;
    let generation = format!("g-{}", uuid::Uuid::new_v4().simple());
    let key = uuid::Uuid::parse_str(key_id).unwrap();
    db.insert_generation(&generation, Some(&key), "mock/video-gen", "mock", "video", Some("job"), 0.0, Some(DEFAULT_ORG_ID), Some(DEFAULT_APP_ID)).await.unwrap();
    db.update_generation_if_active(&generation, "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
    let claimed = db.claim_webhook_delivery_of(&generation, "test-sender", chrono::Duration::seconds(60)).await.unwrap().expect("its terminal write recorded it");
    let attempt = litegen::db::OutboxAttempt { attempts: 1, outcome, status_code: Some(500), error: None, payload_json: None };
    assert!(db.finish_webhook_attempt(&claimed.id, "test-sender", &attempt).await.unwrap());
    claimed.id
}

fn code(r: &Reply) -> &str {
    r.body["error"]["code"].as_str().unwrap_or("")
}

/// A delivery that ended is sent again when asked: 202 and its state (due,
/// a fresh series), then a replica's dispatcher sends it under the same
/// delivery id. While it is being sent a second ask is a 409, and within a
/// minute of the last redeliver a 429 with `Retry-After`.
#[tokio::test]
async fn an_ended_delivery_is_sent_again_under_its_id_once_asked() {
    use litegen::db::DatabaseStore;
    use litegen::proxy::webhook::{Outbox, SendContext, WebhookDeliveries, WEBHOOK_RETRY_DELAYS};
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let app = app_over_db(&db, dir.path()).await;
    let owner = owner(&app).await;
    let hook = internal_hook().await;
    let made = owner.call(&app, "POST", "/v1/keys", Some(json!({ "name": "hooked", "webhook_url": format!("{}/hook", hook.uri()) }))).await;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    let key_id = made.body["id"].as_str().unwrap().to_string();
    let delivery = ended_delivery(&db, &key_id, litegen::db::OutboxOutcome::Failed).await;
    let path = format!("/v1/keys/{key_id}/webhook-deliveries/{delivery}/redeliver");

    let r = owner.call(&app, "POST", &path, None).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.body);
    assert_eq!((r.body["delivery_id"].as_str(), r.body["status"].as_str(), &r.body["attempts"]), (Some(delivery.as_str()), Some("pending"), &json!(0)), "{:?}", r.body);
    assert!(r.body["redelivered_at"].is_string() && r.body["finished_at"].is_null() && r.body["last_status_code"].is_null(), "{:?}", r.body);
    let busy = owner.call(&app, "POST", &path, None).await;
    assert_eq!((busy.status, code(&busy)), (StatusCode::CONFLICT, "delivery_pending"), "{:?}", busy.body);

    // A replica's dispatcher sends it, under its id.
    let slots = Arc::new(WebhookDeliveries::new(8, 4));
    let outbox = Arc::new(Outbox::new("replica-test", slots.clone(), WEBHOOK_RETRY_DELAYS.to_vec()));
    let store: Arc<dyn DatabaseStore> = db.clone();
    let ctx = SendContext::new(Mode::SingleTenant, None, litegen::proxy::webhook::webhook_client(Mode::SingleTenant));
    assert_eq!(outbox.tick(&store, &ctx).await, 1, "due at once");
    assert!(slots.drain(tokio::time::Instant::now() + std::time::Duration::from_secs(10)).await.is_empty());
    let got = hook.received_requests().await.unwrap_or_default();
    assert_eq!(got.len(), 1, "sent once");
    let sent_as = got[0].headers.get("x-litegen-delivery-id").and_then(|v| v.to_str().ok()).unwrap_or_default();
    assert_eq!(sent_as, delivery, "under its delivery id");

    let soon = owner.call(&app, "POST", &path, None).await;
    assert_eq!((soon.status, code(&soon)), (StatusCode::TOO_MANY_REQUESTS, "redeliver_too_soon"), "{:?}", soon.body);
    let retry: u64 = soon.headers.get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).expect("Retry-After");
    assert!((1..=60).contains(&retry), "{retry}");
}

/// Who may send a key's delivery again: whoever may read the key's
/// deliveries and send its webhook. An Owner (any key of the org), a
/// Member (their own key), an admin-scope key and the master key may; a
/// Member for another's key, a Viewer (who may read their own key's
/// deliveries, not send them) and a key without `admin` may not. An unknown
/// delivery, or another key's, is not found; a key with no webhook now is
/// a 400.
#[tokio::test]
async fn who_may_send_a_keys_delivery_again() {
    use litegen::db::{DatabaseStore, OutboxOutcome};
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let app = app_over_db(&db, dir.path()).await;
    let owner = owner(&app).await;
    let (member, _) = invited(&app, &owner, "redeliver-member@example.com", "member").await;
    let (viewer, viewer_id) = invited(&app, &owner, "redeliver-viewer@example.com", "viewer").await;
    let hooked = |name: &str| json!({ "name": name, "scopes": "generate,read", "webhook_url": "https://8.8.8.8/hook" });
    let id_of = |r: Reply| {
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        r.body["id"].as_str().unwrap().to_string()
    };
    let owners = id_of(owner.call(&app, "POST", "/v1/keys", Some(hooked("owners"))).await);
    let members = id_of(member.call(&app, "POST", "/v1/keys", Some(hooked("members"))).await);
    // A Viewer makes no keys: one made with the master key is given to them.
    let viewers = id_of(send(&app, "POST", "/v1/keys", Some(hooked("viewers")), &[("authorization", &format!("Bearer {MASTER_KEY}"))]).await);
    db.set_api_key_owner(&uuid::Uuid::parse_str(&viewers).unwrap(), &viewer_id).await.unwrap();
    let (_, generate_only) = owner.create_key(&app, "generate,read").await;
    let path = |key: &str, delivery: &str| format!("/v1/keys/{key}/webhook-deliveries/{delivery}/redeliver");
    let ended = |key: String| {
        let db = db.clone();
        async move { ended_delivery(&db, &key, OutboxOutcome::Delivered).await }
    };

    let r = member.call(&app, "POST", &path(&members, &ended(members.clone()).await), None).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "a member, their own key's: {:?}", r.body);
    let r = owner.call(&app, "POST", &path(&members, &ended(members.clone()).await), None).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "an owner, any key of the org: {:?}", r.body);
    let bearer = |key: &str| format!("Bearer {key}");
    let r = send(&app, "POST", &path(&owners, &ended(owners.clone()).await), None, &[("authorization", &bearer(MASTER_KEY))]).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "the master key: {:?}", r.body);

    let r = member.call(&app, "POST", &path(&owners, &ended(owners.clone()).await), None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "a member, another's key: {:?}", r.body);
    let r = viewer.call(&app, "POST", &path(&viewers, &ended(viewers.clone()).await), None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "a viewer, their own key's: {:?}", r.body);
    let r = send(&app, "POST", &path(&owners, &ended(owners.clone()).await), None, &[("authorization", &bearer(&generate_only))]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "a key without admin: {:?}", r.body);

    let r = owner.call(&app, "POST", &path(&owners, "whd-00000000-0000-4000-8000-000000000000"), None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "an unknown delivery: {:?}", r.body);
    let r = owner.call(&app, "POST", &path(&owners, &ended(members.clone()).await), None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "another key's delivery under this key: {:?}", r.body);

    let delivery = ended(owners.clone()).await;
    let cleared = owner.call(&app, "PATCH", &format!("/v1/keys/{owners}"), Some(json!({ "webhook_url": null }))).await;
    assert_eq!(cleared.status, StatusCode::OK, "{:?}", cleared.body);
    let r = owner.call(&app, "POST", &path(&owners, &delivery), None).await;
    assert_eq!((r.status, code(&r)), (StatusCode::BAD_REQUEST, "no_webhook_configured"), "{:?}", r.body);
}

/// A job's webhook refused when it is sent still shows in the key's
/// delivery list, with its delivery id and why, so its managers can see why
/// it never came; once the cause is fixed, the redeliver route sends it. Here
/// the key is a demoted Admin's whose webhook points inside the deployment,
/// and the fix is making them an Admin again.
#[tokio::test]
async fn a_refused_webhook_is_listed_and_redelivered_once_its_cause_is_fixed() {
    use litegen::db::DatabaseStore;
    use litegen::proxy::webhook::{Outbox, SendContext, WebhookDeliveries, WEBHOOK_RETRY_DELAYS};
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let app = app_over_db(&db, dir.path()).await;
    let owner = owner(&app).await;
    let (admin, admin_id) = invited(&app, &owner, "refused-admin@example.com", "admin").await;
    let hook = internal_hook().await;
    let made = admin.call(&app, "POST", "/v1/keys", Some(json!({ "name": "ops", "scopes": "generate,read", "webhook_url": format!("{}/ops-hook", hook.uri()) }))).await;
    assert_eq!(made.status, StatusCode::CREATED, "an admin's key may: {:?}", made.body);
    let (key_id, secret) = (made.body["id"].as_str().unwrap().to_string(), made.body["key"].as_str().unwrap().to_string());
    let org = owner.call(&app, "GET", "/v1/orgs", None).await.body[0]["id"].as_str().unwrap().to_string();
    let role = |role: &'static str| json!({ "role": role });
    let demoted = owner.call(&app, "PATCH", &format!("/v1/orgs/{org}/members/{admin_id}"), Some(role("member"))).await;
    assert_eq!(demoted.status, StatusCode::OK, "demote: {:?}", demoted.body);

    // The poll that ends the job sends its first attempt, which is refused.
    let job = finished_video(&app, &secret).await;
    let list = format!("/v1/keys/{key_id}/webhook-deliveries");
    let mut listed = Value::Null;
    for _ in 0..200 {
        let r = owner.call(&app, "GET", &list, None).await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        if r.body["data"].as_array().is_some_and(|rows| !rows.is_empty()) {
            listed = r.body["data"][0].clone();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(listed["generation_id"].as_str(), Some(job.as_str()), "the refused delivery is in the key's list: {listed}");
    assert!(listed["status_code"].is_null() && listed["success"] == false, "{listed}");
    assert!(listed["error_message"].as_str().is_some_and(|e| e.contains("not allowed")), "it says why: {listed}");
    let delivery = listed["delivery_id"].as_str().expect("its delivery id").to_string();
    assert_eq!(arrivals(&hook, "/ops-hook").await, 0, "nothing reached the deployment's network");

    // Fixed: an Admin again. The redeliver route makes it due, and a replica sends it.
    let promoted = owner.call(&app, "PATCH", &format!("/v1/orgs/{org}/members/{admin_id}"), Some(role("admin"))).await;
    assert_eq!(promoted.status, StatusCode::OK, "promote: {:?}", promoted.body);
    let r = owner.call(&app, "POST", &format!("{list}/{delivery}/redeliver"), None).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.body);
    let slots = Arc::new(WebhookDeliveries::new(8, 4));
    let outbox = Arc::new(Outbox::new("replica-test", slots.clone(), WEBHOOK_RETRY_DELAYS.to_vec()));
    let store: Arc<dyn DatabaseStore> = db.clone();
    let ctx = SendContext::new(Mode::SingleTenant, None, litegen::proxy::webhook::webhook_client(Mode::SingleTenant));
    assert_eq!(outbox.tick(&store, &ctx).await, 1, "due at once");
    assert!(slots.drain(tokio::time::Instant::now() + std::time::Duration::from_secs(10)).await.is_empty());
    let got = hook.received_requests().await.unwrap_or_default();
    assert_eq!(got.len(), 1, "sent once its cause is fixed");
    let sent_as = got[0].headers.get("x-litegen-delivery-id").and_then(|v| v.to_str().ok()).unwrap_or_default();
    assert_eq!(sent_as, delivery, "under its delivery id");
}
