//! Keeping nothing (`keep_outputs: false`, docs/storage-access.md): litegen
//! writes no output file anywhere and hands out the vendor's own link, or the
//! image itself.

mod harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use litegen::api::middleware::DEFAULT_APP_ID;
use litegen::proxy::storage::{ImageStore, ImageStoreError};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

const MASTER: &str = "sk-keep-nothing-master";

/// The full app behind [`MASTER`], its assets in a fresh directory, with `tweak` applied.
async fn app(tweak: impl FnOnce(&mut litegen::config::AppConfig)) -> (Router, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("asset dir").keep();
    let (app, _db) = harness::app_with_hooks_tweaked(&dir, Some(MASTER), Vec::new(), tweak).await;
    (app, dir)
}

async fn call(app: &Router, method: &str, uri: &str, headers: &[(&str, &str)], body: Option<serde_json::Value>) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
    let mut req = Request::builder().method(method).uri(uri).header("authorization", format!("Bearer {MASTER}"));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

async fn keep_nothing(app: &Router) {
    let (s, _, v) = call(app, "PUT", &format!("/v1/apps/{DEFAULT_APP_ID}/access"), &[], Some(serde_json::json!({ "keep_outputs": false }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

async fn image(app: &Router, headers: &[(&str, &str)]) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
    let body = serde_json::json!({ "model": "mock/image-gen", "prompt": "a red fox", "response_format": "url" });
    call(app, "POST", "/v1/images/generations", headers, Some(body)).await
}

/// Every file under `dir` whose path names `id`.
fn files_for(dir: &std::path::Path, id: &str) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.to_string_lossy().contains(id) {
                found.push(path);
            }
        }
    }
    found
}

