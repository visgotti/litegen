use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRequest, MaterializedRefForm};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// Stability AI image generation provider.
///
/// Supports SD3, SD3-Turbo, Core, Ultra, SDXL, and SD 1.6 via the Stability Platform API.
/// V1 engines (SDXL, SD1.6): JSON body → base64 response
/// V2 endpoints (SD3, Core, Ultra): multipart form → raw image bytes
///
/// @see <https://platform.stability.ai/docs/api-reference#tag/Generate> — V2beta Stable Image generate (SD3/Core/Ultra)
/// @see <https://platform.stability.ai/docs/api-reference> — V1 engine `POST /v1/generation/{engine}/text-to-image`
pub struct StabilityProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
}

impl StabilityProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.stability.ai")
    }

    fn api_key(&self) -> Result<String, ProviderError> {
        if let Some(pool) = &self.key_pool {
            return Ok(pool.next().to_string());
        }
        self.config
            .as_ref()
            .map(|c| c.api_key.clone())
            .ok_or_else(|| ProviderError::NotConfigured("stability".into()))
    }

    /// Strip provider prefix to get the native model ID.
    fn native_model_id(model_id: &str) -> &str {
        if let Some(rest) = model_id.strip_prefix("stability/") {
            rest
        } else {
            model_id
        }
    }

    fn resolve_model(&self, model_id: &str) -> String {
        if let Some(cfg) = &self.config {
            if let Some(mapped) = cfg.model_mapping.get(model_id) {
                return mapped.clone();
            }
        }
        let native = Self::native_model_id(model_id);
        // Map canonical names to Stability API model names
        // The /v2beta/stable-image/generate/sd3 `model` enum is exactly
        // sd3.5-large | sd3.5-large-turbo | sd3.5-medium. The bare sd3-*
        // values are no longer members.
        // @see <https://api.stability.ai/v2alpha/openapi>
        match native {
            "sd3-large" | "sd3.5-large" => "sd3.5-large".to_string(),
            "sd3-turbo" | "sd3.5-large-turbo" => "sd3.5-large-turbo".to_string(),
            "sd3-medium" | "sd3.5-medium" => "sd3.5-medium".to_string(),
            "core" => "core".to_string(),
            "ultra" => "ultra".to_string(),
            "inpaint" => "inpaint".to_string(),
            "remove-background" => "remove-background".to_string(),
            "outpaint" => "outpaint".to_string(),
            "search-and-replace" => "search-and-replace".to_string(),
            "sdxl" => "stable-diffusion-xl-1024-v1-0".to_string(),
            "sd-1.6" | "sd1.6" => "stable-diffusion-v1-6".to_string(),
            _ => native.to_string(),
        }
    }

    /// Determine which V2 route to use for a given model, under
    /// `/v2beta/stable-image/`.
    ///
    /// @see <https://platform.stability.ai/docs/api-reference#tag/Generate> — V2beta Stable Image generate routes
    /// @see <https://platform.stability.ai/docs/api-reference#tag/Edit> — the edit routes (`edit/inpaint`,
    ///   `edit/remove-background`, `edit/outpaint`, `edit/search-and-replace`)
    fn v2_endpoint(model: &str) -> Option<&'static str> {
        match model {
            "sd3.5-large" | "sd3.5-large-turbo" | "sd3.5-medium" => Some("generate/sd3"),
            "core" => Some("generate/core"),
            "ultra" => Some("generate/ultra"),
            "inpaint" => Some("edit/inpaint"),
            "remove-background" => Some("edit/remove-background"),
            "outpaint" => Some("edit/outpaint"),
            "search-and-replace" => Some("edit/search-and-replace"),
            _ => None,
        }
    }

    fn is_v2(model: &str) -> bool {
        Self::v2_endpoint(model).is_some()
    }

    /// Map pixel dimensions to Stability V2 aspect ratios.
    fn aspect_ratio_from_size(size: &str) -> Option<&'static str> {
        let parts: Vec<&str> = size.split('x').collect();
        if parts.len() != 2 {
            return None;
        }
        let w: f64 = parts[0].parse().ok()?;
        let h: f64 = parts[1].parse().ok()?;
        if h == 0.0 {
            return None;
        }
        let ratio = w / h;
        // The v2 `aspect_ratio` enum is exactly
        // 21:9 | 16:9 | 3:2 | 5:4 | 1:1 | 4:5 | 2:3 | 9:16 | 9:21.
        // `4:3` and `3:4` are NOT members; the nearest legal neighbours are
        // `5:4` and `4:5`.
        Some(if ratio >= 2.1 {
            "21:9"
        } else if ratio >= 1.6 {
            "16:9"
        } else if ratio >= 1.35 {
            "3:2"
        } else if ratio >= 1.12 {
            "5:4"
        } else if ratio >= 0.9 {
            "1:1"
        } else if ratio >= 0.74 {
            "4:5"
        } else if ratio >= 0.58 {
            "2:3"
        } else if ratio >= 0.48 {
            "9:16"
        } else {
            "9:21"
        })
    }
}

