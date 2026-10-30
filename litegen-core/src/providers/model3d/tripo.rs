//! Tripo (VAST AI) 3D: rigging (PixelGenie 3D doc §3B) on Tripo's v3 API.
//!
//! ```text
//! POST {base}/files                  multipart `file` → data.file_token
//! POST {base}/animations/rig-check   {input} → data.task_id   (free; output.riggable, output.rig_type)
//! POST {base}/animations/rig         {input, model, rig_type, spec, out_format} → data.task_id
//! GET  {base}/tasks/{task_id}        status, progress, output.model_url, error_code, error_message
//! ```
//!
//! `{base}` is `https://openapi.tripo3d.ai/v3` for international accounts.
//! Keys belong to their region, so a China-mainland key sets `api_base` to
//! `https://openapi.tripo3d.com/v3`.

//! # References (read 2026-10-30)
//!
//! - Base URL and auth — "https://openapi.tripo3d.ai/v3";
//!   "Authorization: Bearer {api_key}"; the envelope is `{"code": 0, "data"}`
//!   and an error `{"code", "status": "error", "message", "suggestion"}`:
//!   <https://developers.tripo3d.ai/en/docs/introduction>
//! - Rig-check, free and asynchronous — output `riggable`, `rig_type`:
//!   <https://developers.tripo3d.ai/en/docs/animations-rig-check>
//! - Rig — Verbatim: "`v3.0-20260909` — recommended for humanoid characters …
//!   The default is unchanged; v3.0 must be requested explicitly." and, of
//!   `spec`, "Only takes effect when `rig_type` is `biped`.":
//!   <https://developers.tripo3d.ai/en/docs/animations-rig>
//! - Task query — `queued | running | success | failed | cancelled`; Verbatim:
//!   "If a legacy response returns `banned` or `expired`, treat either as
//!   terminal. Other values, such as `unknown`, may occur: use a bounded wait":
//!   <https://developers.tripo3d.ai/en/docs/task-query>
//! - Files — `POST /v3/files`, multipart `file`; Verbatim: "Models: `GLB`,
//!   `GLTF`, `FBX`, `OBJ`, `STL`. Max **150 MB**.":
//!   <https://developers.tripo3d.ai/en/docs/files>
//! - Errors — Verbatim: "A task-creation request may have succeeded even if
//!   its response was lost; avoid resubmitting until its outcome is known.":
//!   <https://developers.tripo3d.ai/en/docs/error-handling>
//! - Price — "1 credit = $0.01 USD" · "Auto Rig / 25" · "Rig Check / Free":
//!   <https://developers.tripo3d.ai/en/pricing>

use async_trait::async_trait;
use reqwest::{Client, Method, RequestBuilder, StatusCode};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::download::AssetError;
use crate::capabilities::ModelSchema;
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    build_cost_estimate, ApiKeyPool, BaseGenerationRequest, HealthCheckResult, Model3dExtras, Model3dFile,
    Model3dGenerationHandle, Model3dGenerationPollResult, Model3dProvider, Preflight, ProviderError,
    ProviderInstanceConfig, SourceDelivery, SourceInput,
};
use crate::proxy::materializer::MaterializedRequest;
use crate::types::*;

/// Tripo's v3 API for international accounts. `api_base` overrides it.
const DEFAULT_API_BASE: &str = "https://openapi.tripo3d.ai/v3";

/// Largest result pulled down: litegen's mesh reader refuses more anyway.
const MAX_ASSET_BYTES: u64 = crate::mesh::MAX_INPUT_BYTES as u64;

/// How often rig-check is polled: "Query once every 1-2 seconds" (task lifecycle).
const CHECK_INTERVAL: Duration = Duration::from_millis(1500);

/// How long the preflight waits for rig-check; the router's vendor timeout
/// bounds it too.
const CHECK_BUDGET: Duration = Duration::from_secs(100);

/// The litegen models served here. A test ties it to `models/tripo.yaml`
/// both ways.
pub const MODELS: &[&str] = &["tripo/rig"];

/// Tripo's `rig_type` for a canonical skeleton (3D doc §3B: its `biped` is
/// litegen's `humanoid`, its `others` litegen's `other`).
fn rig_type(skeleton: &str) -> Option<&'static str> {
    Some(match skeleton {
        "humanoid" => "biped",
        "quadruped" => "quadruped",
        "hexapod" => "hexapod",
        "octopod" => "octopod",
        "avian" => "avian",
        "serpentine" => "serpentine",
        "aquatic" => "aquatic",
        "other" => "others",
        _ => return None,
    })
}

/// The canonical skeleton for a `rig_type` rig-check reported.
fn skeleton_of(rig_type: &str) -> &str {
    match rig_type {
        "biped" => "humanoid",
        "others" => "other",
        t => t,
    }
}

