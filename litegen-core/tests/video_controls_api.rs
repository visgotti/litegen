//! Video controls for sprites (PixelGenie L10): a still camera, no audio, a
//! loop and a short, allowed duration are request fields; and a finished job
//! says what came back — `{duration_s, fps, width, height, has_audio}` — read
//! from the video itself.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap().to_vec())
}

async fn post(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    let (s, b) = call(app, Request::post("/v1/videos/generations").header("content-type", "application/json").body(Body::from(body.to_string())).unwrap()).await;
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

async fn finished(app: &axum::Router, id: &str) -> Value {
    for _ in 0..40 {
        let (_, b) = call(app, Request::get(format!("/v1/videos/{id}")).body(Body::empty()).unwrap()).await;
        let v: Value = serde_json::from_slice(&b).unwrap();
        if v["status"] == "completed" || v["status"] == "failed" {
            return v;
        }
    }
    panic!("{id} never finished");
}

#[tokio::test]
async fn a_looping_still_silent_clip_reports_what_came_back() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (status, resp) = post(&app, json!({
        "model": "mock/sprite-video", "prompt": "a knight idles", "duration_seconds": 2,
        "camera": "fixed", "audio": false, "loop": true,
    })).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let done = finished(&app, resp["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "completed", "{done}");
    let media = &done["media"];
    assert_eq!((media["width"].as_u64(), media["height"].as_u64(), media["fps"].as_f64(), media["has_audio"].as_bool()), (Some(64), Some(64), Some(10.0), Some(false)), "{done}");
    assert!((media["duration_s"].as_f64().unwrap() - 2.0).abs() < 1e-6, "{done}");

    let id = done["id"].as_str().unwrap();
    let (_, gif) = call(&app, Request::get(format!("/v1/generations/{id}/assets/0")).body(Body::empty()).unwrap()).await;
    use image::AnimationDecoder;
    let frames: Vec<_> = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(gif)).unwrap().into_frames().collect_frames().unwrap();
    assert!(frames.first().unwrap().buffer() == frames.last().unwrap().buffer(), "a loop ends on its first frame");
}

#[tokio::test]
async fn controls_and_durations_a_model_lacks_are_refused() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for (body, code) in [
        (json!({ "model": "mock/sprite-video", "prompt": "x", "duration_seconds": 3 }), "param_enum_mismatch"),
        (json!({ "model": "mock/sprite-video", "prompt": "x", "camera": "orbit" }), "param_enum_mismatch"),
        (json!({ "model": "mock/video-gen", "prompt": "x", "loop": true }), "param_unsupported"),
    ] {
        let (status, resp) = post(&app, body.clone()).await;
        assert_eq!((status, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{body}: {resp}");
    }
}

#[tokio::test]
async fn the_listing_says_which_controls_and_durations_a_model_takes() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (_, b) = call(&app, Request::get("/v1/models?media_type=video").body(Body::empty()).unwrap()).await;
    let list: Value = serde_json::from_slice(&b).unwrap();
    let caps = |id: &str| list["data"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id}"))["capabilities"].clone();
    let s = caps("mock/sprite-video");
    assert_eq!(
        (s["supports_fixed_camera"].as_bool(), s["supports_audio_toggle"].as_bool(), s["supports_loop"].as_bool(), s["duration_values"].clone()),
        (Some(true), Some(true), Some(true), json!([2.0, 4.0])),
        "{s}"
    );
    let plain = caps("mock/video-gen");
    assert_eq!((plain["supports_loop"].as_bool(), plain["duration_values"].clone()), (Some(false), json!([])), "{plain}");
}
