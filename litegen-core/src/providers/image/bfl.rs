use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// Black Forest Labs (FLUX) direct image provider.
///
/// Asynchronous: `POST /v1/{model}` returns `{id, polling_url}`; poll the
/// returned `polling_url` until `status == "Ready"`, then download
/// `result.sample` (a signed URL valid ~10 min). Authenticated with the `x-key`
/// header. Image-only (BFL has no first-party video API).
///
/// @see <https://docs.bfl.ai/flux_models/flux_1_1_pro> — generate + poll
///   Verbatim: "curl -X POST 'https://api.bfl.ai/v1/flux-pro-1.1' -H \"x-key: ${BFL_API_KEY}\" -H 'Content-Type: application/json' -d '{ \"prompt\": ..., \"width\": 1024, \"height\": 1024 }'"
///   Verbatim: "echo \"Image ready: $(echo $result | jq -r .result.sample)\""
/// @see <https://docs.bfl.ai/kontext/kontext_image_editing> — input_image (base64) editing
pub struct BflProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl BflProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::raw_header("x-key"),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.bfl.ai")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("bfl".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// The native model id is the endpoint path segment (e.g. `flux-pro-1.1`).
    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("bfl/").unwrap_or(model_id)
    }

    async fn poll_result(&self, polling_url: &str) -> Result<Value, ProviderError> {
        let creds = self.creds()?;
        let max_attempts = 120; // 120 * 2s = 4 min
        for _ in 0..max_attempts {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(polling_url))?;
            let resp = crate::providers::inject_trace_headers(builder)
                .send()
                .await
                .map_err(|e| ProviderError::RequestFailed {
                    message: e.to_string(),
                    status_code: e.status().map(|s| s.as_u16()),
                    provider_error: None,
                    retryable: true,
                })?;
            let data: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to parse BFL poll response: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;
            match data["status"].as_str() {
                Some("Ready") => return Ok(data),
                Some("Error") | Some("Content Moderated") | Some("Request Moderated") | Some("Task not found") => {
                    return Err(ProviderError::RequestFailed {
                        message: format!("BFL generation failed: {}", data["status"].as_str().unwrap_or("error")),
                        status_code: None,
                        provider_error: Some(data),
                        retryable: false,
                    });
                }
                _ => continue, // Pending
            }
        }
        Err(ProviderError::Timeout { timeout_ms: max_attempts * 2000 })
    }

    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let resp = self.client.get(url).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to fetch BFL image: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: true,
        })?;
        if !resp.status().is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("BFL image URL returned HTTP {}", resp.status()),
                status_code: Some(resp.status().as_u16()),
                provider_error: None,
                retryable: false,
            });
        }
        Ok(resp
            .bytes()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to read BFL image bytes: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
            .to_vec())
    }
}

impl Default for BflProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for BflProvider {
    fn name(&self) -> &str {
        "bfl"
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
            .is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.key_pool.is_some())
    }

    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);
        let url = format!("{}/v1/{native}", self.api_base());

        let mut body = json!({ "prompt": base.prompt });
        // Width/height from size; else aspect_ratio for ultra/kontext models.
        if let Some(size) = extras.size.as_deref() {
            if let Some((w, h)) = size.split_once('x') {
                if let (Ok(w), Ok(h)) = (w.parse::<u64>(), h.parse::<u64>()) {
                    body["width"] = Value::Number(w.into());
                    body["height"] = Value::Number(h.into());
                }
            }
        } else if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }
        body["output_format"] = Value::String("png".to_string());

        // Reference images: input_image (Kontext edit base) or image_prompt (Redux).
        for r in &materialized.refs {
            if let MaterializedRefForm::Base64(b64) = &r.form {
                match r.role.as_str() {
                    "redux" | "image_prompt" => body["image_prompt"] = Value::String(b64.clone()),
                    _ => body["input_image"] = Value::String(b64.clone()),
                }
            } else if let MaterializedRefForm::Url(u) = &r.form {
                body["input_image"] = Value::String(u.clone());
            }
        }

        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(obj) = body.as_object_mut() {
                for (k, v) in extra_map {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("BFL request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?;

        let status = resp.status();
        let submit: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse BFL response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        if !status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("BFL API error: {}", submit),
                status_code: Some(status.as_u16()),
                provider_error: Some(submit),
                retryable: status.as_u16() >= 500,
            });
        }

        let id = submit["id"].as_str().unwrap_or_default().to_string();
        // Prefer the returned polling_url; fall back to the documented path.
        let polling_url = submit["polling_url"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("{}/v1/get_result?id={id}", self.api_base()));

        let result = self.poll_result(&polling_url).await?;
        let sample = result["result"]["sample"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "BFL result missing result.sample".to_string(),
            status_code: None,
            provider_error: Some(result.clone()),
            retryable: false,
        })?;

        let bytes = self.fetch_image_bytes(sample).await?;
        let mut metadata = HashMap::new();
        metadata.insert("id".to_string(), Value::String(id));
        metadata.insert("url".to_string(), Value::String(sample.to_string()));

        Ok(GenerationOutput {
            data: bytes,
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

    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "BFL provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult {
            healthy: true,
            message: "BFL provider configured".into(),
            latency_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> BflProvider {
        let mut p = BflProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_key: "bfl-key".to_string(),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        cfg.credentials.api_key = Some("bfl-key".to_string());
        p.configure(cfg);
        p
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(),
            model: model.to_string(),
            n: 1,
            negative_prompt: None,
            seed: Some(7),
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
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".to_string(),
            extra: None,
        }
    }

    #[tokio::test]
    async fn generates_flux_pro_via_polling_url() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/sample.png", image_server.uri());
        let polling_url = format!("{}/v1/get_result", server.uri());

        Mock::given(method("POST"))
            .and(path("/v1/flux-pro-1.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "bfl-1", "polling_url": format!("{}?id=bfl-1", polling_url)
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/get_result"))
            .and(query_param("id", "bfl-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "Ready", "result": { "sample": image_url.clone() }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"FLUXPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("bfl/flux-pro-1.1");
        let base = make_base("a cyberpunk fox", "bfl/flux-pro-1.1");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"FLUXPNG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("x-key").unwrap(), "bfl-key");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["prompt"], "a cyberpunk fox");
        assert_eq!(body["width"], 1024);
        assert_eq!(body["height"], 1024);
        assert_eq!(body["seed"], 7);
    }
}
