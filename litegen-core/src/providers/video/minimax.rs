use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig,
    VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// MiniMax (Hailuo) video generation provider.
///
/// Async, three-step: `POST /v1/video_generation` -> `task_id`; poll
/// `GET /v1/query/video_generation?task_id=` until `status == "Success"` (yields
/// `file_id`); then `GET /v1/files/retrieve?file_id=` -> `file.download_url`.
/// Bearer auth; region split via api_base.
///
/// @see <https://platform.minimax.io/docs/guides/video-generation>
///   Verbatim: "if status == \"Success\": return response_json[\"file_id\"]"
///   Verbatim: "download_url = response.json()[\"file\"][\"download_url\"]"
pub struct MiniMaxVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl MiniMaxVideoProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
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
            .unwrap_or("https://api.minimax.io/v1")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("minimax".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("minimax/").unwrap_or(model_id)
    }

    /// Resolve the `file.download_url` for a completed task's file.
    async fn retrieve_file_url(&self, file_id: &str) -> Result<String, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/files/retrieve?file_id={}", self.api_base(), file_id);
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(&url))?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: e.to_string(),
                status_code: None,
                provider_error: None,
                retryable: true,
            })?;
        let data: Value = resp.json().await.unwrap_or_default();
        data["file"]["download_url"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "MiniMax files/retrieve missing file.download_url".to_string(),
                status_code: None,
                provider_error: Some(data),
                retryable: false,
            })
    }
}

impl Default for MiniMaxVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for MiniMaxVideoProvider {
    fn name(&self) -> &str {
        "minimax"
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
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);
        let url = format!("{}/video_generation", self.api_base());

        let mut body = json!({ "model": native, "prompt": base.prompt });
        if extras.duration_seconds > 0.0 {
            body["duration"] = Value::Number((extras.duration_seconds as i64).into());
        }
        if let Some(res) = extras.resolution.as_deref() {
            body["resolution"] = Value::String(res.to_string());
        }
        // first/last frame images (URL or base64 data URL).
        for r in &materialized.refs {
            let img = match &r.form {
                MaterializedRefForm::Url(u) => Some(u.clone()),
                MaterializedRefForm::Base64(b64) => Some(format!("data:image/png;base64,{b64}")),
                _ => None,
            };
            if let Some(f) = img {
                match r.role.as_str() {
                    "last_frame" => body["last_frame_image"] = Value::String(f),
                    "subject" | "reference" => {
                        body["subject_reference"] = json!([{ "type": "character", "image_file": f }])
                    }
                    _ => body["first_frame_image"] = Value::String(f),
                }
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
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("MiniMax video request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?;

        let status = resp.status();
        let data: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse MiniMax response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        let code = data["base_resp"]["status_code"].as_i64().unwrap_or(0);
        if !status.is_success() || code != 0 {
            let msg = data["base_resp"]["status_msg"].as_str().unwrap_or("Unknown error").to_string();
            return Err(ProviderError::RequestFailed {
                message: format!("MiniMax API error ({code}): {msg}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                retryable: status.as_u16() >= 500,
            });
        }

        let task_id = data["task_id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "MiniMax response missing task_id".to_string(),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        })?;

        Ok(VideoGenerationHandle {
            provider_job_id: task_id.to_string(),
            provider: "minimax".to_string(),
            model: model.id.clone(),
        })
    }

    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/query/video_generation?task_id={}", self.api_base(), handle.provider_job_id);
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(&url))?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: e.to_string(),
                status_code: None,
                provider_error: None,
                retryable: true,
            })?;
        let data: Value = resp.json().await.unwrap_or_default();

        let status_str = data["status"].as_str().unwrap_or("");
        let (status, video_url) = match status_str {
            "Success" => {
                let file_id = data["file_id"].as_str().unwrap_or_default().to_string();
                let url = if file_id.is_empty() { None } else { self.retrieve_file_url(&file_id).await.ok() };
                (GenerationStatus::Completed, url)
            }
            "Fail" => (GenerationStatus::Failed, None),
            _ => (GenerationStatus::Processing, None),
        };

        Ok(VideoGenerationPollResult {
            status,
            progress: if status == GenerationStatus::Completed { 100 } else { 50 },
            video_url,
            video_data: None,
            content_type: Some("video/mp4".into()),
            error: if status == GenerationStatus::Failed {
                Some(data["base_resp"]["status_msg"].as_str().unwrap_or("failed").to_string())
            } else {
                None
            },
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
                message: "MiniMax video provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "MiniMax video provider configured".into(), latency_ms: None }
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

    fn make_provider(api_base: &str) -> MiniMaxVideoProvider {
        let mut p = MiniMaxVideoProvider::new();
        let mut cfg = ProviderInstanceConfig { api_key: "mm-key".to_string(), api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.api_key = Some("mm-key".to_string());
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
        VideoExtras { duration_seconds: 6.0, aspect_ratio: None, resolution: Some("1080P".to_string()), fps: None, extra: None }
    }

    #[tokio::test]
    async fn submits_video_and_polls_to_download_url() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/video_generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "task_id": "mm-task-1", "base_resp": { "status_code": 0, "status_msg": "success" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/query/video_generation"))
            .and(query_param("task_id", "mm-task-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "Success", "file_id": "file-99", "base_resp": { "status_code": 0 }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/files/retrieve"))
            .and(query_param("file_id", "file-99"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "file": { "download_url": "https://cdn.minimax/video.mp4" }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("minimax/MiniMax-Hailuo-02");
        let base = make_base("a drone shot over a canyon", "minimax/MiniMax-Hailuo-02");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "mm-task-1");

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_url.unwrap(), "https://cdn.minimax/video.mp4");
    }
}