/// Tripo's rig model for a rig type: v3.0 is "recommended for humanoid
/// characters" and the only one taking `others`; v2.5 is for "non-humanoid
/// creatures".
fn rig_model(rig_type: &str) -> &'static str {
    match rig_type {
        "biped" | "others" => "v3.0-20260909",
        _ => "v2.5-20260210",
    }
}

/// Tripo's `spec` for a joint naming. `vendor` sends none, keeping Tripo's
/// own (`tripo`). `vrm`, `unity` and `unreal` need the v3.0 rig model, which
/// every humanoid rig here uses.
fn spec(naming: &str) -> Option<&'static str> {
    match naming {
        "vrm" => Some("vrm"),
        "mixamo" => Some("mixamo"),
        "unity" => Some("unity"),
        "unreal" => Some("unreal"),
        _ => None,
    }
}

/// The message out of Tripo's error envelope.
fn envelope_message(body: &Value) -> String {
    let msg = body["message"].as_str().filter(|m| !m.is_empty()).unwrap_or("Unknown error");
    let mut out = match body["code"].as_i64() {
        Some(c) => format!("{msg} (code {c})"),
        None => msg.to_string(),
    };
    if let Some(s) = body["suggestion"].as_str().filter(|s| !s.is_empty()) {
        out.push_str(&format!("; {s}"));
    }
    out
}

/// A failed task's own reason.
fn task_error(task: &Value) -> String {
    let msg = task["error_message"].as_str().filter(|m| !m.is_empty()).unwrap_or("no reason given");
    match task["error_code"].as_i64() {
        Some(c) => format!("{msg} (error_code {c})"),
        None => msg.to_string(),
    }
}

/// The container a result URL names, from its path's extension.
fn format_of(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let ext = path.rsplit('/').next().and_then(|n| n.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase());
    match ext {
        Some(e) if !e.is_empty() && e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()) => e,
        _ => DEFAULT_MODEL3D_FORMAT.to_string(),
    }
}

fn poll(status: GenerationStatus, progress: u8, error: Option<String>, files: Vec<Model3dFile>, meta: HashMap<String, Value>) -> Model3dGenerationPollResult {
    Model3dGenerationPollResult { status, progress, files, error, metadata: meta, stage_context: None, next_stage: None }
}

pub struct TripoModel3dProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    /// Tripo API calls.
    client: Client,
    /// Result downloads: no redirects, so a URL that passed the public-address
    /// check cannot bounce into a private one.
    asset_client: Client,
    /// Whether a result URL must resolve to a public address. Only the tests,
    /// whose mock vendor runs on loopback, turn it off.
    public_assets_only: bool,
    max_asset_bytes: u64,
    check_interval: Duration,
}

