//! PixelGenie L2: the job row exists when the submit returns, it records the
//! request (never reference bytes), and a submit whose row could not be
//! written fails instead of answering with an id nothing can poll.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router over a sqlite DB

async fn post(app: &axum::Router, path: &str, body: &Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(Request::post(path).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 8 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// An app over a file-backed DB, and a second connection to the same file.
async fn app_on_file() -> (axum::Router, Arc<litegen::db::sqlite::SqliteDatabase>, sqlx::SqlitePool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}/litegen.db?mode=rwc", dir.path().display());
    let db = Arc::new(litegen::db::sqlite::SqliteDatabase::connect(&url).await.unwrap());
    let app = harness::app_over(db.clone(), dir.path(), |_| {}).await;
    let raw = sqlx::SqlitePool::connect(&url).await.unwrap();
    (app, db, raw, dir)
}

async fn refuse_generation_inserts(raw: &sqlx::SqlitePool) {
    sqlx::query("CREATE TRIGGER refuse_generations BEFORE INSERT ON generations BEGIN SELECT RAISE(ABORT, 'disk full'); END;")
        .execute(raw)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_video_submit_whose_row_cannot_be_written_fails() {
    let (app, _db, raw, _dir) = app_on_file().await;
    refuse_generation_inserts(&raw).await;
    let (status, body) = post(&app, "/v1/videos/generations", &json!({ "model": "mock/video-gen", "prompt": "a wave" })).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["error"]["code"], "generation_record_failed", "{body}");
}

#[tokio::test]
async fn a_3d_submit_whose_row_cannot_be_written_fails() {
    let (app, _db, raw, _dir) = app_on_file().await;
    refuse_generation_inserts(&raw).await;
    let (status, body) = post(&app, "/v1/models3d/generations", &json!({ "model": "mock/mesh-3d", "prompt": "a chair" })).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["error"]["code"], "generation_record_failed", "{body}");
}

#[tokio::test]
async fn a_video_id_answers_on_the_very_next_request() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (_, submitted) = post(&app, "/v1/videos/generations", &json!({ "model": "mock/video-gen", "prompt": "a wave" })).await;
    let id = submitted["id"].as_str().expect("an id");
    let resp = app.clone().oneshot(Request::get(format!("/v1/generations/{id}")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_row_records_the_request_but_never_reference_bytes() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let png = litegen::providers::image::mock::MockProvider::placeholder_png();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    let (status, submitted) = post(&app, "/v1/models3d/generations", &json!({
        "model": "mock/mesh-3d",
        "prompt": "a red chair",
        "target_polycount": 5000,
        "reference_images": [{ "type": "base64", "value": b64, "role": "init" }],
    })).await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let id = submitted["id"].as_str().unwrap();
    let row = db.get_generation(id).await.unwrap().expect("row written before the response");
    let req = &row.metadata.expect("metadata")["request"];
    assert_eq!(req["prompt"], "a red chair", "{req}");
    assert_eq!(req["params"]["target_polycount"], 5000, "{req}");
    let r0 = &req["reference_images"][0];
    assert_eq!(r0["role"], "init", "{req}");
    assert_eq!(r0["type"], "base64", "{req}");
    assert_eq!(r0["bytes"], png.len(), "{req}");
    use sha2::Digest;
    assert_eq!(r0["sha256"], hex::encode(sha2::Sha256::digest(&png)), "{req}");
    assert!(!req.to_string().contains(&b64), "the image itself is never stored");
}
