//! Known gaps closed in round 1 (`docs/superpowers/specs/2026-11-02-known-gaps-round-1-design.md`),
//! over the real route table.

mod harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn app() -> Router {
    let dir = tempfile::tempdir().expect("asset dir").keep();
    harness::app_with(&dir, |_| {}).await.0
}

async fn send(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_video_with_nothing_to_keep_fails_at_the_first_poll() {
    let app = app().await;
    let (status, submitted) = send(&app, "POST", "/v1/videos/generations", Some(json!({ "model": "mock/empty-video", "prompt": "a wave" }))).await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let id = submitted["id"].as_str().unwrap();
    // The row write is spawned at submit: let it land.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let (status, polled) = send(&app, "GET", &format!("/v1/videos/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{polled}");
    assert_eq!(polled["status"], "failed", "decided at the first poll: {polled}");
    assert!(polled["error"].as_str().unwrap_or("").contains("neither"), "{polled}");

    let (_, row) = send(&app, "GET", &format!("/v1/generations/{id}"), None).await;
    assert_eq!(row["status"], "failed", "{row}");
    assert_eq!(row["cost_usd"], 0.0, "refunded: {row}");
    let (_, again) = send(&app, "GET", &format!("/v1/videos/{id}"), None).await;
    assert_eq!(again["status"], "failed", "it reads the same afterwards: {again}");
}

/// An SVG (Recraft's vector models answer one) is kept as `.svg`, reported
/// as `svg` and served as an SVG: sandboxed and never sniffed on every route
/// that serves it from litegen's own origin, since an SVG can carry script.
#[tokio::test]
async fn an_svg_result_is_kept_as_svg() {
    let app = app().await;
    let (status, made) = send(&app, "POST", "/v1/images/generations", Some(json!({ "model": "mock/svg-image", "prompt": "a logo" }))).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let id = made["id"].as_str().unwrap();
    let (_, row) = send(&app, "GET", &format!("/v1/generations/{id}"), None).await;
    let asset = &row["metadata"]["assets"][0];
    assert_eq!(asset["format"], "svg", "{row}");
    let url = asset["url"].as_str().unwrap();
    assert!(url.split('?').next().unwrap().ends_with("/image_0.svg"), "{url}");

    for uri in [url[url.find("/v1/").unwrap()..].to_string(), format!("/v1/generations/{id}/assets/0")] {
        let resp = app.clone().oneshot(Request::get(&uri).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        let headers = resp.headers().clone();
        assert_eq!(headers["content-type"], "image/svg+xml", "{uri}");
        assert_eq!(headers["content-security-policy"], "sandbox", "{uri}");
        assert_eq!(headers["x-content-type-options"], "nosniff", "{uri}");
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        assert!(body.starts_with(b"<svg"), "{uri}");
    }
}
