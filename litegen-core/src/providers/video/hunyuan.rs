use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{tc3, AuthSpec, ProviderCredentials};
use crate::providers::{
    BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig, VideoExtras,
    VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

const TC3_CONTENT_TYPE: &str = "application/json; charset=utf-8";
const VCLM_VERSION: &str = "2024-05-23";

/// Tencent Hunyuan video generation provider (Video Creation Large Model / vclm).
///
/// RPC-style, TC3-HMAC-SHA256 signed. Async: `SubmitImageToVideoJob` (on
/// `vclm.tencentcloudapi.com`) returns `Response.JobId`; poll
/// `QueryImageToVideoJob` until done, then read the result video URL. Region
/// `ap-guangzhou`. The vclm product is separate from the Hunyuan image product.
///
/// @see <https://github.com/TencentCloud/tencentcloud-sdk-nodejs/blob/master/src/services/vclm/v20240523/vclm_client.ts>
///   Verbatim: "endpoint 'vclm.tencentcloudapi.com' ... apiVersion '2024-05-23' ... SubmitImageToVideoJob ..."
/// @see <https://www.tencentcloud.com/document/product/845/32207> — TC3-HMAC-SHA256
pub struct HunyuanVideoProvider {
    config: Option<ProviderInstanceConfig>,
    auth: AuthSpec,
    client: Client,
}

impl HunyuanVideoProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            auth: AuthSpec::TencentTc3 { service: "vclm".into(), default_region: "ap-guangzhou".into() },
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn endpoint(&self) -> String {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "https://vclm.tencentcloudapi.com/".to_string())
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

    async fn rpc(&self, creds: &ProviderCredentials, region: &str, action: &str, body: &Value) -> Result<Value, ProviderError> {
        let body_bytes = serde_json::to_vec(body).map_err(|e| ProviderError::InvalidRequest(e.to_string()))?;
        let url = reqwest::Url::parse(&self.endpoint()).map_err(|e| ProviderError::InvalidRequest(format!("bad vclm url: {e}")))?;
        let signed = tc3::sign(creds, "vclm", &url, TC3_CONTENT_TYPE, &body_bytes)?;

        let mut req = self
            .client
            .post(url)
            .header("Content-Type", TC3_CONTENT_TYPE)
            .header("X-TC-Action", action)
            .header("X-TC-Version", VCLM_VERSION)
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
}

impl Default for HunyuanVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for HunyuanVideoProvider {
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
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        _extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let creds = self.creds()?;
        let region = self.region(&creds);

        let mut body = json!({ "Prompt": base.prompt });
        // Driving/first-frame image: ImageUrl (URL) or ImageBase64 (base64).
        for r in &materialized.refs {
            match &r.form {
                MaterializedRefForm::Url(u) => body["ImageUrl"] = Value::String(u.clone()),
                MaterializedRefForm::Base64(b64) => body["ImageBase64"] = Value::String(b64.clone()),
                _ => {}
            }
        }
        if let Some(Value::Object(map)) = &_extras.extra {
            if let Some(obj) = body.as_object_mut() {
                for (k, v) in map {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let resp = self.rpc(&creds, &region, "SubmitImageToVideoJob", &body).await?;
        let job_id = resp["Response"]["JobId"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Hunyuan submit missing Response.JobId".to_string(),
            status_code: None,
            provider_error: Some(resp.clone()),
            retryable: false,
        })?;

        Ok(VideoGenerationHandle {
            provider_job_id: job_id.to_string(),
            provider: "hunyuan".to_string(),
            model: model.id.clone(),
        })
    }

    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        let region = self.region(&creds);
        let q = self.rpc(&creds, &region, "QueryImageToVideoJob", &json!({ "JobId": handle.provider_job_id })).await?;
        let resp = &q["Response"];

        // vclm reports status via a Status/StatusCode string; the finished video
        // URL appears as ResultVideoUrl (presence implies completion).
        let video_url = resp["ResultVideoUrl"].as_str().or_else(|| resp["VideoUrl"].as_str()).map(String::from);
        let status_str = resp["Status"].as_str().or_else(|| resp["StatusCode"].as_str()).unwrap_or("");
        let status = if video_url.is_some() {
            GenerationStatus::Completed
        } else if matches!(status_str, "FAIL" | "FAILED" | "4") {
            GenerationStatus::Failed
        } else {
            GenerationStatus::Processing
        };

        Ok(VideoGenerationPollResult {
            status,
            progress: if status == GenerationStatus::Completed { 100 } else { 50 },
            video_url,
            video_data: None,
            content_type: Some("video/mp4".into()),
            error: resp["StatusMsg"].as_str().filter(|_| status == GenerationStatus::Failed).map(String::from),
            metadata: HashMap::new(),
        })
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &VideoGenerationRequest,
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
                message: "Hunyuan video provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Hunyuan video provider configured".into(), latency_ms: None }
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

    fn make_provider(api_base: &str) -> HunyuanVideoProvider {
        let mut p = HunyuanVideoProvider::new();
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

    fn make_extras() -> VideoExtras {
        VideoExtras { duration_seconds: 5.0, aspect_ratio: None, resolution: None, fps: None, extra: None }
    }

    #[tokio::test]
    async fn submits_and_polls_video_with_tc3() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-tc-action", "SubmitImageToVideoJob"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "Response": { "JobId": "hv-1" } })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(header("x-tc-action", "QueryImageToVideoJob"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Response": { "Status": "DONE", "ResultVideoUrl": "https://cdn.tencent/v.mp4" }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("hunyuan/hunyuan-video");
        let base = make_base("a calligraphy brush stroke coming alive", "hunyuan/hunyuan-video");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "hv-1");

        let received = server.received_requests().await.unwrap();
        let submit = received.iter().find(|r| r.headers.get("x-tc-action").map(|v| v == "SubmitImageToVideoJob").unwrap_or(false)).unwrap();
        let auth = submit.headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("TC3-HMAC-SHA256 Credential=AKIDtest/"), "auth: {auth}");

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_url.unwrap(), "https://cdn.tencent/v.mp4");
    }
}
