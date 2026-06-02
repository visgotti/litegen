use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig,
    VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// PixVerse video generation provider.
///
/// Reference images use a two-step upload: `POST /openapi/v2/image/upload`
/// (multipart) returns an integer `img_id`, which is then referenced in the
/// generate call. Generate is async: `POST /openapi/v2/video/{text,img,
/// transition}/generate` returns `Resp.video_id`; poll `GET /openapi/v2/video/
/// result/{video_id}` until `Resp.status == 1`. Auth: `API-KEY` header plus a
/// fresh `Ai-trace-id` (UUID) per request. All responses use an
/// `{ErrCode, ErrMsg, Resp}` envelope. Video-only.
///
/// @see <https://docs.platform.pixverse.ai/how-does-the-api-work-882967m0>
///   Verbatim: "A different Ai-trace-id for each unique request ... If you use the same ai-trace-id multiple times, You won't get a new video generated"
/// @see <https://docs.platform.pixverse.ai/transitionfirst-last-frame-feature-882973m0>
///   Verbatim: "\"first_frame_img\": 0, \"last_frame_img\": 0"
pub struct PixverseProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl PixverseProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::raw_header("API-KEY"),
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
            .unwrap_or("https://app-api.pixverse.ai")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("pixverse".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("pixverse/").unwrap_or(model_id)
    }

    /// Apply API-KEY auth + a fresh per-request Ai-trace-id.
    fn auth_builder(
        &self,
        creds: &ProviderCredentials,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        let traced = builder.header("Ai-trace-id", Uuid::new_v4().to_string());
        crate::providers::auth::apply(&self.auth, creds, traced)
    }

    /// Upload reference bytes, returning the integer img_id.
    async fn upload_image(&self, creds: &ProviderCredentials, bytes: &[u8], content_type: &str) -> Result<i64, ProviderError> {
        let url = format!("{}/openapi/v2/image/upload", self.api_base());
        let part = Part::bytes(bytes.to_vec())
            .file_name("ref.png")
            .mime_str(content_type)
            .map_err(|e| ProviderError::InvalidRequest(format!("pixverse ref mime: {e}")))?;
        let form = Form::new().part("image", part);
        let builder = self.auth_builder(creds, self.client.post(&url).multipart(form))?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Pixverse upload failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: true,
            })?;
        let data: Value = resp.json().await.unwrap_or_default();
        if data["ErrCode"].as_i64().unwrap_or(-1) != 0 {
            return Err(ProviderError::RequestFailed {
                message: format!("Pixverse upload error: {}", data["ErrMsg"].as_str().unwrap_or("unknown")),
                status_code: None,
                provider_error: Some(data),
                retryable: false,
            });
        }
        data["Resp"]["img_id"].as_i64().ok_or_else(|| ProviderError::RequestFailed {
            message: "Pixverse upload missing Resp.img_id".to_string(),
            status_code: None,
            provider_error: Some(data),
            retryable: false,
        })
    }
}

