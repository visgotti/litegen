//! fal.ai 3D (mesh) generation — litegen's first real `Model3dProvider`.
//!
//! fal hosts each 3D model as its own queue endpoint with its own input schema;
//! there is no shared 3D vocabulary between them (see `EndpointSpec`). Every
//! endpoint is driven through fal's queue API, authenticated with
//! `Authorization: Key <FAL_KEY>` — not `Bearer`, the single most common mistake
//! against this API:
//!
//! ```text
//! POST {queue}/{endpoint}                        submit → request_id, status_url, response_url, cancel_url
//! GET  {queue}/{endpoint}/requests/{id}/status   IN_QUEUE | IN_PROGRESS | COMPLETED (+ error, error_type)
//! GET  {queue}/{endpoint}/requests/{id}          the stored result: model_mesh (+ textures, on Rodin)
//! PUT  {queue}/{endpoint}/requests/{id}/cancel   202 CANCELLATION_REQUESTED | 400 ALREADY_COMPLETED | 404 NOT_FOUND
//! ```
//!
//! # References (re-read 2026-10-12)
//!
//! - Queue API — submit, the status fields (a failed run is `COMPLETED` with
//!   `error` and `error_type`; there is no FAILED status), result, cancel:
//!   <https://fal.ai/docs/documentation/model-apis/inference/queue>
//! - Model errors — `detail` is an array of `{loc, msg, type, url, ctx}`, and
//!   `X-Fal-Needs-Retry` says whether resubmitting can help:
//!   <https://fal.ai/docs/documentation/model-apis/errors>
//! - Request errors — `detail` is a string beside `error_type`, which is also
//!   sent as the `X-Fal-Error-Type` header:
//!   <https://fal.ai/docs/documentation/model-apis/request-errors>
//! - Authentication: <https://fal.ai/docs/documentation/setting-up/authentication>
//! - File inputs, including base64 `data:` URIs:
//!   <https://fal.ai/docs/documentation/model-apis/fal-cdn>
//! - Retention — output media on the CDN expires per the account's lifecycle
//!   settings; request payloads (and so the stored result) are kept 30 days:
//!   <https://fal.ai/docs/documentation/model-apis/media-expiration>
//! - Key check behind `health_check` (Platform API; free, read-only):
//!   <https://fal.ai/docs/platform-apis/v1/models/pricing>
//! - Each endpoint's input/output schema, the source of every field in
//!   `resolve_spec`: `https://fal.ai/api/openapi/queue/openapi.json?endpoint_id={endpoint}`
//! - Each endpoint's price: `https://fal.ai/models/{endpoint}/llms.txt`
//!
//! How each canonical param lands on each endpoint is tabled in the
//! `providers::model3d` module docs.

use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::providers::{
    build_cost_estimate, ApiKeyPool, BaseGenerationRequest, CancelOutcome, HealthCheckResult,
    Model3dExtras, Model3dFile, Model3dGenerationHandle, Model3dGenerationPollResult,
    Model3dProvider, ProviderError, ProviderInstanceConfig,
};
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::proxy::model3d_convert;
use crate::types::*;

/// fal's queue API. `api_base` in the provider config overrides it, which is
/// how the tests point the adapter at a local mock.
const DEFAULT_QUEUE_BASE: &str = "https://queue.fal.run";

/// fal's Platform API, used only to check a key. `options.platform_api_base`
/// overrides it.
const DEFAULT_PLATFORM_BASE: &str = "https://api.fal.ai/v1";

/// The endpoint `health_check` asks the price of — any public id will do.
const HEALTH_CHECK_ENDPOINT: &str = "fal-ai/trellis";

/// Largest mesh or texture pulled down from a vendor URL. The bytes are held in
/// memory through rehost, and `mesh::MAX_INPUT_BYTES` refuses to parse anything
/// larger anyway, so accepting more would only mean buffering a file we are
/// about to reject.
const MAX_ASSET_BYTES: u64 = 128 * 1024 * 1024;

/// fal reports queue position, never percent complete, and inventing a curve
/// from it would be a lie with a number attached. Two honest steps instead.
const PROGRESS_QUEUED: u8 = 5;
const PROGRESS_RUNNING: u8 = 50;

/// Every litegen model id this adapter serves — the arms of `resolve_spec`. A
/// test ties it to the catalog in both directions, so a fal 3D row cannot
/// reach `models/fal.yaml` without an endpoint here, or the reverse.
pub const MODELS: &[&str] = &["fal/hyper3d-rodin", "fal/trellis", "fal/hunyuan3d-v2"];

/// What one fal 3D endpoint's input schema accepts.
///
/// Declared per endpoint because fal has no shared 3D vocabulary: the format
/// parameter is spelled four different ways across its 3D catalogue and most
/// endpoints have none; the image field is `image_url`, `input_image_url`, or
/// an ARRAY under `input_image_urls`; Rodin's seed is a u16 where the rest of
/// fal takes u32. Sending a field an endpoint does not define is not harmless,
/// so nothing here is guessed.
#[derive(Debug, Clone, Copy)]
struct EndpointSpec {
    /// Queue endpoint for a request without reference images; `None` when the
    /// model has no text-to-3D arm.
    text_to_3d: Option<&'static str>,
    /// Queue endpoint for a request with reference images; `None` when the
    /// model has no image arm.
    image_to_3d: Option<&'static str>,
    /// Whether the input schema defines `prompt` at all.
    prompt: bool,
    /// The field carrying the reference image(s).
    image_field: &'static str,
    /// Whether `image_field` takes every image (an array) or one.
    image_is_array: bool,
    /// The endpoint's own name for the output container, when it lets you pick.
    format_field: Option<&'static str>,
    /// The containers `format_field` accepts.
    format_values: &'static [&'static str],
    /// The largest seed the endpoint accepts.
    seed_max: Option<i64>,
    /// Boolean texture toggle — where canonical `texture` goes.
    texture_field: Option<&'static str>,
    /// `PBR | Shaded` field — where canonical `pbr` goes.
    material_field: Option<&'static str>,
}

impl EndpointSpec {
    /// Every field this adapter fills from a validated canonical param. `extra`
    /// may never write one: in lax mode the validator lets every `extra` key
    /// through, and an `extra.geometry_file_format` overriding the container
    /// would make fal return something other than what the completion guard
    /// enforces — failing the generation after it was billed.
    fn mapped_fields(&self) -> Vec<&'static str> {
        let mut out = vec!["prompt", "seed", self.image_field];
        out.extend(self.format_field);
        out.extend(self.texture_field);
        out.extend(self.material_field);
        out
    }

    /// The endpoint a job was submitted to, recovered from the model alone —
    /// for a row whose `stage_context` never landed. Unambiguous when the model
    /// has one endpoint for both arms, or only one arm.
    fn sole_endpoint(&self) -> Option<&'static str> {
        match (self.text_to_3d, self.image_to_3d) {
            (Some(t), Some(i)) if t == i => Some(t),
            (Some(t), None) => Some(t),
            (None, Some(i)) => Some(i),
            _ => None,
        }
    }
}

/// Resolve a litegen model id to its fal endpoint(s) and field support.
fn resolve_spec(model: &str) -> Option<EndpointSpec> {
    match model {
        // Deemos Hyper3D Rodin. One endpoint serves both arms; its prompt is
        // "Required for Text-to-3D mode. Optional for Image-to-3D mode."
        //
        // Hyper3dRodinInput: prompt (maxLength 1024), input_image_urls[],
        //   condition_mode (fuse | concat), seed (0..=65535),
        //   geometry_file_format (glb | usdz | fbx | obj | stl; default glb),
        //   material (PBR | Shaded; default PBR), quality, tier, use_hyper,
        //   TAPose, bbox_condition, addons.
        // Hyper3dRodinOutput: model_mesh (File), seed, textures (Image[]).
        //
        // @see <https://fal.ai/models/fal-ai/hyper3d/rodin/api>
        // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/hyper3d/rodin>
        // @see <https://fal.ai/models/fal-ai/hyper3d/rodin/llms.txt> — "$0.4 per generations"
        "fal/hyper3d-rodin" => Some(EndpointSpec {
            text_to_3d: Some("fal-ai/hyper3d/rodin"),
            image_to_3d: Some("fal-ai/hyper3d/rodin"),
            prompt: true,
            image_field: "input_image_urls",
            image_is_array: true,
            format_field: Some("geometry_file_format"),
            format_values: &["glb", "usdz", "fbx", "obj", "stl"],
            seed_max: Some(65_535),
            texture_field: None,
            material_field: Some("material"),
        }),
        // Microsoft TRELLIS. Image-to-3D only, GLB only.
        //
        // TrellisInput: image_url (REQUIRED), seed, ss_guidance_strength,
        //   ss_sampling_steps, slat_guidance_strength, slat_sampling_steps,
        //   mesh_simplify, texture_size (512 | 1024 | 2048). No prompt.
        // TrellisOutput: model_mesh (File), timings.
        //
        // @see <https://fal.ai/models/fal-ai/trellis/api>
        // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/trellis>
        // @see <https://fal.ai/models/fal-ai/trellis/llms.txt> — "$0.02 per generations"
        "fal/trellis" => Some(EndpointSpec {
            text_to_3d: None,
            image_to_3d: Some("fal-ai/trellis"),
            prompt: false,
            image_field: "image_url",
            image_is_array: false,
            format_field: None,
            format_values: &[],
            // An unbounded integer in the schema; u32 is what fal documents
            // everywhere else.
            seed_max: Some(i64::from(u32::MAX)),
            texture_field: None,
            material_field: None,
        }),
        // Tencent Hunyuan3D v2. Image-to-3D only, GLB only.
        //
        // Hunyuan3dV2Input: input_image_url (REQUIRED), seed,
        //   num_inference_steps, guidance_scale, octree_resolution,
        //   textured_mesh ("the price charged would be 3 times that of white
        //   mesh"). No prompt.
        // Hunyuan3dV2Output: model_mesh (File), seed.
        //
        // @see <https://fal.ai/models/fal-ai/hunyuan3d/v2/api>
        // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/hunyuan3d/v2>
        // @see <https://fal.ai/models/fal-ai/hunyuan3d/v2/llms.txt> — "$0.16 per generations"
        "fal/hunyuan3d-v2" => Some(EndpointSpec {
            text_to_3d: None,
            image_to_3d: Some("fal-ai/hunyuan3d/v2"),
            prompt: false,
            image_field: "input_image_url",
            image_is_array: false,
            format_field: None,
            format_values: &[],
            seed_max: Some(i64::from(u32::MAX)),
            texture_field: Some("textured_mesh"),
            material_field: None,
        }),
        _ => None,
    }
}