impl Default for TripoModel3dProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl TripoModel3dProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
            client: Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("failed to build HTTP client"),
            asset_client: crate::util::ssrf::no_redirect_download_client(Duration::from_secs(600), Duration::from_secs(60)),
            public_assets_only: true,
            max_asset_bytes: MAX_ASSET_BYTES,
            check_interval: CHECK_INTERVAL,
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
            .ok_or_else(|| ProviderError::NotConfigured("tripo".into()))?;
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

    /// Send a call and read Tripo's envelope: its `data` on `code: 0`.
    ///
    /// A call that creates a task is never retried once it may have reached
    /// Tripo: "A task-creation request may have succeeded even if its response
    /// was lost". A connection that never opened, a 429 and a 5xx are answers
    /// that it did not.
    async fn call(&self, req: RequestBuilder, what: &str, creates_task: bool) -> Result<Value, ProviderError> {
        let resp = req.send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("tripo {what} failed: {e}"),
            status_code: None,
            provider_error: None,
            retryable: e.is_connect() || !creates_task,
        })?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("tripo {what} reply could not be read: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: !status.is_success() || !creates_task,
        })?;
        let json: Option<Value> = serde_json::from_str(&text).ok();
        if !status.is_success() {
            let retryable = status.is_server_error()
                || status == StatusCode::REQUEST_TIMEOUT
                || status == StatusCode::TOO_MANY_REQUESTS;
            let message = match &json {
                Some(j) => envelope_message(j),
                None => text.chars().take(200).collect(),
            };
            return Err(ProviderError::RequestFailed {
                message: format!("tripo {what} returned HTTP {}: {message}", status.as_u16()),
                status_code: Some(status.as_u16()),
                provider_error: json,
                retryable,
            });
        }
        let Some(json) = json else {
            return Err(ProviderError::RequestFailed {
                message: format!("tripo {what} answered {} with a body that is not JSON", status.as_u16()),
                status_code: Some(status.as_u16()),
                provider_error: None,
                retryable: !creates_task,
            });
        };
        if json["code"].as_i64() != Some(0) {
            return Err(ProviderError::RequestFailed {
                message: format!("tripo {what} was refused: {}", envelope_message(&json)),
                status_code: Some(status.as_u16()),
                provider_error: Some(json),
                retryable: false,
            });
        }
        Ok(json["data"].clone())
    }

    /// A created task's id out of `data`.
    fn task_id(data: &Value, what: &str) -> Result<String, ProviderError> {
        data["task_id"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .ok_or_else(|| ProviderError::RequestFailed {
                message: format!("tripo {what} answered without a task_id"),
                status_code: None,
                provider_error: Some(data.clone()),
                retryable: false,
            })
    }

    /// Upload a source litegen holds: `POST /v3/files`, multipart `file`.
    async fn upload(&self, source: &SourceInput) -> Result<String, ProviderError> {
        let part = reqwest::multipart::Part::bytes(source.bytes.clone())
            .file_name(format!("source.{}", source.format))
            .mime_str("application/octet-stream")
            .map_err(|e| ProviderError::InvalidRequest(format!("tripo upload: {e}")))?;
        let form = reqwest::multipart::Form::new().part("file", part);
        let data = self.call(self.request(Method::POST, "files")?.multipart(form), "upload", false).await?;
        data["file_token"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "tripo upload answered without a file_token".into(),
                status_code: None,
                provider_error: Some(data.clone()),
                retryable: false,
            })
    }

    /// The source as Tripo's `input`: its own task id when the source came from
    /// a Tripo job on this account, else an uploaded file's token.
    async fn input(&self, source: &SourceInput) -> Result<(String, SourceDelivery), ProviderError> {
        match &source.vendor_task_id {
            Some(id) => Ok((id.clone(), SourceDelivery::VendorTask)),
            None => Ok((self.upload(source).await?, SourceDelivery::Upload)),
        }
    }

    fn served(model: &ModelSchema) -> Result<(), ProviderError> {
        if MODELS.contains(&model.id.as_str()) {
            Ok(())
        } else {
            Err(ProviderError::ModelNotSupported { model: model.id.clone(), provider: "tripo".into() })
        }
    }
}

