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

/// A clip of no length, or less, is refused (E1): it used to be quoted,
/// reserved and charged at $0 while the vendor made, and billed, its default
/// clip. Only the quote route is called for the vendor row, so no vendor is
/// reached.
#[tokio::test]
async fn a_clip_of_no_length_is_refused_not_quoted_or_charged_at_nothing() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for d in [json!(0), json!(-3)] {
        for at in ["/v1/videos/cost", "/v1/videos/generations"] {
            let (s, resp) = post(&app, at, json!({ "model": "mock/priced-video", "prompt": "x", "duration_seconds": d })).await;
            assert_eq!(
                (s, resp["error"]["code"].as_str(), resp["error"]["param"].as_str()),
                (StatusCode::BAD_REQUEST, Some("param_out_of_range"), Some("duration_seconds")),
                "{at} with duration_seconds {d}: {resp}"
            );
        }
    }
    let app = quoting_app(&["vidu"]).await;
    let (s, resp) = post(&app, "/v1/videos/cost", json!({ "model": "vidu/viduq3-turbo", "prompt": "x", "duration_seconds": 0 })).await;
    assert_eq!(
        (s, resp["error"]["code"].as_str(), resp["error"]["param"].as_str()),
        (StatusCode::BAD_REQUEST, Some("param_out_of_range"), Some("duration_seconds")),
        "vidu/viduq3-turbo quoted a 0 s clip: {resp}"
    );
}

/// A ratio the frame image overrides is refused before it is quoted (E5): the
/// adapter used to leave it out of the image-to-video body, so a 9:16 request
/// with a frame was quoted, made in the frame's shape and billed, with
/// nothing refused or reported. Only `/cost` is called, so no vendor is
/// reached.
#[tokio::test]
async fn a_ratio_an_image_overrides_is_refused_before_it_is_quoted() {
    let app = quoting_app(&["kling", "vidu", "pixverse", "tokenhub", "runway", "bytedance"]).await;
    let image = |role: &str| json!({ "type": "url", "value": "https://example.com/a.png", "role": role });
    let body = |model: &str, refs: Value| json!({ "model": model, "prompt": "x", "aspect_ratio": "9:16", "reference_images": refs });
    for (model, refs) in [
        ("kling/video-kling-v4", json!([image("first_frame")])),
        ("vidu/viduq3-turbo", json!([image("first_frame")])),
        ("pixverse/v6", json!([image("first_frame")])),
        ("tokenhub/hy-video-v1.5", json!([image("first_frame")])),
        ("runway/kling_4_flash", json!([image("first_frame")])),
        ("runway/kling_4_flash", json!([image("first_frame"), image("reference")])),
    ] {
        let (s, resp) = post(&app, "/v1/videos/cost", body(model, refs.clone())).await;
        assert_eq!(
            (s, resp["error"]["code"].as_str(), resp["error"]["param"].as_str()),
            (StatusCode::BAD_REQUEST, Some("param_unsupported"), Some("aspect_ratio")),
            "{model} with 9:16 and {refs}: {resp}"
        );
    }
    // Guards: the ratio without a frame, Vidu's reference mode (it takes a
    // ratio; a frame beside the reference is refused, see
    // `a_frame_with_a_reference_is_refused_before_it_is_quoted_or_reserved`),
    // and Seedance, which honours a ratio on a first frame.
    for (model, refs) in [
        ("kling/video-kling-v4", json!([])),
        ("vidu/viduq3-turbo", json!([])),
        ("pixverse/v6", json!([])),
        ("tokenhub/hy-video-v1.5", json!([])),
        ("runway/kling_4_flash", json!([])),
        ("vidu/viduq1", json!([image("reference")])),
        ("bytedance/doubao-seedance-1-0-pro-250528", json!([image("first_frame")])),
    ] {
        let (s, resp) = post(&app, "/v1/videos/cost", body(model, refs.clone())).await;
        assert_eq!(s, StatusCode::OK, "{model} with 9:16 and {refs}: {resp}");
    }
}