impl Default for StabilityProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for StabilityProvider {
    fn name(&self) -> &str {
        "stability"
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

    /// Generate an image. V2 models (SD3/Core/Ultra) POST a multipart form to
    /// `/v2beta/stable-image/generate/{endpoint}`; V1 engines (SDXL, SD 1.6)
    /// POST JSON to `/v1/generation/{engine}/text-to-image`.
    ///
    /// @see <https://platform.stability.ai/docs/api-reference#tag/Generate> — V2 generate; proves the multipart
    ///   fields (`prompt`, `model`, `output_format`, `aspect_ratio`, `seed`, `negative_prompt`, `strength`)
    ///   and the raw-image-bytes (`Accept: image/*`) response.
    /// @see <https://platform.stability.ai/docs/api-reference> — V1 `text-to-image`; proves `text_prompts[]`
    ///   (with `weight`), `cfg_scale`, `steps`, `samples`, `width`/`height`, and the `artifacts[].base64` response.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let api_key = self.api_key()?;
        let model_name = self.resolve_model(&model.id);

        if Self::is_v2(&model_name) {
            // V2 multipart form API
            let route = Self::v2_endpoint(&model_name).unwrap();
            let url = format!("{}/v2beta/stable-image/{route}", self.api_base());
            let endpoint = route.rsplit('/').next().unwrap_or(route);
            let inpaint = route == "edit/inpaint";
            // Remove Background (PixelGenie L8) takes only `image` and
            // `output_format`.
            let matting = route == "edit/remove-background";
            // Outpaint (3D doc §4) grows the canvas by `left`, `up`, `right`
            // and `down`; its prompt is optional.
            let outpaint = route == "edit/outpaint";

            let mut form = reqwest::multipart::Form::new();
            if !matting && !(outpaint && base.prompt.trim().is_empty()) {
                form = form.text("prompt", base.prompt.clone());
            }
            // Search and Replace (3D doc §4) finds the area `mask_prompt` names;
            // `grow_mask` goes as 0, as for inpaint (its default 3 blurs it).
            let search = route == "edit/search-and-replace";
            if search {
                form = form.text("search_prompt", extras.mask_prompt.clone().unwrap_or_default()).text("grow_mask", "0");
            }
            if let (true, Some(e)) = (outpaint, extras.expand) {
                form = form
                    .text("left", e.left.to_string())
                    .text("up", e.top.to_string())
                    .text("right", e.right.to_string())
                    .text("down", e.bottom.to_string());
            }

            // Only /generate/sd3 has a `model` property — the core and ultra
            // request schemas do not define one; the route selects the model.
            if endpoint == "sd3" {
                form = form.text("model", model_name.clone());
            }

            if inpaint {
                // Inpaint edits where its mask is white; without one it would
                // read the image's alpha, which litegen never relies on.
                let has_mask = materialized.refs.iter().any(|r| {
                    matches!(&r.form, MaterializedRefForm::MultipartField { field_name, .. } if field_name == "mask")
                });
                if !has_mask {
                    return Err(ProviderError::InvalidRequest(format!(
                        "'{}' edits a masked area: send a mask or a region", model.id
                    )));
                }
                // Explicit, even 0: Stability's default grows the mask by 5.
                // @see <https://api.stability.ai/v2alpha/openapi> — grow_mask "default": 5
                let grow = extras.edit.as_ref().and_then(|e| e.vendor_grow_px).unwrap_or(0);
                form = form.text("grow_mask", grow.to_string());
            }

            // The asked container (PixelGenie L9); png when none is asked.
            form = form.text("output_format", extras.output_format.clone().unwrap_or_else(|| "png".to_string()));

            // sd3 gates image-to-image behind `mode`; without it the uploaded
            // `image` part is ignored and the request runs as text-to-image.
            let has_image_ref = materialized.refs.iter().any(|r| {
                matches!(&r.form, MaterializedRefForm::MultipartField { field_name, .. }
                    if field_name == "image")
            });
            if endpoint == "sd3" {
                form = form.text(
                    "mode",
                    if has_image_ref { "image-to-image" } else { "text-to-image" },
                );
            }

            // `style` is validated against the API's style_preset values.
            if let Some(style) = extras.style.as_deref() {
                form = form.text("style_preset", style.to_string());
            }

            // Aspect ratio: prefer direct aspect_ratio field, fallback to size
            // derivation. Skipped in image-to-image mode — the spec says
            // "This parameter is only valid for text-to-image requests";
            // there the output takes its shape from the input image.
            if !has_image_ref && !inpaint {
                if let Some(ar) = extras.aspect_ratio.as_deref() {
                    form = form.text("aspect_ratio", ar.to_string());
                } else if let Some(size) = extras.size.as_deref() {
                    if let Some(ar) = Self::aspect_ratio_from_size(size) {
                        form = form.text("aspect_ratio", ar.to_string());
                    }
                }
            }

            if let Some(seed) = base.seed {
                form = form.text("seed", seed.to_string());
            }

            if let Some(np) = base.negative_prompt.as_deref() {
                form = form.text("negative_prompt", np.to_string());
            }

            // `strength` is *required* for image-to-image ("image-to-image
            // requires the prompt, image, and strength parameters") and is
            // "only valid for image-to-image requests" — so default it when a
            // reference image is present, and never send it otherwise.
            if has_image_ref && !inpaint && !matting && !outpaint && !search {
                form = form.text("strength", format!("{:.2}", extras.strength.unwrap_or(0.5)));
            }

            // extra fields shallow-merged (allowlist enforced by validator)
            if let Some(Value::Object(extra_map)) = &extras.extra {
                for (k, v) in extra_map {
                    let val_str = match v {
                        Value::String(s) => s.clone(),
                        _ => v.to_string(),
                    };
                    form = form.text(k.clone(), val_str);
                }
            }

            // Attach any multipart ref images
            for r in &materialized.refs {
                if let MaterializedRefForm::MultipartField { field_name, bytes, content_type } = &r.form {
                    let part = reqwest::multipart::Part::bytes(bytes.to_vec())
                        .mime_str(content_type)
                        .unwrap_or_else(|_| reqwest::multipart::Part::bytes(bytes.to_vec()))
                        .file_name("image.png");
                    form = form.part(field_name.clone(), part);
                }
            }

            let resp = crate::providers::inject_trace_headers(
                self.client
                    .post(&url)
                    .header("Authorization", format!("Bearer {api_key}"))
                    .header("Accept", "image/*")
                    .multipart(form),
            )
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Stability V2 request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?;

            let status = resp.status();
            if !status.is_success() {
                let error_body = resp.text().await.unwrap_or_default();
                return Err(ProviderError::RequestFailed {
                    message: format!("Stability API error: {error_body}"),
                    status_code: Some(status.as_u16()),
                    provider_error: None,
                    retryable: status.as_u16() >= 500,
                });
            }

            let content_type = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("image/png")
                .to_string();

            let bytes = resp.bytes().await.map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to read Stability response: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;

            let mut metadata = HashMap::new();
            metadata.insert("model".to_string(), Value::String(model_name));

            Ok(GenerationOutput {
                data: bytes.to_vec(),
                content_type,
                metadata,
                more: Vec::new(),
            })
        } else {
            // V1 engine API. Plain text-to-image POSTs a JSON body. When init/mask
            // refs are present (SDXL image_to_image / inpainting), Stability requires
            // a *separate* multipart endpoint:
            //   /v1/generation/{engine}/image-to-image          (init_image present)
            //   /v1/generation/{engine}/image-to-image/masking  (mask_image present)
            // All three return the same JSON `artifacts[].base64` payload.
            //
            // @see <https://platform.stability.ai/docs/api-reference> — V1 `image-to-image`
            //   (multipart `init_image`, `image_strength`, `text_prompts[*][text|weight]`) and
            //   `image-to-image/masking` (multipart `init_image`, `mask_image`, `mask_source`).
            let mut has_init = false;
            let mut has_mask = false;
            for r in &materialized.refs {
                if let MaterializedRefForm::MultipartField { field_name, .. } = &r.form {
                    match field_name.as_str() {
                        "init_image" => has_init = true,
                        "mask_image" => has_mask = true,
                        _ => {}
                    }
                }
            }
            let use_multipart = has_init || has_mask;

            let resp = if use_multipart {
                // Multipart image-to-image / masking path.
                let route = if has_mask {
                    "image-to-image/masking"
                } else {
                    "image-to-image"
                };
                let url = format!(
                    "{}/v1/generation/{}/{}",
                    self.api_base(),
                    model_name,
                    route
                );

                let mut form = reqwest::multipart::Form::new()
                    .text("text_prompts[0][text]", base.prompt.clone())
                    .text("text_prompts[0][weight]", "1.0")
                    .text("samples", base.n.max(1).to_string());

                if let Some(np) = base.negative_prompt.as_deref() {
                    form = form
                        .text("text_prompts[1][text]", np.to_string())
                        .text("text_prompts[1][weight]", "-1.0");
                }

                if let Some(steps) = extras.steps {
                    form = form.text("steps", steps.to_string());
                }

                if let Some(gs) = extras.guidance_scale {
                    form = form.text("cfg_scale", gs.to_string());
                }

                if let Some(seed) = base.seed {
                    form = form.text("seed", seed.to_string());
                }

                // init_strength controls how much the init image is preserved.
                if let Some(strength) = extras.strength {
                    form = form.text("image_strength", format!("{:.2}", strength));
                }

                // Masking requires telling the API how the mask is encoded.
                if has_mask {
                    form = form.text("mask_source", "MASK_IMAGE_WHITE");
                }

                // extra fields shallow-merged (allowlist enforced by validator).
                if let Some(Value::Object(extra_map)) = &extras.extra {
                    for (k, v) in extra_map {
                        let val_str = match v {
                            Value::String(s) => s.clone(),
                            _ => v.to_string(),
                        };
                        form = form.text(k.clone(), val_str);
                    }
                }

                // Attach materialized init_image / mask_image ref bytes.
                for r in &materialized.refs {
                    if let MaterializedRefForm::MultipartField { field_name, bytes, content_type } = &r.form {
                        let part = reqwest::multipart::Part::bytes(bytes.to_vec())
                            .mime_str(content_type)
                            .unwrap_or_else(|_| reqwest::multipart::Part::bytes(bytes.to_vec()))
                            .file_name("image.png");
                        form = form.part(field_name.clone(), part);
                    }
                }

                crate::providers::inject_trace_headers(
                    self.client
                        .post(&url)
                        .header("Authorization", format!("Bearer {api_key}"))
                        .header("Accept", "application/json")
                        .multipart(form),
                )
                .send()
                .await
                .map_err(|e| ProviderError::RequestFailed {
                    message: format!("Stability V1 request failed: {e}"),
                    status_code: e.status().map(|s| s.as_u16()),
                    provider_error: None,
                    retryable: e.is_timeout() || e.is_connect(),
                })?
            } else {
                // Plain text-to-image — JSON body, returns base64.
                let url = format!(
                    "{}/v1/generation/{}/text-to-image",
                    self.api_base(),
                    model_name
                );

                let mut text_prompts = vec![json!({
                    "text": base.prompt,
                    "weight": 1.0
                })];

                if let Some(np) = base.negative_prompt.as_deref() {
                    text_prompts.push(json!({ "text": np, "weight": -1.0 }));
                }

                let mut body = json!({
                    "text_prompts": text_prompts,
                    "samples": base.n.max(1),
                });

                if let Some(size) = extras.size.as_deref() {
                    let parts: Vec<&str> = size.split('x').collect();
                    if parts.len() == 2 {
                        if let (Ok(w), Ok(h)) = (parts[0].parse::<u64>(), parts[1].parse::<u64>()) {
                            body["width"] = Value::Number(w.into());
                            body["height"] = Value::Number(h.into());
                        }
                    }
                }

                if let Some(steps) = extras.steps {
                    body["steps"] = Value::Number(steps.into());
                }

                if let Some(gs) = extras.guidance_scale {
                    body["cfg_scale"] = json!(gs);
                }

                if let Some(seed) = base.seed {
                    body["seed"] = Value::Number(seed.into());
                }

                // Shallow-merge extra
                if let Some(Value::Object(extra_map)) = &extras.extra {
                    if let Some(body_map) = body.as_object_mut() {
                        for (k, v) in extra_map {
                            body_map.insert(k.clone(), v.clone());
                        }
                    }
                }

                crate::providers::inject_trace_headers(
                    self.client
                        .post(&url)
                        .header("Authorization", format!("Bearer {api_key}"))
                        .header("Content-Type", "application/json")
                        .header("Accept", "application/json")
                        .json(&body),
                )
                .send()
                .await
                .map_err(|e| ProviderError::RequestFailed {
                    message: format!("Stability V1 request failed: {e}"),
                    status_code: e.status().map(|s| s.as_u16()),
                    provider_error: None,
                    retryable: e.is_timeout() || e.is_connect(),
                })?
            };

            let status = resp.status();
            let resp_json: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to parse Stability response: {e}"),
                status_code: Some(status.as_u16()),
                provider_error: None,
                retryable: false,
            })?;

