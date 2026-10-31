// Fal.ai video generation provider
// API Reference: https://fal.ai/docs/documentation/model-apis/inference/queue
// Queue API: POST https://queue.fal.run/{endpoint} → poll status → fetch result
// Carried models: LTX Video and LTX-2.3

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRequest, MaterializedRefForm};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig,
    CancelOutcome, VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::providers::model3d::fal::{
    fal_error_message, needs_retry, platform_key_check, queue_app, same_origin_url, transient, QueueReply, DEFAULT_PLATFORM_BASE,
};
use crate::types::*;

/// What litegen itself sends in a submit body; `extra` cannot replace it.
const OWNED_FIELDS: &[&str] = &["prompt", "duration", "resolution", "fps", "aspect_ratio", "seed", "image_url"];

/// Fal.ai video generation provider.
///
/// Routes to model-specific Fal endpoints for LTX Video and LTX-2.3.
///
/// @see <https://fal.ai/docs/documentation/model-apis/inference/queue> — Verbatim: "The request
///   moves through three states before completion" (`IN_QUEUE`, `IN_PROGRESS`, `COMPLETED`); an
///   `error` is "present only if the request failed (only when `COMPLETED`)"
/// @see <https://fal.ai/models> — per-model API pages documenting each endpoint's input schema
pub struct FalVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
}