/// N3: a video model whose text-to-video mode needs the prompt (its image
/// mode does not, so the row's prompt is optional) refuses a request with no
/// first frame and no prompt at validation, before it is quoted, reserved or
/// sent. `runway/veo3.1` with nothing in it used to be quoted at $3.20, and a
/// generation reserved, before its adapter refused it.
#[tokio::test]
async fn a_text_to_video_request_with_no_prompt_is_refused_before_it_is_quoted_or_reserved() {
    use litegen::db::DatabaseStore;
    let dir = tempfile::tempdir().unwrap().keep();
    let (app, db) = harness::app_with(&dir, |cfg| {
        for v in ["runway", "tokenhub"] {
            cfg.providers.insert(v.to_string(), serde_json::from_value(json!({ "enabled": true, "api_key": "k" })).unwrap());
        }
    })
    .await;
    let image = |role: &str| json!({ "type": "url", "value": "https://example.com/a.png", "role": role });
    // The rows whose vendor refuses text-to-video without a prompt: Runway's
    // `t2v_needs_prompt` table, TokenHub's "文生视频必选".
    for model in [
        "runway/veo3.1", "runway/veo3.1_fast", "runway/gemini_omni_flash", "runway/kling_4", "runway/kling_4_flash",
        "runway/seedance2", "runway/seedance2_fast", "runway/seedance2_mini", "runway/grok_imagine_1_5",
        "runway/grok_imagine_1_5_lite", "runway/happyhorse_1_0", "tokenhub/hy-video-v1.5", "tokenhub/minimax-video-v2.3",
    ] {
        for at in ["/v1/videos/cost", "/v1/videos/generations"] {
            let (s, resp) = post(&app, at, json!({ "model": model })).await;
            assert_eq!(
                (s, resp["error"]["code"].as_str(), resp["error"]["param"].as_str()),
                (StatusCode::BAD_REQUEST, Some("prompt_required"), Some("prompt")),
                "{model} at {at}: {resp}"
            );
        }
    }
    // A reference image is no frame: Runway still makes text-to-video of it.
    let (s, resp) = post(&app, "/v1/videos/cost", json!({ "model": "runway/kling_4", "reference_images": [image("reference")] })).await;
    assert_eq!((s, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("prompt_required")), "{resp}");
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed");
    assert_eq!(db.count_generations(None).await.unwrap(), 0, "nothing reserved or stored");
    // Guards: a first frame needs no prompt, a prompt needs no image, and a
    // model whose text mode takes no prompt (Kling 3.0 on Runway: its shots
    // can go in `providerSettings.multiPrompt`) is left to its vendor.
    for body in [
        json!({ "model": "runway/veo3.1", "reference_images": [image("first_frame")] }),
        json!({ "model": "runway/veo3.1", "prompt": "a lighthouse at dusk" }),
        json!({ "model": "tokenhub/hy-video-v1.5", "reference_images": [image("first_frame")] }),
        json!({ "model": "runway/kling3.0_standard" }),
    ] {
        let (s, resp) = post(&app, "/v1/videos/cost", body.clone()).await;
        assert_eq!(s, StatusCode::OK, "{body}: {resp}");
    }
}

/// Whether each vendor that takes a `last_frame` ends a clip on it only after
/// a first frame, with what says so. A vendor not listed fails the test: a
/// new row with a `last_frame` role needs this decided.
const LAST_FRAME_NEEDS_A_FIRST: &[(&str, bool, &str)] = &[
    ("bytedance", true, "bytedance.rs: \"needs a first_frame to go with a last_frame\" (\"Two image_url objects must be provided\")"),
    ("fal", true, "fal.rs `route`: \"a last frame needs a first frame\""),
    ("ideogram", true, "ideogram.rs: \"a last frame needs a first frame (`image` is required)\""),
    ("kling", true, "kling.rs: \"last-frame-only video is not supported\""),
    ("pixverse", true, "pixverse.rs: \"a last_frame needs a first_frame\" (transition needs first_frame_img and last_frame_img)"),
    ("runway", true, "runway.rs `resold_request`: \"a last frame needs a first frame\" (Veo: \"does not support generating with only a last frame\")"),
    ("tokenhub", true, "tokenhub.rs, every body: \"a last_frame needs a first_frame\""),
    ("vidu", true, "vidu.rs: \"needs a first_frame to go with a last_frame\" (img2video takes its image as the first frame)"),
    ("bedrock", false, "Ray 2 on Bedrock: \"start (frame0) and/or end (frame1) keyframes\""),
    ("luma", false, "Luma's keyframes: frame0 and frame1, each optional"),
    ("minimax", false, "MiniMax first-last-frame: \"required: - model - last_frame_image\"; V2 (H3): \"Image-to-video, last frame: `text` + 1 `image_url` (`role=last_frame`).\""),
    ("mock", false, "the mock adapter makes either"),
];

