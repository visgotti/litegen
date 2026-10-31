use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::MaterializedRequest;
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig,
    CancelOutcome, VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// Runway video generation provider: gen4.5 (text- and image-to-video) and
/// gen4_turbo (image-to-video).
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

/// What litegen itself sends to `/text_to_video` and `/image_to_video`;
/// `extra` cannot replace it.
const OWNED_FIELDS: &[&str] = &["model", "promptText", "promptImage", "duration", "ratio", "seed"];

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
    fn resolve_model(model: &str) -> Option<&'static str> {
        match model {
            "runway/gen4.5" | "runway-gen4.5" | "gen4.5" => Some("gen4.5"),
            "runway/gen4-turbo" | "runway-gen4-turbo" | "gen4_turbo" => Some("gen4_turbo"),
            // An id with no arm is refused, not billed as gen4_turbo (L13).
            _ => None,
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
        let model_name = Self::resolve_model(&model.id).ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "runway".into() })?;

        // The init-role image, as a URL or a data URI of its own type; one
        // Runway cannot take is refused here.
        let prompt_image: Option<String> = match materialized.refs.iter().find(|r| r.role == "init") {
            Some(r) => crate::providers::image::runway::runway_uri(&r.form)?,
            None => None,
        };

        // gen4_turbo is only in the `/v1/image_to_video` model union — Runway's
        // `/v1/text_to_video` has no gen4_turbo branch — so a prompt-only
        // request can only be rejected. Fail here, before the round trip.
        // @see <https://docs.dev.runwayml.com/openapi.json>
        if prompt_image.is_none() && model_name == "gen4_turbo" {
            return Err(ProviderError::InvalidRequest(
                "runway gen4_turbo is image-to-video only: supply an init image, \
                 or use runway/gen4.5 for text-to-video"
                    .to_string(),
            ));
        }

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
            "duration": duration,
        });
        // promptText is "A non-empty string", and optional for gen4_turbo.
        if !base.prompt.is_empty() {
            body["promptText"] = Value::String(base.prompt.clone());
        }

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

        // Shallow-merge extra, never over what litegen sends.
        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(body_obj) = body.as_object_mut() {
                for (k, v) in extra_map.iter().filter(|(k, _)| !OWNED_FIELDS.contains(&k.as_str())) {
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
            // Only a connection that never opened is safe to retry: a request
            // that timed out may have been accepted and billed.
            retryable: e.is_connect(),
        })?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let resp_json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let err = resp_json["message"]
                .as_str()
                .or_else(|| resp_json["error"].as_str())
                .map(str::to_string)
                .unwrap_or_else(|| text.chars().take(200).collect());
            return Err(ProviderError::RequestFailed {
                message: format!("Runway API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(resp_json),
                // Turned away, not run: "429 | If the client makes too many requests … | Yes".
                // @see <https://docs.dev.runwayml.com/errors/errors.md>
                retryable: status.as_u16() == 429 || status.is_server_error(),
            });
        }
        let resp_json: Value = serde_json::from_str(&text).map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Runway response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

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
        // A 404 is "the referenced resource is not available", not a task
        // still running; a 5xx or 429 is asked again.
        // @see <https://docs.dev.runwayml.com/errors/errors.md>
        let data = super::read_poll_json(resp, "runway").await?;

        // The task response union's status constants are
        // PENDING | THROTTLED | CANCELLED | RUNNING | FAILED | SUCCEEDED.
        // CANCELLED is terminal — treating it as Pending polls until timeout.
        let mut status = match data["status"].as_str() {
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

        let mut error = data["failure"]
            .as_str()
            .or_else(|| data["failureCode"].as_str())
            .map(String::from);
        // A success with nothing to deliver is a failure, not an empty result.
        if status == GenerationStatus::Completed && video_url.is_none() {
            status = GenerationStatus::Failed;
            error = Some("runway task succeeded with no output".into());
        }

        // RUNNING `progress` is a fraction: "minimum": 0, "maximum": 1.
        let progress = match status {
            GenerationStatus::Completed => 100,
            GenerationStatus::Processing => (data["progress"].as_f64().unwrap_or(0.5) * 100.0).round().clamp(1.0, 99.0) as u8,
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

    /// Cancel a task that has not finished (`DELETE {api_base}/tasks/{id}`).
    /// The same call DELETES a finished task — refunding nothing and dropping
    /// its output — so the task's status is read first and a finished one is
    /// left alone.
    ///
    /// @see <https://docs.dev.runwayml.com/openapi.json> — `DELETE /v1/tasks/{id}`, "Cancel or delete
    /// a task": "Tasks that are running, pending, or throttled can be canceled by invoking this
    /// method. Invoking this method for other tasks will delete them." 204 on success; 404 for a
    /// task already deleted or aborted.
    async fn cancel(&self, handle: &VideoGenerationHandle) -> Result<CancelOutcome, ProviderError> {
        let api_key = self.api_key()?;
        let url = format!("{}/tasks/{}", self.api_base(), handle.provider_job_id);
        let failed = |e: reqwest::Error| ProviderError::RequestFailed {
            message: format!("runway cancel failed: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        };
        let task = crate::providers::inject_trace_headers(
            self.client
                .get(&url)
                .header("Authorization", format!("Bearer {api_key}"))
                .header("X-Runway-Version", "2024-11-06"),
        )
        .send()
        .await
        .map_err(failed)?;
        if task.status().as_u16() == 404 {
            return Ok(CancelOutcome::NotFound);
        }
        let task: Value = task.json().await.unwrap_or(Value::Null);
        if matches!(task["status"].as_str(), Some("SUCCEEDED" | "FAILED" | "CANCELLED")) {
            return Ok(CancelOutcome::AlreadyFinished);
        }
        let resp = crate::providers::inject_trace_headers(
            self.client
                .delete(&url)
                .header("Authorization", format!("Bearer {api_key}"))
                .header("X-Runway-Version", "2024-11-06"),
        )
        .send()
        .await
        .map_err(failed)?;
        match resp.status().as_u16() {
            200..=299 => Ok(CancelOutcome::Requested),
            404 => Ok(CancelOutcome::NotFound),
            code => Err(ProviderError::RequestFailed {
                message: format!("runway cancel answered {code}"),
                status_code: Some(code),
                provider_error: resp.json().await.ok(),
                retryable: code >= 500,
            }),
        }
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
    use crate::proxy::materializer::MaterializedRefForm;
    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::{method, path};

    /// L13: every catalog row has its own arm, and an id with none is refused
    /// rather than billed as another model.
    #[test]
    fn every_catalog_row_resolves_and_an_unknown_id_does_not() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        for m in catalog.for_provider("runway").filter(|m| m.media_type == crate::capabilities::MediaType::Video) {
            assert!(RunwayProvider::resolve_model(&m.id).is_some(), "{} has no arm", m.id);
        }
        assert!(RunwayProvider::resolve_model("runway/not-a-model").is_none());
    }

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
            camera: None, audio: None, r#loop: None,
            source: None, direction: None, time_range: None, replace: None,
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

        // Text-to-video is gen4.5's job: gen4_turbo is not in /v1/text_to_video.
        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4.5");
        let base = make_base("a drone shot over a misty forest", "runway/gen4.5");
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
        assert_eq!(body["model"], "gen4.5");
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
    /// The current fast model is `gen4_turbo`, which is image-to-video only.
    #[tokio::test]
    async fn sends_gen4_turbo_not_the_retired_gen3a_turbo() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/image_to_video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1" })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4-turbo");
        let base = make_base("a drone shot over a misty forest", "runway/gen4-turbo");
        let materialized = crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: MaterializedRefForm::Url("https://example.test/first.png".to_string()),
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };
        provider
            .generate(&schema, &base, &make_extras(), &materialized)
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "gen4_turbo");
        assert_eq!(body["promptImage"], "https://example.test/first.png");
    }

    /// `/v1/text_to_video` has no `gen4_turbo` branch in
    /// docs.dev.runwayml.com/openapi.json (re-read 2026-09-11), so a
    /// prompt-only gen4_turbo request must fail here rather than be sent to an
    /// endpoint that can only reject it.
    #[tokio::test]
    async fn gen4_turbo_without_an_init_image_is_rejected_before_any_request() {
        let server = MockServer::start().await;
        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4-turbo");
        let base = make_base("a city at night", "runway/gen4-turbo");
        let err = provider
            .generate(&schema, &base, &make_extras(), &empty_materialized())
            .await
            .err()
            .expect("gen4_turbo cannot do text-to-video");
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "got {err:?}");
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "no request may reach Runway"
        );
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

    /// `1280:768` / `768:1280` are gen3-era ratios. `/v1/text_to_video` takes
    /// only `1280:720 | 720:1280` for `gen4.5` (the six-value gen4 enum is
    /// `/v1/image_to_video`'s), so the default we substitute when the caller
    /// omits one must be a member of that, the narrowest enum.
    #[tokio::test]
    async fn default_ratio_is_a_member_of_the_gen4_enum() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/text_to_video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1" })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4.5");
        let base = make_base("a city at night", "runway/gen4.5");
        provider
            .generate(&schema, &base, &make_extras(), &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        const T2V_RATIOS: [&str; 2] = ["1280:720", "720:1280"];
        let ratio = body["ratio"].as_str().expect("ratio must always be sent");
        assert!(
            T2V_RATIOS.contains(&ratio),
            "ratio {ratio:?} is not in the gen4.5 text_to_video enum {T2V_RATIOS:?}"
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

    // ─── Cancel (L18) ───────────────────────────────────────────────────────

    fn task(id: &str) -> VideoGenerationHandle {
        VideoGenerationHandle { provider_job_id: id.to_string(), provider: "runway".into(), model: "runway/gen4-turbo".into() }
    }

    #[tokio::test]
    async fn cancel_deletes_a_task_that_has_not_finished() {
        use crate::providers::CancelOutcome;
        use wiremock::matchers::header;
        for state in ["PENDING", "THROTTLED", "RUNNING"] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/tasks/t1"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1", "status": state })))
                .mount(&server)
                .await;
            Mock::given(method("DELETE"))
                .and(path("/tasks/t1"))
                .and(header("authorization", "Bearer test-key"))
                .and(header("x-runway-version", "2024-11-06"))
                .respond_with(ResponseTemplate::new(204))
                .expect(1)
                .mount(&server)
                .await;
            assert_eq!(make_provider(&server.uri()).cancel(&task("t1")).await.unwrap(), CancelOutcome::Requested, "{state}");
        }
    }

    /// For a finished task the same DELETE deletes it, which refunds nothing
    /// and throws its output away: a finished task is left alone.
    #[tokio::test]
    async fn cancel_leaves_a_finished_task_alone_and_reports_one_that_is_gone() {
        use crate::providers::CancelOutcome;
        for state in ["SUCCEEDED", "FAILED", "CANCELLED"] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/tasks/t1"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1", "status": state })))
                .mount(&server)
                .await;
            Mock::given(method("DELETE")).respond_with(ResponseTemplate::new(204)).expect(0).mount(&server).await;
            assert_eq!(make_provider(&server.uri()).cancel(&task("t1")).await.unwrap(), CancelOutcome::AlreadyFinished, "{state}");
        }
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/tasks/t2")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        assert_eq!(make_provider(&server.uri()).cancel(&task("t2")).await.unwrap(), CancelOutcome::NotFound);
    }

    // ─── Runway audit, 2026-10-30 ───────────────────────────────────────────

    /// Runway answers every submit with a 400, so only the outbound body matters.
    async fn refusing_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": "stop" })))
            .mount(&server)
            .await;
        server
    }

    async fn sent_bodies(server: &MockServer) -> Vec<Value> {
        server.received_requests().await.unwrap().iter().map(|r| serde_json::from_slice(&r.body).unwrap()).collect()
    }

    fn with_init(form: MaterializedRefForm) -> MaterializedRequest {
        MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef { role: "init".into(), form, time_s: None }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    /// #16: `extra` adds Runway's own knobs but never replaces what litegen sends.
    #[tokio::test]
    async fn extra_cannot_replace_what_litegen_sends() {
        let server = refusing_server().await;
        let mut base = make_base("a slow pan", "runway/gen4.5");
        base.seed = Some(3);
        let mut extras = make_extras();
        extras.extra = Some(json!({
            "model": "gen4_turbo", "promptText": "other", "promptImage": "https://x/y.png",
            "duration": 10, "ratio": "960:960", "seed": 9, "contentModeration": { "publicFigureThreshold": "low" }
        }));
        let _ = make_provider(&server.uri()).generate(&ref_schema("runway/gen4.5"), &base, &extras, &empty_materialized()).await;
        let body = &sent_bodies(&server).await[0];
        assert_eq!(body["model"], "gen4.5");
        assert_eq!(body["promptText"], "a slow pan");
        assert!(body.get("promptImage").is_none(), "{body}");
        assert_eq!(body["duration"], 5);
        assert_eq!(body["ratio"], "1280:720");
        assert_eq!(body["seed"], 3);
        assert_eq!(body["contentModeration"]["publicFigureThreshold"], "low");
    }

    /// #29: gen4_turbo's `promptText` is optional (`required: ["promptImage",
    /// "ratio","model"]`) and "A non-empty string", so no prompt sends none.
    /// @see <https://docs.dev.runwayml.com/openapi.json> (POST /v1/image_to_video, 2026-10-30)
    #[tokio::test]
    async fn gen4_turbo_without_a_prompt_sends_no_prompt_text() {
        let server = refusing_server().await;
        let refs = with_init(MaterializedRefForm::Url("https://example.com/first.png".into()));
        let _ = make_provider(&server.uri())
            .generate(&ref_schema("runway/gen4-turbo"), &make_base("", "runway/gen4-turbo"), &make_extras(), &refs)
            .await;
        let body = &sent_bodies(&server).await[0];
        assert!(body.get("promptText").is_none(), "{body}");
        assert_eq!(body["promptImage"], "https://example.com/first.png");
    }

    /// #17, #18: a submit that timed out may have been accepted and billed, so
    /// it is never sent again; a 429 was turned away and may be.
    /// @see <https://docs.dev.runwayml.com/errors/errors.md> (2026-10-30)
    #[tokio::test]
    async fn a_timed_out_submit_is_never_sent_again_and_a_rate_limited_one_may_be() {
        let slow = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t" })).set_delay(std::time::Duration::from_secs(5)))
            .mount(&slow)
            .await;
        let mut p = make_provider(&slow.uri());
        p.client = reqwest::Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        let schema = ref_schema("runway/gen4.5");
        let err = p.generate(&schema, &make_base("x", "runway/gen4.5"), &make_extras(), &empty_materialized()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");

        let limited = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({ "error": "Too many requests" })))
            .mount(&limited)
            .await;
        let err = make_provider(&limited.uri())
            .generate(&schema, &make_base("x", "runway/gen4.5"), &make_extras(), &empty_materialized())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: true, status_code: Some(429), .. }), "{err:?}");
    }

    /// #21: a base64 first frame goes with its own type, and a GIF ("GIF images
    /// are not supported") is refused before any call.
    /// @see <https://docs.dev.runwayml.com/assets/inputs.md> (2026-10-30)
    #[tokio::test]
    async fn a_frame_goes_with_its_own_type_and_one_runway_cannot_take_is_refused() {
        use base64::Engine;
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let server = refusing_server().await;
        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4-turbo");
        let base = make_base("x", "runway/gen4-turbo");
        let webp = with_init(MaterializedRefForm::Base64(b64(b"RIFF\x24\x00\x00\x00WEBPVP8 ")));
        let _ = provider.generate(&schema, &base, &make_extras(), &webp).await;
        let sent = sent_bodies(&server).await;
        assert!(sent[0]["promptImage"].as_str().unwrap().starts_with("data:image/webp;base64,"), "{}", sent[0]);

        let gif = with_init(MaterializedRefForm::Base64(b64(b"GIF89a\x01\x00\x01\x00")));
        let err = provider.generate(&schema, &base, &make_extras(), &gif).await.unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        assert_eq!(server.received_requests().await.unwrap().len(), 1, "a GIF reached Runway");
    }

    /// #13, #20, #25: a poll Runway answers with an error is that error; a
    /// running task's `progress` is a fraction ("minimum": 0, "maximum": 1);
    /// a success without an output fails rather than completing with nothing.
    /// @see <https://docs.dev.runwayml.com/openapi.json> (GET /v1/tasks/{id}, 2026-10-30)
    #[tokio::test]
    async fn a_poll_says_what_runway_said() {
        let server = MockServer::start().await;
        for (id, status, body) in [
            ("gone", 404, json!({ "error": "Task not found" })),
            ("running", 200, json!({ "id": "running", "status": "RUNNING", "progress": 0.42 })),
            ("empty", 200, json!({ "id": "empty", "status": "SUCCEEDED", "output": [] })),
        ] {
            Mock::given(method("GET"))
                .and(path(format!("/tasks/{id}")))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .mount(&server)
                .await;
        }
        let provider = make_provider(&server.uri());
        let handle = |id: &str| VideoGenerationHandle { provider_job_id: id.into(), provider: "runway".into(), model: "runway/gen4.5".into() };

        let err = provider.poll_status(&handle("gone")).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { status_code: Some(404), retryable: false, .. }), "{err:?}");

        let running = provider.poll_status(&handle("running")).await.unwrap();
        assert_eq!((running.status, running.progress), (GenerationStatus::Processing, 42));

        let empty = provider.poll_status(&handle("empty")).await.unwrap();
        assert_eq!(empty.status, GenerationStatus::Failed);
        assert!(empty.error.as_deref().is_some_and(|e| e.contains("no output")), "{:?}", empty.error);
    }
}
