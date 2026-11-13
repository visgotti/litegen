//! `pre_call` through the real router: custom auth by an injected key,
//! refusals, rewrites that litegen still validates, multipart and converter
//! requests, failures and restarts, chains across runtimes, and the routes a
//! hook never sees. Each test runs over every installed runtime.

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

use std::path::PathBuf;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::Router;
use litegen::hooks::HookConfig;
use serde_json::{json, Value};
use sha2::Digest;
use tower::ServiceExt;

const KEY: &str = "test-master";

struct Rt {
    label: &'static str,
    behaviour: &'static str,
    named: Option<&'static str>,
    program: &'static str,
}

const RUNTIMES: &[Rt] = &[
    Rt { label: "python", behaviour: "behaviour.py", named: None, program: "python3" },
    Rt { label: "node", behaviour: "behaviour.mjs", named: None, program: "node" },
    Rt { label: "bun", behaviour: "behaviour.mjs", named: Some("bun"), program: "bun" },
    Rt { label: "bun-ts", behaviour: "behaviour.ts", named: Some("bun"), program: "bun" },
];

fn installed(program: &str) -> bool {
    let ok = std::process::Command::new(program).arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIPPED: {program} not installed");
    }
    ok
}

fn runtimes() -> impl Iterator<Item = &'static Rt> {
    RUNTIMES.iter().filter(|rt| installed(rt.program))
}

fn hook(file: &str, named: Option<&str>, label: &str, extra: Value) -> HookConfig {
    let mut v = json!({ "path": file, "env": { "HOOK_LABEL": label, "TEST_LITEGEN_KEY": KEY } });
    if let Some(r) = named {
        v["runtime"] = json!(r);
    }
    for (k, val) in extra.as_object().cloned().unwrap_or_default() {
        v[k] = val;
    }
    serde_json::from_value(v).unwrap()
}

async fn app(hooks: Vec<HookConfig>) -> (Router, tempfile::TempDir) {
    let assets = tempfile::tempdir().unwrap();
    let (app, _db) = harness::app_with_hooks(assets.path(), Some(KEY), hooks).await;
    (app, assets)
}

async fn behaviour_app(rt: &Rt, extra: Value) -> (Router, tempfile::TempDir) {
    app(vec![hook(rt.behaviour, rt.named, rt.label, extra)]).await
}

