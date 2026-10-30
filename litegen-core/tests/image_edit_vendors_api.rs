//! Partial image edits reaching real vendors' wire formats (PixelGenie 3D doc
//! §4): one white-edits mask, converted to each vendor's own convention, and a
//! region sent as each vendor's own box. The app runs with OpenAI, Stability
//! and Ideogram pointed at a local stand-in that records what arrives.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use image::{ImageBuffer, Rgba, RgbaImage};
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

fn png(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn init_png(w: u32, h: u32) -> Vec<u8> {
    png(&ImageBuffer::from_fn(w, h, |x, y| Rgba([(x % 256) as u8, (y % 256) as u8, 9, 255])))
}

/// A white-edits mask: white inside `(x, y, w, h)`.
fn mask_png(w: u32, h: u32, r: (u32, u32, u32, u32)) -> Vec<u8> {
    png(&ImageBuffer::from_fn(w, h, |x, y| {
        if x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3 { Rgba([255, 255, 255, 255]) } else { Rgba([0, 0, 0, 255]) }
    }))
}

/// The bytes of one part of a multipart body.
fn part(body: &[u8], name: &str) -> Option<Vec<u8>> {
    let marker = format!("name=\"{name}\"");
    let start = body.windows(marker.len()).position(|w| w == marker.as_bytes())?;
    let rest = &body[start..];
    let data_at = rest.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let data = &rest[data_at..];
    let end = data.windows(4).position(|w| w == b"\r\n--")?;
    Some(data[..end].to_vec())
}

fn text_part(body: &[u8], name: &str) -> Option<String> {
    part(body, name).map(|b| String::from_utf8_lossy(&b).into_owned())
}

async fn app_with_vendors(server: &MockServer) -> axum::Router {
    let dir = tempfile::tempdir().unwrap().keep();
    let uri = server.uri();
    let (app, _db) = harness::app_with(&dir, move |cfg| {
        for (name, base) in [
            ("openai", format!("{uri}/v1/images/generations")),
            ("stability", uri.clone()),
            ("ideogram", uri.clone()),
        ] {
            cfg.providers.insert(
                name.to_string(),
                serde_json::from_value(json!({ "enabled": true, "api_key": "k", "api_base": base })).unwrap(),
            );
        }
    })
    .await;
    app
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

fn edit_body(model: &str, init: &[u8], mask: Option<&[u8]>, extra: Value) -> Value {
    let mut refs = vec![json!({ "type": "base64", "value": b64(init), "role": "init" })];
    if let Some(m) = mask {
        refs.push(json!({ "type": "base64", "value": b64(m), "role": "mask" }));
    }
    let mut body = json!({ "model": model, "prompt": "make the visor glow cyan", "reference_images": refs, "response_format": "b64_json" });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    body
}

async fn sent_to(server: &MockServer, at: &str) -> Vec<u8> {
    let reqs: Vec<_> = server.received_requests().await.unwrap().into_iter().filter(|r| r.url.path() == at).collect();
    assert_eq!(reqs.len(), 1, "one call to {at}");
    reqs[0].body.clone()
}

#[tokio::test]
async fn openai_receives_the_mask_transparent_where_the_caller_painted_white() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/edits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "b64_json": b64(&init_png(32, 32)) }] })))
        .mount(&server)
        .await;
    let app = app_with_vendors(&server).await;
    let (s, resp) = post(&app, edit_body("openai/gpt-image-2", &init_png(32, 32), Some(&mask_png(32, 32, (4, 4, 8, 8))), json!({}))).await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    let body = sent_to(&server, "/v1/images/edits").await;
    let mask = image::load_from_memory(&part(&body, "mask").expect("a mask part")).unwrap().to_rgba8();
    assert_eq!(mask.dimensions(), (32, 32));
    for (x, y, p) in mask.enumerate_pixels() {
        let inside = (4..12).contains(&x) && (4..12).contains(&y);
        assert_eq!(p[3], if inside { 0 } else { 255 }, "alpha at {x},{y}");
    }
}

