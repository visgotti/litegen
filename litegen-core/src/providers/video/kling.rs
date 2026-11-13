use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig, VideoExtras,
    VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// Kling (Kuaishou) video generation provider, on Kling's current API design:
/// the model in the path, an API key as the credential.
///
/// Submit: `POST /text-to-video/<model>` (`prompt`, `settings`) or
/// `POST /image-to-video/<model>` (`contents`: the prompt and the frames,
/// `settings`); the task is the answer's `data.id`. Poll:
/// `GET /tasks?task_ids=<id>` until `data[0].status` is `succeeded`, then read
/// the `outputs[]` entry of `type: video`. Auth: `Authorization: Bearer <API
/// key>` ([`AuthSpec::bearer`]) from the dashboard's `api_keys` pool. The
/// handle is the plain task id; a `text2video/<id>` or `image2video/<id>`
/// handle from a job submitted on the legacy design before 2026-11-03 is
/// still asked after on the legacy path.
///
/// @see <https://kling.ai/document-api/api/get-started/authentication.md> (2026-11-03)
///   Verbatim: "API Key (for all models) … API Key authentication must be used to call Kling AI API"
///   Verbatim: "Access Key / Secret Key (API only applicable to legacy version design standards)"
/// @see <https://kling.ai/document-api/api/video/3-0-omni/text-to-video.md> (2026-11-03)
///   Verbatim: "- Method: `POST` - Path: `/text-to-video/kling-3.0` - Auth: `Authorization: Bearer <API_KEY>`"
///   Verbatim (Query Task): "- Method: `GET` - Path: `/tasks`", "`task_ids` … The task ID defined by the
///   system that needs to be queried", "Task status, Enum values: submitted, processing, succeeded, failed"
/// @see <https://kling.ai/document-api/api/video/3-0-omni/image-to-video.md> (2026-11-03)
///   Verbatim: "`contents[].type` … `prompt`, `first_frame`, `last_frame`, `element`"
/// The error for a Kling credential without an API key. "API Key
/// authentication must be used to call Kling AI API"; an access / secret key
/// pair is "only applicable to legacy version design standards", so a
/// credential that is only such a pair (an org's stored pair, from before
/// 2026-11-03) says what to store instead. Both Kling adapters use it.
/// @see <https://kling.ai/document-api/api/get-started/authentication.md> (2026-11-03)
pub(crate) fn no_api_key(creds: &ProviderCredentials) -> ProviderError {
    let pair = creds.key_id.is_some() || creds.key_secret.is_some() || !creds.credential_sets.is_empty();
    ProviderError::NotConfigured(if pair {
        "kling: this credential is an access/secret key pair, which works only on Kling's legacy API; store a Kling API key instead".into()
    } else {
        "kling: no API key".into()
    })
}

pub struct KlingVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

/// The path model of each catalog row on the current design (rule 9: one arm
/// per row, never a guess).
/// @see <https://kling.ai/document-api/api/video/2-5-turbo/text-to-video.md> (2026-11-03) — "Path: `/text-to-video/kling-2.5-turbo`"
/// @see <https://kling.ai/document-api/api/video/2-6/text-to-video.md> (2026-11-03) — "Path: `/text-to-video/kling-2.6`"
/// @see <https://kling.ai/document-api/api/video/3-0-omni/text-to-video.md> (2026-11-03) — "Path: `/text-to-video/kling-3.0`"
/// @see <https://kling.ai/document-api/api/video/3-0-turbo/text-to-video.md> (2026-11-03) — "Path: `/text-to-video/kling-3.0-turbo`"
/// @see <https://kling.ai/document-api/api/video/4-0/text-to-video.md> (2026-11-03) — "Path: `/text-to-video/kling-4.0`"
pub(crate) fn path_model(model_id: &str) -> Option<&'static str> {
    match model_id {
        "kling/video-kling-v2-5-turbo" => Some("kling-2.5-turbo"),
        "kling/video-kling-v2-6" => Some("kling-2.6"),
        "kling/video-kling-v3" => Some("kling-3.0"),
        "kling/video-kling-v3-turbo" => Some("kling-3.0-turbo"),
        "kling/video-kling-v4" => Some("kling-4.0"),
        _ => None,
    }
}