fn post(path: &str, body: Value, headers: &[(&str, &str)]) -> Request<Body> {
    let mut req = Request::post(path).header("content-type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.body(Body::from(body.to_string())).unwrap()
}

fn get(path: &str) -> Request<Body> {
    Request::get(path).header("authorization", format!("Bearer {KEY}")).body(Body::empty()).unwrap()
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    (parts.status, parts.headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn image(prompt: &str) -> Value {
    json!({ "model": "mock/visual-image-gen", "prompt": prompt })
}

/// The request as the generation's row recorded it.
async fn stored_request(app: &Router, id: &str) -> Value {
    let (status, _, row) = send(app, get(&format!("/v1/generations/{id}"))).await;
    assert_eq!(status, StatusCode::OK, "{row}");
    row["metadata"]["request"].clone()
}

fn png(width: u32, height: u32, noise: bool) -> Vec<u8> {
    let mut seed: u32 = 0x1234_5678;
    let img = image::RgbImage::from_fn(width, height, |x, y| {
        if noise {
            // xorshift: incompressible, so the PNG is about width*height*3 bytes.
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            image::Rgb([seed as u8, (seed >> 8) as u8, (seed >> 16) as u8])
        } else {
            image::Rgb([(x * 30) as u8, (y * 30) as u8, 128])
        }
    });
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn multipart(request: &Value, file: &[u8]) -> (String, Vec<u8>) {
    let boundary = "hookboundary42";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"request\"\r\nContent-Type: application/json\r\n\r\n{request}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"image_0\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn error_of(body: &Value) -> (&str, &str, &str) {
    let e = &body["error"];
    (e["code"].as_str().unwrap_or(""), e["message"].as_str().unwrap_or(""), e["hook"].as_str().unwrap_or(""))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_injected_key_authenticates_a_caller_with_no_litegen_key() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let (status, _, body) = send(&app, post("/v1/images/generations", image("a fox"), &[("x-test-action", "inject")])).await;
        assert_eq!(status, StatusCode::OK, "{}: {body}", rt.label);
        assert!(body["id"].is_string(), "{}: {body}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_the_hook_leaves_alone_is_judged_by_litegens_own_auth() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let (status, _, body) = send(&app, post("/v1/images/generations", image("a fox"), &[])).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{}: {body}", rt.label);
        let bearer = format!("Bearer {KEY}");
        let (status, _, body) = send(&app, post("/v1/images/generations", image("a fox"), &[("authorization", &bearer)])).await;
        assert_eq!(status, StatusCode::OK, "{}: {body}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_raised_status_reaches_the_caller_as_that_status_with_its_headers() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let (status, headers, body) = send(&app, post("/v1/images/generations", image("a fox"), &[("x-test-action", "refuse-status")])).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{}: {body}", rt.label);
        assert_eq!(headers.get("www-authenticate").and_then(|v| v.to_str().ok()), Some("Test"), "{}", rt.label);
        assert_eq!(error_of(&body), ("refused_by_hook", "who are you", rt.behaviour), "{}: {body}", rt.label);
        assert_eq!(body["error"]["type"], "hook_error", "{}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_returned_string_refuses_with_400() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let (status, _, body) = send(&app, post("/v1/images/generations", image("a fox"), &[("x-test-action", "refuse-string")])).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{}: {body}", rt.label);
        assert_eq!(error_of(&body), ("refused_by_hook", "not today", rt.behaviour), "{}: {body}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rewritten_body_is_what_the_handler_validates_and_runs() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let (status, _, body) =
            send(&app, post("/v1/images/generations", image("a fox"), &[("x-test-action", "inject,rewrite-prompt")])).await;
        assert_eq!(status, StatusCode::OK, "{}: {body}", rt.label);
        let request = stored_request(&app, body["id"].as_str().unwrap()).await;
        assert_eq!(request["prompt"], "a fox (hooked)", "{}: {request}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rewrite_cannot_skip_validation() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let (status, _, body) = send(&app, post("/v1/images/generations", image("a fox"), &[("x-test-action", "inject,n2")])).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{}: {body}", rt.label);
        assert_eq!(body["error"]["code"], "param_out_of_range", "{}: {body}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_multipart_request_part_is_hooked_and_its_files_pass_through_intact() {
    let file = png(16, 16, false);
    let request = json!({
        "model": "mock/mesh-3d", "prompt": "a fox",
        "reference_images": [{ "type": "blob", "value": "image_0", "role": "init" }],
    });
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let call = |action: &'static str| {
            let (ct, body) = multipart(&request, &file);
            Request::post("/v1/models3d/generations").header("content-type", ct).header("x-test-action", action).body(Body::from(body)).unwrap()
        };

        let (status, _, body) = send(&app, call("report")).await;
        assert_eq!(status, StatusCode::IM_A_TEAPOT, "{}: {body}", rt.label);
        let seen: Value = serde_json::from_str(error_of(&body).1).unwrap();
        assert_eq!(seen["body"]["prompt"], "a fox", "{}: {seen}", rt.label);
        let f = &seen["files"][0];
        assert_eq!((f["name"].as_str(), f["filename"].as_str(), f["content_type"].as_str()), (Some("image_0"), Some("a.png"), Some("image/png")), "{}: {seen}", rt.label);
        assert_eq!(f["size"], json!(file.len()), "{}: {seen}", rt.label);

        let (status, _, body) = send(&app, call("inject,rewrite-prompt")).await;
        assert_eq!(status, StatusCode::OK, "{}: {body}", rt.label);
        let stored = stored_request(&app, body["id"].as_str().unwrap()).await;
        assert_eq!(stored["prompt"], "a fox (hooked)", "{}: {stored}", rt.label);
        assert_eq!(stored["reference_images"][0]["sha256"], json!(hex::encode(sha2::Sha256::digest(&file))), "{}: {stored}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_converter_route_shows_the_hook_headers_and_query_only() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let req = Request::post("/v1/utils/models3d/convert?from=glb&to=obj")
            .header("content-type", "model/gltf-binary")
            .header("x-test-action", "report")
            .body(Body::from(vec![0u8; 4096]))
            .unwrap();
        let (status, _, body) = send(&app, req).await;
        assert_eq!(status, StatusCode::IM_A_TEAPOT, "{}: {body}", rt.label);
        let seen: Value = serde_json::from_str(error_of(&body).1).unwrap();
        assert_eq!(seen["body"], Value::Null, "{}: {seen}", rt.label);
        assert_eq!(seen["query"], "from=glb&to=obj", "{}: {seen}", rt.label);

        // The upload streams through unread: one past the 25 MiB the hook layer
        // buffers still reaches the hook, rather than a 413.
        let big = Request::post("/v1/utils/models3d/convert?from=glb&to=obj")
            .header("content-type", "model/gltf-binary")
            .header("x-test-action", "report")
            .body(Body::from(vec![0u8; 26 * 1024 * 1024]))
            .unwrap();
        let (status, _, body) = send(&app, big).await;
        assert_eq!(status, StatusCode::IM_A_TEAPOT, "{}: {body}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_timed_out_pre_call_refuses_with_503_hook_failed() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({ "timeout_ms": 200 })).await;
        let (status, _, body) = send(&app, post("/v1/images/generations", image("a fox"), &[("x-test-action", "sleep")])).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{}: {body}", rt.label);
        let (code, message, hook) = error_of(&body);
        assert_eq!((code, hook), ("hook_failed", rt.behaviour), "{}: {body}", rt.label);
        assert!(message.contains(rt.behaviour), "{}: {message}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_on_error_allow_a_failed_pre_call_lets_the_request_through() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({ "timeout_ms": 200, "on_error": "allow" })).await;
        let bearer = format!("Bearer {KEY}");
        let (status, _, body) =
            send(&app, post("/v1/images/generations", image("a fox"), &[("x-test-action", "sleep"), ("authorization", &bearer)])).await;
        assert_eq!(status, StatusCode::OK, "{}: {body}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crashed_pre_call_refuses_then_the_restarted_hook_serves() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let (status, _, body) = send(&app, post("/v1/images/generations", image("a fox"), &[("x-test-action", "crash")])).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{}: {body}", rt.label);
        assert_eq!(error_of(&body).0, "hook_failed", "{}: {body}", rt.label);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (status, _, body) = send(&app, post("/v1/images/generations", image("a fox"), &[("x-test-action", "inject")])).await;
            if status == StatusCode::OK {
                break;
            }
            assert!(Instant::now() < deadline, "{}: not serving within 5 s: {status} {body}", rt.label);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_malformed_answer_is_a_hook_failure() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        for action in ["bad-shape", "change-path"] {
            let (status, _, body) = send(&app, post("/v1/images/generations", image("a fox"), &[("x-test-action", action)])).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{} {action}: {body}", rt.label);
            let (code, message, hook) = error_of(&body);
            assert_eq!((code, hook), ("hook_failed", rt.behaviour), "{} {action}: {body}", rt.label);
            assert!(message.contains(rt.behaviour), "{} {action}: {message}", rt.label);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hooks_run_in_order_and_the_first_refusal_stops_the_chain() {
    if !(installed("python3") && installed("node") && installed("bun")) {
        return;
    }
    let chain = vec![
        hook("behaviour.py", None, "python", json!({})),
        hook("behaviour.mjs", None, "node", json!({})),
        hook("behaviour.ts", Some("bun"), "bun-ts", json!({})),
    ];
    let (chained, _a) = app(chain).await;
    let (status, _, body) = send(&chained, post("/v1/images/generations", image("a fox"), &[("x-test-action", "inject,rewrite-prompt")])).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let request = stored_request(&chained, body["id"].as_str().unwrap()).await;
    assert_eq!(request["prompt"], "a fox (hooked) (hooked) (hooked)", "{request}");

    let (refusing, _b) = app(vec![hook("refuse_all.py", None, "python", json!({})), hook("behaviour.mjs", None, "node", json!({}))]).await;
    let (status, _, body) = send(&refusing, post("/v1/images/generations", image("a fox"), &[("x-test-action", "report")])).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(error_of(&body), ("refused_by_hook", "refused by refuse_all", "refuse_all.py"), "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn management_and_read_routes_never_reach_a_hook() {
    if !installed("python3") {
        return;
    }
    let (refusing, _a) = app(vec![hook("refuse_all.py", None, "python", json!({}))]).await;
    // The hook is live on a generation route...
    let bearer = format!("Bearer {KEY}");
    let (status, _, body) = send(&refusing, post("/v1/images/generations", image("a fox"), &[("authorization", &bearer)])).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    // ...and never consulted elsewhere.
    for path in ["/v1/keys", "/v1/models", "/v1/generations"] {
        let (status, _, body) = send(&refusing, get(path)).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn headers_the_hook_did_not_touch_reach_litegen_unchanged() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let req = Request::post("/v1/images/generations")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {KEY}"))
            .header("x-raw", HeaderValue::from_bytes(&[0xff, 0xfe, b'a']).unwrap())
            .header("x-dup", "1")
            .header("x-dup", "2")
            .header("cookie", "a=1; b=2")
            .body(Body::from(image("a fox").to_string()))
            .unwrap();
        let (status, _, body) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "{}: {body}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_large_body_passes_through_an_untouched_hook_intact() {
    use base64::Engine as _;
    let file = png(1300, 1300, true);
    assert!(file.len() > 5_000_000, "{}", file.len());
    let b64 = base64::engine::general_purpose::STANDARD.encode(&file);
    let request = json!({
        "model": "mock/mesh-3d", "prompt": "a fox",
        "reference_images": [{ "type": "base64", "value": format!("data:image/png;base64,{b64}"), "role": "init" }],
    });
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let bearer = format!("Bearer {KEY}");
        let (status, _, body) = send(&app, post("/v1/models3d/generations", request.clone(), &[("authorization", &bearer)])).await;
        assert_eq!(status, StatusCode::OK, "{}: {body}", rt.label);
        let stored = stored_request(&app, body["id"].as_str().unwrap()).await;
        assert_eq!(stored["reference_images"][0]["bytes"], json!(file.len()), "{}: {stored}", rt.label);
        assert_eq!(stored["reference_images"][0]["sha256"], json!(hex::encode(sha2::Sha256::digest(&file))), "{}", rt.label);
    }
}

/// The spec's ~1 ms target, measured rather than asserted (a 10 KB cost
/// quote, which runs no vendor), run with
/// `cargo test --release --test hooks_api -- --ignored pass_through_overhead --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn pass_through_overhead() {
    use base64::Engine as _;
    // About 10 KB of JSON: a 3D cost quote carrying a small inline image.
    let image = base64::engine::general_purpose::STANDARD.encode(png(50, 50, true));
    let body = json!({
        "model": "mock/mesh-3d", "prompt": "a fox",
        "reference_images": [{ "type": "base64", "value": format!("data:image/png;base64,{image}"), "role": "init" }],
    });
    assert!(body.to_string().len() > 9_000, "{}", body.to_string().len());
    let bearer = format!("Bearer {KEY}");
    async fn timings(app: &Router, body: &Value, bearer: &str) -> Vec<Duration> {
        let mut out = Vec::new();
        for _ in 0..200 {
            let t = Instant::now();
            let (status, _, b) = send(app, post("/v1/models3d/cost", body.clone(), &[("authorization", bearer)])).await;
            assert_eq!(status, StatusCode::OK, "{b}");
            out.push(t.elapsed());
        }
        out.sort();
        out
    }
    let pct = |v: &[Duration], p: f64| v[((v.len() as f64 - 1.0) * p) as usize];
    let (plain, _a) = app(Vec::new()).await;
    let base = timings(&plain, &body, &bearer).await;
    for rt in runtimes() {
        let (hooked, _b) = behaviour_app(rt, json!({})).await;
        let with = timings(&hooked, &body, &bearer).await;
        println!(
            "{:7} p50 +{:?}  p95 +{:?}  (no hook: p50 {:?}, p95 {:?})",
            rt.label,
            pct(&with, 0.5).saturating_sub(pct(&base, 0.5)),
            pct(&with, 0.95).saturating_sub(pct(&base, 0.95)),
            pct(&base, 0.5),
            pct(&base, 0.95),
        );
    }
}

#[allow(dead_code)]
fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hooks")
}

// ─── post_call ────────────────────────────────────────────────────────────────

fn bearer() -> String {
    format!("Bearer {KEY}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_call_sees_the_finished_image_response_and_can_rewrite_it() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let auth = bearer();
        let (status, headers, body) =
            send(&app, post("/v1/images/generations", image("a fox"), &[("authorization", &auth), ("x-test-post", "tag")])).await;
        assert_eq!(status, StatusCode::OK, "{}: {body}", rt.label);
        assert_eq!(body["hooked"], rt.label, "{}: {body}", rt.label);
        assert!(body["data"].is_array() && body["id"].is_string(), "{}: the finished images are still there: {body}", rt.label);
        assert_eq!(headers.get("x-hooked").and_then(|v| v.to_str().ok()), Some(rt.label), "{}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_call_sees_the_authenticated_caller() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        // A real API key, so the caller litegen authenticated has a key id.
        let auth = bearer();
        let (status, _, created) = send(&app, post("/v1/keys", json!({ "name": "hook-caller", "scopes": "generate" }), &[("authorization", &auth)])).await;
        assert_eq!(status, StatusCode::CREATED, "{}: {created}", rt.label);
        let key_auth = format!("Bearer {}", created["key"].as_str().unwrap());
        let headers = [("authorization", key_auth.as_str()), ("x-litegen-end-user", "alice"), ("x-test-post", "report")];
        let (status, h, body) = send(&app, post("/v1/images/generations", image("a fox"), &headers)).await;
        assert_eq!(status, StatusCode::OK, "{}: {body}", rt.label);
        let caller: Value = serde_json::from_str(h.get("x-seen-caller").unwrap().to_str().unwrap()).unwrap();
        assert_eq!(caller["key_id"], created["id"], "{}: {caller}", rt.label);
        assert_eq!(caller["end_user"], "alice", "{}: {caller}", rt.label);
        assert_eq!(caller["user"], Value::Null, "{}: an API key has no session user: {caller}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_call_sees_each_video_and_3d_status_response_including_the_finished_one() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let auth = bearer();
        for (submit, model, status_route) in [
            ("/v1/models3d/generations", "mock/mesh-3d", "/v1/models3d"),
            ("/v1/videos/generations", "mock/video-gen", "/v1/videos"),
        ] {
            let (status, _, body) =
                send(&app, post(submit, json!({ "model": model, "prompt": "a fox" }), &[("authorization", &auth), ("x-test-post", "tag")])).await;
            assert_eq!(status, StatusCode::OK, "{} {model}: {body}", rt.label);
            assert_eq!(body["hooked"], rt.label, "{} {model}: the submit answer: {body}", rt.label);
            let id = body["id"].as_str().unwrap().to_string();
            let mut last = Value::Null;
            for _ in 0..20 {
                let req = Request::get(format!("{status_route}/{id}"))
                    .header("authorization", &auth)
                    .header("x-test-post", "tag")
                    .body(Body::empty())
                    .unwrap();
                let (status, _, body) = send(&app, req).await;
                assert_eq!(status, StatusCode::OK, "{} {model}: {body}", rt.label);
                assert_eq!(body["hooked"], rt.label, "{} {model}: every status answer: {body}", rt.label);
                last = body;
                if last["status"] == "completed" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            assert_eq!(last["status"], "completed", "{} {model}: {last}", rt.label);
            let finished = if model == "mock/mesh-3d" { &last["assets"] } else { &last["video_url"] };
            assert!(!finished.is_null() && finished != &json!([]), "{} {model}: the finished result is there: {last}", rt.label);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_post_call_sends_litegens_response_unchanged_by_default() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let auth = bearer();
        let (status, headers, body) =
            send(&app, post("/v1/images/generations", image("a fox"), &[("authorization", &auth), ("x-test-post", "fail")])).await;
        assert_eq!(status, StatusCode::OK, "{}: {body}", rt.label);
        assert!(body["data"].is_array() && body.get("hooked").is_none(), "{}: {body}", rt.label);
        assert!(headers.get("x-hooked").is_none(), "{}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_on_error_refuse_a_failing_post_call_answers_503() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({ "on_error": "refuse" })).await;
        let auth = bearer();
        let (status, _, body) =
            send(&app, post("/v1/images/generations", image("a fox"), &[("authorization", &auth), ("x-test-post", "fail")])).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{}: {body}", rt.label);
        let (code, message, hook) = error_of(&body);
        assert_eq!((code, hook), ("hook_failed", rt.behaviour), "{}: {body}", rt.label);
        assert!(message.contains("boom"), "{}: {message}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_call_can_change_the_status() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let auth = bearer();
        let (status, _, body) =
            send(&app, post("/v1/images/generations", image("a fox"), &[("authorization", &auth), ("x-test-post", "teapot")])).await;
        assert_eq!(status, StatusCode::IM_A_TEAPOT, "{}: {body}", rt.label);
        assert!(body["data"].is_array(), "{}: the body is litegen's: {body}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_call_sees_a_validation_error_response_too() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let auth = bearer();
        let mut invalid = image("a fox");
        invalid["n"] = json!(2);
        let (status, headers, body) = send(&app, post("/v1/images/generations", invalid, &[("authorization", &auth), ("x-test-post", "report")])).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{}: {body}", rt.label);
        assert_eq!(body["error"]["code"], "param_out_of_range", "{}: {body}", rt.label);
        assert!(headers.get("x-seen-caller").is_some(), "{}: post_call ran on the 400", rt.label);
    }
}

async fn convert(app: &Router, glb: &[u8], headers: &[(&str, &str)]) -> (StatusCode, HeaderMap, Vec<u8>) {
    for _ in 0..200 {
        let mut req = Request::post("/v1/utils/models3d/convert?from=glb&to=obj").header("content-type", "model/gltf-binary");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = app.clone().oneshot(req.body(Body::from(glb.to_vec())).unwrap()).await.unwrap();
        let (parts, body) = resp.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap().to_vec();
        // The converter's slots are process-wide: a busy moment is a 503 to retry.
        if parts.status == StatusCode::SERVICE_UNAVAILABLE && String::from_utf8_lossy(&bytes).contains("server_at_capacity") {
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        }
        return (parts.status, parts.headers, bytes);
    }
    panic!("the converter stayed busy");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_converter_response_streams_through_post_call_untouched() {
    let glb = litegen::providers::model3d::glb::generate_cube_glb("a cube");
    let auth = bearer();
    let (plain, _p) = app(Vec::new()).await;
    let (status, _, want) = convert(&plain, &glb, &[("authorization", &auth)]).await;
    assert_eq!(status, StatusCode::OK);
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let (status, headers, got) = convert(&app, &glb, &[("authorization", &auth), ("x-test-post", "tag")]).await;
        assert_eq!(status, StatusCode::OK, "{}", rt.label);
        assert_eq!(got, want, "{}: the converted file is byte for byte the hook-free one", rt.label);
        assert_eq!(headers.get("x-hooked").and_then(|v| v.to_str().ok()), Some(rt.label), "{}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idempotent_replay_passes_through_post_call_again() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let auth = bearer();
        let key = format!("hook-replay-{}", rt.label);
        let headers = [("authorization", auth.as_str()), ("x-test-post", "tag"), ("idempotency-key", key.as_str())];
        let (s1, h1, first) = send(&app, post("/v1/images/generations", image("a fox"), &headers)).await;
        let (s2, h2, second) = send(&app, post("/v1/images/generations", image("a fox"), &headers)).await;
        assert_eq!((s1, s2), (StatusCode::OK, StatusCode::OK), "{}: {first} {second}", rt.label);
        assert_eq!((first["hooked"].clone(), second["hooked"].clone()), (json!(rt.label), json!(rt.label)), "{}", rt.label);
        assert_eq!(first["id"], second["id"], "{}", rt.label);
        assert!(h1.get("idempotent-replayed").is_none(), "{}", rt.label);
        assert_eq!(h2.get("idempotent-replayed").and_then(|v| v.to_str().ok()), Some("true"), "{}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn past_max_in_flight_a_hook_layer_answers_503_at_once_rather_than_queueing() {
    if !installed("python3") {
        return;
    }
    let assets = tempfile::tempdir().unwrap();
    let hooks = vec![hook("behaviour.py", None, "python", json!({ "timeout_ms": 5000 }))];
    let (app, _db) = harness::app_with_hooks_tweaked(assets.path(), Some(KEY), hooks, |c| c.backpressure.max_in_flight = 2).await;
    // Two requests sit in the hook for 3 s, holding both slots.
    let parked: Vec<_> = (0..2)
        .map(|_| {
            let app = app.clone();
            tokio::spawn(async move { send(&app, post("/v1/images/generations", image("a fox"), &[("x-test-action", "sleep")])).await })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let started = Instant::now();
    let auth = bearer();
    let (status, headers, body) = send(&app, post("/v1/images/generations", image("a fox"), &[("authorization", &auth)])).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "server_at_capacity", "{body}");
    assert_eq!(headers.get("retry-after").and_then(|v| v.to_str().ok()), Some("1"));
    assert!(started.elapsed() < Duration::from_secs(1), "answered after {:?}: it queued", started.elapsed());
    for p in parked {
        p.await.unwrap();
    }
}

/// P9: a body still arriving holds none of the hook layer's slots, so an
/// unauthenticated client that stalls its body blocks no one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_unauthenticated_body_holds_no_hooked_route_slot() {
    use futures::StreamExt;
    if !installed("python3") {
        return;
    }
    let assets = tempfile::tempdir().unwrap();
    let hooks = vec![hook("pre_only.py", None, "python", json!({}))];
    let (app, _db) = harness::app_with_hooks_tweaked(assets.path(), Some(KEY), hooks, |c| c.backpressure.max_in_flight = 1).await;
    let (polled_tx, polled_rx) = tokio::sync::oneshot::channel::<()>();
    let first = futures::stream::once(async move {
        let _ = polled_tx.send(());
        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"{\"model\":"))
    });
    let stalled = Request::post("/v1/images/generations")
        .header("content-type", "application/json")
        .body(Body::from_stream(first.chain(futures::stream::pending())))
        .unwrap();
    let held = tokio::spawn(app.clone().oneshot(stalled));
    polled_rx.await.unwrap();
    let auth = bearer();
    let (status, _, body) = send(&app, post("/v1/images/generations", image("a fox"), &[("authorization", &auth)])).await;
    let (read_status, _, read) = send(&app, get("/v1/videos/litegen-vid-none")).await;
    held.abort();
    assert_eq!(status, StatusCode::OK, "a stalled unauthenticated body held the hook layer's only slot: {body}");
    assert_eq!(read_status, StatusCode::NOT_FOUND, "{read}");
}

/// N3: the hook layer's slots bound the requests whose body it reads. A
/// status read never waits on them, even while every slot is taken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_status_read_never_waits_on_the_hook_layers_slots() {
    if !installed("python3") {
        return;
    }
    let assets = tempfile::tempdir().unwrap();
    let hooks = vec![hook("behaviour.py", None, "python", json!({ "timeout_ms": 5000 }))];
    let (app, _db) = harness::app_with_hooks_tweaked(assets.path(), Some(KEY), hooks, |c| c.backpressure.max_in_flight = 1).await;
    // One request holds the only slot: the layer keeps it across the handler,
    // and `mock/slow-image` answers after a day. Its call is counted only once
    // it is inside the vendor call, so the slot is surely held from then on.
    let prompt = format!("a slow fox {}", uuid::Uuid::new_v4().simple());
    let parked = {
        let (app, prompt, auth) = (app.clone(), prompt.clone(), bearer());
        tokio::spawn(async move {
            send(&app, post("/v1/images/generations", json!({ "model": "mock/slow-image", "prompt": prompt }), &[("authorization", &auth)])).await
        })
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while litegen::providers::image::mock::slow_image_calls(&prompt) == 0 {
        assert!(Instant::now() < deadline, "the parked request never reached the vendor call");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (status, _, body) = send(&app, get("/v1/videos/litegen-vid-none")).await;
    parked.abort();
    assert_eq!(status, StatusCode::NOT_FOUND, "a status read waited on the hook layer's slot: {body}");
}

// ─── file_access: the hook on litegen's file route ──────────────────────────

/// An image made as `auth` for `end_user`; its generation keeps the file.
async fn kept_image(app: &Router, auth: &str, end_user: &str) -> String {
    let (status, _, body) = send(app, post("/v1/images/generations", image("a kept fox"), &[("authorization", auth), ("x-litegen-end-user", end_user)])).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["id"].as_str().unwrap().to_string()
}

/// `GET /v1/generations/{id}/assets/0` with these headers.
fn file_get(id: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut req = Request::get(format!("/v1/generations/{id}/assets/0"));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.body(Body::empty()).unwrap()
}

/// A new API key: its bearer header and its id.
async fn api_key(app: &Router) -> (String, String) {
    let (status, _, created) = send(app, post("/v1/keys", json!({ "name": "file-caller" }), &[("authorization", &bearer())])).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    (format!("Bearer {}", created["key"].as_str().unwrap()), created["id"].as_str().unwrap().to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_access_sees_the_file_and_the_caller() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let (key, key_id) = api_key(&app).await;
        let id = kept_image(&app, &key, "alice").await;
        let (status, _, body) = send(&app, file_get(&id, &[("authorization", &key), ("x-litegen-end-user", "carol"), ("x-test-file", "report")])).await;
        assert_eq!(status, StatusCode::IM_A_TEAPOT, "{}: {body}", rt.label);
        let seen: Value = serde_json::from_str(body["error"]["message"].as_str().unwrap()).unwrap();
        let file = &seen["file"];
        assert_eq!(file["generation_id"], id.as_str(), "{}: {seen}", rt.label);
        assert_eq!(file["index"], 0, "{}: {seen}", rt.label);
        assert_eq!((&file["kind"], &file["media_type"]), (&json!("image"), &json!("image")), "{}: {seen}", rt.label);
        assert_eq!(file["created_by"]["key_id"], key_id.as_str(), "{}: {seen}", rt.label);
        assert_eq!(file["created_by"]["end_user"], "alice", "{}: who made it: {seen}", rt.label);
        assert_eq!(seen["caller"]["key_id"], key_id.as_str(), "{}: {seen}", rt.label);
        assert_eq!(seen["caller"]["end_user"], "carol", "{}: who is asking: {seen}", rt.label);
        let route = format!("/v1/generations/{id}/assets/0");
        assert_eq!((&seen["request"]["method"], &seen["request"]["path"]), (&json!("GET"), &json!(route)), "{}: the request it decides on: {seen}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_access_can_refuse_another_end_users_file() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let id = kept_image(&app, &bearer(), "alice").await;
        let auth = bearer();
        let fetch = |who: &str, action: &str| {
            let headers = [("authorization", auth.as_str()), ("x-litegen-end-user", who), ("x-test-file", action)];
            send(&app, file_get(&id, &headers))
        };
        let (status, _, body) = fetch("bob", "deny-other-end-user").await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{}: {body}", rt.label);
        assert_eq!((&body["error"]["code"], &body["error"]["message"]), (&json!("refused_by_hook"), &json!("not your file")), "{}: {body}", rt.label);
        assert_eq!(fetch("alice", "deny-other-end-user").await.0, StatusCode::OK, "{}: her own file", rt.label);
        // `false` and a string refuse too.
        assert_eq!(fetch("alice", "deny-false").await.0, StatusCode::FORBIDDEN, "{}", rt.label);
        let (status, _, body) = fetch("alice", "deny-string").await;
        assert_eq!((status, &body["error"]["message"]), (StatusCode::FORBIDDEN, &json!("files are closed today")), "{}: {body}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_file_access_refuses_unless_allowed() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let id = kept_image(&app, &bearer(), "alice").await;
        let (status, _, body) = send(&app, file_get(&id, &[("authorization", &bearer()), ("x-test-file", "fail")])).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{}: {body}", rt.label);
        assert_eq!(body["error"]["code"], "hook_failed", "{}: {body}", rt.label);

        let (app, _a) = behaviour_app(rt, json!({ "on_error": "allow" })).await;
        let id = kept_image(&app, &bearer(), "alice").await;
        let (status, _, body) = send(&app, file_get(&id, &[("authorization", &bearer()), ("x-test-file", "fail")])).await;
        assert_eq!(status, StatusCode::OK, "{}: on_error allow serves: {body}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_access_cannot_open_a_file_the_scope_refuses() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let (maker, _) = api_key(&app).await;
        let (other, _) = api_key(&app).await;
        let id = kept_image(&app, &maker, "alice").await;
        // `report` answers 418 whenever the hook runs. It never runs for a
        // caller the owner scope (`key`, the default) already refused.
        let (status, _, body) = send(&app, file_get(&id, &[("authorization", &other), ("x-test-file", "report")])).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{}: {body}", rt.label);
        assert_eq!(body["error"]["code"], "not_file_owner", "{}: {body}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pre_call_custom_auth_reaches_the_file_route() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let id = kept_image(&app, &bearer(), "alice").await;
        assert_eq!(send(&app, file_get(&id, &[])).await.0, StatusCode::UNAUTHORIZED, "{}: no key, no hook", rt.label);
        // No Authorization: the pre_call hook injects the key, as custom auth would.
        let (status, headers, _) = send(&app, file_get(&id, &[("x-test-action", "inject")])).await;
        assert_eq!(status, StatusCode::OK, "{}", rt.label);
        assert_eq!(headers["content-type"], "image/png", "{}: the file itself", rt.label);
        // post_call sees the file's answer too (its body is the file, so null).
        let (status, headers, _) = send(&app, file_get(&id, &[("x-test-action", "inject"), ("x-test-post", "tag")])).await;
        assert_eq!((status, headers.get("x-hooked").map(|v| v.to_str().unwrap())), (StatusCode::OK, Some(rt.label)), "{}", rt.label);
    }
}

// ─── file_access on a stored file a new job reads (review 2026-11-11 SEC-FILEHOOK) ──

/// A finished job (`kind` is `models3d` or `videos`) made with the master
/// key for `end_user`: its id.
async fn finished_for(app: &Router, kind: &str, body: Value, end_user: &str) -> String {
    let auth = bearer();
    let (status, _, made) = send(app, post(&format!("/v1/{kind}/generations"), body, &[("authorization", auth.as_str()), ("x-litegen-end-user", end_user)])).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let id = made["id"].as_str().unwrap().to_string();
    for _ in 0..20 {
        let (_, _, v) = send(app, get(&format!("/v1/{kind}/{id}"))).await;
        if v["status"] == "completed" {
            return id;
        }
        assert_ne!(v["status"], "failed", "{v}");
    }
    panic!("{kind} job {id} never finished");
}

/// A request naming a stored generation as its input reads that file, so the
/// operator's `file_access` hook decides, as on the file route: once the
/// file is chosen, with that file in its `file` view.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chaining_another_end_users_file_asks_file_access() {
    for rt in runtimes() {
        let (app, _a) = behaviour_app(rt, json!({})).await;
        let image = kept_image(&app, &bearer(), "alice").await;
        let mesh = finished_for(&app, "models3d", json!({ "model": "mock/mesh-3d", "prompt": "a chair" }), "alice").await;
        let clip = finished_for(&app, "videos", json!({ "model": "mock/sprite-video", "prompt": "a knight walks", "duration_seconds": 2 }), "alice").await;
        let chains = [
            ("/v1/images/generations", &image, "image", json!({ "model": "mock/base64-refs-image", "prompt": "reuse", "reference_images": [{ "type": "generation", "value": image, "role": "init" }] })),
            ("/v1/models3d/generations", &mesh, "mesh", json!({ "model": "mock/rig-3d", "source_model": { "type": "generation", "value": mesh } })),
            (
                "/v1/videos/generations",
                &clip,
                "video",
                json!({ "model": "mock/extend-video", "prompt": "he keeps walking", "source_video": { "type": "generation", "value": clip }, "direction": "forward", "duration_seconds": 1 }),
            ),
        ];
        let auth = bearer();
        for (uri, source, kind, body) in &chains {
            let as_ = |who: &'static str, action: &'static str| post(uri, body.clone(), &[("authorization", auth.as_str()), ("x-litegen-end-user", who), ("x-test-file", action)]);
            let (status, _, refused) = send(&app, as_("bob", "deny-other-end-user")).await;
            assert_eq!(
                (status, &refused["error"]["code"], &refused["error"]["message"]),
                (StatusCode::FORBIDDEN, &json!("refused_by_hook"), &json!("not your file")),
                "{}: bob chains alice's file into {uri}: {refused}",
                rt.label
            );
            let (status, _, seen) = send(&app, as_("alice", "report")).await;
            assert_eq!(status, StatusCode::IM_A_TEAPOT, "{}: {uri}: the hook runs: {seen}", rt.label);
            let seen: Value = serde_json::from_str(seen["error"]["message"].as_str().unwrap()).unwrap();
            assert_eq!((&seen["file"]["generation_id"], &seen["file"]["kind"]), (&json!(source), &json!(kind)), "{}: {uri}: the file it reads: {seen}", rt.label);
            assert_eq!((&seen["request"]["method"], &seen["request"]["path"]), (&json!("POST"), &json!(uri)), "{}: {uri}: the request it decides on: {seen}", rt.label);
            let (status, _, made) = send(&app, as_("alice", "deny-other-end-user")).await;
            assert_eq!(status, StatusCode::OK, "{}: alice chains her own file into {uri}: {made}", rt.label);
        }
    }
}
