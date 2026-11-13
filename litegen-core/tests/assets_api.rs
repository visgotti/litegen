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
    // Poll in a later second than the submit, so a poll stamped with the
    // poll time cannot pass for the submit time.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let first = finished_video(&app, id).await;
    assert_eq!(first["status"], "completed", "{first}");
    let url = first["video_url"].as_str().unwrap();
    assert!(url.starts_with("http") && url.contains("/assets/"), "re-hosted by litegen, not the vendor's: {url}");

    let (_, second) = get_json(&app, &format!("/v1/videos/{id}")).await;
    assert_eq!(first, second, "a finished video reads the same every time");
    // `created` is when the job was submitted, on every read: the row's.
    let (_, row_now) = get_json(&app, &format!("/v1/generations/{id}")).await;
    let created_at = chrono::DateTime::parse_from_rfc3339(row_now["created_at"].as_str().unwrap()).unwrap().timestamp();
    assert_eq!(first["created"], created_at, "{first}");
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

// ─── Stored files stream, and answer one range (review 2026-11-11, P6) ─────

/// A finished generation row in the default app whose one mesh is at `url`.
async fn completed_mesh_row(db: &litegen::db::sqlite::SqliteDatabase, id: &str, url: &str) {
    use litegen::api::middleware::{DEFAULT_APP_ID, DEFAULT_ORG_ID};
    use litegen::db::DatabaseStore;
    db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", Some("j"), 0.0, Some(DEFAULT_ORG_ID), Some(DEFAULT_APP_ID))
        .await
        .unwrap();
    let metadata = json!({ "assets": [{ "kind": "mesh", "format": "glb", "url": url }] });
    db.update_generation_if_active(id, "completed", 100, Some(url), None, Some(chrono::Utc::now()), Some(&metadata))
        .await
        .unwrap();
}

/// An S3 "bucket" on 127.0.0.1 that answers any request with a 4 MiB glb it
/// never finishes: 64 KiB of it (starting `glTF`), then it holds the
/// connection. Returns its endpoint.
async fn stalling_bucket() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut req = [0u8; 8192];
                let _ = sock.read(&mut req).await;
                let head = b"HTTP/1.1 200 OK\r\nContent-Type: model/gltf-binary\r\nContent-Length: 4194304\r\n\r\n";
                let mut chunk = vec![7u8; 64 * 1024];
                chunk[..4].copy_from_slice(b"glTF");
                if sock.write_all(head).await.is_ok() && sock.write_all(&chunk).await.is_ok() {
                    tokio::time::sleep(std::time::Duration::from_secs(120)).await;
                }
            });
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn a_file_in_s3_starts_streaming_before_the_bucket_has_sent_it_all() {
    use futures::StreamExt;
    let endpoint = stalling_bucket().await;
    let dir = tempfile::tempdir().unwrap();
    let ep = endpoint.clone();
    let (app, db) = harness::app_with(dir.path(), move |c| {
        c.image_storage.backend = "s3".into();
        c.image_storage.s3 = Some(litegen::config::S3StorageConfig {
            bucket_name: "b".into(),
            region: "us-east-1".into(),
            access_key_id: Some("k".into()),
            secret_access_key: Some("s".into()),
            endpoint_url: Some(ep),
            custom_public_url: None,
            session_token: None,
            credentials_expire_at: None,
        });
    })
    .await;
    let id = "litegen-3d-s3stream";
    completed_mesh_row(&db, id, &format!("{endpoint}/b/litegen/3d/{id}/model.glb")).await;

    let wait = std::time::Duration::from_secs(5);
    for route in [format!("/v1/generations/{id}/assets/0"), format!("/v1/models3d/assets/litegen/3d/{id}/model.glb")] {
        let resp = tokio::time::timeout(wait, app.clone().oneshot(Request::get(&route).body(Body::empty()).unwrap()))
            .await
            .unwrap_or_else(|_| panic!("{route} waited for the whole object before answering"))
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{route}");
        let mut body = resp.into_body().into_data_stream();
        let mut head = Vec::new();
        while head.len() < 4 {
            let chunk = tokio::time::timeout(wait, body.next())
                .await
                .unwrap_or_else(|_| panic!("{route} sent nothing while the bucket was still sending"))
                .expect("a first chunk")
                .unwrap();
            head.extend_from_slice(&chunk);
        }
        assert_eq!(&head[..4], b"glTF", "{route}");
    }
}