/// The ONE container to ask the vendor for — `None` when the endpoint emits GLB
/// with no choice.
///
/// litegen derives the other containers locally, so one is all a vendor ever
/// needs to produce. A requested container the vendor's own export should
/// supply (`model3d_convert::prefers_vendor_native`: fbx, usdz, anything not
/// derivable) wins when the endpoint offers it — asking for glb when the caller
/// wanted an underivable fbx would deliver a glb, and the completion guard would
/// then fail a generation the vendor could have fulfilled, after it was billed.
/// Otherwise GLB: the conversion source carrying the most.
fn vendor_format(spec: &EndpointSpec, requested: &[String]) -> Option<String> {
    spec.format_field?;
    let offers = |f: &str| spec.format_values.iter().any(|v| v.eq_ignore_ascii_case(f));
    if let Some(native) = requested
        .iter()
        .find(|r| model3d_convert::prefers_vendor_native(r) && offers(r))
    {
        return Some(native.to_ascii_lowercase());
    }
    if offers(DEFAULT_MODEL3D_FORMAT) {
        return Some(DEFAULT_MODEL3D_FORMAT.to_string());
    }
    // No GLB on offer: something the caller actually wants, then whatever the
    // endpoint lists first.
    requested
        .iter()
        .find(|r| offers(r))
        .map(|r| r.to_ascii_lowercase())
        .or_else(|| spec.format_values.first().map(|s| s.to_string()))
}

/// Whether asking again can help: a server-side failure, a timeout, or fal's
/// concurrency limiter (429 — two runs at once on a new account).
fn transient(status: StatusCode) -> bool {
    status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
}

/// A human-readable message out of any of fal's THREE error envelopes.
///
/// They are genuinely different shapes, and reading only one turns a precise
/// vendor message into "Unknown error" exactly when someone needs it:
///
/// - model errors: `detail` is an ARRAY of `{loc, msg, type, …}`;
/// - request errors: `detail` is a STRING beside `error_type`;
/// - endpoints still being migrated use neither.
///
/// @see <https://fal.ai/docs/documentation/model-apis/errors>
/// @see <https://fal.ai/docs/documentation/model-apis/request-errors>
fn fal_error_message(body: &Value) -> String {
    if let Some(s) = body["detail"].as_str().filter(|s| !s.is_empty()) {
        return match body["error_type"].as_str().filter(|t| !t.is_empty()) {
            Some(t) => format!("{s} ({t})"),
            None => s.to_string(),
        };
    }
    if let Some(items) = body["detail"].as_array() {
        let msgs: Vec<String> = items
            .iter()
            .filter_map(|d| {
                let msg = d["msg"].as_str()?;
                // `loc` names the offending field, which is the whole value of
                // a 422 — "field required" alone says nothing.
                match d["loc"].as_array() {
                    Some(loc) if !loc.is_empty() => {
                        let path: Vec<String> = loc
                            .iter()
                            .map(|p| p.as_str().map(str::to_string).unwrap_or_else(|| p.to_string()))
                            .collect();
                        Some(format!("{}: {msg}", path.join(".")))
                    }
                    _ => Some(msg.to_string()),
                }
            })
            .collect();
        if !msgs.is_empty() {
            return msgs.join("; ");
        }
    }
    body["message"].as_str().unwrap_or("Unknown error").to_string()
}

/// The container a vendor URL or content type describes.
///
/// The URL's extension is trusted ahead of the content type because fal serves
/// meshes from object storage, which labels everything
/// `application/octet-stream` — while the filename it generates does carry the
/// real extension.
fn format_of(url: &str, content_type: &str) -> String {
    let ext = url
        .split('?')
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.').map(|(_, e)| e))
        .unwrap_or("")
        .to_ascii_lowercase();
    if !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return ext;
    }
    match content_type {
        "model/gltf-binary" => "glb".into(),
        "model/gltf+json" => "gltf".into(),
        "model/obj" => "obj".into(),
        "model/stl" => "stl".into(),
        "model/vnd.usdz+zip" => "usdz".into(),
        "image/png" => "png".into(),
        "image/jpeg" => "jpg".into(),
        _ => DEFAULT_MODEL3D_FORMAT.into(),
    }
}

/// `{url, content_type}` out of a fal `File` (or `Image`) object.
fn file_url(v: &Value) -> Option<(&str, &str)> {
    let url = v.get("url")?.as_str()?;
    let ct = v.get("content_type").and_then(|c| c.as_str()).unwrap_or("");
    Some((url, ct))
}

/// A response header as text, if present.
fn header<'a>(headers: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty())
}

/// fal's `X-Fal-Needs-Retry`, when it sent one.
fn needs_retry(headers: &reqwest::header::HeaderMap) -> Option<bool> {
    header(headers, "x-fal-needs-retry").map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

/// One reply from a fal queue URL, read whole: status, fal's error-type header,
/// and the body — parsed when it is JSON, kept as text either way.
struct QueueReply {
    status: StatusCode,
    error_type: Option<String>,
    json: Option<Value>,
    text: String,
}

impl QueueReply {
    /// What went wrong, in fal's words when it sent any.
    fn describe(&self) -> String {
        match &self.json {
            Some(j) => {
                let msg = fal_error_message(j);
                match &self.error_type {
                    Some(t) if !msg.contains(t.as_str()) => format!("{msg} ({t})"),
                    _ => msg,
                }
            }
            None => {
                let snippet: String = self.text.chars().take(200).collect();
                if snippet.trim().is_empty() { "no body".to_string() } else { snippet }
            }
        }
    }

    /// Whether this non-2xx reply is fal's VERDICT on the run, rather than the
    /// queue being unreachable for a moment. Anything outside 5xx/408/429 is
    /// definitive. A transient code counts only when it carries fal's own error
    /// envelope — a stored request error (`error_type`) or a model error
    /// (`detail` array) — and not, say, a gateway's HTML page.
    fn is_verdict(&self) -> bool {
        if !transient(self.status) {
            return true;
        }
        self.error_type.is_some()
            || self.json.as_ref().is_some_and(|j| {
                j.get("error_type").is_some_and(Value::is_string) || j["detail"].is_array()
            })
    }

    fn into_error(self, what: &str, retryable: bool) -> ProviderError {
        ProviderError::RequestFailed {
            message: format!("fal 3d {what} returned HTTP {}: {}", self.status.as_u16(), self.describe()),
            status_code: Some(self.status.as_u16()),
            provider_error: self.json,
            retryable,
        }
    }
}

/// Why an asset did not download.
enum AssetError {
    /// Asking again may work: the connection dropped, or the host answered
    /// 5xx / 408 / 429.
    Transient(ProviderError),
    /// It never will: the URL is gone, refused, private, or too large.
    Gone(String),
}

/// Where a job lives on fal — rebuilt from the handle on every poll, so the
/// poller can use it after a restart.
struct FalJob {
    request_id: String,
    status_url: String,
    response_url: String,
    cancel_url: String,
}

pub struct FalModel3dProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    /// Queue and Platform API calls.
    client: Client,
    /// Asset downloads. Follows no redirects, so a URL that passed the
    /// public-address check cannot bounce into a private one — fal's CDN
    /// answers directly (checked 2026-10-12).
    asset_client: Client,
    /// Whether an asset URL must resolve to a public address. A result naming
    /// `http://169.254.169.254/…` would otherwise be fetched from inside the
    /// deployment and served back to the caller as a mesh. Only the tests,
    /// whose mock vendor runs on loopback, turn it off.
    public_assets_only: bool,
    max_asset_bytes: u64,
}