impl Default for PixverseProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for PixverseProvider {
    fn name(&self) -> &str {
        "pixverse"
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
        let native = Self::resolve_model(&model.id);

        // Upload any reference images first (-> img_id), tracking first/last.
        let mut first_img: Option<i64> = None;
        let mut last_img: Option<i64> = None;
        for r in &materialized.refs {
            if let MaterializedRefForm::MultipartField { bytes, content_type, .. } = &r.form {
                let img_id = self.upload_image(&creds, bytes, content_type).await?;
                if r.role == "last_frame" {
                    last_img = Some(img_id);
                } else {
                    first_img = Some(img_id);
                }
            }
        }

        // Choose endpoint by what was uploaded.
        let (path, mut body) = if first_img.is_some() && last_img.is_some() {
            (
                "/openapi/v2/video/transition/generate",
                json!({ "first_frame_img": first_img, "last_frame_img": last_img }),
            )
        } else if let Some(id) = first_img {
            ("/openapi/v2/video/img/generate", json!({ "img_id": id }))
        } else {
            ("/openapi/v2/video/text/generate", json!({}))
        };
        body["model"] = Value::String(native.to_string());
        body["prompt"] = Value::String(base.prompt.clone());
        if let Some(res) = extras.resolution.as_deref() {
            body["quality"] = Value::String(res.to_string());
        }
        if extras.duration_seconds > 0.0 {
            body["duration"] = Value::Number((extras.duration_seconds as i64).into());
        }
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }
        if let Some(np) = base.negative_prompt.as_deref() {
            body["negative_prompt"] = Value::String(np.to_string());
        }
        if let Some(Value::Object(map)) = &extras.extra {
            if let Some(obj) = body.as_object_mut() {
                for (k, v) in map {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let url = format!("{}{path}", self.api_base());
        let builder = self.auth_builder(&creds, self.client.post(&url).header("Content-Type", "application/json").json(&body))?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Pixverse request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?;

        let status = resp.status();
        let data: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Pixverse response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        if data["ErrCode"].as_i64().unwrap_or(-1) != 0 {
            return Err(ProviderError::RequestFailed {
                message: format!("Pixverse API error: {}", data["ErrMsg"].as_str().unwrap_or("unknown")),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                retryable: false,
            });
        }

        let video_id = data["Resp"]["video_id"].as_i64().ok_or_else(|| ProviderError::RequestFailed {
            message: "Pixverse response missing Resp.video_id".to_string(),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        })?;

        Ok(VideoGenerationHandle {
            provider_job_id: video_id.to_string(),
            provider: "pixverse".to_string(),
            model: model.id.clone(),
        })
    }

    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/openapi/v2/video/result/{}", self.api_base(), handle.provider_job_id);
        let builder = self.auth_builder(&creds, self.client.get(&url))?;
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

        // Resp.status: 1=success, 5=processing, 7=moderation fail, 8=failed.
        let (status, video_url) = match data["Resp"]["status"].as_i64() {
            Some(1) => (GenerationStatus::Completed, data["Resp"]["url"].as_str().map(String::from)),
            Some(7) | Some(8) => (GenerationStatus::Failed, None),
            _ => (GenerationStatus::Processing, None),
        };

        Ok(VideoGenerationPollResult {
            status,
            progress: if status == GenerationStatus::Completed { 100 } else { 50 },
            video_url,
            video_data: None,
            content_type: Some("video/mp4".into()),
            error: if status == GenerationStatus::Failed {
                Some(data["ErrMsg"].as_str().unwrap_or("failed").to_string())
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
                message: "Pixverse provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Pixverse provider configured".into(), latency_ms: None }
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

    fn make_provider(api_base: &str) -> PixverseProvider {
        let mut p = PixverseProvider::new();
        let mut cfg = ProviderInstanceConfig { api_key: "pv-key".to_string(), api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.api_key = Some("pv-key".to_string());
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
        VideoExtras { duration_seconds: 5.0, aspect_ratio: Some("16:9".to_string()), resolution: Some("720p".to_string()), fps: None, extra: None }
    }

    #[tokio::test]
    async fn submits_text_to_video_and_polls() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/openapi/v2/video/text/generate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ErrCode": 0, "ErrMsg": "success", "Resp": { "video_id": 12345 }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/openapi/v2/video/result/12345"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ErrCode": 0, "Resp": { "status": 1, "url": "https://cdn.pixverse/v.mp4" }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("pixverse/v4.5");
        let base = make_base("a paper airplane gliding", "pixverse/v4.5");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "12345");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("api-key").unwrap(), "pv-key");
        assert!(post.headers.get("ai-trace-id").is_some(), "Ai-trace-id header must be present");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["model"], "v4.5");
        assert_eq!(body["quality"], "720p");

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_url.unwrap(), "https://cdn.pixverse/v.mp4");
    }
}
