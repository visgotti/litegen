//! `Idempotency-Key` on every billed POST (PixelGenie L1, 3D doc §3G): the
//! same key and body within 24 hours replays the first response and bills
//! nothing; the same key with another body is a 409. And a vendor call that
//! timed out is never sent again.

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

async fn post(app: &axum::Router, path: &str, key: Option<&str>, body: &Value) -> (StatusCode, HeaderMap, Value) {
    let mut req = Request::post(path).header("content-type", "application/json");
    if let Some(k) = key {
        req = req.header("idempotency-key", k);
    }
    let resp = app.clone().oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = axum::body::to_bytes(resp.into_body(), 32 * 1024 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn replayed(h: &HeaderMap) -> bool {
    h.get("idempotent-replayed").and_then(|v| v.to_str().ok()) == Some("true")
}

async fn request_logs(db: &litegen::db::sqlite::SqliteDatabase) -> usize {
    use litegen::db::DatabaseStore;
    db.get_request_logs(1, 1000).await.unwrap().0.len()
}

#[tokio::test]
async fn a_repeated_3d_submit_returns_the_same_generation_and_bills_once() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let body = json!({ "model": "mock/mesh-3d", "prompt": "a red chair" });
    let (s1, h1, first) = post(&app, "/v1/models3d/generations", Some("pg-3d-1"), &body).await;
    let (s2, h2, second) = post(&app, "/v1/models3d/generations", Some("pg-3d-1"), &body).await;
    assert_eq!((s1, s2), (StatusCode::OK, StatusCode::OK), "{first} {second}");
    assert_eq!(first["id"], second["id"]);
    assert!(!replayed(&h1) && replayed(&h2));
    assert_eq!(db.count_generations(None).await.unwrap(), 1, "one vendor job, one row");
    assert_eq!(request_logs(&db).await, 1, "one billing row");
}

#[tokio::test]
async fn a_repeated_video_submit_returns_the_same_generation() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let body = json!({ "model": "mock/video-gen", "prompt": "a wave" });
    let (_, _, first) = post(&app, "/v1/videos/generations", Some("pg-vid-1"), &body).await;
    let (s2, h2, second) = post(&app, "/v1/videos/generations", Some("pg-vid-1"), &body).await;
    assert_eq!(s2, StatusCode::OK, "{second}");
    assert_eq!(first["id"], second["id"]);
    assert!(replayed(&h2));
    assert_eq!(db.count_generations(None).await.unwrap(), 1);
}

#[tokio::test]
async fn a_repeated_image_returns_the_same_image_and_logs_once() {
    let (app, db) = harness::app_with_mock_3d().await;
    let body = json!({ "model": "mock/image-gen", "prompt": "a lighthouse", "response_format": "b64_json" });
    let (_, _, first) = post(&app, "/v1/images/generations", Some("pg-img-1"), &body).await;
    let (s2, h2, second) = post(&app, "/v1/images/generations", Some("pg-img-1"), &body).await;
    assert_eq!(s2, StatusCode::OK, "{second}");
    assert_eq!(first, second, "byte-for-byte the first response");
    assert!(replayed(&h2));
    assert_eq!(request_logs(&db).await, 1);
}

#[tokio::test]
async fn the_same_key_with_another_body_is_refused() {
    let (app, _db) = harness::app_with_mock_3d().await;
    post(&app, "/v1/models3d/generations", Some("pg-reuse"), &json!({ "model": "mock/mesh-3d", "prompt": "a chair" })).await;
    let (s, _, body) = post(&app, "/v1/models3d/generations", Some("pg-reuse"), &json!({ "model": "mock/mesh-3d", "prompt": "a table" })).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "idempotency_key_reused", "{body}");
}

