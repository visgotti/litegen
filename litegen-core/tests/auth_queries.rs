//! How many database reads authenticating a request costs (review 2026-11-11,
//! P14). A key and its maker's standing, a session and its user, and a stored
//! file's generation and access settings are each one statement, so a busy
//! gateway's auth costs one round trip, not three.
//!
//! Its own test binary, because it counts statements through a process-wide
//! tracing subscriber (sqlx's SQLite driver logs `sqlx::query` events from
//! its own worker thread, which a per-thread subscriber never reaches). The
//! tests take `ONE_AT_A_TIME` first, so no two count at once.

mod harness;

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use litegen::config::Mode;
use litegen::db::sqlite::SqliteDatabase;
use serde_json::{json, Value};
use tower::ServiceExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

const MASTER_KEY: &str = "test-master-key";
const PASSWORD: &str = "a-long-test-password-1";

/// The tests count one at a time. A tokio lock: the guard is held across awaits.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The `summary` of every `sqlx::query` event since the last [`reset`].
static STATEMENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Records each `sqlx::query` event's `summary` field.
struct Statements;

struct Summary(Option<String>);

impl tracing::field::Visit for Summary {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "summary" {
            self.0 = Some(value.to_string());
        }
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "summary" && self.0.is_none() {
            self.0 = Some(format!("{value:?}").trim_matches('"').to_string());
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for Statements {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        let mut summary = Summary(None);
        event.record(&mut summary);
        if let Some(s) = summary.0 {
            STATEMENTS.lock().unwrap_or_else(|p| p.into_inner()).push(s);
        }
    }
}

fn install() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let layer = Statements.with_filter(tracing_subscriber::EnvFilter::new("sqlx::query=trace"));
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer));
    });
}

fn selects() -> Vec<String> {
    STATEMENTS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|s| s.trim_start().to_ascii_uppercase().starts_with("SELECT"))
        .cloned()
        .collect()
}

fn reset() {
    STATEMENTS.lock().unwrap_or_else(|p| p.into_inner()).clear();
}

/// Wait until the database has been quiet for 100 ms (work a request left in
/// the background, such as an audit insert, has landed), then start counting.
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    for _ in 0..50 {
        reset();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if STATEMENTS.lock().unwrap_or_else(|p| p.into_inner()).is_empty() {
            return;
        }
    }
    panic!("the database never went quiet: {:?}", STATEMENTS.lock().unwrap_or_else(|p| p.into_inner()));
}

/// The SELECTs that `req` ran, once its answer is in and its reads logged.
async fn selects_of(app: &Router, req: Request<Body>) -> (StatusCode, Vec<String>) {
    settle().await;
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let _ = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    (status, selects())
}

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
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = axum::body::to_bytes(resp.into_body(), 10 * 1024 * 1024).await.unwrap();
    Reply { status, headers, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null) }
}

/// A single-tenant app behind the master key, and its first user (the owner):
/// their session cookie and CSRF token.
async fn app_and_owner() -> (Router, tempfile::TempDir, String, String) {
    install();
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let app = harness::app_over_with(db, dir.path(), |_| {}, Some(MASTER_KEY), Mode::SingleTenant).await;
    let r = send(&app, "POST", "/v1/auth/signup", Some(json!({ "email": "owner@example.com", "password": PASSWORD })), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "signup: {:?}", r.body);
    let cookie = r
        .headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|s| s.split(';').next())
        .find(|kv| kv.starts_with("litegen_session="))
        .expect("litegen_session cookie")
        .to_string();
    let csrf = send(&app, "GET", "/v1/auth/csrf", None, &[("cookie", &cookie)]).await;
    assert_eq!(csrf.status, StatusCode::OK, "csrf: {:?}", csrf.body);
    let csrf = csrf.body["csrf_token"].as_str().unwrap().to_string();
    (app, dir, cookie, csrf)
}

#[tokio::test]
async fn a_key_its_owner_made_is_checked_in_one_statement() {
    let _one = ONE_AT_A_TIME.lock().await;
    let (app, _dir, cookie, csrf) = app_and_owner().await;
    let made = send(&app, "POST", "/v1/keys", Some(json!({ "name": "a key", "scopes": "generate,read" })), &[("cookie", &cookie), ("x-csrf-token", &csrf)]).await;
    assert_eq!(made.status, StatusCode::CREATED, "create key: {:?}", made.body);
    let key = format!("Bearer {}", made.body["key"].as_str().unwrap());

    let req = Request::get("/v1/utils/models3d/formats").header("authorization", &key).body(Body::empty()).unwrap();
    let (status, selects) = selects_of(&app, req).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(selects.len(), 1, "authenticating a key its owner made ran {} statements ({selects:?}); want 1", selects.len());
}

#[tokio::test]
async fn a_session_and_its_user_are_read_in_one_statement() {
    let _one = ONE_AT_A_TIME.lock().await;
    let (app, _dir, cookie, _csrf) = app_and_owner().await;

    let req = Request::get("/v1/utils/models3d/formats").header("cookie", &cookie).body(Body::empty()).unwrap();
    let (status, selects) = selects_of(&app, req).await;
    assert_eq!(status, StatusCode::OK);
    // The session and its user in one, then the org it acts in.
    assert!(selects.len() <= 2, "a session request ran {} statements ({selects:?}); want at most 2", selects.len());
}

#[tokio::test]
async fn an_anonymous_file_request_reads_its_generation_and_access_in_one_statement() {
    let _one = ONE_AT_A_TIME.lock().await;
    let (app, _dir, _cookie, _csrf) = app_and_owner().await;
    // Made with the master key: no webhook task reads the database after.
    let master = format!("Bearer {MASTER_KEY}");
    let submitted = send(&app, "POST", "/v1/models3d/generations", Some(json!({ "model": "mock/mesh-3d", "prompt": "a chair" })), &[("authorization", &master)]).await;
    let id = submitted.body["id"].as_str().expect("submitted").to_string();
    let mut done = Value::Null;
    for _ in 0..10 {
        done = send(&app, "GET", &format!("/v1/models3d/{id}"), None, &[("authorization", &master)]).await.body;
        if done["status"] == "completed" {
            break;
        }
    }
    assert_eq!(done["status"], "completed", "{done}");
    let url = done["assets"][0]["url"].as_str().unwrap();
    let key = url.split_once("/v1/models3d/assets/").expect("litegen's own asset route").1.split('?').next().unwrap();

    let req = Request::get(format!("/v1/models3d/assets/{key}")).body(Body::empty()).unwrap();
    let (status, selects) = selects_of(&app, req).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(selects.len(), 1, "an anonymous file request ran {} statements ({selects:?}); want 1", selects.len());
}
