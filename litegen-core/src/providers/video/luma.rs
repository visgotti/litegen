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

/// Luma Dream Machine video generation provider.
///
/// Supports Ray model family (ray-2, ray-flash-2, ray-3, ray-hdr-3, etc.)
/// with text-to-video, image-to-video (keyframes), and camera motion control.
///
/// @see <https://docs.lumalabs.ai/reference/creategeneration> — create a generation
/// @see <https://docs.lumalabs.ai/reference/getgeneration> — get a generation (poll)
/// @see <https://docs.lumalabs.ai/docs/api> — Dream Machine API overview
pub struct LumaProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
}

impl LumaProvider {
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

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.lumalabs.ai/dream-machine/v1")
    }

    fn api_key(&self) -> Result<String, ProviderError> {
        if let Some(pool) = &self.key_pool {
            return Ok(pool.next().to_string());
        }
        self.config
            .as_ref()
            .map(|c| c.api_key.clone())
            .ok_or_else(|| ProviderError::NotConfigured("luma".into()))
    }

    /// Map internal model IDs to the actual Luma API model name.
    ///
    /// @see <https://docs.lumalabs.ai/reference/creategeneration> — `model` field accepted values
    fn resolve_model(model: &str) -> &'static str {
        match model {
            "luma/dream-machine" | "luma-dream-machine" => "ray-2",
            "luma/dream-machine-1.5" | "luma-dream-machine-1.5" => "ray-1-6",
            "luma/ray-2" | "ray-2" => "ray-2",
            "luma/ray-flash-2" | "ray-flash-2" => "ray-flash-2",
            "luma/ray-3" | "ray-3" => "ray-3",
            "luma/ray-hdr-3" | "ray-hdr-3" => "ray-hdr-3",
            "luma/ray-3-14" | "ray-3-14" => "ray-3-14",
            "luma/ray-hdr-3-14" | "ray-hdr-3-14" => "ray-hdr-3-14",
            "luma/ray-1-6" | "ray-1-6" => "ray-1-6",
            _ => "ray-2",
        }
    }

    /// Map camera motion values to Luma API format.
    fn map_camera_motion(motion: &str) -> String {
        match motion {
            "static" => "static".to_string(),
            "pan-left" | "pan_left" => "pan left".to_string(),
            "pan-right" | "pan_right" => "pan right".to_string(),
            "pan-up" | "pan_up" => "pan up".to_string(),
            "pan-down" | "pan_down" => "pan down".to_string(),
            "zoom-in" | "zoom_in" => "zoom in".to_string(),
            "zoom-out" | "zoom_out" => "zoom out".to_string(),
            "orbit" | "orbit-left" | "orbit_left" => "orbit left".to_string(),
            "orbit-right" | "orbit_right" => "orbit right".to_string(),
            other => other.to_string(),
        }
    }
}

