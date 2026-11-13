use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig,
    VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// Vidu video generation provider.
///
/// Async: `POST /ent/v2/{text2video|img2video|start-end2video|reference2video}`
/// returns `{task_id, state}`; poll `GET /ent/v2/tasks/{id}/creations` until
/// `state == "success"`, then read `creations[0].url`. Auth: the custom
/// `Authorization: Token <key>` header. Video-only.
///
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/text-to-video.md> (2026-11-09)
///   Verbatim: "POST https://api.vidu.com/ent/v2/text2video"
///   Verbatim: "`Authorization` | `string` | Required | Replace {your api key} with your token；Example: `Token {your api key}`"
/// @see <https://platform.vidu.com/docs/api-reference/task-management/task-detail.md> (2026-11-09)
///   Verbatim: "GET /ent/v2/tasks/{id}/creations"
///   (state "Possible values: `created` … `queueing` … `processing` … `success` … `failed`"; result at creations[].url)
pub struct ViduProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl ViduProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (result downloads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(120), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::Header { name: "Authorization".to_string(), value_prefix: "Token ".to_string() },
            client: Self::api_client(&Default::default()),
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.vidu.com")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("vidu".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("vidu/").unwrap_or(model_id)
    }

    fn ref_image(form: &MaterializedRefForm) -> Option<String> {
        match form {
            MaterializedRefForm::Url(u) => Some(u.clone()),
            // A data URI of the bytes' own type.
            MaterializedRefForm::Base64(b64) => Some(crate::proxy::materializer::base64_data_uri(b64, Some("image/png"))),
            _ => None,
        }
    }

    fn is_last_frame(role: &str) -> bool {
        role == "last_frame" || role == "end_frame"
    }

    /// Whether the endpoint's body has an `audio` switch for this model:
    /// viduq1 text2video ("Default: true"; its reference2video `images` form
    /// has none, only the subjects form litegen does not send), viduq2-pro
    /// img2video ("Default: false"), every Q3 endpoint.
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/text-to-video.md> (2026-11-09)
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/reference-to-video.md> (2026-11-09)
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q2/image-to-video.md> (2026-11-03)
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/start-end-to-video.md> (2026-11-03)
    /// Q4 Preview: img2video and reference2video, "Default: `true`" on both.
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q4-preview/image-to-video.md> (2026-11-03)
    fn takes_audio(native: &str, endpoint: &str) -> bool {
        match native {
            "viduq3-turbo" | "viduq3-pro" => true,
            "viduq4-preview" => matches!(endpoint, "img2video" | "reference2video"),
            // Each from its own operation (2026-11-03): viduq3 reference2video
            // ("Default is true"), viduq3-pro-fast / viduq2-turbo /
            // viduq1-classic img2video, viduq2 text2video ("Default: true");
            // viduq3-mix's operation has no `audio`.
            // @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/reference-to-video.md> (2026-11-03)
            "viduq3" => endpoint == "reference2video",
            "viduq3-pro-fast" | "viduq2-turbo" | "viduq1-classic" => endpoint == "img2video",
            "viduq2" => endpoint == "text2video",
            "viduq1" => endpoint == "text2video",
            "viduq2-pro" => endpoint == "img2video",
            _ => false,
        }
    }

    /// An allowlisted `extra` field the endpoint's body lacks for this model,
    /// left out rather than sent (rule 10): viduq1's text2video has neither
    /// `movement_amplitude` nor `bgm`, which its start-end2video and its
    /// reference2video `images` form take.
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/text-to-video.md> (2026-11-09)
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/start-end-to-video.md> (2026-11-09)
    fn endpoint_lacks(native: &str, endpoint: &str, field: &str) -> bool {
        native == "viduq1" && endpoint == "text2video" && matches!(field, "movement_amplitude" | "bgm")
    }

    /// vidu2.0 makes only what Vidu prices: image-to-video and first-last
    /// frame "4S" at 360p, 720p or 1080p and "8S" at 720p; reference-to-video
    /// "4S" at 360p or 720p. Anything else is refused, never snapped (L13).
    /// Other models pass: their rows' bounds already hold them.
    /// @see <https://platform.vidu.com/docs/pricing.md> (2026-11-03)
    ///   Verbatim: "| Image-to-video | vidu2.0 | 8S | 720p | General | 100 | 50 |",
    ///   "| Reference-to-video | vidu2.0 | 4S | 720p | General | 80 | 40 |"
    fn vidu2_makes(native: &str, endpoint: &str, secs: i64, resolution: Option<&str>) -> Result<(), String> {
        if native != "vidu2.0" {
            return Ok(());
        }
        let res = resolution.unwrap_or("720p");
        let priced = match endpoint {
            "reference2video" => secs == 4 && matches!(res, "360p" | "720p"),
            _ => (secs == 4 && matches!(res, "360p" | "720p" | "1080p")) || (secs == 8 && res == "720p"),
        };
        if priced {
            Ok(())
        } else {
            Err(format!("vidu2.0 makes no {secs} s {res} clip on {endpoint} (4 s at 360p/720p/1080p, 8 s at 720p; references 4 s at 360p/720p)"))
        }
    }
}

