//! Fields a request has no name for (E2): refused with `param_unsupported`
//! when `strict` (the default), and dropped and named in
//! `X-Litegen-Dropped-Params` when `strict: false`, on every generation and
//! quote route, at the top level and inside the request's objects (N6). A
//! field the model passes through (its `extra_allowlist`) is forwarded as if
//! sent in `extra`, which keeps an OpenAI SDK's `user` working. And every
//! 2xx that can drop something says what it dropped (N1). All of these used
//! to be ignored, `strict: true` included.

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use base64::Engine;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn post(app: &axum::Router, at: &str, body: Value) -> (StatusCode, HeaderMap, Value) {
    call(app, Request::post(at).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap()).await
}

/// The dropped-params header. It must be visible ASCII: the MCP note and the
/// SDKs read it with `to_str()`.
fn dropped(headers: &HeaderMap) -> Option<String> {
    headers.get("x-litegen-dropped-params").map(|v| v.to_str().expect("visible ASCII").to_string())
}

/// `(code, param)` of an error answer.
fn refusal(resp: &Value) -> (Option<&str>, Option<&str>) {
    (resp["error"]["code"].as_str(), resp["error"]["param"].as_str())
}

/// Each route, a model it serves, and a field its request has no name for.
fn routes() -> Vec<(&'static str, &'static str, &'static str, Value)> {
    vec![
        ("/v1/images/generations", "mock/image-gen", "colour", json!("red")),
        ("/v1/images/cost", "mock/image-gen", "colour", json!("red")),
        ("/v1/videos/generations", "mock/video-gen", "duration", json!(10)),
        ("/v1/videos/cost", "mock/video-gen", "duration", json!(10)),
        ("/v1/models3d/generations", "mock/mesh-3d", "texture_quality", json!("high")),
        ("/v1/models3d/cost", "mock/mesh-3d", "texture_quality", json!("high")),
    ]
}

#[tokio::test]
async fn an_unknown_field_is_refused_in_strict_mode_on_every_route() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for (at, model, field, value) in routes() {
        let mut body = json!({ "model": model, "prompt": "a red cube" });
        body[field] = value;
        let (s, _, resp) = post(&app, at, body).await;
        assert_eq!((s, refusal(&resp)), (StatusCode::BAD_REQUEST, (Some("param_unsupported"), Some(field))), "{at}: {resp}");
    }
}

#[tokio::test]
async fn an_unknown_field_is_dropped_and_named_in_lax_mode_on_every_route() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for (at, model, field, value) in routes() {
        let mut body = json!({ "model": model, "prompt": "a red cube", "strict": false });
        body[field] = value;
        let (s, h, resp) = post(&app, at, body).await;
        assert_eq!(s, StatusCode::OK, "{at}: {resp}");
        assert_eq!(dropped(&h).as_deref(), Some(field), "{at}: {resp}");
    }
}

/// An image field on a video request is no video field: refused, naming the
/// first in key order and both in the message, which says whose they are.
#[tokio::test]
async fn another_familys_field_is_unknown_here() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let body = json!({ "model": "mock/video-gen", "prompt": "a red cube", "size": "1024x1024", "quality": "hd", "strict": true });
    let (s, _, resp) = post(&app, "/v1/videos/generations", body).await;
    assert_eq!((s, refusal(&resp)), (StatusCode::BAD_REQUEST, (Some("param_unsupported"), Some("quality"))), "{resp}");
    let message = resp["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("`quality`") && message.contains("`size`") && message.contains("an image request field"), "{message}");
}

/// A field sent as `null` asks for nothing, known or not, so a client that
/// writes out its unset optional fields keeps working. (Green before E2 too;
/// it guards the null rule.)
#[tokio::test]
async fn a_field_sent_as_null_asks_for_nothing() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let body = json!({ "model": "mock/video-gen", "prompt": "a red cube", "duration_seconds": null, "aspect_ratio": null, "duration": null });
    let (s, h, resp) = post(&app, "/v1/videos/generations", body).await;
    assert_eq!((s, dropped(&h)), (StatusCode::OK, None), "{resp}");
}

/// A model of another family is the mistake to report, before any field.
/// (Green before E2 too; it guards the order.)
#[tokio::test]
async fn the_wrong_family_is_still_reported_first() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (s, _, resp) = post(&app, "/v1/videos/generations", json!({ "model": "mock/image-gen", "prompt": "a red cube", "colour": "red" })).await;
    assert_eq!((s, refusal(&resp).0), (StatusCode::BAD_REQUEST, Some("model_media_type_mismatch")), "{resp}");
}

