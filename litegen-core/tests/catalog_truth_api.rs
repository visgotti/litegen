//! A truthful catalog (L14): each model says whether it is active, deprecated
//! or retired, when, and what replaces it; the listing reports real reference
//! and duration limits and a `catalog_version` that changes with the catalog.

use axum::body::Body;
use axum::http::Request;
use serde_json::Value;
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

async fn listing(app: &axum::Router) -> Value {
    let resp = app.clone().oneshot(Request::get("/v1/models").body(Body::empty()).unwrap()).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn model<'a>(list: &'a Value, id: &str) -> &'a Value {
    list["data"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id} listed"))
}

#[tokio::test]
async fn the_listing_says_each_models_lifecycle() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let list = listing(&app).await;
    // https://ai.google.dev/gemini-api/docs/deprecations (2026-10-30):
    // "| `gemini-2.5-flash-image` | October 2, 2025 | March 15, 2027 | `gemini-3.1-flash-lite-image` |"
    let old = model(&list, "google/gemini-2.5-flash-image");
    assert_eq!(
        (old["status"].as_str(), old["retires_at"].as_str(), old["replaced_by"].as_str()),
        (Some("deprecated"), Some("2027-03-15"), Some("google/gemini-3.1-flash-lite-image")),
        "{old}"
    );
    // The changelog (Oct 6): "The `gemini-3.1-flash-image` model is deprecated
    // (no shutdown date announced). Migrate to `gemini-nano-banana-2.1`."
    let flash = model(&list, "google/gemini-3.1-flash-image");
    assert_eq!(
        (flash["status"].as_str(), flash["deprecated_at"].as_str(), flash.get("retires_at"), flash["replaced_by"].as_str()),
        (Some("deprecated"), Some("2026-10-06"), None, Some("google/gemini-nano-banana-2.1")),
        "{flash}"
    );
    let current = model(&list, "mock/image-gen");
    assert_eq!((current["status"].as_str(), current.get("retires_at")), (Some("active"), None), "{current}");
}

#[tokio::test]
async fn the_listing_reports_real_limits_and_a_catalog_version() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let list = listing(&app).await;
    let caps = &model(&list, "mock/keyframe-video")["capabilities"];
    assert_eq!(caps["max_duration_seconds"].as_f64(), Some(9.0), "the duration param's max: {caps}");
    assert_eq!((caps["max_reference_images"].as_u64(), caps["max_images"].as_u64()), (Some(10), Some(10)), "{caps}");
    let version = list["catalog_version"].as_str().unwrap_or_default();
    assert!(version.len() >= 16, "{version:?}");
    assert_eq!(listing(&app).await["catalog_version"], list["catalog_version"], "stable while the catalog is");
}
