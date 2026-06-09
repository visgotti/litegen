// Fal.ai video generation provider
// API Reference: https://docs.fal.ai/model-apis/model-endpoints/queue
// Queue API: POST https://queue.fal.run/{endpoint} → poll status → fetch result
// Supported models: Kling, MiniMax/Hailuo, AnimateDiff, SVD, LTX Video

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRequest, MaterializedRefForm};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig,
    VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// Fal.ai video generation provider.
///
/// Routes to model-specific Fal endpoints for Kling, MiniMax, AnimateDiff,
/// Stable Video Diffusion, and LTX Video.
///
/// @see <https://docs.fal.ai/model-apis/model-endpoints/queue> — asynchronous queue API (submit → status → result)
/// @see <https://fal.ai/models> — per-model API pages documenting each endpoint's input schema
pub struct FalVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
}

impl FalVideoProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn api_key(&self) -> Result<String, ProviderError> {
        if let Some(pool) = &self.key_pool {
            return Ok(pool.next().to_string());
        }
        self.config
            .as_ref()
            .map(|c| c.api_key.clone())
            .ok_or_else(|| ProviderError::NotConfigured("fal".into()))
    }

    /// The base URL (defaults to https://queue.fal.run, overridable for tests).
    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://queue.fal.run")
    }

    /// Resolve internal model ID to the Fal queue endpoint.
    /// Each model maps to a specific Fal endpoint path.
    ///
    /// @see <https://fal.ai/models> — individual model pages (the endpoint paths returned here)
    fn resolve_endpoint(model: &str) -> &'static str {
        match model {
            // Kling Video v1 standard — image-to-video
            "fal/kling" | "fal-kling" => "fal-ai/kling-video/v1/standard/image-to-video",
            // Kling Video v1.5 pro — higher quality, longer duration
            "fal/kling-pro" | "fal-kling-pro" => "fal-ai/kling-video/v1.5/pro/image-to-video",
            // MiniMax Video — image-to-video
            "fal/minimax" | "fal-minimax" => "fal-ai/minimax-video/image-to-video",
            // MiniMax Hailuo (Video-01-Live) — image-to-video
            "fal/minimax-hailuo" | "fal-minimax-hailuo" => {
                "fal-ai/minimax/video-01-live/image-to-video"
            }
            // AnimateDiff Turbo — text-to-video
            "fal/animate-diff" | "fal-animate-diff-turbo" => {
                "fal-ai/fast-animatediff/turbo/text-to-video"
            }
            // Stable Video Diffusion — image-to-video
            "fal/svd" | "fal-svd" => "fal-ai/stable-video",
            // LTX Video — text/image-to-video
            "fal/ltx-video" | "fal-ltx-video" => "fal-ai/ltx-video",
            // Generic fal/video — default to LTX Video
            _ => "fal-ai/ltx-video",
        }
    }
}

impl Default for FalVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for FalVideoProvider {
    fn name(&self) -> &str {
        "fal"
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
            .is_some_and(|c| !c.api_key.is_empty() || self.key_pool.is_some())
    }

    /// Submit a job to the Fal queue (`POST {queue.fal.run}/{endpoint}`),
    /// returning a `request_id` (plus `status_url`/`response_url`) to poll.
    ///
    /// @see <https://docs.fal.ai/model-apis/model-endpoints/queue> — submit a request; proves the
    ///   `Authorization: Key <token>` auth and the `request_id`/`status_url`/`response_url` response fields.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let api_key = self.api_key()?;
        let endpoint = Self::resolve_endpoint(&model.id);
        let url = format!("{}/{}", self.api_base(), endpoint);

        let mut body = json!({
            "prompt": base.prompt,
        });

        if extras.duration_seconds > 0.0 {
            body["duration"] = json!(extras.duration_seconds);
        }

