//! Timed keyframes (PixelGenie 3D doc §5): `keyframe` references carry
//! `time_s`, seconds from the start of the clip, and the video shows each
//! keyframe's image at its time. litegen checks the times before anything is
//! billed and converts them into each vendor's own form.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use image::{ImageBuffer, Rgba, RgbaImage};
use serde_json::{json, Value};
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap().to_vec())
}

async fn post(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    let req = Request::post("/v1/videos/generations").header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
    let (s, b) = call(app, req).await;
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

async fn get_json(app: &axum::Router, path: &str) -> Value {
    let (_, b) = call(app, Request::get(path).body(Body::empty()).unwrap()).await;
    serde_json::from_slice(&b).unwrap_or(Value::Null)
}

/// A two-colour checkerboard: few colours, so a GIF frame holds it exactly,
/// and no two keyframes alike.
fn checker(a: [u8; 3], b: [u8; 3]) -> RgbaImage {
    ImageBuffer::from_fn(32, 24, |x, y| {
        let c = if (x / 4 + y / 4) % 2 == 0 { a } else { b };
        Rgba([c[0], c[1], c[2], 255])
    })
}

fn keyframe(img: &RgbaImage, time_s: f64) -> Value {
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    json!({ "type": "base64", "value": base64::engine::general_purpose::STANDARD.encode(png), "role": "keyframe", "time_s": time_s })
}

/// The finished video's frames, decoded.
async fn frames(app: &axum::Router, id: &str) -> Vec<RgbaImage> {
    let mut done = Value::Null;
    for _ in 0..40 {
        done = get_json(app, &format!("/v1/videos/{id}")).await;
        if done["status"] == "completed" || done["status"] == "failed" {
            break;
        }
    }
    assert_eq!(done["status"], "completed", "{done}");
    let (s, bytes) = call(app, Request::get(format!("/v1/generations/{id}/assets/0")).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    use image::AnimationDecoder;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)).unwrap();
    decoder.into_frames().collect_frames().unwrap().into_iter().map(|f| f.into_buffer()).collect()
}

#[tokio::test]
async fn the_frame_at_each_keyframes_time_is_that_keyframes_image() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let pinned = [
        (0.0, checker([200, 30, 30], [30, 30, 200])),
        (1.5, checker([20, 160, 40], [250, 250, 250])),
        (3.0, checker([0, 0, 0], [240, 220, 0])),
    ];
    // Sent out of order: the times place them.
    let refs: Vec<Value> = [2, 0, 1].iter().map(|&i| keyframe(&pinned[i].1, pinned[i].0)).collect();
    let (status, resp) = post(&app, json!({
        "model": "mock/keyframe-video", "prompt": "a knight's swing", "duration_seconds": 3.0, "fps": 10,
        "reference_images": refs,
    })).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let frames = frames(&app, resp["id"].as_str().unwrap()).await;
    assert_eq!(frames.len(), 31, "3 s at 10 fps, both ends included");
    for (t, img) in &pinned {
        let at = (t * 10.0_f64).round() as usize;
        assert!(frames[at] == *img, "the frame at {t} s is its keyframe");
    }
    assert!(frames[7] != pinned[0].1 && frames[7] != pinned[1].1, "between keyframes, the motion");
}

#[tokio::test]
async fn keyframe_times_are_checked_before_anything_is_billed() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let k = |t: f64| keyframe(&checker([1, 2, 3], [4, 5, 6]), t);
    let mut untimed = k(0.0);
    untimed.as_object_mut().unwrap().remove("time_s");
    let mut timed_init = k(1.0);
    timed_init["role"] = json!("first_frame");
    for (refs, code) in [
        (vec![untimed], "keyframe_time_required"),
        (vec![k(0.0), k(3.5)], "keyframe_time_out_of_range"),
        (vec![k(-0.5)], "keyframe_time_out_of_range"),
        (vec![k(1.0), k(1.0)], "keyframe_time_out_of_range"),
        (vec![timed_init], "param_unsupported"),
    ] {
        let (status, resp) = post(&app, json!({
            "model": "mock/keyframe-video", "prompt": "x", "duration_seconds": 3.0, "reference_images": refs,
        })).await;
        assert_eq!((status, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{resp}");
    }
    assert_eq!(db.get_request_logs(1, 10).await.unwrap().0.len(), 0, "nothing billed");
}

#[tokio::test]
async fn the_listing_says_which_models_take_timed_keyframes() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let list = get_json(&app, "/v1/models?media_type=video").await;
    let caps = |id: &str| list["data"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id}"))["capabilities"].clone();
    let k = caps("mock/keyframe-video");
    assert_eq!((k["supports_keyframes"].as_bool(), k["max_keyframes"].as_u64(), k["keyframe_timing"].as_str()), (Some(true), Some(8), Some("any")), "{k}");
    let plain = caps("mock/video-gen");
    assert_eq!((plain["supports_keyframes"].as_bool(), plain.get("max_keyframes")), (Some(false), None), "{plain}");
}
