//! Per-box prompts and masks named in text (PixelGenie 3D doc §4, P2):
//! `layout` places objects and words on a text-to-image canvas by boxes in
//! canvas fractions; `mask_prompt` names the area an edit replaces instead of
//! drawing it.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use image::{ImageBuffer, Rgba, RgbaImage};
use serde_json::{json, Value};
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

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

fn picture(resp: &Value) -> RgbaImage {
    let b64 = resp["data"][0]["b64_json"].as_str().unwrap_or_else(|| panic!("no image: {resp}"));
    image::load_from_memory(&base64::engine::general_purpose::STANDARD.decode(b64).unwrap()).unwrap().to_rgba8()
}

fn init_img() -> RgbaImage {
    ImageBuffer::from_fn(40, 20, |x, y| Rgba([(x * 6) as u8, (y * 12) as u8, 9, 255]))
}

fn init() -> Value {
    let mut png = Vec::new();
    init_img().write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    json!([{ "type": "base64", "value": base64::engine::general_purpose::STANDARD.encode(png), "role": "init" }])
}

#[tokio::test]
async fn each_layout_box_is_painted_where_its_fractions_put_it() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (status, resp) = post(&app, json!({
        "model": "mock/layout-image", "prompt": "a shop front", "response_format": "b64_json",
        "layout": [
            { "box": { "x": 0.0, "y": 0.0, "w": 0.5, "h": 0.25 }, "kind": "text", "text": "OPEN" },
            { "box": { "x": 0.5, "y": 0.5, "w": 0.25, "h": 0.5 }, "kind": "object", "prompt": "a cat" },
        ],
    })).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let img = picture(&resp);
    assert_eq!(img.dimensions(), (256, 256));
    // The mock paints box i in palette colour i.
    assert_eq!(img.get_pixel(64, 32).0, [0xe6, 0x19, 0x4b, 0xff], "the first box");
    assert_eq!(img.get_pixel(160, 192).0, [0x3c, 0xb4, 0x4b, 0xff], "the second box");
    assert_ne!(img.get_pixel(32, 200).0, [0xe6, 0x19, 0x4b, 0xff], "outside every box");
}

#[tokio::test]
async fn a_mask_named_in_text_edits_what_it_names() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (status, resp) = post(&app, json!({
        "model": "mock/mask-prompt-image", "prompt": "a red door", "mask_prompt": "the door",
        "reference_images": init(), "response_format": "b64_json",
    })).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let (out, src) = (picture(&resp), init_img());
    // The mock "finds" the middle half of the image.
    for (x, y, p) in out.enumerate_pixels() {
        if (10..30).contains(&x) && (5..15).contains(&y) {
            assert_eq!(p.0, [0xff, 0xcd, 0x75, 0xff], "edited at {x},{y}");
        } else {
            assert_eq!(p, src.get_pixel(x, y), "kept at {x},{y}");
        }
    }
}

#[tokio::test]
async fn layouts_and_text_masks_a_model_cannot_take_are_refused_before_billing() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let item = |b: Value, kind: &str| json!({ "box": b, "kind": kind, "prompt": "x", "text": "x" });
    let unit = json!({ "x": 0.1, "y": 0.1, "w": 0.2, "h": 0.2 });
    for (body, code) in [
        (json!({ "model": "mock/layout-image", "prompt": "x", "layout": [item(json!({ "x": 0.9, "y": 0.0, "w": 0.2, "h": 0.1 }), "object")] }), "param_out_of_range"),
        (json!({ "model": "mock/layout-image", "prompt": "x", "layout": [{ "box": unit.clone(), "kind": "text" }] }), "param_required"),
        (json!({ "model": "mock/layout-image", "prompt": "x", "layout": [] }), "param_out_of_range"),
        (json!({ "model": "mock/format-image", "prompt": "x", "layout": [item(unit.clone(), "object")] }), "param_unsupported"),
        (json!({ "model": "mock/mask-prompt-image", "prompt": "x", "mask_prompt": "the door" }), "init_required"),
        (json!({ "model": "mock/format-image", "prompt": "x", "mask_prompt": "the door" }), "param_unsupported"),
    ] {
        let (s, resp) = post(&app, body.clone()).await;
        assert_eq!((s, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{body}: {resp}");
    }
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed");
}

#[tokio::test]
async fn the_listing_says_which_models_take_a_layout_or_a_text_mask() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/v1/models?media_type=image").body(Body::empty()).unwrap()).await.unwrap();
    let list: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap()).unwrap();
    let caps = |id: &str| list["data"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id}"))["capabilities"].clone();
    let layout = caps("mock/layout-image");
    assert_eq!((layout["supports_layout"].as_bool(), layout["layout_kinds"].clone()), (Some(true), json!(["object", "text"])), "{layout}");
    assert_eq!(caps("mock/mask-prompt-image")["supports_mask_prompt"], true);
    let plain = caps("mock/format-image");
    assert_eq!((plain["supports_layout"].as_bool(), plain["supports_mask_prompt"].as_bool()), (Some(false), Some(false)));
}
