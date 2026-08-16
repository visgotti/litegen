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

/// Runway Gen-3 video generation provider.
///
/// Submits text-to-video / image-to-video tasks to the Runway API
/// (`api.dev.runwayml.com`) with the required `X-Runway-Version: 2024-11-06` header.
///
/// @see <https://docs.dev.runwayml.com/api/> — Runway API reference (Start generating + Task management)
/// @see <https://docs.dev.runwayml.com/guides/using-the-api/> — API getting-started guide
pub struct RunwayProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
}

impl RunwayProvider {
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
            .unwrap_or("https://api.dev.runwayml.com/v1")
    }

    fn api_key(&self) -> Result<String, ProviderError> {
        if let Some(pool) = &self.key_pool {
            return Ok(pool.next().to_string());
        }
        self.config
            .as_ref()
            .map(|c| c.api_key.clone())
            .ok_or_else(|| ProviderError::NotConfigured("runway".into()))
    }

    /// Map internal model ID to Runway API model name.
    ///
    /// `gen3a_turbo` and `gen4_aleph` were removed from the API on 2026-07-30
    /// ("Requests that use these model identifiers will fail") and are not in
    /// the `/v1/image_to_video` model union. `gen4_turbo` is the fast tier,
    /// `gen4.5` the quality tier.
    /// @see <https://docs.dev.runwayml.com/api-details/api_changelog/>
    fn resolve_model(model: &str) -> &'static str {
        match model {
            "runway/gen4.5" | "runway-gen4.5" | "gen4.5" => "gen4.5",
            "runway/gen4-turbo" | "runway-gen4-turbo" | "gen4_turbo" => "gen4_turbo",
            _ => "gen4_turbo",
        }
    }
}

