use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::MaterializedRequest;
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::{CostEstimate, CostSource, ImageGenerationRequest};

/// OpenAI DALL-E image generation provider.
/// Supports DALL-E 2 and DALL-E 3 via the OpenAI API.
///
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

    fn normalize_size(size: &str, is_dalle3: bool) -> &'static str {
        let parts: Vec<&str> = size.split('x').collect();
        let (w, h) = if parts.len() == 2 {
            (
                parts[0].parse::<u32>().unwrap_or(1024),
                parts[1].parse::<u32>().unwrap_or(1024),
            )
        } else {
            (1024, 1024)
        };

        if is_dalle3 {
            if w > h && w >= 1792 {
                "1792x1024"
            } else if h > w && h >= 1792 {
                "1024x1792"
            } else {
                "1024x1024"
            }
        } else {
            if w <= 256 || h <= 256 {
                "256x256"
            } else if w <= 512 || h <= 512 {
                "512x512"
            } else {
                "1024x1024"
            }
        }
    }

    fn is_dalle3(model: &str) -> bool {
        let m = model.to_lowercase();
        m.contains("dall-e-3") || m.contains("dalle-3") || m.contains("dalle3")
    }

    /// Strip the provider prefix (e.g. "openai/dall-e-3" → "dall-e-3").
    fn native_model_id(model_id: &str) -> &str {
        if let Some(rest) = model_id.strip_prefix("openai/") {
            rest
        } else {
            model_id
        }
    }

    fn resolve_model_name(&self, model: &str) -> String {
        if let Some(cfg) = &self.config {
            if let Some(mapped) = cfg.model_mapping.get(model) {
                return mapped.clone();
            }
        }
        let native = Self::native_model_id(model);
        // If it's already a native name, return as-is; otherwise guess
        if native.starts_with("dall-e-") {
            native.to_string()
        } else if Self::is_dalle3(native) {
            "dall-e-3".to_string()
        } else {
            "dall-e-2".to_string()
        }
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
        _materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let api_key = self.api_key()?;
        let model_name = self.resolve_model_name(&model.id);
        let is_dalle3 = Self::is_dalle3(&model_name);

        // Determine size
        let size = extras.size.as_deref().unwrap_or("1024x1024");
        let size = Self::normalize_size(size, is_dalle3);

        // Determine response_format: if caller asked for b64_json use that, otherwise url
        let response_format = if extras.response_format == "b64_json" {
            "b64_json"
        } else {
            "url"
        };

        // Build JSON body
        let mut body = json!({
            "model": model_name,
            "prompt": base.prompt,
            "size": size,
            "n": base.n.max(1),
            "response_format": response_format,
        });

        // OpenAI DALL-E 3 supports quality and style
        if is_dalle3 {
            if let Some(q) = extras.quality.as_deref() {
                body["quality"] = Value::String(q.to_string());
            }
            if let Some(s) = extras.style.as_deref() {
                body["style"] = Value::String(s.to_string());
            }
        }

        // For DALL-E 2 image-to-image: check if we have a multipart ref — not handled here
        // (DALL-E 2 edits use a different endpoint; we do text-to-image only here).
        // Check for base64 refs in materialized (DALL-E 2 edit path is not needed for Task 10).

        // Shallow-merge extras.extra into body
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
        )
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

        // Get image bytes
        let (image_bytes, content_type) = if response_format == "b64_json" {
            let b64 = data["b64_json"].as_str().ok_or_else(|| ProviderError::RequestFailed {
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

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        request: &ImageGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        let model_name = self.resolve_model_name(&model.id);
        let is_dalle3 = Self::is_dalle3(&model_name);
        let size = Self::normalize_size(
            request.size.as_deref().unwrap_or("1024x1024"),
            is_dalle3,
        );
        let is_hd = matches!(request.quality.as_deref(), Some("hd") | Some("high"));

        let base_cost = if is_dalle3 {
            match (size, is_hd) {
                ("1024x1024", false) => 0.040,
                ("1024x1024", true) => 0.080,
                ("1792x1024" | "1024x1792", false) => 0.080,
                ("1792x1024" | "1024x1792", true) => 0.120,
                _ => 0.040,
            }
        } else {
            match size {
                "256x256" => 0.016,
                "512x512" => 0.018,
                "1024x1024" => 0.020,
                _ => 0.020,
            }
        };

        Ok(build_cost_estimate(
            base_cost,
            0.0,
            CostSource::Estimated,
            Some(json!({
                "model": model_name,
                "size": size,
                "quality": if is_hd { "hd" } else { "standard" },
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
        let schema = ref_schema("openai/dall-e-3");
        let base = make_base("a beautiful landscape", "openai/dall-e-3");
        let extras = make_extras(Some("1024x1024"), Some("standard"), Some("vivid"));
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
        assert_eq!(body["model"], "dall-e-3");
        assert_eq!(body["prompt"], "a beautiful landscape");
        assert_eq!(body["size"], "1024x1024");
        assert_eq!(body["quality"], "standard");
        assert_eq!(body["style"], "vivid");
        assert_eq!(body["n"], 1);
        assert_eq!(body["response_format"], "b64_json");
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
}