impl Default for ViduProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for ViduProvider {
    fn name(&self) -> &str {
        "vidu"
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

        // Choose the endpoint + image set by ref roles.
        let has_reference = materialized.refs.iter().any(|r| r.role == "reference");
        let has_last = materialized.refs.iter().any(|r| Self::is_last_frame(&r.role));
        // img2video uses its one image as the FIRST frame: "The model will use
        // the image provided via this parameter as the first frame", so a last
        // frame alone is refused rather than sent as a start.
        // @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/image-to-video.md> (2026-11-09)
        if has_last && !has_reference && !materialized.refs.iter().any(|r| r.role == "first_frame" || r.role == "init") {
            return Err(ProviderError::InvalidRequest(format!("{} needs a first_frame to go with a last_frame", model.id)));
        }
        // On reference2video Vidu documents viduq3-turbo only as
        // "Subjects-Reference to Video" (`subjects` "Required"); the `images`
        // form below names `viduq3` alone ("Accepted values: `viduq3`").
        // @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/reference-to-video.md> (2026-11-09)
        if has_reference && native == "viduq3-turbo" {
            return Err(ProviderError::InvalidRequest(format!(
                "{} takes no reference images: Vidu documents only a subjects form for it on reference2video",
                model.id
            )));
        }
        // start-end2video reads `images` positionally (start frame, then end
        // frame), so order last-frame refs after the rest instead of trusting
        // the order the caller listed them in. The sort is stable.
        let mut refs: Vec<_> = materialized.refs.iter().collect();
        refs.sort_by_key(|r| Self::is_last_frame(&r.role));
        let images: Vec<String> = refs.iter().filter_map(|r| Self::ref_image(&r.form)).collect();

        let endpoint = if has_reference {
            "reference2video"
        } else if images.len() >= 2 && has_last {
            "start-end2video"
        } else if !images.is_empty() {
            "img2video"
        } else if model.capabilities.text_to_video {
            "text2video"
        } else {
            // text2video only accepts the models the catalog marks
            // `text_to_video`: each Q3/Q2/Q1 text2video operation names its
            // own ("Accepted values: `viduq3-turbo`", "optional value:
            // `viduq3-pro`", "Accepted values: `viduq2`", "Accepted values:
            // `viduq1`"), and nothing before the adapter checks capability
            // flags — refuse instead of sending a request Vidu rejects.
            // @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/text-to-video.md> (2026-11-09)
            // @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q2/text-to-video.md> (2026-11-09)
            // @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/text-to-video.md> (2026-11-09)
            return Err(ProviderError::InvalidRequest(format!(
                "{} has no text-to-video mode; attach a first_frame or reference image",
                model.id
            )));
        };
        let url = format!("{}/ent/v2/{endpoint}", self.api_base());

        let mut body = json!({ "model": crate::providers::wire_model(self.config.as_ref(), &model.id, native), "prompt": base.prompt });
        if !images.is_empty() {
            body["images"] = Value::Array(images.into_iter().map(Value::String).collect());
        }
        // The row's default when none is named: the clip runs at what it was
        // priced at, never at a default Vidu picks by model and duration.
        let row_default = match model.params.get("resolution") {
            Some(crate::capabilities::ParamSpec::String(s)) => s.default.clone(),
            _ => None,
        };
        let resolution = extras.resolution.as_deref().or(row_default.as_deref());
        if extras.duration_seconds > 0.0 {
            // Whole seconds: every Vidu row's duration is `kind: int` (C5).
            let duration = extras.duration_seconds as i64;
            Self::vidu2_makes(native, endpoint, duration, resolution).map_err(ProviderError::InvalidRequest)?;
            body["duration"] = Value::Number(duration.into());
        }
        if let Some(res) = resolution {
            body["resolution"] = Value::String(res.to_string());
        }
        // Only text2video and reference2video take an aspect ratio; img2video
        // and start-end2video follow their images.
        if matches!(endpoint, "text2video" | "reference2video") {
            if let Some(ar) = extras.aspect_ratio.as_deref() {
                body["aspect_ratio"] = Value::String(ar.to_string());
            }
        }
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }
        // `extra` adds the endpoint's own fields; it cannot replace one litegen
        // sets (rule 11).
        if let Some(obj) = body.as_object_mut() {
            let owned = |k: &str| {
                matches!(k, "model" | "prompt" | "images" | "duration" | "resolution" | "aspect_ratio" | "seed")
                    || Self::endpoint_lacks(native, endpoint, k)
            };
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), owned) {
                obj.insert(k.clone(), v.clone());
            }
        }
        // PixelGenie L10: audio on or off (Vidu's own `audio`), only where the
        // endpoint's body has the switch (rule 10).
        if let Some(audio) = extras.audio {
            if Self::takes_audio(native, endpoint) {
                body["audio"] = Value::Bool(audio);
            } else if audio {
                return Err(ProviderError::InvalidRequest(format!(
                    "{} has no audio switch on {endpoint}: ask without audio, or use a mode that has one", model.id
                )));
            }
        }

        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Vidu request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // A call that timed out may still run, and bill: only a
                // connection that never opened is sent again.
                retryable: e.is_connect(),
            })?;

        let data = crate::providers::read_vendor_json(resp, "Vidu").await?;

        let task_id = data["task_id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Vidu response missing task_id".to_string(),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        })?;

        Ok(VideoGenerationHandle {
            provider_job_id: task_id.to_string(),
            provider: "vidu".to_string(),
            model: model.id.clone(),
        })
    }

    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/ent/v2/tasks/{}/creations", self.api_base(), handle.provider_job_id);
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(&url))?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: e.to_string(),
                status_code: None,
                provider_error: None,
                retryable: true,
            })?;
        let data = super::read_poll_json(resp, "vidu").await?;

        let status = match data["state"].as_str() {
            Some("success") => GenerationStatus::Completed,
            Some("failed") => GenerationStatus::Failed,
            // state: "Possible values: `created` Created successfully `queueing`
            // Task queued `processing` Task processing `success` Task succeeded
            // `failed` Task failed".
            // @see <https://platform.vidu.com/docs/api-reference/task-management/task-detail.md> (2026-11-09)
            Some("created") | Some("queueing") | Some("processing") => GenerationStatus::Processing,
            other => return Ok(super::unknown_state("vidu", other)),
        };
        let video_url = data["creations"][0]["url"].as_str().map(String::from);
        // A success with no creation to fetch is a failure: nothing can be
        // delivered from it.
        let (status, error) = match (status, &video_url) {
            (GenerationStatus::Completed, None) => (GenerationStatus::Failed, Some("Vidu reported success without a creation URL".to_string())),
            (GenerationStatus::Failed, _) => (
                status,
                Some(data["err_code"].as_str().filter(|s| !s.is_empty()).unwrap_or("Vidu task failed").to_string()),
            ),
            _ => (status, None),
        };

        Ok(VideoGenerationPollResult {
            status,
            progress: if status.is_terminal() { 100 } else { 50 },
            video_url: video_url.filter(|_| status == GenerationStatus::Completed),
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
                message: "Vidu provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Vidu provider configured".into(), latency_ms: None }
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

    fn make_provider(api_base: &str) -> ViduProvider {
        let mut p = ViduProvider::new();
        let mut cfg = ProviderInstanceConfig { api_key: "vidu-key".to_string(), api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.api_key = Some("vidu-key".to_string());
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
    async fn submits_text2video_with_token_auth_and_polls() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/ent/v2/text2video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": "vd-1", "state": "created" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/ent/v2/tasks/vd-1/creations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": "success", "creations": [{ "id": "c1", "url": "https://cdn.vidu/v.mp4" }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("vidu/viduq1");
        let base = make_base("a hummingbird in slow motion", "vidu/viduq1");
        let mut extras = make_extras();
        extras.audio = Some(false);
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "vd-1");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].headers.get("authorization").unwrap(), "Token vidu-key");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["audio"], false, "PixelGenie L10: audio false");
        assert_eq!(body["model"], "viduq1");

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_url.unwrap(), "https://cdn.vidu/v.mp4");
    }

    fn url_ref(role: &str, url: &str) -> MaterializedRef {
        MaterializedRef { role: role.to_string(), form: MaterializedRefForm::Url(url.to_string()), time_s: None }
    }

    async fn mount_submit(server: &MockServer, endpoint: &str) {
        Mock::given(method("POST"))
            .and(path(format!("/ent/v2/{endpoint}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": "vd-2", "state": "created" })))
            .mount(server)
            .await;
    }

    /// text2video accepts viduq3-turbo, viduq3-pro, viduq2 and viduq1 only (each
    /// Q3/Q2/Q1 text2video operation names its own model), and nothing upstream
    /// of the adapter checks `capabilities.text_to_video`, so a prompt-only
    /// request for an image-only model must never reach text2video.
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/text-to-video.md> (2026-11-09)
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q2/text-to-video.md> (2026-11-09)
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/text-to-video.md> (2026-11-09)
    #[tokio::test]
    async fn refuses_prompt_only_requests_for_image_only_models() {
        let server = MockServer::start().await;
        mount_submit(&server, "text2video").await;

        let provider = make_provider(&server.uri());
        for id in ["vidu/vidu2.0", "vidu/viduq2-pro"] {
            let schema = ref_schema(id);
            let base = make_base("a hummingbird in slow motion", id);
            let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
            let res = provider.generate(&schema, &base, &make_extras(), &materialized).await;
            assert!(matches!(res, Err(ProviderError::InvalidRequest(_))), "{id}: {res:?}");
        }
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "no request may be sent to text2video for an image-only model"
        );
    }

    /// start-end2video reads `images` as [start frame, end frame] ("The first
    /// uploaded image is treated as the start frame and the second as the end
    /// frame"); the order the caller listed its refs in must not swap them.
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/start-end-to-video.md> (2026-11-09)
    #[tokio::test]
    async fn start_end_images_are_ordered_by_role() {
        let server = MockServer::start().await;
        mount_submit(&server, "start-end2video").await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("vidu/viduq1");
        let base = make_base("a flower opening", "vidu/viduq1");
        let materialized = MaterializedRequest {
            refs: vec![
                url_ref("last_frame", "https://img.test/end.png"),
                url_ref("first_frame", "https://img.test/start.png"),
            ],
            cleanup: Cleanup::empty(),
        };
        provider.generate(&schema, &base, &make_extras(), &materialized).await.expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["images"], json!(["https://img.test/start.png", "https://img.test/end.png"]));
    }

    /// vidu2.0 makes only what Vidu prices: image-to-video and first-last
    /// frame "4S" at 360p, 720p or 1080p and "8S" at 720p; reference-to-video
    /// "4S" at 360p or 720p. Anything else is refused before anything is sent,
    /// never snapped to a length the caller did not ask for (L13).
    /// @see <https://platform.vidu.com/docs/pricing.md> (2026-11-03)
    #[tokio::test]
    async fn vidu2_makes_only_the_lengths_and_sizes_vidu_prices() {
        let server = MockServer::start().await;
        mount_submit(&server, "img2video").await;
        mount_submit(&server, "reference2video").await;
        let provider = make_provider(&server.uri());
        let schema = ref_schema("vidu/vidu2.0");
        let base = make_base("a hummingbird in slow motion", "vidu/vidu2.0");
        let ask = |role: &str, secs: f64, res: &str| {
            let mut extras = make_extras();
            extras.duration_seconds = secs;
            extras.resolution = Some(res.to_string());
            (extras, MaterializedRequest { refs: vec![url_ref(role, "https://img.test/a.png")], cleanup: Cleanup::empty() })
        };
        for (role, secs, res) in [("first_frame", 8.0, "1080p"), ("first_frame", 5.0, "720p"), ("reference", 8.0, "720p"), ("reference", 4.0, "1080p")] {
            let (extras, refs) = ask(role, secs, res);
            let err = provider.generate(&schema, &base, &extras, &refs).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{role} {secs} s {res}: {err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing sent");
        for (role, secs, res) in [("first_frame", 8.0, "720p"), ("first_frame", 4.0, "1080p"), ("reference", 4.0, "720p")] {
            let (extras, refs) = ask(role, secs, res);
            provider.generate(&schema, &base, &extras, &refs).await.unwrap_or_else(|e| panic!("{role} {secs} s {res}: {e:?}"));
        }
        let sent: Vec<i64> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap()["duration"].as_i64().unwrap())
            .collect();
        assert_eq!(sent, [8, 4, 4], "the length asked for, unchanged");
    }

    /// `audio` goes only to an endpoint whose body has the switch: viduq1's
    /// text2video, viduq2-pro's img2video, every Q3 endpoint. Asking for sound where there is no switch is refused before
    /// anything is sent; `audio: false` there sends nothing.
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/start-end-to-video.md> (2026-11-03)
    #[tokio::test]
    async fn audio_goes_only_where_the_endpoint_has_a_switch() {
        let server = MockServer::start().await;
        for e in ["text2video", "start-end2video", "img2video", "reference2video"] {
            mount_submit(&server, e).await;
        }
        let p = make_provider(&server.uri());
        let none: Vec<&str> = vec![];
        let ask = |audio: bool, roles: &[&str]| {
            let mut extras = make_extras();
            extras.audio = Some(audio);
            let refs = roles.iter().map(|r| url_ref(r, "https://img.test/a.png")).collect();
            (extras, MaterializedRequest { refs, cleanup: Cleanup::empty() })
        };
        for (id, audio, roles) in [("vidu/viduq1", true, &["first_frame", "last_frame"][..]), ("vidu/viduq2-pro", true, &["reference"][..])] {
            let (extras, refs) = ask(audio, roles);
            let err = p.generate(&ref_schema(id), &make_base("x", id), &extras, &refs).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{id} {roles:?}: {err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing sent");
        for (id, audio, roles) in [
            ("vidu/viduq1", false, &none[..]),
            ("vidu/viduq1", false, &["first_frame", "last_frame"][..]),
            ("vidu/viduq2-pro", true, &["first_frame"][..]),
            ("vidu/viduq3-turbo", true, &["first_frame", "last_frame"][..]),
        ] {
            let (extras, refs) = ask(audio, roles);
            p.generate(&ref_schema(id), &make_base("x", id), &extras, &refs).await.unwrap_or_else(|e| panic!("{id} {roles:?}: {e:?}"));
        }
        let sent: Vec<Value> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap()["audio"].clone())
            .collect();
        assert_eq!(sent, [json!(false), Value::Null, json!(true), json!(true)]);
    }

    /// The models carried since 2026-11-03 send `audio` only where their own
    /// operation has the switch: viduq3 reference2video, viduq3-pro-fast,
    /// viduq2-turbo and viduq1-classic img2video, viduq2 text2video; viduq3-mix
    /// has none, nor does start-end2video for viduq2-turbo or viduq1-classic.
    #[tokio::test]
    async fn the_newly_carried_models_send_audio_only_where_their_endpoint_has_it() {
        let server = MockServer::start().await;
        for e in ["text2video", "start-end2video", "img2video", "reference2video"] {
            mount_submit(&server, e).await;
        }
        let p = make_provider(&server.uri());
        let ask = |audio: bool, roles: &[&str]| {
            let mut extras = make_extras();
            extras.audio = Some(audio);
            let refs = roles.iter().map(|r| url_ref(r, "https://img.test/a.png")).collect();
            (extras, MaterializedRequest { refs, cleanup: Cleanup::empty() })
        };
        for (id, roles) in [
            ("vidu/viduq3-mix", &["reference"][..]),
            ("vidu/viduq2-turbo", &["first_frame", "last_frame"][..]),
            ("vidu/viduq1-classic", &["first_frame", "last_frame"][..]),
        ] {
            let (extras, refs) = ask(true, roles);
            let err = p.generate(&ref_schema(id), &make_base("x", id), &extras, &refs).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{id} {roles:?}: {err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing sent");
        for (id, roles, endpoint) in [
            ("vidu/viduq3", &["reference"][..], "/ent/v2/reference2video"),
            ("vidu/viduq3-pro-fast", &["first_frame"][..], "/ent/v2/img2video"),
            ("vidu/viduq2", &[][..], "/ent/v2/text2video"),
            ("vidu/viduq2-turbo", &["first_frame"][..], "/ent/v2/img2video"),
            ("vidu/viduq1-classic", &["first_frame"][..], "/ent/v2/img2video"),
        ] {
            let (extras, refs) = ask(true, roles);
            p.generate(&ref_schema(id), &make_base("x", id), &extras, &refs).await.unwrap_or_else(|e| panic!("{id}: {e:?}"));
            let last = server.received_requests().await.unwrap().pop().unwrap();
            let body: Value = serde_json::from_slice(&last.body).unwrap();
            assert_eq!((last.url.path(), body["audio"].as_bool()), (endpoint, Some(true)), "{id}: {body}");
        }
    }

    /// Q4 Preview: "audio … Default: `true`" on img2video and reference2video
    /// alike, so the switch goes on both; only reference2video takes an aspect
    /// ratio ("Only 1 image is accepted" on img2video, which follows it).
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q4-preview/image-to-video.md> (2026-11-03)
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q4-preview/reference-to-video.md> (2026-11-03)
    #[tokio::test]
    async fn q4_preview_sends_its_audio_switch_on_both_endpoints() {
        let server = MockServer::start().await;
        for e in ["img2video", "reference2video"] {
            mount_submit(&server, e).await;
        }
        let p = make_provider(&server.uri());
        let id = "vidu/viduq4-preview";
        for roles in [&["first_frame"][..], &["reference", "reference"][..]] {
            let mut extras = make_extras();
            extras.audio = Some(false);
            let refs = MaterializedRequest { refs: roles.iter().map(|r| url_ref(r, "https://img.test/a.png")).collect(), cleanup: Cleanup::empty() };
            p.generate(&ref_schema(id), &make_base("x", id), &extras, &refs).await.unwrap_or_else(|e| panic!("{roles:?}: {e:?}"));
        }
        let sent: Vec<(String, Value)> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| (r.url.path().to_string(), serde_json::from_slice::<Value>(&r.body).unwrap()))
            .collect();
        assert_eq!((sent[0].0.as_str(), sent[1].0.as_str()), ("/ent/v2/img2video", "/ent/v2/reference2video"));
        for (path, body) in &sent {
            assert_eq!((body["audio"].as_bool(), body["model"].as_str()), (Some(false), Some("viduq4-preview")), "{path}: {body}");
        }
        assert!(sent[0].1.get("aspect_ratio").is_none(), "img2video follows its image");
        assert_eq!(sent[1].1["aspect_ratio"], "16:9");
    }

    /// "seed … Manual setting uses the specified seed": the canonical seed goes
    /// as Vidu's own `seed`, and `extra` cannot replace it.
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/text-to-video.md> (2026-11-03)
    #[tokio::test]
    async fn a_seed_goes_as_vidus_seed() {
        let server = MockServer::start().await;
        mount_submit(&server, "text2video").await;
        let mut base = make_base("a lighthouse", "vidu/viduq3-turbo");
        base.seed = Some(1234);
        let mut extras = make_extras();
        extras.extra = Some(json!({ "seed": 1 }));
        make_provider(&server.uri())
            .generate(&ref_schema("vidu/viduq3-turbo"), &base, &extras, &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() })
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body["seed"], 1234, "{body}");
    }

    // ─── ByteDance/MiniMax/Vidu audit, 2026-10-30: W3, W9, W10, W13, W15 ────

    fn b64_ref(role: &str, bytes: &[u8]) -> MaterializedRef {
        use base64::Engine;
        MaterializedRef { role: role.to_string(), form: MaterializedRefForm::Base64(base64::engine::general_purpose::STANDARD.encode(bytes)), time_s: None }
    }

    const JPEG: [u8; 12] = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0, 1];

    /// W3: a last frame alone would be sent to img2video, which uses it as
    /// the FIRST frame — refused, with nothing sent.
    #[tokio::test]
    async fn a_last_frame_without_a_first_is_refused_before_any_call() {
        let server = MockServer::start().await;
        let materialized = MaterializedRequest { refs: vec![url_ref("last_frame", "https://x/end.png")], cleanup: Cleanup::empty() };
        let err = make_provider(&server.uri())
            .generate(&ref_schema("vidu/viduq3-pro"), &make_base("x", "vidu/viduq3-pro"), &make_extras(), &materialized)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// W10, W13, W9: img2video takes no `aspect_ratio`; a base64 frame goes
    /// as a data URI of its own type; `extra` cannot replace the model.
    #[tokio::test]
    async fn an_image_request_sends_only_what_its_endpoint_takes() {
        let server = MockServer::start().await;
        mount_submit(&server, "img2video").await;
        let mut extras = make_extras();
        extras.extra = Some(json!({ "model": "viduq3-turbo", "callback_url": "https://cb.example", "payload": "p1" }));
        let materialized = MaterializedRequest { refs: vec![b64_ref("first_frame", &JPEG)], cleanup: Cleanup::empty() };
        make_provider(&server.uri())
            .generate(&ref_schema("vidu/viduq3-pro"), &make_base("x", "vidu/viduq3-pro"), &extras, &materialized)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert!(body.get("aspect_ratio").is_none(), "{body}");
        assert!(body["images"][0].as_str().unwrap().starts_with("data:image/jpeg;base64,"), "{body}");
        assert_eq!((body["model"].as_str(), body["payload"].as_str()), (Some("viduq3-pro"), Some("p1")), "{body}");
        // E4a: where the result goes is litegen's (it polls, then fires the
        // key's webhook), so Vidu's `callback_url` is never sent.
        assert!(body.get("callback_url").is_none(), "{body}");
    }

    /// W15: a finished job reads 100, and a success with no URL is a failure.
    #[tokio::test]
    async fn a_success_without_a_url_fails_and_a_finished_job_reads_100() {
        for (answer, status, progress) in [
            (json!({ "state": "success", "creations": [] }), GenerationStatus::Failed, 100),
            (json!({ "state": "failed", "err_code": "CreationPolicyViolation" }), GenerationStatus::Failed, 100),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(answer.clone())).mount(&server).await;
            let handle = VideoGenerationHandle { provider_job_id: "vd-9".into(), provider: "vidu".into(), model: "vidu/viduq3-pro".into() };
            let poll = make_provider(&server.uri()).poll_status(&handle).await.unwrap();
            assert_eq!((poll.status, poll.progress), (status, progress), "{answer}");
            assert!(poll.error.is_some(), "{answer}");
        }
    }

    /// W14: a submit that timed out may still run, and bill, at the vendor:
    /// it is never sent again.
    #[tokio::test]
    async fn a_timed_out_submit_is_never_sent_again() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})).set_delay(std::time::Duration::from_secs(5)))
            .mount(&server)
            .await;
        let mut p = make_provider(&server.uri());
        p.client = reqwest::Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        let err = p.generate(&ref_schema("vidu/viduq1"), &make_base("x", "vidu/viduq1"), &make_extras(), &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
    }

    /// W14: a gateway's 5xx page is not JSON and a 429 turned the call away;
    /// both are judged by their status before any parse and may be sent again.
    #[tokio::test]
    async fn a_gateway_page_or_a_rate_limit_may_be_sent_again() {
        for (status, body) in [(502, "<html>bad gateway</html>"), (429, "{\"code\":\"rate_limited\"}")] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).respond_with(ResponseTemplate::new(status).set_body_string(body)).mount(&server).await;
            let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
            let err = make_provider(&server.uri())
                .generate(&ref_schema("vidu/viduq3-pro"), &make_base("x", "vidu/viduq3-pro"), &make_extras(), &materialized)
                .await
                .unwrap_err();
            assert!(err.is_retryable(), "{status}: {err:?}");
        }
    }

    /// A request that names no resolution runs at the row's default (720p),
    /// which is what it was priced at — never at a default Vidu picks.
    #[tokio::test]
    async fn an_unnamed_resolution_is_sent_as_the_rows_default() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": "t1", "state": "created" })))
            .mount(&server)
            .await;
        let mut extras = make_extras();
        extras.resolution = None;
        let materialized = MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "first_frame".into(),
                form: MaterializedRefForm::Url("https://x/cat.png".into()),
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };
        make_provider(&server.uri())
            .generate(&ref_schema("vidu/viduq2-pro"), &make_base("a cat", "vidu/viduq2-pro"), &extras, &materialized)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body["resolution"], "720p", "{body}");
    }

    /// On reference2video Vidu documents viduq3-turbo only as "Subjects-Reference
    /// to Video" (`subjects` "Required"); the `images` form this adapter sends
    /// names `viduq3` alone ("Accepted values: `viduq3`"). A viduq3-turbo
    /// reference is refused before anything is sent.
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/reference-to-video.md> (2026-11-09)
    #[tokio::test]
    async fn viduq3_turbo_refuses_reference_images_before_any_call() {
        let server = MockServer::start().await;
        mount_submit(&server, "reference2video").await;
        let refs = MaterializedRequest { refs: vec![url_ref("reference", "https://img.test/a.png")], cleanup: Cleanup::empty() };
        let err = make_provider(&server.uri())
            .generate(&ref_schema("vidu/viduq3-turbo"), &make_base("x", "vidu/viduq3-turbo"), &make_extras(), &refs)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing sent");
    }

    /// viduq1's reference2video `images` form has no `audio` row (only the
    /// subjects form does), so `audio: true` there is refused before anything
    /// is sent and `audio: false` sends nothing.
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/reference-to-video.md> (2026-11-09)
    #[tokio::test]
    async fn viduq1_reference_images_send_no_audio_switch() {
        let server = MockServer::start().await;
        mount_submit(&server, "reference2video").await;
        let p = make_provider(&server.uri());
        let ask = |audio: bool| {
            let mut extras = make_extras();
            extras.audio = Some(audio);
            (extras, MaterializedRequest { refs: vec![url_ref("reference", "https://img.test/a.png")], cleanup: Cleanup::empty() })
        };
        let (extras, refs) = ask(true);
        let err = p.generate(&ref_schema("vidu/viduq1"), &make_base("x", "vidu/viduq1"), &extras, &refs).await.unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing sent");
        let (extras, refs) = ask(false);
        p.generate(&ref_schema("vidu/viduq1"), &make_base("x", "vidu/viduq1"), &extras, &refs).await.expect("generate");
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert!(body.get("audio").is_none(), "{body}");
        assert_eq!(body["images"], json!(["https://img.test/a.png"]), "{body}");
    }

    /// viduq1's text2video body has neither `movement_amplitude` nor `bgm`;
    /// its start-end2video and reference2video bodies have both. The row
    /// allowlists them, so text2video drops them and the others keep them.
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/text-to-video.md> (2026-11-09)
    /// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/start-end-to-video.md> (2026-11-09)
    #[tokio::test]
    async fn viduq1_text2video_sends_no_movement_amplitude_or_bgm() {
        let server = MockServer::start().await;
        for e in ["text2video", "start-end2video"] {
            mount_submit(&server, e).await;
        }
        let p = make_provider(&server.uri());
        let mut extras = make_extras();
        extras.extra = Some(json!({ "movement_amplitude": "large", "bgm": true, "payload": "p", "callback_url": "https://hooks.test/v" }));
        let none = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
        let frames = MaterializedRequest {
            refs: vec![url_ref("first_frame", "https://img.test/s.png"), url_ref("last_frame", "https://img.test/e.png")],
            cleanup: Cleanup::empty(),
        };
        let schema = ref_schema("vidu/viduq1");
        let base = make_base("x", "vidu/viduq1");
        p.generate(&schema, &base, &extras, &none).await.expect("text2video");
        p.generate(&schema, &base, &extras, &frames).await.expect("start-end2video");
        let bodies: Vec<Value> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap())
            .collect();
        assert!(bodies[0].get("movement_amplitude").is_none() && bodies[0].get("bgm").is_none(), "text2video: {}", bodies[0]);
        assert_eq!(bodies[0]["payload"], "p", "text2video keeps payload");
        // E4a: neither endpoint sends Vidu's `callback_url`.
        assert!(bodies.iter().all(|b| b.get("callback_url").is_none()), "{bodies:?}");
        assert_eq!((bodies[1]["movement_amplitude"].as_str(), bodies[1]["bgm"].as_bool()), (Some("large"), Some(true)), "start-end2video: {}", bodies[1]);
    }
}