/// FU-LASTFRAME (dashboard-server review #5): a model whose vendor ends a
/// clip on a last frame only after a first refuses a last frame sent alone
/// at validation, with `ref_role_required`, before it is quoted, reserved or
/// sent. It used to be quoted and reserved, then refused by the adapter; and
/// without a prompt it was refused for the prompt, which adding one did not
/// fix. Every catalog row with a `last_frame` role is checked.
#[tokio::test]
async fn a_last_frame_without_a_first_is_refused_before_it_is_quoted_or_reserved() {
    use base64::Engine;
    use litegen::db::DatabaseStore;
    let configured = ["bytedance", "fal", "ideogram", "kling", "pixverse", "runway", "tokenhub", "vidu", "luma", "minimax"];
    let dir = tempfile::tempdir().unwrap().keep();
    let (app, db) = harness::app_with(&dir, |cfg| {
        for v in configured {
            cfg.providers.insert(v.to_string(), serde_json::from_value(json!({ "enabled": true, "api_key": "k" })).unwrap());
        }
    })
    .await;
    let png = base64::engine::general_purpose::STANDARD.encode(litegen::providers::image::visual_mock::generate_visual_image_png("the end"));
    let frame = |role: &str| json!({ "type": "base64", "value": png, "role": role });
    let mut models_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    models_dir.pop();
    models_dir.push("models");
    let catalog = litegen::capabilities::CapabilityRegistry::from_dir(&models_dir).expect("models/");
    let mut rows: Vec<(String, bool)> = catalog
        .all()
        .filter(|m| m.ref_inputs.as_ref().is_some_and(|r| r.roles.contains_key("last_frame")))
        .map(|m| {
            let needs = LAST_FRAME_NEEDS_A_FIRST
                .iter()
                .find(|(vendor, ..)| *vendor == m.provider)
                .unwrap_or_else(|| panic!("{} takes a last_frame: say whether {} needs a first frame with it", m.id, m.provider));
            (m.id.clone(), needs.1)
        })
        .collect();
    rows.sort();
    let mut wrong = Vec::new();
    for (model, needs) in &rows {
        let body = json!({ "model": model, "prompt": "a lighthouse at dusk", "reference_images": [frame("last_frame")] });
        let ats: &[&str] = if *needs { &["/v1/videos/cost", "/v1/videos/generations"] } else { &["/v1/videos/cost"] };
        for at in ats {
            let (s, resp) = post(&app, at, body.clone()).await;
            let got = (s, resp["error"]["code"].as_str().map(str::to_string), resp["error"]["param"].as_str().map(str::to_string));
            let refused = got == (StatusCode::BAD_REQUEST, Some("ref_role_required".into()), Some("reference_images".into()));
            if refused != *needs {
                wrong.push(format!("{model} at {at}: {} {resp}", s.as_u16()));
            }
        }
    }
    // The last frame is the objection, not the prompt: a row whose text mode
    // needs one (N3) says so for a last frame sent alone without one.
    let (s, resp) = post(&app, "/v1/videos/cost", json!({ "model": "runway/veo3.1", "reference_images": [frame("last_frame")] })).await;
    if resp["error"]["code"] != "ref_role_required" {
        wrong.push(format!("runway/veo3.1 with no prompt at /v1/videos/cost: {} {resp}", s.as_u16()));
    }
    assert!(wrong.is_empty(), "{} of {} rows answered otherwise:\n{}", wrong.len(), rows.len(), wrong.join("\n"));
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed or logged as a failed generation");
    assert_eq!(db.count_generations(None).await.unwrap(), 0, "nothing reserved or stored");
    // Guards: a first and a last frame are quoted; a vendor that ends a clip
    // on a last frame alone keeps it (above, every Luma, MiniMax and mock row).
    let (s, resp) = post(&app, "/v1/videos/cost", json!({ "model": "runway/veo3.1", "prompt": "x", "reference_images": [frame("first_frame"), frame("last_frame")] })).await;
    assert_eq!(s, StatusCode::OK, "a first and a last frame: {resp}");
}

/// catalog-rules: a model whose vendor takes reference images only on
/// image-to-video (its `reference` role's `needs_first_frame`: Runway's
/// Kling O3) refuses a reference sent without a first frame at validation,
/// with `ref_role_required`, before it is quoted, reserved or sent. It used
/// to be quoted and reserved, then refused at submit (runway.rs
/// `resold_request`: "reference images go with a start frame"). Every
/// catalog video row with a `reference` role is checked: one without the
/// rule still quotes a reference alone, and one with it quotes a first
/// frame with a reference.
#[tokio::test]
async fn a_reference_that_needs_a_first_frame_is_refused_without_one_before_it_is_quoted_or_reserved() {
    use base64::Engine;
    use litegen::db::DatabaseStore;
    let mut models_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    models_dir.pop();
    models_dir.push("models");
    let catalog = litegen::capabilities::CapabilityRegistry::from_dir(&models_dir).expect("models/");
    let mut rows: Vec<(String, String, bool)> = catalog
        .all()
        .filter(|m| m.media_type == litegen::capabilities::MediaType::Video)
        .filter_map(|m| {
            let reference = m.ref_inputs.as_ref()?.roles.get("reference")?;
            Some((m.id.clone(), m.provider.clone(), reference.needs_first_frame))
        })
        .collect();
    rows.sort();
    let vendors: std::collections::BTreeSet<String> = rows.iter().map(|(_, vendor, _)| vendor.clone()).collect();
    let dir = tempfile::tempdir().unwrap().keep();
    let (app, db) = harness::app_with(&dir, move |cfg| {
        for v in &vendors {
            cfg.providers.insert(v.clone(), serde_json::from_value(json!({ "enabled": true, "api_key": "k" })).unwrap());
        }
    })
    .await;
    let png = base64::engine::general_purpose::STANDARD.encode(litegen::providers::image::visual_mock::generate_visual_image_png("a reference"));
    let image = |role: &str| json!({ "type": "base64", "value": png, "role": role });
    let refused = |s: StatusCode, resp: &Value| {
        s == StatusCode::BAD_REQUEST && resp["error"]["code"] == "ref_role_required" && resp["error"]["param"] == "reference_images"
    };
    let mut wrong = Vec::new();
    for (model, _, needs) in &rows {
        let alone = json!({ "model": model, "prompt": "a lighthouse at dusk", "reference_images": [image("reference")] });
        let (s, resp) = post(&app, "/v1/videos/cost", alone.clone()).await;
        if refused(s, &resp) != *needs {
            wrong.push(format!("{model}: a reference alone at /v1/videos/cost: {} {resp}", s.as_u16()));
            continue;
        }
        if !*needs {
            continue;
        }
        let (s, resp) = post(&app, "/v1/videos/generations", alone).await;
        if !refused(s, &resp) {
            wrong.push(format!("{model}: a reference alone at /v1/videos/generations: {} {resp}", s.as_u16()));
        }
        let framed = json!({ "model": model, "prompt": "a lighthouse at dusk", "reference_images": [image("first_frame"), image("reference")] });
        let (s, resp) = post(&app, "/v1/videos/cost", framed).await;
        if s != StatusCode::OK {
            wrong.push(format!("{model}: a first frame and a reference at /v1/videos/cost: {} {resp}", s.as_u16()));
        }
    }
    assert!(rows.iter().any(|(.., needs)| *needs), "no catalog row's reference role needs a first frame: {rows:?}");
    assert!(wrong.is_empty(), "{} answers of {} rows were otherwise:\n{}", wrong.len(), rows.len(), wrong.join("\n"));
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed or logged as a failed generation");
    assert_eq!(db.count_generations(None).await.unwrap(), 0, "nothing reserved or stored");
}

