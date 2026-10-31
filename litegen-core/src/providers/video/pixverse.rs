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
/// @see <https://docs.platform.pixverse.ai/start-here-2394989m0.md> (2026-10-30)
///   Verbatim: "Use a new UUID v4 value for `Ai-trace-id` with every request."
/// @see <https://docs.platform.pixverse.ai/first-and-last-frame-video-43051213e0.md> (2026-10-30)
///   Verbatim: "required: - model - prompt - duration - quality - first_frame_img - last_frame_img"
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
        match model_id {
            "pixverse/v5-multi-transition" => "v5",
            _ => model_id.strip_prefix("pixverse/").unwrap_or(model_id),
        }
    }

    /// Timed keyframes (3D doc §5) as Multi-Transition items: uploaded in time
    /// order, each `duration` the whole seconds to the next keyframe and the
    /// last 0, every segment carrying the prompt. The clip starts at the
    /// first keyframe, so one must sit at 0 s; that is checked before any call.
    ///
    /// @see <https://docs.platform.pixverse.ai/multi-transition-video-43051215e0.md>
    ///   "Supply frames in order; each duration describes the segment leading to the next frame. For the final item, omit duration or set it to 0."
    async fn multi_transition(
        &self,
        creds: &ProviderCredentials,
        native: &str,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<Value, ProviderError> {
        let mut keyframes: Vec<_> = materialized.refs.iter().filter(|r| r.role == "keyframe").collect();
        keyframes.sort_by(|a, b| a.time_s.unwrap_or_default().total_cmp(&b.time_s.unwrap_or_default()));
        let times: Vec<f64> = keyframes
            .iter()
            .map(|r| r.time_s.ok_or_else(|| ProviderError::InvalidRequest("every keyframe needs time_s".into())))
            .collect::<Result<_, _>>()?;
        if times.first().is_some_and(|t| t.abs() > 1e-3) {
            return Err(ProviderError::InvalidRequest(format!(
                "PixVerse's multi-transition starts at its first keyframe: put one at 0 s, not {} s",
                times[0]
            )));
        }
        let mut items = Vec::with_capacity(keyframes.len());
        for (i, r) in keyframes.iter().enumerate() {
            let MaterializedRefForm::MultipartField { bytes, content_type, .. } = &r.form else {
                return Err(ProviderError::InvalidRequest("PixVerse keyframes are uploaded: the catalog must declare multipart".into()));
            };
            let img_id = self.upload_image(creds, bytes, content_type).await?;
            let duration = times.get(i + 1).map_or(0, |next| (next - times[i]).round() as i64);
            let prompt = if i + 1 < keyframes.len() { base.prompt.as_str() } else { "" };
            items.push(json!({ "img_id": img_id, "duration": duration, "prompt": prompt }));
        }
        let quality = extras.resolution.as_deref().unwrap_or(DEFAULT_QUALITY);
        Ok(json!({ "model": native, "quality": quality, "multi_transition": items }))
    }

    /// Text-, image- and first/last-frame-to-video: one request each.
    async fn single_shot(
        &self,
        creds: &ProviderCredentials,
        native: &str,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<(&'static str, Value), ProviderError> {
        // What PixVerse cannot make is refused before anything is uploaded
        // (an upload counts against the account's quota).
        // @see <https://docs.platform.pixverse.ai/first-and-last-frame-video-43051213e0.md> — the
        //   transition `model` enum is "- c1 - v6 - v5.6 - v5"; it needs "first_frame_img" and
        //   "last_frame_img"
        // @see <https://docs.platform.pixverse.ai/image-to-video-and-video-templates-13016633e0.md> —
        //   "Duration in seconds. C1/V6: 1-15; V5.5/V5.6: 5, 8, or 10; older models: 5 or 8."
        let has_role = |role: &str| materialized.refs.iter().any(|r| r.role == role);
        let refuse = |why: String| Err(ProviderError::InvalidRequest(format!("pixverse/{native}: {why}")));
        if has_role("last_frame") && !has_role("first_frame") {
            return refuse("a last_frame needs a first_frame".into());
        }
        if has_role("last_frame") && !matches!(native, "c1" | "v6" | "v5.6" | "v5") {
            return refuse("first and last frames are not a mode it has".into());
        }
        if matches!(native, "v3.5" | "v4.5" | "v5") && ![5.0, 8.0].contains(&extras.duration_seconds) {
            return refuse(format!("{} s is not a length it makes (5 or 8 s)", extras.duration_seconds));
        }

        // Upload any reference images first (-> img_id), tracking first/last.
        let mut first_img: Option<i64> = None;
        let mut last_img: Option<i64> = None;
        for r in &materialized.refs {
            if let MaterializedRefForm::MultipartField { bytes, content_type, .. } = &r.form {
                let img_id = self.upload_image(creds, bytes, content_type).await?;
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
            (TEXT_TO_VIDEO_PATH, json!({}))
        };
        body["model"] = Value::String(native.to_string());
        body["prompt"] = Value::String(base.prompt.clone());
        let quality = extras.resolution.as_deref().unwrap_or(DEFAULT_QUALITY);
        body["quality"] = Value::String(quality.to_string());
        if extras.duration_seconds > 0.0 {
            body["duration"] = Value::Number((extras.duration_seconds as i64).into());
        }
        // Only text/ takes an aspect ratio (it requires one); img/ and
        // transition/ take the image's shape.
        if path == TEXT_TO_VIDEO_PATH {
            body["aspect_ratio"] = Value::String(extras.aspect_ratio.as_deref().unwrap_or(DEFAULT_ASPECT_RATIO).to_string());
        }
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }
        // `extra` adds the endpoint's own fields; it cannot replace one litegen
        // sets (rule 11). transition/ has no multi-clip switch.
        let transition = path == "/openapi/v2/video/transition/generate";
        if let (Some(Value::Object(map)), Some(obj)) = (&extras.extra, body.as_object_mut()) {
            for (k, v) in map {
                let owned = matches!(k.as_str(), "model" | "prompt" | "quality" | "duration" | "aspect_ratio" | "seed" | "img_id" | "first_frame_img" | "last_frame_img");
                if !owned && !(transition && k == "generate_multi_clip_switch") {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
        // PixelGenie L10: audio on or off.
        if let Some(audio) = extras.audio {
            body["generate_audio_switch"] = Value::Bool(audio);
        }
        Ok((path, body))
    }

    /// Submit a generate call and read the job id out of PixVerse's envelope.
    async fn submit(
        &self,
        model: &ModelSchema,
        creds: &ProviderCredentials,
        path: &str,
        body: &Value,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let url = format!("{}{path}", self.api_base());
        let builder = self.auth_builder(creds, self.client.post(&url).header("Content-Type", "application/json").json(body))?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Pixverse request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // A call that timed out may still run, and bill.
                retryable: e.is_connect(),
            })?;

        let status = resp.status();
        let data: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Pixverse response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        let code = data["ErrCode"].as_i64().unwrap_or(-1);
        if code != 0 {
            return Err(ProviderError::RequestFailed {
                message: format!("Pixverse API error {code}: {}", data["ErrMsg"].as_str().unwrap_or("unknown")),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                // Turned away at a limit, so never accepted: "500044 | Reached
                // the limit for concurrent generations", "500069 | The system is
                // experiencing high load".
                // @see <https://docs.platform.pixverse.ai/error-codes-2394992m0.md>
                retryable: matches!(code, 500010 | 500044 | 500069),
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

/// `quality` sent when the request leaves `resolution` unset. All three
/// generate endpoints require it and PixVerse has no server-side default;
/// 540p is the spec's example value and, with 360p, the cheapest v3.5–v5 tier
/// (45 credits per 5s clip) — the price `base_cost_usd` assumes for v4.5/v5.
/// @see <https://docs.platform.pixverse.ai/text-to-video-13016634e0.md> (2026-10-30)
///   (`required: - model - prompt - duration - quality - aspect_ratio`)
/// @see <https://docs.platform.pixverse.ai/pixverse-api-pricing-2394995m0.md> (2026-10-30)
const DEFAULT_QUALITY: &str = "540p";

/// `aspect_ratio` sent to text/generate, which requires it, when the request
/// leaves it unset. img/ and transition/ take their shape from the image and
/// have no such field.
const DEFAULT_ASPECT_RATIO: &str = "16:9";

const TEXT_TO_VIDEO_PATH: &str = "/openapi/v2/video/text/generate";

#[async_trait]
impl VideoProvider for PixverseProvider {
    fn name(&self) -> &str {
        "pixverse"
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

    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);

        let (path, body) = if materialized.refs.iter().any(|r| r.role == "keyframe") {
            ("/openapi/v2/video/multi_transition/generate", self.multi_transition(&creds, native, base, extras, materialized).await?)
        } else {
            self.single_shot(&creds, native, base, extras, materialized).await?
        };
        self.submit(model, &creds, path, &body).await
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
        let data = super::read_poll_json(resp, "pixverse").await?;
        // "ErrCode is the numeric business error code in the response body …
        // Do not rely on the HTTP status alone": an error is an error, never
        // a job still processing until the reaper.
        // @see <https://docs.platform.pixverse.ai/error-codes-2394992m0.md>
        let code = data["ErrCode"].as_i64().unwrap_or(-1);
        if code != 0 {
            return Err(ProviderError::RequestFailed {
                message: format!("Pixverse poll error {code}: {}", data["ErrMsg"].as_str().unwrap_or("unknown")),
                status_code: None,
                provider_error: Some(data.clone()),
                retryable: matches!(code, 500 | 500069 | 99999),
            });
        }

        // Resp.status: 1=success, 5=processing, 7=moderation fail, 8=failed;
        // `Resp` has no failure reason, so the status says which.
        // @see <https://docs.platform.pixverse.ai/query-task-status-43051212e0.md>
        let url = data["Resp"]["url"].as_str().map(String::from);
        let (status, video_url, error) = match data["Resp"]["status"].as_i64() {
            Some(1) if url.is_some() => (GenerationStatus::Completed, url, None),
            Some(1) => (GenerationStatus::Failed, None, Some("PixVerse reported success without a video URL".to_string())),
            Some(7) => (GenerationStatus::Failed, None, Some("content moderation failed (status 7)".to_string())),
            Some(8) => (GenerationStatus::Failed, None, Some("generation failed (status 8)".to_string())),
            _ => (GenerationStatus::Processing, None, None),
        };

        Ok(VideoGenerationPollResult {
            status,
            progress: if status.is_terminal() { 100 } else { 50 },
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
        VideoExtras { duration_seconds: 5.0, aspect_ratio: Some("16:9".to_string()), resolution: Some("720p".to_string()), fps: None, extra: None, camera: None, audio: None, r#loop: None, source: None, direction: None, time_range: None, replace: None }
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
        let mut extras = make_extras();
        extras.audio = Some(true);
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "12345");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("api-key").unwrap(), "pv-key");
        assert!(post.headers.get("ai-trace-id").is_some(), "Ai-trace-id header must be present");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["generate_audio_switch"], true, "PixelGenie L10: audio true");
        assert_eq!(body["model"], "v4.5");
        assert_eq!(body["quality"], "720p");

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_url.unwrap(), "https://cdn.pixverse/v.mp4");
    }

    fn extras_without_quality_or_ratio() -> VideoExtras {
        VideoExtras { duration_seconds: 5.0, aspect_ratio: None, resolution: None, fps: None, extra: None, camera: None, audio: None, r#loop: None, source: None, direction: None, time_range: None, replace: None }
    }

    async fn mount_ok(server: &MockServer, endpoint: &str, resp: Value) {
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ErrCode": 0, "ErrMsg": "success", "Resp": resp
            })))
            .mount(server)
            .await;
    }

    fn keyframes(times: &[f64]) -> MaterializedRequest {
        MaterializedRequest {
            refs: times
                .iter()
                .map(|t| MaterializedRef {
                    role: "keyframe".to_string(),
                    form: MaterializedRefForm::MultipartField {
                        field_name: "image".to_string(),
                        bytes: bytes::Bytes::from(format!("keyframe-at-{t}s").into_bytes()),
                        content_type: "image/png".to_string(),
                    },
                    time_s: Some(*t),
                })
                .collect(),
            cleanup: Cleanup::empty(),
        }
    }

    /// Timed keyframes reach Multi-Transition in time order, each `duration`
    /// the whole seconds to the next keyframe and the last 0, every segment
    /// carrying the prompt.
    /// @see <https://docs.platform.pixverse.ai/multi-transition-video-43051215e0.md>
    ///   "Supply frames in order; each duration describes the segment leading to the next frame. For the final item, omit duration or set it to 0."
    #[tokio::test]
    async fn timed_keyframes_reach_multi_transition_as_segment_durations() {
        let server = MockServer::start().await;
        for (priority, id) in [(1u8, 11), (2, 12), (3, 13)] {
            Mock::given(method("POST"))
                .and(path("/openapi/v2/image/upload"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ErrCode": 0, "ErrMsg": "success", "Resp": { "img_id": id } })))
                .up_to_n_times(1)
                .with_priority(priority)
                .mount(&server)
                .await;
        }
        mount_ok(&server, "/openapi/v2/video/multi_transition/generate", json!({ "video_id": 9 })).await;
        let id = "pixverse/v5-multi-transition";
        make_provider(&server.uri())
            .generate(&ref_schema(id), &make_base("the knight swings", id), &extras_without_quality_or_ratio(), &keyframes(&[0.0, 2.0, 5.0]))
            .await
            .expect("generate");
        let received = server.received_requests().await.unwrap();
        let uploads: Vec<bool> = received.iter().filter(|r| r.url.path() == "/openapi/v2/image/upload").map(|r| r.body.windows(14).any(|w| w == b"keyframe-at-0s")).collect();
        assert_eq!(uploads, [true, false, false], "uploaded in time order");
        let submit = received.iter().find(|r| r.url.path() == "/openapi/v2/video/multi_transition/generate").expect("a submit");
        let body: Value = serde_json::from_slice(&submit.body).unwrap();
        assert_eq!(body, json!({
            "model": "v5",
            "quality": "540p",
            "multi_transition": [
                { "img_id": 11, "duration": 2, "prompt": "the knight swings" },
                { "img_id": 12, "duration": 3, "prompt": "the knight swings" },
                { "img_id": 13, "duration": 0, "prompt": "" },
            ],
        }));
    }

    #[tokio::test]
    async fn a_multi_transition_starts_at_its_first_keyframe() {
        let server = MockServer::start().await;
        let id = "pixverse/v5-multi-transition";
        let err = make_provider(&server.uri())
            .generate(&ref_schema(id), &make_base("x", id), &extras_without_quality_or_ratio(), &keyframes(&[1.0, 3.0]))
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err}");
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing reached PixVerse");
    }

    /// text/generate requires `aspect_ratio` and `quality`, with no server-side
    /// default, so a request that leaves resolution/aspect_ratio unset must still
    /// carry both.
    /// @see <https://docs.platform.pixverse.ai/text-to-video-generation-13016634e0.md>
    #[tokio::test]
    async fn text_to_video_fills_required_quality_and_aspect_ratio() {
        let server = MockServer::start().await;
        mount_ok(&server, "/openapi/v2/video/text/generate", json!({ "video_id": 7 })).await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("pixverse/v5");
        let base = make_base("a paper airplane gliding", "pixverse/v5");
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
        provider
            .generate(&schema, &base, &extras_without_quality_or_ratio(), &materialized)
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["quality"], "540p");
        assert_eq!(body["aspect_ratio"], "16:9");
    }

    /// img/generate requires `quality` too, but has no `aspect_ratio` field (the
    /// image sets the shape), so none is invented for it.
    /// @see <https://docs.platform.pixverse.ai/image-to-video-generation-13016633e0.md>
    #[tokio::test]
    async fn image_to_video_fills_quality_but_invents_no_aspect_ratio() {
        let server = MockServer::start().await;
        mount_ok(&server, "/openapi/v2/image/upload", json!({ "img_id": 99 })).await;
        mount_ok(&server, "/openapi/v2/video/img/generate", json!({ "video_id": 8 })).await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("pixverse/v4.5");
        let base = make_base("the kite lifts off", "pixverse/v4.5");
        let materialized = MaterializedRequest {
            refs: vec![MaterializedRef {
                role: "first_frame".to_string(),
                form: MaterializedRefForm::MultipartField {
                    field_name: "image".to_string(),
                    bytes: bytes::Bytes::from_static(b"PNG"),
                    content_type: "image/png".to_string(),
                },
                time_s: None,
            }],
            cleanup: Cleanup::empty(),
        };
        provider
            .generate(&schema, &base, &extras_without_quality_or_ratio(), &materialized)
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let submit = received
            .iter()
            .find(|r| r.url.path() == "/openapi/v2/video/img/generate")
            .expect("img/generate was called");
        let body: Value = serde_json::from_slice(&submit.body).unwrap();
        assert_eq!(body["img_id"], 99);
        assert_eq!(body["quality"], "540p");
        assert!(body.get("aspect_ratio").is_none(), "img/generate has no aspect_ratio field");
    }

    // ─── PixVerse audit, 2026-10-30: P1, P5–P11 ─────────────────────────────

    fn uploaded(roles: &[&str]) -> MaterializedRequest {
        MaterializedRequest {
            refs: roles
                .iter()
                .map(|role| MaterializedRef {
                    role: role.to_string(),
                    form: MaterializedRefForm::MultipartField { field_name: "image".into(), bytes: bytes::Bytes::from_static(b"PNG"), content_type: "image/png".into() },
                    time_s: None,
                })
                .collect(),
            cleanup: Cleanup::empty(),
        }
    }

    /// P1, P6, P10: what PixVerse cannot make is refused before anything is
    /// uploaded — a last frame alone, first and last frames on a model the
    /// transition endpoint does not take ("- c1 - v6 - v5.6 - v5"), and a
    /// length v3.5 / v4.5 / v5 does not make (5 or 8 s).
    #[tokio::test]
    async fn what_a_model_cannot_make_is_refused_before_any_upload() {
        let server = MockServer::start().await;
        let p = make_provider(&server.uri());
        let mut six = make_extras();
        six.duration_seconds = 6.0;
        for (id, extras, refs) in [
            ("pixverse/v6", make_extras(), uploaded(&["last_frame"])),
            ("pixverse/v4.5", make_extras(), uploaded(&["first_frame", "last_frame"])),
            ("pixverse/v5", six, uploaded(&[])),
        ] {
            let err = p.generate(&ref_schema(id), &make_base("x", id), &extras, &refs).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{id}: {err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing uploaded or sent");
    }

    /// P5, P11: img/ and transition/ take the image's shape, and transition/
    /// has no multi-clip switch; `extra` cannot replace the model.
    #[tokio::test]
    async fn the_transition_body_has_only_what_transition_takes() {
        let server = MockServer::start().await;
        mount_ok(&server, "/openapi/v2/image/upload", json!({ "img_id": 7 })).await;
        mount_ok(&server, "/openapi/v2/video/transition/generate", json!({ "video_id": 9 })).await;
        let mut extras = make_extras();
        extras.extra = Some(json!({ "generate_multi_clip_switch": true, "model": "c1" }));
        make_provider(&server.uri())
            .generate(&ref_schema("pixverse/v6"), &make_base("x", "pixverse/v6"), &extras, &uploaded(&["first_frame", "last_frame"]))
            .await
            .unwrap();
        let sent = server.received_requests().await.unwrap();
        let submit = sent.iter().find(|r| r.url.path() == "/openapi/v2/video/transition/generate").unwrap();
        let body: Value = serde_json::from_slice(&submit.body).unwrap();
        assert!(body.get("aspect_ratio").is_none() && body.get("generate_multi_clip_switch").is_none(), "{body}");
        assert_eq!(body["model"], "v6", "{body}");
    }

    /// P7, P8: a poll whose ErrCode is not 0 is an error, never a job still
    /// processing; a failed job says which failure it was.
    #[tokio::test]
    async fn a_poll_error_is_an_error_and_a_failure_says_which() {
        let handle = VideoGenerationHandle { provider_job_id: "9".into(), provider: "pixverse".into(), model: "pixverse/v6".into() };
        for (body, retryable) in [(json!({ "ErrCode": 500008, "ErrMsg": "Requested data not found" }), false), (json!({ "ErrCode": 500069, "ErrMsg": "high load" }), true)] {
            let server = MockServer::start().await;
            Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(body.clone())).mount(&server).await;
            match make_provider(&server.uri()).poll_status(&handle).await {
                Err(ProviderError::RequestFailed { retryable: r, .. }) => assert_eq!(r, retryable, "{body}"),
                other => panic!("{body}: {other:?}"),
            }
        }
        for (code, says) in [(7, "content moderation"), (8, "generation failed")] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ErrCode": 0, "ErrMsg": "success", "Resp": { "id": 9, "status": code } })))
                .mount(&server)
                .await;
            let poll = make_provider(&server.uri()).poll_status(&handle).await.unwrap();
            assert_eq!((poll.status, poll.progress), (GenerationStatus::Failed, 100));
            assert!(poll.error.as_deref().unwrap_or("").contains(says), "{code}: {:?}", poll.error);
        }
    }

    /// P9: a submit PixVerse turned away at its limits was never accepted,
    /// so it may be sent again.
    #[tokio::test]
    async fn a_submit_turned_away_at_the_limit_may_be_sent_again() {
        for (code, retryable) in [(500044, true), (500069, true), (400017, false)] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ErrCode": code, "ErrMsg": "no" })))
                .mount(&server)
                .await;
            let err = make_provider(&server.uri()).generate(&ref_schema("pixverse/v6"), &make_base("x", "pixverse/v6"), &make_extras(), &uploaded(&[])).await.unwrap_err();
            assert!(matches!(err, ProviderError::RequestFailed { retryable: r, .. } if r == retryable), "{code}: {err:?}");
        }
    }
}
