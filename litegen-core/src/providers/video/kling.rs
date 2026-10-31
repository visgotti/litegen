use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    BaseGenerationRequest, CredentialPool, HealthCheckResult, ProviderError, ProviderInstanceConfig, VideoExtras,
    VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// Kling (Kuaishou) video generation provider.
///
/// Async: `POST /v1/videos/text2video` or `/v1/videos/image2video` returns
/// `data.task_id`; poll `GET /v1/videos/{kind}/{task_id}` until
/// `data.task_status == "succeed"`, then read `data.task_result.videos[0].url`.
/// Auth: per-request HS256 JWT ([`AuthSpec::KlingJwt`]). The job handle encodes
/// `{kind}/{task_id}` so polling targets the right sub-path.
///
/// @see <https://kling.ai/document-api/api/video/2-6/image-to-video.md> (2026-10-30)
///   ("`settings.duration` | int | No | `5` | `5`, `10`"). That page documents
///   Kling's newer API; the legacy `/v1/videos/{text2video,image2video}` pages
///   this adapter was written from (first frame `image`, last frame
///   `image_tail`, mode std|pro) are no longer published, though the endpoints
///   still answer.
pub struct KlingVideoProvider {
    config: Option<ProviderInstanceConfig>,
    cred_pool: Option<CredentialPool>,
    auth: AuthSpec,
    client: Client,
}

impl KlingVideoProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            cred_pool: None,
            auth: AuthSpec::KlingJwt,
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
            // "The API endpoint has been changed from https://api.klingai.com to
            // https://api-singapore.klingai.com."
            // @see <https://kling.ai/document-api/api/get-started/authentication.md>
            .unwrap_or("https://api-singapore.klingai.com")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("kling".into()))?;
        if let Some(pool) = &self.cred_pool {
            return Ok(base.with_signing(pool.next()));
        }
        Ok(base)
    }

    fn resolve_model(model_id: &str) -> &str {
        // Video model ids carry a `video-` prefix to stay distinct from image
        // model ids that share a model_name (e.g. kling-v2); strip both.
        let s = model_id.strip_prefix("kling/").unwrap_or(model_id);
        s.strip_prefix("video-").unwrap_or(s)
    }

    fn ref_image(form: &MaterializedRefForm) -> Option<String> {
        match form {
            MaterializedRefForm::Base64(b64) => Some(b64.clone()),
            MaterializedRefForm::Url(u) => Some(u.clone()),
            _ => None,
        }
    }
}

/// Read a Kling reply whole: its HTTP status and body `code` together. A
/// request Kling turned away before accepting it — a rate limit (1302-1304)
/// or a server error (5000-5002) — may be sent again; any other `code` is a
/// verdict on the request (bad key 1000-1004, task not found 1203, …).
///
/// @see <https://kling.ai/document-api/api/get-started/error-codes.md> — "429 | 1302 | … Too many
///   requests; rate limit exceeded", "500 | 5000 | … Server internal error"
pub(crate) async fn read_kling(resp: reqwest::Response, what: &str) -> Result<Value, ProviderError> {
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(|e| ProviderError::RequestFailed {
        message: format!("Kling {what} body could not be read: {e}"),
        status_code: Some(status),
        provider_error: None,
        retryable: true,
    })?;
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let code = body["code"].as_i64();
    if status < 300 && code == Some(0) {
        return Ok(body);
    }
    let retryable = match code {
        Some(c) => matches!(c, 1302..=1304 | 5000..=5002),
        None => status >= 500 || status == 429 || status == 408,
    };
    let why = body["message"].as_str().map(str::to_string).unwrap_or_else(|| text.chars().take(200).collect());
    Err(ProviderError::RequestFailed {
        message: format!("Kling {what} answered HTTP {status}{}: {why}", code.map(|c| format!(", code {c}")).unwrap_or_default()),
        status_code: Some(status),
        provider_error: (!body.is_null()).then_some(body),
        retryable,
    })
}