/// round 3: a request the vendor refuses for a rule across two of its fields,
/// which the catalog can't say, is refused at the quote in the adapter's own
/// words (its `preflight`), and the generation refuses it the same way before
/// anything is reserved. TokenHub's Kling 2.6 makes native audio only at
/// 1080p ("audio=native 时仅支持 1080p"): `audio: true` at the row's default
/// 720p used to be quoted and reserved, then refused at submit. At 1080p it
/// is quoted.
#[tokio::test]
async fn a_request_the_vendor_refuses_across_two_fields_is_refused_at_the_quote_in_its_words() {
    use litegen::db::DatabaseStore;
    let dir = tempfile::tempdir().unwrap().keep();
    let (app, db) = harness::app_with(&dir, |cfg| {
        cfg.providers.insert("tokenhub".into(), serde_json::from_value(json!({ "enabled": true, "api_key": "k" })).unwrap());
    })
    .await;
    let body = json!({ "model": "tokenhub/kling-video-v2.6", "prompt": "a lighthouse at dusk", "audio": true });
    for at in ["/v1/videos/cost", "/v1/videos/generations"] {
        let (s, resp) = post(&app, at, body.clone()).await;
        assert_eq!((s, resp["error"]["code"].as_str(), resp["error"]["retryable"].as_bool()), (StatusCode::BAD_REQUEST, Some("request_unsupported"), Some(false)), "at {at}: {resp}");
        assert_eq!(resp["error"]["message"], "tokenhub/kling-video-v2.6: native audio needs resolution 1080p, and no last frame", "at {at}");
    }
    let (s, resp) = post(&app, "/v1/videos/cost", json!({ "model": "tokenhub/kling-video-v2.6", "prompt": "x", "audio": true, "resolution": "1080p" })).await;
    assert_eq!(s, StatusCode::OK, "native audio at 1080p: {resp}");
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed or logged as a failed generation");
    assert_eq!(db.count_generations(None).await.unwrap(), 0, "nothing reserved or stored");
}

/// round 3: the direct adapters' own two-field rules are refused at the
/// quote as well, in their words, and the generation refuses them before it
/// reserves: Kling 2.6's native audio only at 1080p, a loop without a last
/// frame on Luma and on Ray 2 through Bedrock ("Loop is not supported for
/// keyframes"), PixVerse v3.5 never 8 s at 1080p, Hailuo 02 only 6 s at
/// 1080P. Each used to be quoted and reserved, then refused at submit.
#[tokio::test]
async fn a_direct_vendors_two_field_rule_is_refused_at_the_quote_in_its_words() {
    use base64::Engine;
    use litegen::db::DatabaseStore;
    let dir = tempfile::tempdir().unwrap().keep();
    let (app, db) = harness::app_with(&dir, |cfg| {
        for v in ["kling", "luma", "pixverse", "minimax", "bedrock"] {
            cfg.providers.insert(v.to_string(), serde_json::from_value(json!({ "enabled": true, "api_key": "k" })).unwrap());
        }
    })
    .await;
    let png = base64::engine::general_purpose::STANDARD.encode(litegen::providers::image::visual_mock::generate_visual_image_png("a frame"));
    let image = |role: &str| json!({ "type": "base64", "value": png, "role": role });
    let cases = [
        (
            json!({ "model": "kling/video-kling-v2-6", "prompt": "a lighthouse at dusk", "audio": true }),
            "kling/video-kling-v2-6: native audio needs resolution 1080p, and no last frame",
        ),
        (
            json!({ "model": "luma/ray-2", "prompt": "a lighthouse at dusk", "loop": true, "reference_images": [image("first_frame"), image("last_frame")] }),
            "luma/ray-2: a loop cannot have a last frame",
        ),
        (
            json!({ "model": "pixverse/v3.5", "prompt": "a lighthouse at dusk", "duration_seconds": 8, "resolution": "1080p" }),
            "pixverse/v3.5: 8 s is not a length it makes at 1080p (5 s is)",
        ),
        (
            json!({ "model": "minimax/MiniMax-Hailuo-02", "prompt": "a lighthouse at dusk", "duration_seconds": 10, "resolution": "1080P" }),
            "minimax/MiniMax-Hailuo-02 makes 6 or 10 s, and only 6 s at 1080P; asked for 10 s",
        ),
        (
            json!({ "model": "bedrock/luma.ray-v2:0", "prompt": "a lighthouse at dusk", "loop": true, "reference_images": [image("first_frame"), image("last_frame")] }),
            "bedrock Ray 2: a loop can't end on a last frame (\"Loop is not supported for keyframes\")",
        ),
    ];
    let mut wrong = Vec::new();
    for (body, words) in &cases {
        for at in ["/v1/videos/cost", "/v1/videos/generations"] {
            let (s, resp) = post(&app, at, body.clone()).await;
            if (s, resp["error"]["code"].as_str(), resp["error"]["message"].as_str()) != (StatusCode::BAD_REQUEST, Some("request_unsupported"), Some(*words)) {
                wrong.push(format!("{} at {at}: {} {resp}", body["model"], s.as_u16()));
                break;
            }
        }
    }
    assert!(wrong.is_empty(), "{} of {} answered otherwise:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed or logged as a failed generation");
    assert_eq!(db.count_generations(None).await.unwrap(), 0, "nothing reserved or stored");
}

