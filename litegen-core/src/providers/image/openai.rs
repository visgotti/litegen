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
/// @see <https://platform.openai.com/docs/api-reference/images/create> — Images API "create" reference
///   (the request body and response object this provider conforms to).
pub struct OpenAiProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
}

impl OpenAiProvider {
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

    fn api_url(&self) -> String {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.openai.com/v1/images/generations")
            .to_string()
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

    /// Clamp a requested size to something the target model accepts.
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
    fn normalize_size(size: &str, model: &str) -> String {
        let parts: Vec<&str> = size.split('x').collect();
        let (w, h) = if parts.len() == 2 {
            (
                parts[0].parse::<u32>().unwrap_or(1024),
                parts[1].parse::<u32>().unwrap_or(1024),
            )
        } else {
            (1024, 1024)
        };

        if Self::is_dalle3(model) {
            return if w > h && w >= 1792 {
                "1792x1024"
            } else if h > w && h >= 1792 {
                "1024x1792"
            } else {
                "1024x1024"
            }
            .to_string();
        }
        if model.starts_with("dall-e-2") {
            return if w <= 256 || h <= 256 {
                "256x256"
            } else if w <= 512 || h <= 512 {
                "512x512"
            } else {
                "1024x1024"
            }
            .to_string();
        }

        // gpt-image-2 accepts arbitrary 16-aligned sizes within its edge and
        // total-pixel limits. The old `h <= 2160` cap rejected documented
        // portrait sizes such as 2160x3840, and the old 256 floor let sizes
        // below the 655,360-pixel minimum through.
        let pixels = u64::from(w) * u64::from(h);
        if model.starts_with("gpt-image-2")
            && w % 16 == 0
            && h % 16 == 0
            && w <= 3840
            && h <= 3840
            && (655_360..=8_294_400).contains(&pixels)
        {
            let ratio = w as f64 / h as f64;
            if (1.0 / 3.0..=3.0).contains(&ratio) {
                return format!("{w}x{h}");
            }
        }

        // Every other GPT image model takes one of the three standard sizes.
        if w > h {
            "1536x1024".to_string()
        } else if h > w {
            "1024x1536".to_string()
        } else {
            "1024x1024".to_string()
        }
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
            if let Some(mapped) = cfg.model_mapping.get(model) {
                return mapped.clone();
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
        let generations = self.api_url();
        match generations.strip_suffix("/generations") {
            Some(prefix) => format!("{prefix}/edits"),
            None => generations,
        }
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
        let mut has_image = false;

        for r in &materialized.refs {
            if let MaterializedRefForm::MultipartField {
                field_name,
                bytes,
                content_type,
            } = &r.form
            {
                // The catalog maps `init` → `image` and `mask` → `mask`.
                if field_name != "image" && field_name != "mask" {
                    continue;
                }
                let part = reqwest::multipart::Part::bytes(bytes.to_vec())
                    .file_name(format!("{field_name}.png"))
                    .mime_str(content_type)
                    .map_err(|e| {
                        ProviderError::InvalidRequest(format!("openai edit ref mime: {e}"))
                    })?;
                form = form.part(field_name.clone(), part);
                has_image |= field_name == "image";
            }
        }

        if !has_image {
            return Ok(None);
        }

        form = form
            .text("model", model_name.to_string())
            .text("prompt", base.prompt.clone())
            .text("size", size.to_string())
            .text("n", base.n.max(1).to_string());

        if let Some(q) = extras.quality.as_deref() {
            form = form.text("quality", q.to_string());
        }
        if let Some(Value::Object(extra_map)) = &extras.extra {
            for (k, v) in extra_map {
                let s = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                form = form.text(k.clone(), s);
            }
        }

        Ok(Some(
            self.client
                .post(self.edits_url())
                .header("Authorization", format!("Bearer {api_key}"))
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
                retryable: e.is_timeout() || e.is_connect(),
            })?;

        let status = resp.status();
        let resp_json: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse OpenAI response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        if !status.is_success() {
            let error_msg = resp_json["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error")
                .to_string();
            return Err(ProviderError::RequestFailed {
                message: format!("OpenAI API error: {error_msg}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(resp_json),
                retryable: status.as_u16() >= 500,
            });
        }

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
        })
    }

    /// Download the rendered image from the URL the Images API returns when
    /// `response_format` is `url` (a plain HTTPS GET of the OpenAI-hosted
    /// asset named in the `data[].url` response field — not a REST endpoint).
    ///
    /// @see <https://platform.openai.com/docs/api-reference/images/create> — response `data[].url` field
    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let resp = self.client.get(url).send().await.map_err(|e| {
            ProviderError::RequestFailed {
                message: format!("Failed to fetch image from URL: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: true,
            }
        })?;
        if !resp.status().is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Image URL returned HTTP {}", resp.status()),
                status_code: Some(resp.status().as_u16()),
                provider_error: None,
                retryable: false,
            });
        }
        let bytes = resp.bytes().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to read image bytes: {e}"),
            status_code: None,
            provider_error: None,
            retryable: false,
        })?;
        Ok(bytes.to_vec())
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
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| !c.api_key.is_empty() || self.key_pool.is_some())
    }

    /// Create an image via `POST {api_base}/v1/images/generations`.
    ///
    /// @see <https://platform.openai.com/docs/api-reference/images/create> — Images: create.
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
        let size = Self::normalize_size(size, &model_name);

        // `response_format` applies to `dall-e-2`/`dall-e-3` only: "This
        // parameter isn't supported for the GPT image models, which always
        // return base64-encoded images."
        // @see <https://developers.openai.com/api/reference> `CreateImageRequest`
        let response_format = if is_gpt_image {
            "b64_json"
        } else if extras.response_format == "b64_json" {
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

        // Build JSON body
        let mut body = json!({
            "model": model_name,
            "prompt": base.prompt,
            "size": size,
            "n": base.n.max(1),
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

        // Shallow-merge extras.extra into body (background, output_format,
        // output_compression, moderation, user — all GPT-image-only fields
        // gated by the model's extra_allowlist).
        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(body_map) = body.as_object_mut() {
                for (k, v) in extra_map {
                    body_map.insert(k.clone(), v.clone());
                }
            }
        }

        let resp = crate::providers::inject_trace_headers(
            self.client
                .post(self.api_url())
                .header("Authorization", format!("Bearer {api_key}"))
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
        let size = Self::normalize_size(
            request.size.as_deref().unwrap_or("1024x1024"),
            &model_name,
        );
        let quality = request.quality.as_deref().unwrap_or("auto");

        // GPT image pricing scales with quality and output resolution; use the
        // catalog's base cost as the anchor and scale from it rather than
        // hardcoding a per-size table that will drift.
        let pixels: f64 = size
            .split_once('x')
            .and_then(|(w, h)| Some(w.parse::<f64>().ok()? * h.parse::<f64>().ok()?))
            .unwrap_or(1024.0 * 1024.0);
        let size_factor = (pixels / (1024.0 * 1024.0)).max(1.0);
        let quality_factor = match quality {
            "low" => 0.5,
            "medium" => 1.0,
            "high" | "hd" => 2.0,
            _ => 1.0,
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
    /// @see <https://platform.openai.com/docs/api-reference/models/list> — Models: list
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

        match self
            .client
            .get("https://api.openai.com/v1/models")
            .header("Authorization", format!("Bearer {api_key}"))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
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
            size: size.map(|s| s.to_string()),
            aspect_ratio: None,
            quality: quality.map(|s| s.to_string()),
            style: style.map(|s| s.to_string()),
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "b64_json".to_string(),
            extra: None,
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
        let schema = ref_schema("openai/gpt-image-1");
        let base = make_base("a beautiful landscape", "openai/gpt-image-1");
        let extras = make_extras(Some("1024x1024"), Some("high"), None);
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
        assert_eq!(body["model"], "gpt-image-1");
        assert_eq!(body["prompt"], "a beautiful landscape");
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
    #[test]
    fn gpt_image_2_sizes_follow_the_documented_constraints() {
        for listed in ["1024x1024", "2048x2048", "2048x1152", "3840x2160", "2160x3840"] {
            assert_eq!(
                OpenAiProvider::normalize_size(listed, "gpt-image-2"),
                listed,
                "{listed} is a listed gpt-image-2 size and must pass through"
            );
        }
        for out_of_range in ["512x512", "3840x3840"] {
            assert_ne!(
                OpenAiProvider::normalize_size(out_of_range, "gpt-image-2"),
                out_of_range,
                "{out_of_range} breaks the pixel limits and must not be forwarded verbatim"
            );
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
        for id in ["openai/gpt-image-2", "openai/gpt-image-1"] {
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
        let schema = ref_schema("openai/gpt-image-1");
        let base = make_base("put a hat on the cat", "openai/gpt-image-1");
        let extras = make_extras(Some("1024x1024"), None, None);
        let materialized = crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: crate::proxy::materializer::MaterializedRefForm::MultipartField {
                    field_name: "image".to_string(),
                    bytes: bytes::Bytes::from_static(b"source-png"),
                    content_type: "image/png".to_string(),
                },
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
}