#[tokio::test]
async fn a_refused_request_does_not_spend_its_key() {
    let (app, _db) = harness::app_with_mock_3d().await;
    // Refused by validation before anything ran: a corrected request may reuse the key.
    let (s1, _, b1) = post(&app, "/v1/models3d/generations", Some("pg-fix"), &json!({ "model": "mock/mesh-3d", "prompt": "" })).await;
    assert_eq!(s1, StatusCode::BAD_REQUEST, "{b1}");
    let (s2, h2, b2) = post(&app, "/v1/models3d/generations", Some("pg-fix"), &json!({ "model": "mock/mesh-3d", "prompt": "a chair" })).await;
    assert_eq!(s2, StatusCode::OK, "{b2}");
    assert!(!replayed(&h2));
}

#[tokio::test]
async fn a_malformed_key_is_refused() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let long = "k".repeat(256);
    for key in ["has space", long.as_str()] {
        let (s, _, body) = post(&app, "/v1/models3d/generations", Some(key), &json!({ "model": "mock/mesh-3d", "prompt": "x" })).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{key:?}: {body}");
        assert_eq!(body["error"]["code"], "idempotency_key_invalid", "{body}");
    }
}

/// L1's done-when: a mock whose first attempt times out shows exactly one
/// vendor call, and the 504 it produced is what a retry with the key gets.
#[tokio::test]
async fn a_timed_out_vendor_call_is_never_resent() {
    let dir = tempfile::tempdir().unwrap();
    let (app, db) = harness::app_with(dir.path(), |c| c.server.vendor_timeout_secs = Some(1)).await;
    let body = json!({ "model": "mock/slow-image", "prompt": "a slow lighthouse" });
    let before = litegen::providers::image::mock::slow_image_calls("a slow lighthouse");

    let first = {
        let app = app.clone();
        let body = body.clone();
        tokio::spawn(async move { post(&app, "/v1/images/generations", Some("pg-slow"), &body).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (s_dup, h_dup, b_dup) = post(&app, "/v1/images/generations", Some("pg-slow"), &body).await;
    assert_eq!(s_dup, StatusCode::CONFLICT, "a duplicate while the first runs: {b_dup}");
    assert_eq!(b_dup["error"]["code"], "idempotency_request_in_progress");
    assert!(h_dup.get("retry-after").is_some());

    let (s1, _, b1) = first.await.unwrap();
    assert_eq!(s1, StatusCode::GATEWAY_TIMEOUT, "{b1}");
    assert_eq!(b1["error"]["code"], "vendor_timeout", "{b1}");
    assert_eq!(litegen::providers::image::mock::slow_image_calls("a slow lighthouse") - before, 1, "one vendor call, never re-sent");

    let (s3, h3, b3) = post(&app, "/v1/images/generations", Some("pg-slow"), &body).await;
    assert_eq!(s3, StatusCode::GATEWAY_TIMEOUT, "{b3}");
    assert!(replayed(&h3), "the outcome is replayed, not re-run");
    assert_eq!(litegen::providers::image::mock::slow_image_calls("a slow lighthouse") - before, 1);
    assert_eq!(request_logs(&db).await, 1);
}

/// A client that hangs up mid-call does not strand its key: the request runs
/// to its end and its outcome is recorded, so the retry the client makes is
/// a replay — never a 409 for minutes, then a second, billed run.
#[tokio::test]
async fn a_client_that_hangs_up_mid_call_gets_the_recorded_outcome_on_retry() {
    let dir = tempfile::tempdir().unwrap();
    let (app, _db) = harness::app_with(dir.path(), |c| c.server.vendor_timeout_secs = Some(1)).await;
    let body = json!({ "model": "mock/slow-image", "prompt": "a lighthouse, abandoned" });

    let hung_up = tokio::time::timeout(std::time::Duration::from_millis(300), post(&app, "/v1/images/generations", Some("pg-hangup"), &body)).await;
    assert!(hung_up.is_err(), "the client gave up before the vendor answered");
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

    let (s, h, b) = post(&app, "/v1/images/generations", Some("pg-hangup"), &body).await;
    assert_eq!(s, StatusCode::GATEWAY_TIMEOUT, "{b}");
    assert!(replayed(&h), "the abandoned run's outcome, replayed: {b}");
}