impl Default for RunwayProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for RunwayProvider {
    fn name(&self) -> &str {
        "runway"
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

    /// Submit a task: `POST {api_base}/text_to_video` (or `/image_to_video`
    /// when an init image is supplied), with `X-Runway-Version: 2024-11-06`.
    ///
    /// @see <https://docs.dev.runwayml.com/api/> — Start generating. Proves the `model`/`promptText`/
    ///   `promptImage`/`duration`/`ratio`/`seed` request fields, the version header, and the `id` response.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let api_key = self.api_key()?;
        let model_name = Self::resolve_model(&model.id);

        // Find init-role image ref (URL form)
        let prompt_image: Option<String> = materialized.refs.iter().find_map(|r| {
            if r.role == "init" {
                match &r.form {
                    MaterializedRefForm::Url(u) => Some(u.clone()),
                    MaterializedRefForm::Base64(b64) => {
                        Some(format!("data:image/png;base64,{b64}"))
                    }
                    _ => None,
                }
            } else {
                None
            }
        });

        // Choose endpoint: image_to_video if image provided, else text_to_video
        let endpoint = if prompt_image.is_some() {
            "image_to_video"
        } else {
            "text_to_video"
        };

        let url = format!("{}/{}", self.api_base(), endpoint);

        let duration = extras.duration_seconds as u64;

        let mut body = json!({
            "model": model_name,
            "promptText": base.prompt,
            "duration": duration,
        });

        if let Some(img_url) = prompt_image {
            body["promptImage"] = Value::String(img_url);
        }

        // Runway video requires `ratio` as a pixel-pair string. The gen4 enum is
        // 1280:720 | 720:1280 | 1104:832 | 832:1104 | 960:960 | 1584:672 — the
        // gen3-era 1280:768 / 768:1280 pair is no longer accepted. The unified
        // aspect_ratio (validated against models/runway.yaml, which lists exactly
        // the enum above) is forwarded verbatim; default to landscape 720p when
        // none was supplied so the required field is always present.
        let ratio = extras.aspect_ratio.as_deref().unwrap_or("1280:720");
        body["ratio"] = Value::String(ratio.to_string());

        if let Some(seed) = base.seed {
            body["seed"] = json!(seed);
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
                .header("Authorization", format!("Bearer {api_key}"))
                .header("X-Runway-Version", "2024-11-06")
                .header("Content-Type", "application/json")
                .json(&body),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("Runway request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_timeout() || e.is_connect(),
        })?;

        let status = resp.status();
        let resp_json: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Runway response: {e}"),
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
                message: format!("Runway API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(resp_json),
                retryable: status.as_u16() >= 500,
            });
        }

        let job_id = resp_json["id"]
            .as_str()
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "Runway response missing id".to_string(),
                status_code: None,
                provider_error: Some(resp_json.clone()),
                retryable: false,
            })?
            .to_string();

        Ok(VideoGenerationHandle {
            provider_job_id: job_id,
            provider: "runway".to_string(),
            model: model.id.clone(),
        })
    }

    /// Poll a task (`GET {api_base}/tasks/{id}`), reading `status`
    /// (`SUCCEEDED`/`FAILED`/`RUNNING`/`THROTTLED`) and the `output[]` URLs.
    ///
    /// @see <https://docs.dev.runwayml.com/api/> — Task management (retrieve a task)
    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let api_key = self.api_key()?;

        let resp = crate::providers::inject_trace_headers(
            self.client
                .get(format!("{}/tasks/{}", self.api_base(), handle.provider_job_id))
                .header("Authorization", format!("Bearer {api_key}"))
                .header("X-Runway-Version", "2024-11-06"),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: e.to_string(),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;

        let data: Value = resp.json().await.map_err(|e| {
            ProviderError::RequestFailed {
                message: format!("Failed to parse poll response: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            }
        })?;

        // The task response union's status constants are
        // PENDING | THROTTLED | CANCELLED | RUNNING | FAILED | SUCCEEDED.
        // CANCELLED is terminal — treating it as Pending polls until timeout.
        let status = match data["status"].as_str() {
            Some("SUCCEEDED") => GenerationStatus::Completed,
            Some("FAILED") | Some("CANCELLED") => GenerationStatus::Failed,
            Some("RUNNING") | Some("THROTTLED") => GenerationStatus::Processing,
            _ => GenerationStatus::Pending,
        };

        let video_url = data["output"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .map(String::from);

        let error = data["failure"]
            .as_str()
            .or_else(|| data["failureCode"].as_str())
            .map(String::from);

        let progress = match status {
            GenerationStatus::Completed => 100,
            GenerationStatus::Processing => data["progress"].as_u64().unwrap_or(50) as u8,
            _ => 0,
        };

        Ok(VideoGenerationPollResult {
            status,
            progress,
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

    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Runway provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult {
            healthy: true,
            message: "Runway provider configured".into(),
            latency_ms: None,
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

    fn make_provider(api_base: &str) -> RunwayProvider {
        let mut p = RunwayProvider::new();
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
    async fn submits_runway_job_and_returns_handle() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/text_to_video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "runway_job_1",
                "status": "RUNNING"
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4-turbo");
        let base = make_base("a drone shot over a misty forest", "runway/gen4-turbo");
        let extras = make_extras();
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let handle = result.unwrap();

        assert_eq!(handle.provider_job_id, "runway_job_1");
        assert_eq!(handle.provider, "runway");

        // Verify outbound request body
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "gen4_turbo");
        assert_eq!(body["promptText"], "a drone shot over a misty forest");
        assert_eq!(body["duration"], 5);

        // Verify Runway version header sent
        let req = &received[0];
        let version_header = req.headers.get("x-runway-version");
        assert!(version_header.is_some(), "X-Runway-Version header missing");
        assert_eq!(version_header.unwrap(), "2024-11-06");
    }

    /// `gen3a_turbo` was removed from the Runway API on 2026-07-30 ("Requests
    /// that use these model identifiers will fail"), and does not appear in the
    /// `/v1/image_to_video` model union in docs.dev.runwayml.com/openapi.json.
    /// The current fast model is `gen4_turbo`.
    #[tokio::test]
    async fn sends_gen4_turbo_not_the_retired_gen3a_turbo() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/text_to_video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1" })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4-turbo");
        let base = make_base("a drone shot over a misty forest", "runway/gen4-turbo");
        provider
            .generate(&schema, &base, &make_extras(), &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "gen4_turbo");
    }

    /// The quality tier is `gen4.5` (dot, not underscore) per the spec's
    /// discriminated-union variant title.
    #[tokio::test]
    async fn sends_gen4_5_model_id_verbatim() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/text_to_video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1" })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4.5");
        let base = make_base("a slow push through a canyon", "runway/gen4.5");
        provider
            .generate(&schema, &base, &make_extras(), &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "gen4.5");
    }

    /// `1280:768` / `768:1280` are gen3-era ratios. The gen4 `ratio` enum is
    /// `1280:720 | 720:1280 | 1104:832 | 832:1104 | 960:960 | 1584:672`, so the
    /// default we substitute when the caller omits one must be a member of it.
    #[tokio::test]
    async fn default_ratio_is_a_member_of_the_gen4_enum() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/text_to_video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1" })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4-turbo");
        let base = make_base("a city at night", "runway/gen4-turbo");
        provider
            .generate(&schema, &base, &make_extras(), &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        const GEN4_RATIOS: [&str; 6] = [
            "1280:720", "720:1280", "1104:832", "832:1104", "960:960", "1584:672",
        ];
        let ratio = body["ratio"].as_str().expect("ratio must always be sent");
        assert!(
            GEN4_RATIOS.contains(&ratio),
            "ratio {ratio:?} is not in the gen4 enum {GEN4_RATIOS:?}"
        );
    }

    /// Every catalog ratio must also be a member of the vendor enum — the
    /// validator accepts anything in `allowed` and forwards it verbatim.
    #[test]
    fn catalog_video_ratios_are_all_in_the_gen4_enum() {
        const GEN4_RATIOS: [&str; 6] = [
            "1280:720", "720:1280", "1104:832", "832:1104", "960:960", "1584:672",
        ];
        for id in ["runway/gen4-turbo", "runway/gen4.5"] {
            let schema = ref_schema(id);
            let allowed: Vec<String> = match schema.params.get("aspect_ratio") {
                Some(crate::capabilities::schema::ParamSpec::AspectRatio(ar)) => ar.allowed.clone(),
                other => panic!("{id} aspect_ratio param is {other:?}, expected AspectRatio"),
            };
            assert!(!allowed.is_empty(), "{id} declares no aspect_ratio.allowed");
            for ar in allowed {
                assert!(
                    GEN4_RATIOS.contains(&ar.as_str()),
                    "{id} allows ratio {ar:?}, which Runway's gen4 enum rejects"
                );
            }
        }
    }

    /// `CANCELLED` is one of the six task states in the spec's response union.
    /// Falling through to `Pending` makes a cancelled job poll until timeout.
    #[tokio::test]
    async fn cancelled_task_reports_failed_rather_than_pending() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tasks/task-cancelled"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "task-cancelled",
                "status": "CANCELLED"
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let handle = VideoGenerationHandle {
            provider_job_id: "task-cancelled".to_string(),
            provider: "runway".to_string(),
            model: "runway/gen4-turbo".to_string(),
        };

        let poll = provider.poll_status(&handle).await.expect("poll");
        assert_eq!(poll.status, GenerationStatus::Failed);
    }
}
