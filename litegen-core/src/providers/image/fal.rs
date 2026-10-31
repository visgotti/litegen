use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRequest, MaterializedRefForm};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderError, ProviderInstanceConfig, SegmentationInput, VendorMask, build_cost_estimate,
};
use crate::providers::model3d::download::{fetch_asset, AssetError};
use crate::providers::model3d::fal::{needs_retry, platform_key_check, transient, QueueReply, DEFAULT_PLATFORM_BASE, DEFAULT_QUEUE_BASE};
use crate::types::*;

/// fal's synchronous host, where images run.
const DEFAULT_SYNC_BASE: &str = "https://fal.run";

/// The most a result image may weigh.
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;

/// What litegen itself sends in a generation body; `extra` cannot replace it.
/// `num_images` and `sync_mode` are here although litegen sends neither: fal
/// would bill every image it made while litegen keeps one, and a `data:` URI
/// result is not one the adapter fetches.
const OWNED_FIELDS: &[&str] = &[
    "prompt", "num_images", "image_size", "seed", "negative_prompt", "guidance_scale",
    "num_inference_steps", "strength", "style", "image_url", "sync_mode", "output_format",
];

/// Fal.ai image generation provider.
///
/// Routes to model-specific Fal endpoints for Flux, SDXL, SD3.5, Recraft, etc.
/// Sync API: `POST https://fal.run/{endpoint}` → `{images: [{url, ...}]}`
///
/// @see <https://fal.ai/docs/documentation/model-apis/inference/synchronous> — Verbatim: "`run`
///   sends a direct HTTP request to `fal.run`. There is no queue and no status polling."
/// @see <https://fal.ai/models> — per-model API pages documenting each endpoint's input schema
pub struct FalProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
    /// Result downloads: no redirects, so a URL that passed the public-address
    /// check cannot bounce into a private one.
    asset_client: Client,
    /// Whether a result URL must resolve to a public address. Only the tests,
    /// whose mock vendor runs on loopback, turn it off.
    pub(crate) public_assets_only: bool,
}

