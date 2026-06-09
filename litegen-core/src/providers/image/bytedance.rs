use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
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

/// ByteDance Seedream image provider (Volcengine Ark / BytePlus ModelArk).
///
/// Synchronous, OpenAI-compatible: `POST /api/v3/images/generations` returns
/// `{data: [{url | b64_json}]}`. Bearer auth with the Ark API key. Host via
/// api_base (BytePlus `ark.ap-southeast.bytepluses.com/api/v3` international or
/// Volcengine `ark.cn-beijing.volces.com/api/v3` China).
///
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1298459> — base + Bearer auth
///   Verbatim: "The data plane API base URL is https://ark.ap-southeast.bytepluses.com/api/v3 ; for API key authentication use \"Authorization: Bearer $ARK_API_KEY\"."
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1824718>
///   Verbatim: "seedream-4.0 Model ID: seedream-4-0-250828 ... Pricing: $0.03 USD per generated image; No charge for failed generations."
pub struct ByteDanceImageProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl ByteDanceImageProvider {
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
            .unwrap_or("https://ark.ap-southeast.bytepluses.com/api/v3")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("bytedance".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("bytedance/").unwrap_or(model_id)
    }
}

impl Default for ByteDanceImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for ByteDanceImageProvider {
    fn name(&self) -> &str {
        "bytedance"
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
        let url = format!("{}/images/generations", self.api_base());

        let mut body = json!({
            "model": native,
            "prompt": base.prompt,
            "response_format": "b64_json",
        });
        if let Some(size) = extras.size.as_deref() {
            body["size"] = Value::String(size.to_string());
        }
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }
        // image-to-image / multi-reference: `image` accepts url or base64 data URI.
        let mut images: Vec<String> = Vec::new();
        for r in &materialized.refs {
            match &r.form {
                MaterializedRefForm::Url(u) => images.push(u.clone()),
                MaterializedRefForm::Base64(b64) => images.push(format!("data:image/png;base64,{b64}")),
                _ => {}
            }
        }
        if images.len() == 1 {
            body["image"] = Value::String(images.remove(0));
        } else if !images.is_empty() {
            body["image"] = Value::Array(images.into_iter().map(Value::String).collect());
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
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("ByteDance request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?;

        let status = resp.status();
        let data: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse ByteDance response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        if !status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("ByteDance API error: {data}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                retryable: status.as_u16() >= 500,
            });
        }

        let first = data["data"].get(0).ok_or_else(|| ProviderError::RequestFailed {
            message: "ByteDance response missing data[0]".to_string(),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        })?;
        let bytes = if let Some(b64) = first["b64_json"].as_str() {
            B64.decode(b64).map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to decode ByteDance b64_json: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
        } else if let Some(u) = first["url"].as_str() {
            let r = self.client.get(u).send().await.map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to fetch ByteDance image: {e}"),
                status_code: None,
                provider_error: None,
                retryable: true,
            })?;
            r.bytes().await.map(|b| b.to_vec()).map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to read ByteDance image: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
        } else {
            return Err(ProviderError::RequestFailed {
                message: "ByteDance data[0] has neither b64_json nor url".to_string(),
                status_code: None,
                provider_error: Some(data.clone()),
                retryable: false,
            });
        };

        let mut metadata = HashMap::new();
        metadata.insert("model".to_string(), Value::String(native.to_string()));

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
                message: "ByteDance provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "ByteDance provider configured".into(), latency_ms: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> ByteDanceImageProvider {
        let mut p = ByteDanceImageProvider::new();
        let mut cfg = ProviderInstanceConfig { api_key: "ark-key".to_string(), api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.api_key = Some("ark-key".to_string());
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
            size: Some("2048x2048".to_string()), aspect_ratio: None, quality: None, style: None,
            steps: None, guidance_scale: None, strength: None, response_format: "url".to_string(), extra: None,
        }
    }

    #[tokio::test]
    async fn generates_seedream() {
        let server = MockServer::start().await;
        let png = b"SEEDREAMPNG";
        let b64 = B64.encode(png);
        Mock::given(method("POST"))
            .and(path("/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "seedream-4-0-250828", "created": 1700000000,
                "data": [{ "b64_json": b64 }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("bytedance/seedream-4-0-250828");
        let base = make_base("an ornate ancient map", "bytedance/seedream-4-0-250828");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, png);

        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].headers.get("authorization").unwrap(), "Bearer ark-key");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "seedream-4-0-250828");
        assert_eq!(body["size"], "2048x2048");
        assert_eq!(body["response_format"], "b64_json");
    }
}
