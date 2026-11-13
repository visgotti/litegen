//! Every adapter honors the operator's provider config
//! (docs/superpowers/specs/2026-11-03-provider-config-design.md): each
//! catalog row's generation goes to one mock vendor (`api_base`), and every
//! request it makes there carries the operator's `extra_headers` and the
//! adapter's auth, and names the model the operator mapped the row to.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::capabilities::{MediaType, ModelSchema, ModelTask, RefProviderFormat};
use crate::proxy::materializer::{MaterializedRef, MaterializedRefForm, MaterializedRequest};
use crate::proxy::registry::{
    build_image_provider, build_model3d_provider, build_video_provider, IMAGE_PROVIDERS, MODEL3D_PROVIDERS,
    VIDEO_PROVIDERS,
};
use crate::providers::{ImageExtras, Model3dExtras, ProviderCredentials, ProviderInstanceConfig, VideoExtras};
use crate::types::BaseGenerationRequest;

const HEADER: &str = "x-litegen-conformance";
const MAPPED: &str = "litegen-mapped-model";
/// A reference image the mock vendor serves, for adapters that fetch one.
const REF_PATH: &str = "/conformance/ref.png";
/// Where the mock vendor has adapters upload a reference (a presigned
/// storage URL): not the vendor's API, so it never gets `extra_headers`.
const UPLOAD_PATH: &str = "/conformance/upload";
/// The smallest valid PNG (1×1, transparent).
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49,
    0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0x00, 0x02, 0x00, 0x00, 0x05, 0x00, 0x01, 0x7a, 0x5e, 0xab, 0x3f, 0x00,
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

use crate::providers::NO_MODEL_ID;

fn catalog() -> crate::capabilities::CapabilityRegistry {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.push("models");
    crate::capabilities::CapabilityRegistry::from_dir(&p).expect("the shipped catalog loads")
}

fn config(server: &MockServer, model_id: &str) -> ProviderInstanceConfig {
    let credentials = ProviderCredentials {
        api_key: Some("conformance-key".into()),
        key_id: Some("AKIDCONFORMANCE".into()),
        key_secret: Some("conformance-secret".into()),
        region: Some("us-east-1".into()),
        // Bedrock video writes its output to the operator's bucket.
        extra: [("s3_output_uri".to_string(), "s3://conformance/out/".to_string())].into_iter().collect(),
        ..Default::default()
    };
    ProviderInstanceConfig {
        api_key: "conformance-key".into(),
        api_base: Some(server.uri()),
        extra_headers: [(HEADER.to_string(), "yes".to_string())].into_iter().collect(),
        model_mapping: [(model_id.to_string(), MAPPED.to_string())].into_iter().collect(),
        options: Some(json!({ "platform_api_base": server.uri(), "sync_api_base": server.uri() })),
        credentials,
        ..Default::default()
    }
}

/// The refs a row's required roles need, in the form its vendor takes.
fn refs(row: &ModelSchema, server: &MockServer) -> Option<MaterializedRequest> {
    let mut refs = Vec::new();
    if let Some(spec) = &row.ref_inputs {
        let c = &row.capabilities;
        let text_mode = match row.media_type {
            MediaType::Image => c.text_to_image,
            MediaType::Video => c.text_to_video,
            _ => c.text_to_3d,
        };
        let mut roles: Vec<(&String, u32)> =
            spec.roles.iter().filter(|(_, r)| r.required).map(|(k, r)| (k, r.min_count.max(1))).collect();
        // A row with no text mode needs its start image even where the role
        // isn't required (it also takes other roles).
        if roles.is_empty() && !text_mode {
            if let Some((role, _)) = spec.roles.iter().find(|(k, _)| *k == "first_frame").or_else(|| spec.roles.iter().find(|(k, _)| *k == "init")) {
                roles.push((role, 1));
            }
        }
        roles.sort_by(|a, b| a.0.cmp(b.0));
        let form = |role: &str| match &spec.provider_format {
            RefProviderFormat::Url => MaterializedRefForm::Url(format!("{}{REF_PATH}", server.uri())),
            RefProviderFormat::Base64 => MaterializedRefForm::Base64(base64_png()),
            RefProviderFormat::Multipart(m) => MaterializedRefForm::MultipartField {
                field_name: m.field_map.get(role).cloned().unwrap_or_else(|| role.to_string()),
                bytes: bytes::Bytes::from_static(PNG),
                content_type: "image/png".into(),
            },
        };
        for (role, count) in roles {
            if !matches!(role.as_str(), "init" | "first_frame" | "reference") && !role.starts_with("view-") {
                return None; // a mask, a source video or a 3D source: not a plain generation
            }
            for _ in 0..count {
                refs.push(MaterializedRef { role: role.clone(), form: form(role), time_s: None });
            }
        }
        // An edit row with no text mode whose mask is optional (a region can
        // stand in for it) is sent one: fal's fill and erase refuse a request
        // without `mask_url`.
        if !text_mode && c.inpainting && spec.roles.get("mask").is_some_and(|m| !m.required) {
            refs.push(MaterializedRef { role: "mask".into(), form: form("mask"), time_s: None });
        }
    }
    Some(MaterializedRequest { refs, ..Default::default() })
}

fn base64_png() -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(PNG)
}

fn base(row: &ModelSchema) -> BaseGenerationRequest {
    let prompt = if row.prompt.max_length == Some(0) { "" } else { "a lighthouse at dusk" };
    serde_json::from_value(json!({ "prompt": prompt, "model": row.id })).unwrap()
}

