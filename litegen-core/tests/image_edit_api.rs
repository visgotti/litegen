//! Partial image edits (PixelGenie 3D doc §4): one white-edits mask, a
//! `region` box and `mask_grow_px`, checked against the init image and turned
//! into each model's own form before anything is billed.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use image::{ImageBuffer, Rgba, RgbaImage};
use serde_json::{json, Value};
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

const FILL: [u8; 4] = [0xff, 0xcd, 0x75, 0xff];

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn png(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

/// A patterned init image: every pixel distinct enough to catch a change.
fn init_img(w: u32, h: u32) -> RgbaImage {
    ImageBuffer::from_fn(w, h, |x, y| Rgba([(x * 3 % 256) as u8, (y * 5 % 256) as u8, ((x + y) % 256) as u8, 255]))
}

/// An opaque white-edits mask: white inside `(x, y, w, h)`.
fn mask_img(w: u32, h: u32, r: (u32, u32, u32, u32)) -> RgbaImage {
    ImageBuffer::from_fn(w, h, |x, y| {
        if x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3 { Rgba([255, 255, 255, 255]) } else { Rgba([0, 0, 0, 255]) }
    })
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

fn edit(model: &str, init: &RgbaImage, mask: Option<&RgbaImage>, extra: Value) -> Value {
    let mut refs = vec![json!({ "type": "base64", "value": b64(&png(init)), "role": "init" })];
    if let Some(m) = mask {
        refs.push(json!({ "type": "base64", "value": b64(&png(m)), "role": "mask" }));
    }
    let mut body = json!({ "model": model, "prompt": "make it glow", "reference_images": refs, "response_format": "b64_json" });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    body
}

fn result_image(resp: &Value) -> RgbaImage {
    let data = resp["data"][0]["b64_json"].as_str().unwrap_or_else(|| panic!("no image: {resp}"));
    image::load_from_memory(&base64::engine::general_purpose::STANDARD.decode(data).unwrap()).unwrap().to_rgba8()
}

/// Every pixel inside `inside` is the fill; every other pixel is the init's.
fn assert_filled_exactly(out: &RgbaImage, init: &RgbaImage, inside: impl Fn(u32, u32) -> bool) {
    assert_eq!(out.dimensions(), init.dimensions());
    for (x, y, p) in out.enumerate_pixels() {
        if inside(x, y) {
            assert_eq!(p.0, FILL, "edited at {x},{y}");
        } else {
            assert_eq!(p, init.get_pixel(x, y), "kept at {x},{y}");
        }
    }
}

#[tokio::test]
async fn a_bad_edit_is_refused_before_anything_is_billed() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let init = init_img(64, 48);
    let mut translucent = mask_img(64, 48, (0, 0, 8, 8));
    translucent.put_pixel(63, 47, Rgba([0, 0, 0, 200]));
    let cases = [
        (json!({ "model": "mock/inpainting-image", "prompt": "x", "region": { "x": 0, "y": 0, "w": 4, "h": 4 } }), 400, "init_required"),
        (edit("mock/inpainting-image", &init, Some(&mask_img(32, 48, (0, 0, 4, 4))), json!({})), 400, "mask_size_mismatch"),
        (edit("mock/inpainting-image", &init, Some(&translucent), json!({})), 400, "mask_ambiguous"),
        (edit("mock/inpainting-image", &init, None, json!({ "region": { "x": 60, "y": 0, "w": 8, "h": 8 } })), 400, "region_out_of_bounds"),
        (edit("mock/region-image", &init_img(200, 200), None, json!({ "region": { "x": 10, "y": 10, "w": 20, "h": 20 } })), 422, "region_too_small"),
    ];
    for (body, status, code) in cases {
        let (s, resp) = post(&app, body).await;
        assert_eq!((s.as_u16(), resp["error"]["code"].as_str()), (status, Some(code)), "{resp}");
    }
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed");
    assert_eq!(db.count_generations(None).await.unwrap(), 0);
}

#[tokio::test]
async fn the_inpainting_mock_fills_exactly_the_masked_area() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let init = init_img(64, 48);
    let (s, resp) = post(&app, edit("mock/inpainting-image", &init, Some(&mask_img(64, 48, (10, 5, 20, 7))), json!({}))).await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    assert_filled_exactly(&result_image(&resp), &init, |x, y| (10..30).contains(&x) && (5..12).contains(&y));
}

