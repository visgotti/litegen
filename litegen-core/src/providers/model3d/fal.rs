// Fal.ai 3D (mesh) generation provider.
//
// Queue API: POST https://queue.fal.run/{endpoint} → poll status → fetch result,
// the same shape `video::fal` uses. Auth is `Authorization: Key <token>` — NOT
// Bearer, which is the single most common mistake against this API.
//
// @see <https://docs.fal.ai/model-apis/model-endpoints/queue>

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::providers::{
    build_cost_estimate, ApiKeyPool, BaseGenerationRequest, HealthCheckResult, Model3dExtras,
    Model3dFile, Model3dGenerationHandle, Model3dGenerationPollResult, Model3dProvider,
    ProviderError, ProviderInstanceConfig,
};
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::types::*;

/// Largest mesh or texture we will pull down from a vendor URL.
///
/// The bytes are held in memory through rehost, and `mesh::MAX_INPUT_BYTES`
/// refuses to parse anything larger anyway, so accepting more would only mean
/// paying to buffer a file we are about to reject.
const MAX_ASSET_BYTES: u64 = 128 * 1024 * 1024;

/// What one fal 3D endpoint's input schema actually accepts.
///
/// fal's 3D endpoints are per-model and their schemas differ sharply — sending a
/// field the schema does not define is not harmless, and there is no shared
/// vocabulary to fall back on: the format parameter alone is spelled four
/// different ways across fal's 3D catalogue, and most endpoints have none at
/// all. So each is declared rather than guessed.
///
/// Every field here was read from that endpoint's own OpenAPI document at
/// `https://fal.ai/api/openapi/queue/openapi.json?endpoint_id={slug}`.
struct EndpointSpec {
    /// Endpoint used with no reference image (text-to-3D). Empty when the model
    /// has no text-to-3D mode at all.
    text_to_3d: &'static str,
    /// Endpoint used when a reference image is supplied.
    image_to_3d: &'static str,
    /// The field carrying the input image URL. fal is not consistent:
    /// `fal-ai/trellis` takes `image_url`, `fal-ai/hunyuan3d/v2` takes
    /// `input_image_url`, and `fal-ai/hyper3d/rodin` takes an ARRAY under
    /// `input_image_urls`.
    image_field: &'static str,
    /// Whether `image_field` is an array of URLs rather than one.
    image_is_array: bool,
    /// The endpoint's own name for the output container, and the values it
    /// accepts. Empty name = the endpoint emits GLB with no choice, which is
    /// most of them — and exactly the case litegen's local conversion turns
    /// into four deliverable formats.
    format_field: &'static str,
    format_values: &'static [&'static str],
    /// `true` when the endpoint takes a `seed`; `seed_max` because Rodin's is
    /// u16, not the u32 the rest of fal uses. Sending 2^31 there is a 422.
    seed_max: Option<i64>,
    /// The endpoint's texture toggle, if any, and whether it is a bare bool.
    texture_field: &'static str,
    /// Rodin expresses PBR-vs-flat as `material: PBR|Shaded` rather than a bool.
    material_field: &'static str,
}

const UNSUPPORTED: EndpointSpec = EndpointSpec {
    text_to_3d: "",
    image_to_3d: "",
    image_field: "image_url",
    image_is_array: false,
    format_field: "",
    format_values: &[],
    seed_max: None,
    texture_field: "",
    material_field: "",
};

