//! Vendor errors that say what happened (PixelGenie L15): a vendor's 4xx is a
//! litegen 4xx `vendor_rejected` carrying `provider_error {status, code,
//! message}`; a 5xx is a 502 that says whether to retry; every error `code`
//! is a string.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn fal_answering(status: u16, body: Value) -> (axum::Router, MockServer) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/fal-ai/flux/dev"))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap().keep();
    let uri = server.uri();
    let (app, _db) = harness::app_with(&dir, move |cfg| {
        cfg.providers.insert("fal".into(), serde_json::from_value(json!({ "enabled": true, "api_key": "k", "api_base": uri })).unwrap());
    })
    .await;
    (app, server)
}

fn generate() -> Request<Body> {
    Request::post("/v1/images/generations")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "model": "fal/flux-dev", "prompt": "a lighthouse", "response_format": "b64_json" }).to_string()))
        .unwrap()
}

#[tokio::test]
async fn a_vendors_4xx_is_a_litegen_4xx_that_names_the_vendors_reason() {
    let detail = json!({ "detail": [{ "loc": ["body", "prompt"], "msg": "prompt flagged by the safety checker", "type": "content_policy_violation" }] });
    let (app, _server) = fal_answering(422, detail).await;
    let (status, body) = call(&app, generate()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let e = &body["error"];
    assert_eq!((e["type"].as_str(), e["code"].as_str()), (Some("vendor_rejected"), Some("vendor_rejected")), "{body}");
    let p = &e["provider_error"];
    assert_eq!((p["status"].as_u64(), p["code"].as_str()), (Some(422), Some("content_policy_violation")), "{body}");
    assert!(p["message"].as_str().unwrap_or_default().contains("safety checker"), "{body}");
    assert_eq!(e["retryable"], json!(false), "{body}");
}

#[tokio::test]
async fn a_vendors_5xx_is_a_502_that_says_to_retry() {
    let (app, _server) = fal_answering(503, json!({ "detail": "upstream overloaded" })).await;
    let (status, body) = call(&app, generate()).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    let e = &body["error"];
    assert_eq!((e["type"].as_str(), e["code"].as_str(), e["retryable"].as_bool()), (Some("vendor_error"), Some("vendor_error"), Some(true)), "{body}");
    assert_eq!(e["provider_error"]["status"].as_u64(), Some(503), "{body}");
}

#[tokio::test]
async fn every_error_code_is_a_string() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (status, body) = call(&app, Request::get("/v1/models/nobody/nothing").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"]["code"].is_string(), "{body}");
    let (status, body) = call(
        &app,
        Request::post("/v1/images/generations")
            .header("content-type", "application/json")
            .body(Body::from(json!({ "model": "openai/gpt-image-2", "prompt": "x" }).to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no openai configured: {body}");
    assert!(body["error"]["code"].is_string(), "{body}");
}

/// A verdict with no HTTP status is the vendor's last word, not an outage: a
/// moderated BFL job says not to retry — the same request would be moderated
/// again, and paid for again.
#[tokio::test]
async fn a_moderated_job_says_not_to_retry() {
    let server = MockServer::start().await;
    let polling_url = format!("{}/v1/get_result?id=bfl-mod", server.uri());
    Mock::given(method("POST"))
        .and(path("/v1/flux-dev"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "bfl-mod", "polling_url": polling_url })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/get_result"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "bfl-mod", "status": "Content Moderated" })))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap().keep();
    let uri = server.uri();
    let (app, _db) = harness::app_with(&dir, move |cfg| {
        cfg.providers.insert("bfl".into(), serde_json::from_value(json!({ "enabled": true, "api_key": "k", "api_base": uri })).unwrap());
    })
    .await;
    let req = Request::post("/v1/images/generations")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "model": "bfl/flux-dev", "prompt": "a lighthouse" }).to_string()))
        .unwrap();
    let (status, body) = call(&app, req).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["error"]["retryable"], json!(false), "{body}");
}

// ─── E7: a failed submit is in the request log ──────────────────────────────

/// An app whose fal refuses everything (422 on any path), and its database.
async fn fal_refusing_everything() -> (axum::Router, std::sync::Arc<litegen::db::sqlite::SqliteDatabase>, MockServer) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({ "detail": [{ "msg": "refused", "type": "x" }] })))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap().keep();
    let uri = server.uri();
    let (app, db) = harness::app_with(&dir, move |cfg| {
        cfg.providers.insert("fal".into(), serde_json::from_value(json!({ "enabled": true, "api_key": "k", "api_base": uri })).unwrap());
    })
    .await;
    (app, db, server)
}

fn submit(path: &str, body: Value) -> Request<Body> {
    Request::post(path).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap()
}

/// The request log's rows, once there are `n` (or after 2 s).
async fn logged(db: &litegen::db::sqlite::SqliteDatabase, n: usize) -> Vec<litegen::types::RequestLog> {
    use litegen::db::DatabaseStore;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let rows = db.get_request_logs(1, 10).await.unwrap().0;
        if rows.len() >= n || std::time::Instant::now() > deadline {
            return rows;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// E7: a video submit the vendor refused is a `failed` row in the request
/// log, naming its provider, and counts in `/v1/stats`. It used to leave no
/// row at all.
#[tokio::test]
async fn a_failed_video_submit_is_in_the_request_log() {
    use litegen::db::DatabaseStore;
    let (app, db, _server) = fal_refusing_everything().await;
    let (status, body) = call(&app, submit("/v1/videos/generations", json!({ "model": "fal/ltx-2.3", "prompt": "a wave" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "vendor_rejected", "{body}");
    let rows = logged(&db, 1).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!((row.status, row.media_type, row.model.as_str(), row.provider.as_str()), (litegen::types::GenerationStatus::Failed, litegen::types::MediaType::Video, "fal/ltx-2.3", "fal"), "{row:?}");
    assert!(row.error.as_deref().unwrap_or_default().contains("refused"), "{row:?}");
    assert_eq!(db.get_stats().await.unwrap().failed_requests, 1);
}

/// E7: the same for a 3D submit.
#[tokio::test]
async fn a_failed_3d_submit_is_in_the_request_log() {
    use base64::Engine;
    let (app, db, _server) = fal_refusing_everything().await;
    let png = litegen::providers::image::visual_mock::generate_visual_image_png("a knight");
    let (status, body) = call(
        &app,
        submit(
            "/v1/models3d/generations",
            json!({
                "model": "fal/trellis",
                "reference_images": [{ "type": "base64", "value": base64::engine::general_purpose::STANDARD.encode(png), "role": "init" }],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let rows = logged(&db, 1).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!((row.status, row.media_type, row.provider.as_str()), (litegen::types::GenerationStatus::Failed, litegen::types::MediaType::Model3d, "fal"), "{row:?}");
}

/// E7: a failed image's row names its provider. It said "unknown".
#[tokio::test]
async fn a_failed_image_submit_names_its_provider() {
    let (app, db, _server) = fal_refusing_everything().await;
    let (status, body) = call(&app, submit("/v1/images/generations", json!({ "model": "fal/flux-dev", "prompt": "a lighthouse" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let rows = logged(&db, 1).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].provider, "fal", "{:?}", rows[0]);
}
