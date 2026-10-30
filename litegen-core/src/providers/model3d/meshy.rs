//! Meshy 3D: rigging and animation (PixelGenie 3D doc §3B, §3C) on Meshy's API.
//!
//! ```text
//! POST {base}/v1/rigging      {model_url | input_task_id, height_meters, animation_type} → {"result": task id}
//! GET  {base}/v1/rigging/{id}     status, progress, task_error, result.rigged_character_{glb,fbx}_url
//! POST {base}/v1/animations   {rig_task_id, action_ids, post_process} → {"result": task id}
//! GET  {base}/v1/animations/{id}  … result.animation_{glb,fbx}_url, processed_animation_fps_fbx_url
//! ```
//!
//! `{base}` is `https://api.meshy.ai/openapi`.
//!
//! # References (read 2026-10-30)
//!
//! - Base and auth — "API Base URL: `https://api.meshy.ai/openapi/`";
//!   "Authorization: Bearer msy_YOUR_API_KEY": <https://docs.meshy.ai/llms.txt>
//! - Statuses — "`PENDING` -> `IN_PROGRESS` -> `SUCCEEDED` | `FAILED` |
//!   `CANCELED`"; and, Verbatim: "Don't resend a create request
//!   automatically after a timeout, dropped connection or `5xx`." (Meshy has
//!   no idempotency key): <https://docs.meshy.ai/llms.txt>
//! - Rigging — Verbatim: "The model must be a GLB (`.glb`) file of at most
//!   400 MB from an `http` or `https` URL, or 100 MB as a data URI.":
//!   <https://docs.meshy.ai/en/api/rigging>; `animation_type` (biped |
//!   quadruped) is in <https://docs.meshy.ai/openapi.json> (RiggingRequest).
//! - Animation — Verbatim: "The IDs of 1 to 10 preset animations to apply at
//!   once, with no repeats." and, of `post_process`, `change_fps` "changes
//!   the frame rate to `fps`" (24, 25, 30 or 60), returning
//!   `processed_animation_fps_fbx_url`: <https://docs.meshy.ai/en/api/animation>
//! - Errors — request errors are `{"message": "…"}`; a failed task carries
//!   `task_error.message`: <https://docs.meshy.ai/openapi.json>
//! - Price — credits only ("Automatic rigging | 5 per request", "3 per
//!   action"): <https://docs.meshy.ai/en/api/pricing>

use async_trait::async_trait;
use reqwest::{Client, Method, RequestBuilder, StatusCode};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

use super::download::AssetError;
use crate::capabilities::ModelSchema;
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    build_cost_estimate, ApiKeyPool, BaseGenerationRequest, HealthCheckResult, Model3dExtras, Model3dFile,
    Model3dGenerationHandle, Model3dGenerationPollResult, Model3dProvider, Preflight, ProviderError,
    ProviderInstanceConfig, SourceDelivery,
};
use crate::proxy::materializer::MaterializedRequest;
use crate::types::*;

/// Meshy's API. `api_base` overrides it.
const DEFAULT_API_BASE: &str = "https://api.meshy.ai/openapi";

/// Largest result pulled down: litegen's mesh reader refuses more anyway.
const MAX_ASSET_BYTES: u64 = crate::mesh::MAX_INPUT_BYTES as u64;

/// The frame rates `change_fps` makes.
const CHANGE_FPS_RATES: [u32; 4] = [24, 25, 30, 60];

/// The litegen models served here. A test ties it to `models/meshy.yaml`.
pub const MODELS: &[&str] = &["meshy/rig", "meshy/animate"];

fn poll(status: GenerationStatus, progress: u8, error: Option<String>, files: Vec<Model3dFile>, meta: HashMap<String, Value>) -> Model3dGenerationPollResult {
    Model3dGenerationPollResult { status, progress, files, error, metadata: meta, stage_context: None, next_stage: None }
}

pub struct MeshyModel3dProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
    /// Result downloads, following no redirects past the public-address check.
    asset_client: Client,
    /// Only the tests, whose mock vendor runs on loopback, turn it off.
    public_assets_only: bool,
}

