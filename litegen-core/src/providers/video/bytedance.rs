use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt::Write as _;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig,
    VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// ByteDance Seedance video provider (Volcengine Ark / BytePlus ModelArk).
///
/// Async, task-based (NOT OpenAI-compatible): `POST /api/v3/contents/generations/
/// tasks` with a `content` array (a text block carrying `--flags` for
/// resolution/duration/ratio, plus optional `image_url` blocks) returns `{id}`;
/// poll `GET /api/v3/contents/generations/tasks/{id}` until `status ==
/// "succeeded"`, then read `content.video_url`. Bearer (Ark API key).
///
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1520757> — create video task
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1521309> — retrieve task
///   (status ∈ queued|running|succeeded|failed|cancelled; result at content.video_url)
///   Verbatim (apidog cross-ref): "POST … /api/v3/contents/generations/tasks ; GET … /api/v3/contents/generations/tasks/{task_id} ; Authorization: Bearer YOUR_ARK_API_KEY"
pub struct ByteDanceVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl ByteDanceVideoProvider {
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

impl Default for ByteDanceVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for ByteDanceVideoProvider {
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
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);
        let url = format!("{}/contents/generations/tasks", self.api_base());

        // Seedance encodes parameters as --flags appended to the text prompt.
        let mut text = base.prompt.clone();
        if let Some(res) = extras.resolution.as_deref() {
            let _ = write!(text, " --resolution {res}");
        }
        if extras.duration_seconds > 0.0 {
            let _ = write!(text, " --duration {}", extras.duration_seconds as i64);
        }
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            let _ = write!(text, " --ratio {ar}");
        }

        let mut content: Vec<Value> = vec![json!({ "type": "text", "text": text })];
        for r in &materialized.refs {
            let u = match &r.form {
                MaterializedRefForm::Url(u) => Some(u.clone()),
                MaterializedRefForm::Base64(b64) => Some(format!("data:image/png;base64,{b64}")),
                _ => None,
            };
            if let Some(u) = u {
                let mut block = json!({ "type": "image_url", "image_url": { "url": u } });
                if r.role == "last_frame" {
                    block["role"] = Value::String("last_frame".to_string());
                }
                content.push(block);
            }
        }

        let mut body = json!({ "model": native, "content": content });
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
                message: format!("ByteDance video request failed: {e}"),
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

        let id = data["id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "ByteDance response missing task id".to_string(),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        })?;

        Ok(VideoGenerationHandle {
            provider_job_id: id.to_string(),
            provider: "bytedance".to_string(),
            model: model.id.clone(),
        })
    }

    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/contents/generations/tasks/{}", self.api_base(), handle.provider_job_id);
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

        let status = match data["status"].as_str() {
            Some("succeeded") => GenerationStatus::Completed,
            Some("failed") | Some("cancelled") => GenerationStatus::Failed,
            _ => GenerationStatus::Processing,
        };
        let video_url = data["content"]["video_url"].as_str().map(String::from);

        Ok(VideoGenerationPollResult {
            status,
            progress: if status == GenerationStatus::Completed { 100 } else { 50 },
            video_url,
            video_data: None,
            content_type: Some("video/mp4".into()),
            error: data["error"]["message"].as_str().map(String::from),
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
                message: "ByteDance video provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "ByteDance video provider configured".into(), latency_ms: None }
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

    fn make_provider(api_base: &str) -> ByteDanceVideoProvider {
        let mut p = ByteDanceVideoProvider::new();
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

    fn make_extras() -> VideoExtras {
        VideoExtras { duration_seconds: 5.0, aspect_ratio: Some("16:9".to_string()), resolution: Some("1080p".to_string()), fps: None, extra: None }
    }

    #[tokio::test]
    async fn submits_seedance_task_and_polls() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/contents/generations/tasks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "sd-task-1" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/contents/generations/tasks/sd-task-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "sd-task-1", "status": "succeeded", "content": { "video_url": "https://cdn.ark/v.mp4" }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("bytedance/doubao-seedance-1-0-pro-250528");
        let base = make_base("a timelapse of city traffic", "bytedance/doubao-seedance-1-0-pro-250528");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "sd-task-1");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].headers.get("authorization").unwrap(), "Bearer ark-key");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "doubao-seedance-1-0-pro-250528");
        let text = body["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("--resolution 1080p"), "text: {text}");
        assert!(text.contains("--duration 5"), "text: {text}");

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_url.unwrap(), "https://cdn.ark/v.mp4");
    }
}