impl FalProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .expect("failed to build HTTP client"),
            asset_client: crate::util::ssrf::no_redirect_download_client(
                std::time::Duration::from_secs(120),
                std::time::Duration::from_secs(30),
            ),
            public_assets_only: true,
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

    /// The host images RUN on. `providers.fal.api_base` is shared with the
    /// video and 3D adapters, for which it is fal's QUEUE host, so fal's own
    /// queue host means its sync host here, and any other base (a proxy) is
    /// used for both; `options.sync_api_base` names the sync host outright.
    ///
    /// @see <https://fal.ai/docs/documentation/model-apis/inference/queue> — Verbatim: "Submit
    ///   requests to queue.fal.run"
    pub(crate) fn sync_base(&self) -> String {
        let config = self.config.as_ref();
        if let Some(base) = config.and_then(|c| c.options.as_ref()).and_then(|o| o["sync_api_base"].as_str()) {
            return base.trim_end_matches('/').to_string();
        }
        match config.and_then(|c| c.api_base.as_deref()).map(|b| b.trim_end_matches('/')) {
            None => DEFAULT_SYNC_BASE.to_string(),
            Some(b) if b == DEFAULT_QUEUE_BASE => DEFAULT_SYNC_BASE.to_string(),
            Some(b) => b.to_string(),
        }
    }

    /// fal's Platform API, which the health check reads.
    fn platform_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.options.as_ref())
            .and_then(|o| o["platform_api_base"].as_str())
            .unwrap_or(DEFAULT_PLATFORM_BASE)
    }

    /// Resolve litegen model ID to the Fal endpoint path (e.g. "fal-ai/flux/dev").
    /// `None` for an id with no arm: refused, not billed as FLUX.1 [dev] (L13).
    fn resolve_endpoint(&self, model_id: &str) -> Option<&'static str> {
        let native = if let Some(rest) = model_id.strip_prefix("fal/") {
            rest
        } else {
            model_id
        };

        Some(match native {
            "flux-schnell" => "fal-ai/flux/schnell",
            "flux-dev" => "fal-ai/flux/dev",
            // FLUX.2 [dev] text-to-image. Without this arm the id fell back to
            // FLUX.1 [dev] below and billed as a different model.
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2>
            "flux-2" => "fal-ai/flux-2",
            "flux-pro" => "fal-ai/flux-pro/v1.1",
            "sdxl" | "sdxl-lightning" => "fal-ai/fast-sdxl",
            "sd35-medium" | "sd3.5-medium" => "fal-ai/stable-diffusion-v35-medium",
            // fal renamed `fal-ai/recraft-v3`: its model page 308-redirects to
            // this id and the model registry no longer lists the old one. The
            // input schema is identical, so only the path moves.
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft/v3/text-to-image>
            "recraft-v3" => "fal-ai/recraft/v3/text-to-image",
            "auraflow" | "aura-flow" => "fal-ai/aura-flow",
            // Background removal (PixelGenie L8).
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/bria/background/remove>
            "bria-rmbg-2" => "fal-ai/bria/background/remove",
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/birefnet/v2>
            "birefnet-v2" => "fal-ai/birefnet/v2",
            _ => return None,
        })
    }

    /// Parse a size string "WxH" into {width, height}.
    fn parse_size(size: &str) -> Option<(u64, u64)> {
        let parts: Vec<&str> = size.split('x').collect();
        if parts.len() == 2 {
            let w = parts[0].parse::<u64>().ok()?;
            let h = parts[1].parse::<u64>().ok()?;
            Some((w, h))
        } else {
            None
        }
    }

    /// Download a result from the URL fal answered with: without the key,
    /// only from a public address, and never past the size limit. fal has run
    /// and billed the job by now, so no failure here is worth sending the
    /// request again — every error is final.
    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        match fetch_asset(&self.asset_client, url, MAX_IMAGE_BYTES, self.public_assets_only, "fal image").await {
            Ok((bytes, _)) => Ok(bytes),
            Err(AssetError::Transient(ProviderError::RequestFailed { message, status_code, provider_error, .. })) => {
                Err(ProviderError::RequestFailed { message, status_code, provider_error, retryable: false })
            }
            Err(AssetError::Transient(e)) => Err(e),
            Err(AssetError::Gone(why)) => Err(ProviderError::RequestFailed {
                message: format!("fal image result could not be fetched: {why}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            }),
        }
    }

    /// A result file's bytes: decoded from a `data:` URI (`sync_mode`), or
    /// fetched without the key.
    async fn result_bytes(&self, at: &str) -> Result<Vec<u8>, ProviderError> {
        use base64::Engine;
        match at.strip_prefix("data:").and_then(|d| d.split_once(";base64,")) {
            Some((_, b64)) => base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| ProviderError::RequestFailed {
                message: format!("Fal result is not base64: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            }),
            None => self.fetch_image_bytes(at).await,
        }
    }

    /// POST `body` to `{sync}/{endpoint}` and read the JSON answer. The reply
    /// is read whole before its status is judged, so a gateway's HTML page is
    /// a retryable 502, not a parse error; a vendor error names every message
    /// in any of fal's error shapes; and fal's `X-Fal-Needs-Retry` decides
    /// whether to ask again when it sent one. A connection that never opened
    /// is the only send failure worth repeating: fal runs, and bills, a
    /// request it received.
    ///
    /// @see <https://fal.ai/docs/api-reference/inference-api/sync/run.md> — Verbatim: "fal does not
    ///   retry synchronous requests. If a request fails because of a temporary error, the response
    ///   has `X-Fal-Needs-Retry: 1`."
    /// @see <https://fal.ai/docs/documentation/model-apis/errors> — Verbatim: "Contains a `detail`
    ///   field which is an array of `Error` objects."
    async fn run(&self, endpoint: &str, body: &Value) -> Result<Value, ProviderError> {
        let api_key = self.api_key()?;
        let url = format!("{}/{}", self.sync_base(), endpoint);
        let resp = crate::providers::inject_trace_headers(self.client.post(&url).header("Authorization", format!("Key {api_key}")).json(body))
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Fal request failed: {e}"),
                status_code: None,
                provider_error: None,
                retryable: e.is_connect(),
            })?;
        let retry = needs_retry(resp.headers());
        let reply = QueueReply::read(resp).await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Fal response could not be read: {e}"),
            status_code: None,
            provider_error: None,
            retryable: false,
        })?;
        if !reply.status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Fal API error: {}", reply.describe()),
                status_code: Some(reply.status.as_u16()),
                retryable: retry.unwrap_or_else(|| transient(reply.status)),
                provider_error: reply.json,
            });
        }
        reply.json.ok_or_else(|| ProviderError::RequestFailed {
            message: format!("Fal answered HTTP {} with a body that is not JSON", reply.status.as_u16()),
            status_code: Some(reply.status.as_u16()),
            provider_error: None,
            retryable: false,
        })
    }

    /// Background removal (PixelGenie L8): only the image, the endpoint's own
    /// knobs from `extra`, then the asked container, so `extra` cannot change
    /// it. Bria's optional `prompt` is billed $0.03 instead of $0.018 and is
    /// never sent. Both endpoints answer one `image`.
    async fn remove_background(&self, endpoint: &str, extras: &ImageExtras, materialized: &MaterializedRequest) -> Result<GenerationOutput, ProviderError> {
        let image_url = materialized.refs.iter().find(|r| r.role == "init").and_then(|r| match &r.form {
            MaterializedRefForm::Url(u) => Some(u.clone()),
            MaterializedRefForm::Base64(b64) => Some(format!("data:image/png;base64,{b64}")),
            MaterializedRefForm::MultipartField { .. } => None,
        });
        let image_url = image_url.ok_or_else(|| ProviderError::InvalidRequest("background removal needs an init image".into()))?;
        let mut body = json!({ "image_url": image_url, "sync_mode": true });
        if let (Some(Value::Object(extra)), Some(obj)) = (&extras.extra, body.as_object_mut()) {
            for (k, v) in extra {
                if !OWNED_FIELDS.contains(&k.as_str()) {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
        if let Some(f) = extras.output_format.as_deref() {
            body["output_format"] = Value::String(f.to_string());
        }
        let answer = self.run(endpoint, &body).await?;
        let at = answer["image"]["url"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Fal response has no image".into(),
            status_code: None,
            provider_error: Some(answer.clone()),
            retryable: false,
        })?;
        Ok(GenerationOutput {
            data: self.result_bytes(at).await?,
            content_type: answer["image"]["content_type"].as_str().unwrap_or("image/png").to_string(),
            metadata: HashMap::new(),
            more: Vec::new(),
        })
    }
}

/// fal's segmentation endpoints (PixelGenie L7), by catalog id. SAM 3 and
/// SAM 3.1 publish the same input.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/sam-3/image>
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/sam-3-1/image>
pub(crate) const SEGMENT_MODELS: &[(&str, &str)] = &[("fal/sam-3", "fal-ai/sam-3/image"), ("fal/sam-3.1", "fal-ai/sam-3-1/image")];

fn segment_endpoint(model_id: &str) -> Option<&'static str> {
    SEGMENT_MODELS.iter().find(|(id, _)| *id == model_id).map(|(_, e)| *e)
}