#[async_trait]
impl Model3dProvider for TripoModel3dProvider {
    fn name(&self) -> &str {
        "tripo"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        // Guarded: `ApiKeyPool::shared` panics on an empty list, and the
        // registry builds every provider once with a default config.
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

    /// Tripo's free rig-check. Its verdict decides whether the billed rig
    /// runs: a mesh it cannot rig, or reads as another skeleton than the one
    /// asked for, is refused `not_riggable`. The upload (or chained task id)
    /// and the skeleton it found carry over to the submit.
    async fn preflight(&self, model: &ModelSchema, extras: &Model3dExtras) -> Result<Preflight, ProviderError> {
        Self::served(model)?;
        let source = extras
            .source
            .as_ref()
            .ok_or_else(|| ProviderError::InvalidRequest(format!("'{}' needs a source_model", model.id)))?;
        let (input, delivery) = self.input(source).await?;
        let created = self
            .call(self.request(Method::POST, "animations/rig-check")?.json(&json!({ "input": input })), "rig-check", true)
            .await?;
        let task = Self::task_id(&created, "rig-check")?;
        let started = Instant::now();
        loop {
            let t = self.call(self.request(Method::GET, &format!("tasks/{task}"))?, "rig-check poll", false).await?;
            match t["status"].as_str().unwrap_or("") {
                "success" => {
                    let found = t["output"]["rig_type"].as_str().unwrap_or("").to_string();
                    if t["output"]["riggable"].as_bool() != Some(true) {
                        return Ok(Preflight::Refuse {
                            code: "not_riggable",
                            message: "Tripo's rig-check says this mesh cannot be rigged".into(),
                        });
                    }
                    // rig-check never reports `others`, so `other` is never
                    // contradicted.
                    if let Some(want) = extras.skeleton.as_deref().filter(|w| *w != "other") {
                        if !found.is_empty() && rig_type(want) != Some(found.as_str()) {
                            return Ok(Preflight::Refuse {
                                code: "not_riggable",
                                message: format!(
                                    "Tripo's rig-check reads this mesh as a {} skeleton, not the {want} asked for",
                                    skeleton_of(&found)
                                ),
                            });
                        }
                    }
                    let delivery = match delivery {
                        SourceDelivery::VendorTask => "vendor_task",
                        _ => "upload",
                    };
                    return Ok(Preflight::Proceed {
                        context: Some(json!({ "input": input, "delivery": delivery, "rig_type": found })),
                    });
                }
                "failed" | "banned" | "expired" => {
                    return Ok(Preflight::Refuse {
                        code: "not_riggable",
                        message: format!("Tripo's rig-check failed: {}", task_error(&t)),
                    });
                }
                "cancelled" => {
                    return Err(ProviderError::RequestFailed {
                        message: "tripo rig-check was cancelled".into(),
                        status_code: None,
                        provider_error: Some(t),
                        retryable: true,
                    });
                }
                _ => {}
            }
            if started.elapsed() >= CHECK_BUDGET {
                return Err(ProviderError::RequestFailed {
                    message: format!("tripo rig-check did not finish within {}s", CHECK_BUDGET.as_secs()),
                    status_code: None,
                    provider_error: None,
                    retryable: true,
                });
            }
            tokio::time::sleep(self.check_interval).await;
        }
    }

    /// Submit the rig: `POST /v3/animations/rig`.
    async fn generate(
        &self,
        model: &ModelSchema,
        _base: &BaseGenerationRequest,
        extras: &Model3dExtras,
        _materialized: &MaterializedRequest,
    ) -> Result<Model3dGenerationHandle, ProviderError> {
        Self::served(model)?;
        let source = extras
            .source
            .as_ref()
            .ok_or_else(|| ProviderError::InvalidRequest(format!("'{}' needs a source_model", model.id)))?;
        let pre = extras.preflight.clone().unwrap_or(Value::Null);
        // The requested skeleton, else what rig-check found, else Tripo's own
        // default (biped).
        let rig = match extras.skeleton.as_deref() {
            Some(s) => rig_type(s).ok_or_else(|| ProviderError::InvalidRequest(format!("skeleton '{s}' has no Tripo rig_type")))?,
            None => pre["rig_type"].as_str().and_then(|found| rig_type(skeleton_of(found))).unwrap_or("biped"),
        };
        let naming = extras.joint_naming.as_deref().unwrap_or("vendor");
        let spec = spec(naming);
        if naming != "vendor" && (rig != "biped" || spec.is_none()) {
            return Err(ProviderError::InvalidRequest(format!(
                "joint_naming '{naming}' applies to humanoid rigs only: Tripo's spec \"only takes effect when rig_type is biped\""
            )));
        }
        let (input, delivery) = match (pre["input"].as_str(), pre["delivery"].as_str()) {
            (Some(i), Some("vendor_task")) => (i.to_string(), SourceDelivery::VendorTask),
            (Some(i), Some(_)) => (i.to_string(), SourceDelivery::Upload),
            _ => self.input(source).await?,
        };
        // One container from Tripo; litegen derives the rest. Its own fbx when
        // one was promised (`prefers_vendor_native`).
        let out_format = if extras
            .output_formats
            .iter()
            .any(|f| f.eq_ignore_ascii_case("fbx") && crate::proxy::model3d_convert::prefers_vendor_native(f))
        {
            "fbx"
        } else {
            "glb"
        };
        let mut body = json!({ "input": input, "model": rig_model(rig), "rig_type": rig, "out_format": out_format });
        if let Some(s) = spec {
            body["spec"] = json!(s);
        }
        let created = self
            .call(self.request(Method::POST, "animations/rig")?.json(&body), "rig", true)
            .await?;
        let task = Self::task_id(&created, "rig")?;
        source.record(delivery);
        Ok(Model3dGenerationHandle {
            provider_job_id: task.clone(),
            provider: "tripo".to_string(),
            model: model.id.clone(),
            stage_context: Some(json!({ "task_id": task })),
        })
    }

    /// `GET /v3/tasks/{task_id}`; on success, download `output.model_url`
    /// (a temporary signed URL) in the same call.
    async fn poll_status(&self, handle: &Model3dGenerationHandle) -> Result<Model3dGenerationPollResult, ProviderError> {
        let task = handle
            .stage_context
            .as_ref()
            .and_then(|c| c["task_id"].as_str())
            .filter(|t| !t.is_empty())
            .unwrap_or(&handle.provider_job_id)
            .to_string();
        let t = self.call(self.request(Method::GET, &format!("tasks/{task}"))?, "poll", false).await?;
        let meta: HashMap<String, Value> =
            HashMap::from([("provider".to_string(), json!("tripo")), ("task_id".to_string(), json!(task))]);
        let progress = t["progress"].as_u64().map(|p| p.min(100) as u8);
        let status = t["status"].as_str().unwrap_or("");
        match status {
            "queued" => Ok(poll(GenerationStatus::Pending, progress.unwrap_or(0), None, Vec::new(), meta)),
            "success" => {
                let Some(url) = t["output"]["model_url"].as_str().filter(|u| !u.is_empty()) else {
                    return Ok(poll(GenerationStatus::Failed, 100, Some("tripo finished without a model_url".into()), Vec::new(), meta));
                };
                let (bytes, served_ct) =
                    match super::download::fetch_asset(&self.asset_client, url, self.max_asset_bytes, self.public_assets_only, "tripo").await {
                        Ok(v) => v,
                        Err(AssetError::Transient(e)) => return Err(e),
                        Err(AssetError::Gone(why)) => {
                            return Ok(poll(GenerationStatus::Failed, 100, Some(format!("tripo model could not be downloaded: {why}")), Vec::new(), meta))
                        }
                    };
                let format = format_of(url);
                let content_type = crate::proxy::storage::model3d_content_type(&format, &served_ct).to_string();
                let mut files = vec![Model3dFile {
                    kind: Model3dAssetKind::Mesh,
                    format,
                    content_type,
                    bytes,
                    polycount: None,
                    width: None,
                    height: None,
                    manifest: None,
                }];
                // A preview is a courtesy: one that will not download never
                // fails a good mesh.
                if let Some(img) = t["output"]["rendered_image_url"].as_str().filter(|u| !u.is_empty()) {
                    match super::download::fetch_asset(&self.asset_client, img, self.max_asset_bytes, self.public_assets_only, "tripo").await {
                        Ok((bytes, ct)) => files.push(Model3dFile {
                            kind: Model3dAssetKind::Preview,
                            format: format_of(img),
                            content_type: ct,
                            bytes,
                            polycount: None,
                            width: None,
                            height: None,
                            manifest: None,
                        }),
                        Err(_) => tracing::warn!(task = %task, "tripo preview could not be downloaded; skipped"),
                    }
                }
                Ok(poll(GenerationStatus::Completed, 100, None, files, meta))
            }
            "failed" => Ok(poll(GenerationStatus::Failed, 100, Some(format!("tripo task failed: {}", task_error(&t))), Vec::new(), meta)),
            "cancelled" => Ok(poll(GenerationStatus::Failed, 100, Some("tripo task was cancelled".into()), Vec::new(), meta)),
            "banned" | "expired" => Ok(poll(GenerationStatus::Failed, 100, Some(format!("tripo task {status}: {}", task_error(&t))), Vec::new(), meta)),
            // `running`, and any status Tripo adds: a bounded wait, which the
            // poller's max age provides.
            _ => Ok(poll(GenerationStatus::Processing, progress.unwrap_or(50), None, Vec::new(), meta)),
        }
    }

    async fn estimate_cost(&self, model: &ModelSchema, _request: &Model3dGenerationRequest) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, None))
    }

    /// Tripo documents no free, read-only call to probe a key with, so this
    /// reports configuration only.
    async fn health_check(&self) -> HealthCheckResult {
        let configured = self.is_configured();
        HealthCheckResult {
            healthy: configured,
            message: if configured {
                "Tripo key configured (not probed: Tripo has no free status call)".into()
            } else {
                "Tripo not configured".into()
            },
            latency_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{SourceDelivery, SourceInput};
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn ref_schema(id: &str) -> ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        crate::capabilities::CapabilityRegistry::from_dir(&p).unwrap().get(id).unwrap().clone()
    }

    fn provider(server: &MockServer) -> TripoModel3dProvider {
        let mut p = TripoModel3dProvider::new();
        p.configure(ProviderInstanceConfig {
            api_key: String::new(),
            api_keys: vec![ApiKeyEntry { key: "tsk_pooled".into(), weight: 1, label: None }],
            api_base: Some(format!("{}/v3", server.uri())),
            ..Default::default()
        });
        p.public_assets_only = false;
        p.check_interval = Duration::from_millis(5);
        p
    }

    fn base() -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: String::new(), model: "tripo/rig".into(), n: 1, negative_prompt: None, seed: None,
            reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    fn source(vendor_task_id: Option<&str>) -> SourceInput {
        SourceInput {
            bytes: crate::providers::model3d::glb::generate_cube_glb("a knight"),
            format: "glb".into(),
            sha256: "00".into(),
            facts: crate::proxy::model3d_source::SourceFacts { triangles: 12, skinned: false, textured: false },
            vendor_task_id: vendor_task_id.map(str::to_string),
            delivered: Default::default(),
        }
    }

    fn extras(formats: &[&str], skeleton: Option<&str>, naming: Option<&str>, src: SourceInput) -> Model3dExtras {
        Model3dExtras {
            output_formats: formats.iter().map(|f| f.to_string()).collect(),
            texture: None, pbr: None, target_polycount: None, symmetry: None, topology: None, rig: None,
            skeleton: skeleton.map(str::to_string),
            joint_naming: naming.map(str::to_string),
            height_m: None,
            extra: None,
            source: Some(src),
            preflight: None,
        }
    }

    fn no_refs() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    fn ok(data: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "data": data }))
    }

    async fn mount_upload(server: &MockServer, token: &str) {
        Mock::given(method("POST"))
            .and(path("/v3/files"))
            .and(header("authorization", "Bearer tsk_pooled"))
            .respond_with(ok(json!({ "file_token": token })))
            .mount(server)
            .await;
    }

    async fn mount_task(server: &MockServer, id: &str, data: Value) {
        Mock::given(method("GET"))
            .and(path(format!("/v3/tasks/{id}")))
            .and(header("authorization", "Bearer tsk_pooled"))
            .respond_with(ok(data))
            .mount(server)
            .await;
    }

    async fn mount_check(server: &MockServer, input: &str, riggable: bool, rig_type: &str) {
        Mock::given(method("POST"))
            .and(path("/v3/animations/rig-check"))
            .and(body_json(json!({ "input": input })))
            .respond_with(ok(json!({ "task_id": "chk-1" })))
            .mount(server)
            .await;
        mount_task(server, "chk-1", json!({
            "task_id": "chk-1", "type": "animate_prerigcheck", "status": "success", "progress": 100,
            "output": { "riggable": riggable, "rig_type": rig_type },
        })).await;
    }

    async fn received(server: &MockServer, at: &str) -> Vec<Request> {
        server.received_requests().await.unwrap().into_iter().filter(|r| r.url.path() == at).collect()
    }

    // ─── Preflight: rig-check (free) ────────────────────────────────────────
    // @see <https://developers.tripo3d.ai/en/docs/animations-rig-check>

    #[tokio::test]
    async fn the_preflight_uploads_the_source_and_asks_rig_check() {
        let server = MockServer::start().await;
        mount_upload(&server, "file_abc").await;
        mount_check(&server, "file_abc", true, "biped").await;
        let p = provider(&server);
        let x = extras(&["glb"], None, None, source(None));
        let verdict = p.preflight(&ref_schema("tripo/rig"), &x).await.unwrap();
        let Preflight::Proceed { context: Some(ctx) } = verdict else { panic!("{verdict:?}") };
        assert_eq!(ctx["input"], "file_abc");
        assert_eq!(ctx["delivery"], "upload");
        assert_eq!(ctx["rig_type"], "biped", "what rig-check found");
        let upload = &received(&server, "/v3/files").await[0];
        let body = String::from_utf8_lossy(&upload.body);
        assert!(body.contains("name=\"file\"") && body.contains("filename=\"source.glb\""), "multipart field `file`: {body}");
    }

    #[tokio::test]
    async fn a_source_rig_check_refuses_is_not_riggable() {
        let server = MockServer::start().await;
        mount_upload(&server, "file_abc").await;
        mount_check(&server, "file_abc", false, "biped").await;
        let verdict = provider(&server).preflight(&ref_schema("tripo/rig"), &extras(&["glb"], None, None, source(None))).await.unwrap();
        assert!(matches!(&verdict, Preflight::Refuse { code: "not_riggable", .. }), "{verdict:?}");
        assert!(received(&server, "/v3/animations/rig").await.is_empty(), "nothing billed");
    }

    #[tokio::test]
    async fn a_skeleton_rig_check_did_not_find_is_not_riggable() {
        let server = MockServer::start().await;
        mount_upload(&server, "file_abc").await;
        mount_check(&server, "file_abc", true, "quadruped").await;
        let verdict = provider(&server)
            .preflight(&ref_schema("tripo/rig"), &extras(&["glb"], Some("humanoid"), None, source(None)))
            .await
            .unwrap();
        let Preflight::Refuse { code, message } = verdict else { panic!("{verdict:?}") };
        assert_eq!(code, "not_riggable");
        assert!(message.contains("quadruped") && message.contains("humanoid"), "{message}");
    }

    #[tokio::test]
    async fn a_chained_source_goes_to_tripo_by_its_own_task_id() {
        let server = MockServer::start().await;
        mount_check(&server, "tripo-gen-1", true, "biped").await;
        let verdict = provider(&server)
            .preflight(&ref_schema("tripo/rig"), &extras(&["glb"], None, None, source(Some("tripo-gen-1"))))
            .await
            .unwrap();
        let Preflight::Proceed { context: Some(ctx) } = verdict else { panic!("{verdict:?}") };
        assert_eq!((ctx["input"].as_str(), ctx["delivery"].as_str()), (Some("tripo-gen-1"), Some("vendor_task")));
        assert!(received(&server, "/v3/files").await.is_empty(), "no upload");
    }

    // ─── Rig: the billed call ───────────────────────────────────────────────
    // @see <https://developers.tripo3d.ai/en/docs/animations-rig>

    async fn submit(server: &MockServer, x: Model3dExtras, preflight: Value) -> (Model3dGenerationHandle, Value, Model3dExtras) {
        Mock::given(method("POST"))
            .and(path("/v3/animations/rig"))
            .and(header("authorization", "Bearer tsk_pooled"))
            .respond_with(ok(json!({ "task_id": "rig-1" })))
            .mount(server)
            .await;
        let mut x = x;
        x.preflight = Some(preflight);
        let h = provider(server).generate(&ref_schema("tripo/rig"), &base(), &x, &no_refs()).await.unwrap();
        let sent = received(server, "/v3/animations/rig").await;
        assert_eq!(sent.len(), 1);
        (h, serde_json::from_slice(&sent[0].body).unwrap(), x)
    }

    #[tokio::test]
    async fn a_humanoid_rig_asks_for_v3_and_the_requested_naming() {
        let server = MockServer::start().await;
        let (h, body, x) = submit(
            &server,
            extras(&["glb"], Some("humanoid"), Some("vrm"), source(None)),
            json!({ "input": "file_abc", "delivery": "upload", "rig_type": "biped" }),
        ).await;
        assert_eq!(body, json!({ "input": "file_abc", "model": "v3.0-20260909", "rig_type": "biped", "spec": "vrm", "out_format": "glb" }));
        assert_eq!(h.provider_job_id, "rig-1");
        assert_eq!(h.stage_context, Some(json!({ "task_id": "rig-1" })));
        assert_eq!(x.source.as_ref().unwrap().delivery(), Some(SourceDelivery::Upload));
    }

    #[tokio::test]
    async fn vendor_naming_sends_no_spec_and_a_creature_asks_for_v2_5() {
        let server = MockServer::start().await;
        let (_, body, _) = submit(
            &server,
            extras(&["glb"], Some("quadruped"), None, source(None)),
            json!({ "input": "file_abc", "delivery": "upload", "rig_type": "quadruped" }),
        ).await;
        assert_eq!(body, json!({ "input": "file_abc", "model": "v2.5-20260210", "rig_type": "quadruped", "out_format": "glb" }));
    }

    #[tokio::test]
    async fn an_omitted_skeleton_rigs_what_rig_check_found() {
        let server = MockServer::start().await;
        let (_, body, x) = submit(
            &server,
            extras(&["glb"], None, None, source(Some("tripo-gen-1"))),
            json!({ "input": "tripo-gen-1", "delivery": "vendor_task", "rig_type": "avian" }),
        ).await;
        assert_eq!(body["rig_type"], "avian");
        assert_eq!(body["input"], "tripo-gen-1");
        assert_eq!(x.source.as_ref().unwrap().delivery(), Some(SourceDelivery::VendorTask));
    }

    #[tokio::test]
    async fn an_fbx_promise_asks_tripo_for_its_own_fbx() {
        let server = MockServer::start().await;
        let (_, body, _) = submit(
            &server,
            extras(&["glb", "fbx"], Some("humanoid"), None, source(None)),
            json!({ "input": "file_abc", "delivery": "upload", "rig_type": "biped" }),
        ).await;
        assert_eq!(body["out_format"], "fbx");
    }

    #[tokio::test]
    async fn joint_naming_on_a_creature_is_refused_before_any_call() {
        let server = MockServer::start().await;
        let mut x = extras(&["glb"], Some("quadruped"), Some("vrm"), source(None));
        x.preflight = Some(json!({ "input": "file_abc", "delivery": "upload", "rig_type": "quadruped" }));
        let err = provider(&server).generate(&ref_schema("tripo/rig"), &base(), &x, &no_refs()).await.unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(ref m) if m.contains("humanoid")), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn tripos_error_envelope_reaches_the_caller() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/animations/rig"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({
                "code": 2010, "status": "error", "message": "your credits are insufficient",
                "suggestion": "top up", "request_id": "req_1",
            })))
            .mount(&server)
            .await;
        let mut x = extras(&["glb"], None, None, source(None));
        x.preflight = Some(json!({ "input": "file_abc", "delivery": "upload", "rig_type": "biped" }));
        let err = provider(&server).generate(&ref_schema("tripo/rig"), &base(), &x, &no_refs()).await.unwrap_err();
        assert!(!err.is_retryable(), "{err:?}");
        assert!(err.to_string().contains("your credits are insufficient") && err.to_string().contains("2010"), "{err}");

        let busy = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/animations/rig"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({ "code": 2000, "message": "concurrency limit" })))
            .mount(&busy)
            .await;
        let err = provider(&busy).generate(&ref_schema("tripo/rig"), &base(), &x, &no_refs()).await.unwrap_err();
        assert!(err.is_retryable(), "a 429 was refused, not accepted: {err:?}");
    }

    // ─── Poll ───────────────────────────────────────────────────────────────
    // @see <https://developers.tripo3d.ai/en/docs/task-query>

    fn rig_handle() -> Model3dGenerationHandle {
        Model3dGenerationHandle {
            provider_job_id: "rig-1".into(),
            provider: "tripo".into(),
            model: "tripo/rig".into(),
            stage_context: Some(json!({ "task_id": "rig-1" })),
        }
    }

    #[tokio::test]
    async fn a_finished_rig_is_downloaded_and_tagged() {
        let server = MockServer::start().await;
        let rig = crate::providers::model3d::humanoid::rigged_humanoid_glb(&crate::providers::model3d::humanoid::MOCK_JOINTS);
        Mock::given(method("GET"))
            .and(path("/cdn/rigged.glb"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(rig.clone()))
            .mount(&server)
            .await;
        mount_task(&server, "rig-1", json!({
            "task_id": "rig-1", "type": "animate_rig", "status": "success", "progress": 100,
            "output": { "model_url": format!("{}/cdn/rigged.glb?Expires=1", server.uri()) },
        })).await;
        let done = provider(&server).poll_status(&rig_handle()).await.unwrap();
        assert_eq!(done.status, GenerationStatus::Completed, "{:?}", done.error);
        assert_eq!(done.files.len(), 1);
        let f = &done.files[0];
        assert_eq!((f.kind, f.format.as_str(), f.content_type.as_str()), (Model3dAssetKind::Mesh, "glb", "model/gltf-binary"));
        assert_eq!(f.bytes, rig);
        let cdn = received(&server, "/cdn/rigged.glb").await;
        assert!(cdn[0].headers.get("authorization").is_none(), "no key to the CDN");
    }

    #[tokio::test]
    async fn every_task_status_is_mapped() {
        for (status, extra, expect) in [
            ("queued", json!({}), GenerationStatus::Pending),
            ("running", json!({}), GenerationStatus::Processing),
            ("unknown", json!({}), GenerationStatus::Processing),
            ("failed", json!({ "error_code": 2008, "error_message": "content policy" }), GenerationStatus::Failed),
            ("cancelled", json!({}), GenerationStatus::Failed),
            ("banned", json!({}), GenerationStatus::Failed),
            ("expired", json!({}), GenerationStatus::Failed),
            ("success", json!({ "output": {} }), GenerationStatus::Failed),
        ] {
            let server = MockServer::start().await;
            let mut data = json!({ "task_id": "rig-1", "status": status, "progress": 40 });
            if let Value::Object(m) = extra {
                for (k, v) in m {
                    data[k] = v;
                }
            }
            mount_task(&server, "rig-1", data).await;
            let r = provider(&server).poll_status(&rig_handle()).await.unwrap();
            assert_eq!(r.status, expect, "{status}");
            if expect == GenerationStatus::Failed {
                assert!(r.error.as_deref().is_some_and(|e| !e.is_empty()), "{status}: a reason");
            }
            if status == "failed" {
                assert!(r.error.as_deref().unwrap().contains("content policy"), "{:?}", r.error);
            }
            if status == "running" {
                assert_eq!(r.progress, 40);
            }
        }
    }

    #[tokio::test]
    async fn a_poll_tripo_cannot_answer_yet_is_retried_and_a_lost_task_is_not() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v3/tasks/rig-1"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        assert!(provider(&server).poll_status(&rig_handle()).await.unwrap_err().is_retryable());
        let gone = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v3/tasks/rig-1"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "code": 2001, "message": "task not found" })))
            .mount(&gone)
            .await;
        assert!(!provider(&gone).poll_status(&rig_handle()).await.unwrap_err().is_retryable());
    }

    // ─── Catalog ↔ adapter ──────────────────────────────────────────────────

    #[test]
    fn every_tripo_row_is_served_here_and_every_model_here_is_in_the_catalog() {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&p).unwrap();
        let rows: Vec<String> = catalog
            .for_provider("tripo")
            .filter(|m| m.media_type == crate::capabilities::MediaType::Model3d)
            .map(|m| m.id.clone())
            .collect();
        let mut sorted_rows = rows.clone();
        sorted_rows.sort();
        let mut served: Vec<String> = MODELS.iter().map(|m| m.to_string()).collect();
        served.sort();
        assert_eq!(sorted_rows, served);
    }

    #[test]
    fn credentials_satisfy_the_bearer_scheme() {
        let mut p = TripoModel3dProvider::new();
        p.configure(ProviderInstanceConfig { credentials: ProviderCredentials { api_key: Some("tsk_1".into()), ..Default::default() }, ..Default::default() });
        assert!(p.is_configured());
        let mut empty = TripoModel3dProvider::new();
        empty.configure(ProviderInstanceConfig::default());
        assert!(!empty.is_configured());
    }
}
