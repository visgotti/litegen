use crate::providers::auth::Authorize;
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
/// Supports SD 3.5, Core, Ultra, SDXL and the edit tools via the Stability API.
/// V1 engine (SDXL): JSON body → base64 response
/// V2 endpoints (SD 3.5, Core, Ultra, edit): multipart form → raw image bytes
///
/// @see <https://platform.stability.ai/docs/api-reference> (2026-11-09) — "The Stability AI API reference
///   has moved to https://docs.stability.ai/api-reference."
/// @see <https://docs.stability.ai/api-reference/generate/stable-diffusion-35> — V2beta generate (SD 3.5/Core/Ultra)
/// @see <https://docs.stability.ai/api-reference/sdxl-10/text-to-image> — V1 `POST /v1/generation/{engine}/text-to-image`
pub struct StabilityProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
    /// How every request to the vendor's API is authenticated.
    auth: crate::providers::auth::AuthSpec,
}

impl StabilityProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (result downloads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(120), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            client: Self::api_client(&Default::default()),
            auth: crate::providers::auth::AuthSpec::bearer(),
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
            if let Some(mapped) = cfg.mapped_model(model_id) {
                return mapped.to_string();
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
            _ => native.to_string(),
        }
    }

    /// Stability's error in its own words: "Errors return a JSON body with a
    /// machine-readable `name` and a list of human-readable `errors`". It may
    /// be sent again only when Stability turned the call away (429, 5xx).
    /// @see <https://docs.stability.ai/api-reference/index.md> (2026-10-30)
    fn vendor_error(status: reqwest::StatusCode, retry_after: Option<u64>, text: &str) -> ProviderError {
        let body: Option<Value> = serde_json::from_str(text).ok();
        let said = body
            .as_ref()
            .and_then(|b| {
                b["errors"]
                    .as_array()
                    .map(|e| e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; "))
                    .filter(|e| !e.is_empty())
                    .or_else(|| b["message"].as_str().map(str::to_string))
            })
            .unwrap_or_else(|| text.chars().take(200).collect());
        if let Some(e) = crate::providers::rate_limited("stability", status, retry_after, &format!("Stability API error: {said}"), body.as_ref()) {
            return e;
        }
        ProviderError::RequestFailed {
            message: format!("Stability API error: {said}"),
            status_code: Some(status.as_u16()),
            provider_error: Some(body.unwrap_or_else(|| Value::String(text.to_string()))),
            retryable: status.as_u16() == 429 || status.is_server_error(),
        }
    }

    /// A blurred image is not a result: "`CONTENT_FILTERED` = successful
    /// generation, however the output violated our content moderation policy
    /// and has been blurred as a result".
    /// @see <https://api.stability.ai/v2alpha/openapi> (ImageGenerationSuccess, 2026-10-30)
    fn filtered(finish_reason: &str) -> Result<(), ProviderError> {
        if finish_reason == "CONTENT_FILTERED" {
            return Err(ProviderError::RequestFailed {
                message: "Stability blurred the image: finish reason CONTENT_FILTERED (it violated the content moderation policy)".into(),
                status_code: None,
                provider_error: None,
                retryable: false,
            });
        }
        Ok(())
    }

    /// Determine which V2 route to use for a given model, under
    /// `/v2beta/stable-image/`.
    ///
    /// @see <https://docs.stability.ai/api-reference/generate/stable-image-core> — V2beta Stable Image generate routes
    /// @see <https://docs.stability.ai/api-reference/edit/inpaint> — the edit routes (`edit/inpaint`,
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
        self.client = Self::api_client(&config.extra_headers);
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
    /// @see <https://docs.stability.ai/api-reference/generate/stable-diffusion-35> — V2 generate; proves the multipart
    ///   fields (`prompt`, `model`, `output_format`, `aspect_ratio`, `seed`, `negative_prompt`, `strength`)
    ///   and the raw-image-bytes (`Accept: image/*`) response.
    /// @see <https://docs.stability.ai/api-reference/sdxl-10/text-to-image> — V1 `text-to-image`; proves `text_prompts[]`
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

            // Search and Replace (3D doc §4) finds the area `mask_prompt` names.
            let search = route == "edit/search-and-replace";
            // Stability refuses an outpaint with no side above 0 and a search
            // with nothing to find; refuse first, before anything is sent.
            if outpaint && extras.expand.is_none_or(|e| e.left == 0 && e.top == 0 && e.right == 0 && e.bottom == 0) {
                return Err(ProviderError::InvalidRequest(
                    "outpaint needs `expand` with at least one side above 0".into(),
                ));
            }
            if search && extras.mask_prompt.as_deref().is_none_or(|p| p.trim().is_empty()) {
                return Err(ProviderError::InvalidRequest(
                    "search-and-replace needs `mask_prompt`: what to find and replace".into(),
                ));
            }

            let mut form = reqwest::multipart::Form::new();
            if !matting && !(outpaint && base.prompt.trim().is_empty()) {
                form = form.text("prompt", base.prompt.clone());
            }
            // Search and Replace's `grow_mask` goes as 0, as for inpaint (its
            // default 3 blurs it).
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

            // /generate/sd3 only: "cfg_scale … How strictly the diffusion
            // process adheres to the prompt text" (1–10; "The _Large_ and
            // _Medium_ models use a default of `4`. The _Turbo_ … `1`").
            // @see https://api.stability.ai/v2alpha/openapi (/generate/sd3 cfg_scale, 2026-11-09)
            if let (true, Some(gs)) = (endpoint == "sd3", extras.guidance_scale) {
                form = form.text("cfg_scale", gs.to_string());
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

            // extra fields shallow-merged (allowlist enforced by validator),
            // never as a second part of a field litegen sends: two `model`
            // parts could run sd3.5-large at sd3.5-medium's price.
            const V2_OWNED_FIELDS: &[&str] = &[
                "prompt", "model", "mode", "strength", "aspect_ratio", "output_format", "seed", "negative_prompt", "image",
                "mask", "grow_mask", "search_prompt", "left", "up", "right", "down", "cfg_scale",
            ];
            let owned = |k: &str| V2_OWNED_FIELDS.contains(&k) || (k == "style_preset" && extras.style.is_some());
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), owned) {
                let val_str = match v {
                    Value::String(s) => s.clone(),
                    _ => v.to_string(),
                };
                form = form.text(k.clone(), val_str);
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
                    .authorized(&self.auth, &api_key)?
                    .header("Accept", "image/*")
                    .multipart(form),
            )
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Stability V2 request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // Stability answers in the same call: one that timed out may
                // have run and been billed.
                retryable: e.is_connect(),
            })?;

            let status = resp.status();
            if !status.is_success() {
                let retry_after = crate::providers::retry_after_ms(resp.headers());
                let error_body = resp.text().await.unwrap_or_default();
                return Err(Self::vendor_error(status, retry_after, &error_body));
            }
            let finish_reason = resp.headers().get("finish-reason").and_then(|v| v.to_str().ok()).map(str::to_string);
            if let Some(reason) = finish_reason.as_deref() {
                Self::filtered(reason)?;
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
            if let Some(reason) = finish_reason {
                metadata.insert("finish_reason".to_string(), Value::String(reason));
            }

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
            // @see <https://docs.stability.ai/api-reference/sdxl-10/image-to-image-with-prompt> — V1 `image-to-image`
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
            // Masking is `required: ["text_prompts","init_image","mask_source"]`.
            // @see <https://docs.stability.ai/api-reference/sdxl-10/image-to-image-with-a-mask.md> (2026-11-09)
            if has_mask && !has_init {
                return Err(ProviderError::InvalidRequest(format!("'{}' masks an image: send an init image with the mask", model.id)));
            }
            let use_multipart = has_init || has_mask;
            // What litegen itself sends on v1; `extra` cannot replace it.
            const V1_OWNED_FIELDS: &[&str] = &[
                "text_prompts", "samples", "steps", "cfg_scale", "seed", "image_strength", "mask_source", "init_image",
                "mask_image", "width", "height", "style_preset",
            ];
            // "style_preset" on text-to-image, image-to-image and masking alike.
            // @see https://docs.stability.ai/api-reference/sdxl-10/text-to-image.md (2026-11-09)
            // @see https://docs.stability.ai/api-reference/sdxl-10/image-to-image-with-prompt.md (2026-11-09)
            // @see https://docs.stability.ai/api-reference/sdxl-10/image-to-image-with-a-mask.md (2026-11-09)
            let style_preset = extras.style.as_deref();

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

                if let Some(style) = style_preset {
                    form = form.text("style_preset", style.to_string());
                }

                if let Some(seed) = base.seed {
                    form = form.text("seed", seed.to_string());
                }

                // `image_strength` near 1 keeps the init image ("Values close to
                // `1` will yield images very similar to the `init_image`"), the
                // opposite of litegen's (and sd3's) `strength`, where 0 keeps
                // it — so it goes inverted, and only to /image-to-image:
                // masking has no `image_strength`.
                if let (Some(strength), false) = (extras.strength, has_mask) {
                    form = form.text("image_strength", format!("{:.2}", 1.0 - strength));
                }

                // Masking requires telling the API how the mask is encoded.
                if has_mask {
                    form = form.text("mask_source", "MASK_IMAGE_WHITE");
                }

                // extra fields shallow-merged (allowlist enforced by validator).
                for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| V1_OWNED_FIELDS.contains(&k) || k.starts_with("text_prompts[")) {
                    let val_str = match v {
                        Value::String(s) => s.clone(),
                        _ => v.to_string(),
                    };
                    form = form.text(k.clone(), val_str);
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
                        .authorized(&self.auth, &api_key)?
                        .header("Accept", "application/json")
                        .multipart(form),
                )
                .send()
                .await
                .map_err(|e| ProviderError::RequestFailed {
                    message: format!("Stability V1 request failed: {e}"),
                    status_code: e.status().map(|s| s.as_u16()),
                    provider_error: None,
                    retryable: e.is_connect(),
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

                if let Some(style) = style_preset {
                    body["style_preset"] = Value::String(style.to_string());
                }

                if let Some(seed) = base.seed {
                    body["seed"] = Value::Number(seed.into());
                }

                // Shallow-merge extra, never over what litegen sends.
                if let Some(body_map) = body.as_object_mut() {
                    for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| V1_OWNED_FIELDS.contains(&k)) {
                        body_map.insert(k.clone(), v.clone());
                    }
                }

                crate::providers::inject_trace_headers(
                    self.client
                        .post(&url)
                        .authorized(&self.auth, &api_key)?
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
                    retryable: e.is_connect(),
                })?
            };

            let status = resp.status();
            let retry_after = crate::providers::retry_after_ms(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(Self::vendor_error(status, retry_after, &text));
            }
            let resp_json: Value = serde_json::from_str(&text).map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to parse Stability response: {e}"),
                status_code: Some(status.as_u16()),
                provider_error: None,
                retryable: false,
            })?;

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

            if let Some(reason) = artifact["finishReason"].as_str() {
                Self::filtered(reason)?;
            }
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
    /// @see <https://docs.stability.ai/api-reference/engines/list-engines.md> — list engines (`GET /v1/engines/list`, 2026-11-09)
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Stability provider not configured".into(),
                latency_ms: None,
            };
        }
        let start = std::time::Instant::now();
        let request = match self
            .client
            .get(format!("{}/v1/engines/list", self.api_base()))
            .authorized(&self.auth, &self.api_key().unwrap_or_default())
        {
            Ok(request) => request,
            Err(e) => return HealthCheckResult { healthy: false, message: format!("Stability health check failed: {e}"), latency_ms: None },
        };
        match request.timeout(std::time::Duration::from_secs(10)).send().await {
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
            // Not stated: only the server knows its body limit.
            max_request_bytes: 0,
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
            supports_animation: false,
            supports_skeleton: false,
            supports_joint_naming: false,
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
            supports_mask_grow: false,
            supports_source_video: false, extend_output: None, supports_time_range: false,
            native_pixel_art: false, pixel_size: None, supports_palette: false, supports_forced_palette: false, directions: Vec::new(),
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
            resolution: None, aspect_ratio: Some("1:1".to_string()),
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
            palette: None, directions: None,
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
        let extras = ImageExtras { resolution: None, aspect_ratio: None, size: None, style: None, strength: None, ..make_extras_sd3() };
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
            resolution: None, aspect_ratio: None,
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

    /// Outpaint needs a side to grow and Search and Replace a thing to find;
    /// without them Stability refuses, so litegen refuses first, unsent.
    /// @see <https://api.stability.ai/v2alpha/openapi> ("At least one outpainting direction must be supplied with a non-zero value"; `search_prompt` required)
    #[tokio::test]
    async fn an_edit_missing_what_it_needs_is_refused_before_any_call() {
        let server = MockServer::start().await;
        let materialized = || crate::proxy::materializer::MaterializedRequest {
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
        let provider = make_provider(&server.uri());
        for expand in [None, Some(crate::types::Expand { left: 0, top: 0, right: 0, bottom: 0 })] {
            let extras = ImageExtras { resolution: None, aspect_ratio: None, size: None, style: None, strength: None, expand, ..make_extras_sd3() };
            let err = provider
                .generate(&ref_schema("stability/outpaint"), &make_base("", "stability/outpaint"), &extras, &materialized())
                .await
                .expect_err("nothing to grow");
            assert!(matches!(err, ProviderError::InvalidRequest(ref m) if m.contains("expand")), "{err:?}");
        }
        let extras = ImageExtras { resolution: None, aspect_ratio: None, size: None, style: None, strength: None, mask_prompt: Some("  ".into()), ..make_extras_sd3() };
        let err = provider
            .generate(&ref_schema("stability/search-and-replace"), &make_base("a cat", "stability/search-and-replace"), &extras, &materialized())
            .await
            .expect_err("nothing to find");
        assert!(matches!(err, ProviderError::InvalidRequest(ref m) if m.contains("mask_prompt")), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing was sent");
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
        let extras = ImageExtras { resolution: None, aspect_ratio: None, size: None, style: None, strength: None, mask_prompt: Some("the door".into()), ..make_extras_sd3() };
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

    // ─── Stability audit, 2026-10-30 ────────────────────────────────────────

    const SDXL_ENGINE: &str = "/v1/generation/stable-diffusion-xl-1024-v1-0";

    fn uploaded(fields: &[&str]) -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: fields
                .iter()
                .map(|f| crate::proxy::materializer::MaterializedRef {
                    role: if *f == "mask_image" { "mask".into() } else { "init".into() },
                    form: MaterializedRefForm::MultipartField {
                        field_name: f.to_string(),
                        bytes: bytes::Bytes::from_static(b"\x89PNG\r\n\x1a\n"),
                        content_type: "image/png".into(),
                    },
                    time_s: None,
                })
                .collect(),
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    /// F6: every field goes once: `extra` never adds a second `model` (which
    /// could run sd3.5-large at sd3.5-medium's price), `output_format` or
    /// `prompt` part.
    #[tokio::test]
    async fn a_v2_request_carries_each_field_once() {
        let server = MockServer::start().await;
        mock_v2(&server, "sd3").await;
        let mut extras = make_extras_sd3();
        extras.extra = Some(json!({ "model": "sd3.5-large", "output_format": "webp", "prompt": "other", "style_preset": "anime" }));
        let _ = make_provider(&server.uri())
            .generate(&ref_schema("stability/sd3.5-medium"), &make_base("a dune", "stability/sd3.5-medium"), &extras, &empty_materialized())
            .await;
        let received = server.received_requests().await.unwrap();
        let body = String::from_utf8_lossy(&received[0].body).to_string();
        for (field, value) in [("model", "sd3.5-medium"), ("output_format", "png"), ("prompt", "a dune")] {
            assert_eq!(body.matches(&format!("name=\"{field}\"")).count(), 1, "{field}: {body}");
            assert_eq!(multipart_field(&received[0].body, field).as_deref(), Some(value), "{field}");
        }
        assert_eq!(multipart_field(&received[0].body, "style_preset").as_deref(), Some("anime"));
    }

    /// F7, rule 14: "`CONTENT_FILTERED` = successful generation, however the
    /// output violated our content moderation policy and has been blurred as
    /// a result" — a blurred image fails with Stability's reason.
    /// @see <https://api.stability.ai/v2alpha/openapi> (ImageGenerationSuccess, 2026-10-30)
    /// @see <https://docs.stability.ai/api-reference/index.md> (2026-10-30)
    #[tokio::test]
    async fn a_content_filtered_image_fails_with_stabilitys_reason() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2beta/stable-image/generate/sd3"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"\x89PNG\r\n\x1a\n".to_vec())
                    .insert_header("content-type", "image/png")
                    .insert_header("finish-reason", "CONTENT_FILTERED"),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{SDXL_ENGINE}/text-to-image")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "artifacts": [{ "base64": "iVBORw0KGgo=", "finishReason": "CONTENT_FILTERED", "seed": 1 }]
            })))
            .mount(&server)
            .await;
        let provider = make_provider(&server.uri());
        for (id, extras) in [("stability/sd3.5-medium", make_extras_sd3()), ("stability/sdxl", ImageExtras { aspect_ratio: None, ..make_extras_sd3() })] {
            let err = provider.generate(&ref_schema(id), &make_base("x", id), &extras, &empty_materialized()).await.unwrap_err();
            assert!(
                matches!(&err, ProviderError::RequestFailed { retryable: false, message, .. } if message.contains("CONTENT_FILTERED")),
                "{id}: {err:?}"
            );
        }
    }

    /// F12: "Errors return a JSON body with a machine-readable `name` and a
    /// list of human-readable `errors`" — the message is Stability's, the body
    /// is kept, and a 429 was turned away and may be sent again.
    /// @see <https://docs.stability.ai/api-reference/index.md> (2026-10-30)
    #[tokio::test]
    async fn an_error_says_what_stability_said() {
        for (status, retryable) in [(400, false), (429, true)] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!({
                    "id": "e1", "name": "bad_request", "errors": ["prompt: is required"]
                })))
                .mount(&server)
                .await;
            let provider = make_provider(&server.uri());
            for (id, extras) in [("stability/sd3.5-medium", make_extras_sd3()), ("stability/sdxl", ImageExtras { aspect_ratio: None, ..make_extras_sd3() })] {
                let err = provider.generate(&ref_schema(id), &make_base("x", id), &extras, &empty_materialized()).await.unwrap_err();
                assert!(
                    matches!(&err, ProviderError::RequestFailed { status_code: Some(s), retryable: r, provider_error: Some(_), message }
                        if *s == status && *r == retryable && message.contains("prompt: is required")),
                    "{id} {status}: {err:?}"
                );
            }
        }
    }

    /// F8: masking is `required: ["text_prompts","init_image","mask_source"]`,
    /// so a mask without an init image is refused before any call.
    /// @see <https://docs.stability.ai/api-reference/sdxl-10/image-to-image-with-a-mask.md> (2026-11-09)
    #[tokio::test]
    async fn a_mask_without_an_init_image_is_refused_before_any_call() {
        let server = MockServer::start().await;
        let err = make_provider(&server.uri())
            .generate(&ref_schema("stability/sdxl"), &make_base("x", "stability/sdxl"), &make_extras_sd3(), &uploaded(&["mask_image"]))
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// F9: sd3's `strength` 0 "would yield an image that is identical to the
    /// input"; v1's `image_strength` close to 1 yields "images very similar to
    /// the `init_image`". The canonical strength goes inverted, and only to
    /// `/image-to-image` (masking has no `image_strength`).
    /// @see <https://api.stability.ai/v2alpha/openapi> (/generate/sd3 strength, 2026-11-09)
    /// @see <https://docs.stability.ai/api-reference/sdxl-10/image-to-image-with-prompt.md> (image_strength, 2026-11-09)
    #[tokio::test]
    async fn sdxl_strength_is_sent_inverted_and_only_to_image_to_image() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "name": "stop", "errors": ["stop"] })))
            .mount(&server)
            .await;
        let provider = make_provider(&server.uri());
        let mut extras = make_extras_sd3();
        extras.strength = Some(0.3);
        for fields in [&["init_image"][..], &["init_image", "mask_image"][..]] {
            let _ = provider.generate(&ref_schema("stability/sdxl"), &make_base("x", "stability/sdxl"), &extras, &uploaded(fields)).await;
        }
        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].url.path(), format!("{SDXL_ENGINE}/image-to-image"));
        assert_eq!(multipart_field(&received[0].body, "image_strength").as_deref(), Some("0.70"));
        assert_eq!(received[1].url.path(), format!("{SDXL_ENGINE}/image-to-image/masking"));
        assert!(multipart_field(&received[1].body, "image_strength").is_none());
    }

    // ─── Vendor feature audit, 2026-11-03 ───────────────────────────────────

    /// /generate/sd3: "cfg_scale … How strictly the diffusion process adheres
    /// to the prompt text" (1–10) is the canonical `guidance_scale`, sent once
    /// (`extra` cannot add a second part); /ultra and /core define no
    /// cfg_scale, so it never goes there.
    /// @see <https://api.stability.ai/v2alpha/openapi> (2026-11-09)
    #[tokio::test]
    async fn guidance_goes_as_cfg_scale_and_only_to_the_sd3_route() {
        let server = MockServer::start().await;
        mock_v2(&server, "sd3").await;
        mock_v2(&server, "ultra").await;
        let provider = make_provider(&server.uri());
        let mut extras = make_extras_sd3();
        extras.guidance_scale = Some(4.5);
        extras.extra = Some(json!({ "cfg_scale": 9 }));
        for id in ["stability/sd3-large", "stability/ultra"] {
            provider.generate(&ref_schema(id), &make_base("a dune", id), &extras, &empty_materialized()).await.expect(id);
        }
        let received = server.received_requests().await.unwrap();
        let sd3 = String::from_utf8_lossy(&received[0].body).to_string();
        assert_eq!(sd3.matches("name=\"cfg_scale\"").count(), 1, "{sd3}");
        assert_eq!(multipart_field(&received[0].body, "cfg_scale").as_deref(), Some("4.5"));
        assert!(multipart_field(&received[1].body, "cfg_scale").is_none(), "ultra has no cfg_scale");
    }

    /// The v1 SDXL engine takes "style_preset" on text-to-image (JSON),
    /// image-to-image and masking (multipart): the canonical `style` goes as
    /// it on all three.
    /// @see <https://docs.stability.ai/api-reference/sdxl-10/text-to-image.md> (2026-11-09)
    /// @see <https://docs.stability.ai/api-reference/sdxl-10/image-to-image-with-prompt.md> (2026-11-09)
    /// @see <https://docs.stability.ai/api-reference/sdxl-10/image-to-image-with-a-mask.md> (2026-11-09)
    #[tokio::test]
    async fn sdxl_sends_style_as_style_preset_on_every_v1_route() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "name": "stop", "errors": ["stop"] })))
            .mount(&server)
            .await;
        let provider = make_provider(&server.uri());
        let mut extras = make_extras_sd3();
        extras.aspect_ratio = None;
        extras.style = Some("anime".into());
        for fields in [&[][..], &["init_image"][..], &["init_image", "mask_image"][..]] {
            let _ = provider.generate(&ref_schema("stability/sdxl"), &make_base("x", "stability/sdxl"), &extras, &uploaded(fields)).await;
        }
        let received = server.received_requests().await.unwrap();
        let json_body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(json_body["style_preset"], "anime", "{json_body}");
        for r in &received[1..] {
            assert_eq!(multipart_field(&r.body, "style_preset").as_deref(), Some("anime"), "{}", r.url.path());
        }
    }

    /// Stability answers in the same call, so one that timed out may have run
    /// and been billed: it is never sent again.
    #[tokio::test]
    async fn a_timed_out_call_is_never_sent_again() {
        let slow = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"x".to_vec()).set_delay(std::time::Duration::from_secs(5)))
            .mount(&slow)
            .await;
        let mut p = make_provider(&slow.uri());
        p.client = reqwest::Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        for (id, extras) in [("stability/sd3.5-medium", make_extras_sd3()), ("stability/sdxl", ImageExtras { aspect_ratio: None, ..make_extras_sd3() })] {
            let err = p.generate(&ref_schema(id), &make_base("x", id), &extras, &empty_materialized()).await.unwrap_err();
            assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{id}: {err:?}");
        }
    }
}
