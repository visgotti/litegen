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

/// Google Veo video generation provider (Gemini Developer API).
///
/// Text-to-video and image-to-video (first/last keyframe + reference images).
/// Generation is asynchronous: the create call returns a long-running operation
/// `name`; poll the operation until `done`, then read the sample `video.uri`.
/// Authenticated with the same Google API key used by the image provider, sent
/// as the `x-goog-api-key` header.
///
/// NOTE: the returned `video.uri` must be downloaded with the API key (it is not
/// a public CDN URL). This provider returns the uri as `video_url`; re-hosting
/// auth-gated outputs to litegen storage is a tracked cross-provider follow-up
/// (shared with Bedrock Nova Reel).
///
/// @see <https://ai.google.dev/gemini-api/docs/video> — Veo generate + poll
///   Verbatim:
///     curl -s "${BASE_URL}/models/veo-3.1-generate-preview:predictLongRunning" \
///     -H "x-goog-api-key: $GEMINI_API_KEY" \
///     status_response=$(curl -s -H "x-goog-api-key: $GEMINI_API_KEY" "${BASE_URL}/${operation_name}")
pub struct GoogleVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl GoogleVideoProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::raw_header("x-goog-api-key"),
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
            .unwrap_or("https://generativelanguage.googleapis.com/v1beta")
    }

    /// Resolve the per-request credentials, honoring the weighted key pool.
    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("google".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// Strip the `google/` prefix; the remainder is the native Veo model id.
    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("google/").unwrap_or(model_id)
    }
}

