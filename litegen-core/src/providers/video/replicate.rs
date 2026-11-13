// Replicate video generation provider
// API Reference: https://replicate.com/docs/reference/http
// POST /v1/predictions — create prediction (with version hash)
// GET  /v1/predictions/{id} — poll for status
// Models: AnimateDiff (replicate/video)

use crate::providers::auth::Authorize;
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

/// Replicate video generation provider.
///
/// Runs AnimateDiff (`lucataco/animate-diff`), the model behind
/// `replicate/video`, through the Replicate predictions API.
///
/// @see <https://replicate.com/docs/reference/http#predictions.create> — create (`POST /v1/predictions`)
/// @see <https://replicate.com/docs/reference/http#predictions.get> — poll (`GET /v1/predictions/{id}`)
pub struct ReplicateVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
    /// How every request to the vendor's API is authenticated.
    auth: crate::providers::auth::AuthSpec,
}

/// What litegen itself sends in a prediction's `input`; `extra` cannot
/// replace it.
const OWNED_FIELDS: &[&str] = &["prompt", "seed", "n_prompt"];

/// The Replicate version a litegen video id runs; an id with no arm is
/// refused, not billed as AnimateDiff. (Stable Video Diffusion was removed
/// from Replicate, and Zeroscope and Deforum were never catalog rows.)
///
/// @see <https://replicate.com/docs/reference/http#predictions.create> — the `version` hash sent in the create body
/// @see <https://replicate.com/lucataco/animate-diff/versions> — `beecf59c…`, the latest version (2026-10-30)
fn resolve_model_version(model: &str) -> Option<&'static str> {
    match model {
        "replicate/video" | "replicate/animate-diff" => Some("beecf59c4aee8d81bf04f0381033dfa10dc16e845b4ae00d281e2fa377e48a9f"),
        _ => None,
    }
}

impl ReplicateVideoProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (result downloads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(600), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            client: Self::api_client(&Default::default()),
            auth: crate::providers::auth::AuthSpec::bearer(),
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.replicate.com/v1")
    }

    fn api_key(&self) -> Result<String, ProviderError> {
        if let Some(pool) = &self.key_pool {
            return Ok(pool.next().to_string());
        }
        self.config
            .as_ref()
            .map(|c| c.api_key.clone())
            .ok_or_else(|| ProviderError::NotConfigured("replicate".into()))
    }
}

