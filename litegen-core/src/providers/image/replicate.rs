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
/// Supports FLUX, SDXL and SD3 hosted on Replicate.
/// Uses two endpoint patterns:
///
/// - Versioned: POST /v1/predictions with version hash
/// - Official: POST /v1/models/{owner}/{name}/predictions
///
/// @see <https://replicate.com/docs/reference/http#create-a-prediction> — versioned create (`POST /v1/predictions`)
/// @see <https://replicate.com/docs/reference/http#create-a-prediction-using-an-official-model> — official create (`POST /v1/models/{owner}/{name}/predictions`)
/// @see <https://replicate.com/docs/reference/http#get-a-prediction> — poll (`GET /v1/predictions/{id}`)
/// What litegen itself sends in a prediction's `input`; `extra` cannot
/// replace it.
const OWNED_FIELDS: &[&str] = &[
    "prompt", "aspect_ratio", "width", "height", "seed", "negative_prompt", "guidance", "guidance_scale", "cfg",
    "steps", "num_inference_steps", "prompt_strength", "image", "output_format",
];

pub struct ReplicateProvider {
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

impl ReplicateProvider {
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
            // Official model, addressed by owner/name with no version.
            // @see <https://replicate.com/black-forest-labs/flux-1.1-pro/api/schema>
            "flux-1.1-pro" => ("black-forest-labs/flux-1.1-pro".into(), None),
            "flux-dev" => ("black-forest-labs/flux-dev".into(), None),
            "flux-schnell" => ("black-forest-labs/flux-schnell".into(), None),
            "sdxl" => (
                "stability-ai/sdxl".into(),
                Some("39ed52f2a78e934b3ba6e2a89f5b1c712de7dfea535525255b1aa35c5565e08b".into()),
            ),
            "sd3" => ("stability-ai/stable-diffusion-3".into(), None),
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

    /// The `(guidance, steps)` input field names of a Replicate model. Models
    /// spell these controls differently, and an input key that a model's schema
    /// does not declare never reaches it — so the generic names silently dropped
    /// the caller's values on BFL's FLUX models and on SD3.
    ///
    /// @see <https://replicate.com/black-forest-labs/flux-dev/api/schema> — `guidance`, `num_inference_steps`
    /// @see <https://replicate.com/black-forest-labs/flux-pro/api/schema> — `guidance`, `steps` (deprecated)
    /// @see <https://replicate.com/stability-ai/stable-diffusion-3/api/schema> — `cfg`, `steps`
    /// @see <https://replicate.com/stability-ai/sdxl/versions> — `guidance_scale`, `num_inference_steps`
    fn input_field_names(owner_name: &str) -> (&'static str, &'static str) {
        match owner_name {
            "black-forest-labs/flux-pro" => ("guidance", "steps"),
            "black-forest-labs/flux-dev" => ("guidance", "num_inference_steps"),
            "stability-ai/stable-diffusion-3" => ("cfg", "steps"),
            // stability-ai/sdxl, and black-forest-labs/flux-schnell (whose
            // steps field is `num_inference_steps`; it has no guidance input).
            _ => ("guidance_scale", "num_inference_steps"),
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

            let resp = match crate::providers::inject_trace_headers(
                self.client
                    .get(&poll_url)
                    .header("Authorization", format!("Bearer {api_key}")),
            )
            .send()
            .await
            {
                Ok(resp) => resp,
                // The prediction is Replicate's now; a dropped poll is asked again.
                Err(_) => continue,
            };
            // A 5xx or 429 is asked again; any other error status is the answer.
            let result = match crate::providers::video::read_poll_json(resp, "replicate").await {
                Ok(result) => result,
                Err(ProviderError::RequestFailed { retryable: true, .. }) => continue,
                Err(e) => return Err(e),
            };

            match result["status"].as_str() {
                Some("succeeded") => return Ok(result),
                Some("starting") | Some("processing") => continue,
                // failed, canceled, aborted ("terminated before it started
                // running") — or a status outside the documented enum.
                other => {
                    let status = other.unwrap_or("with no status");
                    let message = match result["error"].as_str().filter(|e| !e.is_empty()) {
                        Some(e) => format!("Replicate prediction {status}: {e}"),
                        None => format!("Replicate prediction {status}"),
                    };
                    return Err(ProviderError::RequestFailed {
                        message,
                        status_code: None,
                        provider_error: Some(result),
                        retryable: false,
                    });
                }
            }
        }

        Err(ProviderError::Timeout { timeout_ms: max_attempts * 2000 })
    }

