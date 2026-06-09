use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    BaseGenerationRequest, CredentialPool, GenerationOutput, HealthCheckResult, ImageExtras, ImageProvider,
    ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// Kling (Kuaishou) image generation provider.
///
/// Async: `POST /v1/images/generations` returns `data.task_id`; poll
/// `GET /v1/images/generations/{task_id}` until `data.task_status == "succeed"`,
/// then download `data.task_result.images[0].url`. Auth: a per-request HS256
/// JWT minted from the account access key (key_id) + secret key (key_secret),
/// sent as `Authorization: Bearer <jwt>` (the [`AuthSpec::KlingJwt`] scheme).
///
/// @see <https://app.klingai.com/global/dev/document-api/apiReference/model/imageToVideo>
///   (auth: HS256 JWT iss=access_key, exp=+1800, nbf=-5; response envelope
///   {code, message, data:{task_id, task_status, task_result}})
pub struct KlingImageProvider {
    config: Option<ProviderInstanceConfig>,
    cred_pool: Option<CredentialPool>,
    auth: AuthSpec,
    client: Client,
}

impl KlingImageProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            cred_pool: None,
            auth: AuthSpec::KlingJwt,
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
            .unwrap_or("https://api.klingai.com")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("kling".into()))?;
        if let Some(pool) = &self.cred_pool {
            return Ok(base.with_signing(pool.next()));
        }
        Ok(base)
    }

    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("kling/").unwrap_or(model_id)
    }

    async fn poll_task(&self, task_id: &str) -> Result<Value, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/v1/images/generations/{}", self.api_base(), task_id);
        let max_attempts = 90;
        for _ in 0..max_attempts {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(&url))?;
            let data: Value = crate::providers::inject_trace_headers(builder)
                .send()
                .await
                .map_err(|e| ProviderError::RequestFailed {
                    message: e.to_string(),
                    status_code: None,
                    provider_error: None,
                    retryable: true,
                })?
                .json()
                .await
                .unwrap_or_default();
            match data["data"]["task_status"].as_str() {
                Some("succeed") => return Ok(data),
                Some("failed") => {
                    return Err(ProviderError::RequestFailed {
                        message: format!("Kling task failed: {}", data["data"]["task_status_msg"].as_str().unwrap_or("")),
                        status_code: None,
                        provider_error: Some(data),
                        retryable: false,
                    })
                }
                _ => continue,
            }
        }
        Err(ProviderError::Timeout { timeout_ms: max_attempts * 2000 })
    }

    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let resp = self.client.get(url).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to fetch Kling image: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;
        Ok(resp
            .bytes()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to read Kling image: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
            .to_vec())
    }
}

impl Default for KlingImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for KlingImageProvider {
    fn name(&self) -> &str {
        "kling"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.credentials.credential_sets.is_empty() {
            self.cred_pool = Some(CredentialPool::shared(config.credentials.credential_sets.clone()));
        }
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.cred_pool.is_some())
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
        let url = format!("{}/v1/images/generations", self.api_base());

        let mut body = json!({
            "model_name": native,
            "prompt": base.prompt,
            "n": base.n.max(1),
        });
        if let Some(np) = base.negative_prompt.as_deref() {
            body["negative_prompt"] = Value::String(np.to_string());
        }
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }
        // Reference image (subject) as base64 (no data: prefix) or URL.
        for r in &materialized.refs {
            match &r.form {
                MaterializedRefForm::Base64(b64) => body["image"] = Value::String(b64.clone()),
                MaterializedRefForm::Url(u) => body["image"] = Value::String(u.clone()),
                _ => {}
            }
        }
        if let Some(Value::Object(map)) = &extras.extra {
            if let Some(obj) = body.as_object_mut() {
                for (k, v) in map {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let submit: Value = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Kling request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?
            .json()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to parse Kling response: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;
        if submit["code"].as_i64().unwrap_or(-1) != 0 {
            return Err(ProviderError::RequestFailed {
                message: format!("Kling API error: {}", submit["message"].as_str().unwrap_or("unknown")),
                status_code: None,
                provider_error: Some(submit),
                retryable: false,
            });
        }

        let task_id = submit["data"]["task_id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Kling response missing data.task_id".to_string(),
            status_code: None,
            provider_error: Some(submit.clone()),
            retryable: false,
        })?;

        let result = self.poll_task(task_id).await?;
        let image_url = result["data"]["task_result"]["images"][0]["url"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Kling result missing task_result.images[0].url".to_string(),
            status_code: None,
            provider_error: Some(result.clone()),
            retryable: false,
        })?;

        let bytes = self.fetch_image_bytes(image_url).await?;
        let mut metadata = HashMap::new();
        metadata.insert("task_id".to_string(), Value::String(task_id.to_string()));
        metadata.insert("url".to_string(), Value::String(image_url.to_string()));

        Ok(GenerationOutput { data: bytes, content_type: "image/png".to_string(), metadata })
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
                message: "Kling provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Kling provider configured".into(), latency_ms: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> KlingImageProvider {
        let mut p = KlingImageProvider::new();
        let mut cfg = ProviderInstanceConfig { api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.key_id = Some("ak-test".to_string());
        cfg.credentials.key_secret = Some("sk-test".to_string());
        p.configure(cfg);
        p
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(), model: model.to_string(), n: 1, negative_prompt: None,
            seed: None, reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    fn make_extras() -> ImageExtras {
        ImageExtras {
            size: None, aspect_ratio: Some("16:9".to_string()), quality: None, style: None,
            steps: None, guidance_scale: None, strength: None, response_format: "url".to_string(), extra: None,
        }
    }

    #[tokio::test]
    async fn generates_with_jwt_auth_and_poll() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/k.png", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/v1/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0, "message": "SUCCEED", "data": { "task_id": "kl-1", "task_status": "submitted" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/v1/images/generations/kl-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0, "data": { "task_status": "succeed",
                    "task_result": { "images": [{ "index": 0, "url": image_url.clone() }] } }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"KLINGPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("kling/kling-v2");
        let base = make_base("a jade dragon", "kling/kling-v2");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"KLINGPNG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        // JWT Bearer auth present.
        let auth = post.headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("Bearer ey"), "expected JWT bearer, got {auth}");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["model_name"], "kling-v2");
        assert_eq!(body["prompt"], "a jade dragon");
    }
}