#[tokio::test]
async fn a_multipart_request_part_is_checked_too() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let part = json!({ "model": "mock/video-gen", "prompt": "a red cube", "duration": 10 });
    let body = format!(
        "--XBOUNDARY\r\nContent-Disposition: form-data; name=\"request\"\r\nContent-Type: application/json\r\n\r\n{part}\r\n--XBOUNDARY--\r\n"
    );
    let req = Request::post("/v1/videos/generations")
        .header("content-type", "multipart/form-data; boundary=XBOUNDARY")
        .body(Body::from(body))
        .unwrap();
    let (s, _, resp) = call(&app, req).await;
    assert_eq!((s, refusal(&resp)), (StatusCode::BAD_REQUEST, (Some("param_unsupported"), Some("duration"))), "{resp}");
}

/// A 4×4 PNG, base64: a reference image that needs no network.
fn png() -> String {
    let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 30, 30, 255]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    base64::engine::general_purpose::STANDARD.encode(out)
}

/// Unknown keys inside the request's objects (N6) are named by their path.
#[tokio::test]
async fn an_unknown_key_inside_a_requests_object_is_named_by_its_path() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let refs = json!([{ "type": "base64", "value": png(), "role": "first_frame", "weight": 2 }]);
    let (s, _, resp) = post(&app, "/v1/videos/generations", json!({ "model": "mock/video-gen", "prompt": "a red cube", "reference_images": refs })).await;
    assert_eq!((s, refusal(&resp)), (StatusCode::BAD_REQUEST, (Some("param_unsupported"), Some("reference_images[0].weight"))), "{resp}");
    let lax = json!({ "model": "mock/video-gen", "prompt": "a red cube", "reference_images": refs, "strict": false });
    let (s, h, resp) = post(&app, "/v1/videos/cost", lax).await;
    assert_eq!((s, dropped(&h).as_deref()), (StatusCode::OK, Some("reference_images[0].weight")), "{resp}");
}

/// A quote drops what the generation would, and says so (N1): the three
/// `/cost` routes never sent the header.
#[tokio::test]
async fn the_cost_routes_report_what_they_dropped() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for (at, model, param, value) in [
        ("/v1/videos/cost", "mock/priced-video", "fps", json!(24)),
        ("/v1/images/cost", "mock/image-gen", "guidance_scale", json!(7.5)),
        ("/v1/models3d/cost", "mock/mesh-3d", "rig", json!(true)),
    ] {
        let mut body = json!({ "model": model, "prompt": "a red cube", "strict": false });
        body[param] = value;
        let (s, h, resp) = post(&app, at, body).await;
        assert_eq!(s, StatusCode::OK, "{at}: {resp}");
        assert_eq!(dropped(&h).as_deref(), Some(param), "{at}: {resp}");
    }
}

/// An image answered 202 (`Prefer: respond-async`) says what it dropped,
/// as its 200 does (N1).
#[tokio::test]
async fn an_image_answered_202_reports_what_it_dropped() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let body = json!({ "model": "mock/priced-image", "prompt": "a lighthouse job_ms=150", "strict": false, "style": "noir" });
    let req = Request::post("/v1/images/generations")
        .header("content-type", "application/json")
        .header("prefer", "respond-async")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (s, h, resp) = call(&app, req).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{resp}");
    assert_eq!(dropped(&h).as_deref(), Some("style"), "{resp}");
}

/// The `curl -X POST http://localhost:4000/v1/…` examples of the README with
/// a JSON body (`-d '…'`): `(path, body)`.
fn readme_examples() -> Vec<(String, Value)> {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../README.md")).expect("README.md");
    let mut out = Vec::new();
    for (at, _) in readme.match_indices("curl -X POST http://localhost:4000/v1/") {
        let rest = &readme[at + "curl -X POST http://localhost:4000".len()..];
        let route = rest.split_whitespace().next().unwrap_or_default().to_string();
        let Some(start) = rest.find("-d '") else { continue };
        let body = &rest[start + 4..];
        let body = &body[..body.find('\'').expect("a closing quote")];
        let json = serde_json::from_str(body).unwrap_or_else(|e| panic!("README body for {route} is no JSON ({e}): {body}"));
        out.push((route, json));
    }
    out
}

