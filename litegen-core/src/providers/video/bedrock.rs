use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{sigv4, AuthSpec, ProviderCredentials};
use crate::providers::{
    BaseGenerationRequest, CredentialPool, HealthCheckResult, ProviderError, ProviderInstanceConfig, VideoExtras,
    VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// Amazon Bedrock Nova Reel video provider.
///
/// Async: `POST /async-invoke` `{modelId, modelInput, outputDataConfig}` returns
/// `{invocationArn}`; poll `GET /async-invoke/{invocationArn}` until
/// `status == "Completed"`. The rendered `output.mp4` is written to the caller's
/// S3 bucket (not returned inline), so this provider REQUIRES an
/// `s3_output_uri` option and returns that S3 location as `video_url`
/// (downloading it needs S3 access — a tracked follow-up shared with Veo).
/// AWS SigV4 auth (service `bedrock`).
///
/// @see <https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_StartAsyncInvoke.html>
///   Verbatim: "POST /async-invoke HTTP/1.1 ... {\"clientRequestToken\",\"modelId\",\"modelInput\",\"outputDataConfig\",\"tags\"}"
/// @see <https://docs.aws.amazon.com/nova/latest/userguide/video-gen-access.html>
///   Verbatim: "For Amazon Nova Reel, this is \"amazon.nova-reel-v1:1\""
pub struct BedrockVideoProvider {
    config: Option<ProviderInstanceConfig>,
    cred_pool: Option<CredentialPool>,
    auth: AuthSpec,
    client: Client,
}

impl BedrockVideoProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            cred_pool: None,
            auth: AuthSpec::AwsSigV4 { service: "bedrock".into(), default_region: "us-east-1".into() },
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("bedrock".into()))?;
        if let Some(pool) = &self.cred_pool {
            return Ok(base.with_signing(pool.next()));
        }
        Ok(base)
    }

    fn region(&self, creds: &ProviderCredentials) -> String {
        creds.region.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| match &self.auth {
            AuthSpec::AwsSigV4 { default_region, .. } => default_region.clone(),
            _ => "us-east-1".to_string(),
        })
    }

    fn host_base(&self, region: &str) -> String {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("https://bedrock-runtime.{region}.amazonaws.com"))
    }

    /// The caller-provided S3 output URI (from options or credentials_extra).
    fn s3_output_uri(&self, creds: &ProviderCredentials) -> Option<String> {
        if let Some(opts) = self.config.as_ref().and_then(|c| c.options.as_ref()) {
            if let Some(s) = opts.get("s3_output_uri").and_then(|v| v.as_str()) {
                return Some(s.to_string());
            }
        }
        creds.extra.get("s3_output_uri").cloned()
    }

    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("bedrock/").unwrap_or(model_id)
    }
}