/// What a row's own params need for a request to be made at all: an
/// outpainting row's `expand` (one that adds nothing is refused).
fn image_extras(row: &ModelSchema) -> ImageExtras {
    ImageExtras {
        vendor_job: Default::default(),
        size: None, resolution: None, aspect_ratio: None, quality: None, style: None, steps: None,
        guidance_scale: None, strength: None, response_format: "b64_json".to_string(),
        edit: None, views: Vec::new(), extra: None, output_format: None, background: None,
        expand: row.params.contains_key("expand").then_some(crate::types::Expand { left: 64, ..Default::default() }),
        layout: None, mask_prompt: None, preserve_unmasked: None, feather_px: None,
        palette: None, directions: None,
    }
}

fn video_extras(row: &ModelSchema) -> VideoExtras {
    VideoExtras {
        duration_seconds: crate::api::middleware::validator::resolve_duration_seconds(row, None),
        aspect_ratio: None, resolution: None, fps: None, extra: None,
        camera: None, audio: None, r#loop: None,
        source: None, direction: None, time_range: None, replace: None,
    }
}

fn model3d_extras() -> Model3dExtras {
    Model3dExtras {
        output_formats: vec!["glb".into()],
        texture: None, pbr: None, target_polycount: None, symmetry: None, topology: None, rig: None,
        skeleton: None, joint_naming: None, height_m: None,
        extra: None, source: None, preflight: None, fbx_color_space: None, fbx_light_units: None, usd_light_units: None, dae_morphs: None, usdz_root: None, usd_clips: None, format_source: None,output_packages: None, ktx2_textures: None,
        animations: Vec::new(), fps: None,
        texture_resolution: None, keep_uvs: None,
        ground: None, center: None,
    }
}

/// What one row's generation sent the mock vendor.
struct Sent {
    row: String,
    provider: String,
    requests: Vec<wiremock::Request>,
    /// The adapter's answer, for the report.
    outcome: String,
}

/// Run `row`'s generation against a fresh mock vendor that answers every
/// call 503, and keep what reached it (the reference image excepted).
async fn send(row: &ModelSchema) -> Option<Sent> {
    send_answering(row, ResponseTemplate::new(503).set_body_json(json!({ "error": "conformance" }))).await
}