    /// Download the rendered image from the prediction `output` URL (a plain
    /// HTTPS GET of the Replicate-hosted asset, not a REST endpoint).
    ///
    /// @see <https://replicate.com/docs/reference/http#get-a-prediction> — `output` URL field
    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        super::fetch_result(&self.asset_client, url, self.public_assets_only, "replicate image").await
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
            self.key_pool = Some(ApiKeyPool::shared(config.api_keys.clone()));
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

        let (guidance_field, steps_field) = Self::input_field_names(&owner_name);

        if let Some(gc) = extras.guidance_scale {
            input[guidance_field] = json!(gc);
        }

        if let Some(steps) = extras.steps {
            input[steps_field] = Value::Number(steps.into());
        }

        // img2img strength: "prompt_strength — Prompt strength when using img2img".
        // @see <https://replicate.com/black-forest-labs/flux-dev/api/schema> (2026-10-30)
        if let Some(strength) = extras.strength {
            input["prompt_strength"] = json!(strength);
        }

        // Reference image as a URL, or a data URI of its own type.
        for r in &materialized.refs {
            match &r.form {
                MaterializedRefForm::Url(url) => {
                    input["image"] = Value::String(url.clone());
                }
                MaterializedRefForm::Base64(b64) => {
                    input["image"] = Value::String(crate::proxy::materializer::base64_data_uri(b64, Some("image/png")));
                }
                _ => {}
            }
        }