impl Default for BedrockVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for BedrockVideoProvider {
    fn name(&self) -> &str {
        "bedrock"
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
        let native = Self::resolve_model(&model.id);

        let s3_uri = self.s3_output_uri(&creds).ok_or_else(|| {
            ProviderError::InvalidRequest(
                "bedrock video (Nova Reel) requires an `s3_output_uri` option (output is written to your S3 bucket)".into(),
            )
        })?;

        let mut t2v = json!({ "text": base.prompt });
        // Optional starting keyframe (base64 PNG/JPEG, must be 1280x720 per docs).
        for r in &materialized.refs {
            if let MaterializedRefForm::Base64(b64) = &r.form {
                t2v["images"] = json!([{ "format": "png", "source": { "bytes": b64 } }]);
                break;
            }
        }
        let mut video_config = json!({});
        if extras.duration_seconds > 0.0 {
            video_config["durationSeconds"] = Value::Number((extras.duration_seconds as i64).into());
        }
        if let Some(fps) = extras.fps {
            video_config["fps"] = Value::Number(fps.into());
        }
        if let Some(dim) = extras.resolution.as_deref() {
            video_config["dimension"] = Value::String(dim.to_string());
        }
        if let Some(seed) = base.seed {
            video_config["seed"] = Value::Number(seed.max(0).into());
        }

        let body = json!({
            "modelId": native,
            "modelInput": {
                "taskType": "TEXT_VIDEO",
                "textToVideoParams": t2v,
                "videoGenerationConfig": video_config,
            },
            "outputDataConfig": { "s3OutputDataConfig": { "s3Uri": s3_uri } },
        });
        let body_bytes = serde_json::to_vec(&body).map_err(|e| ProviderError::InvalidRequest(e.to_string()))?;

        let endpoint = format!("{}/async-invoke", self.host_base(&region));
        let url = reqwest::Url::parse(&endpoint).map_err(|e| ProviderError::InvalidRequest(format!("bad bedrock url: {e}")))?;
        let signed = sigv4::sign(&creds, "bedrock", &region, "POST", &url, &[("content-type".to_string(), "application/json".to_string())], &body_bytes)?;

        let mut req = self.client.post(url).header("content-type", "application/json");
        for (k, v) in &signed {
            req = req.header(k, v);
        }
        let resp = crate::providers::inject_trace_headers(req.body(body_bytes))
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Bedrock async-invoke failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?;

        let status = resp.status();
        let data: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Bedrock response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        if !status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Bedrock API error: {}", data["message"].as_str().unwrap_or("unknown")),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                retryable: status.as_u16() >= 500,
            });
        }

        let arn = data["invocationArn"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Bedrock response missing invocationArn".to_string(),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        })?;

        Ok(VideoGenerationHandle {
            provider_job_id: arn.to_string(),
            provider: "bedrock".to_string(),
            model: model.id.clone(),
        })
    }

    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        let region = self.region(&creds);
        // Raw arn in the path: reqwest sends ':' literally and SigV4 encodes it
        // to %3A, matching how AWS canonicalizes the received path.
        let endpoint = format!("{}/async-invoke/{}", self.host_base(&region), handle.provider_job_id);
        let url = reqwest::Url::parse(&endpoint).map_err(|e| ProviderError::InvalidRequest(format!("bad bedrock url: {e}")))?;
        let signed = sigv4::sign(&creds, "bedrock", &region, "GET", &url, &[], b"")?;

        let mut req = self.client.get(url);
        for (k, v) in &signed {
            req = req.header(k, v);
        }
        let data: Value = crate::providers::inject_trace_headers(req)
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

        let status = match data["status"].as_str() {
            Some("Completed") => GenerationStatus::Completed,
            Some("Failed") => GenerationStatus::Failed,
            _ => GenerationStatus::Processing,
        };
        // Output lands in the caller's S3 bucket; echo that location.
        let video_url = if status == GenerationStatus::Completed {
            data["outputDataConfig"]["s3OutputDataConfig"]["s3Uri"]
                .as_str()
                .map(|s| format!("{}/output.mp4", s.trim_end_matches('/')))
        } else {
            None
        };

        Ok(VideoGenerationPollResult {
            status,
            progress: if status == GenerationStatus::Completed { 100 } else { 50 },
            video_url,
            video_data: None,
            content_type: Some("video/mp4".into()),
            error: data["failureMessage"].as_str().map(String::from),
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
                message: "Bedrock video provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Bedrock video provider configured".into(), latency_ms: None }
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

    fn make_provider(api_base: &str) -> BedrockVideoProvider {
        let mut p = BedrockVideoProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_base: Some(api_base.to_string()),
            options: Some(json!({ "s3_output_uri": "s3://my-bucket/litegen/" })),
            ..Default::default()
        };
        cfg.credentials.key_id = Some("AKIDEXAMPLE".to_string());
        cfg.credentials.key_secret = Some("secretkey".to_string());
        cfg.credentials.region = Some("us-east-1".to_string());
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
        VideoExtras { duration_seconds: 6.0, aspect_ratio: None, resolution: Some("1280x720".to_string()), fps: Some(24), extra: None }
    }

    #[tokio::test]
    async fn submits_async_invoke_with_sigv4_and_requires_s3() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/async-invoke"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "invocationArn": "arn:aws:bedrock:us-east-1:123456789012:async-invoke/abc"
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("bedrock/amazon.nova-reel-v1:1");
        let base = make_base("waves crashing on rocks", "bedrock/amazon.nova-reel-v1:1");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "arn:aws:bedrock:us-east-1:123456789012:async-invoke/abc");

        let received = server.received_requests().await.unwrap();
        let post = &received[0];
        let auth = post.headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"), "auth: {auth}");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["modelId"], "amazon.nova-reel-v1:1");
        assert_eq!(body["modelInput"]["taskType"], "TEXT_VIDEO");
        assert_eq!(body["outputDataConfig"]["s3OutputDataConfig"]["s3Uri"], "s3://my-bucket/litegen/");
    }

    #[tokio::test]
    async fn video_without_s3_uri_errors() {
        let mut p = BedrockVideoProvider::new();
        let mut cfg = ProviderInstanceConfig::default();
        cfg.credentials.key_id = Some("AKIDEXAMPLE".to_string());
        cfg.credentials.key_secret = Some("secretkey".to_string());
        cfg.credentials.region = Some("us-east-1".to_string());
        p.configure(cfg);

        let schema = ref_schema("bedrock/amazon.nova-reel-v1:1");
        let base = make_base("x", "bedrock/amazon.nova-reel-v1:1");
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
        let res = p.generate(&schema, &base, &make_extras(), &materialized).await;
        assert!(matches!(res, Err(ProviderError::InvalidRequest(_))));
    }
}
