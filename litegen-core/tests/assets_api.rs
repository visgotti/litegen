//! PixelGenie L3: litegen keeps every result and serves it. A finished video
//! is re-hosted (the vendor's URL may expire or need the vendor's key) and
//! reads the same however often it is read; image results get a row; and
//! `GET /v1/generations/{id}/assets` lists every stored file with its hash,
//! each one streamable with only a litegen key.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use serde_json::{json, Value};
use sha2::Digest;
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap().to_vec();
    (status, headers, bytes)
}

async fn get_json(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    let (s, _, b) = call(app, Request::get(path).body(Body::empty()).unwrap()).await;
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

async fn post_json(app: &axum::Router, path: &str, body: Value) -> Value {
    let (_, _, b) = call(app, Request::post(path).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap()).await;
    serde_json::from_slice(&b).unwrap()
}

async fn finished_video(app: &axum::Router, id: &str) -> Value {
    for _ in 0..20 {
        let (_, v) = get_json(app, &format!("/v1/videos/{id}")).await;
        if v["status"] == "completed" || v["status"] == "failed" {
            return v;
        }
    }
    panic!("video {id} never finished");
}

#[tokio::test]
async fn a_finished_video_is_kept_by_litegen_and_reads_the_same_twice() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let submitted = post_json(&app, "/v1/videos/generations", json!({ "model": "mock/video-gen", "prompt": "a wave" })).await;
    let id = submitted["id"].as_str().unwrap();
    let first = finished_video(&app, id).await;
    assert_eq!(first["status"], "completed", "{first}");
    let url = first["video_url"].as_str().unwrap();
    assert!(url.starts_with("http") && url.contains("/assets/"), "re-hosted by litegen, not the vendor's: {url}");

    let (_, second) = get_json(&app, &format!("/v1/videos/{id}")).await;
    assert_eq!(first, second, "a finished video reads the same every time");
    let (_, row) = get_json(&app, &format!("/v1/generations/{id}")).await;
    assert_eq!(row["result_url"], first["video_url"], "{row}");
}

#[tokio::test]
async fn the_assets_route_lists_every_file_and_streams_each_one() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let submitted = post_json(&app, "/v1/models3d/generations", json!({ "model": "mock/mesh-3d", "prompt": "a red chair" })).await;
    let id = submitted["id"].as_str().unwrap().to_string();
    harness::poll_until_terminal(&app, &id).await;

    let (status, list) = get_json(&app, &format!("/v1/generations/{id}/assets")).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["generation_id"], id);
    let data = list["data"].as_array().expect("data");
    assert!(data.iter().any(|a| a["kind"] == "mesh" && a["format"] == "glb"), "{list}");
    for (i, a) in data.iter().enumerate() {
        assert_eq!(a["index"], i, "{a}");
        let (s, headers, bytes) = call(&app, Request::get(format!("/v1/generations/{id}/assets/{i}")).body(Body::empty()).unwrap()).await;
        assert_eq!(s, StatusCode::OK, "asset {i}");
        assert_eq!(a["size_bytes"], bytes.len(), "{a}");
        assert_eq!(a["sha256"], hex::encode(sha2::Sha256::digest(&bytes)), "{a}");
        if a["kind"] == "mesh" {
            assert_eq!(headers["content-type"], "model/gltf-binary");
        }
    }
    let (s, _) = get_json(&app, &format!("/v1/generations/{id}/assets/{}", data.len())).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "an index past the end");
}

#[tokio::test]
async fn a_video_asset_streams_its_bytes() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let submitted = post_json(&app, "/v1/videos/generations", json!({ "model": "mock/video-gen", "prompt": "a wave" })).await;
    let id = submitted["id"].as_str().unwrap();
    finished_video(&app, id).await;
    let (_, list) = get_json(&app, &format!("/v1/generations/{id}/assets")).await;
    let video = &list["data"][0];
    assert_eq!(video["kind"], "video", "{list}");
    let (s, _, bytes) = call(&app, Request::get(format!("/v1/generations/{id}/assets/0")).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!bytes.is_empty());
    assert_eq!(video["sha256"], hex::encode(sha2::Sha256::digest(&bytes)));
}

#[tokio::test]
async fn an_image_result_gets_a_completed_row_and_its_bytes_are_kept() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = post_json(&app, "/v1/images/generations", json!({ "model": "mock/image-gen", "prompt": "a lighthouse", "response_format": "b64_json" })).await;
    let id = resp["id"].as_str().expect("an image id");
    let returned = base64::engine::general_purpose::STANDARD.decode(resp["data"][0]["b64_json"].as_str().unwrap()).unwrap();

    let (status, row) = get_json(&app, &format!("/v1/generations/{id}")).await;
    assert_eq!(status, StatusCode::OK, "{row}");
    assert_eq!(row["status"], "completed");
    assert_eq!(row["media_type"], "image");

    let (_, list) = get_json(&app, &format!("/v1/generations/{id}/assets")).await;
    assert_eq!(list["data"][0]["kind"], "image", "{list}");
    let (s, _, bytes) = call(&app, Request::get(format!("/v1/generations/{id}/assets/0")).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(bytes, returned, "the stored image is the one the caller got");
}

#[tokio::test]
async fn another_orgs_assets_are_not_found() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    db.insert_generation("litegen-3d-elsewhere", None, "mock/mesh-3d", "mock", "model3d", Some("j"), 0.0, Some("another-org"), Some("another-app")).await.unwrap();
    db.update_generation_if_active(
        "litegen-3d-elsewhere", "completed", 100, Some("https://x/model.glb"), None, Some(chrono::Utc::now()),
        Some(&json!({ "assets": [{ "kind": "mesh", "url": "https://x/model.glb", "format": "glb" }] })),
    ).await.unwrap();
    for path in ["/v1/generations/litegen-3d-elsewhere/assets", "/v1/generations/litegen-3d-elsewhere/assets/0"] {
        let (s, _) = get_json(&app, path).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{path}");
    }
}