impl KlingVideoProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (result downloads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(120), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
            client: Self::api_client(&Default::default()),
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
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        if base.api_key.as_deref().is_some_and(|k| !k.is_empty()) {
            return Ok(base);
        }
        Err(no_api_key(&base))
    }

    async fn get(&self, url: &str, what: &str) -> Result<Value, ProviderError> {
        let creds = self.creds()?;
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(url))?;
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
        read_kling(resp, what).await
    }

    /// A job submitted on the legacy design before 2026-11-03
    /// (`GET /v1/videos/{kind}/{task_id}`; task_status submitted |
    /// processing | succeed | failed). Kling's API key is "for all models".
    async fn poll_legacy(&self, handle: &str) -> Result<VideoGenerationPollResult, ProviderError> {
        let data = self.get(&format!("{}/v1/videos/{handle}", self.api_base()), "video poll").await?;
        let video_url = data["data"]["task_result"]["videos"][0]["url"].as_str().map(String::from);
        let status = match (data["data"]["task_status"].as_str(), &video_url) {
            (Some("succeed"), Some(_)) => GenerationStatus::Completed,
            (Some("succeed"), None) => return Ok(finished_without_a_video(&data)),
            (Some("failed"), _) => GenerationStatus::Failed,
            (Some("submitted"), _) => GenerationStatus::Pending,
            (Some("processing"), _) => GenerationStatus::Processing,
            (other, _) => return Ok(super::unknown_state("kling", other)),
        };
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
}

/// The first and last frames of a request, as Kling's `contents[].url` takes
/// them: "The content of the input can be provided via URL or Base64" — raw
/// base64 on Kling's own API, a data URI where a reseller asks for one
/// (TokenHub: "Base64：… 格式要求：data:image/<图片格式>;base64").
pub(crate) fn kling_frames(materialized: &MaterializedRequest, data_uri: bool) -> (Option<String>, Option<String>) {
    let mut first = None;
    let mut tail = None;
    for r in &materialized.refs {
        let img = match &r.form {
            MaterializedRefForm::Base64(b64) if data_uri => crate::proxy::materializer::base64_data_uri(b64, Some("image/png")),
            MaterializedRefForm::Base64(b64) => b64.clone(),
            MaterializedRefForm::Url(u) => u.clone(),
            MaterializedRefForm::MultipartField { .. } => continue,
        };
        if r.role == "last_frame" {
            tail = Some(img);
        } else {
            first = Some(img);
        }
    }
    (first, tail)
}

/// Kling's `settings`: "`settings.duration` | int", "`settings.resolution` …
/// `720p`, `1080p`[, `4k`]", "`settings.aspect_ratio`" (text-to-video only:
/// image-to-video takes the frames' shape), "`settings.audio` … `native`,
/// `off`" (only the rows that declare `audio`), and the endpoint's own
/// settings from `extra` (Kling 3.0's `multi_shot`), which cannot replace one
/// litegen sets (rule 11). The same shape on Kling's API and TokenHub's.
pub(crate) fn kling_settings(model: &ModelSchema, extras: &VideoExtras, image: bool) -> Value {
    let mut settings = json!({ "duration": extras.duration_seconds as i64 });
    if let (false, Some(ar)) = (image, extras.aspect_ratio.as_deref()) {
        settings["aspect_ratio"] = Value::String(ar.to_string());
    }
    // The row's default when none is asked (E10): the clip is made at the
    // resolution it was quoted and charged at, not at Kling's own default.
    if let Some(res) = extras.resolution.as_deref().or(model.string_default("resolution")) {
        settings["resolution"] = Value::String(res.to_string());
    }
    // PixelGenie L10: native audio is Kling's `settings.audio`, native or off.
    if let Some(audio) = extras.audio {
        settings["audio"] = Value::String(if audio { "native" } else { "off" }.to_string());
    }
    if let Some(obj) = settings.as_object_mut() {
        let owned = |k: &str| matches!(k, "duration" | "aspect_ratio" | "resolution" | "audio");
        for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), owned) {
            obj.insert(k.clone(), v.clone());
        }
    }
    settings
}