impl Default for MeshyModel3dProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MeshyModel3dProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
            client: Client::builder().timeout(Duration::from_secs(120)).build().expect("failed to build HTTP client"),
            asset_client: crate::util::ssrf::no_redirect_download_client(Duration::from_secs(600), Duration::from_secs(60)),
            public_assets_only: true,
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or(DEFAULT_API_BASE)
            .trim_end_matches('/')
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("meshy".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, ProviderError> {
        let builder = self.client.request(method, format!("{}/{path}", self.api_base()));
        let builder = crate::providers::auth::apply(&self.auth, &self.creds()?, builder)?;
        Ok(crate::providers::inject_trace_headers(builder))
    }

    /// Send a call and read its JSON. A create is never re-sent once it may
    /// have reached Meshy — not even after a 5xx, which Meshy says not to
    /// resend — so only a connection that never opened, or a 429, is
    /// retryable for one.
    async fn call(&self, req: RequestBuilder, what: &str, creates: bool) -> Result<Value, ProviderError> {
        let resp = req.send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("meshy {what} failed: {e}"),
            status_code: None,
            provider_error: None,
            retryable: e.is_connect() || !creates,
        })?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("meshy {what} reply could not be read: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: !creates,
        })?;
        let json: Option<Value> = serde_json::from_str(&text).ok();
        if !status.is_success() {
            let transient = status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT || status == StatusCode::TOO_MANY_REQUESTS;
            let retryable = if creates { status == StatusCode::TOO_MANY_REQUESTS } else { transient };
            let message = json
                .as_ref()
                .and_then(|j| j["message"].as_str())
                .map(str::to_string)
                .unwrap_or_else(|| text.chars().take(200).collect());
            return Err(ProviderError::RequestFailed {
                message: format!("meshy {what} returned HTTP {}: {message}", status.as_u16()),
                status_code: Some(status.as_u16()),
                provider_error: json,
                retryable,
            });
        }
        json.ok_or_else(|| ProviderError::RequestFailed {
            message: format!("meshy {what} answered {} with a body that is not JSON", status.as_u16()),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: !creates,
        })
    }

    /// A create's task id: `{"result": "<id>"}`.
    fn task_id(created: &Value, what: &str) -> Result<String, ProviderError> {
        created["result"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .ok_or_else(|| ProviderError::RequestFailed {
                message: format!("meshy {what} answered without a task id"),
                status_code: None,
                provider_error: Some(created.clone()),
                retryable: false,
            })
    }

    /// The containers to download: glb, plus Meshy's own fbx when one was
    /// promised (`prefers_vendor_native`).
    fn want(extras: &Model3dExtras) -> Vec<&'static str> {
        let mut want = vec!["glb"];
        if extras
            .output_formats
            .iter()
            .any(|f| f.eq_ignore_ascii_case("fbx") && crate::proxy::model3d_convert::prefers_vendor_native(f))
        {
            want.push("fbx");
        }
        want
    }

    fn handle(model: &ModelSchema, task: String, ctx: Value) -> Model3dGenerationHandle {
        Model3dGenerationHandle { provider_job_id: task, provider: "meshy".to_string(), model: model.id.clone(), stage_context: Some(ctx) }
    }

    /// `POST /v1/rigging`: the source by Meshy's own task id when it made it,
    /// else inline as a glb `data:` URI.
    async fn submit_rig(&self, model: &ModelSchema, extras: &Model3dExtras) -> Result<Model3dGenerationHandle, ProviderError> {
        use base64::Engine;
        let source = extras
            .source
            .as_ref()
            .ok_or_else(|| ProviderError::InvalidRequest(format!("'{}' needs a source_model", model.id)))?;
        let mut body = serde_json::Map::new();
        let delivery = match &source.vendor_task_id {
            Some(id) => {
                body.insert("input_task_id".into(), json!(id));
                SourceDelivery::VendorTask
            }
            None => {
                if !source.format.eq_ignore_ascii_case("glb") {
                    return Err(ProviderError::InvalidRequest(format!("Meshy rigs a glb; the source is {}", source.format)));
                }
                let uri = format!("data:model/gltf-binary;base64,{}", base64::engine::general_purpose::STANDARD.encode(&source.bytes));
                body.insert("model_url".into(), json!(uri));
                SourceDelivery::DataUri
            }
        };
        if let Some(h) = extras.height_m {
            body.insert("height_meters".into(), json!(h));
        }
        if let Some(sk) = extras.skeleton.as_deref() {
            let animation_type = match sk {
                "humanoid" => "biped",
                "quadruped" => "quadruped",
                other => return Err(ProviderError::InvalidRequest(format!("Meshy rigs bipeds and quadrupeds, not '{other}'"))),
            };
            body.insert("animation_type".into(), json!(animation_type));
        }
        let created = self.call(self.request(Method::POST, "v1/rigging")?.json(&Value::Object(body)), "rigging", true).await?;
        let task = Self::task_id(&created, "rigging")?;
        source.record(delivery);
        let ctx = json!({ "task_id": task, "kind": "rigging", "want": Self::want(extras), "fps_fbx": false });
        Ok(Self::handle(model, task, ctx))
    }

    /// `POST /v1/animations` on the Meshy rig the source is.
    async fn submit_animation(&self, model: &ModelSchema, extras: &Model3dExtras) -> Result<Model3dGenerationHandle, ProviderError> {
        let source = extras
            .source
            .as_ref()
            .ok_or_else(|| ProviderError::InvalidRequest(format!("'{}' needs a source_model", model.id)))?;
        let rig = source.vendor_task_id.clone().ok_or_else(|| {
            ProviderError::InvalidRequest("Meshy animates only a rig it made on this account; rig with meshy/rig first".into())
        })?;
        let mut ids: Vec<i64> = Vec::new();
        for c in &extras.animations {
            let preset = c.preset.as_deref().ok_or_else(|| ProviderError::InvalidRequest(format!("clip '{}': Meshy animates library actions only", c.name)))?;
            let id: i64 = preset
                .parse()
                .map_err(|_| ProviderError::InvalidRequest(format!("clip '{}': '{preset}' is not a Meshy action id", c.name)))?;
            if ids.contains(&id) {
                return Err(ProviderError::InvalidRequest(format!(
                    "Meshy takes each action once per request; action {id} is asked twice"
                )));
            }
            ids.push(id);
        }
        if !(1..=10).contains(&ids.len()) {
            return Err(ProviderError::InvalidRequest(format!("Meshy applies 1 to 10 actions per request; {} asked", ids.len())));
        }
        let mut body = json!({ "rig_task_id": rig, "action_ids": ids });
        // Meshy's own rate change makes only an fbx; litegen keys the glb.
        let fps_fbx = extras.fps.is_some_and(|f| CHANGE_FPS_RATES.contains(&f));
        if let Some(fps) = extras.fps.filter(|_| fps_fbx) {
            body["post_process"] = json!({ "operation_type": "change_fps", "fps": fps });
        }
        let created = self.call(self.request(Method::POST, "v1/animations")?.json(&body), "animations", true).await?;
        let task = Self::task_id(&created, "animations")?;
        source.record(SourceDelivery::VendorTask);
        let ctx = json!({ "task_id": task, "kind": "animations", "want": Self::want(extras), "fps_fbx": fps_fbx });
        Ok(Self::handle(model, task, ctx))
    }

    async fn fetch(&self, url: &str) -> Result<(Vec<u8>, String), AssetError> {
        super::download::fetch_asset(&self.asset_client, url, MAX_ASSET_BYTES, self.public_assets_only, "meshy").await
    }
}