impl Default for FalProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for FalProvider {
    fn name(&self) -> &str {
        "fal"
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
            .is_some_and(|c| !c.api_key.is_empty() || self.key_pool.is_some())
    }

    /// Run a model synchronously via `POST {fal.run}/{endpoint}`.
    ///
    /// @see <https://fal.ai/docs/documentation/model-apis/inference/synchronous> — synchronous run;
    ///   `Authorization: Key <token>`, JSON body, and the `{images: [{url, content_type}]}` response
    ///   shape this method parses.
    /// @see <https://fal.ai/models> — per-model pages proving the `image_size`, `seed`,
    ///   `guidance_scale`, `num_inference_steps`, `strength`, and `image_url` input fields.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let endpoint = self.resolve_endpoint(&model.id).ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "fal".into() })?;
        if model.capabilities.background_removal {
            return self.remove_background(endpoint, extras, materialized).await;
        }

        // Build request body
        let mut body = json!({
            "prompt": base.prompt,
        });

        // Fal models use image_size: {width, height}
        if let Some(size_str) = extras.size.as_deref() {
            if let Some((w, h)) = Self::parse_size(size_str) {
                body["image_size"] = json!({
                    "width": w,
                    "height": h
                });
            }
        }

        // No `num_images`: every carried endpoint defaults to 1, and litegen
        // returns (and bills) one image — fal would bill every one it made.

        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }

        if let Some(np) = base.negative_prompt.as_deref() {
            body["negative_prompt"] = Value::String(np.to_string());
        }

        if let Some(gc) = extras.guidance_scale {
            body["guidance_scale"] = json!(gc);
        }

        if let Some(steps) = extras.steps {
            body["num_inference_steps"] = Value::Number(steps.into());
        }

        if let Some(strength) = extras.strength {
            body["strength"] = json!(strength);
        }

        // Style (e.g. recraft-v3: realistic_image, digital_illustration, vector_illustration)
        if let Some(style) = extras.style.as_deref() {
            body["style"] = Value::String(style.to_string());
        }

        // Reference images (URL format for Fal)
        for r in &materialized.refs {
            match &r.form {
                MaterializedRefForm::Url(img_url) => {
                    body["image_url"] = Value::String(img_url.clone());
                }
                MaterializedRefForm::Base64(b64) => {
                    body["image_url"] = Value::String(format!("data:image/png;base64,{b64}"));
                }
                _ => {}
            }
        }

        // `extra` adds the endpoint's own knobs; it cannot replace a field
        // litegen sets (rule 11), which lax mode would otherwise let it.
        if let (Some(Value::Object(extra_map)), Some(body_obj)) = (&extras.extra, body.as_object_mut()) {
            for (k, v) in extra_map {
                if !OWNED_FIELDS.contains(&k.as_str()) {
                    body_obj.insert(k.clone(), v.clone());
                }
            }
        }
        // PixelGenie L9: the asked container.
        if let Some(f) = extras.output_format.as_deref() {
            body["output_format"] = Value::String(f.to_string());
        }

        let resp_json = self.run(endpoint, &body).await?;

        // Fal returns: {images: [{url: "...", content_type: "image/jpeg", ...}]}
        let images = resp_json["images"].as_array().ok_or_else(|| {
            ProviderError::RequestFailed {
                message: "Fal response missing images array".to_string(),
                status_code: None,
                provider_error: Some(resp_json.clone()),
                retryable: false,
            }
        })?;

        let first_image = images.first().ok_or_else(|| {
            ProviderError::RequestFailed {
                message: "Fal response has empty images array".to_string(),
                status_code: None,
                provider_error: None,
                retryable: false,
            }
        })?;

        let image_url = first_image["url"].as_str().ok_or_else(|| {
            ProviderError::RequestFailed {
                message: "Fal image missing url".to_string(),
                status_code: None,
                provider_error: None,
                retryable: false,
            }
        })?;

        let content_type = first_image["content_type"]
            .as_str()
            .unwrap_or("image/jpeg")
            .to_string();

        let image_bytes = self.result_bytes(image_url).await?;

        let mut metadata = HashMap::new();
        metadata.insert("url".to_string(), Value::String(image_url.to_string()));
        metadata.insert("model".to_string(), Value::String(model.id.clone()));

        Ok(GenerationOutput {
            data: image_bytes,
            content_type,
            metadata,
            more: Vec::new(),
        })
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &ImageGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(
            model.pricing.base_cost_usd,
            0.0,
            CostSource::Estimated,
            Some(json!({ "model": model.id })),
        ))
    }

    /// Promptable segmentation on SAM 3 and SAM 3.1 (PixelGenie L7), run
    /// synchronously like `generate`. Points and boxes go as one object
    /// (`object_id` 1), boxes as corners; a text prompt asks for every
    /// instance. `sync_mode` returns masks as data URIs; one that comes as a
    /// URL is downloaded without the key.
    ///
    /// @see <https://fal.ai/models/fal-ai/sam-3/image/api> — the endpoint and its examples
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/sam-3/image> — `prompt`
    ///   (default "wheel"), `point_prompts`, `box_prompts` (`x_min`…`y_max`, "use object_id to group
    ///   boxes for the same object"), `return_multiple_masks`, `max_masks` 1–32, `apply_mask`,
    ///   `include_scores`, `sync_mode`; output `masks`, `scores`, `metadata[{index, score}]`
    async fn segment(&self, model: &ModelSchema, input: &SegmentationInput) -> Result<Vec<VendorMask>, ProviderError> {
        use base64::Engine;
        let endpoint = segment_endpoint(&model.id).ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "fal".into() })?;
        let (mut text, mut points, mut boxes) = (None, Vec::new(), Vec::new());
        for p in &input.prompts {
            match p {
                SegmentationPrompt::Point { x, y, label } => points.push(json!({ "x": x, "y": y, "label": label, "object_id": 1 })),
                SegmentationPrompt::Box { x, y, w, h } => {
                    boxes.push(json!({ "x_min": x, "y_min": y, "x_max": x + w, "y_max": y + h, "object_id": 1 }))
                }
                SegmentationPrompt::Text { text: t } => text = Some(t.clone()),
            }
        }
        let mut body = json!({
            "image_url": format!("data:{};base64,{}", input.content_type, base64::engine::general_purpose::STANDARD.encode(&input.image)),
            // The schema's default prompt is "wheel": a request without text sends none.
            "prompt": text.clone().unwrap_or_default(),
            "point_prompts": points,
            "box_prompts": boxes,
            "return_multiple_masks": text.is_some(),
            "apply_mask": false,
            "include_scores": true,
            "sync_mode": true,
            "output_format": "png",
        });
        if text.is_some() {
            body["max_masks"] = json!(input.max_masks);
        }
        let answer = self.run(endpoint, &body).await?;
        let masks = answer["masks"].as_array().ok_or_else(|| ProviderError::RequestFailed {
            message: "Fal response missing masks".into(),
            status_code: None,
            provider_error: Some(answer.clone()),
            retryable: false,
        })?;
        let mut out = Vec::with_capacity(masks.len());
        for (i, m) in masks.iter().enumerate() {
            let Some(at) = m["url"].as_str() else {
                return Err(ProviderError::RequestFailed {
                    message: format!("Fal mask {i} has no url"),
                    status_code: None,
                    provider_error: None,
                    retryable: false,
                });
            };
            let bytes = self.result_bytes(at).await?;
            let score = answer["scores"][i].as_f64().or_else(|| {
                answer["metadata"].as_array()?.iter().find(|d| d["index"].as_u64() == Some(i as u64))?["score"].as_f64()
            });
            out.push(VendorMask { bytes, score });
        }
        Ok(out)
    }

    /// Check the key against fal's Platform API (`platform_key_check`).
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Fal provider not configured".into(),
                latency_ms: None,
            };
        }
        let Ok(api_key) = self.api_key() else {
            return HealthCheckResult { healthy: false, message: "No API key".into(), latency_ms: None };
        };
        platform_key_check(&self.client, self.platform_base(), &api_key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::{method, path};

    /// L13: every catalog row has its own arm, and an id with none is refused
    /// rather than billed as another model.
    #[test]
    fn every_catalog_row_resolves_and_an_unknown_id_does_not() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        let segments = |m: &crate::capabilities::ModelSchema| m.task == crate::capabilities::ModelTask::Segment;
        for m in catalog.for_provider("fal").filter(|m| m.media_type == crate::capabilities::MediaType::Image) {
            if segments(m) {
                assert!(segment_endpoint(&m.id).is_some(), "{} has no segmentation arm", m.id);
            } else {
                assert!(FalProvider::new().resolve_endpoint(&m.id).is_some(), "{} has no arm", m.id);
            }
        }
        for (id, _) in SEGMENT_MODELS {
            assert!(catalog.get(id).is_some_and(segments), "{id} is not a catalog segmentation row");
        }
        assert!(FalProvider::new().resolve_endpoint("fal/not-a-model").is_none());
        assert!(segment_endpoint("fal/flux-dev").is_none());
    }

    // ─── Background removal (PixelGenie L8) ────────────────────────────────────

    fn init_ref(form: crate::proxy::materializer::MaterializedRefForm) -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef { role: "init".into(), form, time_s: None }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    fn matting_extras() -> ImageExtras {
        ImageExtras { size: None, steps: None, guidance_scale: None, ..make_extras() }
    }

    /// Bria's `prompt` "Change[s] what the cutout keeps … Billed at $0.03
    /// instead of $0.018", so it is never sent; nor is any generation field.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/bria/background/remove>
    #[tokio::test]
    async fn bria_rmbg_is_sent_only_the_image_and_its_cutout_comes_back() {
        use crate::proxy::materializer::MaterializedRefForm;
        let server = MockServer::start().await;
        let cutout = png_of(4, 4, |_, _| true);
        Mock::given(method("POST"))
            .and(path("/fal-ai/bria/background/remove"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "image": { "url": data_uri(&cutout), "content_type": "image/png" } })))
            .mount(&server)
            .await;
        let out = make_provider(&server.uri())
            .generate(
                &ref_schema("fal/bria-rmbg-2"),
                &make_base("", "fal/bria-rmbg-2"),
                &matting_extras(),
                &init_ref(MaterializedRefForm::Url("https://cdn.example/sprite.png".into())),
            )
            .await
            .expect("generate");
        assert_eq!(out.data, cutout);
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body, json!({ "image_url": "https://cdn.example/sprite.png", "sync_mode": true }));
    }

    /// BiRefNet takes the asked container and its own knobs from `extra`;
    /// its result can come back as a URL, fetched without the key.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/birefnet/v2>
    #[tokio::test]
    async fn birefnet_gets_the_asked_container_and_its_own_knobs() {
        use crate::proxy::materializer::MaterializedRefForm;
        let (server, cdn) = (MockServer::start().await, MockServer::start().await);
        Mock::given(method("POST"))
            .and(path("/fal-ai/birefnet/v2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "image": { "url": format!("{}/cut.webp", cdn.uri()) }, "mask_image": null })))
            .mount(&server)
            .await;
        Mock::given(method("GET")).and(path("/cut.webp")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"WEBPCUT".to_vec())).mount(&cdn).await;
        let extras = ImageExtras {
            output_format: Some("webp".into()),
            extra: Some(json!({ "operating_resolution": "2048x2048" })),
            ..matting_extras()
        };
        let out = make_provider(&server.uri())
            .generate(&ref_schema("fal/birefnet-v2"), &make_base("", "fal/birefnet-v2"), &extras, &init_ref(MaterializedRefForm::Base64("aGk=".into())))
            .await
            .expect("generate");
        assert_eq!(out.data, b"WEBPCUT");
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(
            body,
            json!({ "image_url": "data:image/png;base64,aGk=", "sync_mode": true, "output_format": "webp", "operating_resolution": "2048x2048" })
        );
        assert!(cdn.received_requests().await.unwrap()[0].headers.get("authorization").is_none(), "no key to the CDN");
    }

    // ─── Segmentation (PixelGenie L7) ─────────────────────────────────────────

    fn png_of(w: u32, h: u32, white: impl Fn(u32, u32) -> bool) -> Vec<u8> {
        let img = image::GrayImage::from_fn(w, h, |x, y| image::Luma([if white(x, y) { 255 } else { 0 }]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    fn data_uri(bytes: &[u8]) -> String {
        use base64::Engine;
        format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    fn seg_input(prompts: Value) -> SegmentationInput {
        SegmentationInput {
            image: png_of(32, 32, |_, _| false).into(),
            content_type: "image/png".into(),
            width: 32,
            height: 32,
            prompts: serde_json::from_value(prompts).unwrap(),
            max_masks: 32,
        }
    }

    /// Points and boxes select one object: SAM 3 groups prompts by
    /// `object_id`, and takes boxes as `x_min, y_min, x_max, y_max`. Its
    /// `prompt` defaults to "wheel", so a request without text sends "".
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/sam-3/image>
    #[tokio::test]
    async fn sam_3_gets_points_and_boxes_as_one_object_and_returns_its_masks() {
        let server = MockServer::start().await;
        let mask = png_of(32, 32, |x, _| x < 4);
        Mock::given(method("POST"))
            .and(path("/fal-ai/sam-3/image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "masks": [{ "url": data_uri(&mask), "content_type": "image/png" }],
                "scores": [0.93],
            })))
            .mount(&server)
            .await;
        let input = seg_input(json!([
            { "type": "point", "x": 5, "y": 6 },
            { "type": "point", "x": 7, "y": 8, "label": 0 },
            { "type": "box", "x": 1, "y": 2, "w": 10, "h": 20 },
        ]));
        let masks = make_provider(&server.uri()).segment(&ref_schema("fal/sam-3"), &input).await.expect("segment");
        assert_eq!(masks.len(), 1);
        assert_eq!((masks[0].bytes.clone(), masks[0].score), (mask, Some(0.93)));

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].headers.get("authorization").unwrap(), "Key test-key");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert!(body["image_url"].as_str().unwrap().starts_with("data:image/png;base64,"), "{body}");
        assert_eq!(body["prompt"], "");
        assert_eq!(
            body["point_prompts"],
            json!([{ "x": 5, "y": 6, "label": 1, "object_id": 1 }, { "x": 7, "y": 8, "label": 0, "object_id": 1 }])
        );
        assert_eq!(body["box_prompts"], json!([{ "x_min": 1, "y_min": 2, "x_max": 11, "y_max": 22, "object_id": 1 }]));
        for (field, want) in [
            ("return_multiple_masks", json!(false)),
            ("apply_mask", json!(false)),
            ("sync_mode", json!(true)),
            ("include_scores", json!(true)),
            ("output_format", json!("png")),
        ] {
            assert_eq!(body[field], want, "{field}: {body}");
        }
    }

    /// A text prompt asks for every instance, at most `max_masks` of them
    /// (SAM 3 takes 1–32); masks may come back as URLs, and scores in
    /// `metadata` when `scores` is absent.
    #[tokio::test]
    async fn a_text_prompt_asks_sam_3_for_every_instance() {
        let server = MockServer::start().await;
        let cdn = MockServer::start().await;
        let (first, second) = (png_of(32, 32, |x, _| x < 4), png_of(32, 32, |x, _| x > 20));
        Mock::given(method("POST"))
            .and(path("/fal-ai/sam-3/image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "masks": [{ "url": format!("{}/m0.png", cdn.uri()) }, { "url": data_uri(&second) }],
                "metadata": [{ "index": 0, "score": 0.8 }, { "index": 1, "score": 0.6 }],
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET")).and(path("/m0.png")).respond_with(ResponseTemplate::new(200).set_body_bytes(first.clone())).mount(&cdn).await;
        let mut input = seg_input(json!([{ "type": "text", "text": "visor" }]));
        input.max_masks = 5;
        let masks = make_provider(&server.uri()).segment(&ref_schema("fal/sam-3"), &input).await.expect("segment");
        assert_eq!(masks.iter().map(|m| (m.bytes.clone(), m.score)).collect::<Vec<_>>(), vec![(first, Some(0.8)), (second, Some(0.6))]);
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!((body["prompt"].clone(), body["return_multiple_masks"].clone(), body["max_masks"].clone()), (json!("visor"), json!(true), json!(5)));
        assert_eq!((body["point_prompts"].clone(), body["box_prompts"].clone()), (json!([]), json!([])));
        assert!(cdn.received_requests().await.unwrap()[0].headers.get("authorization").is_none(), "no key to the CDN");
    }

    #[tokio::test]
    async fn sam_3_refusing_the_request_is_the_vendors_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/fal-ai/sam-3-1/image"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({ "detail": [{ "msg": "image_url could not be read" }] })))
            .mount(&server)
            .await;
        let err = make_provider(&server.uri())
            .segment(&ref_schema("fal/sam-3.1"), &seg_input(json!([{ "type": "box", "x": 0, "y": 0, "w": 4, "h": 4 }])))
            .await
            .unwrap_err();
        match err {
            ProviderError::RequestFailed { status_code: Some(422), retryable: false, provider_error: Some(body), .. } => {
                assert!(body.to_string().contains("could not be read"), "{body}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[cfg(test)]
    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    #[cfg(test)]
    fn empty_materialized() -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: vec![],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    /// A provider against a mock fal. Its result URLs are on loopback, so the
    /// public-address check is off — `a_result_is_fetched_only_from_a_public_address`
    /// covers it.
    fn make_provider(api_base: &str) -> FalProvider {
        let mut p = FalProvider::new();
        p.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: "test-key".to_string(),
            api_keys: vec![],
            api_base: Some(api_base.to_string()),
            model_mapping: Default::default(),
            extra_headers: Default::default(),
            options: None,
        });
        p.public_assets_only = false;
        p
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(),
            model: model.to_string(),
            n: 1,
            negative_prompt: None,
            seed: Some(42),
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        }
    }

    fn make_extras() -> ImageExtras {
        ImageExtras {
            vendor_job: Default::default(),
            size: Some("1024x1024".to_string()),
            aspect_ratio: None,
            quality: None,
            style: None,
            steps: Some(28),
            guidance_scale: Some(3.5),
            strength: None,
            response_format: "url".to_string(),
            edit: None, views: Vec::new(),
            extra: None,
            output_format: None, background: None,
            expand: None,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        }
    }

    #[tokio::test]
    async fn generates_flux_dev_image() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;

        let image_url = format!("{}/output.png", image_server.uri());

        // Mock POST /fal-ai/flux/dev
        Mock::given(method("POST"))
            .and(path("/fal-ai/flux/dev"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "images": [{
                    "url": image_url,
                    "content_type": "image/jpeg",
                    "width": 1024,
                    "height": 1024
                }],
                "timings": {},
                "seed": 42
            })))
            .mount(&server)
            .await;

        // Mock GET to image URL
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"FAKEJPEGDATA".to_vec())
                    .insert_header("content-type", "image/jpeg"),
            )
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("fal/flux-dev");
        let base = make_base("a neon city at night", "fal/flux-dev");
        let mut extras = make_extras();
        extras.output_format = Some("png".into());
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let output = result.unwrap();
        assert_eq!(output.data, b"FAKEJPEGDATA");

        // Verify outbound request body
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();

        assert_eq!(body["prompt"], "a neon city at night");
        assert_eq!(body["image_size"]["width"], 1024);
        assert_eq!(body["image_size"]["height"], 1024);
        assert_eq!(body["seed"], 42);
        assert_eq!(body["guidance_scale"], 3.5);
        assert_eq!(body["num_inference_steps"], 28);
        assert_eq!(body["output_format"], "png", "PixelGenie L9: the asked container");
    }

    /// fal renamed `fal-ai/recraft-v3` to `fal-ai/recraft/v3/text-to-image`:
    /// <https://fal.ai/models/fal-ai/recraft-v3> 308-redirects there, and the
    /// model registry (`api.fal.ai/v1/models`) returns the old id without
    /// metadata. Both ids publish the same input schema (`prompt` of at most
    /// 1000 chars, `style`, `image_size`, `colors`, `style_id`), so the catalog
    /// id `fal/recraft-v3` keeps working by moving only the endpoint path.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft/v3/text-to-image>
    #[tokio::test]
    async fn recraft_v3_posts_to_the_renamed_endpoint() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/output.webp", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/fal-ai/recraft/v3/text-to-image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "images": [{ "url": image_url, "content_type": "image/webp" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"FAKEWEBP".to_vec()))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("fal/recraft-v3");
        let mut base = make_base("a minimalist fox logo", "fal/recraft-v3");
        base.seed = None;
        // Only what the catalog lets through for this model: prompt + style.
        let extras = ImageExtras {
            vendor_job: Default::default(),
            size: None,
            steps: None,
            guidance_scale: None,
            style: Some("vector_illustration".to_string()),
            ..make_extras()
        };

        let output = provider
            .generate(&schema, &base, &extras, &empty_materialized())
            .await
            .expect("generate");
        assert_eq!(output.data, b"FAKEWEBP");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].url.path(), "/fal-ai/recraft/v3/text-to-image");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["prompt"], "a minimalist fox logo");
        assert_eq!(body["style"], "vector_illustration");
    }

    /// `fal/flux-2` is FLUX.2 [dev] at `fal-ai/flux-2`. Unknown ids fall back
    /// to `fal-ai/flux/dev`, so without its own arm the request silently ran
    /// on FLUX.1 [dev]. Its input takes `guidance_scale`, `num_inference_steps`
    /// and `image_size` like the other FLUX endpoints.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2>
    #[tokio::test]
    async fn flux_2_posts_to_the_flux_2_endpoint() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/output.png", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/fal-ai/flux-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "images": [{ "url": image_url, "content_type": "image/png" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"FAKEPNG".to_vec()))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("fal/flux-2");
        let base = make_base("a neon city at night", "fal/flux-2");
        let output = provider
            .generate(&schema, &base, &make_extras(), &empty_materialized())
            .await
            .expect("generate");
        assert_eq!(output.data, b"FAKEPNG");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].url.path(), "/fal-ai/flux-2");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["guidance_scale"], 3.5);
        assert_eq!(body["num_inference_steps"], 28);
        assert_eq!(body["image_size"]["width"], 1024);
    }

    // ─── fal audit, 2026-10-30: F04-F09, F11 ────────────────────────────────

    fn configured(api_base: Option<&str>, options: Option<Value>) -> FalProvider {
        let mut p = FalProvider::new();
        p.configure(ProviderInstanceConfig { api_key: "test-key".into(), api_base: api_base.map(str::to_string), options, ..Default::default() });
        p
    }

    /// F09: `providers.fal.api_base` is fal's QUEUE host for video and 3D;
    /// images run on its sync host, which is not that one.
    #[test]
    fn images_run_on_fals_sync_host_beside_its_queue_host() {
        assert_eq!(configured(None, None).sync_base(), "https://fal.run");
        assert_eq!(configured(Some("https://queue.fal.run"), None).sync_base(), "https://fal.run");
        assert_eq!(configured(Some("https://fal-proxy.example/fal"), None).sync_base(), "https://fal-proxy.example/fal");
        let split = configured(Some("https://queue.fal.run"), Some(json!({ "sync_api_base": "https://sync.example/" })));
        assert_eq!(split.sync_base(), "https://sync.example");
    }

    async fn image_answer(server: &MockServer, endpoint: &str, images: &MockServer) {
        Mock::given(method("POST"))
            .and(path(format!("/{endpoint}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "images": [{ "url": format!("{}/out.png", images.uri()), "content_type": "image/png" }] })))
            .mount(server)
            .await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"PNG".to_vec())).mount(images).await;
    }

    /// F05, F06: what litegen sets, `extra` cannot replace, and `num_images`
    /// is never sent — fal would bill every image it made and litegen keeps
    /// one, and Recraft V3 has no such field.
    #[tokio::test]
    async fn extra_cannot_replace_what_litegen_sends_and_num_images_is_never_sent() {
        let (server, images) = (MockServer::start().await, MockServer::start().await);
        image_answer(&server, "fal-ai/flux/dev", &images).await;
        let mut base = make_base("a lighthouse", "fal/flux-dev");
        base.n = 2;
        let mut extras = make_extras();
        extras.extra = Some(json!({ "prompt": "something else", "num_images": 4, "sync_mode": true, "seed": 7, "image_size": "square", "enable_safety_checker": false }));
        make_provider(&server.uri()).generate(&ref_schema("fal/flux-dev"), &base, &extras, &empty_materialized()).await.unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!((body["prompt"].as_str(), body["seed"].as_i64()), (Some("a lighthouse"), Some(42)), "{body}");
        assert_eq!(body["image_size"], json!({ "width": 1024, "height": 1024 }), "{body}");
        assert!(body.get("num_images").is_none() && body.get("sync_mode").is_none(), "{body}");
        assert_eq!(body["enable_safety_checker"], false, "a knob litegen does not own still passes: {body}");
    }

    /// F04: once fal answered 2xx it has run, and billed, the job — a result
    /// that will not download is never worth a second, billed run.
    #[tokio::test]
    async fn a_result_that_will_not_download_after_fal_ran_is_never_run_again() {
        let (server, images) = (MockServer::start().await, MockServer::start().await);
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(503)).mount(&images).await;
        // A host that answers 503, and one that refuses the connection.
        for url in [format!("{}/out.png", images.uri()), "http://127.0.0.1:1/out.png".to_string()] {
            server.reset().await;
            Mock::given(method("POST"))
                .and(path("/fal-ai/flux/dev"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "images": [{ "url": url, "content_type": "image/png" }] })))
                .mount(&server)
                .await;
            let err = make_provider(&server.uri())
                .generate(&ref_schema("fal/flux-dev"), &make_base("x", "fal/flux-dev"), &make_extras(), &empty_materialized())
                .await
                .unwrap_err();
            assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{url}: {err:?}");
        }
    }

    /// F08: fal's model errors are an array of `{loc, msg}`, a gateway's page
    /// is not JSON at all, and `X-Fal-Needs-Retry` says whether to ask again.
    #[tokio::test]
    async fn fals_errors_are_read_whole_whatever_their_shape() {
        for (answer, says, retryable) in [
            (ResponseTemplate::new(422).set_body_json(json!({ "detail": [{ "loc": ["body", "image_size"], "msg": "unsupported size", "type": "value_error" }] })), "body.image_size: unsupported size", false),
            (ResponseTemplate::new(502).set_body_string("<html>Bad Gateway</html>"), "Bad Gateway", true),
            (ResponseTemplate::new(429).set_body_json(json!({ "detail": "Too many concurrent requests", "error_type": "rate_limited" })), "Too many concurrent requests", true),
            (ResponseTemplate::new(500).insert_header("x-fal-needs-retry", "0").set_body_json(json!({ "detail": "the model failed" })), "the model failed", false),
            (ResponseTemplate::new(400).insert_header("x-fal-needs-retry", "1").set_body_json(json!({ "detail": "try again" })), "try again", true),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).and(path("/fal-ai/flux/dev")).respond_with(answer).mount(&server).await;
            let err = make_provider(&server.uri())
                .generate(&ref_schema("fal/flux-dev"), &make_base("x", "fal/flux-dev"), &make_extras(), &empty_materialized())
                .await
                .unwrap_err();
            match &err {
                ProviderError::RequestFailed { message, retryable: r, .. } => {
                    assert!(message.contains(says), "{says}: {message}");
                    assert_eq!(*r, retryable, "{says}: {err:?}");
                }
                other => panic!("{other:?}"),
            }
        }
    }

    /// F11: a result URL is fetched only from a public address, and never
    /// past the size limit.
    #[tokio::test]
    async fn a_result_is_fetched_only_from_a_public_address() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/fal-ai/flux/dev"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "images": [{ "url": "http://169.254.169.254/latest/meta-data", "content_type": "image/png" }] })))
            .mount(&server)
            .await;
        let mut p = make_provider(&server.uri());
        p.public_assets_only = true;
        let err = p
            .generate(&ref_schema("fal/flux-dev"), &make_base("x", "fal/flux-dev"), &make_extras(), &empty_materialized())
            .await
            .unwrap_err();
        assert!(matches!(&err, ProviderError::RequestFailed { message, retryable: false, .. } if message.contains("refused")), "{err:?}");
    }

    /// F07: the key is checked against fal's Platform API; the old
    /// `rest.alpha.fal.ai/tokens/current` route is gone (404 for every key).
    #[tokio::test]
    async fn health_check_validates_the_key_against_the_platform_api() {
        use wiremock::matchers::{header, query_param};
        for (status, healthy) in [(200, true), (401, false)] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/models/pricing"))
                .and(query_param("endpoint_id", "fal-ai/trellis"))
                .and(header("authorization", "Key test-key"))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!({})))
                .expect(1)
                .mount(&server)
                .await;
            let p = configured(None, Some(json!({ "platform_api_base": server.uri() })));
            assert_eq!(p.health_check().await.healthy, healthy, "{status}");
        }
    }
}