/// [`send`], with the vendor's API answering every call with `answer`.
async fn send_answering(row: &ModelSchema, answer: ResponseTemplate) -> Option<Sent> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REF_PATH))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "image/png").set_body_bytes(PNG))
        .mount(&server)
        .await;
    // Leonardo uploads a reference first: its API hands out a presigned URL.
    Mock::given(method("POST"))
        .and(path("/init-image"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "uploadInitImage": { "id": "init-1", "url": format!("{}{UPLOAD_PATH}", server.uri()), "fields": "{\"key\":\"k\"}" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST")).and(path(UPLOAD_PATH)).respond_with(ResponseTemplate::new(204)).mount(&server).await;
    Mock::given(wiremock::matchers::any()).respond_with(answer).mount(&server).await;
    let materialized = refs(row, &server)?;
    let cfg = config(&server, &row.id);
    let base = base(row);
    let wait = Duration::from_secs(20);
    let outcome = match row.media_type {
        MediaType::Image => {
            let p = build_image_provider(&row.provider, &cfg)?;
            format!("{:?}", tokio::time::timeout(wait, p.generate(row, &base, &image_extras(row), &materialized)).await.map(|r| r.err()))
        }
        MediaType::Video => {
            let p = build_video_provider(&row.provider, &cfg)?;
            format!("{:?}", tokio::time::timeout(wait, p.generate(row, &base, &video_extras(row), &materialized)).await.map(|r| r.err()))
        }
        MediaType::Model3d => {
            let p = build_model3d_provider(&row.provider, &cfg)?;
            format!("{:?}", tokio::time::timeout(wait, p.generate(row, &base, &model3d_extras(), &materialized)).await.map(|r| r.err()))
        }
    };
    let requests = server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path() != REF_PATH)
        .collect();
    Some(Sent { row: row.id.clone(), provider: row.provider.clone(), requests, outcome })
}

/// Adapters registered without a catalog row (no model is served through
/// them today), with one stand-in row each so they are held to the same
/// rules: OpenAI and Google video (Bedrock image has its Stability rows since
/// 2026-11-04, Bedrock video its Luma Ray 2 row since 2026-11-10).
fn uncatalogued_rows() -> Vec<ModelSchema> {
    let yaml = r#"
models:
  - id: openai/sora-2
    provider: openai
    media_type: video
    display_name: Sora 2
    pricing: { base_cost_usd: 0.4 }
    capabilities: { text_to_video: true }
    prompt: { required: true, max_length: 1000 }
    params: {}
  - id: google/veo-3.0-generate-001
    provider: google
    media_type: video
    display_name: Veo 3
    pricing: { base_cost_usd: 0.4 }
    capabilities: { text_to_video: true }
    prompt: { required: true, max_length: 1000 }
    params: {}
"#;
    let r = crate::capabilities::CapabilityRegistry::from_yaml_strs(&[("uncatalogued.yaml", yaml)]).expect("stand-in rows load");
    let mut rows: Vec<ModelSchema> = r.all().cloned().collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows
}

/// Every plain-generation catalog row of every adapter but the mocks, and
/// the stand-ins for adapters without one.
fn rows() -> Vec<ModelSchema> {
    let mut rows: Vec<ModelSchema> = catalog()
        .all()
        .filter(|r| r.provider != "mock" && r.task == ModelTask::Generate)
        .filter(|r| {
            let names = match r.media_type {
                MediaType::Image => IMAGE_PROVIDERS,
                MediaType::Video => VIDEO_PROVIDERS,
                MediaType::Model3d => MODEL3D_PROVIDERS,
            };
            names.contains(&r.provider.as_str())
        })
        .cloned()
        .collect();
    rows.extend(uncatalogued_rows());
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows
}

async fn sent_by_every_row() -> Vec<Sent> {
    let mut all = Vec::new();
    for row in rows() {
        if let Some(sent) = send(&row).await {
            all.push(sent);
        }
    }
    all
}

fn has_auth(r: &wiremock::Request) -> bool {
    ["authorization", "x-key", "x-api-key", "api-key", "x-goog-api-key"].iter().any(|h| r.headers.contains_key(*h))
        || r.url.query_pairs().any(|(k, _)| k == "key")
}

#[tokio::test]
async fn every_adapter_sends_extra_headers_and_auth_to_its_api() {
    let sent = sent_by_every_row().await;
    let mut problems = Vec::new();
    let mut reached: std::collections::BTreeSet<(String, String)> = Default::default();
    for s in &sent {
        if s.requests.is_empty() {
            problems.push(format!("{}: no request reached the configured api_base ({})", s.row, s.outcome));
            continue;
        }
        reached.insert((s.provider.clone(), s.row.clone()));
        for r in &s.requests {
            if r.url.path() == UPLOAD_PATH {
                if r.headers.contains_key(HEADER) {
                    problems.push(format!("{}: the upload to presigned storage carries {HEADER}", s.row));
                }
                continue;
            }
            if r.headers.get(HEADER).map(|v| v.as_bytes()) != Some(b"yes".as_slice()) {
                problems.push(format!("{}: {} {} has no {HEADER}", s.row, r.method, r.url.path()));
            }
            if !has_auth(r) {
                problems.push(format!("{}: {} {} has no auth", s.row, r.method, r.url.path()));
            }
        }
    }
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
    assert!(sent.len() > 40, "only {} rows ran", sent.len());
}

#[tokio::test]
async fn every_adapter_sends_the_mapped_model_id() {
    let sent = sent_by_every_row().await;
    let exempt: HashMap<&str, &str> = NO_MODEL_ID.iter().copied().collect();
    let mut problems = Vec::new();
    for s in &sent {
        if exempt.contains_key(s.provider.as_str()) || exempt.contains_key(s.row.as_str()) || s.requests.is_empty() {
            continue;
        }
        let named = s.requests.iter().any(|r| {
            r.url.as_str().contains(MAPPED) || String::from_utf8_lossy(&r.body).contains(MAPPED)
        });
        if !named {
            let first = &s.requests[0];
            problems.push(format!("{}: no request names {MAPPED} (first: {} {})", s.row, first.method, first.url));
        }
    }
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
}

/// Adapter sources, outside their `#[cfg(test)]` modules.
fn adapter_sources() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/providers");
    let mut out = Vec::new();
    for family in ["image", "video", "model3d"] {
        let mut files: Vec<_> = std::fs::read_dir(root.join(family)).unwrap().flatten().map(|e| e.path()).collect();
        files.retain(|p| p.is_file() && p.extension().is_some_and(|e| e == "rs"));
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            let shipped = text.split("#[cfg(test)]").next().unwrap_or_default().to_string();
            out.push((format!("{family}/{}", f.file_name().unwrap().to_string_lossy()), shipped));
        }
    }
    out
}

#[test]
fn no_adapter_writes_its_own_auth_header() {
    let offenders: Vec<String> = adapter_sources()
        .into_iter()
        .filter(|(_, src)| src.contains(".header(\"Authorization\"") || src.contains(".bearer_auth("))
        .map(|(name, _)| name)
        .collect();
    assert!(offenders.is_empty(), "auth written by hand instead of AuthSpec + auth::apply: {offenders:?}");
}

#[test]
fn result_downloads_never_use_the_api_client() {
    // A download through the API client would carry the operator's
    // extra_headers (and skip the public-address check) to a CDN.
    let offenders: Vec<String> = adapter_sources()
        .into_iter()
        .filter(|(_, src)| {
            src.contains("self.client.get(url).send()") || src.contains("self.client.get(u).send()")
        })
        .map(|(name, _)| name)
        .collect();
    assert!(offenders.is_empty(), "result downloads through the API client: {offenders:?}");
}

/// A vendor's rate limit reaches the retry policy with the vendor's own wait
/// (docs/gateway-settings.md): every submit answered 429 with `Retry-After`
/// is `RateLimited` carrying it, so a retry waits at least that long.
#[tokio::test]
async fn every_submit_reads_a_429s_retry_after() {
    let mut problems = Vec::new();
    for row in rows() {
        let answer = ResponseTemplate::new(429)
            .insert_header("retry-after", "7")
            .set_body_json(json!({ "error": { "message": "slow down" }, "detail": "slow down" }));
        let Some(sent) = send_answering(&row, answer).await else { continue };
        if !(sent.outcome.contains("RateLimited") && sent.outcome.contains("retry_after_ms: 7000")) {
            problems.push(format!("{}: {}", sent.row, sent.outcome.chars().take(160).collect::<String>()));
        }
    }
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
}

