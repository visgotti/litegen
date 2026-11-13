use crate::providers::auth::Authorize;
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
/// Serves the Dream Machine models (`"VideoModel": {"enum": ["ray-2",
/// "ray-flash-2"]}`) with text-to-video and image-to-video (keyframes).
/// Camera motion is prompt text on Luma, not a field.
///
/// @see <https://docs.lumalabs.ai/reference/creategeneration> — create a generation
/// @see <https://docs.lumalabs.ai/reference/getgeneration> — get a generation (poll)
/// @see <https://docs.lumalabs.ai/docs/api> — Dream Machine API overview
pub struct LumaProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
    /// How every request to the vendor's API is authenticated.
    auth: crate::providers::auth::AuthSpec,
}

impl LumaProvider {
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
    fn resolve_model(model: &str) -> Option<&'static str> {
        Some(match model {
            "luma/dream-machine" | "luma/ray-2" => "ray-2",
            "luma/ray-flash-2" => "ray-flash-2",
            // An id with no arm is refused, not billed as ray-2 (L13).
            _ => return None,
        })
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
        let model_name = Self::resolve_model(&model.id).ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "luma".into() })?;
        let url = format!("{}/generations", self.api_base());

        // What Luma cannot make is refused before any call:
        // `"VideoModelOutputDuration": {"anyOf":[{"enum":["5s","9s"]}…]}`, and
        // "Loop is not supported for keyframes" (both frame0 and frame1).
        // @see <https://docs.lumalabs.ai/reference/creategeneration>
        // @see <https://docs.lumalabs.ai/docs/errors>
        let refuse = |why: String| Err(ProviderError::InvalidRequest(format!("{}: {why}", model.id)));
        if extras.duration_seconds > 0.0 && ![5.0, 9.0].contains(&extras.duration_seconds) {
            return refuse(format!("{} s is not a length it makes (5 or 9 s)", extras.duration_seconds));
        }
        if extras.r#loop == Some(true) && materialized.refs.iter().any(|r| r.role == "last_frame") {
            return refuse("a loop cannot have a last frame".into());
        }

        let mut body = json!({
            "model": crate::providers::wire_model(self.config.as_ref(), &model.id, model_name),
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
                        json!({ "type": "image", "url": crate::proxy::materializer::base64_data_uri(b64, Some("image/png")) }),
                    );
                }
                _ => {}
            }
        }
        if !keyframes.is_empty() {
            body["keyframes"] = Value::Object(keyframes);
        }

        // `extra` adds the endpoint's own fields; it cannot replace one litegen
        // sets (rule 11), and `camera_motion` is not a field at all.
        if let Some(body_obj) = body.as_object_mut() {
            let owned = |k: &str| matches!(k, "model" | "prompt" | "aspect_ratio" | "duration" | "resolution" | "keyframes" | "camera_motion");
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), owned) {
                body_obj.insert(k.clone(), v.clone());
            }
        }
        // PixelGenie L10: a clip that ends on its first frame.
        if let Some(looped) = extras.r#loop {
            body["loop"] = Value::Bool(looped);
        }

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
            message: format!("Luma request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            // A call that timed out may still run, and bill.
            retryable: e.is_connect(),
        })?;

        let status = resp.status();
        let retry_after = crate::providers::retry_after_ms(resp.headers());
        let text = resp.text().await.unwrap_or_default();
        let resp_json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);

        if !status.is_success() {
            // Luma's error is `{"detail": "Invalid API key is provided"}`; a rate
            // limit (429) was never accepted, so it may be sent again, after the
            // wait Luma names.
            let err = resp_json["detail"]
                .as_str()
                .or_else(|| resp_json["message"].as_str())
                .or_else(|| resp_json["error"].as_str())
                .map(str::to_string)
                .unwrap_or_else(|| text.chars().take(200).collect());
            if let Some(e) = crate::providers::rate_limited("luma", status, retry_after, &format!("Luma API error: {err}"), (!resp_json.is_null()).then_some(&resp_json)) {
                return Err(e);
            }
            return Err(ProviderError::RequestFailed {
                message: format!("Luma API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: (!resp_json.is_null()).then_some(resp_json),
                retryable: status.as_u16() >= 500 || status.as_u16() == 429,
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

        let data = super::read_poll_json(resp, "luma").await?;

        let status = match data["state"].as_str() {
            Some("completed") => GenerationStatus::Completed,
            Some("failed") => GenerationStatus::Failed,
            // state: queued | dreaming | completed | failed.
            Some("dreaming") | Some("processing") => GenerationStatus::Processing,
            Some("queued") => GenerationStatus::Pending,
            other => return Ok(super::unknown_state("luma", other)),
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
        let request = match self.client.get(format!("{}/generations?limit=1", self.api_base())).authorized(&self.auth, &api_key) {
            Ok(request) => request,
            Err(e) => return HealthCheckResult { healthy: false, message: format!("Luma health check failed: {e}"), latency_ms: None },
        };
        match request.timeout(std::time::Duration::from_secs(10)).send().await {
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

    /// L13: every catalog row has its own arm, and an id with none is refused
    /// rather than billed as another model.
    #[test]
    fn every_catalog_row_resolves_and_an_unknown_id_does_not() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        for m in catalog.for_provider("luma").filter(|m| m.media_type == crate::capabilities::MediaType::Video) {
            assert!(LumaProvider::resolve_model(&m.id).is_some(), "{} has no arm", m.id);
        }
        assert!(LumaProvider::resolve_model("luma/not-a-model").is_none());
    }

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
                time_s: None,
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
            camera: None, audio: None, r#loop: None,
            source: None, direction: None, time_range: None, replace: None,
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
        let mut extras = make_extras();
        extras.r#loop = Some(true);
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
        assert_eq!(body["loop"], true, "PixelGenie L10: loop");
        assert_eq!(body["prompt"], "a sunset over the ocean");
        assert_eq!(body["keyframes"]["frame0"]["url"], "https://x");
        assert_eq!(body["keyframes"]["frame0"]["type"], "image");
    }

    // ─── Luma audit, 2026-10-30: L2, L4–L7 ──────────────────────────────────

    fn frames(roles: &[&str]) -> MaterializedRequest {
        MaterializedRequest {
            refs: roles.iter().map(|r| MaterializedRef { role: r.to_string(), form: MaterializedRefForm::Url(format!("https://x/{r}.png")), time_s: None }).collect(),
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    /// L2: only the catalog's ids have arms ("VideoModel": ray-2 |
    /// ray-flash-2); the Ray 1.6 / Ray 3 arms are gone.
    #[test]
    fn only_the_dream_machine_models_resolve() {
        for gone in ["luma/ray-3", "luma/ray-hdr-3", "luma/ray-1-6", "luma/dream-machine-1.5", "luma/ray-3-14"] {
            assert!(LumaProvider::resolve_model(gone).is_none(), "{gone}");
        }
        assert_eq!(LumaProvider::resolve_model("luma/ray-flash-2"), Some("ray-flash-2"));
    }

    /// L5, L7: what Luma cannot make is refused before any call — a length
    /// other than 5 or 9 s ("VideoModelOutputDuration": "5s" | "9s"), and a
    /// loop with a last frame ("Loop is not supported for keyframes").
    #[tokio::test]
    async fn what_luma_cannot_make_is_refused_before_any_call() {
        let server = MockServer::start().await;
        let p = make_provider(&server.uri());
        let schema = ref_schema("luma/ray-2");
        let mut seven = make_extras();
        seven.duration_seconds = 7.0;
        let mut looped = make_extras();
        looped.r#loop = Some(true);
        for (extras, refs) in [(seven, frames(&[])), (looped, frames(&["first_frame", "last_frame"]))] {
            let err = p.generate(&schema, &make_base("a lighthouse", "luma/ray-2"), &extras, &refs).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// L4: `camera_motion` is not a request field ("Just add the camera motion
    /// value as part of prompt itself"); it is never sent.
    #[tokio::test]
    async fn camera_motion_is_never_sent_as_a_field() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "g1" }))).mount(&server).await;
        let mut extras = make_extras();
        extras.extra = Some(json!({ "camera_motion": "orbit-left", "model": "ray-flash-2" }));
        make_provider(&server.uri()).generate(&ref_schema("luma/ray-2"), &make_base("a lighthouse", "luma/ray-2"), &extras, &frames(&[])).await.unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert!(body.get("camera_motion").is_none(), "{body}");
        assert_eq!(body["model"], "ray-2", "{body}");
    }

    /// E4a: where the result goes is litegen's (it polls, then fires the
    /// key's webhook), so Luma's `callback_url` is never sent; Luma's own
    /// fields still are.
    #[tokio::test]
    async fn callback_url_is_never_sent() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "g1" }))).mount(&server).await;
        let mut extras = make_extras();
        extras.extra = Some(json!({ "callback_url": "https://attacker.example/cb", "concepts": [{ "key": "dolly_zoom" }] }));
        make_provider(&server.uri()).generate(&ref_schema("luma/ray-2"), &make_base("a lighthouse", "luma/ray-2"), &extras, &frames(&[])).await.unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert!(body.get("callback_url").is_none(), "{body}");
        assert_eq!(body["concepts"], json!([{ "key": "dolly_zoom" }]), "{body}");
    }

    /// L6: Luma's error is its `detail`, and a rate limit was never accepted.
    #[tokio::test]
    async fn an_error_says_luma_s_detail_and_a_rate_limit_may_be_sent_again() {
        for (status, retryable) in [(400, false), (429, true)] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!({ "detail": "Invalid API key is provided" })))
                .mount(&server)
                .await;
            let err = make_provider(&server.uri()).generate(&ref_schema("luma/ray-2"), &make_base("a lighthouse", "luma/ray-2"), &make_extras(), &frames(&[])).await.unwrap_err();
            match err {
                ProviderError::RequestFailed { message, retryable: r, .. } => {
                    assert!(message.contains("Invalid API key"), "{message}");
                    assert_eq!(r, retryable, "{status}");
                }
                other => panic!("{other:?}"),
            }
        }
    }
}