/// The request log's artifact for `id`, once the asynchronous write lands.
async fn artifact(app: &Router, id: &str) -> serde_json::Value {
    for _ in 0..100 {
        let (s, _, v) = call(app, "GET", &format!("/v1/logs/{id}/artifact"), &[], None).await;
        if s == StatusCode::OK {
            return v;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("no artifact for {id}");
}

#[tokio::test]
async fn an_app_that_keeps_nothing_gets_inline_images_and_no_copy() {
    let (app, dir) = app(|_| {}).await;
    keep_nothing(&app).await;
    let (s, _, v) = image(&app, &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["data"][0]["b64_json"].as_str().is_some_and(|b| !b.is_empty()), "the image itself: {v}");
    assert!(v["data"][0]["url"].is_null(), "no link to a copy: {v}");
    let id = v["id"].as_str().unwrap();

    let (s, _, gen) = call(&app, "GET", &format!("/v1/generations/{id}"), &[], None).await;
    assert_eq!(s, StatusCode::OK, "the call is still recorded: {gen}");
    assert_eq!(gen["status"], "completed", "{gen}");
    assert!(gen["result_url"].is_null(), "{gen}");
    assert!(gen["metadata"]["assets"].as_array().is_none_or(|a| a.is_empty()), "{gen}");
    assert_eq!(gen["metadata"]["keep_outputs"], false, "{gen}");
    assert!(files_for(&dir, id).is_empty(), "nothing written: {:?}", files_for(&dir, id));

    // The request log keeps no copy of the image either.
    let a = artifact(&app, id).await;
    assert_eq!(a["output_kind"], "not_kept", "{a}");
    assert!(a["output_value"].is_null(), "{a}");
}

#[tokio::test]
async fn the_header_keeps_nothing_for_one_request() {
    let (app, dir) = app(|_| {}).await;
    let (s, _, v) = image(&app, &[("x-litegen-keep-outputs", "false")]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let id = v["id"].as_str().unwrap();
    assert!(files_for(&dir, id).is_empty(), "nothing written: {:?}", files_for(&dir, id));
    assert_eq!(artifact(&app, id).await["output_kind"], "not_kept");

    // The next request, without the header, is kept as before.
    let (s, _, v) = image(&app, &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let id = v["id"].as_str().unwrap();
    assert_eq!(files_for(&dir, id).len(), 1, "a copy of the image");
    let (_, _, assets) = call(&app, "GET", &format!("/v1/generations/{id}/assets"), &[], None).await;
    assert_eq!(assets["data"].as_array().map(Vec::len), Some(1), "{assets}");
}

#[tokio::test]
async fn the_header_takes_only_false() {
    let (app, _dir) = app(|_| {}).await;
    for bad in ["true", "no", ""] {
        let (s, _, v) = image(&app, &[("x-litegen-keep-outputs", bad)]).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad:?}: {v}");
        assert_eq!(v["error"]["code"], "invalid_keep_outputs_header", "{bad:?}: {v}");
    }
    let (s, _, v) = image(&app, &[("x-litegen-keep-outputs", "FALSE")]).await;
    assert_eq!(s, StatusCode::OK, "any case: {v}");
}

#[tokio::test]
async fn keeping_nothing_bypasses_the_cache() {
    let (app, _dir) = app(|c| c.cache.enabled = true).await;
    keep_nothing(&app).await;
    let (s1, h1, v1) = image(&app, &[]).await;
    let (s2, h2, v2) = image(&app, &[]).await;
    assert_eq!((s1, s2), (StatusCode::OK, StatusCode::OK), "{v1} {v2}");
    assert_eq!(h1.get("x-litegen-cache").and_then(|h| h.to_str().ok()), Some("bypass"));
    assert_eq!(h2.get("x-litegen-cache").and_then(|h| h.to_str().ok()), Some("bypass"));
    assert_ne!(v1["id"], v2["id"], "the second answer is a fresh call, not a cached one");
}

/// An S3 image store that counts what it is asked to keep.
struct RecordingImageStore(AtomicUsize);

#[async_trait::async_trait]
impl ImageStore for RecordingImageStore {
    async fn store(&self, _data: &[u8], _content_type: &str, generation_id: &str) -> Result<String, ImageStoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(format!("https://bucket.example/litegen/images/{generation_id}.png"))
    }
}

#[tokio::test]
async fn keeping_nothing_skips_an_s3_image_store() {
    let dir = tempfile::tempdir().expect("asset dir").keep();
    let store = Arc::new(RecordingImageStore(AtomicUsize::new(0)));
    let (app, _db) = harness::app_with_image_store(&dir, Some(MASTER), store.clone(), |_| {}).await;
    let (s, _, v) = image(&app, &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(store.0.load(Ordering::SeqCst), 1, "kept, as before");
    assert!(v["data"][0]["url"].as_str().is_some_and(|u| u.starts_with("https://bucket.example/")), "{v}");

    keep_nothing(&app).await;
    let (s, _, v) = image(&app, &[]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(store.0.load(Ordering::SeqCst), 1, "nothing more was stored");
    assert!(v["data"][0]["url"].is_null(), "{v}");
    assert!(v["data"][0]["b64_json"].is_string(), "{v}");
}

// ─── Video ──────────────────────────────────────────────────────────────────

/// The mock vendor's own link for a finished `mock/video-gen`.
const VENDOR_VIDEO: &str = "https://example.com/mock-video.mp4";

async fn set_keep(app: &Router, keep: bool) {
    let (s, _, v) = call(app, "PUT", &format!("/v1/apps/{DEFAULT_APP_ID}/access"), &[], Some(serde_json::json!({ "keep_outputs": keep }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

async fn submit_video(app: &Router, model: &str, headers: &[(&str, &str)]) -> (StatusCode, serde_json::Value) {
    let (s, _, v) = call(app, "POST", "/v1/videos/generations", headers, Some(serde_json::json!({ "model": model, "prompt": "a wave" }))).await;
    (s, v)
}

async fn video_id(app: &Router, headers: &[(&str, &str)]) -> String {
    let (s, v) = submit_video(app, "mock/video-gen", headers).await;
    assert!(s.is_success(), "{s}: {v}");
    v["id"].as_str().unwrap().to_string()
}

/// Poll `GET /v1/videos/{id}` until it ends.
async fn video_outcome(app: &Router, id: &str) -> serde_json::Value {
    for _ in 0..40 {
        let (_, _, v) = call(app, "GET", &format!("/v1/videos/{id}"), &[], None).await;
        if v["status"] == "completed" || v["status"] == "failed" {
            return v;
        }
    }
    panic!("video {id} never ended");
}

#[tokio::test]
async fn a_video_that_keeps_nothing_answers_the_vendor_link() {
    let (app, dir) = app(|_| {}).await;
    keep_nothing(&app).await;
    let id = video_id(&app, &[]).await;
    let done = video_outcome(&app, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["video_url"], VENDOR_VIDEO, "{done}");
    let (_, _, gen) = call(&app, "GET", &format!("/v1/generations/{id}"), &[], None).await;
    assert_eq!(gen["metadata"]["keep_outputs"], false, "{gen}");
    assert_eq!(gen["metadata"]["assets"][0]["kept"], false, "{gen}");
    assert_eq!(gen["result_url"], VENDOR_VIDEO, "{gen}");
    assert!(files_for(&dir, &id).is_empty(), "nothing written: {:?}", files_for(&dir, &id));
}

#[tokio::test]
async fn the_header_keeps_nothing_for_one_video() {
    let (app, dir) = app(|_| {}).await;
    let id = video_id(&app, &[("x-litegen-keep-outputs", "false")]).await;
    assert_eq!(video_outcome(&app, &id).await["video_url"], VENDOR_VIDEO);
    assert!(files_for(&dir, &id).is_empty());
    let id = video_id(&app, &[]).await;
    let done = video_outcome(&app, &id).await;
    assert_ne!(done["video_url"], VENDOR_VIDEO, "kept, as before: {done}");
    assert_eq!(files_for(&dir, &id).len(), 1, "a copy of the video");
    let (s, v) = submit_video(&app, "mock/video-gen", &[("x-litegen-keep-outputs", "true")]).await;
    assert_eq!((s, v["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_keep_outputs_header")), "{v}");
}

#[tokio::test]
async fn the_decision_is_taken_at_submit() {
    let (app, dir) = app(|_| {}).await;
    keep_nothing(&app).await;
    let id = video_id(&app, &[]).await;
    set_keep(&app, true).await;
    assert_eq!(video_outcome(&app, &id).await["video_url"], VENDOR_VIDEO, "submitted keeping nothing");
    assert!(files_for(&dir, &id).is_empty());

    let id = video_id(&app, &[]).await;
    keep_nothing(&app).await;
    let done = video_outcome(&app, &id).await;
    assert_ne!(done["video_url"], VENDOR_VIDEO, "submitted keeping: {done}");
    assert_eq!(files_for(&dir, &id).len(), 1, "a copy of the video");
}

#[tokio::test]
async fn a_video_with_no_link_fails_at_once_when_keeping_nothing() {
    let (app, _dir) = app(|_| {}).await;
    keep_nothing(&app).await;
    let (s, v) = submit_video(&app, "mock/visual-video-gen", &[]).await;
    assert!(s.is_success(), "{s}: {v}");
    let done = video_outcome(&app, v["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "failed", "{done}");
    assert!(done["error"].as_str().is_some_and(|e| e.contains("keeps nothing")), "{done}");
}

// ─── 3D ─────────────────────────────────────────────────────────────────────

async fn submit_3d(app: &Router, formats: &[&str]) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({ "model": "mock/mesh-3d", "prompt": "a cube", "output_formats": formats });
    let (s, _, v) = call(app, "POST", "/v1/models3d/generations", &[], Some(body)).await;
    (s, v)
}

#[tokio::test]
async fn a_3d_request_for_a_converted_format_is_refused_when_keeping_nothing() {
    let (app, _dir) = app(|_| {}).await;
    let (s, v) = submit_3d(&app, &["obj"]).await;
    assert!(s.is_success(), "kept, litegen converts the vendor's glb: {s} {v}");
    keep_nothing(&app).await;
    let (s, v) = submit_3d(&app, &["obj"]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["error"]["code"], "format_needs_keeping", "{v}");
    assert!(v["error"]["message"].as_str().is_some_and(|m| m.contains("glb")), "names what the model makes: {v}");
}

/// The full app with Tripo and Meshy on `vendor` (wiremock), so a request
/// litegen lets through reaches it.
async fn app_with_rig_vendors(vendor: &wiremock::MockServer) -> (Router, std::path::PathBuf) {
    let base = vendor.uri();
    app(move |c| {
        for name in ["tripo", "meshy"] {
            c.providers.insert(
                name.into(),
                serde_json::from_value(serde_json::json!({ "enabled": true, "api_key": "vendor-key", "api_base": base })).unwrap(),
            );
        }
    })
    .await
}

/// A rig by `model` of an inline glb, promising `formats`, that keeps nothing.
async fn rig_keeping_nothing(app: &Router, model: &str, formats: &[&str], format_source: Option<&str>) -> (StatusCode, serde_json::Value) {
    use base64::Engine;
    let glb = litegen::providers::model3d::glb::generate_cube_glb("a knight");
    let mut body = serde_json::json!({
        "model": model,
        "output_formats": formats,
        "source_model": { "type": "base64", "value": base64::engine::general_purpose::STANDARD.encode(glb), "format": "glb" },
    });
    if let Some(source) = format_source {
        body["format_source"] = source.into();
    }
    let (s, _, v) = call(app, "POST", "/v1/models3d/generations", &[("x-litegen-keep-outputs", "false")], Some(body)).await;
    (s, v)
}

/// Tripo makes one container per job (its own fbx when one is promised), so
/// a request that keeps nothing can't be promised a glb and an fbx: litegen
/// would have to convert one. Refused before anything is sent or billed.
#[tokio::test]
async fn two_containers_from_a_one_container_vendor_are_refused_when_keeping_nothing() {
    let vendor = wiremock::MockServer::start().await;
    let (app, _dir) = app_with_rig_vendors(&vendor).await;
    let (s, v) = rig_keeping_nothing(&app, "tripo/rig", &["glb", "fbx"], None).await;
    assert_eq!((s, v["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("format_needs_keeping")), "{v}");
    assert!(v["error"]["message"].as_str().is_some_and(|m| m.contains("returns only fbx")), "names what Tripo returns: {v}");
    assert!(vendor.received_requests().await.unwrap().is_empty(), "nothing reached the vendor");
}

/// With `format_source: glb`, litegen makes the fbx from the vendor's glb;
/// keeping nothing, it makes nothing, so Meshy's rig refuses the fbx up
/// front, though Meshy returns an fbx of its own beside its glb.
#[tokio::test]
async fn an_fbx_made_from_the_glb_is_refused_when_keeping_nothing() {
    let vendor = wiremock::MockServer::start().await;
    let (app, _dir) = app_with_rig_vendors(&vendor).await;
    let (s, v) = rig_keeping_nothing(&app, "meshy/rig", &["fbx"], Some("glb")).await;
    assert_eq!((s, v["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("format_needs_keeping")), "{v}");
    assert!(v["error"]["message"].as_str().is_some_and(|m| m.contains("format_source")), "says why: {v}");
    assert!(vendor.received_requests().await.unwrap().is_empty(), "nothing reached the vendor");
}

/// Meshy's rig returns a glb and an fbx from one job, so a request that
/// keeps nothing can be promised both.
#[tokio::test]
async fn a_vendor_job_that_returns_a_glb_and_an_fbx_can_keep_nothing_of_both() {
    let vendor = wiremock::MockServer::start().await;
    let (app, _dir) = app_with_rig_vendors(&vendor).await;
    let (s, v) = rig_keeping_nothing(&app, "meshy/rig", &["glb", "fbx"], None).await;
    assert_ne!(v["error"]["code"], "format_needs_keeping", "{s}: {v}");
}

#[tokio::test]
async fn a_3d_job_that_keeps_nothing_hands_out_the_vendors_files() {
    let (app, dir) = app(|_| {}).await;
    keep_nothing(&app).await;
    let (s, v) = submit_3d(&app, &["glb"]).await;
    assert!(s.is_success(), "{s} {v}");
    let id = v["id"].as_str().unwrap().to_string();
    let mut done = serde_json::Value::Null;
    for _ in 0..20 {
        let (_, _, v) = call(&app, "GET", &format!("/v1/models3d/{id}"), &[], None).await;
        if v["status"] == "completed" || v["status"] == "failed" {
            done = v;
            break;
        }
    }
    assert_eq!(done["status"], "completed", "{done}");
    let assets = done["assets"].as_array().expect("assets");
    assert!(assets.iter().any(|a| a["kind"] == "mesh"), "{done}");
    for a in assets {
        assert!(a["url"].as_str().is_some_and(|u| u.starts_with("https://example.com/mock-3d/")), "the vendor's link: {a}");
        assert_eq!(a["kept"], false, "{a}");
        assert!(a["size_bytes"].as_u64().is_some_and(|n| n > 0), "the size its link check found, not a download: {a}");
    }
    assert!(files_for(&dir, &id).is_empty(), "nothing written: {:?}", files_for(&dir, &id));
}

/// The poller, not a client's poll, sees a keep-nothing 3D job finish: it
/// hands out the vendor's files with the sizes their link checks found, and
/// writes nothing.
#[tokio::test]
async fn a_3d_job_the_poller_finishes_keeping_nothing_hands_out_the_vendors_files() {
    use litegen::db::DatabaseStore;
    let dir = tempfile::tempdir().expect("asset dir").keep();
    let db = Arc::new(litegen::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_in(db.clone(), &dir, |_| {}, Some(MASTER), litegen::config::Mode::SingleTenant).await;
    let body = serde_json::json!({ "model": "mock/mesh-3d", "prompt": "a cube", "output_formats": ["glb"] });
    let (s, _, v) = call(&app, "POST", "/v1/models3d/generations", &[("x-litegen-keep-outputs", "false")], Some(body)).await;
    assert!(s.is_success(), "{s} {v}");
    let id = v["id"].as_str().unwrap().to_string();
    for _ in 0..20 {
        litegen::proxy::poller::poll_once_presenting(&state.db, &state.router.registry, state.secrets_key, state.mode, &state.router.model3d_store, None).await;
        if db.get_generation(&id).await.unwrap().unwrap().status.is_terminal() {
            break;
        }
    }
    let (_, _, done) = call(&app, "GET", &format!("/v1/models3d/{id}"), &[], None).await;
    assert_eq!(done["status"], "completed", "{done}");
    let assets = done["assets"].as_array().expect("assets");
    assert!(assets.iter().any(|a| a["kind"] == "mesh"), "{done}");
    for a in assets {
        assert!(a["url"].as_str().is_some_and(|u| u.starts_with("https://example.com/mock-3d/")), "the vendor's link: {a}");
        assert_eq!(a["kept"], false, "{a}");
        assert!(a["size_bytes"].as_u64().is_some_and(|n| n > 0), "the size its link check found, not a download: {a}");
    }
    assert!(files_for(&dir, &id).is_empty(), "nothing written: {:?}", files_for(&dir, &id));
}

// ─── Handing out a vendor's file ────────────────────────────────────────────

/// A finished video of the default app whose file litegen did not keep: the
/// vendor's link is `vendor_url`.
async fn kept_nothing_video(db: &litegen::db::sqlite::SqliteDatabase, id: &str, vendor_url: &str) {
    use litegen::db::DatabaseStore;
    db.insert_generation(id, None, "mock/video-gen", "mock", "video", None, 0.0, Some(litegen::api::middleware::DEFAULT_ORG_ID), Some(DEFAULT_APP_ID)).await.unwrap();
    let meta = serde_json::json!({
        "keep_outputs": false,
        "assets": [{ "index": 0, "kind": "video", "format": "mp4", "url": vendor_url, "kept": false }],
    });
    db.update_generation_if_active(id, "completed", 100, Some(vendor_url), None, Some(chrono::Utc::now()), Some(&meta)).await.unwrap();
}

/// The app with vendor files fetched from any host (wiremock is on 127.0.0.1).
async fn app_reaching(mode: &str) -> (Router, Arc<litegen::db::sqlite::SqliteDatabase>) {
    let dir = tempfile::tempdir().expect("asset dir").keep();
    let mode = mode.to_string();
    harness::app_with_hooks_tweaked(&dir, Some(MASTER), Vec::new(), move |c| {
        c.secrets_key = Some(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [3u8; 32]));
        c.storage_access.mode = mode.parse().unwrap();
        c.storage_access.unchecked_vendor_hosts = true;
    })
    .await
}

async fn get_anonymously(app: &Router, uri: &str) -> (StatusCode, axum::http::HeaderMap, bytes::Bytes) {
    let resp = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    (status, headers, axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap())
}

async fn vendor() -> wiremock::MockServer {
    use wiremock::matchers::{method, path};
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/out/clip.mp4"))
        .respond_with(wiremock::ResponseTemplate::new(200).insert_header("content-type", "video/mp4").set_body_bytes(b"vendor-video-bytes".to_vec()))
        .mount(&server)
        .await;
    wiremock::Mock::given(method("GET"))
        .and(path("/out/gone.mp4"))
        .respond_with(wiremock::ResponseTemplate::new(404))
        .mount(&server)
        .await;
    server
}

fn path_of(url: &str) -> &str {
    url.find("/v1/").map_or(url, |i| &url[i..])
}

#[tokio::test]
async fn a_private_app_streams_the_vendor_file() {
    let server = vendor().await;
    let (app, db) = app_reaching("private").await;
    let link = format!("{}/out/clip.mp4", server.uri());
    kept_nothing_video(&db, "litegen-vid-kn-private", &link).await;
    let (s, _, v) = call(&app, "GET", "/v1/videos/litegen-vid-kn-private", &[], None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let url = v["video_url"].as_str().unwrap();
    assert!(url.ends_with("/v1/generations/litegen-vid-kn-private/assets/0"), "the file route: {v}");
    assert!(!v.to_string().contains(&server.uri()), "the vendor's link never leaves litegen: {v}");
    let req = Request::get(path_of(url)).header("authorization", format!("Bearer {MASTER}")).body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(resp.headers().get("content-security-policy").unwrap(), "sandbox");
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    assert_eq!(&body[..], b"vendor-video-bytes");
}

#[tokio::test]
async fn a_signed_app_gets_a_litegen_link_to_the_vendor_file() {
    let server = vendor().await;
    let (app, db) = app_reaching("signed").await;
    kept_nothing_video(&db, "litegen-vid-kn-signed", &format!("{}/out/clip.mp4", server.uri())).await;
    let (_, _, v) = call(&app, "GET", "/v1/videos/litegen-vid-kn-signed", &[], None).await;
    let url = v["video_url"].as_str().unwrap();
    assert!(url.contains("/assets/0/signed?"), "litegen's own link: {v}");
    assert!(v["video_url_expires_at"].is_string(), "{v}");
    assert!(!v.to_string().contains(&server.uri()), "{v}");
    let (s, _, body) = get_anonymously(&app, path_of(url)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(&body[..], b"vendor-video-bytes");
}

#[tokio::test]
async fn a_public_app_hands_out_the_vendor_link_itself() {
    let server = vendor().await;
    let (app, db) = app_reaching("public").await;
    let link = format!("{}/out/clip.mp4", server.uri());
    kept_nothing_video(&db, "litegen-vid-kn-public", &link).await;
    let (_, _, v) = call(&app, "GET", "/v1/videos/litegen-vid-kn-public", &[], None).await;
    assert_eq!(v["video_url"], link, "{v}");
}

#[tokio::test]
async fn a_vendor_file_that_is_gone_answers_410() {
    let server = vendor().await;
    let (app, db) = app_reaching("private").await;
    kept_nothing_video(&db, "litegen-vid-kn-gone", &format!("{}/out/gone.mp4", server.uri())).await;
    let (s, _, v) = call(&app, "GET", "/v1/generations/litegen-vid-kn-gone/assets/0", &[], None).await;
    assert_eq!(s, StatusCode::GONE, "{v}");
    assert_eq!(v["error"]["code"], "vendor_file_gone", "{v}");
}

#[tokio::test]
async fn a_vendor_link_to_a_private_address_is_not_fetched() {
    // Without the test switch, litegen fetches vendor files from public hosts only.
    let dir = tempfile::tempdir().unwrap().keep();
    let (app, db) = harness::app_with_hooks_tweaked(&dir, Some(MASTER), Vec::new(), |c| c.storage_access.mode = litegen::access::AccessMode::Private).await;
    kept_nothing_video(&db, "litegen-vid-kn-ssrf", "http://127.0.0.1:9/secret.mp4").await;
    let (s, _, v) = call(&app, "GET", "/v1/generations/litegen-vid-kn-ssrf/assets/0", &[], None).await;
    assert_eq!(s, StatusCode::BAD_GATEWAY, "{v}");
    assert!(v["error"]["message"].as_str().is_some_and(|m| m.contains("public")), "{v}");
}

// ─── Handing out a vendor's 3D file ─────────────────────────────────────────

/// A finished 3D generation of the default app whose mesh litegen did not
/// keep: the vendor's link is `vendor_url`.
async fn kept_nothing_mesh(db: &litegen::db::sqlite::SqliteDatabase, id: &str, vendor_url: &str) {
    use litegen::db::DatabaseStore;
    db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", None, 0.0, Some(litegen::api::middleware::DEFAULT_ORG_ID), Some(DEFAULT_APP_ID)).await.unwrap();
    let meta = serde_json::json!({
        "keep_outputs": false,
        "requested_formats": ["glb"],
        "assets": [{ "kind": "mesh", "format": "glb", "url": vendor_url, "size_bytes": 17, "kept": false }],
    });
    db.update_generation_if_active(id, "completed", 100, Some(vendor_url), None, Some(chrono::Utc::now()), Some(&meta)).await.unwrap();
}

/// [`vendor`], also serving a mesh at `/out/model.glb` and a gone one at
/// `/out/gone.glb`.
async fn mesh_vendor() -> wiremock::MockServer {
    use wiremock::matchers::{method, path};
    let server = vendor().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/out/model.glb"))
        .respond_with(wiremock::ResponseTemplate::new(200).insert_header("content-type", "model/gltf-binary").set_body_bytes(b"vendor-mesh-bytes".to_vec()))
        .mount(&server)
        .await;
    wiremock::Mock::given(method("GET")).and(path("/out/gone.glb")).respond_with(wiremock::ResponseTemplate::new(404)).mount(&server).await;
    server
}

/// A 3D generation that kept nothing lists its files as the vendor's:
/// `GET /v1/generations/{id}/assets` says `kept: false` on each, as its
/// `GET /v1/models3d/{id}` answer does.
#[tokio::test]
async fn a_3d_generation_that_keeps_nothing_lists_its_files_as_not_kept() {
    let (app, _dir) = app(|_| {}).await;
    keep_nothing(&app).await;
    let (s, v) = submit_3d(&app, &["glb"]).await;
    assert!(s.is_success(), "{s} {v}");
    let id = v["id"].as_str().unwrap().to_string();
    for _ in 0..20 {
        let (_, _, v) = call(&app, "GET", &format!("/v1/models3d/{id}"), &[], None).await;
        if v["status"] == "completed" || v["status"] == "failed" {
            assert_eq!(v["status"], "completed", "{v}");
            break;
        }
    }
    let (s, _, listed) = call(&app, "GET", &format!("/v1/generations/{id}/assets"), &[], None).await;
    assert_eq!(s, StatusCode::OK, "{listed}");
    let files = listed["data"].as_array().expect("the files");
    assert!(!files.is_empty(), "{listed}");
    for a in files {
        assert_eq!(a["kept"], false, "the vendor's file, not one litegen kept: {a}");
        assert!(a["url"].as_str().is_some_and(|u| u.starts_with("https://example.com/mock-3d/")), "the vendor's link: {a}");
    }
}

/// A private app's 3D generation that kept nothing is served by litegen's
/// file route, which streams the vendor's mesh as it does a vendor's video:
/// the vendor's link never leaves litegen.
#[tokio::test]
async fn a_private_app_streams_the_vendors_3d_file() {
    let server = mesh_vendor().await;
    let (app, db) = app_reaching("private").await;
    kept_nothing_mesh(&db, "litegen-3d-kn-private", &format!("{}/out/model.glb", server.uri())).await;
    let (s, _, v) = call(&app, "GET", "/v1/models3d/litegen-3d-kn-private", &[], None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let url = v["assets"][0]["url"].as_str().unwrap();
    assert!(url.ends_with("/v1/generations/litegen-3d-kn-private/assets/0"), "the file route: {v}");
    assert!(!v.to_string().contains(&server.uri()), "the vendor's link never leaves litegen: {v}");
    let req = Request::get(path_of(url)).header("authorization", format!("Bearer {MASTER}")).body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "sent on to {:?}", resp.headers().get("location"));
    assert_eq!(resp.headers().get("x-content-type-options").unwrap(), "nosniff");
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    assert_eq!(&body[..], b"vendor-mesh-bytes");
}

/// A signed app gets litegen's own signed link to a 3D generation's vendor
/// mesh, which streams it to anyone holding the link.
#[tokio::test]
async fn a_signed_app_gets_a_litegen_link_to_the_vendors_3d_file() {
    let server = mesh_vendor().await;
    let (app, db) = app_reaching("signed").await;
    kept_nothing_mesh(&db, "litegen-3d-kn-signed", &format!("{}/out/model.glb", server.uri())).await;
    let (_, _, v) = call(&app, "GET", "/v1/models3d/litegen-3d-kn-signed", &[], None).await;
    let url = v["assets"][0]["url"].as_str().unwrap();
    assert!(url.contains("/assets/0/signed?"), "litegen's own link: {v}");
    assert!(!v.to_string().contains(&server.uri()), "{v}");
    let (s, headers, body) = get_anonymously(&app, path_of(url)).await;
    assert_eq!(s, StatusCode::OK, "sent on to {:?}", headers.get("location"));
    assert_eq!(&body[..], b"vendor-mesh-bytes");
}

/// The vendor deleted a 3D generation's mesh litegen did not keep: its file
/// route answers 410, as for a video.
#[tokio::test]
async fn a_vendors_3d_file_that_is_gone_answers_410() {
    let server = mesh_vendor().await;
    let (app, db) = app_reaching("private").await;
    kept_nothing_mesh(&db, "litegen-3d-kn-gone", &format!("{}/out/gone.glb", server.uri())).await;
    let (s, headers, v) = call(&app, "GET", "/v1/generations/litegen-3d-kn-gone/assets/0", &[], None).await;
    assert_eq!(s, StatusCode::GONE, "{v} (sent on to {:?})", headers.get("location"));
    assert_eq!(v["error"]["code"], "vendor_file_gone", "{v}");
}

// ─── A cached answer's links ────────────────────────────────────────────────

#[tokio::test]
async fn a_cached_image_answer_in_a_private_app_uses_the_file_route() {
    let dir = tempfile::tempdir().expect("asset dir").keep();
    let store = Arc::new(RecordingImageStore(AtomicUsize::new(0)));
    let (app, _db) = harness::app_with_image_store(&dir, Some(MASTER), store.clone(), |c| {
        c.cache.enabled = true;
        c.storage_access.mode = litegen::access::AccessMode::Private;
    })
    .await;
    let (s, h, first) = image(&app, &[]).await;
    assert_eq!(s, StatusCode::OK, "{first}");
    assert_eq!(h.get("x-litegen-cache").and_then(|v| v.to_str().ok()), Some("miss"));
    let id = first["id"].as_str().unwrap();
    let route = format!("/v1/generations/{id}/assets/0");
    assert!(first["data"][0]["url"].as_str().is_some_and(|u| u.ends_with(&route)), "{first}");

    let (s, h, hit) = image(&app, &[]).await;
    assert_eq!(s, StatusCode::OK, "{hit}");
    assert_eq!(h.get("x-litegen-cache").and_then(|v| v.to_str().ok()), Some("hit"));
    let url = hit["data"][0]["url"].as_str().unwrap();
    assert!(url.ends_with(&route), "the first generation's file route, never the bucket's URL: {url}");
    assert!(!url.contains("bucket.example"), "{url}");
}

/// A video model whose vendor returns only bytes (Google Veo: its link needs
/// the key; Bedrock: an `s3://` URI) has no link litegen could hand out, so
/// a request that keeps nothing is refused before the vendor is called and
/// bills — not failed after (known-gaps: "a video that comes back only as
/// bytes fails a keep-nothing generation").
#[tokio::test]
async fn a_video_model_with_no_link_to_hand_out_refuses_keeping_nothing_up_front() {
    let vendor = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({ "name": "models/veo/operations/op1" })))
        .mount(&vendor)
        .await;
    let uri = vendor.uri();
    let (app, _dir) = app(move |cfg| {
        cfg.providers.insert(
            "google".into(),
            serde_json::from_value(serde_json::json!({ "enabled": true, "api_key": "g-key", "api_base": uri })).unwrap(),
        );
    })
    .await;
    let row = serde_json::json!({
        "id": "google/veo-3.0-generate-001",
        "provider": "google",
        "media_type": "video",
        "display_name": "Veo 3",
        "pricing": { "base_cost_usd": 0.4 },
        "capabilities": { "text_to_video": true },
        "prompt": { "required": true, "max_length": 1000 },
        "params": {}
    });
    let (s, _, v) = call(&app, "POST", "/v1/admin/models", &[], Some(row)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");

    let (s, v) = submit_video(&app, "google/veo-3.0-generate-001", &[("x-litegen-keep-outputs", "false")]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["error"]["code"], "keep_outputs_unsupported", "{v}");
    assert!(vendor.received_requests().await.unwrap().is_empty(), "nothing reached the vendor");

    // Keeping, it goes ahead.
    let (s, v) = submit_video(&app, "google/veo-3.0-generate-001", &[]).await;
    assert!(s.is_success(), "{s}: {v}");
}

/// OpenAI's Sora hands its video out at `/videos/{id}/content`, which answers
/// only a call with the vendor's key: no link litegen could hand out, so a
/// request that keeps nothing is refused before Sora is called, as for Veo.
/// No catalog row uses the adapter (Sora left the catalog), so the row here
/// is an operator's.
#[tokio::test]
async fn a_sora_video_refuses_keeping_nothing_up_front() {
    let vendor = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": "video_1", "status": "queued" })))
        .mount(&vendor)
        .await;
    let uri = vendor.uri();
    let (app, _dir) = app(move |cfg| {
        cfg.providers.insert(
            "openai".into(),
            serde_json::from_value(serde_json::json!({ "enabled": true, "api_key": "sk-openai", "api_base": uri })).unwrap(),
        );
    })
    .await;
    let row = serde_json::json!({
        "id": "openai/sora-2",
        "provider": "openai",
        "media_type": "video",
        "display_name": "Sora 2",
        "pricing": { "base_cost_usd": 0.5 },
        "capabilities": { "text_to_video": true },
        "prompt": { "required": true, "max_length": 4000 },
        "params": {}
    });
    let (s, _, v) = call(&app, "POST", "/v1/admin/models", &[], Some(row)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");

    let (s, v) = submit_video(&app, "openai/sora-2", &[("x-litegen-keep-outputs", "false")]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["error"]["code"], "keep_outputs_unsupported", "{v}");
    assert!(vendor.received_requests().await.unwrap().is_empty(), "nothing reached Sora");

    // Keeping, it goes ahead.
    let (s, v) = submit_video(&app, "openai/sora-2", &[]).await;
    assert!(s.is_success(), "{s}: {v}");
}
