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

/// An app with `vendors` configured (a placeholder key: a quote calls no
/// vendor), so their rows can be quoted.
async fn quoting_app(vendors: &[&str]) -> axum::Router {
    let dir = tempfile::tempdir().unwrap().keep();
    let vendors: Vec<String> = vendors.iter().map(|v| v.to_string()).collect();
    harness::app_with(&dir, move |cfg| {
        for v in &vendors {
            cfg.providers.insert(v.clone(), serde_json::from_value(json!({ "enabled": true, "api_key": "k" })).unwrap());
        }
    })
    .await
    .0
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

/// OpenAI prices an image by its output tokens: by quality ("high" is 2× the
/// medium image the row's base is) and by size past 1024². The quote — and so
/// the reservation and the charge — say so (it used to bill every request at
/// the 1024² medium price).
/// @see <https://developers.openai.com/api/docs/guides/image-generation> (Calculating costs)
#[tokio::test]
async fn an_openai_image_is_quoted_by_its_quality_and_its_size() {
    let app = quoting_app(&["openai"]).await;
    let quote = |body: Value| {
        let app = app.clone();
        async move {
            let (s, q) = post(&app, "/v1/images/cost", body).await;
            assert_eq!(s, StatusCode::OK, "{q}");
            q["total_cost_usd"].as_f64().unwrap()
        }
    };
    let medium = quote(json!({ "model": "openai/gpt-image-2", "prompt": "x", "size": "1024x1024", "quality": "medium" })).await;
    assert!(close(medium, 0.053), "{medium}");
    let high = quote(json!({ "model": "openai/gpt-image-2", "prompt": "x", "size": "1024x1024", "quality": "high" })).await;
    assert!(close(high, 0.106), "{high}");
    let big = quote(json!({ "model": "openai/gpt-image-2", "prompt": "x", "size": "2048x2048", "quality": "medium" })).await;
    assert!(close(big, 0.212), "{big}");
    let max = quote(json!({ "model": "openai/gpt-image-2.5-sunburst", "prompt": "x", "size": "1024x1024", "quality": "max" })).await;
    assert!(close(max, 0.0132 * 16.0), "{max}");
}

/// PixVerse Multi-Transition prices fixed lengths ("1–10, 15, 20, 25 and 30
/// s"): a clip of an unlisted length bills the next listed tier up — never
/// the 5 s base — and a keyframe past the longest priced length is refused
/// before anything is billed.
/// @see <https://docs.platform.pixverse.ai/pixverse-api-pricing-2394995m0.md> (2026-10-30)
#[tokio::test]
async fn a_multi_transition_clip_of_an_unlisted_length_bills_the_next_tier_up() {
    let app = quoting_app(&["pixverse"]).await;
    let keyframes = |last: f64| json!([
        { "type": "url", "value": "https://x/a.png", "role": "keyframe", "time_s": 0.0 },
        { "type": "url", "value": "https://x/b.png", "role": "keyframe", "time_s": last },
    ]);
    let body = |last: f64| json!({ "model": "pixverse/v5-multi-transition", "prompt": "x", "resolution": "1080p", "reference_images": keyframes(last) });
    let (s, q) = post(&app, "/v1/videos/cost", body(29.0)).await;
    assert_eq!(s, StatusCode::OK, "{q}");
    assert!(close(q["total_cost_usd"].as_f64().unwrap(), 3.2), "29 s at 1080p bills the 30 s tier: {q}");
    let (s, q) = post(&app, "/v1/videos/cost", body(12.0)).await;
    assert_eq!(s, StatusCode::OK, "{q}");
    assert!(close(q["total_cost_usd"].as_f64().unwrap(), 1.6), "12 s bills the 15 s tier: {q}");
    let (s, q) = post(&app, "/v1/videos/cost", body(31.0)).await;
    assert_eq!((s, q["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("keyframe_time_out_of_range")), "{q}");
}
