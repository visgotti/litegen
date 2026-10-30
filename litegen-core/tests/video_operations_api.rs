//! Video operations (PixelGenie 3D doc §5, P2): `extend`, `retake` and
//! `video_edit` models work on a `source_video`; the job reports which
//! stretches came from the source and which were generated, and frames
//! outside the generated window are the source's own.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use image::AnimationDecoder;
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

async fn finished(app: &axum::Router, body: Value) -> Value {
    let (status, resp) = post(app, body).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let id = resp["id"].as_str().unwrap().to_string();
    for _ in 0..40 {
        let (_, b) = call(app, Request::get(format!("/v1/videos/{id}")).body(Body::empty()).unwrap()).await;
        let v: Value = serde_json::from_slice(&b).unwrap();
        if v["status"] == "completed" || v["status"] == "failed" {
            assert_eq!(v["status"], "completed", "{v}");
            return v;
        }
    }
    panic!("{id} never finished");
}

async fn frames(app: &axum::Router, id: &str) -> Vec<image::RgbaImage> {
    let (_, gif) = call(app, Request::get(format!("/v1/generations/{id}/assets/0")).body(Body::empty()).unwrap()).await;
    image::codecs::gif::GifDecoder::new(std::io::Cursor::new(gif))
        .unwrap()
        .into_frames()
        .collect_frames()
        .unwrap()
        .into_iter()
        .map(|f| f.into_buffer())
        .collect()
}

/// A finished 2 s, 10 fps mock clip: 20 frames.
async fn source(app: &axum::Router) -> Value {
    finished(app, json!({ "model": "mock/sprite-video", "prompt": "a knight walks", "duration_seconds": 2 })).await
}

fn on(src: &Value) -> Value {
    json!({ "type": "generation", "value": src["id"] })
}

fn close(a: &Value, b: f64) -> bool {
    (a.as_f64().unwrap() - b).abs() < 1e-6
}

#[tokio::test]
async fn extending_forward_keeps_the_source_frames_first_and_says_so() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = source(&app).await;
    let done = finished(&app, json!({
        "model": "mock/extend-video", "prompt": "he keeps walking", "source_video": on(&src),
        "direction": "forward", "duration_seconds": 1,
    })).await;
    assert!(close(&done["media"]["duration_s"], 3.0), "{done}");
    assert_eq!(done["segments"], json!([
        { "start_s": 0.0, "end_s": 2.0, "origin": "source" },
        { "start_s": 2.0, "end_s": 3.0, "origin": "generated" },
    ]), "{done}");
    let (before, after) = (frames(&app, src["id"].as_str().unwrap()).await, frames(&app, done["id"].as_str().unwrap()).await);
    assert_eq!(after.len(), 30);
    assert!(before.iter().zip(&after[..20]).all(|(a, b)| a == b), "the source's frames, unchanged");
}

#[tokio::test]
async fn extending_backward_puts_the_new_frames_first() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = source(&app).await;
    let done = finished(&app, json!({
        "model": "mock/extend-video", "prompt": "he walks in", "source_video": on(&src),
        "direction": "backward", "duration_seconds": 1,
    })).await;
    assert_eq!(done["segments"][0], json!({ "start_s": 0.0, "end_s": 1.0, "origin": "generated" }), "{done}");
    let (before, after) = (frames(&app, src["id"].as_str().unwrap()).await, frames(&app, done["id"].as_str().unwrap()).await);
    assert!(before.iter().zip(&after[10..]).all(|(a, b)| a == b), "the source's frames, last");
}

#[tokio::test]
async fn a_retake_regenerates_only_its_window() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let src = source(&app).await;
    let done = finished(&app, json!({
        "model": "mock/retake-video", "prompt": "he stumbles", "source_video": on(&src),
        "time_range": { "start_s": 0.5, "end_s": 1.5 }, "replace": "video",
    })).await;
    assert_eq!(done["segments"], json!([
        { "start_s": 0.0, "end_s": 0.5, "origin": "source" },
        { "start_s": 0.5, "end_s": 1.5, "origin": "generated" },
        { "start_s": 1.5, "end_s": 2.0, "origin": "source" },
    ]), "{done}");
    let (before, after) = (frames(&app, src["id"].as_str().unwrap()).await, frames(&app, done["id"].as_str().unwrap()).await);
    assert_eq!(after.len(), before.len());
    for (i, (a, b)) in before.iter().zip(&after).enumerate() {
        let generated = (5..15).contains(&i);
        assert_eq!(a == b, !generated, "frame {i}");
    }
}

#[tokio::test]
async fn an_operation_the_source_or_model_cannot_serve_is_refused_before_billing() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let src = source(&app).await;
    let logged = db.get_request_logs(1, 100).await.unwrap().0.len();
    for (body, status, code) in [
        (json!({ "model": "mock/extend-video", "prompt": "x", "direction": "forward", "duration_seconds": 1 }), 400, "source_video_required"),
        (json!({ "model": "mock/extend-video", "prompt": "x", "source_video": on(&src), "direction": "sideways", "duration_seconds": 1 }), 400, "param_enum_mismatch"),
        (json!({ "model": "mock/retake-video", "prompt": "x", "source_video": on(&src) }), 400, "param_required"),
        (json!({ "model": "mock/retake-video", "prompt": "x", "source_video": on(&src), "time_range": { "start_s": 1.0, "end_s": 1.2 } }), 400, "param_out_of_range"),
        (json!({ "model": "mock/retake-video", "prompt": "x", "source_video": on(&src), "time_range": { "start_s": 1.0, "end_s": 2.5 } }), 400, "param_out_of_range"),
        (json!({ "model": "mock/retake-video", "prompt": "x", "source_video": { "type": "generation", "value": "litegen-vid-nope" }, "time_range": { "start_s": 0.0, "end_s": 1.0 } }), 404, "source_not_found"),
        (json!({ "model": "mock/video-gen", "prompt": "x", "source_video": on(&src) }), 400, "param_unsupported"),
    ] {
        let (s, resp) = post(&app, body.clone()).await;
        assert_eq!((s.as_u16(), resp["error"]["code"].as_str()), (status, Some(code)), "{body}: {resp}");
    }
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), logged, "nothing billed");
}

#[tokio::test]
async fn the_listing_says_what_each_operation_works_on() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (_, b) = call(&app, Request::get("/v1/models?media_type=video").body(Body::empty()).unwrap()).await;
    let list: Value = serde_json::from_slice(&b).unwrap();
    let caps = |id: &str| list["data"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id}"))["capabilities"].clone();
    let extend = caps("mock/extend-video");
    assert_eq!((extend["task"].as_str(), extend["supports_source_video"].as_bool(), extend["extend_output"].as_str()), (Some("extend"), Some(true), Some("full")), "{extend}");
    let retake = caps("mock/retake-video");
    assert_eq!((retake["task"].as_str(), retake["supports_time_range"].as_bool()), (Some("retake"), Some(true)), "{retake}");
    assert_eq!(caps("mock/video-gen")["supports_source_video"], false);
}
