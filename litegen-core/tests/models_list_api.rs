//! `GET /v1/models` lists the models the caller may use (the org's pool and
//! the app's model access). `?all=true` lists every model the gateway serves:
//! the dashboard's Providers page edits the org's pool from it, and couldn't
//! re-enable a model the pool left out. Generations still enforce the pool.

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

/// The owner's signed-in session: its cookie and CSRF token.
struct Session {
    cookie: String,
    csrf: String,
}

impl Session {
    async fn call(&self, app: &Router, method: &str, path: &str, body: Option<Value>) -> Reply {
        send(app, method, path, body, &[("cookie", &self.cookie), ("x-csrf-token", &self.csrf)]).await
    }
}

async fn app_and_owner() -> (Router, Session, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let app = harness::app_over_with(db, dir.path(), |_| {}, Some(MASTER_KEY), Mode::SingleTenant).await;
    let signup = send(&app, "POST", "/v1/auth/signup", Some(json!({ "email": "owner@example.com", "password": PASSWORD })), &[]).await;
    assert_eq!(signup.status, StatusCode::OK, "signup: {:?}", signup.body);
    let cookie = signup
        .headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|s| s.split(';').next())
        .find(|kv| kv.starts_with("litegen_session="))
        .expect("litegen_session cookie")
        .to_string();
    let csrf = send(&app, "GET", "/v1/auth/csrf", None, &[("cookie", &cookie)]).await;
    let csrf = csrf.body["csrf_token"].as_str().expect("csrf token").to_string();
    (app, Session { cookie, csrf }, dir)
}

fn ids(r: &Reply) -> Vec<String> {
    r.body["data"]
        .as_array()
        .unwrap_or_else(|| panic!("a model list: {:?}", r.body))
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn all_lists_the_models_outside_the_org_pool_and_generations_still_refuse_them() {
    let (app, owner, _dir) = app_and_owner().await;
    let orgs = owner.call(&app, "GET", "/v1/orgs", None).await;
    let org = orgs.body[0]["id"].as_str().expect("the owner's org").to_string();
    let put = owner
        .call(&app, "PUT", &format!("/v1/orgs/{org}/allowed-models"), Some(json!({ "models": ["mock/image-gen"] })))
        .await;
    assert_eq!(put.status, StatusCode::OK, "{:?}", put.body);

    let pooled = owner.call(&app, "GET", "/v1/models", None).await;
    assert_eq!(ids(&pooled), vec!["mock/image-gen".to_string()], "the pool narrows the default list");

    let all = ids(&owner.call(&app, "GET", "/v1/models?all=true", None).await);
    assert!(all.contains(&"mock/image-gen".to_string()) && all.contains(&"mock/video-gen".to_string()), "{all:?}");

    let videos = owner.call(&app, "GET", "/v1/models?all=true&media_type=video", None).await;
    assert!(ids(&videos).contains(&"mock/video-gen".to_string()), "{:?}", videos.body);
    assert!(
        videos.body["data"].as_array().unwrap().iter().all(|m| m["media_type"] == "video"),
        "media_type still narrows: {:?}",
        videos.body
    );

    let key = owner.call(&app, "POST", "/v1/keys", Some(json!({ "name": "gen", "scopes": "generate" }))).await;
    assert_eq!(key.status, StatusCode::CREATED, "{:?}", key.body);
    let key = key.body["key"].as_str().unwrap().to_string();
    let refused = send(
        &app,
        "POST",
        "/v1/videos/generations",
        Some(json!({ "model": "mock/video-gen", "prompt": "a lighthouse" })),
        &[("authorization", &format!("Bearer {key}"))],
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{:?}", refused.body);
    assert_eq!(refused.body["error"]["code"], "model_not_allowed", "{:?}", refused.body);
}

#[tokio::test]
async fn a_retired_model_answers_410_naming_its_replacement() {
    let (app, _owner, _dir) = app_and_owner().await;
    let master = [("authorization", "Bearer test-master-key")];
    let row = json!({
        "id": "mock/e2e-retired",
        "provider": "mock",
        "media_type": "image",
        "display_name": "Retired mock",
        "pricing": { "base_cost_usd": 0.01 },
        "capabilities": { "text_to_image": true },
        "prompt": { "required": true, "max_length": 100 },
        "deprecated_at": "2020-01-01",
        "retires_at": "2020-06-01",
        "replaced_by": "mock/image-gen"
    });
    let created = send(&app, "POST", "/v1/admin/models", Some(row), &master).await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);

    let r = send(&app, "POST", "/v1/images/generations", Some(json!({ "model": "mock/e2e-retired", "prompt": "a lighthouse" })), &master).await;
    assert_eq!(r.status, StatusCode::GONE, "{:?}", r.body);
    assert_eq!(r.body["error"]["code"], "model_retired", "{:?}", r.body);
    assert_eq!(r.body["error"]["replaced_by"], "mock/image-gen", "{:?}", r.body);
    assert!(r.body["error"]["message"].as_str().unwrap().contains("mock/image-gen"), "{:?}", r.body);

    let unknown = send(&app, "POST", "/v1/images/generations", Some(json!({ "model": "mock/never-was", "prompt": "a lighthouse" })), &master).await;
    assert_eq!((unknown.status, unknown.body["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("model_not_found")), "{:?}", unknown.body);

    let schema = send(&app, "GET", "/v1/models/mock/e2e-retired", None, &master).await;
    assert_eq!(schema.status, StatusCode::GONE, "{:?}", schema.body);
    assert_eq!(schema.body["error"]["replaced_by"], "mock/image-gen", "{:?}", schema.body);
}