/// Image-to-video's `contents`: the prompt and the frames, the first frame
/// before the last ("Required in the first_frame, optional in the last_frame").
pub(crate) fn kling_contents(prompt: &str, first: Option<String>, tail: Option<String>) -> Value {
    let mut contents = vec![json!({ "type": "prompt", "text": prompt })];
    contents.extend(first.map(|u| json!({ "type": "first_frame", "url": u })));
    contents.extend(tail.map(|u| json!({ "type": "last_frame", "url": u })));
    Value::Array(contents)
}

/// The poll answer of Kling's current design (`data[]`: "Task status, Enum
/// values: submitted, processing, succeeded, failed", `outputs[].url`), as a
/// job verdict. `vendor` names who answered (kling, or a reseller).
pub(crate) fn kling_task_result(data: &Value, job: &str, vendor: &str) -> Result<VideoGenerationPollResult, ProviderError> {
    // A task the answer does not list can never be learned: the poller reaps it.
    let tasks = data["data"].as_array().cloned().unwrap_or_default();
    let Some(task) = tasks.iter().find(|t| t["id"].as_str() == Some(job)).or(tasks.first()) else {
        return Err(ProviderError::RequestFailed {
            message: format!("{vendor} knows no Kling task {job}"),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        });
    };
    let video_url = task["outputs"]
        .as_array()
        .and_then(|outs| outs.iter().find(|o| o["type"] == "video"))
        .and_then(|o| o["url"].as_str())
        .map(String::from);
    let status = match (task["status"].as_str(), &video_url) {
        (Some("succeeded"), Some(_)) => GenerationStatus::Completed,
        (Some("succeeded"), None) => return Ok(finished_without_a_video(task)),
        (Some("failed"), _) => GenerationStatus::Failed,
        (Some("submitted"), _) => GenerationStatus::Pending,
        (Some("processing"), _) => GenerationStatus::Processing,
        (other, _) => return Ok(super::unknown_state(vendor, other)),
    };
    Ok(VideoGenerationPollResult {
        status,
        progress: if status == GenerationStatus::Completed { 100 } else { 50 },
        video_url,
        video_data: None,
        content_type: Some("video/mp4".into()),
        // "Task status information, displaying the failure reason when the task fails".
        error: task["message"].as_str().filter(|_| status == GenerationStatus::Failed).map(String::from),
        metadata: HashMap::new(),
    })
}

/// A task Kling calls finished but that names no video: the job ends, with
/// Kling's answer, instead of a completed row with no result.
fn finished_without_a_video(answer: &Value) -> VideoGenerationPollResult {
    VideoGenerationPollResult {
        status: GenerationStatus::Failed,
        progress: 100,
        video_url: None,
        video_data: None,
        content_type: None,
        error: Some(format!("Kling finished the task with no video: {}", answer.to_string().chars().take(300).collect::<String>())),
        metadata: HashMap::new(),
    }
}

