//! Promptable segmentation (PixelGenie L7): `POST /v1/images/segmentations`
//! takes an image and point, box or text prompts and answers at once with
//! masks — white where selected — each with the vendor's score and the box
//! litegen measured around it. A request the model cannot serve is refused
//! before anything is billed.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use image::{GrayImage, ImageBuffer, Rgba, RgbaImage};
use serde_json::{json, Value};
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A 128×96 patterned PNG.
fn photo() -> String {
    let img: RgbaImage = ImageBuffer::from_fn(128, 96, |x, y| Rgba([(x * 2) as u8, (y * 2) as u8, 90, 255]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    b64(&out)
}

async fn post(app: &axum::Router, at: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(Request::post(at).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn segment(app: &axum::Router, prompts: Value) -> Value {
    let (status, resp) = post(app, "/v1/images/segmentations", json!({ "model": "mock/segment-image", "image": photo(), "prompts": prompts })).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    resp
}

/// A mask's pixels: the PNG decodes to the image's size, black or white only.
fn mask(m: &Value) -> GrayImage {
    let png = base64::engine::general_purpose::STANDARD.decode(m["png_base64"].as_str().unwrap_or_else(|| panic!("no mask: {m}"))).unwrap();
    assert!(png.starts_with(b"\x89PNG"), "a PNG");
    let img = image::load_from_memory(&png).unwrap().to_luma8();
    assert_eq!(img.dimensions(), (128, 96), "the image's size");
    img
}

/// Exactly the pixels `inside` says are white.
fn assert_selects(m: &Value, inside: impl Fn(u32, u32) -> bool) {
    for (x, y, p) in mask(m).enumerate_pixels() {
        assert_eq!(p.0[0], if inside(x, y) { 255 } else { 0 }, "at {x},{y}");
    }
}

#[tokio::test]
async fn a_box_selects_exactly_the_box() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = segment(&app, json!([{ "type": "box", "x": 10, "y": 20, "w": 30, "h": 40 }])).await;
    let masks = resp["masks"].as_array().unwrap();
    assert_eq!(masks.len(), 1, "{resp}");
    assert_selects(&masks[0], |x, y| (10..40).contains(&x) && (20..60).contains(&y));
    assert_eq!(masks[0]["box"], json!({ "x": 10, "y": 20, "w": 30, "h": 40 }));
    assert!(masks[0]["score"].is_number(), "{resp}");
    assert_eq!((resp["model"].as_str(), resp["provider"].as_str()), (Some("mock/segment-image"), Some("mock")));
    assert!(resp["id"].as_str().unwrap().starts_with("litegen-seg-"), "{resp}");
}

#[tokio::test]
async fn points_together_select_one_object_a_square_around_each() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = segment(&app, json!([{ "type": "point", "x": 64, "y": 48, "label": 1 }])).await;
    let masks = resp["masks"].as_array().unwrap();
    assert_eq!(masks.len(), 1, "{resp}");
    assert_selects(&masks[0], |x, y| (48..80).contains(&x) && (32..64).contains(&y));
    assert_eq!(masks[0]["box"], json!({ "x": 48, "y": 32, "w": 32, "h": 32 }));

    // A label-0 point takes its square away from the same object; a point
    // with no label is a label-1 point.
    let resp = segment(&app, json!([{ "type": "point", "x": 40, "y": 40 }, { "type": "point", "x": 56, "y": 40, "label": 0 }])).await;
    let masks = resp["masks"].as_array().unwrap();
    assert_eq!(masks.len(), 1, "{resp}");
    assert_selects(&masks[0], |x, y| (24..40).contains(&x) && (24..56).contains(&y));
    assert_eq!(masks[0]["box"], json!({ "x": 24, "y": 24, "w": 16, "h": 32 }));
}

#[tokio::test]
async fn a_text_prompt_finds_every_instance_best_first() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = segment(&app, json!([{ "type": "text", "text": "visor" }])).await;
    let masks = resp["masks"].as_array().unwrap();
    assert_eq!(masks.len(), 2, "the mock finds two: {resp}");
    assert_eq!(masks[0]["box"], json!({ "x": 16, "y": 24, "w": 32, "h": 48 }));
    assert_eq!(masks[1]["box"], json!({ "x": 80, "y": 24, "w": 32, "h": 48 }));
    assert!(masks[0]["score"].as_f64().unwrap() >= masks[1]["score"].as_f64().unwrap(), "{resp}");
    assert_selects(&masks[1], |x, y| (80..112).contains(&x) && (24..72).contains(&y));
}

#[tokio::test]
async fn a_segmentation_is_charged_the_rows_price() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = segment(&app, json!([{ "type": "box", "x": 0, "y": 0, "w": 8, "h": 8 }])).await;
    assert!((resp["usage"]["cost_usd"].as_f64().unwrap() - 0.005).abs() < 1e-9, "{resp}");
}