// ─── Vendor job states (known-gaps: unknown states) ──────────────────────────

/// An undocumented state, as each vendor would spell one.
fn unknown_state(row: &ModelSchema) -> serde_json::Value {
    // PixVerse's status is a number, direct and resold on TokenHub.
    if row.provider == "pixverse" || row.id.starts_with("tokenhub/pixverse-") { json!(99) } else { json!("litegen_unknown_state") }
}

/// Each vendor's status answer with `state` where the job's state goes
/// (`uri` is the mock vendor, for answers that name a URL to poll), and the
/// states its docs say are still in progress. `None` for a vendor whose
/// answer has no state field (Google's long-running operations: `done`).
fn state_answer(row: &ModelSchema, state: &serde_json::Value, uri: &str) -> Option<(serde_json::Value, Vec<serde_json::Value>)> {
    let s = |v: &[&str]| v.iter().map(|x| json!(x)).collect::<Vec<_>>();
    Some(match (row.media_type, row.provider.as_str()) {
        // @see https://www.volcengine.com/docs/82379/1521309 — "queued, running, cancelled, succeeded, failed, expired"
        (MediaType::Video, "bytedance") => (json!({ "status": state }), s(&["queued", "running"])),
        // fal's queue: IN_QUEUE | IN_PROGRESS | COMPLETED.
        (MediaType::Video | MediaType::Model3d, "fal") => (json!({ "status": state }), s(&["IN_QUEUE", "IN_PROGRESS"])),
        // Tencent VCLM: WAIT | RUN | FAIL | DONE.
        (MediaType::Video, "hunyuan") => (json!({ "Response": { "Status": state } }), s(&["WAIT", "RUN"])),
        // Hunyuan image JobStatusCode: 1 waiting, 2 running, 4 failed, 5 done.
        (MediaType::Image, "hunyuan") => (json!({ "Response": { "JobId": "j1", "JobStatusCode": state } }), s(&["1", "2"])),
        // Kling video, current design: GET /tasks → data[]: submitted | processing | succeeded | failed.
        // @see https://kling.ai/document-api/api/video/3-0-omni/text-to-video.md (2026-11-03)
        (MediaType::Video, "kling") => (json!({ "code": 0, "data": [{ "id": "conformance-job", "status": state }] }), s(&["submitted", "processing"])),
        // Kling image: submitted | processing | succeed | failed.
        (_, "kling") => (json!({ "code": 0, "data": { "task_id": "t1", "task_status": state } }), s(&["submitted", "processing"])),
        // Leonardo job_status: PENDING | COMPLETE | FAILED.
        (_, "leonardo") => (
            json!({ "sdGenerationJob": { "generationId": "g1" }, "motionVideoGenerationJob": { "generationId": "g1" }, "generations_by_pk": { "status": state } }),
            s(&["PENDING"]),
        ),
        // Luma state: queued | dreaming | completed | failed.
        (_, "luma") => (json!({ "id": "g1", "state": state }), s(&["queued", "dreaming"])),
        // MiniMax: V1 Preparing | Queueing | Processing | Success | Fail; V2 queued | running | succeeded | failed (| cancelled).
        (MediaType::Video, "minimax") => {
            // The adapter's own list, so a new H3 model needs no second edit here.
            let v2 = crate::providers::video::minimax::MiniMaxVideoProvider::is_v2(&row.id);
            let running = if v2 { s(&["queued", "running"]) } else { s(&["Preparing", "Queueing", "Processing"]) };
            (json!({ "status": state, "base_resp": { "status_code": 0 }, "task": { "status": state } }), running)
        }
        // Sora: queued | in_progress | completed | failed.
        (MediaType::Video, "openai") => (json!({ "id": "v1", "status": state }), s(&["queued", "in_progress"])),
        // PixVerse Resp.status: 1 completed | 5 processing | 7 moderation failed | 8 failed.
        (MediaType::Video, "pixverse") => (json!({ "ErrCode": 0, "Resp": { "status": state } }), vec![json!(5)]),
        // Runway: PENDING | THROTTLED | CANCELLED | RUNNING | FAILED | SUCCEEDED.
        (_, "runway") => (json!({ "id": "t1", "status": state }), s(&["PENDING", "THROTTLED", "RUNNING"])),
        // TokenHub's resold images: Kling task_status submitted | processing | succeed | failed;
        // Vidu state created | queueing | processing | success | failed.
        // @see https://cloud.tencent.com/document/product/1823/138803 (2026-11-04)
        // @see https://cloud.tencent.com/document/product/1823/135746 (2026-11-04)
        (MediaType::Image, "tokenhub") if row.id.starts_with("tokenhub/kling-image-") => {
            (json!({ "code": 0, "data": { "task_id": "t1", "task_status": state } }), s(&["submitted", "processing"]))
        }
        (MediaType::Image, "tokenhub") if row.id.starts_with("tokenhub/vidu-image-") => {
            (json!({ "task_id": "t1", "state": state }), s(&["created", "queueing", "processing"]))
        }
        // TokenHub's resold Kling: Kling's own envelope, data[]: processing | succeeded | failed
        // (submit answers "submitted").
        // @see https://cloud.tencent.com/document/product/1823/135742 (2026-11-04)
        (MediaType::Video, "tokenhub") if row.id.starts_with("tokenhub/kling-") => {
            (json!({ "code": 0, "data": [{ "id": "conformance-job", "status": state }] }), s(&["submitted", "processing"]))
        }
        // TokenHub's resold MiniMax: v2.3 flat ("Processing / Success / Fail"), H3
        // task.status ("queued / running / succeeded / failed / cancelled").
        // @see https://cloud.tencent.com/document/product/1823/135737 (2026-11-04)
        (MediaType::Video, "tokenhub") if row.id.starts_with("tokenhub/minimax-video-v2.3") => {
            (json!({ "task_id": "conformance-job", "status": state, "base_resp": { "status_code": 0 } }), s(&["Processing"]))
        }
        (MediaType::Video, "tokenhub") if row.id.starts_with("tokenhub/minimax-video-h3") => {
            (json!({ "task_id": "conformance-job", "task": { "status": state } }), s(&["queued", "running"]))
        }
        // TokenHub's YouTu: status queued | in_progress | completed | failed (the /v1/api family).
        // @see https://cloud.tencent.com/document/product/1823/135716 (2026-11-04)
        (MediaType::Video, "tokenhub") if row.id == "tokenhub/yt-video-2.0" => (json!({ "object": "video", "status": state }), s(&["queued", "in_progress"])),
        // TokenHub's resold Vidu: state created | queueing | processing | success | failed.
        // @see https://cloud.tencent.com/document/product/1823/135756 (2026-11-04)
        (MediaType::Video, "tokenhub") if row.id.starts_with("tokenhub/vidu-video-") => {
            (json!({ "task_id": "conformance-job", "state": state }), s(&["created", "queueing", "processing"]))
        }
        // TokenHub's resold PixVerse: Resp.status 1 success | 5 processing | 6 deleted | 7 moderated | 8 failed.
        // @see https://cloud.tencent.com/document/product/1823/135325 (2026-11-04)
        (MediaType::Video, "tokenhub") if row.id.starts_with("tokenhub/pixverse-") => (json!({ "ErrCode": 0, "Resp": { "status": state } }), vec![json!(5)]),
        // TokenHub video: queued | running | succeeded | failed.
        // @see https://cloud.tencent.com/document/product/1823/137202 (2026-11-04)
        (MediaType::Video, "tokenhub") => (json!({ "task_id": "conformance-job", "status": state }), s(&["queued", "running"])),
        // Vidu state: created | queueing | processing | success | failed.
        (MediaType::Video, "vidu") => (json!({ "task_id": "t1", "state": state }), s(&["created", "queueing", "processing"])),
        // Bedrock GetAsyncInvoke: InProgress | Completed | Failed.
        (MediaType::Video, "bedrock") => (json!({ "invocationArn": "arn:1", "status": state }), s(&["InProgress"])),
        // @see https://developer.ideogram.ai/openapi/ideogram-api-v2.json — GET /v2/generations/{generation_id}: "pending", "completed", "failed"
        (MediaType::Video, "ideogram") => (json!({ "generation_id": "gen_1", "status": state }), s(&["pending"])),
        // Replicate: starting | processing | succeeded | failed | canceled | aborted.
        (_, "replicate") => (
            json!({ "id": "p1", "status": state, "urls": { "get": format!("{uri}/v1/predictions/p1") } }),
            s(&["starting", "processing"]),
        ),
        // BFL: Pending | Reasoning | Generating | Ready | Error | moderated…
        (MediaType::Image, "bfl") => (json!({ "id": "t1", "polling_url": format!("{uri}/v1/get_result?id=t1"), "status": state }), s(&["Pending", "Reasoning", "Generating"])),
        // Meshy: PENDING | IN_PROGRESS | SUCCEEDED | FAILED | CANCELED.
        (MediaType::Model3d, "meshy") => (json!({ "id": "m1", "status": state, "progress": 10 }), s(&["PENDING", "IN_PROGRESS"])),
        // Tripo: queued | running | success | failed | cancelled | banned | expired | unknown.
        (MediaType::Model3d, "tripo") => (json!({ "code": 0, "data": { "task_id": "t1", "status": state, "progress": 10 } }), s(&["queued", "running"])),
        _ => return None,
    })
}