/// The README's generation examples are requests litegen takes (N4). Each is
/// quoted on its family's `/cost` route, which checks a body as the
/// generation does and calls no vendor. The first example used to send
/// gpt-image-2 `quality: hd`, a value it does not take (400
/// `param_enum_mismatch`). Segmentation, which has no quote route and whose
/// example carries a placeholder image, is left out.
#[tokio::test]
async fn the_readmes_example_requests_are_valid() {
    let dir = tempfile::tempdir().unwrap().keep();
    let (app, _db) = harness::app_with(&dir, |cfg| {
        for vendor in ["openai", "runway"] {
            cfg.providers.insert(vendor.to_string(), serde_json::from_value(json!({ "enabled": true, "api_key": "not-a-real-key" })).unwrap());
        }
    })
    .await;
    let examples: Vec<(String, Value)> = readme_examples().into_iter().filter(|(route, _)| route != "/v1/images/segmentations").collect();
    assert!(examples.len() >= 4, "the image, video and 3D examples: {examples:?}");
    for (route, body) in examples {
        let quote = route.replace("/generations", "/cost");
        let (s, h, resp) = post(&app, &quote, body.clone()).await;
        assert_eq!((s, dropped(&h)), (StatusCode::OK, None), "README example for {route} ({quote}): {body}: {resp}");
    }
}

/// Passthrough review #6: a field the model passes through, sent at the top
/// level with a value its type refuses, is refused under the name the caller
/// sent (`moderation`), in both modes. It was named `extra.moderation`, a
/// field the caller never sent. Sent inside `extra`, it is `extra.moderation`.
#[tokio::test]
async fn a_top_level_field_is_refused_under_its_own_name() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for strict in [true, false] {
        let body = json!({ "model": "openai/gpt-image-2", "prompt": "a red cube", "moderation": "strict", "strict": strict });
        let (s, _, resp) = post(&app, "/v1/images/cost", body).await;
        assert_eq!((s, refusal(&resp)), (StatusCode::BAD_REQUEST, (Some("param_enum_mismatch"), Some("moderation"))), "strict={strict}: {resp}");
        assert!(!resp["error"]["message"].as_str().unwrap_or_default().contains("extra.moderation"), "strict={strict}: {resp}");
    }
    let body = json!({ "model": "openai/gpt-image-2", "prompt": "a red cube", "extra": { "moderation": "strict" } });
    let (s, _, resp) = post(&app, "/v1/images/cost", body).await;
    assert_eq!((s, refusal(&resp)), (StatusCode::BAD_REQUEST, (Some("param_enum_mismatch"), Some("extra.moderation"))), "{resp}");
}

/// The OpenAI SDK sends `user` and `moderation` at the top level. On a row
/// that passes them through (`extra_allowlist`), they reach the vendor as if
/// sent in `extra`, under the default `strict`, instead of being dropped
/// unseen (E2 ruling a).
#[tokio::test]
async fn a_field_the_model_passes_through_reaches_the_vendor_from_the_top_level() {
    let server = MockServer::start().await;
    let image = base64::engine::general_purpose::STANDARD.encode(b"fake-png-bytes");
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "created": 1, "data": [{ "b64_json": image }] })))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap().keep();
    let base = format!("{}/v1/images/generations", server.uri());
    let (app, _db) = harness::app_with(&dir, move |cfg| {
        cfg.providers.insert(
            "openai".to_string(),
            serde_json::from_value(json!({ "enabled": true, "api_key": "k", "api_base": base })).unwrap(),
        );
    })
    .await;
    let body = json!({ "model": "openai/gpt-image-2", "prompt": "a red cube", "response_format": "b64_json", "user": "u1", "moderation": "low" });
    let (s, h, resp) = post(&app, "/v1/images/generations", body).await;
    assert_eq!((s, dropped(&h)), (StatusCode::OK, None), "{resp}");
    let sent: Vec<Value> = server.received_requests().await.unwrap().iter().map(|r| serde_json::from_slice(&r.body).unwrap()).collect();
    assert_eq!(sent.len(), 1, "one vendor call");
    assert_eq!((sent[0]["user"].as_str(), sent[0]["moderation"].as_str()), (Some("u1"), Some("low")), "{}", sent[0]);
}
