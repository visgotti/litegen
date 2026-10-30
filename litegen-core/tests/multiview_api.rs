//! Multiview images (PixelGenie 3D doc §3E): an image model whose task is
//! `multiview` turns one init image into one image per requested view, each
//! named and placed by the yaw convention litegen uses everywhere — the
//! camera's position around +Y, measured from the subject's front (+Z) towards
//! +X: front 0, left 90, back 180, right 270.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

async fn post(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    post_to(app, "/v1/images/generations", body).await
}

async fn post_to(app: &axum::Router, at: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(Request::post(at).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn multiview(views: Option<Vec<&str>>) -> Value {
    let png = litegen::providers::image::visual_mock::generate_visual_image_png("a knight");
    let mut body = json!({
        "model": "mock/multiview-image",
        "prompt": "a knight",
        "reference_images": [{ "type": "base64", "value": base64::engine::general_purpose::STANDARD.encode(png), "role": "init" }],
        "response_format": "b64_json",
    });
    if let Some(v) = views {
        body["views"] = json!(v);
    }
    body
}

#[tokio::test]
async fn one_image_comes_back_per_view_named_and_placed_by_yaw() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (status, resp) = post(&app, multiview(Some(vec!["front", "left", "back", "right", "front-left"]))).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let data = resp["data"].as_array().unwrap();
    let placed: Vec<(&str, f64, u64)> = data
        .iter()
        .map(|d| (d["view"].as_str().unwrap(), d["yaw_deg"].as_f64().unwrap(), d["index"].as_u64().unwrap()))
        .collect();
    assert_eq!(placed, [("front", 0.0, 0), ("left", 90.0, 1), ("back", 180.0, 2), ("right", 270.0, 3), ("front-left", 45.0, 4)]);
    let distinct: std::collections::HashSet<&str> = data.iter().map(|d| d["b64_json"].as_str().unwrap()).collect();
    assert_eq!(distinct.len(), 5, "a different image for each view");
}

#[tokio::test]
async fn a_multiview_call_is_billed_once_whatever_the_number_of_views() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (_, four) = post(&app, multiview(Some(vec!["front", "left", "back", "right"]))).await;
    let (_, one) = post(&app, multiview(Some(vec!["front"]))).await;
    assert_eq!(four["usage"]["cost_usd"], one["usage"]["cost_usd"], "{four} {one}");
    assert!((four["usage"]["cost_usd"].as_f64().unwrap() - 0.01).abs() < 1e-9, "the model's one call: {four}");
}

#[tokio::test]
async fn omitted_views_are_the_models_default_set() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (status, resp) = post(&app, multiview(None)).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let views: Vec<&str> = resp["data"].as_array().unwrap().iter().map(|d| d["view"].as_str().unwrap()).collect();
    assert_eq!(views, ["front", "left", "back", "right"]);
}

#[tokio::test]
async fn views_the_model_does_not_make_or_repeat_are_refused() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for (views, code) in [
        (vec!["front", "upside-down"], "param_enum_mismatch"),
        (vec!["front", "front"], "param_enum_mismatch"),
        (vec![], "param_out_of_range"),
    ] {
        let (status, resp) = post(&app, multiview(Some(views.clone()))).await;
        assert_eq!((status, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{views:?}: {resp}");
    }
}

// ─── Multiview-to-3D: `view-<name>` references ──────────────────────────────

/// A distinct image per view, so a view in another's place shows.
fn view_ref(view: &str) -> Value {
    let png = litegen::providers::image::visual_mock::generate_visual_image_png(view);
    json!({ "type": "base64", "value": base64::engine::general_purpose::STANDARD.encode(png), "role": format!("view-{view}") })
}

fn data_uri(view: &str) -> String {
    let png = litegen::providers::image::visual_mock::generate_visual_image_png(view);
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
}

async fn app_with_fal(server: &MockServer) -> (axum::Router, std::sync::Arc<litegen::db::SqliteDatabase>) {
    let dir = tempfile::tempdir().unwrap().keep();
    let uri = server.uri();
    harness::app_with(&dir, move |cfg| {
        cfg.providers.insert("fal".into(), serde_json::from_value(json!({ "enabled": true, "api_key": "k", "api_base": uri })).unwrap());
    })
    .await
}

#[tokio::test]
async fn views_reach_tripo_on_fal_in_tripos_order_whatever_order_they_were_sent_in() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/tripo3d/h3.1/multiview-to-3d"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "request_id": "r-1" })))
        .mount(&server)
        .await;
    let (app, _db) = app_with_fal(&server).await;
    let (status, resp) = post_to(&app, "/v1/models3d/generations", json!({
        "model": "fal/tripo-h3.1-multiview",
        "reference_images": [view_ref("back"), view_ref("left"), view_ref("front")],
    })).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let sent: Vec<_> = server.received_requests().await.unwrap().into_iter().filter(|r| r.url.path() == "/tripo3d/h3.1/multiview-to-3d").collect();
    let body: Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(body["image_urls"], json!([data_uri("front"), data_uri("left"), data_uri("back")]));
}

/// Rodin reads its views unlabelled and in any order: they reach it front
/// first, and it takes at most five references (its `max_total`).
#[tokio::test]
async fn views_reach_rodin_on_fal_front_first_and_at_most_five() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/fal-ai/hyper3d/rodin"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "request_id": "r-1" })))
        .mount(&server)
        .await;
    let (app, _db) = app_with_fal(&server).await;
    let (status, resp) = post_to(&app, "/v1/models3d/generations", json!({
        "model": "fal/hyper3d-rodin",
        "reference_images": [view_ref("right"), view_ref("front"), view_ref("top")],
    })).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let sent: Vec<_> = server.received_requests().await.unwrap().into_iter().filter(|r| r.url.path() == "/fal-ai/hyper3d/rodin").collect();
    let body: Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(body["input_image_urls"], json!([data_uri("front"), data_uri("right"), data_uri("top")]));

    let six: Vec<Value> = ["front", "left", "back", "right", "top", "bottom"].into_iter().map(view_ref).collect();
    let (status, resp) = post_to(&app, "/v1/models3d/generations", json!({ "model": "fal/hyper3d-rodin", "reference_images": six })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    assert_eq!(server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == "/fal-ai/hyper3d/rodin").count(), 1, "six views never reach fal");
}

#[tokio::test]
async fn views_a_multiview_model_cannot_take_are_refused_before_billing() {
    use litegen::db::DatabaseStore;
    let server = MockServer::start().await;
    let (app, db) = app_with_fal(&server).await;
    for (model, refs, code) in [
        // Tripo reads them by position: right without back would be read as back.
        ("fal/tripo-h3.1-multiview", vec![view_ref("front"), view_ref("left"), view_ref("right")], "ref_role_required"),
        ("fal/hunyuan3d-v3.1-pro", vec![view_ref("back"), view_ref("left")], "ref_role_required"),
        ("fal/hunyuan3d-v3.1-pro", vec![json!({ "type": "url", "value": "https://example.com/a.png", "role": "init" }), view_ref("front")], "ref_role_conflict"),
    ] {
        let (status, resp) = post_to(&app, "/v1/models3d/generations", json!({ "model": model, "reference_images": refs })).await;
        assert_eq!((status, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{model}: {resp}");
    }
    assert!(server.received_requests().await.unwrap().is_empty(), "nothing reached fal");
    assert_eq!(db.get_request_logs(1, 10).await.unwrap().0.len(), 0, "nothing billed");
}