fn job_id(row: &ModelSchema) -> String {
    match row.provider.as_str() {
        "fal" => json!({ "request_id": "r1", "endpoint": "fal-ai/ltx-2.3/text-to-video", "status_url": "", "response_url": "" }).to_string(),
        "replicate" => json!({ "prediction_id": "p1" }).to_string(),
        _ => "conformance-job".to_string(),
    }
}

async fn answering_with(answer: &serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any()).respond_with(ResponseTemplate::new(200).set_body_json(answer.clone())).mount(&server).await;
    server
}

/// A vendor state litegen doesn't know fails the job at once, naming it
/// (it used to poll until the reaper's 2 h maximum age); every state the
/// vendor documents as in progress stays in progress.
#[tokio::test]
async fn every_video_and_3d_poll_fails_an_undocumented_state() {
    let mut problems = Vec::new();
    let mut checked = 0;
    // Every row, operations included: a poll reads the same states whatever
    // the job does.
    let all: Vec<ModelSchema> = catalog()
        .all()
        .filter(|r| r.provider != "mock")
        .cloned()
        .chain(uncatalogued_rows())
        .collect();
    for row in all.into_iter().filter(|r| matches!(r.media_type, MediaType::Video | MediaType::Model3d)) {
        let unknown = unknown_state(&row);
        let Some((_, running)) = state_answer(&row, &unknown, "") else { continue };
        for (state, should_fail) in std::iter::once((unknown.clone(), true)).chain(running.into_iter().map(|s| (s, false))) {
            let probe = MockServer::start().await;
            let (answer, _) = state_answer(&row, &state, &probe.uri()).unwrap();
            let server = answering_with(&answer).await;
            let cfg = config(&server, &row.id);
            let (status, error) = match row.media_type {
                MediaType::Video => {
                    let p = build_video_provider(&row.provider, &cfg).unwrap();
                    let h = crate::providers::VideoGenerationHandle { provider_job_id: job_id(&row), provider: row.provider.clone(), model: row.id.clone() };
                    match p.poll_status(&h).await {
                        Ok(r) => (Some(r.status), r.error),
                        Err(e) => (None, Some(format!("{e} retryable={}", e.is_retryable()))),
                    }
                }
                _ => {
                    let p = build_model3d_provider(&row.provider, &cfg).unwrap();
                    let h = crate::providers::Model3dGenerationHandle {
                        provider_job_id: "r1".into(),
                        provider: row.provider.clone(),
                        model: row.id.clone(),
                        stage_context: Some(json!({ "endpoint": "fal-ai/trellis" })),
                    };
                    match p.poll_status(&h).await {
                        Ok(r) => (Some(r.status), r.error),
                        Err(e) => (None, Some(format!("{e} retryable={}", e.is_retryable()))),
                    }
                }
            };
            checked += 1;
            use crate::types::GenerationStatus::*;
            let shown = state.as_str().map(str::to_string).unwrap_or_else(|| state.to_string());
            match (should_fail, status, error.as_deref()) {
                (true, Some(Failed), Some(e)) if e.contains(&shown) => {}
                (false, Some(Pending | Processing), _) => {}
                (want, got, e) => problems.push(format!("{} state {shown}: wanted {}, got {got:?} {e:?}", row.id, if want { "Failed naming it" } else { "in progress" })),
            }
        }
    }
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
    assert!(checked > 60, "only {checked} polls ran");
}

