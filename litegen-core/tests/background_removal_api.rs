//! Background removal (PixelGenie L8): matting models sit on the edit path —
//! `POST /v1/images/generations` with only an `init` image — and return an
//! RGBA PNG; the listing says which models do it.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use image::{ImageBuffer, Rgba, RgbaImage};
use serde_json::{json, Value};
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

const MAGENTA: [u8; 4] = [0xff, 0x00, 0xff, 0xff];

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A sprite on PixelGenie's flat #ff00ff: a patterned 20×12 block in the
/// middle of a 40×24 magenta field.
fn sprite() -> RgbaImage {
    ImageBuffer::from_fn(40, 24, |x, y| {
        if (10..30).contains(&x) && (6..18).contains(&y) { Rgba([(x * 9) as u8, (y * 7) as u8, 40, 255]) } else { Rgba(MAGENTA) }
    })
}

fn png(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
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

fn init(img: &RgbaImage) -> Value {
    json!([{ "type": "base64", "value": b64(&png(img)), "role": "init" }])
}

#[tokio::test]
async fn the_flat_background_comes_off_and_the_subject_stays_exactly() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let source = sprite();
    let (status, resp) = post(&app, json!({ "model": "mock/remove-background", "reference_images": init(&source), "response_format": "b64_json" })).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["data"][0]["content_type"], "image/png", "{resp}");
    let bytes = base64::engine::general_purpose::STANDARD.decode(resp["data"][0]["b64_json"].as_str().unwrap()).unwrap();
    let out = image::load_from_memory(&bytes).unwrap();
    assert!(out.color().has_alpha(), "an RGBA PNG");
    let out = out.to_rgba8();
    assert_eq!(out.dimensions(), source.dimensions());
    for (x, y, p) in out.enumerate_pixels() {
        let was = source.get_pixel(x, y);
        if was.0 == MAGENTA {
            assert_eq!(p.0[3], 0, "background at {x},{y}");
        } else {
            assert_eq!(p, was, "subject at {x},{y}");
        }
    }
}

#[tokio::test]
async fn a_background_removal_model_takes_only_an_image_and_says_so_before_billing() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let model = "mock/remove-background";
    for (body, code) in [
        (json!({ "model": model }), "ref_role_required"),
        (json!({ "model": model, "prompt": "keep the hat", "reference_images": init(&sprite()) }), "param_unsupported"),
        (json!({ "model": model, "reference_images": init(&sprite()), "output_format": "jpeg" }), "param_unsupported"),
    ] {
        let (s, resp) = post(&app, body.clone()).await;
        assert_eq!((s, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{body}: {resp}");
    }
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed");
}

#[tokio::test]
async fn the_listing_says_which_models_remove_backgrounds() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/v1/models?media_type=image").body(Body::empty()).unwrap()).await.unwrap();
    let list: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap()).unwrap();
    let caps = |id: &str| list["data"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id}"))["capabilities"].clone();
    let rmbg = caps("mock/remove-background");
    assert_eq!(
        (rmbg["supports_background_removal"].as_bool(), rmbg["supports_text_to_image"].as_bool(), rmbg["task"].as_str()),
        (Some(true), Some(false), Some("generate")),
        "{rmbg}"
    );
    assert_eq!(caps("mock/format-image")["supports_background_removal"].as_bool(), Some(false));
}