#[tokio::test]
async fn stability_receives_the_white_mask_as_sent_and_an_explicit_grow_mask() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2beta/stable-image/edit/inpaint"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(init_png(32, 32)).insert_header("content-type", "image/png"))
        .mount(&server)
        .await;
    let app = app_with_vendors(&server).await;
    let sent_mask = mask_png(32, 32, (4, 4, 8, 8));
    let (s, resp) = post(&app, edit_body("stability/inpaint", &init_png(32, 32), Some(&sent_mask), json!({}))).await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    let body = sent_to(&server, "/v2beta/stable-image/edit/inpaint").await;
    assert_eq!(text_part(&body, "grow_mask").as_deref(), Some("0"), "explicit: Stability's default is 5");
    let mask = image::load_from_memory(&part(&body, "mask").unwrap()).unwrap().to_luma8();
    for (x, y, p) in mask.enumerate_pixels() {
        let inside = (4..12).contains(&x) && (4..12).contains(&y);
        assert_eq!(p[0] >= 128, inside, "white edits at {x},{y}");
    }
    assert!(part(&body, "image").is_some());
    assert_eq!(text_part(&body, "prompt").as_deref(), Some("make the visor glow cyan"));
}

#[tokio::test]
async fn stability_grows_its_own_mask_when_asked() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2beta/stable-image/edit/inpaint"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(init_png(32, 32)).insert_header("content-type", "image/png"))
        .mount(&server)
        .await;
    let app = app_with_vendors(&server).await;
    let (s, _) = post(&app, edit_body("stability/inpaint", &init_png(32, 32), Some(&mask_png(32, 32, (4, 4, 8, 8))), json!({ "mask_grow_px": 6 }))).await;
    assert_eq!(s, StatusCode::OK);
    let body = sent_to(&server, "/v2beta/stable-image/edit/inpaint").await;
    assert_eq!(text_part(&body, "grow_mask").as_deref(), Some("6"));
    let mask = image::load_from_memory(&part(&body, "mask").unwrap()).unwrap().to_luma8();
    assert!(mask.get_pixel(2, 4)[0] < 128, "litegen did not grow it too");
}

async fn mount_ideogram(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v2/image/precise-edit/ideogram-4-5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "generation_kind": "sampling",
            "generation_id": "g-1",
            "data": [{ "url": format!("{}/out.png", server.uri()), "seed": 7, "is_image_safe": true, "resolution": "1024x768" }],
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/out.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(init_png(1024, 768)).insert_header("content-type", "image/png"))
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_region_reaches_ideogram_as_its_exact_context_window() {
    let server = MockServer::start().await;
    mount_ideogram(&server).await;
    let app = app_with_vendors(&server).await;
    let (s, resp) = post(&app, edit_body("ideogram/ideogram-4-5-precise-edit", &init_png(1024, 768), None, json!({ "region": { "x": 384, "y": 256, "w": 300, "h": 260 } }))).await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    let body = sent_to(&server, "/v2/image/precise-edit/ideogram-4-5").await;
    assert_eq!(text_part(&body, "context_window").as_deref(), Some("256,384,516,684"), "y_min,x_min,y_max,x_max, ends exclusive");
    assert!(part(&body, "mask").is_none(), "the box says it all");
    assert_eq!(text_part(&body, "prompt").as_deref(), Some("make the visor glow cyan"));
}

#[tokio::test]
async fn a_small_region_reaches_ideogram_as_a_256_window_and_its_exact_area_in_black() {
    let server = MockServer::start().await;
    mount_ideogram(&server).await;
    let app = app_with_vendors(&server).await;
    let (s, resp) = post(&app, edit_body("ideogram/ideogram-4-5-precise-edit", &init_png(1024, 768), None, json!({ "region": { "x": 384, "y": 256, "w": 128, "h": 64 } }))).await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    let body = sent_to(&server, "/v2/image/precise-edit/ideogram-4-5").await;
    assert_eq!(text_part(&body, "context_window").as_deref(), Some("160,320,416,576"));
    let mask = image::load_from_memory(&part(&body, "mask").expect("the exact area")).unwrap().to_luma8();
    for (x, y) in [(384, 256), (511, 319)] {
        assert!(mask.get_pixel(x, y)[0] < 128, "black edits at {x},{y}");
    }
    for (x, y) in [(383, 256), (512, 300), (400, 320)] {
        assert!(mask.get_pixel(x, y)[0] >= 128, "white keeps at {x},{y}");
    }
}