/// The image adapters that poll fail an undocumented state at once
/// (non-retryable: the vendor took the job), and keep polling a documented
/// in-progress one until litegen's deadline.
#[tokio::test]
async fn every_image_poll_fails_an_undocumented_state() {
    let mut problems = Vec::new();
    let mut checked = 0;
    // Cases run at once: an in-progress one waits out its deadline.
    let mut cases = Vec::new();
    for row in rows().into_iter().filter(|r| r.media_type == MediaType::Image) {
        let unknown = unknown_state(&row);
        let Some((_, running)) = state_answer(&row, &unknown, "") else { continue };
        for (state, should_fail) in std::iter::once((unknown.clone(), true)).chain(running.into_iter().map(|s| (s, false))) {
            let row = row.clone();
            cases.push(async move {
                let server = MockServer::start().await;
                let materialized = refs(&row, &server)?;
                let (answer, _) = state_answer(&row, &state, &server.uri()).unwrap();
                Mock::given(wiremock::matchers::any()).respond_with(ResponseTemplate::new(200).set_body_json(answer)).mount(&server).await;
                let p = build_image_provider(&row.provider, &config(&server, &row.id)).unwrap();
                // An undocumented state ends the call at its first poll, so it
                // gets a long deadline (a loaded machine can be slow to submit);
                // a documented in-progress one must run out of a short one.
                let wait = if should_fail { Duration::from_secs(20) } else { Duration::from_millis(600) };
                let deadline = std::time::Instant::now() + wait;
                let got = crate::providers::with_deadline(deadline, p.generate(&row, &base(&row), &image_extras(&row), &materialized)).await;
                let shown = state.as_str().map(str::to_string).unwrap_or_else(|| state.to_string());
                Some(match (should_fail, got) {
                    (true, Err(crate::providers::ProviderError::RequestFailed { message, retryable: false, .. })) if message.contains(&shown) => None,
                    (false, Err(crate::providers::ProviderError::Timeout { .. })) => None,
                    (want, got) => Some(format!(
                        "{} state {shown}: wanted {}, got {:?}",
                        row.id,
                        if want { "a final error naming it" } else { "a timeout (still polling)" },
                        got.err()
                    )),
                })
            });
        }
    }
    for outcome in futures::future::join_all(cases).await.into_iter().flatten() {
        checked += 1;
        problems.extend(outcome);
    }
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
    assert!(checked > 20, "only {checked} image polls ran");
}

/// How each polling image vendor accepts a job and later reports it done,
/// and how its polls are told apart from its submit.
fn image_job_flow(provider: &str, uri: &str) -> Option<(serde_json::Value, serde_json::Value, bool)> {
    let img = format!("{uri}/conformance/result.png");
    // (submit answer, finished poll answer, polls are POSTs told apart by X-TC-Action)
    Some(match provider {
        "bfl" => (json!({ "id": "t1", "polling_url": format!("{uri}/v1/get_result?id=t1") }), json!({ "status": "Ready", "result": { "sample": img } }), false),
        "kling" => (
            json!({ "code": 0, "data": { "task_id": "t1", "task_status": "submitted" } }),
            json!({ "code": 0, "data": { "task_id": "t1", "task_status": "succeed", "task_result": { "images": [{ "index": 0, "url": img }] } } }),
            false,
        ),
        "hunyuan" => (json!({ "Response": { "JobId": "j1" } }), json!({ "Response": { "JobStatusCode": "5", "ResultImage": [img] } }), true),
        "leonardo" => (
            json!({ "sdGenerationJob": { "generationId": "g1" } }),
            json!({ "generations_by_pk": { "status": "COMPLETE", "generated_images": [{ "url": img }] } }),
            false,
        ),
        "luma" => (json!({ "id": "g1", "state": "queued" }), json!({ "id": "g1", "state": "completed", "assets": { "image": img } }), false),
        "runway" => (json!({ "id": "t1" }), json!({ "id": "t1", "status": "SUCCEEDED", "output": [img] }), false),
        "replicate" => (
            json!({ "id": "p1", "status": "starting", "urls": { "get": format!("{uri}/v1/predictions/p1") } }),
            json!({ "id": "p1", "status": "succeeded", "output": [img] }),
            false,
        ),
        _ => return None,
    })
}