/// round 3: a rule that hangs on the endpoint the request's images choose is
/// refused at the quote, in the adapter's words, and the generation refuses
/// it before it reserves: vidu2.0's lengths by endpoint and resolution,
/// viduq1's audio switch (text2video only), FLUX 3 on fal (its first-last
/// and keyframe endpoints priced at 720p and 1080p only, keyframes never
/// beside a first or last frame, two keyframes never on one frame), and
/// MiniMax H3's `prompt_expansion_mode` through Ideogram (reference to video
/// only). Each used to be quoted and reserved, then refused at submit. The
/// same request through an endpoint that takes it is still quoted.
#[tokio::test]
async fn a_rule_of_the_endpoint_the_images_choose_is_refused_at_the_quote_in_its_words() {
    use base64::Engine;
    use litegen::db::DatabaseStore;
    let dir = tempfile::tempdir().unwrap().keep();
    let (app, db) = harness::app_with(&dir, |cfg| {
        for v in ["vidu", "fal", "ideogram"] {
            cfg.providers.insert(v.to_string(), serde_json::from_value(json!({ "enabled": true, "api_key": "k" })).unwrap());
        }
    })
    .await;
    let png = base64::engine::general_purpose::STANDARD.encode(litegen::providers::image::visual_mock::generate_visual_image_png("a frame"));
    let image = |role: &str| json!({ "type": "base64", "value": png, "role": role });
    let keyframe = |at: f64| json!({ "type": "base64", "value": png, "role": "keyframe", "time_s": at });
    let prompt = "a lighthouse at dusk";
    let vidu2 = "(4 s at 360p/720p/1080p, 8 s at 720p; references 4 s at 360p/720p)";
    let refused = [
        (
            json!({ "model": "vidu/vidu2.0", "prompt": prompt, "duration_seconds": 8, "resolution": "1080p", "reference_images": [image("first_frame")] }),
            format!("vidu2.0 makes no 8 s 1080p clip on img2video {vidu2}"),
        ),
        (
            json!({ "model": "vidu/vidu2.0", "prompt": prompt, "duration_seconds": 8, "reference_images": [image("reference")] }),
            format!("vidu2.0 makes no 8 s 720p clip on reference2video {vidu2}"),
        ),
        (
            json!({ "model": "vidu/viduq1", "prompt": prompt, "audio": true, "reference_images": [image("first_frame")] }),
            "vidu/viduq1 has no audio switch on img2video: ask without audio, or use a mode that has one".to_string(),
        ),
        (
            json!({ "model": "fal/flux-3-video", "prompt": prompt, "resolution": "1440p", "reference_images": [image("first_frame"), image("last_frame")] }),
            "fal/flux-3-video: fal prices first-last and keyframe videos at 720p and 1080p only, not 1440p".to_string(),
        ),
        (
            json!({ "model": "fal/flux-3-video", "prompt": prompt, "reference_images": [image("first_frame"), keyframe(2.0)] }),
            "fal/flux-3-video: keyframes don't mix with a first or last frame: send those as keyframes at 0 s and at the end".to_string(),
        ),
        (
            json!({ "model": "fal/flux-3-video", "prompt": prompt, "reference_images": [keyframe(0.0), keyframe(0.01)] }),
            "fal/flux-3-video: two keyframes fall on frame 0 at 24 fps".to_string(),
        ),
        (
            json!({ "model": "ideogram/minimax-h3", "prompt": prompt, "extra": { "prompt_expansion_mode": "fast" } }),
            "ideogram/minimax-h3: `prompt_expansion_mode` is taken by reference-to-video only (attach reference images)".to_string(),
        ),
    ];
    let quoted = [
        json!({ "model": "vidu/vidu2.0", "prompt": prompt, "duration_seconds": 8, "resolution": "720p", "reference_images": [image("first_frame")] }),
        json!({ "model": "vidu/viduq1", "prompt": prompt, "audio": true }),
        json!({ "model": "fal/flux-3-video", "prompt": prompt, "resolution": "1080p", "reference_images": [image("first_frame"), image("last_frame")] }),
        json!({ "model": "fal/flux-3-video", "prompt": prompt, "resolution": "1440p", "reference_images": [image("first_frame")] }),
        json!({ "model": "fal/flux-3-video", "prompt": prompt, "reference_images": [keyframe(0.0), keyframe(0.5)] }),
        json!({ "model": "ideogram/minimax-h3", "prompt": prompt, "extra": { "prompt_expansion_mode": "fast" }, "reference_images": [image("reference")] }),
    ];
    let mut wrong = Vec::new();
    for (body, words) in &refused {
        for at in ["/v1/videos/cost", "/v1/videos/generations"] {
            let (s, resp) = post(&app, at, body.clone()).await;
            let answer = (s, resp["error"]["code"].as_str(), resp["error"]["message"].as_str(), resp["error"]["retryable"].as_bool());
            if answer != (StatusCode::BAD_REQUEST, Some("request_unsupported"), Some(words.as_str()), Some(false)) {
                wrong.push(format!("{} at {at}: {} {resp}", body["model"], s.as_u16()));
                break;
            }
        }
    }
    for body in &quoted {
        let (s, resp) = post(&app, "/v1/videos/cost", body.clone()).await;
        if s != StatusCode::OK {
            wrong.push(format!("{} (an endpoint that takes it) at /v1/videos/cost: {} {resp}", body["model"], s.as_u16()));
        }
    }
    assert!(wrong.is_empty(), "{} answered otherwise:\n{}", wrong.len(), wrong.join("\n"));
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed or logged as a failed generation");
    assert_eq!(db.count_generations(None).await.unwrap(), 0, "nothing reserved or stored");
}