/// A finished `mock/mesh-3d`: its id, its glb's stored key, the glb's index
/// in its assets, and the glb's bytes.
async fn finished_glb(app: &axum::Router) -> (String, String, usize, Vec<u8>) {
    let id = harness::submit(app, "mock/mesh-3d", "a teapot").await;
    let done = harness::poll_until_terminal(app, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    let assets = done["assets"].as_array().unwrap();
    let index = assets.iter().position(|a| a["kind"] == "mesh" && a["format"] == "glb").expect("a glb");
    let url = assets[index]["url"].as_str().unwrap();
    let key = url.split_once("/v1/models3d/assets/").expect("litegen's own asset route").1.split('?').next().unwrap().to_string();
    let (s, _, bytes) = call(app, Request::get(format!("/v1/models3d/assets/{key}")).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    (id, key, index, bytes)
}

/// `route` answers a plain GET, one range, a suffix range and a range past
/// the end as a file server does.
async fn answers_ranges(app: &axum::Router, route: &str, whole: &[u8]) {
    let len = whole.len();
    let get = |range: Option<String>| {
        let mut req = Request::get(route);
        if let Some(r) = range {
            req = req.header("range", r);
        }
        req.body(Body::empty()).unwrap()
    };
    let (s, h, body) = call(app, get(Some("bytes=0-3".into()))).await;
    assert_eq!(s, StatusCode::PARTIAL_CONTENT, "{route}: bytes=0-3");
    assert_eq!(h["content-range"], format!("bytes 0-3/{len}"), "{route}");
    assert_eq!(h["content-length"], "4", "{route}");
    assert_eq!(body, b"glTF", "{route}");

    // Payloads review #10: a range unit is matched without regard to case
    // (RFC 9110 §14.1).
    let (s, h, body) = call(app, get(Some("Bytes=0-3".into()))).await;
    assert_eq!(s, StatusCode::PARTIAL_CONTENT, "{route}: Bytes=0-3");
    assert_eq!(h["content-range"], format!("bytes 0-3/{len}"), "{route}");
    assert_eq!(body, b"glTF", "{route}");

    let (s, h, body) = call(app, get(None)).await;
    assert_eq!(s, StatusCode::OK, "{route}: a plain GET");
    assert_eq!(body, whole, "{route}: a plain GET is the whole file");
    assert_eq!(h.get("accept-ranges").map(|v| v.to_str().unwrap()), Some("bytes"), "{route}: {h:?}");

    let (s, h, body) = call(app, get(Some("bytes=-10".into()))).await;
    assert_eq!(s, StatusCode::PARTIAL_CONTENT, "{route}: bytes=-10");
    assert_eq!(h["content-range"], format!("bytes {}-{}/{len}", len - 10, len - 1), "{route}");
    assert_eq!(body, &whole[len - 10..], "{route}");

    let (s, h, _) = call(app, get(Some(format!("bytes={len}-")))).await;
    assert_eq!(s, StatusCode::RANGE_NOT_SATISFIABLE, "{route}: a range past the end");
    assert_eq!(h["content-range"], format!("bytes */{len}"), "{route}");
}

#[tokio::test]
async fn a_range_request_gets_just_those_bytes() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (_, key, _, whole) = finished_glb(&app).await;
    answers_ranges(&app, &format!("/v1/models3d/assets/{key}"), &whole).await;
}

#[tokio::test]
async fn the_file_route_answers_a_range_too() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (id, _, index, whole) = finished_glb(&app).await;
    answers_ranges(&app, &format!("/v1/generations/{id}/assets/{index}"), &whole).await;
}

// ─── One observer keeps a finished video (review 2026-11-11, P4) ───────────

#[tokio::test]
async fn a_client_poll_stands_aside_while_another_observer_keeps_the_video() {
    use litegen::db::DatabaseStore;
    let dir = tempfile::tempdir().unwrap();
    let (app, db) = harness::app_with_mock_3d_assets_in(dir.path()).await;
    let submitted = post_json(&app, "/v1/videos/generations", json!({ "model": "mock/video-gen", "prompt": "a wave" })).await;
    let id = submitted["id"].as_str().unwrap();
    assert!(db.lease_generation_work(id, "delivery", "elsewhere", chrono::Duration::minutes(5)).await.unwrap());

    let (_, v) = get_json(&app, &format!("/v1/videos/{id}")).await;
    assert_eq!(v["status"], "processing", "the video was kept while another observer held its delivery: {v}");
    assert!(v["video_url"].is_null() && v["error"].is_null(), "{v}");
    let kept = dir.path().join(format!("litegen/video/{id}"));
    assert!(!kept.exists(), "the video was kept while another observer held its delivery: {}", kept.display());
}
