use crate::providers::auth::Authorize;
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::{CostEstimate, CostSource, ImageGenerationRequest};

/// What litegen itself sends in an Images API body; `extra` cannot replace it.
const OWNED_FIELDS: &[&str] = &[
    "model", "prompt", "n", "size", "quality", "style", "response_format", "image", "image[]", "mask", "output_format", "background",
];

/// OpenAI image generation provider (GPT Image family).
///
/// Text-to-image goes to `POST /v1/images/generations`; a request carrying a
/// reference image (and optional mask) goes to `POST /v1/images/edits`.
///
/// DALL-E 2 and DALL-E 3 were shut down on 2026-05-12; their size/quality/style
/// handling is retained only for operators pointing `model_mapping` at a legacy
/// deployment.
///
/// @see <https://developers.openai.com/api/docs/deprecations> — DALL-E shutdown
/// @see <https://developers.openai.com/api/reference/resources/images/methods/generate> — Images API "create" reference
///   (the request body and response object this provider conforms to).
pub struct OpenAiProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
    /// How every request to the vendor's API is authenticated.
    auth: crate::providers::auth::AuthSpec,
    /// Result downloads: no redirects, so a URL that passed the public-address
    /// check cannot bounce into a private one.
    asset_client: Client,
    /// Whether a result URL must resolve to a public address. Only the tests,
    /// whose mock vendor runs on loopback, turn it off.
    pub(crate) public_assets_only: bool,
}