impl Default for KlingVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for KlingVideoProvider {
    fn name(&self) -> &str {
        "kling"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.credentials.credential_sets.is_empty() {
            self.cred_pool = Some(CredentialPool::shared(config.credentials.credential_sets.clone()));
        }
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.cred_pool.is_some())
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

        // first frame -> image2video; otherwise text2video.
        let mut first: Option<String> = None;
        let mut tail: Option<String> = None;
        for r in &materialized.refs {
            if let Some(img) = Self::ref_image(&r.form) {
                if r.role == "last_frame" {
                    tail = Some(img);
                } else {
                    first = Some(img);
                }
            }
        }
        // What Kling cannot make is refused before any call (it would bill,
        // or reject, a request litegen could tell was wrong).
        // @see <https://kling.ai/document-api/guides/capability-map/video.md> — "First/Last Frame … Kling
        //   2.6: Supported: Only supports 1080P silent videos | Kling 2.5 Turbo: Supported: Only
        //   supports 1080P"; "| Kling 2.6 | … | 5s, 10s |", "| Kling 2.5 Turbo | … | 5s, 10s |"
        let refuse = |why: String| Err(ProviderError::InvalidRequest(format!("{}: {why}", model.id)));
        if tail.is_some() && first.is_none() {
            return refuse("a last_frame needs a first_frame".into());
        }
        let extra = extras.extra.as_ref();
        let mode_pro = extra.and_then(|e| e["mode"].as_str()) == Some("pro");
        let audio_on = extras.audio == Some(true) || extra.and_then(|e| e["sound"].as_str()) == Some("on");
        if matches!(native, "kling-v2-5-turbo" | "kling-v2-6") {
            if tail.is_some() && !mode_pro {
                return refuse("first and last frames need mode pro (1080P)".into());
            }
            if ![5.0, 10.0].contains(&extras.duration_seconds) {
                return refuse(format!("{} s is not a length it makes (5 or 10 s)", extras.duration_seconds));
            }
        }
        if native == "kling-v2-6" && audio_on && (!mode_pro || tail.is_some()) {
            return refuse("native audio needs mode pro, and no last frame".into());
        }
        if extras.duration_seconds.fract() != 0.0 {
            return refuse(format!("{} s is not a whole number of seconds", extras.duration_seconds));
        }
        let kind = if first.is_some() { "image2video" } else { "text2video" };
        let url = format!("{}/v1/videos/{kind}", self.api_base());

        let mut body = json!({ "model_name": native, "prompt": base.prompt });
        if let Some(np) = base.negative_prompt.as_deref() {
            body["negative_prompt"] = Value::String(np.to_string());
        }
        if extras.duration_seconds > 0.0 {
            body["duration"] = Value::String((extras.duration_seconds as i64).to_string());
        }
        // Image-to-video takes the image's shape.
        if kind == "text2video" {
            if let Some(ar) = extras.aspect_ratio.as_deref() {
                body["aspect_ratio"] = Value::String(ar.to_string());
            }
        }
        if let Some(img) = first {
            body["image"] = Value::String(img);
        }
        if let Some(t) = tail {
            body["image_tail"] = Value::String(t);
        }
        // `extra` adds the endpoint's own fields; it cannot replace one litegen
        // sets (rule 11).
        if let (Some(Value::Object(map)), Some(obj)) = (&extras.extra, body.as_object_mut()) {
            for (k, v) in map.iter().filter(|(k, _)| !matches!(k.as_str(), "model_name" | "prompt" | "negative_prompt" | "duration" | "aspect_ratio" | "image" | "image_tail")) {
                obj.insert(k.clone(), v.clone());
            }
        }
        // PixelGenie L10: native audio is Kling's `sound`, on or off.
        if let Some(audio) = extras.audio {
            body["sound"] = Value::String(if audio { "on" } else { "off" }.to_string());
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
                message: format!("Kling video request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // A call that timed out may still run, and bill.
                retryable: e.is_connect(),
            })?;
        let submit = read_kling(resp, "video submit").await?;