        // Shallow-merge extra, never over what litegen sends.
        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(input_obj) = input.as_object_mut() {
                for (k, v) in extra_map.iter().filter(|(k, _)| !OWNED_FIELDS.contains(&k.as_str())) {
                    input_obj.insert(k.clone(), v.clone());
                }
            }
        }
        // PixelGenie L9: the asked container, in Replicate's spelling
        // ("webp", "jpg", "png").
        if let Some(f) = extras.output_format.as_deref() {
            input["output_format"] = Value::String(if f == "jpeg" { "jpg" } else { f }.to_string());
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
            // Only a connection that never opened is safe to retry: a request
            // that timed out may have created, and billed, a prediction.
            retryable: e.is_connect(),
        })?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let prediction: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let err = prediction["detail"].as_str().map(str::to_string).unwrap_or_else(|| text.chars().take(200).collect());
            return Err(ProviderError::RequestFailed {
                message: format!("Replicate API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(prediction),
                // Turned away, not run: "If you hit a limit, you will receive a
                // response with status `429`".
                // @see <https://replicate.com/docs/topics/predictions/rate-limits>
                retryable: status.as_u16() == 429 || status.is_server_error(),
            });
        }
        let prediction: Value = serde_json::from_str(&text).map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Replicate response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

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
        // Accepted: from here on a failure must not re-send the job (L1).
        extras.vendor_job.record(prediction_id.clone());

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
        p.public_assets_only = false;
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
            vendor_job: Default::default(),
            size: None,
            aspect_ratio: Some("1:1".to_string()),
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
        let mut extras = make_extras();
        extras.output_format = Some("jpeg".into());
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        // The vendor accepted the job before the poll: the router must never re-send it.
        assert_eq!(extras.vendor_job.get().as_deref(), Some("pred123"));
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
        assert_eq!(body["input"]["output_format"], "jpg", "PixelGenie L9: Replicate spells jpeg jpg");
        // flux-dev's guidance field is `guidance`, not `guidance_scale`.
        assert_eq!(body["input"]["guidance"], 3.5);
        assert!(body["input"].get("guidance_scale").is_none());
        assert_eq!(body["input"]["num_inference_steps"], 28);
    }

    /// Replicate models name guidance and step count differently, and an input
    /// key a model's schema does not declare never reaches it. The generic
    /// `guidance_scale` / `num_inference_steps` therefore dropped the caller's
    /// values on FLUX (`guidance`) and on SD3 (`cfg`, `steps`).
    /// @see <https://replicate.com/black-forest-labs/flux-dev/api/schema>
    /// @see <https://replicate.com/black-forest-labs/flux-pro/api/schema>
    /// @see <https://replicate.com/stability-ai/stable-diffusion-3/api/schema>
    #[tokio::test]
    async fn guidance_and_steps_use_each_models_own_input_names() {
        for (catalog_id, slug, guidance_key, steps_key) in [
            (
                "replicate/flux-dev",
                "black-forest-labs/flux-dev",
                "guidance",
                "num_inference_steps",
            ),
            (
                "replicate/flux-pro",
                "black-forest-labs/flux-pro",
                "guidance",
                "steps",
            ),
            (
                "replicate/sd3",
                "stability-ai/stable-diffusion-3",
                "cfg",
                "steps",
            ),
        ] {
            let server = MockServer::start().await;
            // An already-succeeded prediction skips the 2s polling loop.
            Mock::given(method("POST"))
                .and(path(format!("/v1/models/{slug}/predictions")))
                .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                    "id": "pred-1",
                    "status": "succeeded",
                    "output": [format!("{}/img.png", server.uri())]
                })))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/img.png"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(b"PNG".to_vec()))
                .mount(&server)
                .await;

            let provider = make_provider(&format!("{}/v1", server.uri()));
            let schema = ref_schema(catalog_id);
            let base = make_base("a lighthouse at dusk", catalog_id);
            provider
                .generate(&schema, &base, &make_extras(), &empty_materialized())
                .await
                .expect("generate");

            let received = server.received_requests().await.unwrap();
            let post = received
                .iter()
                .find(|r| r.method == wiremock::http::Method::POST)
                .unwrap();
            let body: Value = serde_json::from_slice(&post.body).unwrap();
            let input = &body["input"];
            assert_eq!(
                input[guidance_key], 3.5,
                "{catalog_id}: guidance belongs in `{guidance_key}`: {input}"
            );
            assert_eq!(
                input[steps_key], 28,
                "{catalog_id}: steps belong in `{steps_key}`: {input}"
            );
            for generic in ["guidance_scale", "num_inference_steps"] {
                if generic != guidance_key && generic != steps_key {
                    assert!(
                        input.get(generic).is_none(),
                        "{catalog_id} declares no `{generic}` input: {input}"
                    );
                }
            }
        }
    }

    /// `replicate/flux-1.1-pro` is BFL's official `black-forest-labs/flux-1.1-pro`
    /// model. Without its own arm the bare `flux-1.1-pro` fell through as the
    /// owner/name, so the prediction went to `/v1/models/flux-1.1-pro/predictions`,
    /// which does not exist.
    /// @see <https://replicate.com/black-forest-labs/flux-1.1-pro/api/schema>
    #[tokio::test]
    async fn flux_1_1_pro_posts_to_the_official_black_forest_labs_model() {
        let provider = ReplicateProvider::new();
        assert_eq!(
            provider.resolve_model_version("replicate/flux-1.1-pro"),
            ("black-forest-labs/flux-1.1-pro".to_string(), None),
        );

        let server = MockServer::start().await;
        // An already-succeeded prediction skips the 2s polling loop.
        Mock::given(method("POST"))
            .and(path(
                "/v1/models/black-forest-labs/flux-1.1-pro/predictions",
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": "pred-1",
                "status": "succeeded",
                "output": format!("{}/img.webp", server.uri())
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/img.webp"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"WEBP".to_vec()))
            .mount(&server)
            .await;

        let provider = make_provider(&format!("{}/v1", server.uri()));
        let schema = ref_schema("replicate/flux-1.1-pro");
        let base = make_base("a lighthouse at dusk", "replicate/flux-1.1-pro");
        let extras = ImageExtras {
            vendor_job: Default::default(),
            steps: None,
            guidance_scale: None,
            ..make_extras()
        };
        provider
            .generate(&schema, &base, &extras, &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let post = received
            .iter()
            .find(|r| r.method == wiremock::http::Method::POST)
            .unwrap();
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert!(
            body.get("version").is_none(),
            "official models take no version: {body}"
        );
        assert_eq!(body["input"]["prompt"], "a lighthouse at dusk");
        assert_eq!(body["input"]["aspect_ratio"], "1:1");
    }

    // ─── Replicate audit, 2026-10-30 ────────────────────────────────────────

    /// Replicate answers every create with a 422, so only the outbound body matters.
    async fn refusing_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({ "detail": "stop" })))
            .mount(&server)
            .await;
        server
    }

    async fn sent_input(server: &MockServer) -> Value {
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        body["input"].clone()
    }

    /// #24: img2img strength is `prompt_strength` ("Prompt strength when using
    /// img2img"); a base64 reference goes with its own type; `extra` never
    /// replaces what litegen sends.
    /// @see <https://replicate.com/black-forest-labs/flux-dev/api/schema> (2026-10-30)
    #[tokio::test]
    async fn the_input_says_what_each_model_takes_and_extra_cannot_replace_it() {
        use base64::Engine;
        let server = refusing_server().await;
        let mut extras = make_extras();
        extras.strength = Some(0.6);
        extras.extra = Some(json!({ "prompt": "other", "image": "https://x/y.png", "seed": 9, "output_quality": 90 }));
        let refs = crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".into(),
                form: MaterializedRefForm::Base64(base64::engine::general_purpose::STANDARD.encode([0xFF, 0xD8, 0xFF, 0xE0, 0, 16])),
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };
        let _ = make_provider(&server.uri())
            .generate(&ref_schema("replicate/flux-dev"), &make_base("a red kite", "replicate/flux-dev"), &extras, &refs)
            .await;
        let input = sent_input(&server).await;
        assert_eq!(input["prompt_strength"], 0.6);
        assert!(input.get("strength").is_none(), "{input}");
        assert!(input["image"].as_str().unwrap().starts_with("data:image/jpeg;base64,"), "{input}");
        assert_eq!(input["prompt"], "a red kite");
        assert_eq!(input["seed"], 123);
        assert_eq!(input["output_quality"], 90);
    }

    /// #17, #18: a create that timed out may have been accepted and billed, so
    /// it is never sent again; a 429 ("Request was throttled") may be.
    /// @see <https://replicate.com/docs/topics/predictions/rate-limits> (2026-10-30)
    #[tokio::test]
    async fn a_timed_out_create_is_never_sent_again_and_a_throttled_one_may_be() {
        let slow = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "p" })).set_delay(std::time::Duration::from_secs(5)))
            .mount(&slow)
            .await;
        let mut p = make_provider(&slow.uri());
        p.client = reqwest::Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        let schema = ref_schema("replicate/flux-dev");
        let base = make_base("x", "replicate/flux-dev");
        let err = p.generate(&schema, &base, &make_extras(), &empty_materialized()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");

        let throttled = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({ "detail": "Request was throttled. Your rate limit resets in ~30s." })))
            .mount(&throttled)
            .await;
        let err = make_provider(&throttled.uri()).generate(&schema, &base, &make_extras(), &empty_materialized()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: true, status_code: Some(429), .. }), "{err:?}");
    }

    /// #7: "`aborted` means the prediction was terminated before it started
    /// running" — an ending, not a prediction still to come.
    /// @see <https://api.replicate.com/openapi.json> (2026-10-30)
    #[tokio::test]
    async fn an_aborted_prediction_is_an_ending() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "p1", "status": "starting" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/predictions/p1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "p1", "status": "aborted" })))
            .mount(&server)
            .await;
        let started = std::time::Instant::now();
        let err = make_provider(&server.uri())
            .generate(&ref_schema("replicate/flux-dev"), &make_base("x", "replicate/flux-dev"), &make_extras(), &empty_materialized())
            .await
            .unwrap_err();
        assert!(matches!(&err, ProviderError::RequestFailed { retryable: false, message, .. } if message.contains("aborted")), "{err:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "polled past an aborted prediction");
    }

    /// #23: the result is fetched without the key, only from a public address,
    /// and at most 64 MiB.
    #[tokio::test]
    async fn a_result_is_fetched_only_from_a_public_address() {
        let server = MockServer::start().await;
        let cdn = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": "p1", "status": "succeeded", "output": [format!("{}/out-0.webp", cdn.uri())]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"RIFF\x24\x00\x00\x00WEBP".to_vec())).mount(&cdn).await;
        let mut provider = make_provider(&server.uri());
        provider.public_assets_only = true;
        let err = provider
            .generate(&ref_schema("replicate/flux-dev"), &make_base("x", "replicate/flux-dev"), &make_extras(), &empty_materialized())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
        assert!(cdn.received_requests().await.unwrap().is_empty(), "a private address was fetched");
    }
}
