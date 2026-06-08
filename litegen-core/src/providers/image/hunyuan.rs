use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{tc3, AuthSpec, ProviderCredentials};
use crate::providers::{
    BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras, ImageProvider,
    ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

const TC3_CONTENT_TYPE: &str = "application/json; charset=utf-8";
const IMAGE_VERSION: &str = "2023-09-01";

/// Tencent Hunyuan image generation provider.
///
/// RPC-style Tencent Cloud API: POST to `/` with the action in the
/// `X-TC-Action` header, signed with TC3-HMAC-SHA256 (SecretId=key_id,
/// SecretKey=key_secret, region). Async: `SubmitHunyuanImageJob` returns
/// `Response.JobId`; poll `QueryHunyuanImageJob` until `JobStatusCode == "5"`,
/// then download `Response.ResultImage[0]`. Region is `ap-guangzhou` only.
///
/// @see <https://cloud.tencent.com/document/product/1729/105969>
///   Verbatim: "本接口仅支持其中的: ap-guangzhou ... X-TC-Action: SubmitHunyuanImageJob, X-TC-Version: 2023-09-01, domain hunyuan.tencentcloudapi.com, POST to path /."
/// @see <https://www.tencentcloud.com/document/product/845/32207> — TC3-HMAC-SHA256
pub struct HunyuanImageProvider {
    config: Option<ProviderInstanceConfig>,
    auth: AuthSpec,
    client: Client,
}

impl HunyuanImageProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            auth: AuthSpec::TencentTc3 { service: "hunyuan".into(), default_region: "ap-guangzhou".into() },
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn endpoint(&self) -> String {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "https://hunyuan.tencentcloudapi.com/".to_string())
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        self.config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("hunyuan".into()))
    }

    fn region(&self, creds: &ProviderCredentials) -> String {
        creds.region.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "ap-guangzhou".to_string())
    }

    /// Issue one TC3-signed RPC call (POST `/`, action via X-TC-Action header).
    async fn rpc(&self, creds: &ProviderCredentials, region: &str, action: &str, body: &Value) -> Result<Value, ProviderError> {
        let body_bytes = serde_json::to_vec(body).map_err(|e| ProviderError::InvalidRequest(e.to_string()))?;
        let endpoint = self.endpoint();
        let url = reqwest::Url::parse(&endpoint).map_err(|e| ProviderError::InvalidRequest(format!("bad hunyuan url: {e}")))?;
        let signed = tc3::sign(creds, "hunyuan", &url, TC3_CONTENT_TYPE, &body_bytes)?;

        let mut req = self
            .client
            .post(url)
            .header("Content-Type", TC3_CONTENT_TYPE)
            .header("X-TC-Action", action)
            .header("X-TC-Version", IMAGE_VERSION)
            .header("X-TC-Region", region);
        for (k, v) in &signed {
            req = req.header(k, v);
        }
        let data: Value = crate::providers::inject_trace_headers(req.body(body_bytes))
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Hunyuan {action} failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?
            .json()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to parse Hunyuan response: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;
        // Tencent error envelope: Response.Error{Code, Message}.
        if let Some(err) = data["Response"]["Error"].as_object() {
            return Err(ProviderError::RequestFailed {
                message: format!("Hunyuan API error: {}", err.get("Message").and_then(|m| m.as_str()).unwrap_or("unknown")),
                status_code: None,
                provider_error: Some(data.clone()),
                retryable: false,
            });
        }
        Ok(data)
    }

    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let resp = self.client.get(url).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to fetch Hunyuan image: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;
        Ok(resp
            .bytes()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to read Hunyuan image: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
            .to_vec())
    }
}

impl Default for HunyuanImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for HunyuanImageProvider {
    fn name(&self) -> &str {
        "hunyuan"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config.as_ref().is_some_and(|c| self.auth.is_satisfied_by(&c.credentials))
    }