impl OpenAiProvider {
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
            asset_client: crate::util::ssrf::public_only_download_client(
                std::time::Duration::from_secs(120),
                std::time::Duration::from_secs(30),
            ),
            public_assets_only: true,
        }
    }

    /// The API root, `https://api.openai.com/v1` unless `api_base` says
    /// otherwise; every URL is built from it. A base set to the generations
    /// URL itself, as configs used to, names the same root.
    fn api_root(&self) -> String {
        let base = self
            .config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.openai.com/v1")
            .trim_end_matches('/');
        base.strip_suffix("/images/generations").unwrap_or(base).to_string()
    }

    fn api_url(&self) -> String {
        format!("{}/images/generations", self.api_root())
    }

    fn api_key(&self) -> Result<String, ProviderError> {
        if let Some(pool) = &self.key_pool {
            return Ok(pool.next().to_string());
        }
        self.config
            .as_ref()
            .map(|c| c.api_key.clone())
            .ok_or_else(|| ProviderError::NotConfigured("openai".into()))
    }

    /// The size to send, or a refusal: a size the model cannot make is
    /// refused, never snapped to one the caller did not ask for (L13). The
    /// catalog's size spec refuses these before billing; this is the
    /// adapter's own guard.
    ///
    /// @see <https://developers.openai.com/api/docs/deprecations> and the
    ///   `CreateImageRequest.size` description: the GPT image models take
    ///   `1024x1024`, `1536x1024` and `1024x1536`; `gpt-image-2` additionally
    ///   takes arbitrary `WIDTHxHEIGHT` with both dimensions divisible by 16, an
    ///   aspect ratio between 1:3 and 3:1, neither edge above 3840, and
    ///   655,360..=8,294,400 total pixels, so `2160x3840` (4K portrait) is
    ///   legal and `512x512` is not. The
    ///   `dall-e-*` arms are retained only for operators who point
    ///   `model_mapping` at a legacy deployment; both models were shut down on
    ///   2026-05-12.
    /// @see <https://developers.openai.com/api/docs/guides/image-generation>
    ///   ("GPT Image 2 settings", re-read 2026-09-11) — the edge and pixel limits.
    fn vendor_size(size: &str, model: &str) -> Result<String, ProviderError> {
        let refuse = || ProviderError::InvalidRequest(format!("{model} cannot make a {size} image"));
        let (w, h) = size
            .split_once('x')
            .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
            .ok_or_else(refuse)?;
        let listed: &[(u32, u32)] = if Self::is_dalle3(model) {
            &[(1024, 1024), (1792, 1024), (1024, 1792)]
        } else if model.starts_with("dall-e-2") {
            &[(256, 256), (512, 512), (1024, 1024)]
        } else {
            &[(1024, 1024), (1536, 1024), (1024, 1536)]
        };
        if listed.contains(&(w, h)) {
            return Ok(format!("{w}x{h}"));
        }
        // gpt-image-2 accepts arbitrary 16-aligned sizes within its edge,
        // total-pixel and aspect limits.
        let pixels = u64::from(w) * u64::from(h);
        let ratio = f64::from(w) / f64::from(h.max(1));
        let custom = model.starts_with("gpt-image-2")
            && w % 16 == 0
            && h % 16 == 0
            && w <= 3840
            && h <= 3840
            && (655_360..=8_294_400).contains(&pixels)
            && (1.0 / 3.0..=3.0).contains(&ratio);
        if custom {
            Ok(format!("{w}x{h}"))
        } else {
            Err(refuse())
        }
    }

    /// GPT Image 2.5 output-token cost of `quality` relative to `medium`, the
    /// quality the catalog's base cost is priced at.
    ///
    /// OpenAI's token calculator (the guide's "Calculating costs" section) puts
    /// a 1024x1024 `gpt-image-2.5` image at 196 / 439 / 1,756 / 3,122 / 7,024
    /// output tokens for low / medium / high / xhigh / max. `max` is 16x
    /// `medium`, so the flat GPT Image factors (high 2x, anything unknown 1x)
    /// would bill a `max` image at a sixteenth of its cost. `auto` depends on
    /// the generated image, so it stays at the medium anchor.
    ///
    /// The calculator is a client-rendered component, so the counts are not
    /// page text: they come from its script's grid (`low:16, medium:24,
    /// high:48, xhigh:64, max:96` for "gpt-image-2.5") and formula, re-derived
    /// 2026-11-09.
    /// @see <https://developers.openai.com/api/docs/guides/image-generation#calculating-costs>
    /// @see <https://developers.openai.com/_astro/GptImageTokenCalculator.react.Ul9pMTwf.js>
    fn gpt_image_2_5_quality_factor(quality: &str) -> f64 {
        const MEDIUM_TOKENS: f64 = 439.0;
        let tokens = match quality {
            "low" => 196.0,
            "high" => 1_756.0,
            "xhigh" => 3_122.0,
            "max" => 7_024.0,
            _ => MEDIUM_TOKENS,
        };
        tokens / MEDIUM_TOKENS
    }

    /// GPT Image 2's price by quality, relative to the medium image the
    /// catalog's base is for: OpenAI's 1024x1024 table, "Low $0.006 · Medium
    /// $0.053 · High $0.211". `auto` depends on the generated image, so it
    /// stays at the medium anchor.
    ///
    /// @see <https://developers.openai.com/api/docs/guides/image-generation.md>
    ///   ("Historical output costs", 2026-11-09)
    fn gpt_image_2_quality_factor(quality: &str) -> f64 {
        const MEDIUM_USD: f64 = 0.053;
        let usd = match quality {
            "low" => 0.006,
            "high" | "hd" => 0.211,
            _ => MEDIUM_USD,
        };
        usd / MEDIUM_USD
    }

    fn is_dalle3(model: &str) -> bool {
        let m = model.to_lowercase();
        m.contains("dall-e-3") || m.contains("dalle-3") || m.contains("dalle3")
    }

    /// True for the GPT image family, which does not accept `response_format`
    /// or `style` and always returns base64.
    fn is_gpt_image(model: &str) -> bool {
        model.starts_with("gpt-image") || model == "chatgpt-image-latest"
    }

    /// Strip the provider prefix (e.g. "openai/gpt-image-2" → "gpt-image-2").
    fn native_model_id(model_id: &str) -> &str {
        if let Some(rest) = model_id.strip_prefix("openai/") {
            rest
        } else {
            model_id
        }
    }

    /// Resolve to the on-the-wire model name.
    ///
    /// An unrecognised id is passed through verbatim rather than guessed at.
    /// This used to fall back to `"dall-e-2"`, which since 2026-05-12 means
    /// every unknown id silently resolved to a model that no longer exists.
    fn resolve_model_name(&self, model: &str) -> String {
        if let Some(cfg) = &self.config {
            if let Some(mapped) = cfg.mapped_model(model) {
                return mapped.to_string();
            }
        }
        let native = Self::native_model_id(model);
        if native.is_empty() {
            return "gpt-image-2".to_string();
        }
        native.to_string()
    }

    /// The sibling `images/edits` route. `api_base` is configured as the full
    /// generations URL, so derive the edits URL from it rather than assuming a
    /// host layout.
    ///
    /// @see <https://developers.openai.com/api/reference> — `POST /v1/images/edits`
    fn edits_url(&self) -> String {
        format!("{}/images/edits", self.api_root())
    }

    /// Build the multipart `images/edits` request when the caller supplied a
    /// reference image, or `None` for a plain text-to-image generation.
    ///
    /// The Images API splits generation and editing across two endpoints; the
    /// generations endpoint has no image input at all, so a ref image used to
    /// be dropped and the request silently ran as text-to-image.
    ///
    /// @see <https://developers.openai.com/api/reference> — `CreateImageEditRequest`
    ///   (`image`, `mask`, `prompt`, `model`, `n`, `size`, `quality`, and the
    ///   GPT-image-only `background` / `output_format` / `output_compression`).
    fn build_edit_request(
        &self,
        api_key: &str,
        model_name: &str,
        size: &str,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<Option<reqwest::RequestBuilder>, ProviderError> {
        let mut form = reqwest::multipart::Form::new();

        // The catalog maps `init` and `reference` → `image`, and `mask` → `mask`.
        // "You can use one or more images as a reference" — several go as
        // repeated `image[]` parts, the edited one (`init`) first, since "If you
        // provide multiple input images, the mask will be applied to the first
        // image"; one goes as `image`.
        // @see <https://developers.openai.com/api/docs/guides/image-generation.md> (2026-11-03)
        let mut images: Vec<&crate::proxy::materializer::MaterializedRef> = materialized
            .refs
            .iter()
            .filter(|r| matches!(&r.form, MaterializedRefForm::MultipartField { field_name, .. } if field_name == "image"))
            .collect();
        if images.is_empty() {
            return Ok(None);
        }
        images.sort_by_key(|r| r.role != "init");
        let image_field = if images.len() > 1 { "image[]" } else { "image" };
        let masks = materialized
            .refs
            .iter()
            .filter(|r| matches!(&r.form, MaterializedRefForm::MultipartField { field_name, .. } if field_name == "mask"));
        for (field, r) in images.iter().map(|r| (image_field, *r)).chain(masks.map(|r| ("mask", r))) {
            if let MaterializedRefForm::MultipartField { bytes, content_type, .. } = &r.form {
                let part = reqwest::multipart::Part::bytes(bytes.to_vec())
                    .file_name(format!("{}.png", field.trim_end_matches("[]")))
                    .mime_str(content_type)
                    .map_err(|e| {
                        ProviderError::InvalidRequest(format!("openai edit ref mime: {e}"))
                    })?;
                form = form.part(field.to_string(), part);
            }
        }

        // One image: litegen reads `data[0]` and bills one, so `n` is never sent.
        form = form
            .text("model", model_name.to_string())
            .text("prompt", base.prompt.clone())
            .text("size", size.to_string())
            .text("n", "1".to_string());

        if let Some(q) = extras.quality.as_deref() {
            form = form.text("quality", q.to_string());
        }
        // `extra` adds the endpoint's own fields; it cannot replace one litegen
        // sets, and `moderation` — which only the generations body and the
        // JSON edit body take — is not a multipart edit field.
        // @see <https://developers.openai.com/api/reference/resources/images/methods/edit>
        for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED_FIELDS.contains(&k) || k == "moderation") {
            let s = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            form = form.text(k.clone(), s);
        }
        // PixelGenie L9: the canonical container and background.
        if let Some(f) = extras.output_format.as_deref() {
            form = form.text("output_format", f.to_string());
        }
        if let Some(b) = extras.background.as_deref() {
            form = form.text("background", b.to_string());
        }

        Ok(Some(
            self.client
                .post(self.edits_url())
                .authorized(&self.auth, api_key)?
                .multipart(form),
        ))
    }

    /// Send an Images API request and turn the `data[0]` entry into bytes.
    /// Shared by the generations and edits paths, which return the same object.
    async fn send_and_read_image(
        &self,
        request: reqwest::RequestBuilder,
        response_format: &str,
    ) -> Result<GenerationOutput, ProviderError> {
        let resp = crate::providers::inject_trace_headers(request)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("OpenAI request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // Only a connection that never opened was surely not received:
                // "Complex prompts may take up to 2 minutes to process", and a
                // call that timed out may still run, and bill.
                retryable: e.is_connect(),
            })?;

        // The status before the body (read_vendor_json): a 429, a 408 or a
        // 5xx turned the call away and is retried, after any `Retry-After`
        // (docs/gateway-settings.md); any other error status is OpenAI's refusal.
        let resp_json = crate::providers::read_vendor_json(resp, "OpenAI").await?;

        let data = resp_json["data"]
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "OpenAI response missing data array".to_string(),
                status_code: None,
                provider_error: Some(resp_json.clone()),
                retryable: false,
            })?;

        let mut metadata: HashMap<String, Value> = HashMap::new();
        if let Some(rp) = data["revised_prompt"].as_str() {
            metadata.insert("revised_prompt".to_string(), Value::String(rp.to_string()));
        }

        // GPT image models always return base64 regardless of what was asked
        // for, so prefer `b64_json` whenever it is present.
        let (image_bytes, content_type) = if response_format == "b64_json"
            || data.get("b64_json").is_some()
        {
            let b64 = data["b64_json"]
                .as_str()
                .ok_or_else(|| ProviderError::RequestFailed {
                    message: "OpenAI response missing b64_json".to_string(),
                    status_code: None,
                    provider_error: None,
                    retryable: false,
                })?;
            let bytes = B64.decode(b64).map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to decode b64_json: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;
            (bytes, "image/png".to_string())
        } else {
            let url = data["url"].as_str().ok_or_else(|| ProviderError::RequestFailed {
                message: "OpenAI response missing url".to_string(),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;
            metadata.insert("url".to_string(), Value::String(url.to_string()));
            let bytes = self.fetch_image_bytes(url).await?;
            (bytes, "image/png".to_string())
        };

        Ok(GenerationOutput {
            data: image_bytes,
            content_type,
            metadata,
            more: Vec::new(),
        })
    }

    /// Download the rendered image from the URL the Images API returns when
    /// `response_format` is `url` (legacy models only: "Unsupported for GPT
    /// image models, which always return base64-encoded images"): without
    /// the key, only from a public address, and at most 64 MiB. OpenAI has run
    /// and billed the job by now, so no failure here is worth a second call.
    ///
    /// @see <https://developers.openai.com/api/reference/resources/images/methods/generate> — `data[].url`
    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        use crate::providers::model3d::download::{fetch_asset, AssetError};
        match fetch_asset(&self.asset_client, url, 64 * 1024 * 1024, self.public_assets_only, "openai image").await {
            Ok((bytes, _)) => Ok(bytes),
            Err(AssetError::Transient(ProviderError::RequestFailed { message, status_code, provider_error, .. })) => {
                Err(ProviderError::RequestFailed { message, status_code, provider_error, retryable: false })
            }
            Err(AssetError::Transient(e)) => Err(e),
            Err(AssetError::Gone(why)) => Err(ProviderError::RequestFailed {
                message: format!("openai image result could not be fetched: {why}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            }),
        }
    }
}

impl Default for OpenAiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
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

    /// Create an image via `POST {api_base}/v1/images/generations`.
    ///
    /// @see <https://developers.openai.com/api/reference/resources/images/methods/generate> — Images: create.
    ///   Proves the request fields used here (`model`, `prompt`, `size`, `n`,
    ///   `response_format`, and the DALL-E 3 `quality`/`style` options) and the
    ///   `data[].b64_json` / `data[].url` / `data[].revised_prompt` response fields.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let api_key = self.api_key()?;
        let model_name = self.resolve_model_name(&model.id);
        let is_dalle3 = Self::is_dalle3(&model_name);
        let is_gpt_image = Self::is_gpt_image(&model_name);

        // Determine size
        let size = extras.size.as_deref().unwrap_or("1024x1024");
        let size = Self::vendor_size(size, &model_name)?;

        // `response_format` applies to `dall-e-2`/`dall-e-3` only: "Legacy
        // response format parameter for retired image models. Unsupported for
        // GPT image models, which always return base64-encoded images."
        // @see <https://developers.openai.com/api/reference/resources/images/methods/generate> `response_format` (2026-11-09)
        let response_format = if is_gpt_image || extras.response_format == "b64_json" {
            "b64_json"
        } else {
            "url"
        };

        // A reference image means this is an edit, which lives on a different
        // endpoint with a multipart body.
        if let Some(request) =
            self.build_edit_request(&api_key, &model_name, &size, base, extras, materialized)?
        {
            return self.send_and_read_image(request, response_format).await;
        }

        // Build JSON body. One image: litegen reads `data[0]` and bills one.
        let mut body = json!({
            "model": model_name,
            "prompt": base.prompt,
            "size": size,
            "n": 1,
        });
        if !is_gpt_image {
            body["response_format"] = Value::String(response_format.to_string());
        }

        // `quality` applies to both families (with different value sets);
        // `style` is `dall-e-3`-only.
        if let Some(q) = extras.quality.as_deref() {
            body["quality"] = Value::String(q.to_string());
        }
        if is_dalle3 {
            if let Some(s) = extras.style.as_deref() {
                body["style"] = Value::String(s.to_string());
            }
        }

        // `extra` adds the endpoint's own fields (output_compression,
        // moderation, user, safety_identifier); it cannot replace one litegen
        // sets — `extra.model` would run a dearer model than the one billed.
        if let Some(body_map) = body.as_object_mut() {
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED_FIELDS.contains(&k)) {
                body_map.insert(k.clone(), v.clone());
            }
        }
        // PixelGenie L9: the canonical container and background, after
        // `extra` so a caller's canonical field wins.
        if let Some(f) = extras.output_format.as_deref() {
            body["output_format"] = Value::String(f.to_string());
        }
        if let Some(b) = extras.background.as_deref() {
            body["background"] = Value::String(b.to_string());
        }

        let resp = crate::providers::inject_trace_headers(
            self.client
                .post(self.api_url())
                .authorized(&self.auth, &api_key)?
                .header("Content-Type", "application/json")
                .json(&body),
        );

        self.send_and_read_image(resp, response_format).await
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        request: &ImageGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        let model_name = self.resolve_model_name(&model.id);
        let size = Self::vendor_size(request.size.as_deref().unwrap_or("1024x1024"), &model_name)?;
        let quality = request.quality.as_deref().unwrap_or("auto");

        // GPT image pricing scales with quality and output resolution; use the
        // catalog's base cost as the anchor and scale from it rather than
        // hardcoding a per-size table that will drift.
        let pixels: f64 = size
            .split_once('x')
            .and_then(|(w, h)| Some(w.parse::<f64>().ok()? * h.parse::<f64>().ok()?))
            .unwrap_or(1024.0 * 1024.0);
        let size_factor = (pixels / (1024.0 * 1024.0)).max(1.0);
        let quality_factor = if model_name.starts_with("gpt-image-2.5") {
            Self::gpt_image_2_5_quality_factor(quality)
        } else {
            Self::gpt_image_2_quality_factor(quality)
        };
        let base_cost = model.pricing.base_cost_usd * size_factor * quality_factor;

        Ok(build_cost_estimate(
            base_cost,
            0.0,
            CostSource::Estimated,
            Some(json!({
                "model": model_name,
                "size": size,
                "quality": quality,
            })),
        ))
    }

    /// Validate the API key with a lightweight `GET https://api.openai.com/v1/models`.
    ///
    /// @see <https://developers.openai.com/api/reference/resources/models/methods/list> — Models: list
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "OpenAI provider not configured".into(),
                latency_ms: None,
            };
        }

        let start = std::time::Instant::now();
        let api_key = match self.api_key() {
            Ok(k) => k,
            Err(e) => {
                return HealthCheckResult {
                    healthy: false,
                    message: format!("Failed to get API key: {e}"),
                    latency_ms: None,
                }
            }
        };

        let request = match self.client.get(format!("{}/models", self.api_root())).authorized(&self.auth, &api_key) {
            Ok(request) => request,
            Err(e) => return HealthCheckResult { healthy: false, message: format!("OpenAI health check failed: {e}"), latency_ms: None },
        };
        match request.timeout(std::time::Duration::from_secs(10)).send().await {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "OpenAI API key valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("OpenAI returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("OpenAI health check failed: {e}"),
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
    use serde_json::json;

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

    /// A provider against a mock OpenAI; result URLs are on loopback, so the
    /// public-address check is off (`a_result_url_is_fetched_only_from_a_public_address`).
    fn make_provider_with_base(base_url: &str) -> OpenAiProvider {
        let mut p = OpenAiProvider::new();
        p.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: "test-key".to_string(),
            api_keys: vec![],
            api_base: Some(base_url.to_string()),
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
            seed: None,
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        }
    }

    fn make_extras(size: Option<&str>, quality: Option<&str>, style: Option<&str>) -> ImageExtras {
        ImageExtras {
            vendor_job: Default::default(),
            size: size.map(|s| s.to_string()),
            resolution: None, aspect_ratio: None,
            quality: quality.map(|s| s.to_string()),
            style: style.map(|s| s.to_string()),
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "b64_json".to_string(),
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
    async fn generates_dalle3_image_b64_json() {
        let server = MockServer::start().await;
        let fake_b64 = B64.encode(b"fake-png-bytes");

        // Mock the POST /v1/images/generations endpoint
        Mock::given(method("POST"))
            .and(path("/v1/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "created": 1234567890,
                "data": [{
                    "b64_json": fake_b64,
                    "revised_prompt": "A beautiful landscape revised"
                }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider_with_base(&format!("{}/v1/images/generations", server.uri()));
        let schema = ref_schema("openai/gpt-image-2");
        let base = make_base("a beautiful landscape", "openai/gpt-image-2");
        let mut extras = make_extras(Some("1024x1024"), Some("high"), None);
        extras.output_format = Some("webp".into());
        extras.background = Some("transparent".into());
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let output = result.unwrap();

        assert_eq!(output.data, b"fake-png-bytes");
        assert_eq!(
            output.metadata.get("revised_prompt").and_then(|v| v.as_str()),
            Some("A beautiful landscape revised")
        );

        // Verify the request body contained the right fields
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "gpt-image-2");
        assert_eq!(body["prompt"], "a beautiful landscape");
        // PixelGenie L9: the canonical container and background.
        assert_eq!((&body["output_format"], &body["background"]), (&json!("webp"), &json!("transparent")));
        assert_eq!(body["size"], "1024x1024");
        assert_eq!(body["quality"], "high");
        assert_eq!(body["n"], 1);
        // `style` is dall-e-3-only and must never be sent to a GPT image model.
        assert!(body.get("style").is_none(), "style is dall-e-3-only");
    }

    // ─── Key-pool wiring (configure → api_key) ──────────────────────────
    // These characterize the already-shipped multi-key path end to end within
    // a real provider: configure() must build the pool, is_configured() must
    // honor a pool-only setup, and api_key() must draw from the pool.

    fn cfg_with_keys(keys: Vec<crate::types::ApiKeyEntry>) -> ProviderInstanceConfig {
        ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: String::new(),
            api_keys: keys,
            api_base: None,
            model_mapping: Default::default(),
            extra_headers: Default::default(),
            options: None,
        }
    }

    fn key(k: &str, weight: u32) -> crate::types::ApiKeyEntry {
        crate::types::ApiKeyEntry { key: k.to_string(), weight, label: None }
    }

    #[test]
    fn configure_with_pool_cycles_keys() {
        let mut p = OpenAiProvider::new();
        // No single api_key — only a pool.
        p.configure(cfg_with_keys(vec![key("sk-a", 1), key("sk-b", 1)]));
        assert!(p.is_configured(), "a key pool alone makes the provider configured");

        let k1 = p.api_key().unwrap();
        let k2 = p.api_key().unwrap();
        let k3 = p.api_key().unwrap();
        assert_ne!(k1, k2, "consecutive picks differ");
        assert_eq!(k1, k3, "round-robin wraps back to the first key");
    }

    #[test]
    fn configure_with_weighted_pool_respects_weights() {
        let mut p = OpenAiProvider::new();
        p.configure(cfg_with_keys(vec![key("sk-a", 3), key("sk-b", 1)]));

        let mut a = 0;
        let mut b = 0;
        for _ in 0..8 {
            match p.api_key().unwrap().as_str() {
                "sk-a" => a += 1,
                "sk-b" => b += 1,
                other => panic!("unexpected key {other}"),
            }
        }
        assert_eq!(a, 6, "weight 3 of 4 over 8 calls");
        assert_eq!(b, 2, "weight 1 of 4 over 8 calls");
    }

    #[test]
    fn configure_without_key_or_pool_is_not_configured() {
        let mut p = OpenAiProvider::new();
        p.configure(cfg_with_keys(vec![]));
        assert!(!p.is_configured(), "no api_key and no pool → not configured");
    }

    // ── Post-DALL·E model line ────────────────────────────────────────────
    // OpenAI shut `dall-e-2` and `dall-e-3` down on 2026-05-12, naming
    // "gpt-image-2, gpt-image-1, or gpt-image-1-mini" as the replacements.
    // @see <https://developers.openai.com/api/docs/deprecations>

    fn registry() -> crate::capabilities::CapabilityRegistry {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load")
    }

    #[test]
    fn catalog_ships_no_retired_dalle_models() {
        let r = registry();
        for dead in ["openai/dall-e-2", "openai/dall-e-3"] {
            assert!(
                r.get(dead).is_none(),
                "{dead} was shut down on 2026-05-12 and must not be advertised"
            );
        }
    }

    #[test]
    fn unknown_model_ids_are_not_rewritten_to_a_dead_model() {
        let p = OpenAiProvider::new();
        let resolved = p.resolve_model_name("openai/some-future-model");
        assert!(
            !resolved.starts_with("dall-e"),
            "unknown id resolved to the retired {resolved:?}"
        );
        assert_eq!(resolved, "some-future-model", "native id should pass through");
    }

    /// gpt-image-2 `size` rules (image generation guide, "GPT Image 2
    /// settings", re-read 2026-09-11): edges <= 3840 and multiples of 16,
    /// long:short <= 3:1, 655,360..=8,294,400 total pixels. `2160x3840` is a
    /// listed size, but the old 2160 height cap snapped it to `1024x1536`;
    /// `512x512` is under the pixel floor, but the old 256 floor forwarded it.
    ///
    /// GPT Image 2.5 (Sunburst, Flare) documents the same custom-size rules
    /// ("Size and quality options"), which the `gpt-image-2` prefix covers.
    #[test]
    fn gpt_image_2_sizes_follow_the_documented_constraints() {
        for model in [
            "gpt-image-2",
            "gpt-image-2.5-sunburst",
            "gpt-image-2.5-flare",
        ] {
            for listed in [
                "1024x1024",
                "2048x2048",
                "2048x1152",
                "3840x2160",
                "2160x3840",
            ] {
                assert_eq!(
                    OpenAiProvider::vendor_size(listed, model).unwrap(),
                    listed,
                    "{listed} is a listed {model} size and must pass through"
                );
            }
            // L13: refused, never snapped to a size the caller did not ask for.
            for out_of_range in ["512x512", "3840x3840", "3840x1024", "1000x1000"] {
                assert!(
                    matches!(OpenAiProvider::vendor_size(out_of_range, model), Err(ProviderError::InvalidRequest(_))),
                    "{out_of_range} breaks {model}'s limits and must be refused"
                );
            }
        }
        assert!(OpenAiProvider::vendor_size("1792x1024", "dall-e-3").is_ok());
        assert!(OpenAiProvider::vendor_size("1800x1024", "dall-e-3").is_err(), "not snapped to 1792x1024");
    }

    async fn estimated_cost(provider: &OpenAiProvider, id: &str, quality: &str) -> f64 {
        let request = ImageGenerationRequest {
            base: make_base("a tide pool at dawn", id),
            size: Some("1024x1024".to_string()),
            resolution: None, aspect_ratio: None,
            quality: Some(quality.to_string()),
            style: None,
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "b64_json".to_string(),
            region: None, mask_grow_px: None, views: None,
            output_format: None, background: None,
            expand: None,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        };
        provider
            .estimate_cost(&ref_schema(id), &request)
            .await
            .expect("estimate_cost")
            .base_cost_usd
    }

    /// GPT Image 2.5 adds `xhigh` and `max`, and its output tokens per quality
    /// are not GPT Image 2's. OpenAI's token calculator (the guide's
    /// "Calculating costs" component, "gpt-image-2.5" at 1024x1024): low 196,
    /// medium 439, high 1,756, xhigh 3,122, max 7,024 output tokens. The flat
    /// factors billed `max`, which they did not know, at the medium price: a
    /// sixteenth of its cost.
    /// @see <https://developers.openai.com/api/docs/guides/image-generation#calculating-costs> (2026-11-09)
    /// @see <https://developers.openai.com/_astro/GptImageTokenCalculator.react.Ul9pMTwf.js> (the grid and formula)
    #[tokio::test]
    async fn gpt_image_2_5_estimate_scales_with_the_documented_quality_tokens() {
        let provider = OpenAiProvider::new();
        for id in [
            "openai/gpt-image-2.5-sunburst",
            "openai/gpt-image-2.5-flare",
        ] {
            let medium = estimated_cost(&provider, id, "medium").await;
            for (quality, tokens) in [
                ("low", 196.0),
                ("high", 1_756.0),
                ("xhigh", 3_122.0),
                ("max", 7_024.0),
            ] {
                let got = estimated_cost(&provider, id, quality).await;
                let want = medium * tokens / 439.0;
                assert!(
                    (got - want).abs() < 1e-9,
                    "{id} {quality}: estimated {got}, OpenAI's calculator implies {want}"
                );
            }
        }
    }

    /// GPT Image 2 at 1024x1024: "Low $0.006 · Medium $0.053 · High $0.211"
    /// (OpenAI's "Historical output costs" table), not the flat half and
    /// double the estimate once used (which billed `high` at half its cost).
    /// @see <https://developers.openai.com/api/docs/guides/image-generation.md> (2026-11-09)
    #[tokio::test]
    async fn gpt_image_2_estimate_is_openais_table_by_quality() {
        let provider = OpenAiProvider::new();
        for (quality, usd) in [("low", 0.006), ("medium", 0.053), ("high", 0.211)] {
            let got = estimated_cost(&provider, "openai/gpt-image-2", quality).await;
            assert!((got - usd).abs() < 1e-9, "gpt-image-2 {quality}: estimated {got}, OpenAI's table says {usd}");
        }
    }

    #[tokio::test]
    async fn sends_gpt_image_2_model_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "created": 1, "data": [{ "b64_json": B64.encode(b"png") }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider_with_base(&format!("{}/v1/images/generations", server.uri()));
        let schema = ref_schema("openai/gpt-image-2");
        let base = make_base("a tide pool at dawn", "openai/gpt-image-2");
        let extras = make_extras(Some("1024x1024"), None, None);
        provider
            .generate(&schema, &base, &extras, &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "gpt-image-2");
    }

    /// "This parameter isn't supported for the GPT image models, which always
    /// return base64-encoded images." — `CreateImageRequest.response_format`
    #[tokio::test]
    async fn omits_response_format_for_gpt_image_models() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "created": 1, "data": [{ "b64_json": B64.encode(b"png") }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider_with_base(&format!("{}/v1/images/generations", server.uri()));
        let schema = ref_schema("openai/gpt-image-2");
        let base = make_base("a tide pool at dawn", "openai/gpt-image-2");
        let extras = make_extras(Some("1024x1024"), None, None);
        provider
            .generate(&schema, &base, &extras, &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert!(
            body.get("response_format").is_none(),
            "response_format must not be sent to GPT image models, got {body}"
        );
    }

    /// GPT image models always return base64, with no `url` variant — so the
    /// adapter must decode `data[0].b64_json` even though the caller did not
    /// ask for `b64_json`.
    #[tokio::test]
    async fn decodes_b64_json_for_gpt_image_even_when_caller_asked_for_url() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "created": 1, "data": [{ "b64_json": B64.encode(b"gpt-image-bytes") }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider_with_base(&format!("{}/v1/images/generations", server.uri()));
        let schema = ref_schema("openai/gpt-image-2");
        let base = make_base("a tide pool at dawn", "openai/gpt-image-2");
        let mut extras = make_extras(Some("1024x1024"), None, None);
        extras.response_format = "url".to_string();

        let out = provider
            .generate(&schema, &base, &extras, &empty_materialized())
            .await
            .expect("generate");
        assert_eq!(out.data, b"gpt-image-bytes");
    }

    #[test]
    fn catalog_quality_values_match_the_gpt_image_enum() {
        let r = registry();
        for id in ["openai/gpt-image-2"] {
            let schema = r.get(id).unwrap_or_else(|| panic!("{id} missing from catalog"));
            let values = match schema.params.get("quality") {
                Some(crate::capabilities::schema::ParamSpec::String(s)) => s.enum_values.clone(),
                None => continue,
                other => panic!("{id} quality param is {other:?}"),
            };
            for v in values {
                assert!(
                    ["auto", "low", "medium", "high"].contains(&v.as_str()),
                    "{id} allows quality {v:?}; `standard`/`hd` are dall-e-3-only"
                );
            }
        }
    }

    /// A reference image means an edit, which is a different endpoint
    /// (`POST /v1/images/edits`, multipart). Previously the ref was dropped and
    /// the request silently ran as text-to-image.
    #[tokio::test]
    async fn reference_image_routes_to_the_edits_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/edits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "created": 1, "data": [{ "b64_json": B64.encode(b"edited") }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider_with_base(&format!("{}/v1/images/generations", server.uri()));
        let schema = ref_schema("openai/gpt-image-2");
        let base = make_base("put a hat on the cat", "openai/gpt-image-2");
        let extras = make_extras(Some("1024x1024"), None, None);
        let materialized = crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: crate::proxy::materializer::MaterializedRefForm::MultipartField {
                    field_name: "image".to_string(),
                    bytes: bytes::Bytes::from_static(b"source-png"),
                    content_type: "image/png".to_string(),
                },
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };

        let out = provider
            .generate(&schema, &base, &extras, &materialized)
            .await
            .expect("generate");
        assert_eq!(out.data, b"edited");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1, "exactly one call, to /images/edits");
        let body = String::from_utf8_lossy(&received[0].body);
        assert!(body.contains("name=\"image\""), "missing image part: {body}");
        assert!(body.contains("name=\"prompt\""), "missing prompt part");
        assert!(body.contains("put a hat on the cat"));
    }

    /// "You can use one or more images as a reference to generate a new
    /// image" — `image` takes "an array of images" (maxItems 16), sent as
    /// repeated `image[]` parts — and "If you provide multiple input images,
    /// the mask will be applied to the first image": several images go as
    /// `image[]`, the edited one (`init`) first whatever order they came in;
    /// one image still goes as `image`.
    /// @see <https://developers.openai.com/api/docs/guides/image-generation.md> (2026-11-03)
    #[tokio::test]
    async fn several_images_go_as_an_image_array_with_the_edited_one_first() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/edits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "created": 1, "data": [{ "b64_json": B64.encode(b"edited") }]
            })))
            .mount(&server)
            .await;
        let provider = make_provider_with_base(&format!("{}/v1/images/generations", server.uri()));
        let id = "openai/gpt-image-2.5-sunburst";
        let part = |role: &str, field: &str, bytes: &'static [u8]| crate::proxy::materializer::MaterializedRef {
            role: role.to_string(),
            form: crate::proxy::materializer::MaterializedRefForm::MultipartField {
                field_name: field.to_string(),
                bytes: bytes::Bytes::from_static(bytes),
                content_type: "image/png".to_string(),
            },
            time_s: None,
        };
        let refs = |parts| crate::proxy::materializer::MaterializedRequest { refs: parts, cleanup: crate::proxy::materializer::Cleanup::empty() };
        let extras = make_extras(Some("1024x1024"), None, None);
        provider
            .generate(&ref_schema(id), &make_base("a gift basket", id), &extras, &refs(vec![
                part("reference", "image", b"REF-A"), part("init", "image", b"INIT-B"), part("mask", "mask", b"MASK"), part("reference", "image", b"REF-C"),
            ]))
            .await
            .expect("generate");
        provider
            .generate(&ref_schema(id), &make_base("a hat", id), &extras, &refs(vec![part("init", "image", b"ONLY")]))
            .await
            .expect("generate");
        let received = server.received_requests().await.unwrap();
        let many = String::from_utf8_lossy(&received[0].body).to_string();
        assert_eq!(many.matches("name=\"image[]\"").count(), 3, "{many}");
        assert!(!many.contains("name=\"image\""), "no bare image part beside the array: {many}");
        let (b, a, c) = (many.find("INIT-B").unwrap(), many.find("REF-A").unwrap(), many.find("REF-C").unwrap());
        assert!(b < a && a < c, "the edited image goes first, the rest in order: {many}");
        assert_eq!(many.matches("name=\"mask\"").count(), 1, "{many}");
        let one = String::from_utf8_lossy(&received[1].body).to_string();
        assert!(one.contains("name=\"image\"") && !one.contains("image[]"), "{one}");
    }

    // ─── OpenAI audit, 2026-10-30: W7–W9, W12–W14 ───────────────────────────

    fn edit_refs() -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: crate::proxy::materializer::MaterializedRefForm::MultipartField {
                    field_name: "image".to_string(),
                    bytes: bytes::Bytes::from_static(b"source-png"),
                    content_type: "image/png".to_string(),
                },
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    async fn answered(server: &MockServer, at: &str) {
        Mock::given(method("POST"))
            .and(path(at))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "created": 1, "data": [{ "b64_json": B64.encode(b"img") }] })))
            .mount(server)
            .await;
    }

    /// W8, W9: one image is asked for (litegen reads `data[0]` and bills
    /// one), and `extra` cannot replace what litegen sends — `extra.model`
    /// would run a dearer model than the one billed.
    #[tokio::test]
    async fn one_image_is_asked_for_and_extra_cannot_replace_what_litegen_sends() {
        let server = MockServer::start().await;
        answered(&server, "/v1/images/generations").await;
        let p = make_provider_with_base(&format!("{}/v1/images/generations", server.uri()));
        let schema = ref_schema("openai/gpt-image-2");
        let mut base = make_base("a lighthouse", "openai/gpt-image-2");
        base.n = 4;
        let mut extras = make_extras(Some("1024x1024"), Some("low"), None);
        extras.extra = Some(json!({ "model": "gpt-image-2.5-sunburst", "prompt": "other", "n": 8, "size": "1536x1024", "quality": "high", "user": "u1" }));
        p.generate(&schema, &base, &extras, &empty_materialized()).await.unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(
            (body["model"].as_str(), body["prompt"].as_str(), body["n"].as_u64(), body["size"].as_str(), body["quality"].as_str(), body["user"].as_str()),
            (Some("gpt-image-2"), Some("a lighthouse"), Some(1), Some("1024x1024"), Some("low"), Some("u1")),
            "{body}"
        );

        // The multipart edit path: one part each, and no `moderation`, which
        // the edits endpoint does not take (W12).
        let server = MockServer::start().await;
        answered(&server, "/v1/images/edits").await;
        let p = make_provider_with_base(&format!("{}/v1/images/generations", server.uri()));
        extras.extra = Some(json!({ "model": "gpt-image-2.5-sunburst", "n": 8, "moderation": "low", "user": "u1" }));
        p.generate(&schema, &base, &extras, &edit_refs()).await.unwrap();
        let form = String::from_utf8_lossy(&server.received_requests().await.unwrap()[0].body).to_string();
        for (part, count) in [("model", 1), ("n", 1), ("moderation", 0), ("user", 1)] {
            assert_eq!(form.matches(&format!("name=\"{part}\"")).count(), count, "{part}: {form}");
        }
        assert!(form.contains("gpt-image-2\r\n") && !form.contains("gpt-image-2.5-sunburst"), "{form}");
    }

    /// W13: `api_base` names the API root (`https://api.openai.com/v1`), from
    /// which generations, edits and the health check's `/models` are built;
    /// a base set to the generations URL itself still works.
    #[tokio::test]
    async fn every_url_is_built_from_the_configured_root() {
        for base in ["/v1", "/v1/images/generations"] {
            let server = MockServer::start().await;
            answered(&server, "/v1/images/generations").await;
            answered(&server, "/v1/images/edits").await;
            Mock::given(method("GET")).and(path("/v1/models")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] }))).expect(1).mount(&server).await;
            let p = make_provider_with_base(&format!("{}{base}", server.uri()));
            let schema = ref_schema("openai/gpt-image-2");
            let extras = make_extras(Some("1024x1024"), None, None);
            p.generate(&schema, &make_base("x", "openai/gpt-image-2"), &extras, &empty_materialized()).await.unwrap();
            p.generate(&schema, &make_base("x", "openai/gpt-image-2"), &extras, &edit_refs()).await.unwrap();
            assert!(p.health_check().await.healthy, "{base}");
        }
    }

    /// W7: a call that timed out may still have run, and been billed: it is
    /// never sent again.
    #[tokio::test]
    async fn a_timed_out_call_is_never_sent_again() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })).set_delay(std::time::Duration::from_secs(5)))
            .mount(&server)
            .await;
        let mut p = make_provider_with_base(&format!("{}/v1", server.uri()));
        p.client = Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        let err = p
            .generate(&ref_schema("openai/gpt-image-2"), &make_base("x", "openai/gpt-image-2"), &make_extras(Some("1024x1024"), None, None), &empty_materialized())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
    }

    /// W14: a result URL is fetched only from a public address.
    #[tokio::test]
    async fn a_result_url_is_fetched_only_from_a_public_address() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "url": "http://169.254.169.254/latest/meta-data" }] })))
            .mount(&server)
            .await;
        // Only a legacy model answers with a URL; GPT Image models always
        // return base64. One is reachable through `model_mapping`.
        let mut p = OpenAiProvider::new();
        p.configure(ProviderInstanceConfig {
            api_key: "test-key".into(),
            api_base: Some(format!("{}/v1", server.uri())),
            model_mapping: [("openai/gpt-image-2".to_string(), "dall-e-3".to_string())].into_iter().collect(),
            ..Default::default()
        });
        let mut extras = make_extras(Some("1024x1024"), None, None);
        extras.response_format = "url".into();
        let err = p.generate(&ref_schema("openai/gpt-image-2"), &make_base("x", "openai/gpt-image-2"), &extras, &empty_materialized()).await.unwrap_err();
        assert!(matches!(&err, ProviderError::RequestFailed { message, retryable: false, .. } if message.contains("refused")), "{err:?}");
    }

    /// A 429 turned the call away and is retried, after the `Retry-After`
    /// OpenAI asked for when it sent one (docs/gateway-settings.md).
    #[tokio::test]
    async fn a_429_is_retryable() {
        let body = json!({ "error": { "message": "Rate limit reached for requests", "type": "requests", "param": null, "code": "rate_limit_exceeded" } });
        for (answer, wait_ms) in [
            (ResponseTemplate::new(429).insert_header("retry-after", "2").set_body_json(body.clone()), Some(2000)),
            (ResponseTemplate::new(429).set_body_json(body.clone()), None),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).respond_with(answer).mount(&server).await;
            let err = make_provider_with_base(&format!("{}/v1", server.uri()))
                .generate(&ref_schema("openai/gpt-image-2"), &make_base("x", "openai/gpt-image-2"), &make_extras(Some("1024x1024"), None, None), &empty_materialized())
                .await
                .unwrap_err();
            match wait_ms {
                Some(ms) => assert!(matches!(&err, ProviderError::RateLimited { retry_after_ms, provider_error: Some(_), .. } if *retry_after_ms == ms), "{err:?}"),
                None => assert!(matches!(&err, ProviderError::RequestFailed { retryable: true, status_code: Some(429), .. }), "{err:?}"),
            }
        }
    }

    /// A gateway's 5xx page is not JSON: the status decides, and the call is
    /// retried rather than failed as an unreadable answer.
    #[tokio::test]
    async fn a_5xx_page_that_is_not_json_is_retryable() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(503).set_body_string("<html>Service Unavailable</html>")).mount(&server).await;
        let err = make_provider_with_base(&format!("{}/v1", server.uri()))
                .generate(&ref_schema("openai/gpt-image-2"), &make_base("x", "openai/gpt-image-2"), &make_extras(Some("1024x1024"), None, None), &empty_materialized())
                .await
                .unwrap_err();
        assert!(matches!(&err, ProviderError::RequestFailed { retryable: true, status_code: Some(503), .. }), "{err:?}");
    }
}