/// A vendor has the job once it answers the submit: a poll that fails on
/// the way (a gateway's 503 page) is asked again, never the end of a job the
/// vendor will bill for (known-gaps: Luma image's unreadable answers).
#[tokio::test]
async fn every_image_poll_asks_again_after_a_passing_failure() {
    // One row per vendor (the poll loop is the adapter's), all at once.
    let mut seen = std::collections::BTreeSet::new();
    let picked: Vec<ModelSchema> = rows()
        .into_iter()
        .filter(|r| r.media_type == MediaType::Image && image_job_flow(&r.provider, "").is_some() && seen.insert(r.provider.clone()))
        .collect();
    let outcomes = futures::future::join_all(picked.into_iter().map(|row| async move {
        let server = MockServer::start().await;
        let (submitted, finished, by_action) = image_job_flow(&row.provider, &server.uri()).unwrap();
        let materialized = refs(&row, &server)?;
        let poll = || {
            if by_action {
                Mock::given(method("POST")).and(wiremock::matchers::header("x-tc-action", "QueryHunyuanImageJob"))
            } else {
                Mock::given(method("GET"))
            }
        };
        // Lower numbers win: the result file, then one failed poll, then the
        // finished answer, then the submit.
        poll().respond_with(ResponseTemplate::new(503).set_body_string("<html>upstream unavailable</html>")).up_to_n_times(1).with_priority(2).mount(&server).await;
        poll().respond_with(ResponseTemplate::new(200).set_body_json(finished)).with_priority(3).mount(&server).await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(submitted)).with_priority(4).mount(&server).await;
        Mock::given(method("GET"))
            .and(path("/conformance/result.png"))
            .respond_with(ResponseTemplate::new(200).insert_header("content-type", "image/png").set_body_bytes(PNG))
            .with_priority(1)
            .mount(&server)
            .await;
        let p = build_image_provider(&row.provider, &config(&server, &row.id)).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let got = crate::providers::with_deadline(deadline, p.generate(&row, &base(&row), &image_extras(&row), &materialized)).await;
        let polls = server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|r| {
                if by_action {
                    r.headers.get("x-tc-action").is_some_and(|v| v == "QueryHunyuanImageJob")
                } else {
                    r.method.as_str() == "GET" && !r.url.path().starts_with("/conformance/")
                }
            })
            .count();
        // The result is on loopback, which the default public-address check
        // refuses: reaching it at all proves the poll loop survived.
        let survived = polls >= 2 && !matches!(&got, Err(e) if e.to_string().contains("503"));
        Some((!survived).then(|| format!("{}: {polls} polls, then {:?}", row.id, got.err())))
    }))
    .await;
    let checked = outcomes.iter().filter(|o| o.is_some()).count();
    let problems: Vec<String> = outcomes.into_iter().flatten().flatten().collect();
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
    assert!(checked >= 7, "only {checked} vendors ran");
}

// ─── Resumable image jobs (late delivery) ────────────────────────────────────

/// Each polling image vendor's status answer for job `t1` (`img` is the
/// result it names when done), as its poll reads it.
fn job_answers(row: &ModelSchema, img: &str) -> Option<(serde_json::Value, serde_json::Value, serde_json::Value)> {
    // (running, done, failed)
    Some(match row.provider.as_str() {
        // TokenHub's resold images run jobs; its Hy-Image and Seedream rows answer at once.
        "tokenhub" if row.id.starts_with("tokenhub/kling-image-") => (
            json!({ "code": 0, "data": { "task_id": "t1", "task_status": "processing" } }),
            json!({ "code": 0, "data": { "task_id": "t1", "task_status": "succeed", "task_result": { "images": [{ "url": img }] } } }),
            json!({ "code": 0, "data": { "task_id": "t1", "task_status": "failed", "task_status_msg": "the vendor said no" } }),
        ),
        "tokenhub" if row.id.starts_with("tokenhub/vidu-image-") => (
            json!({ "task_id": "t1", "state": "processing" }),
            json!({ "task_id": "t1", "state": "success", "creations": [{ "url": img }] }),
            json!({ "task_id": "t1", "state": "failed", "err_code": "the vendor said no" }),
        ),
        "tokenhub" => return None,
        "bfl" => (json!({ "status": "Pending" }), json!({ "status": "Ready", "result": { "sample": img } }), json!({ "status": "Error" })),
        "kling" => (
            json!({ "code": 0, "data": { "task_status": "processing" } }),
            json!({ "code": 0, "data": { "task_status": "succeed", "task_result": { "images": [{ "url": img }] } } }),
            json!({ "code": 0, "data": { "task_status": "failed", "task_status_msg": "the vendor said no" } }),
        ),
        "hunyuan" => (
            json!({ "Response": { "JobStatusCode": "2" } }),
            json!({ "Response": { "JobStatusCode": "5", "ResultImage": [img] } }),
            json!({ "Response": { "JobStatusCode": "4", "JobErrorMsg": "the vendor said no" } }),
        ),
        "leonardo" => (
            json!({ "generations_by_pk": { "status": "PENDING" } }),
            json!({ "generations_by_pk": { "status": "COMPLETE", "generated_images": [{ "url": img }] } }),
            json!({ "generations_by_pk": { "status": "FAILED" } }),
        ),
        "luma" => (
            json!({ "id": "t1", "state": "dreaming" }),
            json!({ "id": "t1", "state": "completed", "assets": { "image": img } }),
            json!({ "id": "t1", "state": "failed", "failure_reason": "the vendor said no" }),
        ),
        "runway" => (
            json!({ "id": "t1", "status": "RUNNING" }),
            json!({ "id": "t1", "status": "SUCCEEDED", "output": [img] }),
            json!({ "id": "t1", "status": "FAILED", "failure": "the vendor said no" }),
        ),
        "replicate" => (
            json!({ "id": "t1", "status": "processing" }),
            json!({ "id": "t1", "status": "succeeded", "output": [img] }),
            json!({ "id": "t1", "status": "failed", "error": "the vendor said no" }),
        ),
        _ => return None,
    })
}