impl Default for LumaProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for LumaProvider {
    fn name(&self) -> &str {
        "luma"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.api_keys.is_empty() {
            self.key_pool = Some(ApiKeyPool::new(config.api_keys.clone()));
        }
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| !c.api_key.is_empty() || self.key_pool.is_some())
    }

    /// Submit a generation (`POST {api_base}/generations`), returning a job `id`.
    ///
    /// @see <https://docs.lumalabs.ai/reference/creategeneration> — create a generation.
    ///   Proves the `model`/`prompt`/`aspect_ratio`/`camera_motion` fields, the `keyframes.frame0`/
    ///   `frame1` `{type: "image", url}` shape, and the `id` response field.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let api_key = self.api_key()?;
        let model_name = Self::resolve_model(&model.id);
        let url = format!("{}/generations", self.api_base());

        let mut body = json!({
            "model": model_name,
            "prompt": base.prompt,
        });

        // Aspect ratio
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }

        // Duration — Luma expects a string like "5s"
        if extras.duration_seconds > 0.0 {
            body["duration"] = Value::String(format!("{}s", extras.duration_seconds as u32));
        }

        // Resolution — Luma expects a string like "720p"/"1080p"/"4k"
        if let Some(res) = extras.resolution.as_deref() {
            body["resolution"] = Value::String(res.to_string());
        }

        // Camera motion from extra
        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(motion) = extra_map.get("camera_motion").and_then(|v| v.as_str()) {
                body["camera_motion"] = Value::String(Self::map_camera_motion(motion));
            }
        }

        // Keyframes from materialized refs
        // first_frame → frame0, last_frame → frame1
        let mut keyframes = serde_json::Map::new();
        for r in &materialized.refs {
            let frame_key = match r.role.as_str() {
                "first_frame" => "frame0",
                "last_frame" => "frame1",
                _ => continue,
            };
            match &r.form {
                MaterializedRefForm::Url(img_url) => {
                    keyframes.insert(
                        frame_key.to_string(),
                        json!({ "type": "image", "url": img_url }),
                    );
                }
                MaterializedRefForm::Base64(b64) => {
                    keyframes.insert(
                        frame_key.to_string(),
                        json!({ "type": "image", "url": format!("data:image/png;base64,{b64}") }),
                    );
                }
                _ => {}
            }
        }
        if !keyframes.is_empty() {
            body["keyframes"] = Value::Object(keyframes);
        }

        // Shallow-merge extra (except camera_motion already handled)
        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(body_obj) = body.as_object_mut() {
                for (k, v) in extra_map {
                    if k != "camera_motion" {
                        body_obj.insert(k.clone(), v.clone());
                    }
                }
            }
        }

        let resp = crate::providers::inject_trace_headers(
            self.client
                .post(&url)
                .header("Authorization", format!("Bearer {api_key}"))
                .header("Content-Type", "application/json")
                .json(&body),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("Luma request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_timeout() || e.is_connect(),
        })?;

        let status = resp.status();
        let resp_json: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Luma response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        if !status.is_success() {
            let err = resp_json["message"]
                .as_str()
                .or_else(|| resp_json["error"].as_str())
                .unwrap_or("Unknown error")
                .to_string();
            return Err(ProviderError::RequestFailed {
                message: format!("Luma API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(resp_json),
                retryable: status.as_u16() >= 500,
            });
        }

        let job_id = resp_json["id"]
            .as_str()
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "Luma response missing id".to_string(),
                status_code: None,
                provider_error: Some(resp_json.clone()),
                retryable: false,
            })?
            .to_string();

        Ok(VideoGenerationHandle {
            provider_job_id: job_id,
            provider: "luma".to_string(),
            model: model.id.clone(),
        })
    }

    /// Poll a generation (`GET {api_base}/generations/{id}`), reading `state`
    /// (`completed`/`failed`/`dreaming`) and `assets.video`.
    ///
    /// @see <https://docs.lumalabs.ai/reference/getgeneration> — get a generation
    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let api_key = self.api_key()?;

        let resp = crate::providers::inject_trace_headers(
            self.client
                .get(format!(
                    "{}/generations/{}",
                    self.api_base(),
                    handle.provider_job_id
                ))
                .header("Authorization", format!("Bearer {api_key}")),
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

        let status = match data["state"].as_str() {
            Some("completed") => GenerationStatus::Completed,
            Some("failed") => GenerationStatus::Failed,
            Some("dreaming") | Some("processing") => GenerationStatus::Processing,
            _ => GenerationStatus::Pending,
        };

        let video_url = data["assets"]["video"].as_str().map(String::from);

        let error = data["failure_reason"].as_str().map(String::from);

        Ok(VideoGenerationPollResult {
            status,
            progress: if status == GenerationStatus::Completed { 100 } else { 50 },
            video_url,
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

    /// Validate the API key via `GET {api_base}/generations?limit=1`.
    ///
    /// @see <https://docs.lumalabs.ai/reference/listgenerations> — list generations
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Luma provider not configured".into(),
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
            .get(format!("{}/generations?limit=1", self.api_base()))
            .header("Authorization", format!("Bearer {api_key}"))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "Luma API key valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("Luma returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("Luma health check failed: {e}"),
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
    use crate::proxy::materializer::{MaterializedRef, MaterializedRefForm};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    #[allow(dead_code)] // kept for symmetry with other provider test helpers
    fn empty_materialized() -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: vec![],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    fn materialized_with_first_frame(url: &str) -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: vec![MaterializedRef {
                role: "first_frame".to_string(),
                form: MaterializedRefForm::Url(url.to_string()),
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    fn make_provider(api_base: &str) -> LumaProvider {
        let mut p = LumaProvider::new();
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
    async fn submits_luma_job_with_first_frame_keyframe() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "luma_job_1",
                "state": "dreaming"
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("luma/dream-machine");
        let base = make_base("a sunset over the ocean", "luma/dream-machine");
        let extras = make_extras();
        let materialized = materialized_with_first_frame("https://x");

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let handle = result.unwrap();

        assert_eq!(handle.provider_job_id, "luma_job_1");
        assert_eq!(handle.provider, "luma");

        // Verify outbound request body has frame0 keyframe
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "ray-2");
        assert_eq!(body["prompt"], "a sunset over the ocean");
        assert_eq!(body["keyframes"]["frame0"]["url"], "https://x");
        assert_eq!(body["keyframes"]["frame0"]["type"], "image");
    }
}
