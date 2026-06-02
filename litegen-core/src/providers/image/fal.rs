use async_trait::async_trait;
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
    fn resolve_endpoint(&self, model_id: &str) -> &'static str {
        if let Some(cfg) = &self.config {
            if cfg.model_mapping.contains_key(model_id) {
                // Can't return from model_mapping as static str; use the static default below
            }
        }

        let native = if let Some(rest) = model_id.strip_prefix("fal/") {
            rest
        } else {
            model_id
        };

        match native {
            "flux-schnell" => "fal-ai/flux/schnell",
            "flux-dev" => "fal-ai/flux/dev",
            "flux-pro" => "fal-ai/flux-pro/v1.1",
            "sdxl" | "sdxl-lightning" => "fal-ai/fast-sdxl",
            "sd35-medium" | "sd3.5-medium" => "fal-ai/stable-diffusion-v35-medium",
            "recraft-v3" => "fal-ai/recraft-v3",
            "auraflow" | "aura-flow" => "fal-ai/aura-flow",
            _ => "fal-ai/flux/dev", // fallback
        }
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
            self.key_pool = Some(ApiKeyPool::new(config.api_keys.clone()));
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
        let endpoint = self.resolve_endpoint(&model.id);
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
            size: Some("1024x1024".to_string()),
            aspect_ratio: None,
            quality: None,
            style: None,
            steps: Some(28),
            guidance_scale: Some(3.5),
            strength: None,
            response_format: "url".to_string(),
            extra: None,
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
        let extras = make_extras();
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
    }
}
