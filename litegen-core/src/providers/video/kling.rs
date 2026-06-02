use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig, VideoExtras,
    VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// Kling (Kuaishou) video generation provider.
///
/// Async: `POST /v1/videos/text2video` or `/v1/videos/image2video` returns
/// `data.task_id`; poll `GET /v1/videos/{kind}/{task_id}` until
/// `data.task_status == "succeed"`, then read `data.task_result.videos[0].url`.
/// Auth: per-request HS256 JWT ([`AuthSpec::KlingJwt`]). The job handle encodes
/// `{kind}/{task_id}` so polling targets the right sub-path.
///
/// @see <https://app.klingai.com/global/dev/document-api/apiReference/model/imageToVideo>
///   (image first frame `image`, last frame `image_tail`; mode std|pro; duration 5|10)
pub struct KlingVideoProvider {
    config: Option<ProviderInstanceConfig>,
    auth: AuthSpec,
    client: Client,
}

impl KlingVideoProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            auth: AuthSpec::KlingJwt,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
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
        self.config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("kling".into()))
    }

    fn resolve_model(model_id: &str) -> &str {
        // Video model ids carry a `video-` prefix to stay distinct from image
        // model ids that share a model_name (e.g. kling-v2); strip both.
        let s = model_id.strip_prefix("kling/").unwrap_or(model_id);
        s.strip_prefix("video-").unwrap_or(s)
    }

    fn ref_image(form: &MaterializedRefForm) -> Option<String> {
        match form {
            MaterializedRefForm::Base64(b64) => Some(b64.clone()),
            MaterializedRefForm::Url(u) => Some(u.clone()),
            _ => None,
        }
    }
}

impl Default for KlingVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for KlingVideoProvider {
    fn name(&self) -> &str {
        "kling"
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
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);

        // first frame -> image2video; otherwise text2video.
        let mut first: Option<String> = None;
        let mut tail: Option<String> = None;
        for r in &materialized.refs {
            if let Some(img) = Self::ref_image(&r.form) {
                if r.role == "last_frame" {
                    tail = Some(img);
                } else {
                    first = Some(img);
                }
            }
        }
        let kind = if first.is_some() { "image2video" } else { "text2video" };
        let url = format!("{}/v1/videos/{kind}", self.api_base());

        let mut body = json!({ "model_name": native, "prompt": base.prompt });
        if let Some(np) = base.negative_prompt.as_deref() {
            body["negative_prompt"] = Value::String(np.to_string());
        }
        if extras.duration_seconds > 0.0 {
            body["duration"] = Value::String((extras.duration_seconds as i64).to_string());
        }
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }
        if let Some(img) = first {
            body["image"] = Value::String(img);
        }
        if let Some(t) = tail {
            body["image_tail"] = Value::String(t);
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
                message: format!("Kling video request failed: {e}"),
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

        Ok(VideoGenerationHandle {
            // Encode the sub-path so poll_status targets the right endpoint.
            provider_job_id: format!("{kind}/{task_id}"),
            provider: "kling".to_string(),
            model: model.id.clone(),
        })
    }

    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/v1/videos/{}", self.api_base(), handle.provider_job_id);
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

        let status = match data["data"]["task_status"].as_str() {
            Some("succeed") => GenerationStatus::Completed,
            Some("failed") => GenerationStatus::Failed,
            _ => GenerationStatus::Processing,
        };
        let video_url = data["data"]["task_result"]["videos"][0]["url"].as_str().map(String::from);

        Ok(VideoGenerationPollResult {
            status,
            progress: if status == GenerationStatus::Completed { 100 } else { 50 },
            video_url,
            video_data: None,
            content_type: Some("video/mp4".into()),
            error: data["data"]["task_status_msg"].as_str().filter(|_| status == GenerationStatus::Failed).map(String::from),
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
                message: "Kling video provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Kling video provider configured".into(), latency_ms: None }
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

    fn make_provider(api_base: &str) -> KlingVideoProvider {
        let mut p = KlingVideoProvider::new();
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

    fn make_extras() -> VideoExtras {
        VideoExtras { duration_seconds: 5.0, aspect_ratio: Some("16:9".to_string()), resolution: None, fps: None, extra: None }
    }

    #[tokio::test]
    async fn submits_text2video_with_jwt_and_polls() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/videos/text2video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0, "data": { "task_id": "kv-1", "task_status": "submitted" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/v1/videos/text2video/kv-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0, "data": { "task_status": "succeed",
                    "task_result": { "videos": [{ "id": "v1", "url": "https://cdn.kling/v.mp4" }] } }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("kling/kling-v2");
        let base = make_base("a phoenix rising", "kling/kling-v2");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "text2video/kv-1");

        let received = server.received_requests().await.unwrap();
        let auth = received[0].headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("Bearer ey"), "expected JWT bearer, got {auth}");

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_url.unwrap(), "https://cdn.kling/v.mp4");
    }
}