#[tokio::test]
async fn a_request_the_model_cannot_serve_is_refused_before_anything_is_billed() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let at = "/v1/images/segmentations";
    let model = "mock/segment-image";
    let cases = [
        (json!({ "model": model, "image": photo(), "prompts": [] }), "param_required"),
        (json!({ "model": model, "image": photo(), "prompts": [{ "type": "text", "text": "visor" }, { "type": "point", "x": 1, "y": 1 }] }), "param_conflict"),
        (json!({ "model": model, "image": photo(), "prompts": [{ "type": "text", "text": "a" }, { "type": "text", "text": "b" }] }), "param_out_of_range"),
        (json!({ "model": model, "image": photo(), "prompts": [{ "type": "point", "x": 1, "y": 1, "label": 2 }] }), "param_out_of_range"),
        (json!({ "model": model, "image": photo(), "prompts": [{ "type": "box", "x": 1, "y": 1, "w": 0, "h": 5 }] }), "param_out_of_range"),
        (json!({ "model": model, "image": photo(), "prompts": [{ "type": "point", "x": 1, "y": 1, "label": 0 }] }), "param_conflict"),
        (json!({ "model": model, "image": photo(), "prompts": [{ "type": "box", "x": 100, "y": 10, "w": 40, "h": 10 }] }), "prompt_out_of_bounds"),
        (json!({ "model": model, "image": photo(), "prompts": [{ "type": "point", "x": 128, "y": 10 }] }), "prompt_out_of_bounds"),
        (json!({ "model": model, "image": b64(b"not an image"), "prompts": [{ "type": "point", "x": 1, "y": 1 }] }), "image_unreadable"),
        (json!({ "model": model, "image": "%%%", "prompts": [{ "type": "point", "x": 1, "y": 1 }] }), "image_unreadable"),
        (json!({ "model": "mock/format-image", "image": photo(), "prompts": [{ "type": "point", "x": 1, "y": 1 }] }), "model_task_mismatch"),
        (json!({ "model": "mock/video-gen", "image": photo(), "prompts": [{ "type": "point", "x": 1, "y": 1 }] }), "model_media_type_mismatch"),
    ];
    for (body, code) in cases {
        let (s, resp) = post(&app, at, body.clone()).await;
        assert_eq!((s, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{}: {resp}", body["prompts"]);
    }
    let (s, resp) = post(&app, at, json!({ "model": "mock/nope", "image": photo(), "prompts": [{ "type": "point", "x": 1, "y": 1 }] })).await;
    assert_eq!((s, resp["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("model_not_found")), "{resp}");
    // A segmentation model makes no images.
    let (s, resp) = post(&app, "/v1/images/generations", json!({ "model": model, "prompt": "x" })).await;
    assert_eq!((s, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("model_task_mismatch")), "{resp}");
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed");
}

/// A field the request has no name for (E2, N3): `prompt` for `prompts` is
/// refused when `strict` (the default), naming the field it is short for, as
/// on every other generation route; with `strict: false` it is dropped and
/// named in `X-Litegen-Dropped-Params`. Segmentation used to ignore both the
/// field and `strict`.
#[tokio::test]
async fn an_unknown_segmentation_field_is_refused_or_named() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let body = |strict: Option<bool>| {
        let mut b = json!({ "model": "mock/segment-image", "image": photo(), "prompts": [{ "type": "box", "x": 10, "y": 20, "w": 30, "h": 40 }], "prompt": "cat" });
        if let Some(s) = strict {
            b["strict"] = json!(s);
        }
        b
    };
    let (s, resp) = post(&app, "/v1/images/segmentations", body(None)).await;
    assert_eq!(
        (s, resp["error"]["code"].as_str(), resp["error"]["param"].as_str()),
        (StatusCode::BAD_REQUEST, Some("param_unsupported"), Some("prompt")),
        "{resp}"
    );
    assert!(resp["error"]["message"].as_str().is_some_and(|m| m.contains("`prompts`")), "the field it is short for: {resp}");
    let resp = app
        .clone()
        .oneshot(Request::post("/v1/images/segmentations").header("content-type", "application/json").body(Body::from(body(Some(false)).to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let named = resp.headers().get("x-litegen-dropped-params").map(|v| v.to_str().expect("visible ASCII").to_string());
    assert_eq!(named.as_deref(), Some("prompt"));
}

#[tokio::test]
async fn the_listing_says_which_models_segment_and_with_which_prompts() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/v1/models?media_type=image").body(Body::empty()).unwrap()).await.unwrap();
    let list: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap()).unwrap();
    let caps = |id: &str| list["data"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id}"))["capabilities"].clone();
    let seg = caps("mock/segment-image");
    assert_eq!(
        (seg["supports_segmentation"].as_bool(), seg["segmentation_prompts"].clone(), seg["task"].as_str()),
        (Some(true), json!(["point", "box", "text"]), Some("segment")),
        "{seg}"
    );
    let plain = caps("mock/format-image");
    assert_eq!((plain["supports_segmentation"].as_bool(), plain["segmentation_prompts"].clone()), (Some(false), json!([])), "{plain}");
}