/// What a given fal endpoint's input schema actually accepts.
///
/// fal endpoints are per-model, and their schemas differ sharply — sending a
/// field the schema does not define is not harmless. `fal-ai/stable-video` has
/// no `prompt` at all; only the Kling and LTX-2.3 endpoints take `duration`;
/// only LTX-2.3 takes `aspect_ratio`, `resolution` and `fps`.
///
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id={id}>
///   — each endpoint publishes its own OpenAPI document.
struct EndpointSpec {
    /// Endpoint used when no reference image is supplied.
    text_to_video: &'static str,
    /// Endpoint used when a reference image is supplied, if the model has a
    /// distinct image-to-video variant.
    image_to_video: &'static str,
    supports_prompt: bool,
    supports_duration: bool,
    supports_aspect_ratio: bool,
    /// The discrete `duration` values (seconds) the endpoint accepts, sent as
    /// an integer; any other value is rejected before submitting. Empty =
    /// forward `duration_seconds` unchanged (when `supports_duration`).
    duration_values: &'static [u32],
    /// Whether the endpoint takes `resolution` (forwarded as given).
    supports_resolution: bool,
    /// The `fps` values the endpoint accepts; any other value is rejected
    /// before submitting. Empty = the endpoint takes no `fps`.
    fps_values: &'static [u32],
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
            .trim_end_matches('/')
    }

    /// fal's Platform API, which the health check reads.
    fn platform_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.options.as_ref())
            .and_then(|o| o["platform_api_base"].as_str())
            .unwrap_or(DEFAULT_PLATFORM_BASE)
    }

    /// A submitted job's status, result and cancel URLs: the ones fal answered
    /// with when they are on the configured base — the key travels with each
    /// call — else rebuilt from that base with the endpoint's app id, since a
    /// URL with the endpoint path answers 405.
    ///
    /// @see <https://fal.ai/docs/documentation/model-apis/inference/queue> — Verbatim: "A URL that
    ///   contains the endpoint path, for example `fal-ai/flux/schnell/requests/...`, returns `405`."
    fn queue_urls(&self, job: &Value) -> (String, String, String) {
        let base = self.api_base();
        let rebuilt = format!(
            "{base}/{}/requests/{}",
            queue_app(job["endpoint"].as_str().unwrap_or("")),
            job["request_id"].as_str().unwrap_or("")
        );
        let trusted = |k: &str| same_origin_url(base, job[k].as_str());
        let response = trusted("response_url").unwrap_or_else(|| rebuilt.clone());
        let status = trusted("status_url").unwrap_or_else(|| format!("{rebuilt}/status"));
        let cancel = format!("{response}/cancel");
        (status, response, cancel)
    }

    /// Resolve internal model ID to its fal endpoints and field support.
    /// `None` for an id with no arm: refused, not sent to LTX Video (L13).
    fn resolve_spec(model: &str) -> Option<EndpointSpec> {
        Some(match model {
            // Kling Video v1 standard — image-to-video only; takes `duration`.
            // LTX-2.3 (Pro). Text-to-video, or image-to-video (which requires
            // `image_url`) when a start frame is supplied. Both take prompt,
            // duration (integer enum 6 | 8 | 10), resolution (1080p | 1440p |
            // 2160p), aspect_ratio (16:9 | 9:16; i2v also `auto`), fps (24 |
            // 25 | 48 | 50) and generate_audio — no seed. Output: `video.url`.
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/ltx-2.3/text-to-video>
            // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/ltx-2.3/image-to-video>
            "fal/ltx-2.3" => EndpointSpec {
                text_to_video: "fal-ai/ltx-2.3/text-to-video",
                image_to_video: "fal-ai/ltx-2.3/image-to-video",
                supports_prompt: true,
                supports_duration: true,
                supports_aspect_ratio: true,
                duration_values: &[6, 8, 10],
                supports_resolution: true,
                fps_values: &[24, 25, 48, 50],
            },
            // LTX Video — the base endpoint is text-to-video only and has no
            // `image_url`; image-to-video is a separate endpoint.
            "fal/video" => EndpointSpec {
                text_to_video: "fal-ai/ltx-video",
                image_to_video: "fal-ai/ltx-video/image-to-video",
                supports_prompt: true,
                supports_duration: false,
                supports_aspect_ratio: false,
                duration_values: &[],
                supports_resolution: false,
                fps_values: &[],
            },
            _ => return None,
        })
    }

    /// `value` as one of an endpoint's discrete integer values, or an
    /// InvalidRequest naming the accepted set (the catalog can only express a
    /// range, so e.g. 7 s passes validation for a 6 | 8 | 10 endpoint).
    fn one_of(model: &str, param: &str, value: f64, allowed: &[u32]) -> Result<u32, ProviderError> {
        allowed
            .iter()
            .copied()
            .find(|&v| f64::from(v) == value)
            .ok_or_else(|| {
                ProviderError::InvalidRequest(format!(
                    "{model} takes {param} one of {allowed:?}; got {value}"
                ))
            })
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
    /// @see <https://fal.ai/docs/documentation/model-apis/inference/queue> — submit a request: the
    ///   `Authorization: Key <token>` auth and the `request_id`/`status_url`/`response_url` response
    ///   fields. Verbatim: "Use the URLs from the submit response."
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let api_key = self.api_key()?;
        let spec = Self::resolve_spec(&model.id)
            .ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "fal".into() })?;

        // Reference image (init role, URL form preferred by Fal).
        let image_url: Option<String> = materialized.refs.iter().find_map(|r| match &r.form {
            MaterializedRefForm::Url(u) => Some(u.clone()),
            MaterializedRefForm::Base64(b64) => Some(format!("data:image/png;base64,{b64}")),
            _ => None,
        });

        // Pick the endpoint that actually accepts an image when one is present.
        let endpoint = if image_url.is_some() {
            spec.image_to_video
        } else {
            spec.text_to_video
        };
        let url = format!("{}/{}", self.api_base(), endpoint);

        let mut body = json!({});
        if spec.supports_prompt {
            body["prompt"] = Value::String(base.prompt.clone());
        }

        if spec.supports_duration && extras.duration_seconds > 0.0 {
            body["duration"] = if spec.duration_values.is_empty() {
                json!(extras.duration_seconds)
            } else {
                json!(Self::one_of(&model.id, "duration", extras.duration_seconds, spec.duration_values)?)
            };
        }

        if spec.supports_resolution {
            if let Some(r) = extras.resolution.as_deref() {
                body["resolution"] = Value::String(r.to_string());
            }
        }

        if !spec.fps_values.is_empty() {
            if let Some(fps) = extras.fps {
                body["fps"] = json!(Self::one_of(&model.id, "fps", f64::from(fps), spec.fps_values)?);
            }
        }

        if spec.supports_aspect_ratio {
            if let Some(ar) = extras.aspect_ratio.as_deref() {
                body["aspect_ratio"] = Value::String(ar.to_string());
            }
        }

        if let Some(seed) = base.seed {
            body["seed"] = json!(seed);
        }

        if let Some(u) = image_url {
            body["image_url"] = Value::String(u);
        }

        // `extra` adds the endpoint's own knobs; it cannot replace a field
        // litegen sets (rule 11), which lax mode would otherwise let it.
        if let (Some(Value::Object(extra_map)), Some(body_obj)) = (&extras.extra, body.as_object_mut()) {
            for (k, v) in extra_map {
                if !OWNED_FIELDS.contains(&k.as_str()) {
                    body_obj.insert(k.clone(), v.clone());
                }
            }
        }
        // PixelGenie L10: audio on or off, where the endpoint takes it.
        if let Some(audio) = extras.audio {
            body["generate_audio"] = Value::Bool(audio);
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
            status_code: None,
            provider_error: None,
            // Only a connection that never opened was surely not received.
            retryable: e.is_connect(),
        })?;

        let retry = needs_retry(resp.headers());
        let reply = QueueReply::read(resp).await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Fal video response could not be read: {e}"),
            status_code: None,
            provider_error: None,
            retryable: false,
        })?;
        if !reply.status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Fal video API error: {}", reply.describe()),
                status_code: Some(reply.status.as_u16()),
                retryable: retry.unwrap_or_else(|| transient(reply.status)),
                provider_error: reply.json,
            });
        }
        let resp_json = reply.json.unwrap_or(Value::Null);

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
    /// fal has no FAILED status: a failed run is `COMPLETED` with an `error`,
    /// or a result endpoint that answers the model's own error. Either is the
    /// job's verdict (`Ok(Failed)`); only a reply without fal's envelope on a
    /// transient code — a gateway's page — is worth asking again.
    ///
    /// @see <https://fal.ai/docs/api-reference/inference-api/queue/result.md> — Verbatim: "If the
    ///   model returned an error, this operation returns the same error."
    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let api_key = self.api_key()?;
        let job_data: Value = serde_json::from_str(&handle.provider_job_id).unwrap_or_default();
        let (status_url, response_url, _) = self.queue_urls(&job_data);
        let failed = |error: String| VideoGenerationPollResult {
            status: GenerationStatus::Failed,
            progress: 0,
            video_url: None,
            video_data: None,
            content_type: None,
            error: Some(error),
            metadata: HashMap::new(),
        };

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
        let data = super::read_poll_json(resp, "fal").await?;

        let status = match data["status"].as_str() {
            Some("COMPLETED") => GenerationStatus::Completed,
            Some("IN_PROGRESS") => GenerationStatus::Processing,
            _ => GenerationStatus::Pending,
        };
        if status != GenerationStatus::Completed {
            return Ok(VideoGenerationPollResult {
                status,
                progress: if status == GenerationStatus::Processing { 50 } else { 10 },
                video_url: None,
                video_data: None,
                content_type: Some("video/mp4".into()),
                error: None,
                metadata: HashMap::new(),
            });
        }
        if let Some(err) = data["error"].as_str() {
            return Ok(failed(match data["error_type"].as_str() {
                Some(kind) => format!("{err} ({kind})"),
                None => err.to_string(),
            }));
        }

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
        let reply = QueueReply::read(result_resp).await.map_err(|e| ProviderError::RequestFailed {
            message: format!("fal video result could not be read: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;
        if !reply.status.is_success() {
            let message = format!("fal video result answered HTTP {}: {}", reply.status.as_u16(), reply.describe());
            if reply.is_verdict() {
                return Ok(failed(message));
            }
            return Err(ProviderError::RequestFailed {
                message,
                status_code: Some(reply.status.as_u16()),
                provider_error: reply.json,
                retryable: true,
            });
        }
        let result = reply.json.unwrap_or(Value::Null);

        // Video URL is in various locations depending on the model
        let video_url = result["video"]["url"]
            .as_str()
            .or_else(|| result["video_url"].as_str())
            .or_else(|| result["videos"].as_array().and_then(|a| a.first()).and_then(|v| v["url"].as_str()))
            .or_else(|| result["output"]["url"].as_str())
            .map(String::from);
        let Some(video_url) = video_url else {
            let message = match result.get("detail") {
                Some(_) => fal_error_message(&result),
                None => format!("fal reported COMPLETED but the result has no video URL: {result}"),
            };
            return Ok(failed(message));
        };
        Ok(VideoGenerationPollResult {
            status: GenerationStatus::Completed,
            progress: 100,
            video_url: Some(video_url),
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

    /// Cancel a request (`PUT .../requests/{id}/cancel`, under the
    /// `response_url` the submit returned): a queued request is removed and
    /// never runs; a running one is signalled and may still finish.
    ///
    /// @see <https://fal.ai/docs/documentation/model-apis/inference/queue> (Cancel a Request:
    /// 200 `OK`, 202 `CANCELLATION_REQUESTED`, 400 `ALREADY_COMPLETED`, 404 `NOT_FOUND`)
    async fn cancel(&self, handle: &VideoGenerationHandle) -> Result<CancelOutcome, ProviderError> {
        let api_key = self.api_key()?;
        let job: Value = serde_json::from_str(&handle.provider_job_id).unwrap_or_default();
        let (_, _, url) = self.queue_urls(&job);
        let resp = crate::providers::inject_trace_headers(
            self.client.put(&url).header("Authorization", format!("Key {api_key}")),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("fal video cancel failed: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        match (status.as_u16(), body["status"].as_str()) {
            (200..=299, _) => Ok(CancelOutcome::Requested),
            (400, Some("ALREADY_COMPLETED")) => Ok(CancelOutcome::AlreadyFinished),
            (404, _) => Ok(CancelOutcome::NotFound),
            _ => Err(ProviderError::RequestFailed {
                message: format!("fal video cancel answered {status}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(body),
                retryable: status.is_server_error(),
            }),
        }
    }

    /// Check the key against fal's Platform API (`platform_key_check`).
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Fal Video provider not configured".into(),
                latency_ms: None,
            };
        }
        let Ok(api_key) = self.api_key() else {
            return HealthCheckResult { healthy: false, message: "No API key".into(), latency_ms: None };
        };
        platform_key_check(&self.client, self.platform_base(), &api_key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        for m in catalog.for_provider("fal").filter(|m| m.media_type == crate::capabilities::MediaType::Video) {
            assert!(FalVideoProvider::resolve_spec(&m.id).map(|s| s.text_to_video).is_some(), "{} has no arm", m.id);
        }
        assert!(FalVideoProvider::resolve_spec("fal/not-a-model").map(|s| s.text_to_video).is_none());
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
            camera: None, audio: None, r#loop: None,
            source: None, direction: None, time_range: None, replace: None,
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
        let mut extras = make_extras();
        extras.audio = Some(false);
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
        assert_eq!(body["generate_audio"], false, "PixelGenie L10: audio false");
    }

    /// `fal-ai/ltx-video`'s input schema is
    /// `[guidance_scale, seed, num_inference_steps, negative_prompt, prompt]`.
    /// There is no `image_url`; the image-to-video variant is a separate
    /// endpoint, `fal-ai/ltx-video/image-to-video`.
    /// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/ltx-video>
    #[tokio::test]
    async fn reference_image_routes_to_the_image_to_video_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/fal-ai/ltx-video/image-to-video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "request_id": "req-1"
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("fal/video");
        let base = make_base("pan across the valley", "fal/video");
        let materialized = crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: crate::proxy::materializer::MaterializedRefForm::Url(
                    "https://example.com/frame.png".to_string(),
                ),
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };

        provider
            .generate(&schema, &base, &make_extras(), &materialized)
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1, "should hit the i2v endpoint exactly once");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["image_url"], "https://example.com/frame.png");
    }

    /// None of the mapped fal video endpoints declare `aspect_ratio`, and only
    /// the Kling ones declare `duration`. Sending them to `fal-ai/ltx-video`
    /// puts fields in the body that its schema does not define.
    #[tokio::test]
    async fn does_not_send_duration_or_aspect_ratio_to_ltx_video() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/fal-ai/ltx-video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "request_id": "req-1"
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("fal/video");
        let base = make_base("pan across the valley", "fal/video");
        // Set both fields explicitly so the assertions below are not vacuous.
        let mut extras = make_extras();
        extras.aspect_ratio = Some("16:9".to_string());
        assert!(extras.duration_seconds > 0.0, "duration must be set for this test to mean anything");

        provider
            .generate(&schema, &base, &extras, &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert!(body.get("aspect_ratio").is_none(), "ltx-video has no aspect_ratio: {body}");
        assert!(body.get("duration").is_none(), "ltx-video has no duration: {body}");
        assert_eq!(body["prompt"], "pan across the valley");
    }

    // ─── LTX-2.3: fal-ai/ltx-2.3/{text,image}-to-video ─────────────────────
    // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/ltx-2.3/text-to-video>
    // @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/ltx-2.3/image-to-video>

    const LTX23: &str = "fal/ltx-2.3";

    fn ltx23_extras() -> VideoExtras {
        VideoExtras {
            duration_seconds: 8.0,
            aspect_ratio: Some("9:16".to_string()),
            resolution: Some("1440p".to_string()),
            fps: Some(24),
            extra: Some(json!({ "generate_audio": false })),
            camera: None, audio: None, r#loop: None,
            source: None, direction: None, time_range: None, replace: None,
        }
    }

    fn start_frame() -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: crate::proxy::materializer::MaterializedRefForm::Url(
                    "https://example.com/frame.png".to_string(),
                ),
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    /// Mount a submit response for `endpoint` whose status/response URLs point
    /// back at the mock server, so a later poll stays local.
    async fn mount_ltx23_submit(server: &MockServer, endpoint: &str) {
        let base = server.uri();
        Mock::given(method("POST"))
            .and(path(format!("/{endpoint}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "request_id": "ltx-1",
                "status": "IN_QUEUE",
                "status_url": format!("{base}/{endpoint}/requests/ltx-1/status"),
                "response_url": format!("{base}/{endpoint}/requests/ltx-1"),
            })))
            .mount(server)
            .await;
    }

    /// Text-only submit: the t2v endpoint, `Key` auth, and the fields its
    /// schema defines — duration and fps as integers from their enums.
    #[tokio::test]
    async fn ltx_2_3_text_only_submits_to_the_text_to_video_endpoint() {
        let server = MockServer::start().await;
        mount_ltx23_submit(&server, "fal-ai/ltx-2.3/text-to-video").await;

        let provider = make_provider(&server.uri());
        let handle = provider
            .generate(&ref_schema(LTX23), &make_base("a lighthouse in a storm", LTX23), &ltx23_extras(), &empty_materialized())
            .await
            .expect("generate");
        assert_eq!(handle.model, LTX23);
        let job: Value = serde_json::from_str(&handle.provider_job_id).unwrap();
        assert_eq!(job["endpoint"], "fal-ai/ltx-2.3/text-to-video");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].url.path(), "/fal-ai/ltx-2.3/text-to-video");
        assert_eq!(received[0].headers.get("authorization").unwrap(), "Key test-key");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["prompt"], "a lighthouse in a storm");
        assert_eq!(body["duration"], json!(8));
        assert!(body["duration"].is_u64(), "duration is an integer enum: {body}");
        assert_eq!(body["resolution"], "1440p");
        assert_eq!(body["aspect_ratio"], "9:16");
        assert_eq!(body["fps"], json!(24));
        assert_eq!(body["generate_audio"], json!(false));
        assert!(body.get("image_url").is_none(), "t2v has no image_url: {body}");
        assert!(body.get("seed").is_none(), "ltx-2.3 has no seed: {body}");
    }

    /// A start frame routes to the i2v endpoint as `image_url` (required there).
    #[tokio::test]
    async fn ltx_2_3_start_frame_submits_to_the_image_to_video_endpoint() {
        let server = MockServer::start().await;
        mount_ltx23_submit(&server, "fal-ai/ltx-2.3/image-to-video").await;

        let provider = make_provider(&server.uri());
        provider
            .generate(&ref_schema(LTX23), &make_base("the waves rise", LTX23), &ltx23_extras(), &start_frame())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].url.path(), "/fal-ai/ltx-2.3/image-to-video");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["image_url"], "https://example.com/frame.png");
        assert_eq!(body["prompt"], "the waves rise");
        assert_eq!(body["duration"], json!(8));
    }

    /// duration is 6 | 8 | 10 and fps 24 | 25 | 48 | 50; the catalog can only
    /// express ranges, so the adapter rejects the gaps without calling fal.
    #[tokio::test]
    async fn ltx_2_3_rejects_duration_and_fps_outside_their_enums() {
        let server = MockServer::start().await;
        mount_ltx23_submit(&server, "fal-ai/ltx-2.3/text-to-video").await;
        let provider = make_provider(&server.uri());
        let schema = ref_schema(LTX23);

        let mut odd_duration = ltx23_extras();
        odd_duration.duration_seconds = 7.0;
        let mut odd_fps = ltx23_extras();
        odd_fps.fps = Some(30);
        for (extras, what) in [(odd_duration, "duration"), (odd_fps, "fps")] {
            match provider.generate(&schema, &make_base("x", LTX23), &extras, &empty_materialized()).await {
                Err(ProviderError::InvalidRequest(msg)) => assert!(msg.contains(what), "{msg}"),
                other => panic!("{what}: expected InvalidRequest, got {other:?}"),
            }
        }
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing may be submitted");
    }

    /// Submit through the ltx-2.3 arm, then mount the given status (and,
    /// optionally, result) responses for the poll.
    async fn ltx23_poll(status: Value, result: Option<Value>) -> VideoGenerationPollResult {
        let server = MockServer::start().await;
        let endpoint = "fal-ai/ltx-2.3/text-to-video";
        mount_ltx23_submit(&server, endpoint).await;
        Mock::given(method("GET"))
            .and(path(format!("/{endpoint}/requests/ltx-1/status")))
            .respond_with(ResponseTemplate::new(200).set_body_json(status))
            .mount(&server)
            .await;
        if let Some(result) = result {
            Mock::given(method("GET"))
                .and(path(format!("/{endpoint}/requests/ltx-1")))
                .respond_with(ResponseTemplate::new(200).set_body_json(result))
                .mount(&server)
                .await;
        }
        let provider = make_provider(&server.uri());
        let handle = provider
            .generate(&ref_schema(LTX23), &make_base("x", LTX23), &ltx23_extras(), &empty_materialized())
            .await
            .expect("generate");
        provider.poll_status(&handle).await.expect("poll")
    }

    #[tokio::test]
    async fn ltx_2_3_queue_states_map_to_pending_and_processing() {
        let queued = ltx23_poll(json!({ "status": "IN_QUEUE", "queue_position": 2 }), None).await;
        assert_eq!(queued.status, GenerationStatus::Pending);
        let running = ltx23_poll(json!({ "status": "IN_PROGRESS" }), None).await;
        assert_eq!(running.status, GenerationStatus::Processing);
        assert!(running.video_url.is_none());
    }

    /// Success: COMPLETED, then the result's `video.url` (LTXV23…Response).
    #[tokio::test]
    async fn ltx_2_3_completed_returns_the_result_video_url() {
        let done = ltx23_poll(
            json!({ "status": "COMPLETED" }),
            Some(json!({ "video": { "url": "https://v3b.fal.media/files/out.mp4", "content_type": "video/mp4" } })),
        )
        .await;
        assert_eq!(done.status, GenerationStatus::Completed);
        assert_eq!(done.video_url.as_deref(), Some("https://v3b.fal.media/files/out.mp4"));
    }

    /// fal has no FAILED status: a failed request is COMPLETED with `error`
    /// (and `error_type`). That must surface as failed with fal's message.
    #[tokio::test]
    async fn ltx_2_3_completed_with_an_error_is_failed_with_the_vendor_message() {
        let failed = ltx23_poll(
            json!({ "status": "COMPLETED", "error": "Image could not be downloaded", "error_type": "image_load_error" }),
            None,
        )
        .await;
        assert_eq!(failed.status, GenerationStatus::Failed);
        let err = failed.error.expect("error message");
        assert!(err.contains("Image could not be downloaded"), "{err}");
        assert!(err.contains("image_load_error"), "{err}");
    }

    /// A COMPLETED result without a video URL is a failure, not an empty success.
    #[tokio::test]
    async fn ltx_2_3_completed_without_a_video_url_is_failed() {
        let empty = ltx23_poll(json!({ "status": "COMPLETED" }), Some(json!({ "video": {} }))).await;
        assert_eq!(empty.status, GenerationStatus::Failed);
        assert!(empty.video_url.is_none());
        assert!(empty.error.expect("error").contains("no video URL"));
    }

    // ─── Cancel (L18) ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn cancel_puts_to_the_jobs_queue_url_and_maps_fals_four_answers() {
        use crate::providers::CancelOutcome;
        use wiremock::matchers::header;
        let cases = [
            (ResponseTemplate::new(200).set_body_json(serde_json::json!({ "status": "OK" })), CancelOutcome::Requested),
            (ResponseTemplate::new(202).set_body_json(serde_json::json!({ "status": "CANCELLATION_REQUESTED" })), CancelOutcome::Requested),
            (ResponseTemplate::new(400).set_body_json(serde_json::json!({ "status": "ALREADY_COMPLETED" })), CancelOutcome::AlreadyFinished),
            (ResponseTemplate::new(404).set_body_json(serde_json::json!({ "status": "NOT_FOUND" })), CancelOutcome::NotFound),
        ];
        for (answer, expected) in cases {
            let server = MockServer::start().await;
            // A sub-path endpoint's request lives under its app id: the
            // response_url fal returned at submit says where.
            Mock::given(method("PUT"))
                .and(path("/fal-ai/kling-video/requests/r1/cancel"))
                .and(header("authorization", "Key test-key"))
                .respond_with(answer)
                .expect(1)
                .mount(&server)
                .await;
            let job = serde_json::json!({
                "request_id": "r1",
                "endpoint": "fal-ai/kling-video/v2/master/text-to-video",
                "status_url": "",
                "response_url": format!("{}/fal-ai/kling-video/requests/r1", server.uri()),
            });
            let handle = VideoGenerationHandle { provider_job_id: job.to_string(), provider: "fal".into(), model: "fal/kling-video-v2-master".into() };
            assert_eq!(make_provider(&server.uri()).cancel(&handle).await.unwrap(), expected);
        }
    }

    // ─── fal audit, 2026-10-30: F02, F03, F05, F07, F10 ─────────────────────

    fn job_handle(model: &str, job: Value) -> VideoGenerationHandle {
        VideoGenerationHandle { provider_job_id: job.to_string(), provider: "fal".into(), model: model.into() }
    }

    /// F02: the key is never sent to a host fal names elsewhere, and a URL
    /// litegen builds names only the app — one with the endpoint path is a 405.
    #[tokio::test]
    async fn a_job_is_polled_and_cancelled_at_the_configured_base_by_its_app_id() {
        use wiremock::matchers::header;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/fal-ai/ltx-2.3/requests/r1/status"))
            .and(header("authorization", "Key test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "IN_PROGRESS" })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/fal-ai/ltx-2.3/requests/r1/cancel"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "status": "CANCELLATION_REQUESTED" })))
            .expect(1)
            .mount(&server)
            .await;
        let elsewhere = json!({
            "request_id": "r1", "endpoint": "fal-ai/ltx-2.3/text-to-video",
            "status_url": "https://elsewhere.example/fal-ai/ltx-2.3/requests/r1/status",
            "response_url": "https://elsewhere.example/fal-ai/ltx-2.3/requests/r1",
        });
        let p = make_provider(&server.uri());
        assert_eq!(p.poll_status(&job_handle("fal/ltx-2.3", elsewhere.clone())).await.unwrap().status, GenerationStatus::Processing);
        assert_eq!(p.cancel(&job_handle("fal/ltx-2.3", elsewhere)).await.unwrap(), CancelOutcome::Requested);
    }

    /// F03: fal has no FAILED status — a failure is COMPLETED with `error`, or
    /// a result that answers the model's own error, on every arm. Each is the
    /// job's verdict; only a gateway's page is worth asking again.
    #[tokio::test]
    async fn fals_verdicts_fail_the_job_and_a_gateway_page_is_asked_again() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/fal-ai/ltx-video/requests/r1/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "COMPLETED", "error": "prompt flagged", "error_type": "content_policy_violation" })))
            .mount(&server)
            .await;
        let job = json!({ "request_id": "r1", "endpoint": "fal-ai/ltx-video", "status_url": "", "response_url": "" });
        let poll = make_provider(&server.uri()).poll_status(&job_handle("fal/video", job)).await.unwrap();
        assert_eq!((poll.status, poll.error.as_deref()), (GenerationStatus::Failed, Some("prompt flagged (content_policy_violation)")));

        for (answer, verdict) in [
            (ResponseTemplate::new(422).set_body_json(json!({ "detail": [{ "loc": ["body", "prompt"], "msg": "field required", "type": "missing" }] })), Some("body.prompt: field required")),
            (ResponseTemplate::new(500).set_body_json(json!({ "detail": "the model crashed", "error_type": "internal_error" })), Some("the model crashed (internal_error)")),
            (ResponseTemplate::new(502).set_body_string("<html>Bad Gateway</html>"), None),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/fal-ai/ltx-2.3/requests/r2/status"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "COMPLETED" })))
                .mount(&server)
                .await;
            Mock::given(method("GET")).and(path("/fal-ai/ltx-2.3/requests/r2")).respond_with(answer).mount(&server).await;
            let job = json!({ "request_id": "r2", "endpoint": "fal-ai/ltx-2.3/text-to-video", "status_url": "", "response_url": "" });
            let got = make_provider(&server.uri()).poll_status(&job_handle("fal/ltx-2.3", job)).await;
            match verdict {
                Some(says) => {
                    let poll = got.unwrap();
                    assert_eq!(poll.status, GenerationStatus::Failed, "{says}");
                    assert!(poll.error.as_deref().unwrap_or("").contains(says), "{says}: {:?}", poll.error);
                }
                None => assert!(matches!(got, Err(ProviderError::RequestFailed { retryable: true, .. })), "{got:?}"),
            }
        }
    }

    /// F05: `extra` cannot replace what litegen sends.
    #[tokio::test]
    async fn extra_cannot_replace_what_litegen_sends() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/fal-ai/ltx-2.3/text-to-video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "request_id": "r3", "status_url": "", "response_url": "" })))
            .mount(&server)
            .await;
        let mut extras = make_extras();
        extras.duration_seconds = 6.0;
        extras.extra = Some(json!({ "prompt": "something else", "duration": 10, "image_url": "https://elsewhere.example/a.png", "enable_prompt_expansion": false }));
        make_provider(&server.uri()).generate(&ref_schema("fal/ltx-2.3"), &make_base("a fox", "fal/ltx-2.3"), &extras, &empty_materialized()).await.unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!((body["prompt"].as_str(), body["duration"].as_u64()), (Some("a fox"), Some(6)), "{body}");
        assert!(body.get("image_url").is_none(), "{body}");
        assert_eq!(body["enable_prompt_expansion"], false, "a knob litegen does not own still passes: {body}");
    }

    /// F07: the key is checked against fal's Platform API.
    #[tokio::test]
    async fn health_check_validates_the_key_against_the_platform_api() {
        use wiremock::matchers::query_param;
        for (status, healthy) in [(200, true), (401, false)] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/models/pricing"))
                .and(query_param("endpoint_id", "fal-ai/trellis"))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!({})))
                .expect(1)
                .mount(&server)
                .await;
            let mut p = FalVideoProvider::new();
            p.configure(ProviderInstanceConfig { api_key: "test-key".into(), options: Some(json!({ "platform_api_base": server.uri() })), ..Default::default() });
            assert_eq!(p.health_check().await.healthy, healthy, "{status}");
        }
    }

    /// F10: no arm serves an endpoint the catalog does not carry.
    #[test]
    fn only_the_carried_endpoints_have_arms() {
        for gone in ["fal/kling", "fal-kling", "fal/kling-pro", "fal/minimax", "fal/minimax-hailuo", "fal/animate-diff", "fal/svd"] {
            assert!(FalVideoProvider::resolve_spec(gone).is_none(), "{gone}");
        }
    }
}