            if !status.is_success() {
                let error_msg = resp_json["message"].as_str().unwrap_or("Unknown").to_string();
                return Err(ProviderError::RequestFailed {
                    message: format!("Stability API error: {error_msg}"),
                    status_code: Some(status.as_u16()),
                    provider_error: Some(resp_json),
                    retryable: status.as_u16() >= 500,
                });
            }

            let artifact = resp_json["artifacts"]
                .as_array()
                .and_then(|a| a.first())
                .ok_or_else(|| ProviderError::RequestFailed {
                    message: "Stability response missing artifacts".to_string(),
                    status_code: None,
                    provider_error: Some(resp_json.clone()),
                    retryable: false,
                })?;

            let b64 = artifact["base64"].as_str().ok_or_else(|| ProviderError::RequestFailed {
                message: "Stability artifact missing base64".to_string(),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;

            let bytes = B64.decode(b64).map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to decode Stability base64: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;

            let mut metadata = HashMap::new();
            metadata.insert("model".to_string(), Value::String(model_name));
            if let Some(finish_reason) = artifact["finishReason"].as_str() {
                metadata.insert("finish_reason".to_string(), Value::String(finish_reason.to_string()));
            }

            Ok(GenerationOutput {
                data: bytes,
                content_type: "image/png".to_string(),
                metadata,
                more: Vec::new(),
            })
        }
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

    /// Validate the API key via `GET {api_base}/v1/engines/list`.
    ///
    /// @see <https://platform.stability.ai/docs/api-reference#tag/Engines> — list engines
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Stability provider not configured".into(),
                latency_ms: None,
            };
        }
        let start = std::time::Instant::now();
        match self
            .client
            .get(format!("{}/v1/engines/list", self.api_base()))
            .header("Authorization", format!("Bearer {}", self.api_key().unwrap_or_default()))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "Stability API key valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("Stability returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("Stability health check failed: {e}"),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
    }
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            task: Default::default(),
            supports_text_to_image: false,
            supports_image_to_image: false,
            supports_inpainting: false,
            supported_sizes: Vec::new(),
            max_reference_images: 1,
            max_images: 1,
            supports_text_to_video: false,
            supports_image_to_video: false,
            supports_first_frame: false,
            supports_last_frame: false,
            max_duration_seconds: None,
            supports_text_to_3d: false,
            supports_image_to_3d: false,
            supports_multiview_to_3d: false,
            supports_pbr: false,
            supports_rig: false,
            supports_texture: false,
            supports_source_model: false,
            output_formats: Vec::new(),
            max_polycount: None,
            supports_region_edit: false,
            region_input: None,
            preserves_unmasked: None,
            mask_convention: None,
            supports_keyframes: false, max_keyframes: None, keyframe_timing: None,
            supports_transparent_background: false, lossy_only: false,
            supports_fixed_camera: false, supports_audio_toggle: false, supports_loop: false, duration_values: Vec::new(),
            supports_background_removal: false, supports_segmentation: false, segmentation_prompts: Vec::new(),
            texture_resolutions: Vec::new(), supports_keep_uvs: false, supports_placement: false, supports_expand: false,
            supports_layout: false, layout_kinds: Vec::new(), supports_mask_prompt: false,
            supports_preserve_unmasked: false,
            supports_source_video: false, extend_output: None, supports_time_range: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::{method, path};

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

    fn make_provider(api_base: &str) -> StabilityProvider {
        let mut p = StabilityProvider::new();
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
            negative_prompt: Some("bad quality".to_string()),
            seed: Some(42),
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        }
    }

    fn make_extras_sd3() -> ImageExtras {
        ImageExtras {
            vendor_job: Default::default(),
            size: None,
            aspect_ratio: Some("1:1".to_string()),
            quality: None,
            style: None,
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".to_string(),
            edit: None, views: Vec::new(),
            extra: None,
            output_format: None, background: None,
            expand: None,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
        }
    }

    #[tokio::test]
    async fn generates_sd3_large_via_v2_multipart() {
        let server = MockServer::start().await;

        // Return fake PNG bytes
        let fake_png = b"FAKEPNG".to_vec();
        Mock::given(method("POST"))
            .and(path("/v2beta/stable-image/generate/sd3"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(fake_png.clone())
                    .insert_header("content-type", "image/png"),
            )
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("stability/sd3-large");
        let base = make_base("a photorealistic landscape", "stability/sd3-large");
        let mut extras = make_extras_sd3();
        extras.output_format = Some("webp".into());
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let output = result.unwrap();
        assert_eq!(output.data, fake_png);

        // Check that the multipart form included required fields
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body_str = String::from_utf8_lossy(&received[0].body);
        assert!(body_str.contains("name=\"output_format\"\r\n\r\nwebp"), "PixelGenie L9: the asked container: {body_str}");
        // Multipart body should contain these field names and values
        assert!(body_str.contains("a photorealistic landscape"), "prompt not in body");
        assert!(body_str.contains("sd3.5-large"), "model not in body");
        assert!(body_str.contains("1:1"), "aspect_ratio not in body");
        assert!(body_str.contains("42"), "seed not in body");
        assert!(body_str.contains("bad quality"), "negative_prompt not in body");
    }

    /// Extract a simple text field's value out of a multipart/form-data body.
    fn multipart_field(body: &[u8], name: &str) -> Option<String> {
        let s = String::from_utf8_lossy(body).into_owned();
        let marker = format!("name=\"{name}\"");
        let at = s.find(&marker)? + marker.len();
        let rest = &s[at..];
        let value_at = rest.find("\r\n\r\n")? + 4;
        let value = &rest[value_at..];
        let end = value.find("\r\n")?;
        Some(value[..end].to_string())
    }

    async fn mock_v2(server: &MockServer, endpoint: &str) {
        Mock::given(method("POST"))
            .and(path(format!("/v2beta/stable-image/generate/{endpoint}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"PNG".to_vec())
                    .insert_header("content-type", "image/png"),
            )
            .mount(server)
            .await;
    }

    /// The `/v2beta/stable-image/generate/sd3` `model` enum is
    /// `sd3.5-large | sd3.5-large-turbo | sd3.5-medium`. `sd3-large` and
    /// `sd3-turbo` are no longer members; `stability/sd3.5-medium` sends its
    /// own value.
    #[tokio::test]
    async fn sd3_catalog_ids_map_to_the_current_sd35_model_values() {
        for (catalog_id, expected) in [
            ("stability/sd3-large", "sd3.5-large"),
            ("stability/sd3-turbo", "sd3.5-large-turbo"),
            ("stability/sd3.5-medium", "sd3.5-medium"),
        ] {
            let server = MockServer::start().await;
            mock_v2(&server, "sd3").await;

            let provider = make_provider(&server.uri());
            let schema = ref_schema(catalog_id);
            let base = make_base("a canyon", catalog_id);
            provider
                .generate(&schema, &base, &make_extras_sd3(), &empty_materialized())
                .await
                .expect("generate");

            let received = server.received_requests().await.unwrap();
            assert_eq!(
                multipart_field(&received[0].body, "model").as_deref(),
                Some(expected),
                "{catalog_id} must send {expected}"
            );
        }
    }

    /// The `core` and `ultra` request schemas have no `model` property at all —
    /// the route selects the model. Only `sd3` takes one.
    #[tokio::test]
    async fn model_field_is_only_sent_to_the_sd3_route() {
        for (catalog_id, endpoint) in [
            ("stability/core", "core"),
            ("stability/ultra", "ultra"),
        ] {
            let server = MockServer::start().await;
            mock_v2(&server, endpoint).await;

            let provider = make_provider(&server.uri());
            let schema = ref_schema(catalog_id);
            let base = make_base("a canyon", catalog_id);
            provider
                .generate(&schema, &base, &make_extras_sd3(), &empty_materialized())
                .await
                .expect("generate");

            let received = server.received_requests().await.unwrap();
            assert!(
                multipart_field(&received[0].body, "model").is_none(),
                "/generate/{endpoint} has no `model` field in its schema"
            );
        }
    }

    /// `style` is validated against the same values as the API's `style_preset`
    /// but was never written to the form — the caller's choice was dropped.
    #[tokio::test]
    async fn style_is_forwarded_as_style_preset() {
        let server = MockServer::start().await;
        mock_v2(&server, "core").await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("stability/core");
        let base = make_base("a canyon", "stability/core");
        let mut extras = make_extras_sd3();
        extras.style = Some("cinematic".to_string());

        provider
            .generate(&schema, &base, &extras, &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        assert_eq!(
            multipart_field(&received[0].body, "style_preset").as_deref(),
            Some("cinematic")
        );
    }

    /// sd3 gates image-to-image behind `mode`. Without it the uploaded `image`
    /// part is ignored and the request runs as text-to-image.
    #[tokio::test]
    async fn image_to_image_sends_mode_image_to_image() {
        let server = MockServer::start().await;
        mock_v2(&server, "sd3").await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("stability/sd3-large");
        let base = make_base("repaint in oils", "stability/sd3-large");
        let mut extras = make_extras_sd3();
        extras.strength = Some(0.6);
        let materialized = crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: crate::proxy::materializer::MaterializedRefForm::MultipartField {
                    field_name: "image".to_string(),
                    bytes: bytes::Bytes::from_static(b"src"),
                    content_type: "image/png".to_string(),
                },
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };

        provider
            .generate(&schema, &base, &extras, &materialized)
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        assert_eq!(
            multipart_field(&received[0].body, "mode").as_deref(),
            Some("image-to-image")
        );
        // "image-to-image requires the prompt, image, and strength parameters".
        assert_eq!(
            multipart_field(&received[0].body, "strength").as_deref(),
            Some("0.60")
        );
        // "aspect_ratio ... is only valid for text-to-image requests".
        assert!(
            multipart_field(&received[0].body, "aspect_ratio").is_none(),
            "aspect_ratio must not be sent in image-to-image mode"
        );
    }

    /// `strength` is "only valid for image-to-image requests" — sending it on a
    /// text-to-image call is an invalid-parameter error.
    #[tokio::test]
    async fn strength_is_not_sent_for_text_to_image() {
        let server = MockServer::start().await;
        mock_v2(&server, "sd3").await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("stability/sd3-large");
        let base = make_base("a canyon", "stability/sd3-large");
        let mut extras = make_extras_sd3();
        extras.strength = Some(0.7);

        provider
            .generate(&schema, &base, &extras, &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        assert_eq!(
            multipart_field(&received[0].body, "mode").as_deref(),
            Some("text-to-image")
        );
        assert!(
            multipart_field(&received[0].body, "strength").is_none(),
            "strength is image-to-image-only"
        );
    }

    /// image-to-image requires `strength`; supply a sane default rather than
    /// letting the request fail when the caller omits it.
    #[tokio::test]
    async fn image_to_image_defaults_strength_when_omitted() {
        let server = MockServer::start().await;
        mock_v2(&server, "sd3").await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("stability/sd3-large");
        let base = make_base("repaint in oils", "stability/sd3-large");
        let materialized = crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: crate::proxy::materializer::MaterializedRefForm::MultipartField {
                    field_name: "image".to_string(),
                    bytes: bytes::Bytes::from_static(b"src"),
                    content_type: "image/png".to_string(),
                },
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };

        provider
            .generate(&schema, &base, &make_extras_sd3(), &materialized)
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        assert!(
            multipart_field(&received[0].body, "strength").is_some(),
            "image-to-image must always carry a strength"
        );
    }

    /// v2 `aspect_ratio` enum: `21:9, 16:9, 3:2, 5:4, 1:1, 4:5, 2:3, 9:16, 9:21`.
    /// The size-derivation helper used to emit `4:3` and `3:4`.
    #[test]
    fn every_derived_aspect_ratio_is_in_the_v2_enum() {
        const SUPPORTED: [&str; 9] = [
            "21:9", "16:9", "3:2", "5:4", "1:1", "4:5", "2:3", "9:16", "9:21",
        ];
        for w in (256..=3072).step_by(16) {
            for h in [256u32, 512, 768, 832, 1024, 1200, 1280, 1536, 2048] {
                if let Some(ar) = StabilityProvider::aspect_ratio_from_size(&format!("{w}x{h}")) {
                    assert!(
                        SUPPORTED.contains(&ar),
                        "{w}x{h} derived unsupported aspect ratio {ar:?}"
                    );
                }
            }
        }
    }

    /// Remove Background takes `image` and `output_format` (png | webp,
    /// default png) and nothing else: no prompt.
    /// @see <https://api.stability.ai/v2alpha/openapi> — `/v2beta/stable-image/edit/remove-background`
    #[tokio::test]
    async fn remove_background_sends_only_the_image_and_its_container() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2beta/stable-image/edit/remove-background"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"CUTOUT".to_vec()).insert_header("content-type", "image/png"))
            .mount(&server)
            .await;
        let materialized = crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: crate::proxy::materializer::MaterializedRefForm::MultipartField {
                    field_name: "image".to_string(),
                    bytes: bytes::Bytes::from_static(b"src"),
                    content_type: "image/png".to_string(),
                },
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };
        let mut base = make_base("", "stability/remove-background");
        base.seed = None;
        let extras = ImageExtras { aspect_ratio: None, size: None, style: None, strength: None, ..make_extras_sd3() };
        let out = make_provider(&server.uri())
            .generate(&ref_schema("stability/remove-background"), &base, &extras, &materialized)
            .await
            .expect("generate");
        assert_eq!(out.data, b"CUTOUT");
        let body = &server.received_requests().await.unwrap()[0].body;
        assert_eq!(multipart_field(body, "output_format").as_deref(), Some("png"));
        assert!(String::from_utf8_lossy(body).contains("name=\"image\""), "the image part");
        for absent in ["prompt", "model", "mode", "strength", "aspect_ratio", "seed", "grow_mask"] {
            assert!(multipart_field(body, absent).is_none(), "{absent} is not a remove-background field");
        }
    }

    /// Outpaint grows the canvas by `left`, `right`, `up` and `down` (0–2000
    /// px each, at least one non-zero) around `image`; the prompt is optional.
    /// @see <https://api.stability.ai/v2alpha/openapi> — `/v2beta/stable-image/edit/outpaint`
    #[tokio::test]
    async fn outpaint_sends_each_side_as_stabilitys_direction() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2beta/stable-image/edit/outpaint"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"WIDER".to_vec()).insert_header("content-type", "image/png"))
            .mount(&server)
            .await;
        let materialized = crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: crate::proxy::materializer::MaterializedRefForm::MultipartField {
                    field_name: "image".to_string(),
                    bytes: bytes::Bytes::from_static(b"src"),
                    content_type: "image/png".to_string(),
                },
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };
        let mut base = make_base("more sky", "stability/outpaint");
        base.seed = None;
        let extras = ImageExtras {
            aspect_ratio: None,
            size: None,
            style: None,
            strength: None,
            expand: Some(crate::types::Expand { left: 5, top: 7, right: 0, bottom: 2 }),
            ..make_extras_sd3()
        };
        let out = make_provider(&server.uri())
            .generate(&ref_schema("stability/outpaint"), &base, &extras, &materialized)
            .await
            .expect("generate");
        assert_eq!(out.data, b"WIDER");
        let body = &server.received_requests().await.unwrap()[0].body;
        for (field, want) in [("left", "5"), ("up", "7"), ("right", "0"), ("down", "2"), ("prompt", "more sky"), ("output_format", "png")] {
            assert_eq!(multipart_field(body, field).as_deref(), Some(want), "{field}");
        }
        for absent in ["model", "mode", "strength", "aspect_ratio", "grow_mask"] {
            assert!(multipart_field(body, absent).is_none(), "{absent} is not an outpaint field");
        }
    }

    /// Search and Replace takes the area as text (`search_prompt`) beside the
    /// prompt, and `grow_mask` 0 (its default 3 would blur the edge).
    /// @see <https://api.stability.ai/v2alpha/openapi> — `/v2beta/stable-image/edit/search-and-replace`
    #[tokio::test]
    async fn search_and_replace_sends_the_named_area_as_search_prompt() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2beta/stable-image/edit/search-and-replace"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"SWAPPED".to_vec()).insert_header("content-type", "image/png"))
            .mount(&server)
            .await;
        let materialized = crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: crate::proxy::materializer::MaterializedRefForm::MultipartField {
                    field_name: "image".to_string(),
                    bytes: bytes::Bytes::from_static(b"src"),
                    content_type: "image/png".to_string(),
                },
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };
        let mut base = make_base("a red door", "stability/search-and-replace");
        base.seed = None;
        let extras = ImageExtras { aspect_ratio: None, size: None, style: None, strength: None, mask_prompt: Some("the door".into()), ..make_extras_sd3() };
        let out = make_provider(&server.uri())
            .generate(&ref_schema("stability/search-and-replace"), &base, &extras, &materialized)
            .await
            .expect("generate");
        assert_eq!(out.data, b"SWAPPED");
        let body = &server.received_requests().await.unwrap()[0].body;
        for (field, want) in [("prompt", "a red door"), ("search_prompt", "the door"), ("grow_mask", "0"), ("output_format", "png")] {
            assert_eq!(multipart_field(body, field).as_deref(), Some(want), "{field}");
        }
        for absent in ["model", "mode", "strength", "aspect_ratio"] {
            assert!(multipart_field(body, absent).is_none(), "{absent} is not a search-and-replace field");
        }
    }
}