/// Read a Kling reply whole: its HTTP status and body `code` together. A
/// request Kling turned away before accepting it — a rate limit (1302-1304)
/// or a server error (5000-5002) — may be sent again; any other `code` is a
/// verdict on the request (bad key 1000-1004, task not found 1203, …).
///
/// @see <https://kling.ai/document-api/api/get-started/error-codes.md> (2026-11-03) — "429 | 1302 | … Too many
///   requests; rate limit exceeded", "500 | 5000 | … Server internal error"
pub(crate) async fn read_kling(resp: reqwest::Response, what: &str) -> Result<Value, ProviderError> {
    let status = resp.status().as_u16();
    let retry_after = crate::providers::retry_after_ms(resp.headers());
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
    let message = format!("Kling {what} answered HTTP {status}{}: {why}", code.map(|c| format!(", code {c}")).unwrap_or_default());
    if let Some(e) = reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|s| crate::providers::rate_limited("kling", s, retry_after, &message, (!body.is_null()).then_some(&body)))
    {
        return Err(e);
    }
    Err(ProviderError::RequestFailed {
        message,
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
        let native = path_model(&model.id).ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "kling".into() })?;

        // The frames: a first frame means image-to-video; otherwise text-to-video.
        let (first, tail) = kling_frames(materialized, false);
        // What Kling cannot make is refused before any call (it would bill,
        // or reject, a request litegen could tell was wrong).
        // @see <https://kling.ai/document-api/guides/capability-map/video.md> (2026-11-03) — "First/Last Frame …
        //   Kling 2.6: Supported: Only supports 1080P silent videos | Kling 2.5 Turbo: Supported: Only
        //   supports 1080P"; "| Kling 2.6 | … | 5s, 10s |", "| Kling 2.5 Turbo | … | 5s, 10s |"
        // @see <https://kling.ai/document-api/api/video/2-6/text-to-video.md> (2026-11-03) — "When generating
        //   videos with native audio, only 1080P resolution is supported."
        let refuse = |why: String| Err(ProviderError::InvalidRequest(format!("{}: {why}", model.id)));
        if tail.is_some() && first.is_none() {
            return refuse("a last_frame needs a first_frame (last-frame-only video is not supported)".into());
        }
        let at_1080 = extras.resolution.as_deref() == Some("1080p");
        let audio_on = extras.audio == Some(true);
        if matches!(native, "kling-2.5-turbo" | "kling-2.6") {
            if tail.is_some() && !at_1080 {
                return refuse("first and last frames need resolution 1080p".into());
            }
            if ![5.0, 10.0].contains(&extras.duration_seconds) {
                return refuse(format!("{} s is not a length it makes (5 or 10 s)", extras.duration_seconds));
            }
        }
        if native == "kling-2.6" && audio_on && (!at_1080 || tail.is_some()) {
            return refuse("native audio needs resolution 1080p, and no last frame".into());
        }
        if extras.duration_seconds.fract() != 0.0 {
            return refuse(format!("{} s is not a whole number of seconds", extras.duration_seconds));
        }

        // `settings`: "`settings.duration` | int", "`settings.resolution` …
        // `720p`, `1080p`[, `4k`]", "`settings.aspect_ratio`" (text-to-video
        // only: image-to-video takes the frames' shape), "`settings.audio` …
        // `native`, `off`" (2.6 and 3.0; only their rows declare `audio`).
        let image = first.is_some();
        let settings = kling_settings(model, extras, image);

        let wire = crate::providers::wire_model(self.config.as_ref(), &model.id, native);
        let (url, body) = if image {
            // Image-to-video: the prompt and the frames are `contents` items,
            // the first frame before the last ("Required in the first_frame,
            // optional in the last_frame").
            let contents = kling_contents(&base.prompt, first, tail);
            (format!("{}/image-to-video/{wire}", self.api_base()), json!({ "contents": contents, "settings": settings }))
        } else {
            (format!("{}/text-to-video/{wire}", self.api_base()), json!({ "prompt": base.prompt, "settings": settings }))
        };

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

        // "data": { "id": "string", // The task ID generated by the system }
        let task_id = submit["data"]["id"].as_str().filter(|id| !id.is_empty()).ok_or_else(|| ProviderError::RequestFailed {
            message: "Kling response missing data.id".to_string(),
            status_code: None,
            provider_error: Some(submit.clone()),
            retryable: false,
        })?;

        Ok(VideoGenerationHandle {
            provider_job_id: task_id.to_string(),
            provider: "kling".to_string(),
            model: model.id.clone(),
        })
    }

    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let job = handle.provider_job_id.as_str();
        if job.starts_with("text2video/") || job.starts_with("image2video/") {
            return self.poll_legacy(job).await;
        }
        let url = reqwest::Url::parse_with_params(&format!("{}/tasks", self.api_base()), &[("task_ids", job)])
            .map_err(|e| ProviderError::RequestFailed { message: format!("Kling poll URL: {e}"), status_code: None, provider_error: None, retryable: false })?;
        let data = self.get(url.as_str(), "video poll").await?;
        kling_task_result(&data, job, "kling")
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
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    /// "API Key authentication must be used to call Kling AI API": the
    /// dashboard's pool (`api_keys`), as a BYO credential arrives.
    fn make_provider(api_base: &str) -> KlingVideoProvider {
        let mut p = KlingVideoProvider::new();
        let cfg = ProviderInstanceConfig {
            api_base: Some(api_base.to_string()),
            api_keys: vec![crate::types::ApiKeyEntry { key: "kling-key".into(), weight: 1, label: None }],
            ..Default::default()
        };
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

    fn frame(role: &str) -> crate::proxy::materializer::MaterializedRef {
        crate::proxy::materializer::MaterializedRef { role: role.into(), form: MaterializedRefForm::Url(format!("https://x/{role}.png")), time_s: None }
    }

    fn refs(roles: &[&str]) -> MaterializedRequest {
        MaterializedRequest { refs: roles.iter().map(|r| frame(r)).collect(), cleanup: Cleanup::empty() }
    }

    /// Kling accepts every submit with the current design's envelope.
    async fn accepting() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "data": { "id": "t1", "status": "submitted" } })))
            .mount(&server)
            .await;
        server
    }

    async fn bodies(server: &MockServer) -> Vec<(String, Value)> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| (r.url.path().to_string(), serde_json::from_slice::<Value>(&r.body).unwrap()))
            .collect()
    }

    // ─── Kling's current API design, 2026-11-03 ─────────────────────────────

    /// Text-to-video on the current design: "- Path: `/text-to-video/kling-2.6`
    /// - Auth: `Authorization: Bearer <API_KEY>`", a top-level `prompt` and a
    /// `settings` object; the task comes back as `data.id` and is asked after
    /// at "`GET` … `/tasks`" with `task_ids`, finishing as `succeeded` with the
    /// video in `outputs`.
    /// @see <https://kling.ai/document-api/api/video/2-6/text-to-video.md> (2026-11-03)
    #[tokio::test]
    async fn text_to_video_goes_to_the_models_path_with_the_api_key_and_polls_tasks() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/text-to-video/kling-2.6"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "data": { "id": "kv-1", "status": "submitted" } })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tasks"))
            .and(query_param("task_ids", "kv-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0, "data": [{ "id": "kv-1", "status": "succeeded",
                    "outputs": [{ "type": "video", "id": "v1", "url": "https://cdn.kling/v.mp4", "duration": "5" }] }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let id = "kling/video-kling-v2-6";
        let mut extras = make_extras();
        extras.audio = Some(false);
        let handle = provider.generate(&ref_schema(id), &make_base("a phoenix rising", id), &extras, &refs(&[])).await.unwrap();
        assert_eq!(handle.provider_job_id, "kv-1");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].headers.get("authorization").unwrap(), "Bearer kling-key");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(
            body,
            json!({ "prompt": "a phoenix rising", "settings": { "duration": 5, "aspect_ratio": "16:9", "resolution": "720p", "audio": "off" } }),
            "no model_name, mode, sound or negative_prompt: the current body has none; the row's resolution when none is asked (E10)"
        );

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!((poll.status, poll.video_url.as_deref(), poll.progress), (GenerationStatus::Completed, Some("https://cdn.kling/v.mp4"), 100));
    }

    /// Image-to-video: "`contents[].type` … `prompt`, `first_frame`,
    /// `last_frame`", each frame as `{type, url}` ("The content of the input
    /// can be provided via URL or Base64"), in that order whatever order the
    /// caller listed them; the frames give the shape, so no aspect_ratio.
    /// @see <https://kling.ai/document-api/api/video/3-0-omni/image-to-video.md> (2026-11-03)
    #[tokio::test]
    async fn image_to_video_sends_contents_with_the_first_frame_first() {
        let server = accepting().await;
        let id = "kling/video-kling-v3";
        let mut extras = make_extras();
        extras.resolution = Some("1080p".into());
        make_provider(&server.uri()).generate(&ref_schema(id), &make_base("a lantern drifts", id), &extras, &refs(&["last_frame", "first_frame"])).await.unwrap();
        let (route, body) = bodies(&server).await.remove(0);
        assert_eq!(route, "/image-to-video/kling-3.0");
        assert_eq!(
            body["contents"],
            json!([
                { "type": "prompt", "text": "a lantern drifts" },
                { "type": "first_frame", "url": "https://x/first_frame.png" },
                { "type": "last_frame", "url": "https://x/last_frame.png" },
            ]),
            "{body}"
        );
        assert_eq!(body["settings"], json!({ "duration": 5, "resolution": "1080p" }), "{body}");
        assert!(body.get("prompt").is_none(), "image-to-video's prompt is a contents item: {body}");
    }

    /// The canonical `resolution` goes as `settings.resolution` as it is
    /// ("`settings.resolution` … `720p`, `1080p`, `4k`"), `extra` cannot
    /// replace a setting litegen owns, and Kling 3.0's `multi_shot` ("Whether
    /// to generate multi-shot video") goes inside `settings`.
    /// @see <https://kling.ai/document-api/api/video/3-0-omni/text-to-video.md> (2026-11-03)
    #[tokio::test]
    async fn resolution_and_multi_shot_go_in_settings() {
        let server = accepting().await;
        let id = "kling/video-kling-v3";
        let mut extras = make_extras();
        extras.resolution = Some("4k".into());
        extras.extra = Some(json!({ "multi_shot": false, "resolution": "720p", "duration": 15 }));
        make_provider(&server.uri()).generate(&ref_schema(id), &make_base("x", id), &extras, &refs(&[])).await.unwrap();
        let (route, body) = bodies(&server).await.remove(0);
        assert_eq!(route, "/text-to-video/kling-3.0");
        assert_eq!(body["settings"], json!({ "duration": 5, "aspect_ratio": "16:9", "resolution": "4k", "multi_shot": false }), "{body}");
        assert!(body.get("multi_shot").is_none() && body.get("mode").is_none(), "{body}");
    }

    /// E10: a clip asked without a `resolution` is quoted and charged at the
    /// row's default (`params.resolution.default`), so Kling is sent that
    /// default instead of being left to pick its own ("`settings.resolution`
    /// | string | No | `720p`"), on text and image alike; 4.0's row says 1080p.
    /// @see <https://kling.ai/document-api/api/video/3-0-omni/text-to-video.md> (drift snapshot `kling/t2v-3-0.txt`, 2026-10-30)
    #[tokio::test]
    async fn an_unasked_resolution_is_sent_at_the_rows_default() {
        let server = accepting().await;
        let p = make_provider(&server.uri());
        let (v3, v4) = ("kling/video-kling-v3", "kling/video-kling-v4");
        p.generate(&ref_schema(v3), &make_base("x", v3), &make_extras(), &refs(&[])).await.unwrap();
        p.generate(&ref_schema(v3), &make_base("x", v3), &make_extras(), &refs(&["first_frame"])).await.unwrap();
        p.generate(&ref_schema(v4), &make_base("x", v4), &make_extras(), &refs(&[])).await.unwrap();
        let sent: Vec<Value> = bodies(&server).await.into_iter().map(|(_, b)| b["settings"]["resolution"].clone()).collect();
        assert_eq!(sent, [json!("720p"), json!("720p"), json!("1080p")]);
    }

    /// Kling 4.0 goes to its own path ("Path: `/text-to-video/kling-4.0`")
    /// with what only it takes — `21:9` and up to 30 s — and 3.0 Turbo to
    /// `/image-to-video/kling-3.0-turbo` with no audio switch.
    /// @see <https://kling.ai/document-api/api/video/4-0/text-to-video.md> (2026-11-03)
    /// @see <https://kling.ai/document-api/api/video/3-0-turbo/image-to-video.md> (2026-11-03)
    #[tokio::test]
    async fn the_new_models_go_to_their_own_paths() {
        let server = accepting().await;
        let p = make_provider(&server.uri());
        let mut wide = make_extras();
        wide.aspect_ratio = Some("21:9".into());
        wide.duration_seconds = 30.0;
        wide.resolution = Some("720p".into());
        p.generate(&ref_schema("kling/video-kling-v4"), &make_base("x", "kling/video-kling-v4"), &wide, &refs(&[])).await.unwrap();
        p.generate(&ref_schema("kling/video-kling-v3-turbo"), &make_base("x", "kling/video-kling-v3-turbo"), &make_extras(), &refs(&["first_frame"])).await.unwrap();
        let sent = bodies(&server).await;
        assert_eq!(sent[0].0, "/text-to-video/kling-4.0");
        assert_eq!(sent[0].1["settings"], json!({ "duration": 30, "aspect_ratio": "21:9", "resolution": "720p" }));
        assert_eq!(sent[1].0, "/image-to-video/kling-3.0-turbo");
        assert!(sent[1].1["settings"].get("audio").is_none(), "{}", sent[1].1);
    }

    /// Every Kling video row goes to its own path model ("Path:
    /// `/text-to-video/kling-2.6`"); an id outside the table is never guessed.
    #[test]
    fn every_kling_video_row_has_a_path_model() {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let reg = crate::capabilities::CapabilityRegistry::from_dir(&p).unwrap();
        let rows: Vec<_> = reg.for_provider("kling").filter(|m| m.media_type == crate::capabilities::MediaType::Video).collect();
        assert!(!rows.is_empty());
        for m in rows {
            let model = path_model(&m.id).unwrap_or_else(|| panic!("{} has no path model", m.id));
            assert!(model.starts_with("kling-"), "{} → {model}", m.id);
        }
        assert!(path_model("kling/video-kling-v9").is_none());
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
    /// last frame alone ("last-frame-only video generation is not
    /// supported"), first and last frames (or native audio) on 2.5 Turbo and
    /// 2.6 below 1080p ("First/Last Frame … Only supports 1080P"), a length
    /// other than 5 or 10 s there, and a fractional length anywhere.
    /// @see <https://kling.ai/document-api/guides/capability-map/video.md> (2026-11-03)
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
        let mut hd_audio_frames = make_extras();
        hd_audio_frames.resolution = Some("1080p".into());
        hd_audio_frames.audio = Some(true);
        let mut sd_audio = make_extras();
        sd_audio.audio = Some(true);
        for (extras, materialized) in [
            (make_extras(), refs(&["last_frame"])),
            (make_extras(), refs(&["first_frame", "last_frame"])),
            (hd_audio_frames, refs(&["first_frame", "last_frame"])),
            (sd_audio, refs(&[])),
            (seven, refs(&[])),
            (half, refs(&[])),
        ] {
            let err = p.generate(&v26, &base, &extras, &materialized).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing sent");

        let mut hd = make_extras();
        hd.resolution = Some("1080p".into());
        p.generate(&v26, &base, &hd, &refs(&["first_frame", "last_frame"])).await.unwrap();
    }

    /// K6: image-to-video takes the image's shape; only text-to-video gets an
    /// aspect ratio ("`settings.aspect_ratio`" is on the text-to-video body
    /// only), and `extra` cannot add one.
    /// @see <https://kling.ai/document-api/api/video/2-6/image-to-video.md> (2026-11-03)
    #[tokio::test]
    async fn only_text_to_video_gets_an_aspect_ratio() {
        let server = accepting().await;
        let mut extras = make_extras();
        extras.extra = Some(json!({ "aspect_ratio": "1:1" }));
        make_provider(&server.uri())
            .generate(&ref_schema("kling/video-kling-v2-6"), &make_base("x", "kling/video-kling-v2-6"), &extras, &refs(&["first_frame"]))
            .await
            .unwrap();
        let (route, body) = bodies(&server).await.remove(0);
        assert_eq!(route, "/image-to-video/kling-2.6");
        assert!(body["settings"].get("aspect_ratio").is_none(), "{body}");
    }

    /// "Task status, Enum values: submitted, processing, succeeded, failed":
    /// each maps, a success with no video fails (never a completed job with
    /// no result), a failure carries Kling's `message`, and any other state
    /// fails the job naming it.
    /// @see <https://kling.ai/document-api/api/video/3-0-omni/text-to-video.md> (Query Task, 2026-11-03)
    #[tokio::test]
    async fn every_documented_status_maps() {
        let handle = VideoGenerationHandle { provider_job_id: "t1".into(), provider: "kling".into(), model: "kling/video-kling-v3".into() };
        let task = |status: &str, extra: Value| {
            let mut t = json!({ "id": "t1", "status": status });
            if let (Some(t), Some(e)) = (t.as_object_mut(), extra.as_object()) {
                t.extend(e.clone());
            }
            json!({ "code": 0, "data": [t] })
        };
        for (answer, want, error) in [
            (task("submitted", json!({})), GenerationStatus::Pending, None),
            (task("processing", json!({})), GenerationStatus::Processing, None),
            (task("succeeded", json!({ "outputs": [{ "type": "video", "url": "https://cdn.kling/v.mp4" }] })), GenerationStatus::Completed, None),
            (task("succeeded", json!({ "outputs": [] })), GenerationStatus::Failed, Some("no video")),
            (task("failed", json!({ "message": "content risk control" })), GenerationStatus::Failed, Some("content risk control")),
            (task("sleeping", json!({})), GenerationStatus::Failed, Some("sleeping")),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET")).and(path("/tasks")).respond_with(ResponseTemplate::new(200).set_body_json(answer.clone())).mount(&server).await;
            let got = make_provider(&server.uri()).poll_status(&handle).await.unwrap_or_else(|e| panic!("{answer}: {e:?}"));
            assert_eq!(got.status, want, "{answer}");
            if let Some(why) = error {
                assert!(got.error.as_deref().unwrap_or("").contains(why), "{answer}: {:?}", got.error);
            }
        }
    }

    /// K3: a poll error is an error — not a job still processing until the
    /// reaper — and says whether to ask again; a task Kling does not know
    /// (an empty `data`) can never be learned.
    #[tokio::test]
    async fn a_poll_error_is_an_error_that_says_whether_to_ask_again() {
        let handle = VideoGenerationHandle { provider_job_id: "t1".into(), provider: "kling".into(), model: "kling/video-kling-v2-6".into() };
        for (status, body, retryable) in [
            (401, json!({ "code": 1001, "message": "Authorization is empty" }), false),
            (200, json!({ "code": 1203, "message": "task not found" }), false),
            (200, json!({ "code": 0, "data": [] }), false),
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

    /// A job submitted before the switch carries a legacy handle
    /// (`text2video/<task_id>`): it is still asked after on the legacy path
    /// (`GET /v1/videos/text2video/<id>`, `task_status` … `succeed`) with the
    /// same API key — Kling's API key is "for all models".
    /// @see <https://kling.ai/document-api/api/get-started/authentication.md> (2026-11-03)
    #[tokio::test]
    async fn a_legacy_handle_still_polls_on_the_legacy_path() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/videos/text2video/kv-0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0, "data": { "task_status": "succeed", "task_result": { "videos": [{ "id": "v0", "url": "https://cdn.kling/old.mp4" }] } }
            })))
            .mount(&server)
            .await;
        let handle = VideoGenerationHandle { provider_job_id: "text2video/kv-0".into(), provider: "kling".into(), model: "kling/video-kling-v2-6".into() };
        let got = make_provider(&server.uri()).poll_status(&handle).await.unwrap();
        assert_eq!((got.status, got.video_url.as_deref()), (GenerationStatus::Completed, Some("https://cdn.kling/old.mp4")));
        assert_eq!(server.received_requests().await.unwrap()[0].headers.get("authorization").unwrap(), "Bearer kling-key");
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

    /// An accepted submit with no task id is an error with the body attached,
    /// never a handle that polls nothing.
    #[tokio::test]
    async fn a_submit_without_a_task_id_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "data": {} }))).mount(&server).await;
        let err = make_provider(&server.uri())
            .generate(&ref_schema("kling/video-kling-v2-6"), &make_base("x", "kling/video-kling-v2-6"), &make_extras(), &refs(&[]))
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, provider_error: Some(_), .. }), "{err:?}");
    }

    /// "API Key authentication must be used to call Kling AI API"; an access /
    /// secret key pair is "only applicable to legacy version design
    /// standards". A credential that is only such a pair (an org's stored
    /// pair) is refused before any call, saying what to store instead.
    /// @see <https://kling.ai/document-api/api/get-started/authentication.md> (2026-11-03)
    #[tokio::test]
    async fn a_stored_key_pair_is_refused_naming_the_api_key_kling_needs() {
        let server = MockServer::start().await;
        let mut p = KlingVideoProvider::new();
        p.configure(ProviderInstanceConfig {
            api_base: Some(server.uri()),
            credentials: ProviderCredentials { key_id: Some("ak".into()), key_secret: Some("sk".into()), ..Default::default() },
            ..Default::default()
        });
        let id = "kling/video-kling-v2-6";
        let err = p.generate(&ref_schema(id), &make_base("x", id), &make_extras(), &refs(&[])).await.unwrap_err();
        assert!(matches!(&err, ProviderError::NotConfigured(m) if m.contains("API key") && m.contains("access/secret key pair")), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing was sent");
    }
}
