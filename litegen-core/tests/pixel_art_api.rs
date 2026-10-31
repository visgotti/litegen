//! Native pixel art (PixelGenie L16): models that draw at native size, a
//! `palette` they keep to, and `directions` returned one labelled image each.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
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

fn pixels(result: &Value) -> image::RgbaImage {
    let b64 = result["b64_json"].as_str().unwrap_or_else(|| panic!("no image: {result}"));
    image::load_from_memory(&base64::engine::general_purpose::STANDARD.decode(b64).unwrap()).unwrap().to_rgba8()
}

/// A 16-colour palette, as `#rrggbb`.
fn sixteen() -> Vec<String> {
    (0..16).map(|i| format!("#{:02x}{:02x}{:02x}", i * 16, 255 - i * 16, (i * 37) % 256)).collect()
}

fn hex(p: &image::Rgba<u8>) -> String {
    format!("#{:02x}{:02x}{:02x}", p[0], p[1], p[2])
}

#[tokio::test]
async fn a_forced_palette_comes_back_at_native_size_in_only_its_colours() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let palette = sixteen();
    let (s, resp) = post(&app, json!({
        "model": "mock/pixel-art-image", "prompt": "a knight", "size": "32x48", "palette": palette, "response_format": "b64_json",
    })).await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    let img = pixels(&resp["data"][0]);
    assert_eq!(img.dimensions(), (32, 48), "native size, never upscaled");
    assert!(img.pixels().all(|p| palette.contains(&hex(p))), "only the palette's colours");
}

#[tokio::test]
async fn eight_directions_come_back_as_eight_labelled_images() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (s, resp) = post(&app, json!({ "model": "mock/pixel-art-image", "prompt": "a knight", "directions": 8, "response_format": "b64_json" })).await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    let data = resp["data"].as_array().unwrap();
    let labels: Vec<&str> = data.iter().map(|d| d["direction"].as_str().unwrap_or("?")).collect();
    // PixelLab's own order: clockwise from facing the viewer.
    assert_eq!(labels, ["south", "south-east", "east", "north-east", "north", "north-west", "west", "south-west"], "{resp}");
    assert!((resp["usage"]["cost_usd"].as_f64().unwrap() - 0.01).abs() < 1e-9, "one call, billed once: {}", resp["usage"]);
    let (_, four) = post(&app, json!({ "model": "mock/pixel-art-image", "prompt": "a knight", "directions": 4 })).await;
    let labels: Vec<&str> = four["data"].as_array().unwrap().iter().map(|d| d["direction"].as_str().unwrap()).collect();
    assert_eq!(labels, ["south", "east", "north", "west"]);
}

#[tokio::test]
async fn a_palette_or_direction_count_the_model_cannot_take_is_refused_before_billing() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let many: Vec<String> = (0..65).map(|i| format!("#{:06x}", i)).collect();
    for (body, code) in [
        (json!({ "model": "mock/pixel-art-image", "prompt": "x", "palette": ["#12345"] }), "param_out_of_range"),
        (json!({ "model": "mock/pixel-art-image", "prompt": "x", "palette": many }), "param_too_many"),
        (json!({ "model": "mock/pixel-art-image", "prompt": "x", "directions": 6 }), "param_enum_mismatch"),
        (json!({ "model": "mock/format-image", "prompt": "x", "directions": 8 }), "param_unsupported"),
        (json!({ "model": "mock/format-image", "prompt": "x", "palette": ["#ffffff"] }), "param_unsupported"),
    ] {
        let (s, resp) = post(&app, body.clone()).await;
        assert_eq!((s, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{body}: {resp}");
    }
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed");
}

#[tokio::test]
async fn the_listing_says_which_models_draw_native_pixel_art() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/v1/models?media_type=image").body(Body::empty()).unwrap()).await.unwrap();
    let list: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap()).unwrap();
    let caps = |id: &str| list["data"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id}"))["capabilities"].clone();
    let px = caps("mock/pixel-art-image");
    assert_eq!(
        (px["native_pixel_art"].as_bool(), px["pixel_size"].clone(), px["supports_forced_palette"].as_bool(), px["directions"].clone()),
        (Some(true), json!({ "min": 16, "max": 128 }), Some(true), json!([4, 8])),
        "{px}"
    );
    let plain = caps("mock/format-image");
    assert_eq!((plain["native_pixel_art"].as_bool(), plain["supports_palette"].as_bool(), plain["directions"].clone()), (Some(false), Some(false), json!([])));
}
