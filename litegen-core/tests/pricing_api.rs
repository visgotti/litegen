//! Prices that match the bill (PixelGenie 3D doc §3G, L14): a row's
//! `variable_pricing` rules price the quote (`/cost`), the quota reservation
//! and the charge alike, so a request away from the default costs what the
//! vendor will bill for it — before, during and after.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

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

/// The `/cost` quote and the generation's charge for one request body.
async fn quote_and_charge(app: &axum::Router, media: &str, body: Value) -> (f64, f64) {
    let (s, quote) = post(app, &format!("/v1/{media}/cost"), body.clone()).await;
    assert_eq!(s, StatusCode::OK, "{quote}");
    let (s, done) = post(app, &format!("/v1/{media}/generations"), body).await;
    assert_eq!(s, StatusCode::OK, "{done}");
    (quote["total_cost_usd"].as_f64().unwrap(), done["usage"]["cost_usd"].as_f64().unwrap())
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[tokio::test]
async fn an_image_is_priced_by_its_megapixels_and_its_quality() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (quote, charge) = quote_and_charge(&app, "images", json!({ "model": "mock/priced-image", "prompt": "x", "size": "2000x1000", "quality": "hd" })).await;
    assert!(close(quote, 0.40) && close(charge, 0.40), "2 MP × $0.10 × 2: quote {quote}, charge {charge}");
    let (quote, charge) = quote_and_charge(&app, "images", json!({ "model": "mock/priced-image", "prompt": "x" })).await;
    assert!(close(quote, 0.10) && close(charge, 0.10), "the default request: quote {quote}, charge {charge}");
}

#[tokio::test]
async fn a_video_is_priced_by_its_seconds_at_its_resolution() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (quote, charge) = quote_and_charge(&app, "videos", json!({ "model": "mock/priced-video", "prompt": "x", "duration_seconds": 8.0, "resolution": "1080p" })).await;
    assert!(close(quote, 1.60) && close(charge, 1.60), "8 s × $0.20: quote {quote}, charge {charge}");
}

#[tokio::test]
async fn a_mesh_is_priced_by_its_multipliers_and_add_ons() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (quote, charge) = quote_and_charge(&app, "models3d", json!({ "model": "mock/priced-3d", "prompt": "x", "texture": true, "pbr": true })).await;
    assert!(close(quote, 0.35) && close(charge, 0.35), "$0.10 × 3 + $0.05: quote {quote}, charge {charge}");
}
