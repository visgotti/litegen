use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{tc3, AuthSpec, ProviderCredentials};
use crate::providers::{
    BaseGenerationRequest, CredentialPool, HealthCheckResult, ProviderError, ProviderInstanceConfig, VideoExtras,
    VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

const TC3_CONTENT_TYPE: &str = "application/json; charset=utf-8";
const VCLM_VERSION: &str = "2024-05-23";
const SUBMIT_ACTION: &str = "SubmitHunyuanToVideoJob";
const DESCRIBE_ACTION: &str = "DescribeHunyuanToVideoJob";

/// Tencent Hunyuan video generation provider (Video Creation Large Model / vclm).
///
/// RPC-style, TC3-HMAC-SHA256 signed. Async: `SubmitHunyuanToVideoJob` (on
/// `vclm.tencentcloudapi.com`) returns `Response.JobId`; poll
/// `DescribeHunyuanToVideoJob` until `Status` is `DONE` (then read
/// `ResultVideoUrl`) or `FAIL`. Region `ap-guangzhou`. The vclm product is
/// separate from the Hunyuan image product.
///
/// This is Tencent's native Hunyuan video model. The provider used to call the
/// Kling-branded `SubmitImageToVideoJob` with `Model: "Kling-V1-6"`, a Kling
/// version retired 2026-09-15, behind an id that promises Hunyuan.
///
/// `SubmitHunyuanToVideoJob` takes exactly `Prompt` (required, at most 200
/// characters), `Image` (`{Base64}` or `{Url}`, optional: without it the job is
/// text-to-video), `Resolution` (720p only), `LogoAdd` and `LogoParam`. There is
/// no model selector.
///
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/vclm/v20240523/models.go>
///   (`SubmitHunyuanToVideoJobRequestParams`, `DescribeHunyuanToVideoJobResponseParams`: "WAIT：等待中，RUN：执行中，FAIL：任务失败，DONE：任务成功")
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/vclm/v20240523/client.go>
/// @see <https://www.tencentcloud.com/document/product/845/32207> — TC3-HMAC-SHA256
pub struct HunyuanVideoProvider {
    config: Option<ProviderInstanceConfig>,
    cred_pool: Option<CredentialPool>,
    auth: AuthSpec,
    client: Client,
}

impl HunyuanVideoProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            cred_pool: None,
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
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("hunyuan".into()))?;
        if let Some(pool) = &self.cred_pool {
            return Ok(base.with_signing(pool.next()));
        }
        Ok(base)
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
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let creds = self.creds()?;
        let region = self.region(&creds);

        // No Model field: SubmitHunyuanToVideoJob has no model selector, and an
        // undefined parameter fails the request (UnknownParameter).
        let mut body = json!({ "Prompt": base.prompt });
        // Optional first-frame image: nested Image{Url} (URL) or Image{Base64}
        // (base64). Without one the job is text-to-video.
        for r in &materialized.refs {
            match &r.form {
                MaterializedRefForm::Url(u) => body["Image"] = json!({ "Url": u }),
                MaterializedRefForm::Base64(b64) => body["Image"] = json!({ "Base64": b64 }),
                _ => {}
            }
        }
        // "目前仅支持720p视频分辨率，默认720p" — the catalog enum is [720p].
        if let Some(res) = extras.resolution.as_deref() {
            body["Resolution"] = Value::String(res.to_string());
        }
        if let Some(Value::Object(map)) = &extras.extra {
            if let Some(obj) = body.as_object_mut() {
                for (k, v) in map {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let resp = self.rpc(&creds, &region, SUBMIT_ACTION, &body).await?;
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
        let q = self.rpc(&creds, &region, DESCRIBE_ACTION, &json!({ "JobId": handle.provider_job_id })).await?;
        let resp = &q["Response"];

        // Status: WAIT (queued) | RUN (running) | FAIL | DONE; ResultVideoUrl
        // (valid 24 h) is set once the job is DONE. ErrorCode/ErrorMessage are
        // "" unless the job FAILed.
        let video_url = resp["ResultVideoUrl"].as_str().filter(|u| !u.is_empty()).map(String::from);
        let non_empty = |k: &str| resp[k].as_str().filter(|s| !s.is_empty()).map(String::from);
        let (status, error) = match resp["Status"].as_str().unwrap_or("") {
            "DONE" if video_url.is_some() => (GenerationStatus::Completed, None),
            // A finished job with no video will never produce one: fail loudly
            // instead of polling until the job times out.
            "DONE" => (GenerationStatus::Failed, Some("Hunyuan job is DONE but returned no ResultVideoUrl".to_string())),
            "FAIL" => (
                GenerationStatus::Failed,
                Some(non_empty("ErrorMessage").or_else(|| non_empty("ErrorCode")).unwrap_or_else(|| "Hunyuan job failed".to_string())),
            ),
            _ => (GenerationStatus::Processing, None),
        };

        Ok(VideoGenerationPollResult {
            status,
            progress: if status == GenerationStatus::Completed { 100 } else { 50 },
            video_url: video_url.filter(|_| status == GenerationStatus::Completed),
            video_data: None,
            content_type: Some("video/mp4".into()),
            error,
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
    use crate::proxy::materializer::{Cleanup, MaterializedRef};
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

    fn submit_body(received: &[wiremock::Request]) -> Value {
        let submit = received
            .iter()
            .find(|r| r.headers.get("x-tc-action").map(|v| v == SUBMIT_ACTION).unwrap_or(false))
            .expect("no SubmitHunyuanToVideoJob request was sent");
        serde_json::from_slice(&submit.body).unwrap()
    }

    async fn mount_describe(server: &MockServer, response: Value) {
        Mock::given(method("POST"))
            .and(header("x-tc-action", "DescribeHunyuanToVideoJob"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "Response": response })))
            .mount(server)
            .await;
    }

    async fn mount_submit(server: &MockServer) {
        // Only the native Hunyuan action is mocked: a request for the Kling
        // resale action (SubmitImageToVideoJob) gets wiremock's 404.
        Mock::given(method("POST"))
            .and(header("x-tc-action", "SubmitHunyuanToVideoJob"))
            .and(header("x-tc-version", "2024-05-23"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "Response": { "JobId": "hv-1" } })))
            .mount(server)
            .await;
    }

    fn handle() -> VideoGenerationHandle {
        VideoGenerationHandle {
            provider_job_id: "hv-1".to_string(),
            provider: "hunyuan".to_string(),
            model: "hunyuan/hunyuan-video".to_string(),
        }
    }

    #[tokio::test]
    async fn submits_hunyuan_to_video_with_the_image_and_polls_with_tc3() {
        let server = MockServer::start().await;
        mount_submit(&server).await;
        mount_describe(&server, json!({ "Status": "DONE", "ResultVideoUrl": "https://cdn.tencent/v.mp4", "ErrorCode": "", "ErrorMessage": "" })).await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("hunyuan/hunyuan-video");
        let base = make_base("一只猫在草原上奔跑，写实风格", "hunyuan/hunyuan-video");
        let extras = make_extras();
        let materialized = MaterializedRequest {
            refs: vec![MaterializedRef {
                role: "first_frame".to_string(),
                form: MaterializedRefForm::Base64("aW1n".to_string()),
            }],
            cleanup: Cleanup::empty(),
        };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "hv-1");

        let received = server.received_requests().await.unwrap();
        let body = submit_body(&received);
        assert_eq!(body["Prompt"], "一只猫在草原上奔跑，写实风格");
        // The image goes in nested as Image{Base64}.
        assert_eq!(body["Image"]["Base64"], "aW1n");
        // SubmitHunyuanToVideoJob has no Model parameter; sending the old
        // Kling selector would fail with UnknownParameter.
        assert!(body.get("Model").is_none(), "Model must not be sent: {body}");

        let submit = received
            .iter()
            .find(|r| r.headers.get("x-tc-action").map(|v| v == SUBMIT_ACTION).unwrap_or(false))
            .unwrap();
        let auth = submit.headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("TC3-HMAC-SHA256 Credential=AKIDtest/"), "auth: {auth}");

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_url.unwrap(), "https://cdn.tencent/v.mp4");
        assert!(poll.error.is_none());
    }

    #[tokio::test]
    async fn text_only_request_sends_just_the_prompt_and_resolution() {
        let server = MockServer::start().await;
        mount_submit(&server).await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("hunyuan/hunyuan-video");
        let base = make_base("a calligraphy brush stroke coming alive", "hunyuan/hunyuan-video");
        let extras = VideoExtras { resolution: Some("720p".to_string()), ..make_extras() };
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "hv-1");

        let body = submit_body(&server.received_requests().await.unwrap());
        assert_eq!(body, json!({ "Prompt": "a calligraphy brush stroke coming alive", "Resolution": "720p" }));
    }

    #[tokio::test]
    async fn wait_and_run_are_processing() {
        for s in ["WAIT", "RUN"] {
            let server = MockServer::start().await;
            mount_describe(&server, json!({ "Status": s, "ResultVideoUrl": "", "ErrorCode": "", "ErrorMessage": "" })).await;
            let provider = make_provider(&server.uri());
            let poll = provider.poll_status(&handle()).await.unwrap();
            assert_eq!(poll.status, GenerationStatus::Processing, "{s}");
            assert!(poll.video_url.is_none(), "{s}");
            assert!(poll.error.is_none(), "{s}");
        }
    }

    #[tokio::test]
    async fn fail_is_failed_with_the_error_message() {
        let server = MockServer::start().await;
        mount_describe(&server, json!({
            "Status": "FAIL", "ResultVideoUrl": "",
            "ErrorCode": "FailedOperation.DriverFailed", "ErrorMessage": "驱动失败"
        }))
        .await;
        let provider = make_provider(&server.uri());
        let poll = provider.poll_status(&handle()).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Failed);
        assert_eq!(poll.error.as_deref(), Some("驱动失败"));
        assert!(poll.video_url.is_none());
    }

    #[tokio::test]
    async fn fail_without_a_message_falls_back_to_the_error_code() {
        let server = MockServer::start().await;
        mount_describe(&server, json!({
            "Status": "FAIL", "ResultVideoUrl": "",
            "ErrorCode": "FailedOperation.DriverFailed", "ErrorMessage": ""
        }))
        .await;
        let provider = make_provider(&server.uri());
        let poll = provider.poll_status(&handle()).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Failed);
        assert_eq!(poll.error.as_deref(), Some("FailedOperation.DriverFailed"));
    }

    #[tokio::test]
    async fn done_without_a_video_url_is_failed_not_polled_forever() {
        let server = MockServer::start().await;
        mount_describe(&server, json!({ "Status": "DONE", "ResultVideoUrl": "", "ErrorCode": "", "ErrorMessage": "" })).await;
        let provider = make_provider(&server.uri());
        let poll = provider.poll_status(&handle()).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Failed);
        assert!(poll.error.is_some());
    }
}
