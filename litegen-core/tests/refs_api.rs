//! Base64 references for URL-only vendors (PixelGenie L4): a caller with no
//! public host sends its image inline; a vendor that refuses `data:` URIs gets
//! it at an unguessable litegen URL that answers for an hour and outlives the
//! request; one that takes `data:` URIs keeps getting them.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

fn png() -> Vec<u8> {
    litegen::providers::image::visual_mock::generate_visual_image_png("a knight")
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

fn with_ref(model: &str, image: &[u8]) -> Value {
    json!({
        "model": model,
        "prompt": "the same knight, at dusk",
        "reference_images": [{ "type": "base64", "value": base64::engine::general_purpose::STANDARD.encode(image), "role": "init" }],
    })
}

#[tokio::test]
async fn a_url_only_vendor_fetches_the_upload_from_an_unguessable_litegen_url() {
    let dir = tempfile::tempdir().unwrap().keep();
    let (app, _db) = harness::app_with(&dir, |cfg| cfg.server.public_base_url = Some("https://litegen.example.com".into())).await;
    let image = png();
    let (status, resp) = post(&app, "/v1/images/generations", with_ref("mock/url-only-image", &image)).await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let url = litegen::providers::image::mock::last_ref_url().expect("the vendor got a URL");
    let token = url.strip_prefix("https://litegen.example.com/v1/refs/").unwrap_or_else(|| panic!("{url}"));
    assert!(token.len() >= 40, "unguessable: {token}");
    let expiry: i64 = token.split('-').next().unwrap().parse().unwrap();
    assert!(expiry <= chrono::Utc::now().timestamp() + 3600, "within the hour");

    // After the request returned, as an async vendor fetches it.
    let resp = app.clone().oneshot(Request::get(format!("/v1/refs/{token}")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "image/png");
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    assert_eq!(bytes.as_ref(), image.as_slice());

    let resp = app.clone().oneshot(Request::get("/v1/refs/1-guessed.png").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn with_no_public_address_a_url_only_vendor_is_refused_before_billing() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await; // http://127.0.0.1:…
    let (status, resp) = post(&app, "/v1/images/generations", with_ref("mock/url-only-image", &png())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp.to_string().contains("public"), "{resp}");
    assert_eq!(db.get_request_logs(1, 10).await.unwrap().0.len(), 0);
}

#[tokio::test]
async fn fal_trellis_still_receives_the_upload_inline_as_a_data_uri() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/fal-ai/trellis"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "request_id": "r-1" })))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap().keep();
    let uri = server.uri();
    let (app, _db) = harness::app_with(&dir, move |cfg| {
        cfg.providers.insert("fal".into(), serde_json::from_value(json!({ "enabled": true, "api_key": "k", "api_base": uri })).unwrap());
    })
    .await;
    let (status, resp) = post(&app, "/v1/models3d/generations", json!({
        "model": "fal/trellis",
        "reference_images": [{ "type": "base64", "value": base64::engine::general_purpose::STANDARD.encode(png()), "role": "init" }],
    })).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let sent: Vec<_> = server.received_requests().await.unwrap().into_iter().filter(|r| r.url.path() == "/fal-ai/trellis").collect();
    let body: Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert!(body["image_url"].as_str().is_some_and(|u| u.starts_with("data:image/png;base64,")), "{body}");
}