pub struct FalModel3dProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
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

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://queue.fal.run")
    }

    /// Resolve a litegen model id to its fal endpoint and field support.
    ///
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/hyper3d/rodin>
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/trellis>
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/hunyuan3d/v2>
    fn resolve_spec(model: &str) -> EndpointSpec {
        match model {
            "fal/hyper3d-rodin" => EndpointSpec {
                text_to_3d: "fal-ai/hyper3d/rodin",
                image_to_3d: "fal-ai/hyper3d/rodin",
                image_field: "input_image_urls",
                image_is_array: true,
                format_field: "geometry_file_format",
                format_values: &["glb", "usdz", "fbx", "obj", "stl"],
                // Rodin's seed is `minimum: 0, maximum: 65535`.
                seed_max: Some(65535),
                texture_field: "",
                material_field: "material",
            },
            "fal/trellis" => EndpointSpec {
                // `image_url` is REQUIRED by TrellisInput — there is no
                // text-to-3D arm to fall back to.
                text_to_3d: "",
                image_to_3d: "fal-ai/trellis",
                image_field: "image_url",
                image_is_array: false,
                format_field: "",
                format_values: &[],
                seed_max: Some(i64::from(u32::MAX)),
                texture_field: "",
                material_field: "",
            },
            "fal/hunyuan3d-v2" => EndpointSpec {
                text_to_3d: "",
                image_to_3d: "fal-ai/hunyuan3d/v2",
                image_field: "input_image_url",
                image_is_array: false,
                format_field: "",
                format_values: &[],
                seed_max: Some(i64::from(u32::MAX)),
                texture_field: "textured_mesh",
                material_field: "",
            },
            _ => UNSUPPORTED,
        }
    }

    /// The single container to ask the vendor for.
    ///
    /// litegen derives the rest locally, so the vendor only ever needs to
    /// produce ONE — and it should be the one that carries the most, since it is
    /// the conversion source. GLB wins whenever the endpoint offers it: it is
    /// the only container in our convertible set that carries UVs and colour.
    /// Anything the caller asked for that this endpoint cannot emit and we
    /// cannot derive has already been rejected in validation.
    fn vendor_format(spec: &EndpointSpec, requested: &[String]) -> Option<String> {
        if spec.format_values.is_empty() {
            return None;
        }
        let offers = |f: &str| spec.format_values.iter().any(|v| v.eq_ignore_ascii_case(f));

        // A container litegen CANNOT derive has to come from the vendor itself,
        // so it wins over the GLB preference below. Getting this backwards is
        // not a missed optimisation: asking for glb when the caller wanted fbx
        // delivers a glb, and the completion guard then fails a generation the
        // vendor was perfectly capable of fulfilling — after it was billed.
        //
        // At most one such format can be here. `vendor_jobs_needed` counts each
        // non-derivable format as its own vendor job, so a request naming two
        // is rejected in validation against this endpoint's `max_items`.
        if let Some(native_only) = requested
            .iter()
            .find(|r| crate::mesh::MeshFormat::parse(r).is_none() && offers(r))
        {
            return Some(native_only.clone());
        }

        // Otherwise GLB: it is the conversion source, and the only container in
        // the derivable set carrying both UVs and colour.
        if offers(DEFAULT_MODEL3D_FORMAT) {
            return Some(DEFAULT_MODEL3D_FORMAT.to_string());
        }
        // No GLB on offer: ask for something the caller actually wants, then
        // whatever the endpoint's own default would have been.
        requested
            .iter()
            .find(|r| offers(r))
            .cloned()
            .or_else(|| spec.format_values.first().map(|s| s.to_string()))
    }

    /// Download one vendor asset into bytes.
    ///
    /// fal hands back plain https URLs rather than bytes, and litegen's contract
    /// is that a provider returns BYTES — several 3D vendors expire download
    /// URLs within minutes of task success, so deferring the fetch to rehost
    /// would be a race we lose intermittently and only in production.
    async fn fetch_asset(&self, url: &str) -> Result<(Vec<u8>, String), ProviderError> {
        let resp = self.client.get(url).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Fal 3d asset download failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_timeout() || e.is_connect(),
        })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Fal 3d asset download returned {status}"),
                status_code: Some(status.as_u16()),
                provider_error: None,
                retryable: status.as_u16() >= 500,
            });
        }
        // Refuse on the declared length before buffering, when the vendor gives
        // one. `bytes()` below is still bounded by the client's own limits.
        if let Some(len) = resp.content_length() {
            if len > MAX_ASSET_BYTES {
                return Err(ProviderError::RequestFailed {
                    message: format!("Fal 3d asset is {len} bytes, over the {MAX_ASSET_BYTES} limit"),
                    status_code: None,
                    provider_error: None,
                    retryable: false,
                });
            }
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .split(';')
            .next()
            .unwrap_or("application/octet-stream")
            .trim()
            .to_string();
        let bytes = resp.bytes().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Fal 3d asset body failed: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;
        if bytes.len() as u64 > MAX_ASSET_BYTES {
            return Err(ProviderError::RequestFailed {
                message: format!("Fal 3d asset is {} bytes, over the limit", bytes.len()),
                status_code: None,
                provider_error: None,
                retryable: false,
            });
        }
        Ok((bytes.to_vec(), content_type))
    }
}