    async fn generate(
        &self,
        _model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let region = self.region(&creds);

        let mut submit = json!({ "Prompt": base.prompt });
        if let Some(np) = base.negative_prompt.as_deref() {
            submit["NegativePrompt"] = Value::String(np.to_string());
        }
        if let Some(size) = extras.size.as_deref() {
            submit["Resolution"] = Value::String(size.replace('x', ":"));
        }
        if let Some(seed) = base.seed {
            submit["Seed"] = Value::Number(seed.max(0).into());
        }
        // Optional reference/control image: nested ContentImage{ImageBase64|ImageUrl}.
        for r in &materialized.refs {
            match &r.form {
                MaterializedRefForm::Base64(b64) => submit["ContentImage"] = json!({ "ImageBase64": b64 }),
                MaterializedRefForm::Url(u) => submit["ContentImage"] = json!({ "ImageUrl": u }),
                _ => {}
            }
        }
        if let Some(Value::Object(map)) = &extras.extra {
            if let Some(obj) = submit.as_object_mut() {
                for (k, v) in map {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let submit_resp = self.rpc(&creds, &region, "SubmitHunyuanImageJob", &submit).await?;
        let job_id = submit_resp["Response"]["JobId"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Hunyuan submit missing Response.JobId".to_string(),
            status_code: None,
            provider_error: Some(submit_resp.clone()),
            retryable: false,
        })?;

        // Poll QueryHunyuanImageJob until JobStatusCode 5 (done) / 4 (failed).
        let max_attempts = 90;
        for _ in 0..max_attempts {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let q = self.rpc(&creds, &region, "QueryHunyuanImageJob", &json!({ "JobId": job_id })).await?;
            let resp = &q["Response"];
            match resp["JobStatusCode"].as_str() {
                Some("5") => {
                    let image_url = resp["ResultImage"][0].as_str().ok_or_else(|| ProviderError::RequestFailed {
                        message: "Hunyuan result missing ResultImage[0]".to_string(),
                        status_code: None,
                        provider_error: Some(q.clone()),
                        retryable: false,
                    })?;
                    let bytes = self.fetch_image_bytes(image_url).await?;
                    let mut metadata = HashMap::new();
                    metadata.insert("job_id".to_string(), Value::String(job_id.to_string()));
                    return Ok(GenerationOutput { data: bytes, content_type: "image/png".to_string(), metadata });
                }
                Some("4") => {
                    return Err(ProviderError::RequestFailed {
                        message: format!("Hunyuan job failed: {}", resp["JobStatusMsg"].as_str().unwrap_or("")),
                        status_code: None,
                        provider_error: Some(q.clone()),
                        retryable: false,
                    })
                }
                _ => continue,
            }
        }
        Err(ProviderError::Timeout { timeout_ms: max_attempts * 2000 })
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
                message: "Hunyuan provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Hunyuan provider configured".into(), latency_ms: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> HunyuanImageProvider {
        let mut p = HunyuanImageProvider::new();
        let mut cfg = ProviderInstanceConfig { api_base: Some(format!("{api_base}/")), ..Default::default() };
        cfg.credentials.key_id = Some("AKIDtest".to_string());
        cfg.credentials.key_secret = Some("secret".to_string());
        cfg.credentials.region = Some("ap-guangzhou".to_string());
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
            size: Some("1024x1024".to_string()), aspect_ratio: None, quality: None, style: None,
            steps: None, guidance_scale: None, strength: None, response_format: "url".to_string(), extra: None,
        }
    }

    #[tokio::test]
    async fn submits_and_polls_with_tc3() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/h.png", image_server.uri());

        // Submit returns a JobId.
        Mock::given(method("POST"))
            .and(header("x-tc-action", "SubmitHunyuanImageJob"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "Response": { "JobId": "hy-1", "RequestId": "r1" } })))
            .mount(&server)
            .await;
        // Query returns done with a result URL.
        Mock::given(method("POST"))
            .and(header("x-tc-action", "QueryHunyuanImageJob"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Response": { "JobStatusCode": "5", "JobStatusMsg": "done", "ResultImage": [image_url.clone()] }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"HUNYUANPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("hunyuan/hunyuan-image");
        let base = make_base("a tranquil zen garden", "hunyuan/hunyuan-image");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"HUNYUANPNG");

        let received = server.received_requests().await.unwrap();
        let submit = received.iter().find(|r| r.headers.get("x-tc-action").map(|v| v == "SubmitHunyuanImageJob").unwrap_or(false)).unwrap();
        let auth = submit.headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("TC3-HMAC-SHA256 Credential=AKIDtest/"), "auth: {auth}");
        assert!(submit.headers.get("x-tc-timestamp").is_some());
        let body: Value = serde_json::from_slice(&submit.body).unwrap();
        assert_eq!(body["Prompt"], "a tranquil zen garden");
        assert_eq!(body["Resolution"], "1024:1024");
    }
}