impl Default for GoogleVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for GoogleVideoProvider {
    fn name(&self) -> &str {
        "google"
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

    /// Submit a generation via `POST {base}/models/{model}:predictLongRunning`,
    /// returning the operation `name` as the job handle.
    ///
    /// @see <https://ai.google.dev/gemini-api/docs/video> — `instances[].prompt`,
    ///   `instances[].image.inlineData`, `instances[].lastFrame`, and
    ///   `parameters.{aspectRatio,durationSeconds,resolution}`.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);
        let url = format!("{}/models/{native}:predictLongRunning", self.api_base());

        // Build the instance (prompt + optional keyframes as base64 inlineData).
        let mut instance = json!({ "prompt": base.prompt });
        for r in &materialized.refs {
            if let MaterializedRefForm::Base64(b64) = &r.form {
                let img = json!({ "inlineData": { "mimeType": "image/png", "data": b64 } });
                match r.role.as_str() {
                    "first_frame" | "init" => instance["image"] = img,
                    "last_frame" => instance["lastFrame"] = img,
                    _ => {}
                }
            }
        }

        // Build the parameters block.
        let mut parameters = json!({});
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            parameters["aspectRatio"] = Value::String(ar.to_string());
        }
        if extras.duration_seconds > 0.0 {
            parameters["durationSeconds"] = Value::Number((extras.duration_seconds as i64).into());
        }
        if let Some(res) = extras.resolution.as_deref() {
            parameters["resolution"] = Value::String(res.to_string());
        }
        if let Some(np) = base.negative_prompt.as_deref() {
            parameters["negativePrompt"] = Value::String(np.to_string());
        }
        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(obj) = parameters.as_object_mut() {
                for (k, v) in extra_map {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let body = json!({ "instances": [instance], "parameters": parameters });

        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Google Veo request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?;

        let status = resp.status();
        let resp_json: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Google Veo response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        if !status.is_success() {
            let err = resp_json["error"]["message"].as_str().unwrap_or("Unknown error").to_string();
            return Err(ProviderError::RequestFailed {
                message: format!("Google Veo API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(resp_json),
                retryable: status.as_u16() >= 500,
            });
        }

        let name = resp_json["name"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Google Veo response missing operation name".to_string(),
            status_code: None,
            provider_error: Some(resp_json.clone()),
            retryable: false,
        })?;

        Ok(VideoGenerationHandle {
            provider_job_id: name.to_string(),
            provider: "google".to_string(),
            model: model.id.clone(),
        })
    }

    /// Poll the long-running operation via `GET {base}/{operation_name}`.
    ///
    /// @see <https://ai.google.dev/gemini-api/docs/video> — operation `done` +
    ///   `response.generateVideoResponse.generatedSamples[0].video.uri`.
    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/{}", self.api_base(), handle.provider_job_id);

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

        // Operation-level error → failed.
        if let Some(err) = data["error"]["message"].as_str() {
            return Ok(VideoGenerationPollResult {
                status: GenerationStatus::Failed,
                progress: 0,
                video_url: None,
                video_data: None,
                content_type: Some("video/mp4".into()),
                error: Some(err.to_string()),
                metadata: HashMap::new(),
            });
        }

        let done = data["done"].as_bool().unwrap_or(false);
        let video_url = data["response"]["generateVideoResponse"]["generatedSamples"][0]["video"]["uri"]
            .as_str()
            .map(String::from);

        let status = if done && video_url.is_some() {
            GenerationStatus::Completed
        } else if done {
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
            error: None,
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

    /// Validate the API key via `GET {generativelanguage}/v1beta/models`.
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Google video provider not configured".into(),
                latency_ms: None,
            };
        }
        let start = std::time::Instant::now();
        let creds = match self.creds() {
            Ok(c) => c,
            Err(_) => {
                return HealthCheckResult { healthy: false, message: "No API key".into(), latency_ms: None }
            }
        };
        let builder = match crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.get(format!("{}/models", self.api_base())).timeout(std::time::Duration::from_secs(10)),
        ) {
            Ok(b) => b,
            Err(e) => {
                return HealthCheckResult { healthy: false, message: e.to_string(), latency_ms: None }
            }
        };
        match builder.send().await {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "Google API key valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("Google returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("Google health check failed: {e}"),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::{Cleanup, MaterializedRef};
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> GoogleVideoProvider {
        let mut p = GoogleVideoProvider::new();
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

    fn make_extras() -> VideoExtras {
        VideoExtras {
            duration_seconds: 8.0,
            aspect_ratio: Some("16:9".to_string()),
            resolution: Some("720p".to_string()),
            fps: None,
            extra: None,
        }
    }

    fn materialized_first_frame(b64: &str) -> MaterializedRequest {
        MaterializedRequest {
            refs: vec![MaterializedRef {
                role: "first_frame".to_string(),
                form: MaterializedRefForm::Base64(b64.to_string()),
            }],
            cleanup: Cleanup::empty(),
        }
    }

    #[tokio::test]
    async fn submits_veo_job_and_sends_api_key_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/models/veo-3\.0-generate-001:predictLongRunning"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "models/veo-3.0-generate-001/operations/abc123"
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&format!("{}/v1beta", server.uri()));
        let schema = ref_schema("google/veo-3.0-generate-001");
        let base = make_base("a cat surfing a wave", "google/veo-3.0-generate-001");
        let extras = make_extras();
        let materialized = materialized_first_frame("Zm9vYmFy");

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let handle = result.unwrap();
        assert_eq!(handle.provider_job_id, "models/veo-3.0-generate-001/operations/abc123");
        assert_eq!(handle.provider, "google");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        // Auth header present.
        assert_eq!(received[0].headers.get("x-goog-api-key").unwrap(), "test-key");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["instances"][0]["prompt"], "a cat surfing a wave");
        assert_eq!(body["instances"][0]["image"]["inlineData"]["data"], "Zm9vYmFy");
        assert_eq!(body["parameters"]["aspectRatio"], "16:9");
        assert_eq!(body["parameters"]["durationSeconds"], 8);
        assert_eq!(body["parameters"]["resolution"], "720p");
    }

    #[tokio::test]
    async fn poll_returns_completed_with_video_uri() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"/models/veo-3\.0-generate-001/operations/abc123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "models/veo-3.0-generate-001/operations/abc123",
                "done": true,
                "response": {
                    "generateVideoResponse": {
                        "generatedSamples": [
                            { "video": { "uri": "https://generativelanguage.googleapis.com/v1beta/files/xyz:download" } }
                        ]
                    }
                }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&format!("{}/v1beta", server.uri()));
        let handle = VideoGenerationHandle {
            provider_job_id: "models/veo-3.0-generate-001/operations/abc123".to_string(),
            provider: "google".to_string(),
            model: "google/veo-3.0-generate-001".to_string(),
        };

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.progress, 100);
        assert!(poll.video_url.unwrap().contains("/files/xyz:download"));
    }
}