/// round 3: an image request an adapter refuses for a vendor rule the
/// catalog can't say (two fields, or a field and the images) is refused at
/// the quote in the adapter's words, and the generation refuses it before
/// it reserves, one case per rule family: TokenHub's Seedream (a tier or a
/// size, "不可混用"), Hy-Image 3.5 (a size, or a tier and/or ratio) and Kling
/// image v3 (no negative prompt with an image), Kling's own, Hunyuan image
/// (four sizes, and no seed, with an image), Ideogram 4.5 (`very_low` needs
/// an image; no size with a mask), Runway (`very_low` and `edit` need an
/// image; Bria FIBO Edit's ratio needs two), Stability (inpaint needs a mask
/// or a region, search-and-replace a `mask_prompt`), fal's FLUX Fill (a
/// mask or a region) and BFL's FLUX 3 Image (at most 4,194,304 pixels with
/// an image). The Playground quotes before it generates, so each
/// used to be quoted, then refused at submit. A request that meets the rule
/// is still quoted, a region standing for the mask it is drawn into.
#[tokio::test]
async fn an_image_rule_the_catalog_cant_say_is_refused_at_the_quote_in_its_words() {
    use base64::Engine;
    use litegen::db::DatabaseStore;
    let dir = tempfile::tempdir().unwrap().keep();
    let (app, db) = harness::app_with(&dir, |cfg| {
        for v in ["tokenhub", "kling", "hunyuan", "ideogram", "runway", "stability", "fal", "bfl"] {
            cfg.providers.insert(v.to_string(), serde_json::from_value(json!({ "enabled": true, "api_key": "k" })).unwrap());
        }
    })
    .await;
    let png = base64::engine::general_purpose::STANDARD.encode(litegen::providers::image::visual_mock::generate_visual_image_png("an init image"));
    let image = |role: &str| json!({ "type": "base64", "value": png, "role": role });
    let prompt = "a lighthouse at dusk";
    let region = json!({ "x": 0, "y": 0, "w": 64, "h": 64 });
    let refused = [
        (
            json!({ "model": "tokenhub/seedream-image-v5.0-pro", "prompt": prompt, "size": "2048x2048", "resolution": "2K" }),
            "tokenhub: Seedream takes a resolution tier or an exact size, not both (\"不可混用\")",
        ),
        (
            json!({ "model": "tokenhub/hy-image-v3.5-preview", "prompt": prompt, "size": "1024x1024", "aspect_ratio": "1:1" }),
            "tokenhub: send an exact size, or resolution and/or aspect_ratio — not size with either",
        ),
        (
            json!({ "model": "tokenhub/kling-image-v3", "prompt": prompt, "negative_prompt": "blur", "reference_images": [image("init")] }),
            "tokenhub/kling-image-v3: a negative_prompt cannot go with a reference image",
        ),
        (
            json!({ "model": "kling/kling-v3", "prompt": prompt, "negative_prompt": "blur", "reference_images": [image("init")] }),
            "kling/kling-v3: a negative_prompt cannot go with a reference image",
        ),
        (
            json!({ "model": "hunyuan/hunyuan-image", "prompt": prompt, "size": "1280x720", "reference_images": [image("init")] }),
            "hunyuan-image: with a reference image the size is 768x768, 768x1024, 1024x768 or 1024x1024, not 1280x720",
        ),
        (
            json!({ "model": "hunyuan/hunyuan-image", "prompt": prompt, "seed": 7, "reference_images": [image("init")] }),
            "hunyuan-image: a seed has no effect with a reference image (Tencent keeps prompt expansion on)",
        ),
        (
            json!({ "model": "ideogram/ideogram-4-5", "prompt": prompt, "quality": "very_low" }),
            "'ideogram/ideogram-4-5': quality very_low needs a source image",
        ),
        (
            json!({ "model": "ideogram/ideogram-4-5", "prompt": prompt, "size": "1024x1024", "reference_images": [image("init"), image("mask")] }),
            "'ideogram/ideogram-4-5': with a mask the output is the source image's size; send no size",
        ),
        (
            json!({ "model": "runway/ideogram_4_5", "prompt": prompt, "quality": "very_low" }),
            "runway/ideogram_4_5: quality very_low needs a reference image and no preciseEdit",
        ),
        (
            json!({ "model": "runway/grok_imagine_image_2", "prompt": prompt, "extra": { "edit": true } }),
            "runway/grok_imagine_image_2: `edit` needs a reference image",
        ),
        (
            json!({ "model": "runway/bria_fibo_edit_1_5", "prompt": prompt, "aspect_ratio": "16:9", "reference_images": [image("init")] }),
            "runway/bria_fibo_edit_1_5 keeps a single image's framing: an aspect_ratio needs at least 2 images",
        ),
        (
            json!({ "model": "stability/inpaint", "prompt": prompt, "reference_images": [image("init")] }),
            "'stability/inpaint' edits a masked area: send a mask or a region",
        ),
        (
            json!({ "model": "stability/search-and-replace", "prompt": prompt, "reference_images": [image("init")] }),
            "search-and-replace needs `mask_prompt`: what to find and replace",
        ),
        (
            json!({ "model": "fal/flux-pro-fill", "prompt": prompt, "reference_images": [image("init")] }),
            "fal/flux-pro-fill: send a mask, or a region to edit",
        ),
        // Moved on reading bfl.rs (no sweep size reached it before): a guard
        // that FLUX 3 Image's smaller ceiling with an image stays at the quote.
        (
            json!({ "model": "bfl/flux-3-image", "prompt": prompt, "size": "4096x2048", "reference_images": [image("init")] }),
            "FLUX 3 Image makes at most 4194304 pixels with reference images; the size asks for 8388608",
        ),
    ];
    let quoted = [
        json!({ "model": "tokenhub/seedream-image-v5.0-pro", "prompt": prompt, "size": "2048x2048" }),
        json!({ "model": "tokenhub/hy-image-v3.5-preview", "prompt": prompt, "resolution": "2K", "aspect_ratio": "1:1" }),
        json!({ "model": "kling/kling-v3", "prompt": prompt, "negative_prompt": "blur" }),
        json!({ "model": "hunyuan/hunyuan-image", "prompt": prompt, "size": "1024x1024", "reference_images": [image("init")] }),
        json!({ "model": "ideogram/ideogram-4-5", "prompt": prompt, "quality": "very_low", "reference_images": [image("init")] }),
        json!({ "model": "ideogram/ideogram-4-5", "prompt": prompt, "reference_images": [image("init"), image("mask")] }),
        json!({ "model": "runway/bria_fibo_edit_1_5", "prompt": prompt, "reference_images": [image("init")] }),
        json!({ "model": "stability/inpaint", "prompt": prompt, "region": region, "reference_images": [image("init")] }),
        json!({ "model": "stability/search-and-replace", "prompt": prompt, "mask_prompt": "the sky", "reference_images": [image("init")] }),
        json!({ "model": "fal/flux-pro-fill", "prompt": prompt, "region": region, "reference_images": [image("init")] }),
        json!({ "model": "bfl/flux-3-image", "prompt": prompt, "size": "4096x2048" }),
    ];
    let mut wrong = Vec::new();
    for (body, words) in &refused {
        for at in ["/v1/images/cost", "/v1/images/generations"] {
            let (s, resp) = post(&app, at, body.clone()).await;
            let answer = (s, resp["error"]["code"].as_str(), resp["error"]["message"].as_str(), resp["error"]["retryable"].as_bool());
            if answer != (StatusCode::BAD_REQUEST, Some("request_unsupported"), Some(*words), Some(false)) {
                wrong.push(format!("{} at {at}: {} {resp}", body["model"], s.as_u16()));
                break;
            }
        }
    }
    for body in &quoted {
        let (s, resp) = post(&app, "/v1/images/cost", body.clone()).await;
        if s != StatusCode::OK {
            wrong.push(format!("{} (meets the rule) at /v1/images/cost: {} {resp}", body["model"], s.as_u16()));
        }
    }
    assert!(wrong.is_empty(), "{} answered otherwise:\n{}", wrong.len(), wrong.join("\n"));
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed or logged as a failed generation");
    assert_eq!(db.count_generations(None).await.unwrap(), 0, "nothing reserved or stored");
}