/// A job the vendor accepted can be asked about again after the call that
/// submitted it returned: `poll_job` reads it as the adapter's own poll loop
/// does — running, done (its result downloaded), failed, or a passing
/// failure to ask again — and only adapters that run jobs say `resumable`.
#[tokio::test]
async fn every_resumable_image_job_is_read_by_poll_job() {
    let mut problems = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    // One server for every row's `resumable()` check, which sends nothing.
    // A server per row hung this test once fal had enough image rows in a
    // row: wiremock's `Drop` waits on a tokio lock through
    // `futures::executor::block_on`, and after enough starts and drops with
    // no await that yields, tokio's coop budget runs out. The lock's wake is
    // then deferred to a scheduler that never runs again (2026-11-04).
    let server = MockServer::start().await;
    for row in rows().into_iter().filter(|r| r.media_type == MediaType::Image) {
        let resumable_vendor = job_answers(&row, "").is_some();
        let p = build_image_provider(&row.provider, &config(&server, &row.id)).unwrap();
        if p.resumable(&row) != resumable_vendor {
            problems.push(format!("{}: resumable() is {}", row.id, p.resumable(&row)));
        }
        // One row per vendor, but every TokenHub route (each resells another API).
        let family = if row.provider == "tokenhub" { row.id.clone() } else { row.provider.clone() };
        if !resumable_vendor || !seen.insert(family) {
            continue;
        }
        let img = format!("{}/conformance/result.png", server.uri());
        let (running, done, failed) = job_answers(&row, &img).unwrap();
        let resume = (row.provider == "bfl").then(|| json!({ "polling_url": format!("{}/v1/get_result?id=t1", server.uri()) }));
        let ask = |answer: ResponseTemplate| {
            let row = row.clone();
            let resume = resume.clone();
            async move {
                let server = MockServer::start().await;
                Mock::given(wiremock::matchers::any()).respond_with(answer).mount(&server).await;
                let mut cfg = config(&server, &row.id);
                cfg.model_mapping.clear();
                let p = build_image_provider(&row.provider, &cfg).unwrap();
                let resume = resume.map(|_| json!({ "polling_url": format!("{}/v1/get_result?id=t1", server.uri()) }));
                p.poll_job(&row.id, "t1", resume.as_ref()).await
            }
        };
        match ask(ResponseTemplate::new(200).set_body_json(running)).await {
            Ok(crate::providers::ImageJobPoll::Running) => {}
            other => problems.push(format!("{}: a running job read as {other:?}", row.id)),
        }
        // Done: the result is on the mock's loopback, which the default
        // public-address check refuses — reaching that refusal proves the
        // adapter read the job as done and went for its result.
        match ask(ResponseTemplate::new(200).set_body_json(done)).await {
            Ok(crate::providers::ImageJobPoll::Done(_)) => {}
            Err(e) if e.to_string().contains("could not be fetched") => {}
            Ok(crate::providers::ImageJobPoll::Failed(why)) if why.contains("could not be fetched") => {}
            other => problems.push(format!("{}: a finished job read as {other:?}", row.id)),
        }
        match ask(ResponseTemplate::new(200).set_body_json(failed)).await {
            Ok(crate::providers::ImageJobPoll::Failed(_)) => {}
            other => problems.push(format!("{}: a failed job read as {other:?}", row.id)),
        }
        match ask(ResponseTemplate::new(503).set_body_string("<html>upstream unavailable</html>")).await {
            Ok(crate::providers::ImageJobPoll::Running) => {}
            Err(e) if e.is_retryable() => {}
            other => problems.push(format!("{}: a passing failure read as {other:?}", row.id)),
        }
    }
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
    // The vendors whose image jobs litegen can ask about again, and each
    // TokenHub route that resells one (2026-11-04).
    let want = [
        "bfl", "hunyuan", "kling", "leonardo", "luma", "replicate", "runway",
        "tokenhub/kling-image-o1", "tokenhub/kling-image-v3", "tokenhub/kling-image-v3-omni", "tokenhub/vidu-image-q2",
    ];
    assert_eq!(seen.iter().map(String::as_str).collect::<Vec<_>>(), want, "{seen:?}");
}
