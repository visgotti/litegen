//! Late and asynchronous images (docs/async-images.md): an image job the
//! vendor took is delivered even when it outlasts litegen's deadline, and a
//! caller can ask not to wait for it (`Prefer: respond-async`, `wait=N`).
//! The mock runs a job for prompts with `job_ms=<n>`.

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

mod harness;

const MODEL: &str = "mock/priced-image";
const PRICE: f64 = 0.10;

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn image(prompt: &str, prefer: Option<&str>) -> Request<Body> {
    let mut req = Request::post("/v1/images/generations").header("content-type", "application/json");
    if let Some(p) = prefer {
        req = req.header("prefer", p);
    }
    req.body(Body::from(json!({ "model": MODEL, "prompt": prompt }).to_string())).unwrap()
}

async fn generation(app: &axum::Router, id: &str) -> Value {
    let (status, _, body) = call(app, Request::get(format!("/v1/generations/{id}")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// One tick of the background poller.
async fn tick(state: &Arc<litegen::api::middleware::AppState>) {
    litegen::proxy::poller::poll_once_presenting(
        &state.db,
        &state.router.registry,
        state.secrets_key,
        state.mode,
        &state.router.model3d_store,
        None,
    )
    .await;
}

async fn app() -> (axum::Router, Arc<litegen::api::middleware::AppState>) {
    let dir = tempfile::tempdir().unwrap().keep();
    let db = Arc::new(litegen::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    harness::app_and_state_over(db, &dir).await
}

#[tokio::test]
async fn respond_async_answers_202_and_the_poller_delivers_the_image() {
    let (app, state) = app().await;
    let (status, headers, body) = call(&app, image("a lighthouse job_ms=150", Some("respond-async"))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!((body["object"].as_str(), body["status"].as_str(), body["model"].as_str()), (Some("image.generation"), Some("processing"), Some(MODEL)), "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(headers.get("location").unwrap(), &format!("/v1/generations/{id}"));
    assert_eq!(headers.get("preference-applied").unwrap(), "respond-async");

    let row = generation(&app, &id).await;
    assert!(matches!(row["status"].as_str(), Some("pending" | "processing")), "{row}");
    assert!((row["cost_usd"].as_f64().unwrap() - PRICE).abs() < 1e-9, "charged when the vendor took it: {row}");

    // Still running: the poller leaves it running.
    tick(&state).await;
    assert!(matches!(generation(&app, &id).await["status"].as_str(), Some("pending" | "processing")));

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    tick(&state).await;
    let done = generation(&app, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    let asset = &done["metadata"]["assets"][0];
    assert_eq!((asset["kind"].as_str(), asset["format"].as_str()), (Some("image"), Some("png")), "{done}");
    assert!(asset["url"].as_str().is_some_and(|u| !u.is_empty()), "{done}");
    assert!((done["cost_usd"].as_f64().unwrap() - PRICE).abs() < 1e-9, "billed once: {done}");
}

#[tokio::test]
async fn a_sync_call_that_outlasts_the_vendor_timeout_is_delivered_later() {
    let dir = tempfile::tempdir().unwrap().keep();
    let db = Arc::new(litegen::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_over_with(db, &dir, |cfg| cfg.server.vendor_timeout_secs = Some(5)).await;
    let (status, _, body) = call(&app, image("a slow lighthouse job_ms=5600", None)).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "{body}");
    let e = &body["error"];
    assert_eq!((e["code"].as_str(), e["status"].as_str(), e["retryable"].as_bool()), (Some("vendor_timeout"), Some("processing"), Some(false)), "{body}");
    let id = e["generation_id"].as_str().expect("the 504 names the generation").to_string();
    assert!(e["vendor_job_id"].as_str().is_some_and(|j| j.starts_with("mock-image-job-")), "{body}");

    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    tick(&state).await;
    let done = generation(&app, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["metadata"]["assets"][0]["kind"], "image", "{done}");
    assert_eq!(done["result_url"], done["metadata"]["assets"][0]["url"], "{done}");
}

#[tokio::test]
async fn a_late_image_the_vendor_fails_ends_failed_with_its_reason() {
    let (app, state) = app().await;
    let (status, _, body) = call(&app, image("a lighthouse job_ms=50 job_fails", Some("respond-async"))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    tick(&state).await;
    let row = generation(&app, &id).await;
    assert_eq!(row["status"], "failed", "{row}");
    assert_eq!(row["cost_usd"].as_f64(), Some(0.0), "refunded: {row}");
    assert!(row["error_message"].as_str().is_some_and(|m| m.contains("mock job failed")) || row["error"].to_string().contains("mock job failed"), "{row}");
}

#[tokio::test]
async fn wait_answers_with_the_image_when_the_job_beats_it_and_202_when_it_does_not() {
    let (app, _state) = app().await;
    let (status, headers, body) = call(&app, image("a quick lighthouse job_ms=100", Some("wait=2"))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"][0]["url"].is_string() || body["data"][0]["b64_json"].is_string(), "{body}");
    assert!(headers.get("preference-applied").is_none(), "nothing deferred: {headers:?}");

    let started = std::time::Instant::now();
    let (status, headers, body) = call(&app, image("a slower lighthouse job_ms=4000", Some("wait=1"))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(headers.get("preference-applied").unwrap(), "wait=1");
    assert!(started.elapsed() < std::time::Duration::from_secs(3), "answered after about a second: {:?}", started.elapsed());
}

#[tokio::test]
async fn a_caller_that_keeps_nothing_is_answered_synchronously() {
    let (app, _state) = app().await;
    let mut req = image("a private lighthouse job_ms=100", Some("respond-async"));
    req.headers_mut().insert("x-litegen-keep-outputs", "false".parse().unwrap());
    let (status, headers, body) = call(&app, req).await;
    assert_eq!(status, StatusCode::OK, "the preference can't be honoured, so it is not: {body}");
    assert!(body["data"][0]["b64_json"].is_string(), "{body}");
    assert!(headers.get("preference-applied").is_none());
}

#[tokio::test]
async fn a_late_image_keeps_its_request_for_the_logs_trace() {
    let (app, _state) = app().await;
    let (status, _, body) = call(&app, image("a traced lighthouse job_ms=150", Some("respond-async"))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    let (status, _, artifact) = call(&app, Request::get(format!("/v1/logs/{id}/artifact")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{artifact}");
    assert_eq!(artifact["prompt"], "a traced lighthouse job_ms=150", "{artifact}");
    assert_eq!(artifact["media_type"], "image", "{artifact}");
}
