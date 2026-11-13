//! Output format and transparency as real fields (PixelGenie L9): JPEG
//! artifacts break palette snapping and a real alpha channel saves keying out
//! the background, so `output_format` and `background` are validated request
//! fields, the listing says what each model can return, and `content_type` is
//! read from the bytes, never taken from the adapter's label.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use serde_json::{json, Value};
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn generate(app: &axum::Router, extra: Value) -> (StatusCode, Value) {
    let mut body = json!({ "model": "mock/format-image", "prompt": "a sprite", "response_format": "b64_json" });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    call(app, Request::post("/v1/images/generations").header("content-type", "application/json").body(Body::from(body.to_string())).unwrap()).await
}

fn image_bytes(resp: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(resp["data"][0]["b64_json"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn the_asked_format_comes_back_labelled_by_its_bytes() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for (format, magic, content_type) in [("jpeg", &b"\xff\xd8\xff"[..], "image/jpeg"), ("webp", &b"RIFF"[..], "image/webp"), ("png", &b"\x89PNG"[..], "image/png")] {
        let (status, resp) = generate(&app, json!({ "output_format": format })).await;
        assert_eq!(status, StatusCode::OK, "{resp}");
        assert!(image_bytes(&resp).starts_with(magic), "{format}");
        // The mock labels everything image/png: the label comes from the bytes.
        assert_eq!(resp["data"][0]["content_type"], content_type, "{format}");
    }
}

#[tokio::test]
async fn a_transparent_background_has_real_alpha() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (status, resp) = generate(&app, json!({ "output_format": "png", "background": "transparent" })).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let img = image::load_from_memory(&image_bytes(&resp)).unwrap().to_rgba8();
    assert_eq!(img.get_pixel(0, 0)[3], 0, "the corner is transparent");
}

#[tokio::test]
async fn formats_and_backgrounds_a_model_cannot_make_are_refused() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for (extra, code) in [
        (json!({ "output_format": "gif" }), "param_enum_mismatch"),
        (json!({ "output_format": "jpeg", "background": "transparent" }), "param_conflict"),
    ] {
        let (status, resp) = generate(&app, extra.clone()).await;
        assert_eq!((status, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{extra}: {resp}");
    }
    let body = json!({ "model": "mock/image-gen", "prompt": "x", "background": "transparent" });
    let (status, resp) = call(&app, Request::post("/v1/images/generations").header("content-type", "application/json").body(Body::from(body.to_string())).unwrap()).await;
    assert_eq!((status, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("param_unsupported")), "undeclared, strict: {resp}");
}

#[tokio::test]
async fn the_listing_says_what_each_model_can_return() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (_, list) = call(&app, Request::get("/v1/models?media_type=image").body(Body::empty()).unwrap()).await;
    let caps = |id: &str| list["data"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id}"))["capabilities"].clone();
    let f = caps("mock/format-image");
    assert_eq!((f["output_formats"].clone(), f["supports_transparent_background"].as_bool(), f["lossy_only"].as_bool()), (json!(["png", "webp", "jpeg"]), Some(true), Some(false)), "{f}");
    // Luma's ImageFormat is "jpg | png" (docs.lumalabs.ai/reference/generateimage.md,
    // 2026-11-03): PixelGenie L9's "Luma always returns JPEG" no longer holds.
    let luma = caps("luma/photon-1");
    assert_eq!((luma["output_formats"].clone(), luma["lossy_only"].as_bool()), (json!(["jpeg", "png"]), Some(false)), "{luma}");
}