        if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }

        if let Some(seed) = base.seed {
            body["seed"] = json!(seed);
        }

        // Reference image (init role, URL form preferred by Fal)
        for r in &materialized.refs {
            match &r.form {
                MaterializedRefForm::Url(img_url) => {
                    body["image_url"] = Value::String(img_url.clone());
                    break;
                }
                MaterializedRefForm::Base64(b64) => {
                    body["image_url"] =
                        Value::String(format!("data:image/png;base64,{b64}"));
                    break;
                }
                _ => {}
            }
        }

        // Shallow-merge extra
        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(body_obj) = body.as_object_mut() {
                for (k, v) in extra_map {
                    body_obj.insert(k.clone(), v.clone());
                }
            }
        }

        let resp = crate::providers::inject_trace_headers(
            self.client
                .post(&url)
                .header("Authorization", format!("Key {api_key}"))
                .header("Content-Type", "application/json")
                .json(&body),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("Fal video request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_timeout() || e.is_connect(),
        })?;

        let status = resp.status();
        let resp_json: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Fal video response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        if !status.is_success() {
            let err = resp_json["detail"]
                .as_str()
                .or_else(|| resp_json["message"].as_str())
                .unwrap_or("Unknown error")
                .to_string();
            return Err(ProviderError::RequestFailed {
                message: format!("Fal video API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(resp_json),
                retryable: status.as_u16() >= 500,
            });
        }

        let request_id = resp_json["request_id"]
            .as_str()
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "Fal video response missing request_id".to_string(),
                status_code: None,
                provider_error: Some(resp_json.clone()),
                retryable: false,
            })?
            .to_string();

        // Store request_id, endpoint, and optional status/response URLs for polling
        let job_data = json!({
            "request_id": request_id,
            "endpoint": endpoint,
            "status_url": resp_json["status_url"].as_str().unwrap_or(""),
            "response_url": resp_json["response_url"].as_str().unwrap_or(""),
        });

        Ok(VideoGenerationHandle {
            provider_job_id: job_data.to_string(),
            provider: "fal".to_string(),
            model: model.id.clone(),
        })
    }

    /// Check queue status, then fetch the result when `COMPLETED`
    /// (`GET .../requests/{id}/status`, then `GET .../requests/{id}`).
    ///
    /// @see <https://docs.fal.ai/model-apis/model-endpoints/queue> — checking request status and getting the result;
    ///   proves the `status` values (`IN_QUEUE`/`IN_PROGRESS`/`COMPLETED`/`FAILED`) and the result `video.url` shape.
    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let api_key = self.api_key()?;

        // Parse stored job data
        let job_data: Value =
            serde_json::from_str(&handle.provider_job_id).unwrap_or_default();
        let request_id = job_data["request_id"].as_str().unwrap_or("");
        let endpoint = job_data["endpoint"].as_str().unwrap_or("");

        let status_url = if let Some(url) = job_data["status_url"].as_str() {
            if !url.is_empty() {
                url.to_string()
            } else {
                format!(
                    "https://queue.fal.run/{endpoint}/requests/{request_id}/status"
                )
            }
        } else {
            format!("https://queue.fal.run/{endpoint}/requests/{request_id}/status")
        };

        // Poll status — https://docs.fal.ai/model-apis/model-endpoints/queue
        let resp = crate::providers::inject_trace_headers(
            self.client
                .get(&status_url)
                .header("Authorization", format!("Key {api_key}")),
        )
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
            Some("COMPLETED") => GenerationStatus::Completed,
            Some("FAILED") => GenerationStatus::Failed,
            Some("IN_PROGRESS") => GenerationStatus::Processing,
            Some("IN_QUEUE") => GenerationStatus::Pending,
            _ => GenerationStatus::Pending,
        };

        // If completed, fetch the result
        if status == GenerationStatus::Completed {
            let response_url = if let Some(url) = job_data["response_url"].as_str() {
                if !url.is_empty() {
                    url.to_string()
                } else {
                    format!(
                        "https://queue.fal.run/{endpoint}/requests/{request_id}"
                    )
                }
            } else {
                format!("https://queue.fal.run/{endpoint}/requests/{request_id}")
            };

            let result_resp = crate::providers::inject_trace_headers(
                self.client
                    .get(&response_url)
                    .header("Authorization", format!("Key {api_key}")),
            )
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: e.to_string(),
                status_code: None,
                provider_error: None,
                retryable: true,
            })?;

            let result: Value = result_resp.json().await.unwrap_or_default();

            // Video URL is in various locations depending on the model
            let video_url = result["video"]["url"]
                .as_str()
                .or_else(|| result["video_url"].as_str())
                .or_else(|| {
                    result["videos"]
                        .as_array()
                        .and_then(|a| a.first())
                        .and_then(|v| v["url"].as_str())
                })
                .or_else(|| result["output"]["url"].as_str())
                .map(String::from);

            return Ok(VideoGenerationPollResult {
                status: GenerationStatus::Completed,
                progress: 100,
                video_url,
                video_data: None,
                content_type: Some("video/mp4".into()),
                error: None,
                metadata: HashMap::new(),
            });
        }

        let error = if status == GenerationStatus::Failed {
            data["error"].as_str().map(String::from)
        } else {
            None
        };

        Ok(VideoGenerationPollResult {
            status,
            progress: match status {
                GenerationStatus::Processing => 50,
                GenerationStatus::Pending => 10,
                _ => 0,
            },
            video_url: None,
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

    /// Validate the API key via `GET https://rest.alpha.fal.ai/tokens/current`
    /// (Fal's token-introspection endpoint).
    ///
    /// @see <https://docs.fal.ai/authentication/key-based> — key-based authentication
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Fal Video provider not configured".into(),
                latency_ms: None,
            };
        }
        let start = std::time::Instant::now();
        let api_key = match self.api_key() {
            Ok(k) => k,
            Err(_) => {
                return HealthCheckResult {
                    healthy: false,
                    message: "No API key".into(),
                    latency_ms: None,
                }
            }
        };
        match self
            .client
            .get("https://rest.alpha.fal.ai/tokens/current")
            .header("Authorization", format!("Key {api_key}"))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "Fal API key valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("Fal returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("Fal health check failed: {e}"),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::{method, path};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn empty_materialized() -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: vec![],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    fn make_provider(api_base: &str) -> FalVideoProvider {
        let mut p = FalVideoProvider::new();
        p.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: "test-key".to_string(),
            api_keys: vec![],
            api_base: Some(api_base.to_string()),
            model_mapping: Default::default(),
            extra_headers: Default::default(),
            options: None,
        });
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
            duration_seconds: 5.0,
            aspect_ratio: None,
            resolution: None,
            fps: None,
            extra: None,
        }
    }

    #[tokio::test]
    async fn submits_fal_video_job_and_returns_handle() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/fal-ai/ltx-video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "request_id": "fal_video_1",
                "status": "IN_QUEUE",
                "status_url": "",
                "response_url": ""
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("fal/video");
        let base = make_base("a robot walking through a city", "fal/video");
        let extras = make_extras();
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let handle = result.unwrap();

        // provider_job_id is JSON containing request_id
        let job_data: Value = serde_json::from_str(&handle.provider_job_id).unwrap();
        assert_eq!(job_data["request_id"], "fal_video_1");

        assert_eq!(handle.provider, "fal");
        assert_eq!(handle.model, "fal/video");

        // Verify outbound request body
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["prompt"], "a robot walking through a city");
    }
}