impl Default for ReplicateVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for ReplicateVideoProvider {
    fn name(&self) -> &str {
        "replicate"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.api_keys.is_empty() {
            self.key_pool = Some(ApiKeyPool::shared(config.api_keys.clone()));
        }
        self.client = Self::api_client(&config.extra_headers);
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| !c.api_key.is_empty() || self.key_pool.is_some())
    }

    /// Create a prediction (`POST {api_base}/predictions`) with a `version` hash.
    ///
    /// @see <https://replicate.com/docs/reference/http#predictions.create> — create a prediction.
    ///   Proves the `{version, input}` body, the `input` fields (`prompt`, `n_prompt`,
    ///   `seed`, plus `steps`/`guidance_scale` via `extra`), and the `id` response
    ///   field. The backing AnimateDiff model does not accept `fps`/`num_frames`/
    ///   `negative_prompt` or an image, so those are intentionally not sent.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        // AnimateDiff has no image input, and the row declares no references.
        _materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let api_key = self.api_key()?;
        let pinned = resolve_model_version(&model.id)
            .ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "replicate".into() })?;
        // Honor an operator-supplied `version` override via model_mapping
        // (value form: "owner/name:version" or just a bare "version" hash).
        let version = self
            .config
            .as_ref()
            .and_then(|c| c.mapped_model(&model.id))
            .map(|mapped| {
                mapped
                    .rsplit_once(':')
                    .map(|(_, v)| v.to_string())
                    .unwrap_or_else(|| mapped.to_string())
            })
            .unwrap_or_else(|| pinned.to_string());
        let url = format!("{}/predictions", self.api_base());

        // Build input payload.
        //
        // The only catalog-exposed video id (`replicate/video`) resolves to
        // lucataco/animate-diff, whose Cog input schema is exactly
        // {motion_module, path, prompt, n_prompt, steps, guidance_scale, seed}.
        // It does NOT accept `fps`, `num_frames`, or `negative_prompt`; it uses
        // `n_prompt` for the negative prompt and hardcodes the frame count. So we
        // send only the widely-safe fields it accepts (prompt, n_prompt, seed) and
        // let `extras.extra` supply `steps`/`guidance_scale` via the shallow-merge
        // below. `fps`/`num_frames`/`negative_prompt` are intentionally NOT sent —
        // Replicate rejects unknown input keys (or silently drops them), so emitting
        // them either 422s the request or makes duration/fps a no-op.
        let mut input = json!({
            "prompt": base.prompt,
        });

        if let Some(seed) = base.seed {
            input["seed"] = json!(seed);
        }

        if let Some(np) = base.negative_prompt.as_deref() {
            input["n_prompt"] = Value::String(np.to_string());
        }

        // Shallow-merge extra, never over what litegen sends.
        if let Some(input_obj) = input.as_object_mut() {
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED_FIELDS.contains(&k)) {
                input_obj.insert(k.clone(), v.clone());
            }
        }

        let body = json!({
            "version": version,
            "input": input,
        });

        let resp = crate::providers::inject_trace_headers(
            self.client
                .post(&url)
                .authorized(&self.auth, &api_key)?
                .header("Content-Type", "application/json")
                .json(&body),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("Replicate request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            // Only a connection that never opened is safe to retry: a request
            // that timed out may have created, and billed, a prediction.
            retryable: e.is_connect(),
        })?;

        let status = resp.status();
        let retry_after = crate::providers::retry_after_ms(resp.headers());
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let resp_json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let err = resp_json["detail"]
                .as_str()
                .or_else(|| resp_json["error"].as_str())
                .map(str::to_string)
                .unwrap_or_else(|| text.chars().take(200).collect());
            if let Some(e) = crate::providers::rate_limited("replicate", status, retry_after, &format!("Replicate API error: {err}"), Some(&resp_json)) {
                return Err(e);
            }
            return Err(ProviderError::RequestFailed {
                message: format!("Replicate API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(resp_json),
                // Turned away, not run: "If you hit a limit, you will receive a
                // response with status `429`".
                // @see <https://replicate.com/docs/topics/predictions/rate-limits>
                retryable: status.as_u16() == 429 || status.is_server_error(),
            });
        }
        let resp_json: Value = serde_json::from_str(&text).map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Replicate response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        let prediction_id = resp_json["id"]
            .as_str()
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "Replicate response missing id".to_string(),
                status_code: None,
                provider_error: Some(resp_json.clone()),
                retryable: false,
            })?
            .to_string();

        // The prediction is polled at the configured base by its id, never at
        // the `urls.get` the response named: that would carry the key to
        // whatever host the response chose.
        let job_data = json!({ "prediction_id": prediction_id });

        Ok(VideoGenerationHandle {
            provider_job_id: job_data.to_string(),
            provider: "replicate".to_string(),
            model: model.id.clone(),
        })
    }

    /// Poll a prediction (`GET {api_base}/predictions/{id}`), reading `status`
    /// (`succeeded`/`failed`/`processing`/`starting`) and the `output` URL(s).
    ///
    /// @see <https://replicate.com/docs/reference/http#predictions.get> — get a prediction
    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let api_key = self.api_key()?;

        // Rows stored before 2026-10-30 also carry a `poll_url`; it is ignored.
        let prediction_id = serde_json::from_str::<Value>(&handle.provider_job_id)
            .ok()
            .and_then(|job| job["prediction_id"].as_str().filter(|id| !id.is_empty()).map(str::to_string))
            .ok_or_else(|| ProviderError::RequestFailed {
                message: format!("replicate job handle names no prediction: {}", handle.provider_job_id),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;
        let poll_url = format!("{}/predictions/{prediction_id}", self.api_base());

        // GET /v1/predictions/{id} — https://replicate.com/docs/reference/http#predictions.get
        let resp = crate::providers::inject_trace_headers(
            self.client
                .get(&poll_url)
                .authorized(&self.auth, &api_key)?,
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: e.to_string(),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;

        let data = super::read_poll_json(resp, "replicate").await?;

        // starting | processing | succeeded | failed | canceled | aborted.
        // @see <https://api.replicate.com/openapi.json> (2026-10-30)
        let mut error = data["error"].as_str().filter(|e| !e.is_empty()).map(String::from);
        let mut status = match data["status"].as_str() {
            Some("succeeded") => GenerationStatus::Completed,
            Some("failed") | Some("canceled") => GenerationStatus::Failed,
            Some("processing") => GenerationStatus::Processing,
            Some("starting") => GenerationStatus::Pending,
            // "`aborted` means the prediction was terminated before it started running".
            Some("aborted") => {
                error.get_or_insert_with(|| "replicate prediction aborted before it started".into());
                GenerationStatus::Failed
            }
            other => return Ok(super::unknown_state("replicate", other)),
        };

        // Video URL from output (string or array of strings)
        let video_url = data["output"]
            .as_str()
            .map(String::from)
            .or_else(|| {
                data["output"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });

        // A success with nothing to deliver is a failure, not an empty result.
        if status == GenerationStatus::Completed && video_url.is_none() {
            status = GenerationStatus::Failed;
            error = Some("replicate prediction succeeded with no output".into());
        }

        let mut metadata = HashMap::new();
        if let Some(predict_time) = data["metrics"]["predict_time"].as_f64() {
            metadata.insert("predict_time".into(), json!(predict_time));
        }

        Ok(VideoGenerationPollResult {
            status,
            progress: match status {
                GenerationStatus::Completed => 100,
                GenerationStatus::Processing => 50,
                _ => 10,
            },
            video_url,
            video_data: None,
            content_type: Some("video/mp4".into()),
            error,
            metadata,
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

    /// Validate the API token via `GET {api_base}/account`.
    ///
    /// @see <https://replicate.com/docs/reference/http#account.get> — Get the authenticated account
    /// Cancel a prediction (`POST {api_base}/predictions/{id}/cancel`). The
    /// reference documents only the call; what it answers is read as the
    /// prediction it returns: one that had already succeeded or failed had
    /// nothing left to stop.
    ///
    /// @see <https://replicate.com/docs/reference/http#predictions.cancel> — "Cancel a prediction
    /// that is currently running."
    async fn cancel(&self, handle: &VideoGenerationHandle) -> Result<CancelOutcome, ProviderError> {
        let api_key = self.api_key()?;
        let job: Value = serde_json::from_str(&handle.provider_job_id).unwrap_or_default();
        let prediction_id = job["prediction_id"].as_str().unwrap_or("");
        let resp = crate::providers::inject_trace_headers(
            self.client
                .post(format!("{}/predictions/{prediction_id}/cancel", self.api_base()))
                .authorized(&self.auth, &api_key)?,
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("replicate cancel failed: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        match status.as_u16() {
            200..=299 => Ok(match body["status"].as_str() {
                Some("succeeded") | Some("failed") => CancelOutcome::AlreadyFinished,
                _ => CancelOutcome::Requested,
            }),
            404 => Ok(CancelOutcome::NotFound),
            code => Err(ProviderError::RequestFailed {
                message: format!("replicate cancel answered {status}"),
                status_code: Some(code),
                provider_error: Some(body),
                retryable: status.is_server_error(),
            }),
        }
    }

    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Replicate Video provider not configured".into(),
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
        let request = match self.client.get(format!("{}/account", self.api_base())).authorized(&self.auth, &api_key) {
            Ok(request) => request,
            Err(e) => return HealthCheckResult { healthy: false, message: format!("Replicate health check failed: {e}"), latency_ms: None },
        };
        match request.timeout(std::time::Duration::from_secs(10)).send().await {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "Replicate API key valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("Replicate returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("Replicate health check failed: {e}"),
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

    fn make_provider(api_base: &str) -> ReplicateVideoProvider {
        let mut p = ReplicateVideoProvider::new();
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
            duration_seconds: 4.0,
            aspect_ratio: None,
            resolution: None,
            fps: None,
            extra: None,
            camera: None, audio: None, r#loop: None,
            source: None, direction: None, time_range: None, replace: None,
        }
    }

    #[tokio::test]
    async fn submits_replicate_prediction_and_returns_handle() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/predictions"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": "rep_video_1",
                "status": "starting",
                "urls": {
                    "get": "https://api.replicate.com/v1/predictions/rep_video_1"
                }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("replicate/video");
        let base = make_base("a time lapse of clouds", "replicate/video");
        let extras = make_extras();
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let handle = result.unwrap();

        // provider_job_id is JSON containing prediction_id
        let job_data: Value = serde_json::from_str(&handle.provider_job_id).unwrap();
        assert_eq!(job_data["prediction_id"], "rep_video_1");

        assert_eq!(handle.provider, "replicate");
        assert_eq!(handle.model, "replicate/video");

        // Verify outbound request body
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert!(body["version"].as_str().is_some(), "version hash missing");
        assert_eq!(body["input"]["prompt"], "a time lapse of clouds");
    }

    /// #15: every catalog row has its own arm, and an id with none — the
    /// removed Stable Video Diffusion among them — is refused rather than
    /// billed as AnimateDiff.
    #[test]
    fn every_catalog_row_resolves_and_an_unknown_id_does_not() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        for m in catalog.for_provider("replicate").filter(|m| m.media_type == crate::capabilities::MediaType::Video) {
            assert!(resolve_model_version(&m.id).is_some(), "{} has no arm", m.id);
        }
        for id in ["replicate/svd", "replicate/svd-xt", "replicate/zeroscope", "replicate/anything-else"] {
            assert!(resolve_model_version(id).is_none(), "{id} resolves");
        }
    }

    // ─── Cancel (L18) ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn cancel_posts_to_the_predictions_cancel_endpoint() {
        use crate::providers::CancelOutcome;
        use wiremock::matchers::header;
        let cases = [
            (200, serde_json::json!({ "id": "p1", "status": "canceled" }), CancelOutcome::Requested),
            (200, serde_json::json!({ "id": "p1", "status": "processing" }), CancelOutcome::Requested),
            (200, serde_json::json!({ "id": "p1", "status": "succeeded" }), CancelOutcome::AlreadyFinished),
            (404, serde_json::json!({ "detail": "Not found." }), CancelOutcome::NotFound),
        ];
        for (status, body, expected) in cases {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/predictions/p1/cancel"))
                .and(header("authorization", "Bearer test-key"))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
            let job = serde_json::json!({ "prediction_id": "p1", "poll_url": "" });
            let handle = VideoGenerationHandle { provider_job_id: job.to_string(), provider: "replicate".into(), model: "replicate/x".into() };
            assert_eq!(make_provider(&server.uri()).cancel(&handle).await.unwrap(), expected);
        }
    }

    // ─── Replicate audit, 2026-10-30 ────────────────────────────────────────

    /// #8: a prediction is polled at the configured `api_base` only, never at
    /// the `urls.get` the create answered with, which would carry the key to
    /// whatever host the response named.
    #[tokio::test]
    async fn a_prediction_is_polled_only_at_the_configured_base() {
        let elsewhere = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "p1", "status": "processing" })))
            .mount(&elsewhere)
            .await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/predictions"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": "p1", "status": "starting", "urls": { "get": format!("{}/predictions/p1", elsewhere.uri()) }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/predictions/p1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "p1", "status": "processing" })))
            .mount(&server)
            .await;
        let provider = make_provider(&server.uri());
        let handle = provider
            .generate(&ref_schema("replicate/video"), &make_base("clouds", "replicate/video"), &make_extras(), &empty_materialized())
            .await
            .unwrap();
        provider.poll_status(&handle).await.unwrap();
        assert!(elsewhere.received_requests().await.unwrap().is_empty(), "the key went to the host the response named");
        assert_eq!(server.received_requests().await.unwrap().iter().filter(|r| r.method.as_str() == "GET").count(), 1);

        let err = provider
            .poll_status(&VideoGenerationHandle { provider_job_id: "not json".into(), provider: "replicate".into(), model: "replicate/video".into() })
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
    }

    /// #7, #25: "`aborted` means the prediction was terminated before it
    /// started running"; a status outside the documented enum and a success
    /// with no output are failures, not predictions still running.
    /// @see <https://api.replicate.com/openapi.json> (2026-10-30)
    #[tokio::test]
    async fn an_aborted_unknown_or_empty_prediction_fails() {
        let server = MockServer::start().await;
        for (id, body) in [
            ("aborted", json!({ "id": "aborted", "status": "aborted" })),
            ("odd", json!({ "id": "odd", "status": "melted" })),
            ("empty", json!({ "id": "empty", "status": "succeeded", "output": null })),
        ] {
            Mock::given(method("GET"))
                .and(path(format!("/predictions/{id}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
        }
        let provider = make_provider(&server.uri());
        for (id, says) in [("aborted", "aborted"), ("odd", "melted"), ("empty", "no output")] {
            let handle = VideoGenerationHandle {
                provider_job_id: json!({ "prediction_id": id }).to_string(),
                provider: "replicate".into(),
                model: "replicate/video".into(),
            };
            let polled = provider.poll_status(&handle).await.unwrap();
            assert_eq!(polled.status, GenerationStatus::Failed, "{id}");
            assert!(polled.error.as_deref().is_some_and(|e| e.contains(says)), "{id}: {:?}", polled.error);
        }
    }

    /// #17, #18: a create that timed out may have been accepted and billed, so
    /// it is never sent again; a 429 may be.
    #[tokio::test]
    async fn a_timed_out_create_is_never_sent_again_and_a_throttled_one_may_be() {
        let slow = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "p" })).set_delay(std::time::Duration::from_secs(5)))
            .mount(&slow)
            .await;
        let mut p = make_provider(&slow.uri());
        p.client = reqwest::Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        let schema = ref_schema("replicate/video");
        let base = make_base("x", "replicate/video");
        let err = p.generate(&schema, &base, &make_extras(), &empty_materialized()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");

        let throttled = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({ "detail": "Request was throttled." })))
            .mount(&throttled)
            .await;
        let err = make_provider(&throttled.uri()).generate(&schema, &base, &make_extras(), &empty_materialized()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: true, status_code: Some(429), .. }), "{err:?}");
    }

    /// AnimateDiff's input is exactly {guidance_scale, motion_module,
    /// n_prompt, path, prompt, seed, steps}: `extra` adds its knobs but never
    /// replaces what litegen sends.
    /// @see <https://replicate.com/lucataco/animate-diff/versions/beecf59c4aee8d81bf04f0381033dfa10dc16e845b4ae00d281e2fa377e48a9f> (2026-10-30)
    #[tokio::test]
    async fn extra_cannot_replace_what_litegen_sends() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({ "detail": "stop" })))
            .mount(&server)
            .await;
        let mut base = make_base("waves", "replicate/video");
        base.seed = Some(5);
        let mut extras = make_extras();
        extras.extra = Some(json!({ "prompt": "other", "seed": 9, "steps": 20 }));
        let _ = make_provider(&server.uri()).generate(&ref_schema("replicate/video"), &base, &extras, &empty_materialized()).await;
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body["input"]["prompt"], "waves");
        assert_eq!(body["input"]["seed"], 5);
        assert_eq!(body["input"]["steps"], 20);
    }
}
