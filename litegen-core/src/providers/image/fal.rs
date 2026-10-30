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
use crate::types::*;

/// Fal.ai image generation provider.
///
/// Routes to model-specific Fal endpoints for Flux, SDXL, SD3.5, Recraft, etc.
/// Sync API: `POST https://fal.run/{endpoint}` → `{images: [{url, ...}]}`
///
/// @see <https://docs.fal.ai/model-apis/model-endpoints> — synchronous model endpoints (`fal.run`)
/// @see <https://fal.ai/models> — per-model API pages documenting each endpoint's input schema
pub struct FalProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
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

    /// The base URL (defaults to https://fal.run, overridable for tests).
    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://fal.run")
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

    /// Download the rendered image from the `images[].url` returned by the sync
    /// `fal.run` call (a plain HTTPS GET of the Fal-hosted asset, not an endpoint).
    ///
    /// @see <https://docs.fal.ai/model-apis/model-endpoints> — response `images[].url` field
    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let resp = self.client.get(url).send().await.map_err(|e| {
            ProviderError::RequestFailed {
                message: format!("Failed to fetch Fal image: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: true,
            }
        })?;
        if !resp.status().is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Fal image URL returned HTTP {}", resp.status()),
                status_code: Some(resp.status().as_u16()),
                provider_error: None,
                retryable: false,
            });
        }
        let bytes = resp.bytes().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to read Fal image bytes: {e}"),
            status_code: None,
            provider_error: None,
            retryable: false,
        })?;
        Ok(bytes.to_vec())
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

    /// POST `body` to `fal.run/{endpoint}` and read the JSON answer; a vendor
    /// error carries fal's own `detail` (a 422's is a list of `{msg}`).
    async fn run(&self, endpoint: &str, body: &Value) -> Result<Value, ProviderError> {
        let api_key = self.api_key()?;
        let url = format!("{}/{}", self.api_base(), endpoint);
        let resp = crate::providers::inject_trace_headers(self.client.post(&url).header("Authorization", format!("Key {api_key}")).json(body))
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Fal request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?;
        let status = resp.status();
        let answer: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Fal response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        if !status.is_success() {
            let detail = answer["detail"]
                .as_str()
                .map(str::to_string)
                .or_else(|| answer["detail"][0]["msg"].as_str().map(str::to_string))
                .or_else(|| answer["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| "Unknown error".into());
            return Err(ProviderError::RequestFailed {
                message: format!("Fal API error: {detail}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(answer),
                retryable: status.as_u16() >= 500,
            });
        }
        Ok(answer)
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
                obj.insert(k.clone(), v.clone());
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
    /// @see <https://docs.fal.ai/model-apis/model-endpoints> — synchronous run; `Authorization: Key <token>`,
    ///   JSON body, and the `{images: [{url, content_type}]}` response shape this method parses.
    /// @see <https://fal.ai/models> — per-model pages proving the `image_size`, `num_images`, `seed`,
    ///   `guidance_scale`, `num_inference_steps`, `strength`, and `image_url` input fields.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let api_key = self.api_key()?;
        let endpoint = self.resolve_endpoint(&model.id).ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "fal".into() })?;
        if model.capabilities.background_removal {
            return self.remove_background(endpoint, extras, materialized).await;
        }
        let url = format!("{}/{}", self.api_base(), endpoint);

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

        // Number of images
        body["num_images"] = Value::Number(base.n.max(1).into());

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

        // Shallow-merge extra
        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(body_obj) = body.as_object_mut() {
                for (k, v) in extra_map {
                    body_obj.insert(k.clone(), v.clone());
                }
            }
        }
        // PixelGenie L9: the asked container, after `extra` so it wins.
        if let Some(f) = extras.output_format.as_deref() {
            body["output_format"] = Value::String(f.to_string());
        }

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
            message: format!("Fal request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_timeout() || e.is_connect(),
        })?;

        let status = resp.status();
        let resp_json: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Fal response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        if !status.is_success() {
            let err = resp_json["detail"]
                .as_str()
                .or_else(|| resp_json["message"].as_str())
                .unwrap_or("Unknown error")
                .to_string();
            return Err(ProviderError::RequestFailed {
                message: format!("Fal API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(resp_json),
                retryable: status.as_u16() >= 500,
            });
        }

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

        let image_bytes = self.fetch_image_bytes(image_url).await?;

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

    /// Validate the API key via `GET https://rest.alpha.fal.ai/tokens/current`
    /// (Fal's token-introspection endpoint), authenticated with `Authorization: Key`.
    ///
    /// @see <https://docs.fal.ai/authentication/key-based> — key-based authentication
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Fal provider not configured".into(),
                latency_ms: None,
            };
        }
        let start = std::time::Instant::now();
        let api_key = match self.api_key() {
            Ok(k) => k,
            Err(_) => {
                return HealthCheckResult {
                    healthy: false,
                    message: "No API key".into(),
                    latency_ms: None,
                }
            }
        };
        match self
            .client
            .get("https://rest.alpha.fal.ai/tokens/current")
            .header("Authorization", format!("Key {api_key}"))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "Fal API key valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("Fal returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("Fal health check failed: {e}"),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
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
}
