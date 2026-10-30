//! Expanded canvases (PixelGenie 3D doc §4, P2): `expand: {left, top,
//! right, bottom}` grows the init image's canvas by that many pixels a side;
//! the result is the whole canvas with the source at (left, top), and every
//! image reports the size it came back at.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use image::{ImageBuffer, Rgba, RgbaImage};
use serde_json::{json, Value};
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

fn init_img() -> RgbaImage {
    ImageBuffer::from_fn(40, 30, |x, y| Rgba([(x * 6) as u8, (y * 8) as u8, 77, 255]))
}

fn init() -> Value {
    let mut png = Vec::new();
    init_img().write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    json!([{ "type": "base64", "value": base64::engine::general_purpose::STANDARD.encode(png), "role": "init" }])
}

async fn post(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(Request::post("/v1/images/generations").header("content-type", "application/json").body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn the_canvas_grows_by_each_side_with_the_source_at_left_top() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (status, resp) = post(&app, json!({
        "model": "mock/expand-image", "prompt": "more sky", "reference_images": init(), "response_format": "b64_json",
        "expand": { "left": 5, "top": 7, "right": 3, "bottom": 0 },
    })).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let data = &resp["data"][0];
    assert_eq!((data["width"].as_u64(), data["height"].as_u64()), (Some(48), Some(37)), "the delivered canvas size: {data}");
    let out = image::load_from_memory(&base64::engine::general_purpose::STANDARD.decode(data["b64_json"].as_str().unwrap()).unwrap()).unwrap().to_rgba8();
    assert_eq!(out.dimensions(), (48, 37));
    let src = init_img();
    for (x, y, p) in out.enumerate_pixels() {
        let inside = (5..45).contains(&x) && (7..37).contains(&y);
        if inside {
            assert_eq!(p, src.get_pixel(x - 5, y - 7), "source at {x},{y}");
        } else {
            assert_eq!(p.0, [0xff, 0xcd, 0x75, 0xff], "new canvas at {x},{y}");
        }
    }
}

#[tokio::test]
async fn an_expansion_the_model_cannot_make_is_refused_before_billing() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let model = "mock/expand-image";
    for (body, code) in [
        (json!({ "model": model, "prompt": "x", "expand": { "left": 4, "top": 0, "right": 0, "bottom": 0 } }), "init_required"),
        (json!({ "model": model, "prompt": "x", "reference_images": init(), "expand": { "left": 0, "top": 0, "right": 0, "bottom": 0 } }), "param_out_of_range"),
        (json!({ "model": model, "prompt": "x", "reference_images": init(), "expand": { "left": 600, "top": 0, "right": 0, "bottom": 0 } }), "param_out_of_range"),
        (json!({ "model": "mock/format-image", "prompt": "x", "expand": { "left": 4, "top": 0, "right": 0, "bottom": 0 } }), "param_unsupported"),
    ] {
        let (s, resp) = post(&app, body.clone()).await;
        assert_eq!((s, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{body}: {resp}");
    }
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed");
}

#[tokio::test]
async fn the_listing_says_which_models_expand() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/v1/models?media_type=image").body(Body::empty()).unwrap()).await.unwrap();
    let list: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap()).unwrap();
    let caps = |id: &str| list["data"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id}"))["capabilities"].clone();
    assert_eq!(caps("mock/expand-image")["supports_expand"], true);
    assert_eq!(caps("mock/format-image")["supports_expand"], false);
}