/// Pull a human-readable message out of any of fal's THREE error envelopes.
///
/// They are genuinely different shapes, and reading only one of them turns a
/// precise vendor message into "Unknown error" at the exact moment someone needs
/// it:
///
/// - 422 validation: `detail` is an ARRAY of `{loc, msg, type, …}`.
/// - infrastructure: `detail` is a STRING, with a sibling `error_type`.
/// - some endpoints are still being migrated and use neither.
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
                        let path: Vec<String> =
                            loc.iter().map(|p| p.as_str().map(str::to_string)
                                .unwrap_or_else(|| p.to_string())).collect();
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
/// meshes from object storage, which commonly labels everything
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

/// Pull `{url, content_type}` out of a fal `File` object.
fn file_url(v: &Value) -> Option<(&str, &str)> {
    let url = v.get("url")?.as_str()?;
    let ct = v.get("content_type").and_then(|c| c.as_str()).unwrap_or("");
    Some((url, ct))
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

    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &Model3dExtras,
        materialized: &MaterializedRequest,
    ) -> Result<Model3dGenerationHandle, ProviderError> {
        let api_key = self.api_key()?;
        let spec = Self::resolve_spec(&model.id);

        // Mode comes from whether a reference image was materialised, exactly as
        // the trait documents. fal wants a URL, so a ref that only exists as
        // bytes or base64 cannot be used here — say so instead of submitting a
        // request that silently ignores the image the caller paid to upload.
        let image_urls: Vec<String> = materialized
            .refs
            .iter()
            .filter_map(|r| match &r.form {
                MaterializedRefForm::Url(u) => Some(u.clone()),
                // fal wants a URL. A ref that only exists as base64 or as a
                // multipart field cannot be forwarded, and submitting without it
                // would silently ignore the image the caller paid to upload.
                _ => None,
            })
            .collect();

        let endpoint = if image_urls.is_empty() {
            if spec.text_to_3d.is_empty() {
                return Err(ProviderError::InvalidRequest(format!(
                    "'{}' is image-to-3D only; supply a reference image",
                    model.id
                )));
            }
            spec.text_to_3d
        } else {
            if spec.image_to_3d.is_empty() {
                return Err(ProviderError::InvalidRequest(format!(
                    "'{}' does not accept a reference image",
                    model.id
                )));
            }
            spec.image_to_3d
        };
        if endpoint.is_empty() {
            return Err(ProviderError::InvalidRequest(format!(
                "'{}' is not a fal 3D model",
                model.id
            )));
        }

        let mut body = json!({});
        if !base.prompt.trim().is_empty() {
            body["prompt"] = Value::String(base.prompt.clone());
        }
        if !image_urls.is_empty() {
            body[spec.image_field] = if spec.image_is_array {
                Value::Array(image_urls.iter().map(|u| Value::String(u.clone())).collect())
            } else {
                Value::String(image_urls[0].clone())
            };
        }

        // ONE container from the vendor; litegen derives the rest.
        if let Some(fmt) = Self::vendor_format(&spec, &extras.output_formats) {
            body[spec.format_field] = Value::String(fmt);
        }

        if let Some(seed) = base.seed {
            // Rodin's seed is a u16. Clamping silently would make `seed` stop
            // meaning "reproducible", so an out-of-range value is refused.
            if let Some(max) = spec.seed_max {
                if seed < 0 || seed > max {
                    return Err(ProviderError::InvalidRequest(format!(
                        "'{}' accepts a seed in 0..={}; got {}",
                        model.id, max, seed
                    )));
                }
                body["seed"] = json!(seed);
            }
        }

        if !spec.texture_field.is_empty() {
            if let Some(t) = extras.texture {
                body[spec.texture_field] = json!(t);
            }
        }
        if !spec.material_field.is_empty() {
            // Rodin: PBR or Shaded. `pbr: false` means flat shading, not "no
            // material" — Shaded is the endpoint's own word for that.
            if let Some(pbr) = extras.pbr {
                body[spec.material_field] = Value::String(
                    if pbr { "PBR".to_string() } else { "Shaded".to_string() },
                );
            }
        }

        // Shallow-merge `extra`, matching every other fal adapter: it is the
        // escape hatch for endpoint-specific fields the canonical vocabulary
        // does not cover (Trellis's `mesh_simplify`, Hunyuan's
        // `octree_resolution`), gated by each model's `extra_allowlist`.
        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(obj) = body.as_object_mut() {
                for (k, v) in extra_map {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let url = format!("{}/{}", self.api_base(), endpoint);
        let resp = crate::providers::inject_trace_headers(
            self.client
                .post(&url)
                .header("Authorization", format!("Key {api_key}"))
                .header("Content-Type", "application/json")
                .json(&body),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("Fal 3d request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_timeout() || e.is_connect(),
        })?;

        let status = resp.status();
        let resp_json: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Fal 3d response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        if !status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Fal 3d API error: {}", fal_error_message(&resp_json)),
                status_code: Some(status.as_u16()),
                provider_error: Some(resp_json),
                // 429 is fal's concurrency limiter, not a bad request: new
                // accounts get 2 simultaneous in-progress runs. It sets
                // `X-Fal-needs-retry: 1` and is exactly what a retry is for.
                retryable: status.as_u16() >= 500 || status.as_u16() == 429,
            });
        }

        let request_id = resp_json["request_id"]
            .as_str()
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "Fal 3d response missing request_id".to_string(),
                status_code: None,
                provider_error: Some(resp_json.clone()),
                retryable: false,
            })?
            .to_string();

        Ok(Model3dGenerationHandle {
            provider_job_id: request_id,
            provider: "fal".to_string(),
            model: model.id.clone(),
            // The endpoint is not derivable from the model id alone once a model
            // has separate text and image arms, and fal's own status/response
            // URLs save reconstructing them. This rides the row (see
            // `get_model3d_stage_context`), so the poller can rebuild the handle
            // after a restart.
            stage_context: Some(json!({
                "endpoint": endpoint,
                "status_url": resp_json["status_url"].as_str().unwrap_or(""),
                "response_url": resp_json["response_url"].as_str().unwrap_or(""),
            })),
        })
    }

    async fn poll_status(
        &self,
        handle: &Model3dGenerationHandle,
    ) -> Result<Model3dGenerationPollResult, ProviderError> {
        let api_key = self.api_key()?;
        let ctx = handle.stage_context.clone().unwrap_or_else(|| json!({}));
        let endpoint = ctx["endpoint"].as_str().unwrap_or("");
        let base = self.api_base();

        let status_url = match ctx["status_url"].as_str() {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => format!("{base}/{endpoint}/requests/{}/status", handle.provider_job_id),
        };

        let resp = crate::providers::inject_trace_headers(
            self.client.get(&status_url).header("Authorization", format!("Key {api_key}")),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("Fal 3d status failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: true,
        })?;

        let http = resp.status();
        let status_json: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Fal 3d status: {e}"),
            status_code: Some(http.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        let meta: HashMap<String, Value> = HashMap::from([
            ("provider".to_string(), json!("fal")),
            ("request_id".to_string(), json!(handle.provider_job_id)),
        ]);

        let queue_status = status_json["status"].as_str().unwrap_or("");
        match queue_status {
            "IN_QUEUE" => {
                return Ok(Model3dGenerationPollResult {
                    status: GenerationStatus::Pending,
                    progress: 5,
                    files: Vec::new(),
                    error: None,
                    metadata: meta,
                })
            }
            "IN_PROGRESS" => {
                return Ok(Model3dGenerationPollResult {
                    status: GenerationStatus::Processing,
                    // fal reports queue position, not percent-complete, and
                    // inventing a curve from it would be a lie with a number
                    // attached. A fixed mid-value says "running" honestly.
                    progress: 50,
                    files: Vec::new(),
                    error: None,
                    metadata: meta,
                })
            }
            "COMPLETED" => {}
            other => {
                return Err(ProviderError::RequestFailed {
                    message: format!("Fal 3d returned an unknown queue status '{other}'"),
                    status_code: Some(http.as_u16()),
                    provider_error: Some(status_json),
                    retryable: false,
                })
            }
        }

        // fal reports a FAILED request as COMPLETED carrying an `error` — there
        // is no FAILED status on this API. Reading only `status` therefore turns
        // a vendor failure into a completed generation with no mesh.
        // @see <https://docs.fal.ai/model-apis/model-endpoints/queue.md>
        if let Some(err) = status_json["error"].as_str().filter(|e| !e.is_empty()) {
            // `error_type` is the machine-readable half (content_policy_violation,
            // image_too_large, request_timeout, …). Carrying it means a caller
            // can branch on the reason without parsing English.
            let detail = match status_json["error_type"].as_str().filter(|t| !t.is_empty()) {
                Some(t) => format!("{err} ({t})"),
                None => err.to_string(),
            };
            return Ok(Model3dGenerationPollResult {
                status: GenerationStatus::Failed,
                progress: 100,
                files: Vec::new(),
                error: Some(format!("Fal 3d generation failed: {detail}")),
                metadata: meta,
            });
        }

        let response_url = match ctx["response_url"].as_str() {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => format!("{base}/{endpoint}/requests/{}", handle.provider_job_id),
        };
        let resp = crate::providers::inject_trace_headers(
            self.client.get(&response_url).header("Authorization", format!("Key {api_key}")),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("Fal 3d result fetch failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: true,
        })?;
        let http = resp.status();
        let result: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Fal 3d result: {e}"),
            status_code: Some(http.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        // Every fal 3D endpoint read so far names the mesh `model_mesh`, but the
        // catalogue is not consistent — some variants use `model_glb` — so both
        // are accepted rather than assuming.
        let mesh = result
            .get("model_mesh")
            .or_else(|| result.get("model_glb"))
            .and_then(file_url);

        let Some((mesh_url, mesh_ct)) = mesh else {
            // COMPLETED, no error, no mesh. Reporting this as success would
            // hand the caller a billed generation they cannot use; the
            // completion guard would fail it anyway, but saying so here puts the
            // vendor's own response in the error.
            return Ok(Model3dGenerationPollResult {
                status: GenerationStatus::Failed,
                progress: 100,
                files: Vec::new(),
                error: Some("Fal 3d completed without a mesh in its result".to_string()),
                metadata: meta,
            });
        };

        let (bytes, ct) = self.fetch_asset(mesh_url).await?;
        let format = format_of(mesh_url, if mesh_ct.is_empty() { &ct } else { mesh_ct });
        let mut files = vec![Model3dFile {
            kind: Model3dAssetKind::Mesh,
            format,
            content_type: if mesh_ct.is_empty() { ct } else { mesh_ct.to_string() },
            bytes,
            // No fal 3D endpoint reports a triangle count. Inventing one from
            // the request's `target_polycount` would report the ASK as the
            // result; leaving it None is the honest answer.
            polycount: None,
            width: None,
            height: None,
        }];

        // Rodin returns its PBR maps separately under `textures`. This is the
        // first producer of `Model3dAssetKind::Texture` — the kind has been
        // declared and consumed since 3D shipped, with nothing filling it.
        if let Some(textures) = result.get("textures").and_then(|t| t.as_array()) {
            for t in textures {
                let Some((url, tct)) = file_url(t) else { continue };
                // One bad texture must not fail a generation whose mesh is
                // fine: the mesh is the deliverable, the maps are a bonus.
                match self.fetch_asset(url).await {
                    Ok((bytes, ct)) => files.push(Model3dFile {
                        kind: Model3dAssetKind::Texture,
                        format: format_of(url, if tct.is_empty() { &ct } else { tct }),
                        content_type: if tct.is_empty() { ct } else { tct.to_string() },
                        bytes,
                        polycount: None,
                        width: None,
                        height: None,
                    }),
                    Err(e) => tracing::warn!(url = %url, error = %e, "fal 3d: skipping a texture that would not download"),
                }
            }
        }

        Ok(Model3dGenerationPollResult {
            status: GenerationStatus::Completed,
            progress: 100,
            files,
            error: None,
            metadata: meta,
        })
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &Model3dGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        // Flat per-generation, as fal documents it. The conditional multipliers
        // (Hunyuan's textured_mesh, Rodin's HighPack addon) are recorded in the
        // catalog's `variable_pricing` rather than applied here: quoting a
        // number this endpoint cannot prove would be worse than quoting the base.
        Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, None))
    }

    async fn health_check(&self) -> HealthCheckResult {
        HealthCheckResult {
            healthy: self.is_configured(),
            message: if self.is_configured() {
                "fal 3d provider configured".to_string()
            } else {
                "fal 3d provider is not configured".to_string()
            },
            latency_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalogued_fal_3d_model_resolves_to_an_endpoint() {
        // A model in fal.yaml with no arm here submits nothing and fails at
        // runtime; catalog_conformance cannot see this table, so this is the
        // only thing that ties the two together.
        for id in ["fal/hyper3d-rodin", "fal/trellis", "fal/hunyuan3d-v2"] {
            let spec = FalModel3dProvider::resolve_spec(id);
            assert!(
                !spec.image_to_3d.is_empty() || !spec.text_to_3d.is_empty(),
                "{id} resolves to no endpoint",
            );
        }
        assert!(FalModel3dProvider::resolve_spec("fal/not-a-3d-model").image_to_3d.is_empty());
    }

    #[test]
    fn the_vendor_is_asked_for_glb_whenever_it_offers_it() {
        // GLB is the conversion source, and the only convertible container that
        // carries UVs and colour — so asking for anything else would discard
        // detail before we ever convert.
        let rodin = FalModel3dProvider::resolve_spec("fal/hyper3d-rodin");
        assert_eq!(
            FalModel3dProvider::vendor_format(&rodin, &["obj".into(), "stl".into()]),
            Some("glb".to_string()),
        );
    }

    #[test]
    fn a_format_we_cannot_derive_is_asked_of_the_vendor_instead_of_glb() {
        // Rodin CAN emit fbx and litegen cannot derive it, so the vendor has to
        // be the one to produce it. Preferring glb here would deliver a glb for
        // a request that said fbx, and the completion guard would then fail a
        // generation the vendor could have fulfilled — after it was billed.
        let rodin = FalModel3dProvider::resolve_spec("fal/hyper3d-rodin");
        assert_eq!(
            FalModel3dProvider::vendor_format(&rodin, &["fbx".into()]),
            Some("fbx".to_string()),
        );
        assert_eq!(
            FalModel3dProvider::vendor_format(&rodin, &["usdz".into()]),
            Some("usdz".to_string()),
        );
        // But a derivable format still sources from glb, because glb carries
        // more than obj or stl can.
        assert_eq!(
            FalModel3dProvider::vendor_format(&rodin, &["obj".into(), "stl".into()]),
            Some("glb".to_string()),
        );
    }

    #[test]
    fn an_endpoint_with_no_format_param_is_sent_none() {
        // Most fal 3D endpoints emit GLB with no choice. Sending them a field
        // their schema does not define is a 422, not a no-op.
        let trellis = FalModel3dProvider::resolve_spec("fal/trellis");
        assert_eq!(FalModel3dProvider::vendor_format(&trellis, &["glb".into()]), None);
        assert_eq!(FalModel3dProvider::vendor_format(&trellis, &["obj".into()]), None);
    }

    #[test]
    fn format_comes_from_the_url_extension_before_the_content_type() {
        // fal serves meshes from object storage, which labels almost everything
        // application/octet-stream while the generated filename carries the
        // real extension.
        assert_eq!(format_of("https://v3.fal.media/files/x/model.glb", "application/octet-stream"), "glb");
        assert_eq!(format_of("https://v3.fal.media/files/x/mesh.obj?token=abc", "application/octet-stream"), "obj");
        assert_eq!(format_of("https://v3.fal.media/files/x/tex.png", ""), "png");
    }

    #[test]
    fn format_falls_back_to_the_content_type_when_the_url_has_no_extension() {
        assert_eq!(format_of("https://v3.fal.media/files/x/abcdef", "model/gltf-binary"), "glb");
        assert_eq!(format_of("https://v3.fal.media/files/x/abcdef", "model/stl"), "stl");
        // Unknown, extensionless: glb is the only honest default, since every
        // fal 3D endpoint documents GLB output.
        assert_eq!(format_of("https://x/y", "application/octet-stream"), "glb");
    }

    #[test]
    fn a_query_string_is_not_mistaken_for_an_extension() {
        assert_eq!(format_of("https://x/model.glb?Expires=1&Signature=a.b.c", ""), "glb");
    }

    #[test]
    fn file_url_needs_a_url_and_tolerates_a_missing_content_type() {
        assert_eq!(file_url(&json!({"url": "https://x/a.glb"})), Some(("https://x/a.glb", "")));
        assert_eq!(
            file_url(&json!({"url": "https://x/a.glb", "content_type": "model/gltf-binary"})),
            Some(("https://x/a.glb", "model/gltf-binary")),
        );
        assert_eq!(file_url(&json!({"content_type": "model/gltf-binary"})), None);
        assert_eq!(file_url(&json!("https://x/a.glb")), None);
    }

    #[test]
    fn rodins_seed_ceiling_is_its_own_not_fals() {
        // Rodin's schema says `maximum: 65535`. Sending the u32 the rest of fal
        // accepts is a 422, and clamping silently would make `seed` stop meaning
        // reproducible.
        assert_eq!(FalModel3dProvider::resolve_spec("fal/hyper3d-rodin").seed_max, Some(65535));
        assert_eq!(
            FalModel3dProvider::resolve_spec("fal/trellis").seed_max,
            Some(i64::from(u32::MAX)),
        );
    }

    #[test]
    fn models_without_a_text_arm_are_declared_as_such() {
        // TrellisInput and Hunyuan3dV2Input both mark their image field
        // REQUIRED, so a text-only request must be refused before it is billed.
        for id in ["fal/trellis", "fal/hunyuan3d-v2"] {
            assert!(
                FalModel3dProvider::resolve_spec(id).text_to_3d.is_empty(),
                "{id} claims a text-to-3D arm it does not have",
            );
        }
        assert!(!FalModel3dProvider::resolve_spec("fal/hyper3d-rodin").text_to_3d.is_empty());
    }

    #[test]
    fn the_image_field_spelling_differs_per_endpoint() {
        // fal is not consistent, and the wrong spelling submits a request that
        // silently ignores the image the caller paid to upload.
        assert_eq!(FalModel3dProvider::resolve_spec("fal/trellis").image_field, "image_url");
        assert_eq!(FalModel3dProvider::resolve_spec("fal/hunyuan3d-v2").image_field, "input_image_url");
        let rodin = FalModel3dProvider::resolve_spec("fal/hyper3d-rodin");
        assert_eq!(rodin.image_field, "input_image_urls");
        assert!(rodin.image_is_array, "Rodin takes an ARRAY of image urls");
    }


    #[test]
    fn every_fal_error_envelope_yields_a_usable_message() {
        // fal has three genuinely different error shapes, and reading only one
        // turns a precise vendor message into "Unknown error" exactly when
        // someone needs it.

        // 422 validation: `detail` is an ARRAY. `loc` names the offending field,
        // which is the whole value — "field required" alone says nothing.
        let v = json!({"detail": [
            {"loc": ["body", "image_url"], "msg": "Field required", "type": "missing"},
            {"loc": ["body", "seed"], "msg": "Input should be <= 65535", "type": "less_than_equal"},
        ]});
        let m = fal_error_message(&v);
        assert!(m.contains("body.image_url: Field required"), "{m}");
        assert!(m.contains("body.seed"), "{m}");

        // Infrastructure: `detail` is a STRING with a sibling error_type.
        let v = json!({"detail": "Request timed out", "error_type": "request_timeout"});
        assert_eq!(fal_error_message(&v), "Request timed out (request_timeout)");

        // Neither shape — some endpoints are still being migrated.
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
