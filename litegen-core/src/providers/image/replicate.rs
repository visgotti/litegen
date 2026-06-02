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

/// Replicate image generation provider.
///
/// Supports Flux, SDXL, SD3, and SD1.5 hosted on Replicate.
/// Uses two endpoint patterns:
///
/// - Versioned: POST /v1/predictions with version hash
/// - Official: POST /v1/models/{owner}/{name}/predictions
///
/// @see <https://replicate.com/docs/reference/http#create-a-prediction> — versioned create (`POST /v1/predictions`)
/// @see <https://replicate.com/docs/reference/http#create-a-prediction-using-an-official-model> — official create (`POST /v1/models/{owner}/{name}/predictions`)
/// @see <https://replicate.com/docs/reference/http#get-a-prediction> — poll (`GET /v1/predictions/{id}`)
pub struct ReplicateProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
}

impl ReplicateProvider {
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

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.replicate.com/v1")
    }

    fn api_key(&self) -> Result<String, ProviderError> {
        if let Some(pool) = &self.key_pool {
            return Ok(pool.next().to_string());
        }
        self.config
            .as_ref()
            .map(|c| c.api_key.clone())
            .ok_or_else(|| ProviderError::NotConfigured("replicate".into()))
    }

    /// Strip provider prefix and resolve to the Replicate owner/name (and optional version).
    /// Returns (owner/name, optional_version_hash).
    fn resolve_model_version(&self, model_id: &str) -> (String, Option<String>) {
        if let Some(cfg) = &self.config {
            if let Some(mapped) = cfg.model_mapping.get(model_id) {
                let parts: Vec<&str> = mapped.splitn(2, ':').collect();
                if parts.len() == 2 {
                    return (parts[0].to_string(), Some(parts[1].to_string()));
                }
                return (mapped.clone(), None);
            }
        }

        let native = if let Some(rest) = model_id.strip_prefix("replicate/") {
            rest
        } else {
            model_id
        };

        // Map litegen model names to Replicate owner/name pairs
        match native {
            "flux-pro" => ("black-forest-labs/flux-pro".into(), None),
            "flux-dev" => ("black-forest-labs/flux-dev".into(), None),
            "flux-schnell" => ("black-forest-labs/flux-schnell".into(), None),
            "sdxl" => (
                "stability-ai/sdxl".into(),
                Some("39ed52f2a78e934b3ba6e2a89f5b1c712de7dfea535525255b1aa35c5565e08b".into()),
            ),
            "sd3" => ("stability-ai/stable-diffusion-3".into(), None),
            "sd1.5" | "sd-1.5" => (
                "stability-ai/stable-diffusion".into(),
                Some("ac732df83cea7fff18b8472768c88ad041fa750ff7682a21affe81863cbe77e4".into()),
            ),
            _ => (native.to_string(), None),
        }
    }

    /// Build the POST URL for creating a prediction.
    /// Official models: /v1/models/{owner}/{name}/predictions
    /// Versioned models: /v1/predictions
    fn prediction_url(&self, owner_name: &str, version: &Option<String>) -> String {
        if version.is_some() {
            format!("{}/predictions", self.api_base())
        } else {
            format!("{}/models/{}/predictions", self.api_base(), owner_name)
        }
    }

    /// Poll a prediction to completion via `GET {api_base}/predictions/{id}`,
    /// reading the `status` field (`succeeded` / `failed` / `canceled`).
    ///
    /// @see <https://replicate.com/docs/reference/http#get-a-prediction> — Get a prediction
    async fn poll_prediction(
        &self,
        prediction_id: &str,
        api_key: &str,
    ) -> Result<Value, ProviderError> {
        let poll_url = format!("{}/predictions/{}", self.api_base(), prediction_id);
        let max_attempts = 120; // 120 * 2s = 4 minutes
        for _ in 0..max_attempts {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            let resp = crate::providers::inject_trace_headers(
                self.client
                    .get(&poll_url)
                    .header("Authorization", format!("Bearer {api_key}")),
            )
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: e.to_string(),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: true,
            })?;

            let result: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to parse poll response: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;

            match result["status"].as_str() {
                Some("succeeded") => return Ok(result),
                Some("failed") | Some("canceled") => {
                    let error = result["error"].as_str().unwrap_or("Unknown error").to_string();
                    return Err(ProviderError::RequestFailed {
                        message: format!("Replicate prediction failed: {error}"),
                        status_code: None,
                        provider_error: Some(result),
                        retryable: false,
                    });
                }
                _ => continue,
            }
        }

        Err(ProviderError::Timeout { timeout_ms: max_attempts * 2000 })
    }

    /// Download the rendered image from the prediction `output` URL (a plain
    /// HTTPS GET of the Replicate-hosted asset, not a REST endpoint).
    ///
    /// @see <https://replicate.com/docs/reference/http#get-a-prediction> — `output` URL field
    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let resp = self.client.get(url).send().await.map_err(|e| {
            ProviderError::RequestFailed {
                message: format!("Failed to fetch image: {e}"),
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

impl Default for ReplicateProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for ReplicateProvider {
    fn name(&self) -> &str {
        "replicate"
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

    /// Create a prediction (versioned `POST /v1/predictions` or official-model
    /// `POST /v1/models/{owner}/{name}/predictions`), then poll for the result.
    ///
    /// @see <https://replicate.com/docs/reference/http#create-a-prediction> — versioned create.
    ///   Proves the `{version, input}` body and the `id`/`status`/`output` response fields.
    /// @see <https://replicate.com/docs/reference/http#create-a-prediction-using-an-official-model> — official-model create (`{input}` body, no version)
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let api_key = self.api_key()?;
        let (owner_name, version) = self.resolve_model_version(&model.id);
        let post_url = self.prediction_url(&owner_name, &version);

        // Build input object
        let mut input = json!({
            "prompt": base.prompt,
        });

        // Aspect ratio: prefer direct, fall back to size derivation
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            input["aspect_ratio"] = Value::String(ar.to_string());
        } else if let Some(size) = extras.size.as_deref() {
            // Parse WxH into width/height for models that use those
            let parts: Vec<&str> = size.split('x').collect();
            if parts.len() == 2 {
                if let (Ok(w), Ok(h)) = (parts[0].parse::<u64>(), parts[1].parse::<u64>()) {
                    input["width"] = Value::Number(w.into());
                    input["height"] = Value::Number(h.into());
                }
            }
        }

        if let Some(seed) = base.seed {
            input["seed"] = Value::Number(seed.into());
        }

        if let Some(np) = base.negative_prompt.as_deref() {
            input["negative_prompt"] = Value::String(np.to_string());
        }

        if let Some(gc) = extras.guidance_scale {
            input["guidance_scale"] = json!(gc);
        }

        if let Some(steps) = extras.steps {
            input["num_inference_steps"] = Value::Number(steps.into());
        }

        if let Some(strength) = extras.strength {
            input["strength"] = json!(strength);
        }

        // Reference image as URL
        for r in &materialized.refs {
            match &r.form {
                MaterializedRefForm::Url(url) => {
                    input["image"] = Value::String(url.clone());
                }
                MaterializedRefForm::Base64(b64) => {
                    input["image"] = Value::String(format!("data:image/png;base64,{b64}"));
                }
                _ => {}
            }
        }

        // Shallow-merge extra
        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(input_obj) = input.as_object_mut() {
                for (k, v) in extra_map {
                    input_obj.insert(k.clone(), v.clone());
                }
            }
        }

        // Build the prediction request body
        let body = if let Some(ver) = &version {
            json!({
                "version": ver,
                "input": input
            })
        } else {
            json!({
                "input": input
            })
        };

        let resp = crate::providers::inject_trace_headers(
            self.client
                .post(&post_url)
                .header("Authorization", format!("Bearer {api_key}"))
                .header("Content-Type", "application/json")
                .json(&body),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("Replicate request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_timeout() || e.is_connect(),
        })?;

        let status = resp.status();
        let prediction: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Replicate response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        if !status.is_success() {
            let err = prediction["detail"].as_str().unwrap_or("Unknown error").to_string();
            return Err(ProviderError::RequestFailed {
                message: format!("Replicate API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(prediction),
                retryable: status.as_u16() >= 500,
            });
        }

        // Get prediction ID and poll for result
        let prediction_id = prediction["id"]
            .as_str()
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "Replicate prediction missing ID".to_string(),
                status_code: None,
                provider_error: Some(prediction.clone()),
                retryable: false,
            })?
            .to_string();

        // If already succeeded (webhooks / fast models), skip polling
        let final_result = if prediction["status"].as_str() == Some("succeeded") {
            prediction
        } else {
            self.poll_prediction(&prediction_id, &api_key).await?
        };

        // Extract output URL — Replicate returns output as array of URLs or a single URL
        let image_url = match &final_result["output"] {
            Value::Array(arr) => arr
                .first()
                .and_then(|v| v.as_str())
                .map(str::to_string),
            Value::String(s) => Some(s.clone()),
            _ => None,
        }
        .ok_or_else(|| ProviderError::RequestFailed {
            message: "Replicate output missing image URL".to_string(),
            status_code: None,
            provider_error: Some(final_result.clone()),
            retryable: false,
        })?;

        let image_bytes = self.fetch_image_bytes(&image_url).await?;

        let mut metadata = HashMap::new();
        metadata.insert("prediction_id".to_string(), Value::String(prediction_id.to_string()));
        metadata.insert("url".to_string(), Value::String(image_url));

        Ok(GenerationOutput {
            data: image_bytes,
            content_type: "image/png".to_string(),
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

    /// Validate the API token via `GET {api_base}/account`.
    ///
    /// @see <https://replicate.com/docs/reference/http#get-the-authenticated-account> — Get the authenticated account
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Replicate provider not configured".into(),
                latency_ms: None,
            };
        }
        let start = std::time::Instant::now();
        match self
            .client
            .get(format!("{}/account", self.api_base()))
            .header("Authorization", format!("Bearer {}", self.api_key().unwrap_or_default()))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "Replicate API token valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("Replicate returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("Replicate health check failed: {e}"),
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

    fn make_provider(api_base: &str) -> ReplicateProvider {
        let mut p = ReplicateProvider::new();
        p.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: "test-token".to_string(),
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
            seed: Some(123),
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        }
    }

    fn make_extras() -> ImageExtras {
        ImageExtras {
            size: None,
            aspect_ratio: Some("1:1".to_string()),
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
    async fn generates_flux_dev_via_polling() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;

        let image_url = format!("{}/img.png", image_server.uri());

        // Mock POST /v1/models/black-forest-labs/flux-dev/predictions → prediction starting
        Mock::given(method("POST"))
            .and(path("/v1/models/black-forest-labs/flux-dev/predictions"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": "pred123",
                "status": "starting",
                "input": {}
            })))
            .mount(&server)
            .await;

        // Mock GET /v1/predictions/pred123 → succeeded
        Mock::given(method("GET"))
            .and(path("/v1/predictions/pred123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "pred123",
                "status": "succeeded",
                "output": [image_url.clone()]
            })))
            .mount(&server)
            .await;

        // Mock GET to image URL
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"FAKEPNGDATA".to_vec())
                    .insert_header("content-type", "image/png"),
            )
            .mount(&image_server)
            .await;

        let provider = make_provider(&format!("{}/v1", server.uri()));
        let schema = ref_schema("replicate/flux-dev");
        let base = make_base("a futuristic city", "replicate/flux-dev");
        let extras = make_extras();
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let output = result.unwrap();
        assert_eq!(output.data, b"FAKEPNGDATA");

        // Verify the POST body had correct input structure
        let received = server.received_requests().await.unwrap();
        assert!(!received.is_empty());

        // Find the POST request
        let post_req = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        let body: Value = serde_json::from_slice(&post_req.body).unwrap();

        assert_eq!(body["input"]["prompt"], "a futuristic city");
        assert_eq!(body["input"]["aspect_ratio"], "1:1");
        assert_eq!(body["input"]["seed"], 123);
        assert_eq!(body["input"]["guidance_scale"], 3.5);
        assert_eq!(body["input"]["num_inference_steps"], 28);
    }
}