impl Default for FalModel3dProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl FalModel3dProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .expect("failed to build HTTP client"),
            asset_client: crate::util::ssrf::no_redirect_client_with_timeout(
                std::time::Duration::from_secs(600),
            ),
            public_assets_only: true,
            max_asset_bytes: MAX_ASSET_BYTES,
        }
    }

    fn api_key(&self) -> Result<String, ProviderError> {
        if let Some(pool) = &self.key_pool {
            return Ok(pool.next().to_string());
        }
        self.config
            .as_ref()
            .map(|c| c.api_key.clone())
            .ok_or_else(|| ProviderError::NotConfigured("fal".into()))
    }

    fn queue_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or(DEFAULT_QUEUE_BASE)
            .trim_end_matches('/')
    }

    fn platform_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.options.as_ref())
            .and_then(|o| o["platform_api_base"].as_str())
            .unwrap_or(DEFAULT_PLATFORM_BASE)
            .trim_end_matches('/')
    }

    /// A queue URL fal handed back, kept only when it is on the origin this
    /// provider is configured for. The API key travels with every queue call,
    /// so a URL from anywhere else is rebuilt from the base instead — the key
    /// is never sent to a host the operator did not configure.
    fn trusted_queue_url(&self, candidate: Option<&str>) -> Option<String> {
        let candidate = candidate.filter(|u| !u.is_empty())?;
        let url = reqwest::Url::parse(candidate).ok()?;
        let base = reqwest::Url::parse(self.queue_base()).ok()?;
        let same_origin = url.scheme() == base.scheme()
            && url.host_str() == base.host_str()
            && url.port_or_known_default() == base.port_or_known_default();
        same_origin.then(|| candidate.to_string())
    }

    fn job(&self, handle: &Model3dGenerationHandle) -> Result<FalJob, ProviderError> {
        let ctx = handle.stage_context.as_ref();
        let field = |k: &str| ctx.and_then(|c| c.get(k)).and_then(Value::as_str);
        let endpoint = field("endpoint")
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .or_else(|| resolve_spec(&handle.model).and_then(|s| s.sole_endpoint()).map(str::to_string))
            .ok_or_else(|| {
                ProviderError::InvalidRequest(format!(
                    "fal 3d job '{}' records no endpoint, and '{}' has more than one",
                    handle.provider_job_id, handle.model
                ))
            })?;
        let base = format!("{}/{}/requests/{}", self.queue_base(), endpoint, handle.provider_job_id);
        Ok(FalJob {
            request_id: handle.provider_job_id.clone(),
            status_url: self.trusted_queue_url(field("status_url")).unwrap_or_else(|| format!("{base}/status")),
            response_url: self.trusted_queue_url(field("response_url")).unwrap_or_else(|| base.clone()),
            cancel_url: self.trusted_queue_url(field("cancel_url")).unwrap_or_else(|| format!("{base}/cancel")),
        })
    }

    /// GET a queue URL and read the reply whole. Only a failure to reach fal at
    /// all is an `Err` here — and always a retryable one, since nothing about
    /// the job is known yet.
    async fn queue_get(&self, url: &str, api_key: &str, what: &str) -> Result<QueueReply, ProviderError> {
        let resp = crate::providers::inject_trace_headers(
            self.client.get(url).header("Authorization", format!("Key {api_key}")),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("fal 3d {what} request failed: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;
        let status = resp.status();
        let error_type = header(resp.headers(), "x-fal-error-type").map(str::to_string);
        let text = resp.text().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("fal 3d {what} body could not be read: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: true,
        })?;
        let json = serde_json::from_str(&text).ok();
        Ok(QueueReply { status, error_type, json, text })
    }

    /// Download one asset into memory, refusing anything over the size limit
    /// before buffering it, and anything that does not resolve to a public
    /// address before connecting.
    ///
    /// fal hands back URLs, not bytes, and litegen's contract is that a
    /// provider returns BYTES: several vendors expire download URLs within
    /// minutes of success, so deferring the fetch to rehost would be a race
    /// lost intermittently, and only in production. No API key is sent — the
    /// CDN URLs are public, and the key must never reach another host.
    async fn fetch_asset(&self, url: &str) -> Result<(Vec<u8>, String), AssetError> {
        if self.public_assets_only {
            crate::util::ssrf::validate_public_url(url)
                .await
                .map_err(|why| AssetError::Gone(format!("asset URL refused: {why}")))?;
        }
        let mut resp = self.asset_client.get(url).send().await.map_err(|e| {
            AssetError::Transient(ProviderError::RequestFailed {
                message: format!("fal 3d asset download failed: {e}"),
                status_code: None,
                provider_error: None,
                retryable: true,
            })
        })?;
        let status = resp.status();
        if !status.is_success() {
            let message = format!("fal 3d asset download returned HTTP {}", status.as_u16());
            return Err(if transient(status) {
                AssetError::Transient(ProviderError::RequestFailed {
                    message,
                    status_code: Some(status.as_u16()),
                    provider_error: None,
                    retryable: true,
                })
            } else {
                AssetError::Gone(message)
            });
        }
        let limit = self.max_asset_bytes;
        if let Some(len) = resp.content_length().filter(|&l| l > limit) {
            return Err(AssetError::Gone(format!("asset is {len} bytes, over the {limit}-byte limit")));
        }
        let content_type = header(resp.headers(), reqwest::header::CONTENT_TYPE.as_str())
            .and_then(|ct| ct.split(';').next())
            .map(|ct| ct.trim().to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        // Streamed against the limit, so a missing or dishonest Content-Length
        // cannot make us buffer an arbitrarily large body.
        let mut bytes = Vec::with_capacity(resp.content_length().unwrap_or(0) as usize);
        while let Some(chunk) = resp.chunk().await.map_err(|e| {
            AssetError::Transient(ProviderError::RequestFailed {
                message: format!("fal 3d asset body failed: {e}"),
                status_code: Some(status.as_u16()),
                provider_error: None,
                retryable: true,
            })
        })? {
            if (bytes.len() + chunk.len()) as u64 > limit {
                return Err(AssetError::Gone(format!("asset exceeds the {limit}-byte limit")));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((bytes, content_type))
    }
}

fn finished(status: GenerationStatus, error: Option<String>, files: Vec<Model3dFile>, meta: HashMap<String, Value>) -> Model3dGenerationPollResult {
    Model3dGenerationPollResult { status, progress: 100, files, error, metadata: meta }
}

#[async_trait]
impl Model3dProvider for FalModel3dProvider {
    fn name(&self) -> &str {
        "fal"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        // Guarded: `ApiKeyPool::shared` panics on an empty list, and the
        // registry builds every provider once with a default config just to
        // prove the name resolves.
        if !config.api_keys.is_empty() {
            self.key_pool = Some(ApiKeyPool::shared(config.api_keys.clone()));
        }
        self.config = Some(config);
    }

    /// Configured means a key is actually reachable, not merely that
    /// `configure` was called — the registry constructs this provider with an
    /// empty config to check the build arm, and that instance must not claim to
    /// be usable.
    fn is_configured(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| !c.api_key.is_empty() || self.key_pool.is_some())
    }

    /// Submit to the model's queue endpoint (`POST {queue}/{endpoint}`).
    ///
    /// @see <https://fal.ai/docs/documentation/model-apis/inference/queue>
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &Model3dExtras,
        materialized: &MaterializedRequest,
    ) -> Result<Model3dGenerationHandle, ProviderError> {
        let api_key = self.api_key()?;
        let spec = resolve_spec(&model.id).ok_or_else(|| ProviderError::ModelNotSupported {
            model: model.id.clone(),
            provider: "fal".into(),
        })?;

        // The catalog declares `provider_format: url`, so every ref arrives as
        // the caller's URL or an inline `data:` URI built from an upload — and
        // fal takes both as a file input. A catalog saying `base64` still works,
        // as a data URI; multipart has no form in a JSON body at all.
        // @see <https://fal.ai/docs/documentation/model-apis/fal-cdn>
        let images = materialized
            .refs
            .iter()
            .map(|r| match &r.form {
                MaterializedRefForm::Url(u) => Ok(u.clone()),
                MaterializedRefForm::Base64(b64) => {
                    Ok(crate::proxy::materializer::base64_data_uri(b64, None))
                }
                MaterializedRefForm::MultipartField { .. } => Err(ProviderError::InvalidRequest(format!(
                    "'{}' takes reference images as URLs; its catalog entry must not declare multipart",
                    model.id
                ))),
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Mode is inferred from the images, exactly as the trait documents. The
        // validator refuses both of these first; checking again here means a
        // caller that skipped it still cannot submit a request fal would only
        // reject — or, worse, accept and silently answer from the wrong arm.
        let endpoint = if images.is_empty() {
            let endpoint = spec.text_to_3d.ok_or_else(|| {
                ProviderError::InvalidRequest(format!(
                    "'{}' has no text-to-3D mode; send a reference image",
                    model.id
                ))
            })?;
            if base.prompt.trim().is_empty() {
                return Err(ProviderError::InvalidRequest(
                    "text-to-3D needs a prompt; send one, or a reference image".into(),
                ));
            }
            endpoint
        } else {
            spec.image_to_3d.ok_or_else(|| {
                ProviderError::InvalidRequest(format!("'{}' does not take reference images", model.id))
            })?
        };

        let mut body = serde_json::Map::new();
        if spec.prompt && !base.prompt.trim().is_empty() {
            body.insert("prompt".into(), json!(base.prompt));
        }
        if !images.is_empty() {
            let value = if spec.image_is_array { json!(images) } else { json!(images[0]) };
            body.insert(spec.image_field.into(), value);
        }
        // ONE container from the vendor; litegen derives the rest.
        if let (Some(field), Some(fmt)) = (spec.format_field, vendor_format(&spec, &extras.output_formats)) {
            body.insert(field.into(), json!(fmt));
        }
        if let Some(seed) = base.seed {
            match spec.seed_max {
                // Refused, not clamped: a clamped seed stops meaning "reproducible".
                Some(max) if !(0..=max).contains(&seed) => {
                    return Err(ProviderError::InvalidRequest(format!(
                        "'{}' accepts a seed in 0..={max}; got {seed}",
                        model.id
                    )));
                }
                Some(_) => {
                    body.insert("seed".into(), json!(seed));
                }
                None => {}
            }
        }
        if let (Some(field), Some(texture)) = (spec.texture_field, extras.texture) {
            body.insert(field.into(), json!(texture));
        }
        if let (Some(field), Some(pbr)) = (spec.material_field, extras.pbr) {
            // `pbr: false` is flat shading, not "no material": Shaded is the
            // endpoint's own word for it.
            body.insert(field.into(), json!(if pbr { "PBR" } else { "Shaded" }));
        }
        if let Some(Value::Object(extra)) = &extras.extra {
            let reserved = spec.mapped_fields();
            for (k, v) in extra {
                if reserved.contains(&k.as_str()) {
                    tracing::warn!(
                        model = %model.id, field = %k,
                        "fal 3d: ignoring an extra field that would override a canonical param"
                    );
                    continue;
                }
                body.insert(k.clone(), v.clone());
            }
        }

        let url = format!("{}/{}", self.queue_base(), endpoint);
        let resp = crate::providers::inject_trace_headers(
            self.client
                .post(&url)
                .header("Authorization", format!("Key {api_key}"))
                .json(&Value::Object(body)),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("fal 3d submit failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_timeout() || e.is_connect(),
        })?;
        let retry_hint = needs_retry(resp.headers());
        let error_type = header(resp.headers(), "x-fal-error-type").map(str::to_string);
        let status = resp.status();
        let text = resp.text().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("fal 3d submit response could not be read: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: true,
        })?;
        let reply = QueueReply { status, error_type, json: serde_json::from_str(&text).ok(), text };

        if !status.is_success() {
            // fal says whether resubmitting can help; without that, 5xx, 408 and
            // 429 are worth another try and anything else is the request's fault.
            let retryable = retry_hint.unwrap_or_else(|| transient(status));
            return Err(reply.into_error("submit", retryable));
        }

        let Some(json) = reply.json else {
            return Err(ProviderError::RequestFailed {
                message: "fal 3d submit answered 2xx with a body that is not JSON".into(),
                status_code: Some(status.as_u16()),
                provider_error: None,
                retryable: false,
            });
        };
        let request_id = json["request_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "fal 3d submit response has no request_id".into(),
                status_code: Some(status.as_u16()),
                provider_error: Some(json.clone()),
                retryable: false,
            })?
            .to_string();

        Ok(Model3dGenerationHandle {
            provider_job_id: request_id,
            provider: "fal".to_string(),
            model: model.id.clone(),
            // Persisted on the row as `stage_context`, so the poller can rebuild
            // the job after a restart. The endpoint is not derivable from the
            // model id once a model has separate text and image arms.
            stage_context: Some(json!({
                "endpoint": endpoint,
                "status_url": json["status_url"].as_str().unwrap_or(""),
                "response_url": json["response_url"].as_str().unwrap_or(""),
                "cancel_url": json["cancel_url"].as_str().unwrap_or(""),
            })),
        })
    }

    /// Check the queue status, and on `COMPLETED` fetch the stored result and
    /// download every file it names.
    ///
    /// A failure to learn anything is an `Err`, retryable unless asking again
    /// can never help. fal's own verdict that the run failed — a `COMPLETED`
    /// status carrying `error`, a stored result that is an error envelope, a
    /// result with no mesh, a mesh URL that is gone — is `Ok(Failed)`, so it
    /// takes the normal terminal path, webhook included.
    ///
    /// @see <https://fal.ai/docs/documentation/model-apis/inference/queue>
    async fn poll_status(
        &self,
        handle: &Model3dGenerationHandle,
    ) -> Result<Model3dGenerationPollResult, ProviderError> {
        let api_key = self.api_key()?;
        let job = self.job(handle)?;
        let meta: HashMap<String, Value> = HashMap::from([
            ("provider".to_string(), json!("fal")),
            ("request_id".to_string(), json!(job.request_id)),
        ]);

        let status = self.queue_get(&job.status_url, &api_key, "status").await?;
        if !status.status.is_success() {
            // 401/403 (the key) and 404 (no such request) never recover; the
            // rest may. Either way nothing is known about the job yet.
            let retryable = transient(status.status);
            return Err(status.into_error("status", retryable));
        }
        let Some(status) = status.json else {
            return Err(ProviderError::RequestFailed {
                message: "fal 3d status answered 2xx with a body that is not JSON".into(),
                status_code: None,
                provider_error: None,
                retryable: true,
            });
        };

        match status["status"].as_str() {
            Some("IN_QUEUE") => {
                return Ok(Model3dGenerationPollResult {
                    status: GenerationStatus::Pending,
                    progress: PROGRESS_QUEUED,
                    files: Vec::new(),
                    error: None,
                    metadata: meta,
                })
            }
            Some("IN_PROGRESS") => {
                return Ok(Model3dGenerationPollResult {
                    status: GenerationStatus::Processing,
                    progress: PROGRESS_RUNNING,
                    files: Vec::new(),
                    error: None,
                    metadata: meta,
                })
            }
            Some("COMPLETED") => {}
            other => {
                return Err(ProviderError::RequestFailed {
                    message: format!("fal 3d returned an unknown queue status {other:?}"),
                    status_code: None,
                    provider_error: Some(status),
                    retryable: false,
                })
            }
        }

        // There is no FAILED status: a failed run is COMPLETED with `error`, and
        // reading `status` alone turns a vendor failure into a completed
        // generation with no mesh. `error_type` is the machine-readable half
        // (content_policy_violation, image_load_error, request_timeout, …).
        if let Some(err) = status["error"].as_str().filter(|e| !e.is_empty()) {
            let detail = match status["error_type"].as_str().filter(|t| !t.is_empty()) {
                Some(t) => format!("{err} ({t})"),
                None => err.to_string(),
            };
            return Ok(finished(
                GenerationStatus::Failed,
                Some(format!("fal 3d generation failed: {detail}")),
                Vec::new(),
                meta,
            ));
        }

        let result = self.queue_get(&job.response_url, &api_key, "result").await?;
        if !result.status.is_success() {
            // The stored result of a COMPLETED request is final. A non-2xx
            // carrying fal's envelope is its verdict on the run — usually a 422
            // the runner raised, which also never reached the status `error`.
            if result.is_verdict() {
                return Ok(finished(
                    GenerationStatus::Failed,
                    Some(format!("fal 3d generation failed: {}", result.describe())),
                    Vec::new(),
                    meta,
                ));
            }
            return Err(result.into_error("result", true));
        }
        let Some(result) = result.json else {
            return Err(ProviderError::RequestFailed {
                message: "fal 3d result answered 2xx with a body that is not JSON".into(),
                status_code: None,
                provider_error: None,
                retryable: true,
            });
        };

        // Every endpoint here names it `model_mesh`; some fal 3D variants say
        // `model_glb`, so both are read rather than assumed.
        let Some((mesh_url, mesh_ct)) = result
            .get("model_mesh")
            .or_else(|| result.get("model_glb"))
            .and_then(file_url)
        else {
            return Ok(finished(
                GenerationStatus::Failed,
                Some("fal 3d completed without a mesh in its result".into()),
                Vec::new(),
                meta,
            ));
        };

        let (bytes, served_ct) = match self.fetch_asset(mesh_url).await {
            Ok(v) => v,
            Err(AssetError::Transient(e)) => return Err(e),
            Err(AssetError::Gone(why)) => {
                return Ok(finished(
                    GenerationStatus::Failed,
                    Some(format!("fal 3d mesh could not be downloaded: {why}")),
                    Vec::new(),
                    meta,
                ))
            }
        };
        let content_type = if mesh_ct.is_empty() { served_ct } else { mesh_ct.to_string() };
        let mut files = vec![Model3dFile {
            kind: Model3dAssetKind::Mesh,
            format: format_of(mesh_url, &content_type),
            content_type,
            bytes,
            // No fal 3D endpoint reports a triangle count, and the request's
            // target_polycount is the ask, not the result.
            polycount: None,
            width: None,
            height: None,
        }];

        // Rodin returns its PBR maps separately, as fal `Image`s — the only
        // producer of `Model3dAssetKind::Texture` so far. One bad texture must
        // not fail a generation whose mesh is fine: the mesh is the deliverable.
        if let Some(textures) = result.get("textures").and_then(Value::as_array) {
            for t in textures {
                let Some((url, declared_ct)) = file_url(t) else { continue };
                match self.fetch_asset(url).await {
                    Ok((bytes, served_ct)) => {
                        let content_type =
                            if declared_ct.is_empty() { served_ct } else { declared_ct.to_string() };
                        let dimension = |k: &str| t.get(k).and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok());
                        files.push(Model3dFile {
                            kind: Model3dAssetKind::Texture,
                            format: format_of(url, &content_type),
                            content_type,
                            bytes,
                            polycount: None,
                            width: dimension("width"),
                            height: dimension("height"),
                        });
                    }
                    Err(AssetError::Transient(e)) => {
                        tracing::warn!(url = %url, error = %e, "fal 3d: skipping a texture that would not download")
                    }
                    Err(AssetError::Gone(why)) => {
                        tracing::warn!(url = %url, reason = %why, "fal 3d: skipping a texture that would not download")
                    }
                }
            }
        }

        Ok(finished(GenerationStatus::Completed, None, files, meta))
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &Model3dGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        // fal's flat per-generation price. Its conditional multipliers
        // (Hunyuan's textured_mesh, Rodin's HighPack addon) are recorded in the
        // catalog's `variable_pricing` and passed along in the breakdown, not
        // folded into the quote: the charge at submit is the base price too, and
        // a quote must match what is charged.
        let mut breakdown = json!({ "model": model.id });
        if let Some(variable) = &model.pricing.variable_pricing {
            breakdown["variable_pricing"] = variable.clone();
        }
        Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, Some(breakdown)))
    }

    /// Check the key against fal's Platform API — the price of one public
    /// endpoint. Free, read-only, and authenticated: 401 means the key is bad.
    ///
    /// @see <https://fal.ai/docs/platform-apis/v1/models/pricing>
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "fal 3d provider is not configured".into(),
                latency_ms: None,
            };
        }
        let Ok(api_key) = self.api_key() else {
            return HealthCheckResult { healthy: false, message: "No API key".into(), latency_ms: None };
        };
        let url = format!("{}/models/pricing?endpoint_id={HEALTH_CHECK_ENDPOINT}", self.platform_base());
        let start = std::time::Instant::now();
        let reply = self
            .client
            .get(&url)
            .header("Authorization", format!("Key {api_key}"))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await;
        let latency_ms = Some(start.elapsed().as_millis() as u64);
        match reply {
            Ok(r) if r.status().is_success() => {
                HealthCheckResult { healthy: true, message: "fal API key valid".into(), latency_ms }
            }
            Ok(r) if matches!(r.status(), StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) => HealthCheckResult {
                healthy: false,
                message: format!("fal rejected the API key (HTTP {})", r.status().as_u16()),
                latency_ms,
            },
            Ok(r) => HealthCheckResult {
                healthy: false,
                message: format!("fal returned HTTP {}", r.status().as_u16()),
                latency_ms,
            },
            Err(e) => HealthCheckResult { healthy: false, message: format!("fal health check failed: {e}"), latency_ms },
        }
    }

    /// `PUT …/requests/{id}/cancel`. A queued request is removed and never runs
    /// (or bills); a running one is signalled and may still finish.
    ///
    /// @see <https://fal.ai/docs/documentation/model-apis/inference/queue> (Cancel a Request)
    async fn cancel(&self, handle: &Model3dGenerationHandle) -> Result<CancelOutcome, ProviderError> {
        let api_key = self.api_key()?;
        let job = self.job(handle)?;
        let resp = crate::providers::inject_trace_headers(
            self.client.put(&job.cancel_url).header("Authorization", format!("Key {api_key}")),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("fal 3d cancel failed: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        match (status.as_u16(), body["status"].as_str()) {
            (200..=299, _) => Ok(CancelOutcome::Requested),
            (400, Some("ALREADY_COMPLETED")) => Ok(CancelOutcome::AlreadyFinished),
            (404, _) => Ok(CancelOutcome::NotFound),
            _ => Err(ProviderError::RequestFailed {
                message: format!("fal 3d cancel returned HTTP {}", status.as_u16()),
                status_code: Some(status.as_u16()),
                provider_error: Some(body),
                retryable: transient(status),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{CapabilityRegistry, ParamSpec, RefProviderFormat};
    use crate::proxy::materializer::{Cleanup, MaterializedRef};
    use wiremock::matchers::{header as header_is, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ─── Fixtures ───────────────────────────────────────────────────────────

    fn catalog() -> CapabilityRegistry {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        CapabilityRegistry::from_dir(&p).expect("the shipped catalog loads")
    }

    fn schema(id: &str) -> ModelSchema {
        catalog().get(id).unwrap_or_else(|| panic!("{id} missing from the catalog")).clone()
    }

    /// A provider pointed at `server` for both the queue and the Platform API.
    /// Assets are served from the same loopback mock, so the public-address
    /// check is off — `asset_urls_must_resolve_to_a_public_address` covers it.
    fn provider(server: &MockServer) -> FalModel3dProvider {
        let mut p = FalModel3dProvider::new();
        p.configure(ProviderInstanceConfig {
            api_key: "test-key".into(),
            api_base: Some(server.uri()),
            options: Some(json!({ "platform_api_base": server.uri() })),
            ..Default::default()
        });
        p.public_assets_only = false;
        p
    }

    fn base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.into(),
            model: model.into(),
            n: 1,
            negative_prompt: None,
            seed: None,
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        }
    }

    fn extras(formats: &[&str]) -> Model3dExtras {
        Model3dExtras {
            output_formats: formats.iter().map(|f| f.to_string()).collect(),
            texture: None,
            pbr: None,
            target_polycount: None,
            symmetry: None,
            topology: None,
            rig: None,
            extra: None,
        }
    }

    fn images(urls: &[&str]) -> MaterializedRequest {
        MaterializedRequest {
            refs: urls
                .iter()
                .map(|u| MaterializedRef { role: "init".into(), form: MaterializedRefForm::Url(u.to_string()) })
                .collect(),
            cleanup: Cleanup::empty(),
        }
    }

    fn no_images() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    /// Accept a submit to `endpoint`, answering with queue URLs on the mock.
    async fn mount_submit(server: &MockServer, endpoint: &str, id: &str) {
        let base = format!("{}/{endpoint}/requests/{id}", server.uri());
        Mock::given(method("POST"))
            .and(path(format!("/{endpoint}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "request_id": id,
                "status_url": format!("{base}/status"),
                "response_url": base,
                "cancel_url": format!("{base}/cancel"),
            })))
            .mount(server)
            .await;
    }

    async fn submitted_body(server: &MockServer) -> Value {
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1, "exactly one submit");
        serde_json::from_slice(&received[0].body).unwrap()
    }

    fn handle(model: &str, endpoint: &str, id: &str, server: &MockServer) -> Model3dGenerationHandle {
        let base = format!("{}/{endpoint}/requests/{id}", server.uri());
        Model3dGenerationHandle {
            provider_job_id: id.into(),
            provider: "fal".into(),
            model: model.into(),
            stage_context: Some(json!({
                "endpoint": endpoint,
                "status_url": format!("{base}/status"),
                "response_url": base,
                "cancel_url": format!("{base}/cancel"),
            })),
        }
    }

    async fn mount_get(server: &MockServer, at: &str, resp: ResponseTemplate) {
        Mock::given(method("GET")).and(path(at.to_string())).respond_with(resp).mount(server).await;
    }

    const PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    // ─── Submit: each endpoint's own field names ───────────────────────────
    // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/hyper3d/rodin>

    #[tokio::test]
    async fn rodin_text_to_3d_sends_rodins_own_field_names() {
        let server = MockServer::start().await;
        mount_submit(&server, "fal-ai/hyper3d/rodin", "r-1").await;
        let mut b = base("a brass astrolabe", "fal/hyper3d-rodin");
        b.seed = Some(7);
        let mut x = extras(&["glb", "obj"]);
        x.pbr = Some(true);

        let h = provider(&server)
            .generate(&schema("fal/hyper3d-rodin"), &b, &x, &no_images())
            .await
            .expect("submit");
        assert_eq!(h.provider_job_id, "r-1");
        assert_eq!(h.stage_context.as_ref().unwrap()["endpoint"], "fal-ai/hyper3d/rodin");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].headers.get("authorization").unwrap(), "Key test-key", "Key, not Bearer");
        let body = submitted_body(&server).await;
        assert_eq!(body["prompt"], "a brass astrolabe");
        assert_eq!(body["geometry_file_format"], "glb", "one source; obj is derived");
        assert_eq!(body["material"], "PBR");
        assert_eq!(body["seed"], 7);
        assert!(body.get("input_image_urls").is_none(), "{body}");
    }

    #[tokio::test]
    async fn rodin_image_to_3d_sends_every_image_as_an_array_and_no_empty_prompt() {
        let server = MockServer::start().await;
        mount_submit(&server, "fal-ai/hyper3d/rodin", "r-2").await;
        provider(&server)
            .generate(
                &schema("fal/hyper3d-rodin"),
                &base("", "fal/hyper3d-rodin"),
                &extras(&["glb"]),
                &images(&["https://example.com/front.png", "https://example.com/back.png"]),
            )
            .await
            .expect("submit");
        let body = submitted_body(&server).await;
        assert_eq!(body["input_image_urls"], json!(["https://example.com/front.png", "https://example.com/back.png"]));
        assert!(body.get("prompt").is_none(), "an empty prompt is not sent: {body}");
    }

    #[tokio::test]
    async fn rodin_is_asked_for_its_own_fbx_rather_than_a_conversion() {
        // fbx is `prefers_vendor_native`: while litegen cannot write fbx at all,
        // asking Rodin for glb would deliver the wrong file and fail the
        // generation after it was billed; once it can, Rodin's export still
        // carries rigs and materials a conversion from glb would not.
        let server = MockServer::start().await;
        mount_submit(&server, "fal-ai/hyper3d/rodin", "r-3").await;
        let mut x = extras(&["fbx"]);
        x.pbr = Some(false);
        provider(&server)
            .generate(&schema("fal/hyper3d-rodin"), &base("a chair", "fal/hyper3d-rodin"), &x, &no_images())
            .await
            .expect("submit");
        let body = submitted_body(&server).await;
        assert_eq!(body["geometry_file_format"], "fbx");
        assert_eq!(body["material"], "Shaded", "pbr: false is flat shading, not no material");
    }

    // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/trellis>
    #[tokio::test]
    async fn trellis_takes_image_url_and_never_a_prompt_or_a_format() {
        let server = MockServer::start().await;
        mount_submit(&server, "fal-ai/trellis", "t-1").await;
        let mut b = base("a prompt TrellisInput has no field for", "fal/trellis");
        b.seed = Some(42);
        let mut x = extras(&["glb", "obj", "stl", "ply"]);
        x.extra = Some(json!({ "mesh_simplify": 0.95, "texture_size": 2048 }));
        provider(&server)
            .generate(&schema("fal/trellis"), &b, &x, &images(&["https://example.com/robot.png"]))
            .await
            .expect("submit");
        let body = submitted_body(&server).await;
        assert_eq!(body["image_url"], "https://example.com/robot.png", "a scalar, not an array");
        assert!(body.get("prompt").is_none(), "TrellisInput defines no prompt: {body}");
        assert!(body.get("geometry_file_format").is_none(), "TRELLIS emits glb with no choice: {body}");
        assert_eq!(body["seed"], 42);
        assert_eq!(body["mesh_simplify"], 0.95);
        assert_eq!(body["texture_size"], 2048);
    }

    // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/hunyuan3d/v2>
    #[tokio::test]
    async fn hunyuan_takes_input_image_url_and_maps_texture_to_textured_mesh() {
        let server = MockServer::start().await;
        mount_submit(&server, "fal-ai/hunyuan3d/v2", "h-1").await;
        let mut x = extras(&["glb"]);
        x.texture = Some(true);
        x.extra = Some(json!({ "octree_resolution": 256 }));
        provider(&server)
            .generate(&schema("fal/hunyuan3d-v2"), &base("", "fal/hunyuan3d-v2"), &x, &images(&["https://example.com/robot.png"]))
            .await
            .expect("submit");
        let body = submitted_body(&server).await;
        assert_eq!(body["input_image_url"], "https://example.com/robot.png");
        assert_eq!(body["textured_mesh"], true);
        assert_eq!(body["octree_resolution"], 256);
        assert!(body.get("prompt").is_none(), "Hunyuan3dV2Input defines no prompt: {body}");
    }

    #[tokio::test]
    async fn an_extra_field_never_overrides_a_canonical_one() {
        // In lax mode every extra key reaches the adapter; one that rewrote the
        // container would break the output_formats promise after billing.
        let server = MockServer::start().await;
        mount_submit(&server, "fal-ai/hyper3d/rodin", "r-4").await;
        let mut x = extras(&["glb"]);
        x.extra = Some(json!({
            "geometry_file_format": "fbx",
            "seed": 99_999_999,
            "input_image_urls": ["http://169.254.169.254/latest"],
            "quality": "high",
        }));
        provider(&server)
            .generate(&schema("fal/hyper3d-rodin"), &base("a lamp", "fal/hyper3d-rodin"), &x, &no_images())
            .await
            .expect("submit");
        let body = submitted_body(&server).await;
        assert_eq!(body["geometry_file_format"], "glb");
        assert!(body.get("seed").is_none(), "an unvalidated seed must not slip in: {body}");
        assert!(body.get("input_image_urls").is_none(), "nor an image the materializer never saw: {body}");
        assert_eq!(body["quality"], "high", "a field no canonical param maps to still passes");
    }

    #[tokio::test]
    async fn an_uploaded_image_in_base64_form_is_sent_as_a_data_uri() {
        let server = MockServer::start().await;
        mount_submit(&server, "fal-ai/trellis", "t-2").await;
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, PNG);
        let refs = MaterializedRequest {
            refs: vec![MaterializedRef { role: "init".into(), form: MaterializedRefForm::Base64(b64.clone()) }],
            cleanup: Cleanup::empty(),
        };
        provider(&server)
            .generate(&schema("fal/trellis"), &base("", "fal/trellis"), &extras(&["glb"]), &refs)
            .await
            .expect("submit");
        assert_eq!(submitted_body(&server).await["image_url"], format!("data:image/png;base64,{b64}"));
    }

    #[tokio::test]
    async fn requests_fal_would_only_reject_are_refused_before_submitting() {
        let server = MockServer::start().await;
        let p = provider(&server);

        // No image for an image-only model.
        let err = p
            .generate(&schema("fal/trellis"), &base("", "fal/trellis"), &extras(&["glb"]), &no_images())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(ref m) if m.contains("no text-to-3D mode")), "{err}");

        // Text-to-3D with nothing to generate from.
        let err = p
            .generate(&schema("fal/hyper3d-rodin"), &base("  ", "fal/hyper3d-rodin"), &extras(&["glb"]), &no_images())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(ref m) if m.contains("needs a prompt")), "{err}");

        // Rodin's u16 seed ceiling: refused, never clamped.
        let mut b = base("a key", "fal/hyper3d-rodin");
        b.seed = Some(65_536);
        let err = p
            .generate(&schema("fal/hyper3d-rodin"), &b, &extras(&["glb"]), &no_images())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(ref m) if m.contains("0..=65535")), "{err}");

        // A multipart ref cannot be expressed in a JSON body.
        let refs = MaterializedRequest {
            refs: vec![MaterializedRef {
                role: "init".into(),
                form: MaterializedRefForm::MultipartField {
                    field_name: "image".into(),
                    bytes: bytes::Bytes::from_static(&PNG),
                    content_type: "image/png".into(),
                },
            }],
            cleanup: Cleanup::empty(),
        };
        let err = p
            .generate(&schema("fal/trellis"), &base("", "fal/trellis"), &extras(&["glb"]), &refs)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err}");

        // Not a fal 3D model at all.
        let mut not_3d = schema("fal/trellis");
        not_3d.id = "fal/flux-pro".into();
        let err = p.generate(&not_3d, &base("x", "fal/flux-pro"), &extras(&["glb"]), &no_images()).await.unwrap_err();
        assert!(matches!(err, ProviderError::ModelNotSupported { .. }), "{err}");

        assert!(server.received_requests().await.unwrap().is_empty(), "nothing may reach fal");
    }

    #[tokio::test]
    async fn a_submit_error_carries_fals_message_and_its_retry_verdict() {
        // A 422 names the field; a 429 is the concurrency limiter; a gateway's
        // HTML page is still retryable; X-Fal-Needs-Retry overrides the status.
        let cases: Vec<(ResponseTemplate, bool, &str)> = vec![
            (
                ResponseTemplate::new(422).set_body_json(json!({ "detail": [
                    { "loc": ["body", "image_url"], "msg": "Field required", "type": "missing" }
                ]})),
                false,
                "body.image_url: Field required",
            ),
            (ResponseTemplate::new(429).set_body_json(json!({ "detail": "Too many requests" })), true, "Too many requests"),
            (ResponseTemplate::new(502).set_body_string("<html>Bad Gateway</html>"), true, "Bad Gateway"),
            (
                ResponseTemplate::new(400).insert_header("X-Fal-Needs-Retry", "1").set_body_json(json!({ "detail": "busy" })),
                true,
                "busy",
            ),
            (
                ResponseTemplate::new(500)
                    .insert_header("X-Fal-Needs-Retry", "0")
                    .insert_header("X-Fal-Error-Type", "runner_server_error")
                    .set_body_json(json!({ "detail": "Runner crashed" })),
                false,
                "Runner crashed (runner_server_error)",
            ),
        ];
        for (resp, retryable_expected, needle) in cases {
            let server = MockServer::start().await;
            Mock::given(method("POST")).and(path("/fal-ai/trellis")).respond_with(resp).mount(&server).await;
            let err = provider(&server)
                .generate(&schema("fal/trellis"), &base("", "fal/trellis"), &extras(&["glb"]), &images(&["https://example.com/a.png"]))
                .await
                .unwrap_err();
            assert_eq!(err.is_retryable(), retryable_expected, "{err}");
            assert!(err.to_string().contains(needle), "{needle:?} missing from: {err}");
        }
    }

    #[tokio::test]
    async fn a_submit_without_a_request_id_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/fal-ai/trellis"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "IN_QUEUE" })))
            .mount(&server)
            .await;
        let err = provider(&server)
            .generate(&schema("fal/trellis"), &base("", "fal/trellis"), &extras(&["glb"]), &images(&["https://example.com/a.png"]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("request_id"), "{err}");
    }

    // ─── Poll ───────────────────────────────────────────────────────────────

    const RODIN: &str = "fal-ai/hyper3d/rodin";

    fn status_path(endpoint: &str, id: &str) -> String {
        format!("/{endpoint}/requests/{id}/status")
    }

    fn result_path(endpoint: &str, id: &str) -> String {
        format!("/{endpoint}/requests/{id}")
    }

    async fn poll(server: &MockServer, model: &str, endpoint: &str, id: &str) -> Result<Model3dGenerationPollResult, ProviderError> {
        provider(server).poll_status(&handle(model, endpoint, id, server)).await
    }

    #[tokio::test]
    async fn queue_states_map_to_pending_and_processing() {
        let server = MockServer::start().await;
        mount_get(&server, &status_path(RODIN, "q"), ResponseTemplate::new(202).set_body_json(json!({ "status": "IN_QUEUE", "queue_position": 3 }))).await;
        mount_get(&server, &status_path(RODIN, "p"), ResponseTemplate::new(202).set_body_json(json!({ "status": "IN_PROGRESS" }))).await;

        let q = poll(&server, "fal/hyper3d-rodin", RODIN, "q").await.unwrap();
        assert_eq!((q.status, q.progress), (GenerationStatus::Pending, PROGRESS_QUEUED));
        let p = poll(&server, "fal/hyper3d-rodin", RODIN, "p").await.unwrap();
        assert_eq!((p.status, p.progress), (GenerationStatus::Processing, PROGRESS_RUNNING));
        assert!(p.files.is_empty());

        let received = server.received_requests().await.unwrap();
        assert!(received.iter().all(|r| r.headers.get("authorization").unwrap() == "Key test-key"));
    }

    #[tokio::test]
    async fn completed_downloads_the_mesh_and_rodins_textures_with_their_dimensions() {
        let server = MockServer::start().await;
        let files = server.uri() + "/files";
        mount_get(&server, &status_path(RODIN, "ok"), ResponseTemplate::new(200).set_body_json(json!({ "status": "COMPLETED" }))).await;
        mount_get(
            &server,
            &result_path(RODIN, "ok"),
            ResponseTemplate::new(200).set_body_json(json!({
                "seed": 7,
                // Object storage labels everything octet-stream; the filename
                // carries the real container.
                "model_mesh": { "url": format!("{files}/base_basic_pbr.glb"), "content_type": "application/octet-stream", "file_size": 12 },
                "textures": [
                    { "url": format!("{files}/texture_diffuse.png"), "content_type": "image/png", "width": 1024, "height": 512 },
                ],
            })),
        )
        .await;
        mount_get(&server, "/files/base_basic_pbr.glb", ResponseTemplate::new(200).set_body_bytes(b"glTF\x02\x00\x00\x00mesh".to_vec())).await;
        mount_get(&server, "/files/texture_diffuse.png", ResponseTemplate::new(200).insert_header("content-type", "image/png").set_body_bytes(PNG.to_vec())).await;

        let done = poll(&server, "fal/hyper3d-rodin", RODIN, "ok").await.unwrap();
        assert_eq!(done.status, GenerationStatus::Completed);
        let mesh = done.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh).expect("a mesh");
        assert_eq!(mesh.format, "glb");
        assert_eq!(&mesh.bytes[0..4], b"glTF");
        assert_eq!(mesh.polycount, None, "fal reports no triangle count");
        let tex = done.files.iter().find(|f| f.kind == Model3dAssetKind::Texture).expect("a texture");
        assert_eq!((tex.format.as_str(), tex.width, tex.height), ("png", Some(1024), Some(512)));
        assert_eq!(tex.bytes, PNG);

        // The asset host gets no key: CDN URLs are public, and the key must
        // never reach a host other than the queue.
        let received = server.received_requests().await.unwrap();
        for r in received.iter().filter(|r| r.url.path().starts_with("/files/")) {
            assert!(r.headers.get("authorization").is_none(), "key sent to the asset host: {}", r.url);
        }
    }

    #[tokio::test]
    async fn completed_with_an_error_is_failed_with_fals_message_and_type() {
        let server = MockServer::start().await;
        mount_get(
            &server,
            &status_path(RODIN, "bad"),
            ResponseTemplate::new(200).set_body_json(json!({
                "status": "COMPLETED", "error": "Image could not be downloaded", "error_type": "image_load_error",
            })),
        )
        .await;
        let failed = poll(&server, "fal/hyper3d-rodin", RODIN, "bad").await.unwrap();
        assert_eq!(failed.status, GenerationStatus::Failed);
        let err = failed.error.unwrap();
        assert!(err.contains("Image could not be downloaded") && err.contains("(image_load_error)"), "{err}");
        assert!(failed.files.is_empty());
    }

    #[tokio::test]
    async fn a_transient_status_failure_stays_retryable() {
        // The poller fails a row on any non-retryable error, and these are
        // billed generations: a blip must cost a poll tick, never the job.
        for (resp, label) in [
            (ResponseTemplate::new(503), "503"),
            (ResponseTemplate::new(429).set_body_json(json!({ "detail": "slow down" })), "429"),
            (ResponseTemplate::new(200).set_body_string("<html>not json</html>"), "unparseable 200"),
        ] {
            let server = MockServer::start().await;
            mount_get(&server, &status_path(RODIN, "s"), resp).await;
            let err = poll(&server, "fal/hyper3d-rodin", RODIN, "s").await.unwrap_err();
            assert!(err.is_retryable(), "{label}: {err}");
        }
    }

    #[tokio::test]
    async fn a_definitive_status_failure_is_not_retried_forever() {
        for status in [401, 403, 404] {
            let server = MockServer::start().await;
            mount_get(&server, &status_path(RODIN, "s"), ResponseTemplate::new(status).set_body_json(json!({ "detail": "nope" }))).await;
            let err = poll(&server, "fal/hyper3d-rodin", RODIN, "s").await.unwrap_err();
            assert!(!err.is_retryable(), "{status}: {err}");
        }
    }

    #[tokio::test]
    async fn a_stored_error_result_is_fals_verdict_but_a_gateway_blip_is_not() {
        let cases: Vec<(ResponseTemplate, Option<&str>)> = vec![
            // A runtime 422 — the run's own result.
            (
                ResponseTemplate::new(422).set_body_json(json!({ "detail": [
                    { "loc": ["body", "input_image_urls", 0], "msg": "Image too small", "type": "image_too_small" }
                ]})),
                Some("body.input_image_urls.0: Image too small"),
            ),
            // A stored request error: string detail plus error_type.
            (
                ResponseTemplate::new(504).set_body_json(json!({ "detail": "Request timed out", "error_type": "request_timeout" })),
                Some("Request timed out (request_timeout)"),
            ),
            // The type only in the header still makes it fal's verdict.
            (
                ResponseTemplate::new(503).insert_header("X-Fal-Error-Type", "runner_scheduling_failure").set_body_json(json!({ "detail": "No runner" })),
                Some("No runner (runner_scheduling_failure)"),
            ),
            // No envelope at all: the queue was unreachable, nothing is decided.
            (ResponseTemplate::new(502).set_body_string("<html>Bad Gateway</html>"), None),
        ];
        for (resp, verdict) in cases {
            let server = MockServer::start().await;
            mount_get(&server, &status_path(RODIN, "r"), ResponseTemplate::new(200).set_body_json(json!({ "status": "COMPLETED" }))).await;
            mount_get(&server, &result_path(RODIN, "r"), resp).await;
            let outcome = poll(&server, "fal/hyper3d-rodin", RODIN, "r").await;
            match verdict {
                Some(needle) => {
                    let failed = outcome.expect("a verdict is Ok(Failed)");
                    assert_eq!(failed.status, GenerationStatus::Failed);
                    let err = failed.error.unwrap();
                    assert!(err.contains(needle), "{needle:?} missing from {err:?}");
                }
                None => assert!(outcome.unwrap_err().is_retryable()),
            }
        }
    }

    #[tokio::test]
    async fn completed_without_a_mesh_is_failed() {
        let server = MockServer::start().await;
        mount_get(&server, &status_path(RODIN, "m"), ResponseTemplate::new(200).set_body_json(json!({ "status": "COMPLETED" }))).await;
        mount_get(&server, &result_path(RODIN, "m"), ResponseTemplate::new(200).set_body_json(json!({ "seed": 1, "textures": [] }))).await;
        let failed = poll(&server, "fal/hyper3d-rodin", RODIN, "m").await.unwrap();
        assert_eq!(failed.status, GenerationStatus::Failed);
        assert!(failed.error.unwrap().contains("without a mesh"));
    }

    async fn mount_completed_with_mesh(server: &MockServer, endpoint: &str, id: &str, mesh: ResponseTemplate) {
        mount_get(server, &status_path(endpoint, id), ResponseTemplate::new(200).set_body_json(json!({ "status": "COMPLETED" }))).await;
        mount_get(
            server,
            &result_path(endpoint, id),
            ResponseTemplate::new(200).set_body_json(json!({ "model_mesh": { "url": format!("{}/files/m.glb", server.uri()) } })),
        )
        .await;
        mount_get(server, "/files/m.glb", mesh).await;
    }

    #[tokio::test]
    async fn a_mesh_that_will_not_download_fails_or_retries_by_cause() {
        // Gone is fal's final answer for this run; a 503 may clear next tick.
        let server = MockServer::start().await;
        mount_completed_with_mesh(&server, "fal-ai/trellis", "g", ResponseTemplate::new(404)).await;
        let failed = poll(&server, "fal/trellis", "fal-ai/trellis", "g").await.unwrap();
        assert_eq!(failed.status, GenerationStatus::Failed);
        assert!(failed.error.unwrap().contains("could not be downloaded"));

        let server = MockServer::start().await;
        mount_completed_with_mesh(&server, "fal-ai/trellis", "t", ResponseTemplate::new(503)).await;
        assert!(poll(&server, "fal/trellis", "fal-ai/trellis", "t").await.unwrap_err().is_retryable());
    }

    #[tokio::test]
    async fn an_oversized_mesh_is_refused_without_being_buffered() {
        let server = MockServer::start().await;
        mount_completed_with_mesh(&server, "fal-ai/trellis", "big", ResponseTemplate::new(200).set_body_bytes(vec![0u8; 4096])).await;
        let mut p = provider(&server);
        p.max_asset_bytes = 1024;
        let failed = p.poll_status(&handle("fal/trellis", "fal-ai/trellis", "big", &server)).await.unwrap();
        assert_eq!(failed.status, GenerationStatus::Failed);
        assert!(failed.error.unwrap().contains("limit"));
    }

    #[tokio::test]
    async fn a_texture_that_will_not_download_is_skipped_not_fatal() {
        let server = MockServer::start().await;
        let files = server.uri() + "/files";
        mount_get(&server, &status_path(RODIN, "tx"), ResponseTemplate::new(200).set_body_json(json!({ "status": "COMPLETED" }))).await;
        mount_get(
            &server,
            &result_path(RODIN, "tx"),
            ResponseTemplate::new(200).set_body_json(json!({
                "model_mesh": { "url": format!("{files}/m.glb") },
                "textures": [{ "url": format!("{files}/gone.png") }],
            })),
        )
        .await;
        mount_get(&server, "/files/m.glb", ResponseTemplate::new(200).set_body_bytes(b"glTF".to_vec())).await;
        mount_get(&server, "/files/gone.png", ResponseTemplate::new(404)).await;
        let done = poll(&server, "fal/hyper3d-rodin", RODIN, "tx").await.unwrap();
        assert_eq!(done.status, GenerationStatus::Completed);
        assert_eq!(done.files.len(), 1, "the mesh is the deliverable; the map is a bonus");
    }

    #[tokio::test]
    async fn asset_urls_must_resolve_to_a_public_address() {
        // The default provider refuses a result pointing inside the deployment
        // (here: the loopback mock itself) instead of fetching and serving it.
        let server = MockServer::start().await;
        mount_completed_with_mesh(&server, "fal-ai/trellis", "ssrf", ResponseTemplate::new(200).set_body_bytes(b"secret".to_vec())).await;
        let mut p = provider(&server);
        p.public_assets_only = true;
        let failed = p.poll_status(&handle("fal/trellis", "fal-ai/trellis", "ssrf", &server)).await.unwrap();
        assert_eq!(failed.status, GenerationStatus::Failed);
        assert!(failed.error.unwrap().contains("refused"));
        let fetched = server.received_requests().await.unwrap().iter().any(|r| r.url.path() == "/files/m.glb");
        assert!(!fetched, "the private URL must never be requested");
    }

    #[tokio::test]
    async fn the_key_only_goes_to_the_configured_queue_origin() {
        // A status_url on any other host is rebuilt from the configured base;
        // the key travels with every queue call.
        let server = MockServer::start().await;
        mount_get(&server, &status_path("fal-ai/trellis", "o"), ResponseTemplate::new(200).set_body_json(json!({ "status": "IN_QUEUE" }))).await;
        let mut h = handle("fal/trellis", "fal-ai/trellis", "o", &server);
        h.stage_context = Some(json!({
            "endpoint": "fal-ai/trellis",
            "status_url": "https://elsewhere.example/fal-ai/trellis/requests/o/status",
        }));
        let polled = provider(&server).poll_status(&h).await.expect("polled on the configured origin");
        assert_eq!(polled.status, GenerationStatus::Pending);
    }

    #[tokio::test]
    async fn a_job_whose_stage_context_was_lost_is_rebuilt_from_the_model() {
        // The row's metadata write can fail; the poller must still find the job.
        let server = MockServer::start().await;
        mount_get(&server, &status_path("fal-ai/hunyuan3d/v2", "lost"), ResponseTemplate::new(200).set_body_json(json!({ "status": "IN_PROGRESS" }))).await;
        let h = Model3dGenerationHandle {
            provider_job_id: "lost".into(),
            provider: "fal".into(),
            model: "fal/hunyuan3d-v2".into(),
            stage_context: None,
        };
        let polled = provider(&server).poll_status(&h).await.expect("rebuilt");
        assert_eq!(polled.status, GenerationStatus::Processing);
    }

    // ─── Cancel and health ──────────────────────────────────────────────────

    #[tokio::test]
    async fn cancel_maps_fals_three_answers() {
        let cases = [
            (ResponseTemplate::new(202).set_body_json(json!({ "status": "CANCELLATION_REQUESTED" })), CancelOutcome::Requested),
            (ResponseTemplate::new(400).set_body_json(json!({ "status": "ALREADY_COMPLETED" })), CancelOutcome::AlreadyFinished),
            (ResponseTemplate::new(404).set_body_json(json!({ "status": "NOT_FOUND" })), CancelOutcome::NotFound),
        ];
        for (resp, expected) in cases {
            let server = MockServer::start().await;
            Mock::given(method("PUT"))
                .and(path("/fal-ai/trellis/requests/c/cancel"))
                .and(header_is("authorization", "Key test-key"))
                .respond_with(resp)
                .mount(&server)
                .await;
            let outcome = provider(&server).cancel(&handle("fal/trellis", "fal-ai/trellis", "c", &server)).await;
            assert_eq!(outcome.unwrap(), expected);
        }
    }

    #[tokio::test]
    async fn health_check_validates_the_key_against_the_platform_api() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models/pricing"))
            .and(header_is("authorization", "Key test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "prices": [] })))
            .mount(&server)
            .await;
        let h = provider(&server).health_check().await;
        assert!(h.healthy, "{}", h.message);

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models/pricing"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({ "error": { "type": "authorization_error" } })))
            .mount(&server)
            .await;
        let h = provider(&server).health_check().await;
        assert!(!h.healthy && h.message.contains("rejected the API key"), "{}", h.message);

        let unconfigured = FalModel3dProvider::new().health_check().await;
        assert!(!unconfigured.healthy);
    }

    #[tokio::test]
    async fn the_estimate_is_the_base_price_with_the_multipliers_alongside() {
        let p = FalModel3dProvider::new();
        let hunyuan = schema("fal/hunyuan3d-v2");
        let req = crate::types::Model3dGenerationRequest {
            base: base("", "fal/hunyuan3d-v2"),
            output_formats: None,
            output_format: None,
            texture: Some(true),
            pbr: None,
            target_polycount: None,
            symmetry: None,
            topology: None,
            rig: None,
        };
        let est = p.estimate_cost(&hunyuan, &req).await.unwrap();
        assert_eq!(est.base_cost_usd, 0.16, "what submit charges");
        assert_eq!(est.breakdown.unwrap()["variable_pricing"]["textured_mesh_multiplier"], 3);
    }

    // ─── The catalog and this adapter must agree ───────────────────────────
    //
    // catalog_conformance cannot see `resolve_spec`, so this is the only thing
    // tying each row of models/fal.yaml to the arm that serves it.

    #[test]
    fn every_fal_3d_row_is_served_here_and_every_arm_is_in_the_catalog() {
        let cat = catalog();
        let rows: Vec<String> = cat
            .all()
            .filter(|m| m.provider == "fal" && m.media_type == crate::capabilities::MediaType::Model3d)
            .map(|m| m.id.clone())
            .collect();
        let mut sorted_models: Vec<String> = MODELS.iter().map(|s| s.to_string()).collect();
        let mut sorted_rows = rows.clone();
        sorted_models.sort();
        sorted_rows.sort();
        assert_eq!(sorted_rows, sorted_models, "models/fal.yaml and fal::MODELS disagree");
        for id in MODELS {
            assert!(resolve_spec(id).is_some(), "{id} has no endpoint");
        }
        assert!(resolve_spec("fal/not-a-3d-model").is_none());
    }

    #[test]
    fn each_catalog_row_declares_exactly_what_its_endpoint_takes() {
        for id in MODELS {
            let m = schema(id);
            let s = resolve_spec(id).unwrap();

            assert_eq!(m.capabilities.text_to_3d, s.text_to_3d.is_some(), "{id}: text_to_3d");
            assert_eq!(m.capabilities.image_to_3d, s.image_to_3d.is_some(), "{id}: image_to_3d");
            assert_eq!(m.prompt.max_length == Some(0), !s.prompt, "{id}: prompt is declared iff the endpoint has one");

            let formats = m.output_formats_spec().expect("every fal 3D row declares output_formats");
            let native: Vec<&str> = if s.format_values.is_empty() { vec!["glb"] } else { s.format_values.to_vec() };
            assert_eq!(formats.enum_values, native, "{id}: output_formats must be what the endpoint emits");
            assert_eq!(formats.max_items, Some(1), "{id}: one container per fal job");

            match (m.params.get("seed"), s.seed_max) {
                (Some(ParamSpec::Seed(seed)), Some(max)) => assert_eq!((seed.min, seed.max), (0, max), "{id}: seed range"),
                (None, None) => {}
                other => panic!("{id}: seed declared {:?} but endpoint max is {:?}", other.0.is_some(), other.1),
            }
            assert_eq!(m.params.contains_key("texture"), s.texture_field.is_some(), "{id}: texture");
            assert_eq!(m.params.contains_key("pbr"), s.material_field.is_some(), "{id}: pbr");
            for unmapped in ["target_polycount", "topology", "symmetry", "rig", "negative_prompt"] {
                assert!(!m.params.contains_key(unmapped), "{id} declares {unmapped}, which no fal 3D endpoint takes");
            }

            let reserved = s.mapped_fields();
            for key in &m.extra_allowlist {
                assert!(!reserved.contains(&key.as_str()), "{id}: extra_allowlist must not expose the mapped field {key}");
            }

            let refs = m.ref_inputs.as_ref().expect("every fal 3D row takes images");
            assert!(matches!(refs.provider_format, RefProviderFormat::Url), "{id}: fal takes images as URLs");
            let init = refs.roles.get("init").expect("an init role");
            if !s.image_is_array {
                assert_eq!(init.max_count, 1, "{id}: {} takes one image", s.image_field);
            }
            assert_eq!(init.required, s.text_to_3d.is_none(), "{id}: the image is required iff there is no text arm");
            assert!(m.docs_url.as_deref().is_some_and(|u| u.contains(s.image_to_3d.unwrap_or_default())), "{id}: docs_url");
        }
    }

    // ─── Pure helpers ───────────────────────────────────────────────────────

    #[test]
    fn the_vendor_is_asked_for_glb_unless_a_format_only_it_can_make_was_promised() {
        let rodin = resolve_spec("fal/hyper3d-rodin").unwrap();
        let ask = |f: &[&str]| vendor_format(&rodin, &f.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(ask(&["obj", "stl"]).as_deref(), Some("glb"), "glb is the richest conversion source");
        assert_eq!(ask(&["fbx"]).as_deref(), Some("fbx"));
        assert_eq!(ask(&["USDZ"]).as_deref(), Some("usdz"), "sent in the endpoint's own lowercase");
        let trellis = resolve_spec("fal/trellis").unwrap();
        assert_eq!(vendor_format(&trellis, &["obj".into()]), None, "no format field, nothing sent");
    }

    #[test]
    fn format_comes_from_the_url_extension_before_the_content_type() {
        assert_eq!(format_of("https://v3.fal.media/files/x/model.glb", "application/octet-stream"), "glb");
        assert_eq!(format_of("https://v3.fal.media/files/x/mesh.obj?token=abc", "application/octet-stream"), "obj");
        assert_eq!(format_of("https://x/model.glb?Expires=1&Signature=a.b.c", ""), "glb", "a query string is not an extension");
        assert_eq!(format_of("https://v3.fal.media/files/x/abcdef", "model/stl"), "stl");
        assert_eq!(format_of("https://x/y", "application/octet-stream"), "glb", "every fal 3D endpoint documents GLB");
    }

    #[test]
    fn file_url_needs_a_url_and_tolerates_a_missing_content_type() {
        assert_eq!(file_url(&json!({"url": "https://x/a.glb"})), Some(("https://x/a.glb", "")));
        assert_eq!(file_url(&json!({"content_type": "model/gltf-binary"})), None);
        assert_eq!(file_url(&json!("https://x/a.glb")), None);
    }

    #[test]
    fn every_fal_error_envelope_yields_a_usable_message() {
        let v = json!({"detail": [
            {"loc": ["body", "image_url"], "msg": "Field required", "type": "missing"},
            {"loc": ["body", "seed"], "msg": "Input should be <= 65535", "type": "less_than_equal"},
        ]});
        let m = fal_error_message(&v);
        assert!(m.contains("body.image_url: Field required") && m.contains("body.seed"), "{m}");
        assert_eq!(fal_error_message(&json!({"detail": "Request timed out", "error_type": "request_timeout"})), "Request timed out (request_timeout)");
        assert_eq!(fal_error_message(&json!({"message": "nope"})), "nope");
        assert_eq!(fal_error_message(&json!({})), "Unknown error");
    }

    #[test]
    fn the_provider_reports_unconfigured_rather_than_pretending() {
        let p = FalModel3dProvider::new();
        assert!(!p.is_configured());
        assert!(matches!(p.api_key(), Err(ProviderError::NotConfigured(_))));
    }
}