        let task_id = submit["data"]["task_id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Kling response missing data.task_id".to_string(),
            status_code: None,
            provider_error: Some(submit.clone()),
            retryable: false,
        })?;

        Ok(VideoGenerationHandle {
            // Encode the sub-path so poll_status targets the right endpoint.
            provider_job_id: format!("{kind}/{task_id}"),
            provider: "kling".to_string(),
            model: model.id.clone(),
        })
    }

    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/v1/videos/{}", self.api_base(), handle.provider_job_id);
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
        // An error is an error, never a job still processing until the reaper.
        let data = read_kling(resp, "video poll").await?;

        let status = match data["data"]["task_status"].as_str() {
            Some("succeed") => GenerationStatus::Completed,
            Some("failed") => GenerationStatus::Failed,
            _ => GenerationStatus::Processing,
        };
        let video_url = data["data"]["task_result"]["videos"][0]["url"].as_str().map(String::from);

        Ok(VideoGenerationPollResult {
            status,
            progress: if status == GenerationStatus::Completed { 100 } else { 50 },
            video_url,
            video_data: None,
            content_type: Some("video/mp4".into()),
            error: data["data"]["task_status_msg"].as_str().filter(|_| status == GenerationStatus::Failed).map(String::from),
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
                message: "Kling video provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Kling video provider configured".into(), latency_ms: None }
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

    fn make_provider(api_base: &str) -> KlingVideoProvider {
        let mut p = KlingVideoProvider::new();
        let mut cfg = ProviderInstanceConfig { api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.key_id = Some("ak-test".to_string());
        cfg.credentials.key_secret = Some("sk-test".to_string());
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
        VideoExtras { duration_seconds: 5.0, aspect_ratio: Some("16:9".to_string()), resolution: None, fps: None, extra: None, camera: None, audio: None, r#loop: None, source: None, direction: None, time_range: None, replace: None }
    }

    #[tokio::test]
    async fn submits_text2video_with_jwt_and_polls() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/videos/text2video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0, "data": { "task_id": "kv-1", "task_status": "submitted" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/v1/videos/text2video/kv-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0, "data": { "task_status": "succeed",
                    "task_result": { "videos": [{ "id": "v1", "url": "https://cdn.kling/v.mp4" }] } }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("kling/video-kling-v2-6");
        let base = make_base("a phoenix rising", "kling/video-kling-v2-6");
        let mut extras = make_extras();
        extras.audio = Some(false);
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "text2video/kv-1");

        let received = server.received_requests().await.unwrap();
        let auth = received[0].headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("Bearer ey"), "expected JWT bearer, got {auth}");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["sound"], "off", "PixelGenie L10: audio false");

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_url.unwrap(), "https://cdn.kling/v.mp4");
    }

    // ─── Kling audit, 2026-10-30: K1, K3–K7, K12 ────────────────────────────

    fn frame(role: &str) -> crate::proxy::materializer::MaterializedRef {
        crate::proxy::materializer::MaterializedRef { role: role.into(), form: MaterializedRefForm::Url(format!("https://x/{role}.png")), time_s: None }
    }

    fn refs(roles: &[&str]) -> MaterializedRequest {
        MaterializedRequest { refs: roles.iter().map(|r| frame(r)).collect(), cleanup: Cleanup::empty() }
    }

    async fn accepting() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "data": { "task_id": "t1" } })))
            .mount(&server)
            .await;
        server
    }

    /// K1: "The API endpoint has been changed from https://api.klingai.com to
    /// https://api-singapore.klingai.com."
    #[test]
    fn the_default_host_is_kling_singapore() {
        let mut p = KlingVideoProvider::new();
        p.configure(ProviderInstanceConfig::default());
        assert_eq!(p.api_base(), "https://api-singapore.klingai.com");
    }

    /// K4, K5, K7: what Kling cannot make is refused before any call — a
    /// last frame alone, first and last frames (or native audio) on 2.5 Turbo
    /// and 2.6 outside mode pro, a length other than 5 or 10 s there, and a
    /// fractional length anywhere.
    #[tokio::test]
    async fn what_a_model_cannot_make_is_refused_before_any_call() {
        let server = accepting().await;
        let p = make_provider(&server.uri());
        let v26 = ref_schema("kling/video-kling-v2-6");
        let base = make_base("x", "kling/video-kling-v2-6");
        let mut seven = make_extras();
        seven.duration_seconds = 7.0;
        let mut half = make_extras();
        half.duration_seconds = 5.5;
        let mut pro_audio_frames = make_extras();
        pro_audio_frames.extra = Some(json!({ "mode": "pro" }));
        pro_audio_frames.audio = Some(true);
        let mut std_audio = make_extras();
        std_audio.audio = Some(true);
        for (extras, materialized) in [
            (make_extras(), refs(&["last_frame"])),
            (make_extras(), refs(&["first_frame", "last_frame"])),
            (pro_audio_frames, refs(&["first_frame", "last_frame"])),
            (std_audio, refs(&[])),
            (seven, refs(&[])),
            (half, refs(&[])),
        ] {
            let err = p.generate(&v26, &base, &extras, &materialized).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing sent");

        let mut pro = make_extras();
        pro.extra = Some(json!({ "mode": "pro" }));
        p.generate(&v26, &base, &pro, &refs(&["first_frame", "last_frame"])).await.unwrap();
    }

    /// K6: image-to-video takes the image's shape; only text-to-video gets
    /// an aspect ratio. `extra` cannot replace the model.
    #[tokio::test]
    async fn only_text_to_video_gets_an_aspect_ratio() {
        let server = accepting().await;
        let mut extras = make_extras();
        extras.extra = Some(json!({ "model_name": "kling-v3" }));
        make_provider(&server.uri())
            .generate(&ref_schema("kling/video-kling-v2-6"), &make_base("x", "kling/video-kling-v2-6"), &extras, &refs(&["first_frame"]))
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert!(body.get("aspect_ratio").is_none(), "{body}");
        assert_eq!(body["model_name"], "kling-v2-6", "{body}");
    }

    /// K3: a poll error is an error — not a job still processing until the
    /// reaper — and says whether to ask again.
    #[tokio::test]
    async fn a_poll_error_is_an_error_that_says_whether_to_ask_again() {
        let handle = VideoGenerationHandle { provider_job_id: "text2video/t1".into(), provider: "kling".into(), model: "kling/video-kling-v2-6".into() };
        for (status, body, retryable) in [
            (401, json!({ "code": 1001, "message": "Authorization is empty" }), false),
            (200, json!({ "code": 1203, "message": "task not found" }), false),
            (429, json!({ "code": 1302, "message": "rate limited" }), true),
            (500, json!({ "code": 5000, "message": "internal error" }), true),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET")).respond_with(ResponseTemplate::new(status).set_body_json(body.clone())).mount(&server).await;
            let got = make_provider(&server.uri()).poll_status(&handle).await;
            match got {
                Err(ProviderError::RequestFailed { retryable: r, .. }) => assert_eq!(r, retryable, "{body}"),
                other => panic!("{body}: {other:?}"),
            }
        }
    }

    /// K12: a submit Kling turned away (rate limit, server error) was never
    /// accepted, so it may be sent again.
    #[tokio::test]
    async fn a_rate_limited_submit_may_be_sent_again() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({ "code": 1303, "message": "concurrency exceeded" })))
            .mount(&server)
            .await;
        let err = make_provider(&server.uri())
            .generate(&ref_schema("kling/video-kling-v2-6"), &make_base("x", "kling/video-kling-v2-6"), &make_extras(), &refs(&[]))
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: true, .. }), "{err:?}");
    }
}