/// The container a result URL names, from its path's extension.
fn format_of(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    match path.rsplit('/').next().and_then(|n| n.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase()) {
        Some(e) if !e.is_empty() && e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()) => e,
        _ => DEFAULT_MODEL3D_FORMAT.to_string(),
    }
}

#[async_trait]
impl Model3dProvider for MeshyModel3dProvider {
    fn name(&self) -> &str {
        "meshy"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.api_keys.is_empty() {
            self.key_pool = Some(ApiKeyPool::shared(config.api_keys.clone()));
        }
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.key_pool.is_some())
    }

    /// Meshy animates only a rig it made: a source without its rig task id is
    /// refused before anything is billed. Meshy has no free check to run.
    async fn preflight(&self, model: &ModelSchema, extras: &Model3dExtras) -> Result<Preflight, ProviderError> {
        if model.id == "meshy/animate" && extras.source.as_ref().is_some_and(|s| s.vendor_task_id.is_none()) {
            return Ok(Preflight::Refuse {
                code: "source_not_chainable",
                message: "Meshy animates only a rig it made on this Meshy account: send a meshy/rig generation's id".into(),
            });
        }
        Ok(Preflight::Proceed { context: None })
    }

    async fn generate(
        &self,
        model: &ModelSchema,
        _base: &BaseGenerationRequest,
        extras: &Model3dExtras,
        _materialized: &MaterializedRequest,
    ) -> Result<Model3dGenerationHandle, ProviderError> {
        match model.id.as_str() {
            "meshy/rig" => self.submit_rig(model, extras).await,
            "meshy/animate" => self.submit_animation(model, extras).await,
            _ => Err(ProviderError::ModelNotSupported { model: model.id.clone(), provider: "meshy".into() }),
        }
    }

    /// `GET /v1/{rigging|animations}/{id}`; on `SUCCEEDED`, download the
    /// files the job wants in the same call (Meshy keeps them at most 3 days,
    /// behind signed URLs).
    async fn poll_status(&self, handle: &Model3dGenerationHandle) -> Result<Model3dGenerationPollResult, ProviderError> {
        let ctx = handle.stage_context.clone().unwrap_or(Value::Null);
        let task = ctx["task_id"].as_str().filter(|t| !t.is_empty()).unwrap_or(&handle.provider_job_id).to_string();
        let kind = match ctx["kind"].as_str() {
            Some(k @ ("rigging" | "animations")) => k.to_string(),
            _ if handle.model == "meshy/animate" => "animations".to_string(),
            _ => "rigging".to_string(),
        };
        let t = self.call(self.request(Method::GET, &format!("v1/{kind}/{task}"))?, "poll", false).await?;
        let meta: HashMap<String, Value> = HashMap::from([("provider".to_string(), json!("meshy")), ("task_id".to_string(), json!(task))]);
        let progress = t["progress"].as_u64().map(|p| p.min(100) as u8);
        let reason = || t["task_error"]["message"].as_str().filter(|m| !m.is_empty()).unwrap_or("no reason given").to_string();
        match t["status"].as_str().unwrap_or("") {
            "PENDING" => Ok(poll(GenerationStatus::Pending, progress.unwrap_or(0), None, Vec::new(), meta)),
            "SUCCEEDED" => {
                let want: Vec<String> = ctx["want"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_else(|| vec!["glb".to_string()]);
                let r = &t["result"];
                let urls: [(&str, Option<&str>); 2] = if kind == "rigging" {
                    [("glb", r["rigged_character_glb_url"].as_str()), ("fbx", r["rigged_character_fbx_url"].as_str())]
                } else {
                    let fbx = if ctx["fps_fbx"].as_bool() == Some(true) {
                        r["processed_animation_fps_fbx_url"].as_str().or(r["animation_fbx_url"].as_str())
                    } else {
                        r["animation_fbx_url"].as_str()
                    };
                    [("glb", r["animation_glb_url"].as_str()), ("fbx", fbx)]
                };
                let mut files = Vec::new();
                for (format, url) in urls {
                    if !want.iter().any(|w| w == format) {
                        continue;
                    }
                    let primary = format == "glb";
                    let Some(url) = url.filter(|u| !u.is_empty()) else {
                        if primary {
                            return Ok(poll(GenerationStatus::Failed, 100, Some(format!("meshy {kind} finished without a glb")), Vec::new(), meta));
                        }
                        continue;
                    };
                    match self.fetch(url).await {
                        Ok((bytes, ct)) => files.push(Model3dFile {
                            kind: Model3dAssetKind::Mesh,
                            format: if format_of(url) == format { format.to_string() } else { format_of(url) },
                            content_type: crate::proxy::storage::model3d_content_type(format, &ct).to_string(),
                            bytes,
                            polycount: None,
                            width: None,
                            height: None,
                            manifest: None,
                        }),
                        Err(AssetError::Transient(e)) => return Err(e),
                        Err(AssetError::Gone(why)) if primary => {
                            return Ok(poll(GenerationStatus::Failed, 100, Some(format!("meshy {kind} glb could not be downloaded: {why}")), Vec::new(), meta))
                        }
                        Err(AssetError::Gone(why)) => tracing::warn!(reason = %why, "meshy fbx would not download; litegen derives it"),
                    }
                }
                Ok(poll(GenerationStatus::Completed, 100, None, files, meta))
            }
            "FAILED" => Ok(poll(GenerationStatus::Failed, 100, Some(format!("meshy {kind} failed: {}", reason())), Vec::new(), meta)),
            "CANCELED" => Ok(poll(GenerationStatus::Failed, 100, Some(format!("meshy {kind} was canceled")), Vec::new(), meta)),
            // IN_PROGRESS, and a task "rescheduled from IN_PROGRESS back to PENDING".
            _ => Ok(poll(GenerationStatus::Processing, progress.unwrap_or(50), None, Vec::new(), meta)),
        }
    }

    async fn estimate_cost(&self, model: &ModelSchema, _request: &Model3dGenerationRequest) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, None))
    }

    /// Meshy documents no free, read-only call to probe a key with, so this
    /// reports configuration only.
    async fn health_check(&self) -> HealthCheckResult {
        let configured = self.is_configured();
        HealthCheckResult {
            healthy: configured,
            message: if configured { "Meshy key configured (not probed: Meshy has no free status call)".into() } else { "Meshy not configured".into() },
            latency_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{AnimationClip, SourceDelivery, SourceInput};
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn ref_schema(id: &str) -> ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        crate::capabilities::CapabilityRegistry::from_dir(&p).unwrap().get(id).unwrap().clone()
    }

    fn provider(server: &MockServer) -> MeshyModel3dProvider {
        let mut p = MeshyModel3dProvider::new();
        p.configure(ProviderInstanceConfig {
            api_keys: vec![ApiKeyEntry { key: "msy_pooled".into(), weight: 1, label: None }],
            api_base: Some(format!("{}/openapi", server.uri())),
            ..Default::default()
        });
        p.public_assets_only = false;
        p
    }

    fn base(model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: String::new(), model: model.into(), n: 1, negative_prompt: None, seed: None,
            reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    fn source(vendor_task_id: Option<&str>) -> SourceInput {
        SourceInput {
            bytes: crate::providers::model3d::glb::generate_cube_glb("a knight"),
            format: "glb".into(),
            sha256: "00".into(),
            facts: crate::proxy::model3d_source::SourceFacts { triangles: 12, skinned: true, textured: true },
            vendor_task_id: vendor_task_id.map(str::to_string),
            front: None,
            delivered: Default::default(),
        }
    }

    fn extras(formats: &[&str], src: SourceInput) -> Model3dExtras {
        Model3dExtras {
            output_formats: formats.iter().map(|f| f.to_string()).collect(),
            texture: None, pbr: None, target_polycount: None, symmetry: None, topology: None, rig: None,
            skeleton: None, joint_naming: None, height_m: None, animations: Vec::new(), fps: None,
            extra: None, source: Some(src), preflight: None,
            texture_resolution: None, keep_uvs: None,
        }
    }

    fn clip(name: &str, preset: &str) -> AnimationClip {
        AnimationClip { name: name.into(), preset: Some(preset.into()), prompt: None, duration_s: None, in_place: None, loop_requested: None }
    }

    fn no_refs() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    async fn mount_create(server: &MockServer, at: &str, id: &str) {
        Mock::given(method("POST"))
            .and(path(at.to_string()))
            .and(header("authorization", "Bearer msy_pooled"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "result": id })))
            .mount(server)
            .await;
    }

    async fn sent(server: &MockServer, at: &str) -> Vec<Request> {
        server.received_requests().await.unwrap().into_iter().filter(|r| r.url.path() == at).collect()
    }

    async fn body_of(server: &MockServer, at: &str) -> Value {
        let s = sent(server, at).await;
        assert_eq!(s.len(), 1, "one create at {at}");
        serde_json::from_slice(&s[0].body).unwrap()
    }

    // ─── Rigging ────────────────────────────────────────────────────────────
    // @see <https://docs.meshy.ai/en/api/rigging>

    #[tokio::test]
    async fn a_rig_sends_the_source_as_a_glb_data_uri_with_its_height_and_skeleton() {
        use base64::Engine;
        let server = MockServer::start().await;
        mount_create(&server, "/openapi/v1/rigging", "rig-1").await;
        let mut x = extras(&["glb"], source(None));
        x.height_m = Some(1.8);
        x.skeleton = Some("quadruped".into());
        let h = provider(&server).generate(&ref_schema("meshy/rig"), &base("meshy/rig"), &x, &no_refs()).await.unwrap();
        let uri = format!("data:model/gltf-binary;base64,{}", base64::engine::general_purpose::STANDARD.encode(&x.source.as_ref().unwrap().bytes));
        assert_eq!(body_of(&server, "/openapi/v1/rigging").await, json!({ "model_url": uri, "height_meters": 1.8, "animation_type": "quadruped" }));
        assert_eq!(h.provider_job_id, "rig-1");
        assert_eq!(h.stage_context.as_ref().unwrap()["kind"], "rigging");
        assert_eq!(x.source.as_ref().unwrap().delivery(), Some(SourceDelivery::DataUri));
    }

    #[tokio::test]
    async fn a_humanoid_rig_asks_for_a_biped_and_a_bare_one_sends_merely_the_source() {
        let server = MockServer::start().await;
        mount_create(&server, "/openapi/v1/rigging", "rig-1").await;
        let mut x = extras(&["glb"], source(None));
        x.skeleton = Some("humanoid".into());
        provider(&server).generate(&ref_schema("meshy/rig"), &base("meshy/rig"), &x, &no_refs()).await.unwrap();
        assert_eq!(body_of(&server, "/openapi/v1/rigging").await["animation_type"], "biped");
        let bare = MockServer::start().await;
        mount_create(&bare, "/openapi/v1/rigging", "rig-2").await;
        provider(&bare).generate(&ref_schema("meshy/rig"), &base("meshy/rig"), &extras(&["glb"], source(None)), &no_refs()).await.unwrap();
        assert_eq!(body_of(&bare, "/openapi/v1/rigging").await.as_object().unwrap().keys().collect::<Vec<_>>(), ["model_url"]);
    }

    // ─── Animations ─────────────────────────────────────────────────────────
    // @see <https://docs.meshy.ai/en/api/animation>

    fn animate(chained: Option<&str>, presets: &[&str], fps: Option<u32>, formats: &[&str]) -> Model3dExtras {
        let mut x = extras(formats, source(chained));
        x.animations = presets.iter().enumerate().map(|(i, p)| clip(&format!("c{i}"), p)).collect();
        x.fps = fps;
        x
    }

    #[tokio::test]
    async fn an_animation_goes_to_the_meshy_rig_with_its_action_ids_and_change_fps() {
        let server = MockServer::start().await;
        mount_create(&server, "/openapi/v1/animations", "anim-1").await;
        let x = animate(Some("rig-9"), &["0", "30"], Some(60), &["glb", "fbx"]);
        let h = provider(&server).generate(&ref_schema("meshy/animate"), &base("meshy/animate"), &x, &no_refs()).await.unwrap();
        assert_eq!(body_of(&server, "/openapi/v1/animations").await, json!({
            "rig_task_id": "rig-9",
            "action_ids": [0, 30],
            "post_process": { "operation_type": "change_fps", "fps": 60 },
        }));
        let ctx = h.stage_context.unwrap();
        assert_eq!((ctx["kind"].as_str(), ctx["fps_fbx"].as_bool()), (Some("animations"), Some(true)));
        assert_eq!(ctx["want"], json!(["glb", "fbx"]));
        assert_eq!(x.source.as_ref().unwrap().delivery(), Some(SourceDelivery::VendorTask));
    }

    #[tokio::test]
    async fn a_rate_meshy_cannot_make_is_left_to_litegen() {
        let server = MockServer::start().await;
        mount_create(&server, "/openapi/v1/animations", "anim-1").await;
        let x = animate(Some("rig-9"), &["0"], Some(48), &["glb"]);
        provider(&server).generate(&ref_schema("meshy/animate"), &base("meshy/animate"), &x, &no_refs()).await.unwrap();
        assert!(body_of(&server, "/openapi/v1/animations").await.get("post_process").is_none());
    }

    #[tokio::test]
    async fn a_repeated_action_is_refused_before_any_call() {
        let server = MockServer::start().await;
        let x = animate(Some("rig-9"), &["30", "30"], None, &["glb"]);
        let err = provider(&server).generate(&ref_schema("meshy/animate"), &base("meshy/animate"), &x, &no_refs()).await.unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(ref m) if m.contains("30")), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_source_meshy_did_not_rig_is_not_chainable() {
        let server = MockServer::start().await;
        let v = provider(&server).preflight(&ref_schema("meshy/animate"), &animate(None, &["0"], None, &["glb"])).await.unwrap();
        assert!(matches!(&v, Preflight::Refuse { code: "source_not_chainable", message } if message.contains("meshy/rig")), "{v:?}");
        let v = provider(&server).preflight(&ref_schema("meshy/animate"), &animate(Some("rig-9"), &["0"], None, &["glb"])).await.unwrap();
        assert!(matches!(v, Preflight::Proceed { .. }));
    }

    // ─── Errors and polling ─────────────────────────────────────────────────

    #[tokio::test]
    async fn meshys_message_reaches_the_caller_and_a_create_is_never_resent_after_a_5xx() {
        for (status, retryable) in [(500, false), (429, true), (402, false)] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/openapi/v1/rigging"))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!({ "message": "Insufficient funds" })))
                .mount(&server)
                .await;
            let err = provider(&server)
                .generate(&ref_schema("meshy/rig"), &base("meshy/rig"), &extras(&["glb"], source(None)), &no_refs())
                .await
                .unwrap_err();
            assert_eq!(err.is_retryable(), retryable, "{status}: {err:?}");
            assert!(err.to_string().contains("Insufficient funds"), "{err}");
        }
    }

    fn handle(kind: &str, want: Value, fps_fbx: bool) -> Model3dGenerationHandle {
        Model3dGenerationHandle {
            provider_job_id: "t-1".into(),
            provider: "meshy".into(),
            model: if kind == "rigging" { "meshy/rig".into() } else { "meshy/animate".into() },
            stage_context: Some(json!({ "task_id": "t-1", "kind": kind, "want": want, "fps_fbx": fps_fbx })),
        }
    }

    #[tokio::test]
    async fn every_task_status_is_mapped() {
        for (status, expect) in [
            ("PENDING", GenerationStatus::Pending),
            ("IN_PROGRESS", GenerationStatus::Processing),
            ("FAILED", GenerationStatus::Failed),
            ("CANCELED", GenerationStatus::Failed),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/openapi/v1/rigging/t-1"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "id": "t-1", "status": status, "progress": 40,
                    "task_error": { "message": if status == "FAILED" { "Pose estimation failed" } else { "" } },
                })))
                .mount(&server)
                .await;
            let r = provider(&server).poll_status(&handle("rigging", json!(["glb"]), false)).await.unwrap();
            assert_eq!(r.status, expect, "{status}");
            if status == "FAILED" {
                assert!(r.error.as_deref().is_some_and(|e| e.contains("Pose estimation failed")), "{:?}", r.error);
            }
        }
    }

    #[tokio::test]
    async fn a_finished_animation_downloads_the_glb_and_the_rate_changed_fbx_promised() {
        let server = MockServer::start().await;
        let glb = crate::providers::model3d::humanoid::rigged_humanoid_glb(&crate::providers::model3d::humanoid::MOCK_JOINTS);
        for (at, bytes) in [("/cdn/anim.glb", glb.clone()), ("/cdn/anim.fbx", b"plain".to_vec()), ("/cdn/processed_60fps.fbx", b"rate-changed".to_vec())] {
            Mock::given(method("GET")).and(path(at)).respond_with(ResponseTemplate::new(200).set_body_bytes(bytes)).mount(&server).await;
        }
        Mock::given(method("GET"))
            .and(path("/openapi/v1/animations/t-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "t-1", "type": "animate", "status": "SUCCEEDED", "progress": 100,
                "result": {
                    "animation_glb_url": format!("{}/cdn/anim.glb?Expires=1", server.uri()),
                    "animation_fbx_url": format!("{}/cdn/anim.fbx?Expires=1", server.uri()),
                    "processed_animation_fps_fbx_url": format!("{}/cdn/processed_60fps.fbx?Expires=1", server.uri()),
                },
            })))
            .mount(&server)
            .await;
        let done = provider(&server).poll_status(&handle("animations", json!(["glb", "fbx"]), true)).await.unwrap();
        assert_eq!(done.status, GenerationStatus::Completed, "{:?}", done.error);
        assert_eq!(done.files.iter().map(|f| f.format.as_str()).collect::<Vec<_>>(), ["glb", "fbx"]);
        assert_eq!(done.files[0].bytes, glb);
        assert_eq!(done.files[1].bytes, b"rate-changed", "Meshy's own fbx at the asked rate");
    }

    #[test]
    fn every_meshy_row_is_served_here_and_every_model_here_is_in_the_catalog() {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&p).unwrap();
        let mut rows: Vec<String> = catalog.for_provider("meshy").map(|m| m.id.clone()).collect();
        rows.sort();
        let mut served: Vec<String> = MODELS.iter().map(|m| m.to_string()).collect();
        served.sort();
        assert_eq!(rows, served);
    }
}