#[tokio::test]
async fn mask_grow_px_grows_the_edit_area() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let init = init_img(64, 48);
    let (s, resp) = post(&app, edit("mock/inpainting-image", &init, Some(&mask_img(64, 48, (10, 10, 4, 4))), json!({ "mask_grow_px": 2 }))).await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    assert_filled_exactly(&result_image(&resp), &init, |x, y| (8..16).contains(&x) && (8..16).contains(&y));
}

#[tokio::test]
async fn a_region_on_a_mask_model_arrives_drawn_as_its_mask_and_overlaps_a_mask() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let init = init_img(64, 48);
    let (_, resp) = post(&app, edit("mock/inpainting-image", &init, None, json!({ "region": { "x": 3, "y": 4, "w": 5, "h": 6 } }))).await;
    assert_filled_exactly(&result_image(&resp), &init, |x, y| (3..8).contains(&x) && (4..10).contains(&y));
    let (_, resp) = post(&app, edit("mock/inpainting-image", &init, Some(&mask_img(64, 48, (0, 0, 6, 48))), json!({ "region": { "x": 3, "y": 4, "w": 5, "h": 6 } }))).await;
    assert_filled_exactly(&result_image(&resp), &init, |x, y| (3..6).contains(&x) && (4..10).contains(&y));
}

#[tokio::test]
async fn a_small_region_on_a_box_model_gets_its_minimum_window_and_its_exact_area() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let init = init_img(1024, 768);
    let (s, resp) = post(&app, edit("mock/region-image", &init, None, json!({ "region": { "x": 384, "y": 256, "w": 128, "h": 64 } }))).await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    assert_eq!(
        litegen::providers::image::mock::last_region_window(),
        Some(litegen::types::Region { x: 320, y: 160, w: 256, h: 256 }),
        "the 256 px window, centred on the region"
    );
    assert_filled_exactly(&result_image(&resp), &init, |x, y| (384..512).contains(&x) && (256..320).contains(&y));
}

#[tokio::test]
async fn the_listing_says_how_each_model_edits_part_of_an_image() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/v1/models?media_type=image").body(Body::empty()).unwrap()).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    let list: Value = serde_json::from_slice(&bytes).unwrap();
    let caps = |id: &str| -> Value {
        list["data"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id} listed"))["capabilities"].clone()
    };
    let region = caps("mock/region-image");
    assert_eq!((region["supports_region_edit"].as_bool(), region["region_input"].as_str()), (Some(true), Some("box")), "{region}");
    assert_eq!((region["preserves_unmasked"].as_str(), region["mask_convention"].as_str()), (Some("exact"), Some("white_edits")), "{region}");
    let inpaint = caps("mock/inpainting-image");
    assert_eq!((inpaint["supports_inpainting"].as_bool(), inpaint["region_input"].as_str()), (Some(true), Some("mask")), "{inpaint}");
    // A caller always sends white-edits; OpenAI's own convention stays in the catalog row.
    let plain = caps("mock/image-gen");
    assert_eq!((plain["supports_region_edit"].as_bool(), plain.get("mask_convention")), (Some(false), None), "{plain}");
    // §2 rule 5: `mask_grow_px` is served as a flag wherever it is taken.
    assert_eq!(
        (region["supports_mask_grow"].as_bool(), inpaint["supports_mask_grow"].as_bool(), plain["supports_mask_grow"].as_bool()),
        (Some(true), Some(true), Some(false)),
    );
}