/// vidu-mix: Vidu takes first/last frames (img2video, start-end2video) or
/// reference images (reference2video), never both in one request. A mix was
/// quoted, reserved and sent to reference2video with the frame as one more
/// reference, so the clip did not start on it. Every Vidu row that takes a
/// frame and a reference, direct or resold on TokenHub, refuses the mix at
/// validation (its `reference` role's `not_with`) with `ref_role_conflict`,
/// before it is quoted, reserved or sent; a last frame too, which a first
/// frame would not help. Each role alone is still quoted.
#[tokio::test]
async fn a_frame_with_a_reference_is_refused_before_it_is_quoted_or_reserved() {
    use base64::Engine;
    use litegen::db::DatabaseStore;
    let dir = tempfile::tempdir().unwrap().keep();
    let (app, db) = harness::app_with(&dir, |cfg| {
        for v in ["vidu", "tokenhub"] {
            cfg.providers.insert(v.to_string(), serde_json::from_value(json!({ "enabled": true, "api_key": "k" })).unwrap());
        }
    })
    .await;
    let png = base64::engine::general_purpose::STANDARD.encode(litegen::providers::image::visual_mock::generate_visual_image_png("a frame"));
    let image = |role: &str| json!({ "type": "base64", "value": png, "role": role });
    let mut models_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    models_dir.pop();
    models_dir.push("models");
    let catalog = litegen::capabilities::CapabilityRegistry::from_dir(&models_dir).expect("models/");
    // Vidu direct, and the TokenHub rows tokenhub.rs sends to `vidu_submit`.
    let mut rows: Vec<(String, Vec<&str>)> = catalog
        .all()
        .filter(|m| m.provider == "vidu" || m.id.starts_with("tokenhub/vidu-video-"))
        .filter_map(|m| {
            let roles = &m.ref_inputs.as_ref()?.roles;
            let frames: Vec<&str> = ["first_frame", "last_frame"].into_iter().filter(|f| roles.contains_key(*f)).collect();
            (roles.contains_key("reference") && !frames.is_empty()).then(|| (m.id.clone(), frames))
        })
        .collect();
    rows.sort();
    assert!(rows.iter().any(|(id, _)| id.starts_with("vidu/")) && rows.iter().any(|(id, _)| id.starts_with("tokenhub/")), "{rows:?}");
    let mut wrong = Vec::new();
    for (model, frames) in &rows {
        for frame in frames {
            let body = json!({ "model": model, "prompt": "a lighthouse at dusk", "reference_images": [image(frame), image("reference")] });
            for at in ["/v1/videos/cost", "/v1/videos/generations"] {
                let (s, resp) = post(&app, at, body.clone()).await;
                let got = (s, resp["error"]["code"].as_str(), resp["error"]["param"].as_str());
                if got != (StatusCode::BAD_REQUEST, Some("ref_role_conflict"), Some("reference_images")) {
                    wrong.push(format!("{model} {frame} + reference at {at}: {} {resp}", s.as_u16()));
                }
            }
        }
        // Guards: each role alone is quoted.
        for role in ["first_frame", "reference"] {
            let (s, resp) = post(&app, "/v1/videos/cost", json!({ "model": model, "prompt": "a lighthouse at dusk", "reference_images": [image(role)] })).await;
            if s != StatusCode::OK {
                wrong.push(format!("{model} {role} alone at /v1/videos/cost: {} {resp}", s.as_u16()));
            }
        }
    }
    assert!(wrong.is_empty(), "{} answers of {} rows were otherwise:\n{}", wrong.len(), rows.len(), wrong.join("\n"));
    assert_eq!(db.get_request_logs(1, 100).await.unwrap().0.len(), 0, "nothing billed or logged as a failed generation");
    assert_eq!(db.count_generations(None).await.unwrap(), 0, "nothing reserved or stored");
}

/// One job makes one clip or one mesh, and one is billed (E3): a quote for
/// `n: 4` used to answer the price of one.
#[tokio::test]
async fn a_quote_for_several_clips_or_meshes_is_refused_not_priced_as_one() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for (at, model) in [("/v1/videos/cost", "mock/priced-video"), ("/v1/models3d/cost", "mock/priced-3d")] {
        let (s, resp) = post(&app, at, json!({ "model": model, "prompt": "x", "n": 4 })).await;
        assert_eq!(
            (s, resp["error"]["code"].as_str(), resp["error"]["param"].as_str()),
            (StatusCode::BAD_REQUEST, Some("param_out_of_range"), Some("n")),
            "{at} {model} n: 4: {resp}"
        );
    }
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
    // OpenAI's GPT Image 2 table at 1024x1024: "High $0.211" (not twice medium).
    // @see <https://developers.openai.com/api/docs/guides/image-generation.md> (Historical output costs, 2026-11-09)
    let high = quote(json!({ "model": "openai/gpt-image-2", "prompt": "x", "size": "1024x1024", "quality": "high" })).await;
    assert!(close(high, 0.211), "{high}");
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
