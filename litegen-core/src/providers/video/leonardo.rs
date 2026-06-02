use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::image::leonardo::upload_init_image;
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig,
    VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// Leonardo.Ai video generation provider (image-to-video / Motion / Veo3 / Kling).
///
/// Async: `POST /generations-image-to-video` (a start frame `imageId` is
/// required) returns a generation id; poll `GET /generations/{id}` until
/// `generations_by_pk.status == "COMPLETE"`, then read
/// `generated_images[0].motionMP4URL`. Bearer auth; start frame uploaded via the
/// shared presigned init-image flow.
///
/// @see <https://docs.leonardo.ai/reference/createimagetovideogeneration>
///   (required: prompt, imageId, imageType; optional: model [MOTION2/VEO3/...],
///    resolution [RESOLUTION_720/...], duration)
/// @see <https://docs.leonardo.ai/docs/generate-with-veo3-veo3-fast-using-start-frame>
///   Verbatim: "\"model\": \"VEO3\", \"imageType\": \"UPLOADED\", \"resolution\": \"RESOLUTION_720\", \"duration\": 8"
pub struct LeonardoVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl LeonardoVideoProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://cloud.leonardo.ai/api/rest/v1")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("leonardo".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// Map the litegen model id to a Leonardo motion model enum.
    fn resolve_model(model_id: &str) -> &'static str {
        match model_id.strip_prefix("leonardo/").unwrap_or(model_id) {
            "motion2" | "motion-2" => "MOTION2",
            "motion2-fast" => "MOTION2FAST",
            "veo3" => "VEO3",
            "veo3-fast" => "VEO3FAST",
            "kling2.1" | "kling-2.1" => "KLING2_1",
            "kling2.5" | "kling-2.5" => "KLING2_5",
            _ => "MOTION2",
        }
    }

    fn resolve_resolution(res: Option<&str>) -> &'static str {
        match res {
            Some("1080p") | Some("1080P") => "RESOLUTION_1080",
            Some("480p") | Some("480P") => "RESOLUTION_480",
            _ => "RESOLUTION_720",
        }
    }
}

impl Default for LeonardoVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for LeonardoVideoProvider {
    fn name(&self) -> &str {
        "leonardo"
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
        let leo_model = Self::resolve_model(&model.id);

        // A start-frame image is required for image-to-video.
        let mut image_id: Option<String> = None;
        let mut image_type = "UPLOADED";
        for r in &materialized.refs {
            if let MaterializedRefForm::MultipartField { bytes, .. } = &r.form {
                image_id = Some(upload_init_image(&self.client, self.api_base(), &self.auth, &creds, bytes).await?);
                break;
            }
        }
        // Allow passing a pre-existing generated/uploaded image id via extra.
        if image_id.is_none() {
            if let Some(Value::Object(map)) = &extras.extra {
                if let Some(id) = map.get("imageId").and_then(|v| v.as_str()) {
                    image_id = Some(id.to_string());
                    if let Some(t) = map.get("imageType").and_then(|v| v.as_str()) {
                        image_type = if t == "GENERATED" { "GENERATED" } else { "UPLOADED" };
                    }
                }
            }
        }
        let image_id = image_id.ok_or_else(|| {
            ProviderError::InvalidRequest("leonardo video requires a start-frame reference image".into())
        })?;

        let mut body = json!({
            "prompt": base.prompt,
            "imageId": image_id,
            "imageType": image_type,
            "model": leo_model,
            "resolution": Self::resolve_resolution(extras.resolution.as_deref()),
        });
        if extras.duration_seconds > 0.0 {
            body["duration"] = Value::Number((extras.duration_seconds as i64).into());
        }
        if let Some(Value::Object(map)) = &extras.extra {
            if let Some(obj) = body.as_object_mut() {
                for (k, v) in map {
                    if k != "imageId" && k != "imageType" {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            }
        }

        let url = format!("{}/generations-image-to-video", self.api_base());
        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let submit: Value = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Leonardo video request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?
            .json()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to parse Leonardo video response: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;

        // The generation id appears under one of these job keys depending on model.
        let gen_id = submit["motionVideoGenerationJob"]["generationId"]
            .as_str()
            .or_else(|| submit["sdGenerationJob"]["generationId"].as_str())
            .or_else(|| submit["motionSvdGenerationJob"]["generationId"].as_str())
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "Leonardo video response missing generationId".to_string(),
                status_code: None,
                provider_error: Some(submit.clone()),
                retryable: false,
            })?;

        Ok(VideoGenerationHandle {
            provider_job_id: gen_id.to_string(),
            provider: "leonardo".to_string(),
            model: model.id.clone(),
        })
    }

    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/generations/{}", self.api_base(), handle.provider_job_id);
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

        let pk = &data["generations_by_pk"];
        let status = match pk["status"].as_str() {
            Some("COMPLETE") => GenerationStatus::Completed,
            Some("FAILED") => GenerationStatus::Failed,
            _ => GenerationStatus::Processing,
        };
        // Motion/video URL is on the generated image as motionMP4URL.
        let video_url = pk["generated_images"][0]["motionMP4URL"]
            .as_str()
            .or_else(|| pk["generated_images"][0]["url"].as_str())
            .map(String::from);

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

    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Leonardo video provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Leonardo video provider configured".into(), latency_ms: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::{Cleanup, MaterializedRef};
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> LeonardoVideoProvider {
        let mut p = LeonardoVideoProvider::new();
        let mut cfg = ProviderInstanceConfig { api_key: "leo-key".to_string(), api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.api_key = Some("leo-key".to_string());
        p.configure(cfg);
        p
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(), model: model.to_string(), n: 1, negative_prompt: None,
            seed: None, reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    fn extras_with_image_id() -> VideoExtras {
        VideoExtras {
            duration_seconds: 8.0,
            aspect_ratio: None,
            resolution: Some("720p".to_string()),
            fps: None,
            // Provide a pre-existing imageId so the test skips the upload step.
            extra: Some(json!({ "imageId": "init-123", "imageType": "UPLOADED" })),
        }
    }

    #[tokio::test]
    async fn submits_image_to_video_and_polls() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/generations-image-to-video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "motionVideoGenerationJob": { "generationId": "leo-vid-1", "apiCreditCost": 2500 }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/generations/leo-vid-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "generations_by_pk": { "status": "COMPLETE",
                    "generated_images": [{ "id": "i1", "motionMP4URL": "https://cdn.leo/v.mp4" }] }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("leonardo/veo3");
        let base = make_base("a slow zoom over mountains", "leonardo/veo3");
        let extras = extras_with_image_id();
        let materialized = MaterializedRequest { refs: vec![] as Vec<MaterializedRef>, cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "leo-vid-1");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "VEO3");
        assert_eq!(body["imageId"], "init-123");
        assert_eq!(body["resolution"], "RESOLUTION_720");
        assert_eq!(body["duration"], 8);

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_url.unwrap(), "https://cdn.leo/v.mp4");
    }
}
