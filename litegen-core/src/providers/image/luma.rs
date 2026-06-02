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

/// Luma Photon image generation provider (Dream Machine API).
///
/// Shares the Luma Dream Machine host and Bearer auth with the Luma video
/// provider. Generation is asynchronous: `POST /generations/image` returns a
/// job id; poll `GET /generations/{id}` until `state == completed`, then read
/// `assets.image` (a CDN URL) and download the bytes.
///
/// Reference images are passed as CDN URLs only (no base64/multipart) — the
/// materializer uploads any base64/blob refs to storage first.
///
/// @see <https://docs.lumalabs.ai/reference/generateimage> — `POST /generations/image`
///   Verbatim: "post https://api.lumalabs.ai/dream-machine/v1/generations/image"
/// @see <https://docs.lumalabs.ai/docs/image-generation> — models + ref roles
///   Verbatim: "You can choose from our two model versions: photon-1 (default) photon-flash-1"
///   Verbatim: "You should upload and use your own cdn image urls, currently this is the only way to pass an image"
pub struct LumaImageProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl LumaImageProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
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
            .unwrap_or("https://api.lumalabs.ai/dream-machine/v1")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("luma".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    fn resolve_model(model_id: &str) -> &str {
        match model_id.strip_prefix("luma/").unwrap_or(model_id) {
            "photon-flash-1" => "photon-flash-1",
            _ => "photon-1",
        }
    }

    async fn poll_generation(&self, id: &str) -> Result<Value, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/generations/{}", self.api_base(), id);
        let max_attempts = 90; // 90 * 2s = 3 min
        for _ in 0..max_attempts {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(&url))?;
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
                message: format!("Failed to parse Luma poll response: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;
            match data["state"].as_str() {
                Some("completed") => return Ok(data),
                Some("failed") => {
                    let err = data["failure_reason"].as_str().unwrap_or("Unknown error").to_string();
                    return Err(ProviderError::RequestFailed {
                        message: format!("Luma image generation failed: {err}"),
                        status_code: None,
                        provider_error: Some(data),
                        retryable: false,
                    });
                }
                _ => continue,
            }
        }
        Err(ProviderError::Timeout { timeout_ms: max_attempts * 2000 })
    }

    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let resp = self.client.get(url).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to fetch Luma image: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: true,
        })?;
        if !resp.status().is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Luma image URL returned HTTP {}", resp.status()),
                status_code: Some(resp.status().as_u16()),
                provider_error: None,
                retryable: false,
            });
        }
        Ok(resp
            .bytes()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to read Luma image bytes: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
            .to_vec())
    }
}

impl Default for LumaImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for LumaImageProvider {
    fn name(&self) -> &str {
        "luma"
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

    /// Submit `POST /generations/image`, poll to completion, fetch the bytes.
    ///
    /// @see <https://docs.lumalabs.ai/reference/generateimage> — request body
    ///   (`model`, `prompt`, `aspect_ratio`, `image_ref`/`style_ref`/`character_ref`/
    ///   `modify_image_ref`) and the `assets.image` result field.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);
        let url = format!("{}/generations/image", self.api_base());

        let mut body = json!({
            "model": native,
            "prompt": base.prompt,
        });
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }

        // Reference images (CDN URLs only). Map roles to Luma's ref fields.
        let mut image_ref: Vec<Value> = Vec::new();
        let mut style_ref: Vec<Value> = Vec::new();
        for r in &materialized.refs {
            if let MaterializedRefForm::Url(u) = &r.form {
                match r.role.as_str() {
                    "style" | "style_ref" => style_ref.push(json!({ "url": u })),
                    "modify" | "modify_image_ref" => body["modify_image_ref"] = json!({ "url": u }),
                    "character" | "character_ref" => {
                        body["character_ref"] = json!({ "identity0": { "images": [u] } })
                    }
                    _ => image_ref.push(json!({ "url": u })),
                }
            }
        }
        if !image_ref.is_empty() {
            body["image_ref"] = Value::Array(image_ref);
        }
        if !style_ref.is_empty() {
            body["style_ref"] = Value::Array(style_ref);
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
                message: format!("Luma image request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?;

        let status = resp.status();
        let submit: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Luma response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        if !status.is_success() {
            let err = submit["detail"].as_str().or_else(|| submit["message"].as_str()).unwrap_or("Unknown error").to_string();
            return Err(ProviderError::RequestFailed {
                message: format!("Luma API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(submit),
                retryable: status.as_u16() >= 500,
            });
        }

        let id = submit["id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Luma response missing id".to_string(),
            status_code: None,
            provider_error: Some(submit.clone()),
            retryable: false,
        })?;

        let final_result = self.poll_generation(id).await?;
        let image_url = final_result["assets"]["image"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Luma completed result missing assets.image".to_string(),
            status_code: None,
            provider_error: Some(final_result.clone()),
            retryable: false,
        })?;

        let bytes = self.fetch_image_bytes(image_url).await?;
        let mut metadata = HashMap::new();
        metadata.insert("id".to_string(), Value::String(id.to_string()));
        metadata.insert("url".to_string(), Value::String(image_url.to_string()));

        Ok(GenerationOutput {
            data: bytes,
            content_type: "image/jpeg".to_string(),
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
                message: "Luma image provider not configured".into(),
                latency_ms: None,
            };
        }
        let start = std::time::Instant::now();
        let creds = match self.creds() {
            Ok(c) => c,
            Err(_) => return HealthCheckResult { healthy: false, message: "No API key".into(), latency_ms: None },
        };
        let builder = match crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.get(format!("{}/generations?limit=1", self.api_base())).timeout(std::time::Duration::from_secs(10)),
        ) {
            Ok(b) => b,
            Err(e) => return HealthCheckResult { healthy: false, message: e.to_string(), latency_ms: None },
        };
        match builder.send().await {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "Luma API key valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("Luma returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("Luma health check failed: {e}"),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::{Cleanup, MaterializedRef};
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> LumaImageProvider {
        let mut p = LumaImageProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_key: "test-key".to_string(),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        cfg.credentials.api_key = Some("test-key".to_string());
        p.configure(cfg);
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

    fn make_extras() -> ImageExtras {
        ImageExtras {
            size: None,
            aspect_ratio: Some("16:9".to_string()),
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
    async fn generates_photon_image_via_polling() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/img.jpg", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/generations/image"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": "luma_img_1", "state": "queued"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/generations/luma_img_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "luma_img_1", "state": "completed", "assets": { "image": image_url.clone() }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"LUMAJPEG".to_vec()).insert_header("content-type", "image/jpeg"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("luma/photon-1");
        let base = make_base("a serene alpine lake", "luma/photon-1");
        let extras = make_extras();
        let materialized = MaterializedRequest {
            refs: vec![MaterializedRef { role: "init".into(), form: MaterializedRefForm::Url("https://cdn/x.jpg".into()) }],
            cleanup: Cleanup::empty(),
        };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"LUMAJPEG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("authorization").unwrap(), "Bearer test-key");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["model"], "photon-1");
        assert_eq!(body["prompt"], "a serene alpine lake");
        assert_eq!(body["aspect_ratio"], "16:9");
        assert_eq!(body["image_ref"][0]["url"], "https://cdn/x.jpg");
    }
}
