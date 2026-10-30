//! Opt-in pixel preservation and background swap (PixelGenie 3D doc §4, P2).
//! `preserve_unmasked: true` pastes the init's pixels back outside the edit
//! area — byte-identical from any vendor — with a hard edge or `feather_px`
//! inside it; a `background_swap` model puts the init's subject on a new
//! background.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use image::{ImageBuffer, Rgba, RgbaImage};
use serde_json::{json, Value};
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

const FILL: [u8; 4] = [0xff, 0xcd, 0x75, 0xff];

fn b64(img: &RgbaImage) -> String {
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    base64::engine::general_purpose::STANDARD.encode(png)
}

fn init_img() -> RgbaImage {
    ImageBuffer::from_fn(48, 32, |x, y| Rgba([(x * 5) as u8, (y * 7) as u8, 33, 255]))
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

fn picture(resp: &Value) -> RgbaImage {
    let b64 = resp["data"][0]["b64_json"].as_str().unwrap_or_else(|| panic!("no image: {resp}"));
    image::load_from_memory(&base64::engine::general_purpose::STANDARD.decode(b64).unwrap()).unwrap().to_rgba8()
}

fn edit(extra: Value) -> Value {
    let mut body = json!({
        "model": "mock/sloppy-edit-image", "prompt": "a lamp", "response_format": "b64_json",
        "reference_images": [{ "type": "base64", "value": b64(&init_img()), "role": "init" }],
        "region": { "x": 10, "y": 8, "w": 20, "h": 12 },
    });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    body
}

#[tokio::test]
async fn preserved_pixels_come_back_byte_identical_from_a_sloppy_vendor() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let init = init_img();
    let inside = |x: u32, y: u32| (10..30).contains(&x) && (8..20).contains(&y);

    // Without it, the vendor's "similar" pixels come back.
    let (s, resp) = post(&app, edit(json!({}))).await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    let sloppy = picture(&resp);
    assert_ne!(sloppy.get_pixel(0, 0), init.get_pixel(0, 0), "the mock alters pixels outside the edit");

    let (s, resp) = post(&app, edit(json!({ "preserve_unmasked": true }))).await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    let kept = picture(&resp);
    for (x, y, p) in kept.enumerate_pixels() {
        if inside(x, y) {
            assert_eq!(p.0, FILL, "the edit at {x},{y}");
        } else {
            assert_eq!(p, init.get_pixel(x, y), "kept exactly at {x},{y}");
        }
    }

    // A feather blends from the init at the edge to the edit 3 px in.
    let (s, resp) = post(&app, edit(json!({ "preserve_unmasked": true, "feather_px": 3 }))).await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    let soft = picture(&resp);
    assert_eq!(soft.get_pixel(0, 0), init.get_pixel(0, 0), "outside is still exact");
    assert_eq!(soft.get_pixel(20, 14).0, FILL, "the middle is the edit");
    let edge = soft.get_pixel(10, 14);
    assert!(edge != init.get_pixel(10, 14) && edge.0 != FILL, "the edge blends: {edge:?}");
}

#[tokio::test]
async fn preservation_the_request_cannot_honour_is_refused_before_billing() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let no_area = {
        let mut b = edit(json!({ "preserve_unmasked": true }));
        b.as_object_mut().unwrap().remove("region");
        b
    };
    for (body, code) in [
        (no_area, "param_conflict"),
        (edit(json!({ "preserve_unmasked": true, "size": "96x64" })), "param_conflict"),
        (edit(json!({ "preserve_unmasked": true, "output_format": "jpeg" })), "param_conflict"),
        (edit(json!({ "feather_px": 4 })), "param_conflict"),
    ] {
        let (s, resp) = post(&app, body).await;
        assert_eq!((s, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{resp}");
    }
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed");
}

#[tokio::test]
async fn a_background_swap_model_puts_the_subject_on_a_new_background() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let sprite: RgbaImage = ImageBuffer::from_fn(30, 20, |x, y| {
        if (10..20).contains(&x) && (5..15).contains(&y) { Rgba([(x * 9) as u8, 200, (y * 9) as u8, 255]) } else { Rgba([0xff, 0, 0xff, 0xff]) }
    });
    let (s, resp) = post(&app, json!({
        "model": "mock/background-swap-image", "prompt": "a clear sky", "response_format": "b64_json",
        "reference_images": [{ "type": "base64", "value": b64(&sprite), "role": "init" }],
    })).await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    let out = picture(&resp);
    for (x, y, p) in out.enumerate_pixels() {
        let was = sprite.get_pixel(x, y);
        if was.0 == [0xff, 0, 0xff, 0xff] {
            assert_eq!(p.0, [0x87, 0xce, 0xeb, 0xff], "new background at {x},{y}");
        } else {
            assert_eq!(p, was, "subject at {x},{y}");
        }
    }

    let resp = app.clone().oneshot(Request::get("/v1/models?media_type=image").body(Body::empty()).unwrap()).await.unwrap();
    let list: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap()).unwrap();
    let caps = |id: &str| list["data"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id}"))["capabilities"].clone();
    assert_eq!(caps("mock/background-swap-image")["task"], "background_swap");
    assert_eq!(caps("mock/sloppy-edit-image")["supports_preserve_unmasked"], true);
    assert_eq!(caps("mock/format-image")["supports_preserve_unmasked"], false);
}
