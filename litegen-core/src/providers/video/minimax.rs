use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::{ModelSchema, ParamSpec};
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig,
    VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// Vendor model ids served only by the V2 video API. Routing is by id so the
/// v1 models' requests are untouched.
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md> (2026-11-03)
///   Verbatim: "Currently available: `MiniMax-H3`, `MiniMax-H3-Highspeed`, `MiniMax-H3-Lite`,
///   `MiniMax-H3-Max`" — all four are carried (a new one needs its id here, or it is sent to v1).
const V2_MODELS: &[&str] = &["MiniMax-H3", "MiniMax-H3-Highspeed", "MiniMax-H3-Lite", "MiniMax-H3-Max"];

/// MiniMax (Hailuo) video generation provider.
///
/// Async, three-step: `POST /v1/video_generation` -> `task_id`; poll
/// `GET /v1/query/video_generation?task_id=` until `status == "Success"` (yields
/// `file_id`); then `GET /v1/files/retrieve?file_id=` -> `file.download_url`.
/// Bearer auth; region split via api_base.
///
/// The H3 family (`V2_MODELS`) exists only on the V2 video API, on the same
/// host: `POST /v2/video_generation` with a multimodal `content` array ->
/// `task_id`; poll `GET /v2/query/video_generation/{task_id}` until
/// `task.status == "succeeded"`, which carries the video at `task.content.url`
/// (no file-retrieve step). Every other model stays on v1.
///
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-query.md> (v1, 2026-11-09)
///   Verbatim: "`Success` – Completed successfully"; `file_id`: "Returned when the task succeeds."
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-download.md> (v1, 2026-11-09)
///   Verbatim: `GET /v1/files/retrieve`; `download_url`: "The URL address for downloading the file, valid for 1 hour."
/// @see <https://platform.minimax.io/docs/guides/video-generation.md> (V2, 2026-09-11)
///   Verbatim: "url = f\"{BASE_URL}/v2/query/video_generation/{task_id}\""
///   Verbatim: "if status == \"succeeded\": return task[\"content\"][\"url\"]"
pub struct MiniMaxVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl MiniMaxVideoProvider {
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
            .unwrap_or("https://api.minimax.io/v1")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("minimax".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("minimax/").unwrap_or(model_id)
    }

    /// Whether a catalog (or vendor) id is served by the V2 video API.
    pub(crate) fn is_v2(model_id: &str) -> bool {
        V2_MODELS.contains(&Self::resolve_model(model_id))
    }

    /// V2 sits beside v1 on the same host (`https://api.minimax.io/v2/...`).
    /// `api_base` carries the `/v1` suffix the v1 paths are relative to, so
    /// swap it for `/v2`; a base without one (a proxy, a test server) gets
    /// `/v2` appended.
    /// @see <https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md>
    ///   Verbatim: "servers: - url: https://api.minimax.io" / "/v2/video_generation"
    fn v2_base(&self) -> String {
        let base = self.api_base().trim_end_matches('/');
        format!("{}/v2", base.strip_suffix("/v1").unwrap_or(base))
    }

    /// A param's catalog default, for V2 body fields that are required
    /// upstream but optional in a litegen request (`resolution`, `duration`,
    /// and `ratio` for text-to-video).
    fn param_default(model: &ModelSchema, key: &str) -> Option<Value> {
        match model.params.get(key)? {
            ParamSpec::String(s) => s.default.clone().map(Value::String),
            ParamSpec::AspectRatio(a) => a.default.clone().map(Value::String),
            ParamSpec::Int(i) => i.default.map(|d| Value::Number(d.into())),
            _ => None,
        }
    }

    /// V2 submit: `POST /v2/video_generation` with the prompt as a `text`
    /// content item and each image as an `image_url` item tagged with its
    /// role. `resolution` and `duration` are required; `ratio` is required
    /// (and may not be `adaptive`) for text-only requests, ignored for
    /// first/last-frame requests and optional for reference requests, so the
    /// catalog default is sent when the request is text-only, and for
    /// MiniMax-H3-Lite, which "does not support `adaptive` yet".
    /// @see <https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md> (2026-11-09)
    ///   Verbatim: "required: - model - content - resolution - duration"
    ///   Verbatim: "Text-to-video (t2va, content contains only `text`)**: `ratio` is required and cannot be `adaptive`"
    async fn generate_v2(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/video_generation", self.v2_base());

        let mut content: Vec<Value> = vec![json!({ "type": "text", "text": base.prompt })];
        for r in &materialized.refs {
            let img = match &r.form {
                MaterializedRefForm::Url(u) => Some(u.clone()),
                MaterializedRefForm::Base64(b64) => Some(crate::proxy::materializer::base64_data_uri(b64, Some("image/png"))),
                _ => None,
            };
            if let Some(u) = img {
                // ContentItem.role enum: first_frame | last_frame | reference_image
                // (reference_video / reference_audio are not image inputs).
                let role = match r.role.as_str() {
                    "last_frame" => "last_frame",
                    "reference" | "reference_image" | "subject" => "reference_image",
                    _ => "first_frame",
                };
                content.push(json!({ "type": "image_url", "image_url": { "url": u }, "role": role }));
            }
        }
        let text_only = content.len() == 1;

        let mut body = json!({ "model": crate::providers::wire_model(self.config.as_ref(), &model.id, Self::resolve_model(&model.id)), "content": content });
        let duration = if extras.duration_seconds > 0.0 {
            Some(Value::Number((extras.duration_seconds.round() as i64).into()))
        } else {
            Self::param_default(model, "duration_seconds")
        };
        if let Some(d) = duration {
            body["duration"] = d;
        }
        let resolution = extras
            .resolution
            .clone()
            .map(Value::String)
            .or_else(|| Self::param_default(model, "resolution"));
        if let Some(r) = resolution {
            body["resolution"] = r;
        }
        // Reference-to-video: "`ratio` is optional and defaults to `adaptive` …
        // `MiniMax-H3-Lite` does not support `adaptive` yet; specify a concrete
        // ratio" — so H3-Lite gets the catalog default too. On first/last-frame
        // requests a ratio "does not error but is ignored and treated as
        // `adaptive`".
        // @see <https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md> (2026-11-09)
        let needs_ratio = text_only || Self::resolve_model(&model.id) == "MiniMax-H3-Lite";
        let ratio = extras.aspect_ratio.clone().map(Value::String).or_else(|| {
            if needs_ratio { Self::param_default(model, "aspect_ratio") } else { None }
        });
        if let Some(r) = ratio {
            body["ratio"] = r;
        }
        // `extra` adds the endpoint's own fields; it cannot replace one litegen
        // sets (rule 11).
        if let Some(obj) = body.as_object_mut() {
            let owned = |k: &str| matches!(k, "model" | "content" | "duration" | "resolution" | "ratio");
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), owned) {
                obj.insert(k.clone(), v.clone());
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
                message: format!("MiniMax V2 video request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // A call that timed out may still run, and bill: only a
                // connection that never opened is sent again.
                retryable: e.is_connect(),
            })?;

        // Errors are OpenAI-style: the real HTTP status, and
        // `{type: "error", error: {type, message, http_code}, request_id}`.
        let status = resp.status();
        let retry_after = crate::providers::retry_after_ms(resp.headers());
        let data: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let msg = data["error"]["message"].as_str().unwrap_or("Unknown error").to_string();
            let message = format!("MiniMax V2 API error (HTTP {}): {msg}", status.as_u16());
            if let Some(e) = crate::providers::rate_limited("minimax", status, retry_after, &message, Some(&data)) {
                return Err(e);
            }
            return Err(ProviderError::RequestFailed {
                message: format!("MiniMax V2 API error (HTTP {}): {msg}", status.as_u16()),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                retryable: status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS,
            });
        }

        let task_id = data["task_id"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| {
            ProviderError::RequestFailed {
                message: "MiniMax V2 response missing task_id".to_string(),
                status_code: Some(status.as_u16()),
                provider_error: Some(data.clone()),
                retryable: false,
            }
        })?;

        Ok(VideoGenerationHandle {
            provider_job_id: task_id.to_string(),
            provider: "minimax".to_string(),
            model: model.id.clone(),
        })
    }

    /// V2 poll: `GET /v2/query/video_generation/{task_id}` -> `{task}`.
    /// `queued` / `running` -> processing; `failed` / `cancelled` -> failed
    /// with `task.error.message`; `succeeded` -> completed with
    /// `task.content.url`. A success without a URL can never produce one, so
    /// it fails rather than polling forever; a body with no `task` is an error.
    /// @see <https://platform.minimax.io/docs/api-reference/video-generation-v2-query.md> (2026-09-11)
    ///   Verbatim: "enum: - queued - running - succeeded - failed - cancelled"
    ///   Verbatim: "Once the task succeeds (`status=succeeded`), retrieve video output from `content.url`"
    async fn poll_status_v2(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/query/video_generation/{}", self.v2_base(), handle.provider_job_id);
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
        let data = super::read_poll_json(resp, "minimax").await?;

        let task = &data["task"];
        if !task.is_object() {
            return Err(ProviderError::RequestFailed {
                message: "MiniMax V2 query response missing task".to_string(),
                status_code: None,
                provider_error: Some(data.clone()),
                retryable: false,
            });
        }
        let video_url = task["content"]["url"].as_str().filter(|u| !u.is_empty()).map(String::from);
        let (status, error) = match task["status"].as_str() {
            Some("succeeded") if video_url.is_some() => (GenerationStatus::Completed, None),
            Some("succeeded") => (
                GenerationStatus::Failed,
                Some("MiniMax V2 task succeeded but returned no content.url".to_string()),
            ),
            Some(s @ ("failed" | "cancelled")) => {
                let msg = task["error"]["message"]
                    .as_str()
                    .filter(|m| !m.is_empty())
                    .or_else(|| task["error"]["code"].as_str())
                    .map(String::from)
                    .unwrap_or_else(|| format!("MiniMax V2 task {s}"));
                (GenerationStatus::Failed, Some(msg))
            }
            Some("queued") | Some("running") => (GenerationStatus::Processing, None),
            other => return Ok(super::unknown_state("minimax", other)),
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

    /// Resolve the `file.download_url` for a completed task's file.
    async fn retrieve_file_url(&self, file_id: &str) -> Result<String, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/files/retrieve?file_id={}", self.api_base(), file_id);
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
        let data = super::read_poll_json(resp, "minimax").await?;
        data["file"]["download_url"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "MiniMax files/retrieve missing file.download_url".to_string(),
                status_code: None,
                provider_error: Some(data),
                retryable: false,
            })
    }
}

impl Default for MiniMaxVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for MiniMaxVideoProvider {
    fn name(&self) -> &str {
        "minimax"
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
        if Self::is_v2(&model.id) {
            return self.generate_v2(model, base, extras, materialized).await;
        }
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);
        let url = format!("{}/video_generation", self.api_base());

        let mut body = json!({ "model": crate::providers::wire_model(self.config.as_ref(), &model.id, native), "prompt": base.prompt });
        // S2V-01's request has no duration or resolution: "required: - model
        // - subject_reference".
        // @see <https://platform.minimax.io/docs/api-reference/video-generation-s2v.md>
        let takes_length = native != "S2V-01";
        if takes_length && extras.duration_seconds > 0.0 {
            let secs = extras.duration_seconds.round() as i64;
            // Hailuo makes 6 or 10 s, and only 6 s at 1080P: "| MiniMax-Hailuo-02 |
            // `6` or `10` | `6` or `10` | `6` |" (512P / 768P / 1080P). A length it
            // cannot make is refused rather than sent.
            // @see <https://platform.minimax.io/docs/api-reference/video-generation-i2v.md>
            if native.starts_with("MiniMax-Hailuo") {
                let at_1080 = extras.resolution.as_deref() == Some("1080P");
                if !(secs == 6 || (secs == 10 && !at_1080)) {
                    return Err(ProviderError::InvalidRequest(format!(
                        "{} makes 6 or 10 s, and only 6 s at 1080P; asked for {secs} s",
                        model.id
                    )));
                }
            }
            body["duration"] = Value::Number(secs.into());
        }
        if takes_length {
            if let Some(res) = extras.resolution.as_deref() {
                body["resolution"] = Value::String(res.to_string());
            }
        }
        // first/last frame images (URL or base64 data URL).
        for r in &materialized.refs {
            let img = match &r.form {
                MaterializedRefForm::Url(u) => Some(u.clone()),
                MaterializedRefForm::Base64(b64) => Some(crate::proxy::materializer::base64_data_uri(b64, Some("image/png"))),
                _ => None,
            };
            if let Some(f) = img {
                match r.role.as_str() {
                    "last_frame" => body["last_frame_image"] = Value::String(f),
                    // The video `SubjectReference` is `{type, image: [..]}`, both
                    // required. `image_file` is the *image* API's field; the
                    // video schema has no such property.
                    // @see <https://platform.minimax.io/docs/api-reference/video-generation-s2v.md>
                    "subject" | "reference" => {
                        body["subject_reference"] = json!([{ "type": "character", "image": [f] }])
                    }
                    _ => body["first_frame_image"] = Value::String(f),
                }
            }
        }
        // `extra` adds the endpoint's own fields; it cannot replace one litegen
        // sets (rule 11). First-and-last-frame requests take no
        // `fast_pretreatment` ("Applies only to" text- and image-to-video).
        let with_last = body.get("last_frame_image").is_some();
        if let Some(obj) = body.as_object_mut() {
            let owned = |k: &str| {
                matches!(k, "model" | "prompt" | "duration" | "resolution" | "first_frame_image" | "last_frame_image" | "subject_reference")
                    || (with_last && k == "fast_pretreatment")
            };
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), owned) {
                obj.insert(k.clone(), v.clone());
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
                message: format!("MiniMax video request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // A call that timed out may still run, and bill: only a
                // connection that never opened is sent again.
                retryable: e.is_connect(),
            })?;

        let status = resp.status();
        let data = crate::providers::read_vendor_json(resp, "MiniMax").await?;
        let code = data["base_resp"]["status_code"].as_i64().unwrap_or(0);
        if code != 0 {
            let msg = data["base_resp"]["status_msg"].as_str().unwrap_or("Unknown error").to_string();
            return Err(ProviderError::RequestFailed {
                message: format!("MiniMax API error ({code}): {msg}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                // "`1002`, Rate limit triggered, please try again later"
                // @see <https://platform.minimax.io/docs/api-reference/video-generation-t2v.md> (2026-10-30)
                retryable: code == 1002,
            });
        }

        let task_id = data["task_id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "MiniMax response missing task_id".to_string(),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        })?;

        Ok(VideoGenerationHandle {
            provider_job_id: task_id.to_string(),
            provider: "minimax".to_string(),
            model: model.id.clone(),
        })
    }

    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        if Self::is_v2(&handle.model) {
            return self.poll_status_v2(handle).await;
        }
        let creds = self.creds()?;
        let url = format!("{}/query/video_generation?task_id={}", self.api_base(), handle.provider_job_id);
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
        let data = super::read_poll_json(resp, "minimax").await?;
        // A query MiniMax turned away answers HTTP 200 with a non-zero
        // `base_resp.status_code` and no status. A passing code (unknown
        // error, timeout, rate limits, internal error) is asked again; any
        // other is final. A task that failed (`Fail`, its code saying why:
        // 1026, 1027) keeps its status and reason below.
        // @see <https://platform.minimax.io/docs/api-reference/errorcode> (2026-11-03)
        let code = data["base_resp"]["status_code"].as_i64().unwrap_or(0);
        if code != 0 && data["status"].as_str().is_none_or(str::is_empty) {
            return Err(ProviderError::RequestFailed {
                message: format!(
                    "MiniMax query answered {code}: {}",
                    data["base_resp"]["status_msg"].as_str().unwrap_or("no message")
                ),
                status_code: None,
                provider_error: Some(data.clone()),
                retryable: matches!(code, 1000 | 1001 | 1002 | 1013 | 1039),
            });
        }

        let status_str = data["status"].as_str().unwrap_or("");
        // `file_id` is "Returned when the task succeeds": a Success without one
        // can never be delivered. A file lookup that fails transiently is
        // asked again next tick; one that fails for good (the file is gone)
        // is a verdict, so the job fails now, naming why, and its webhook fires.
        // @see <https://platform.minimax.io/docs/api-reference/video-generation-query.md>
        let (status, video_url, error) = match status_str {
            "Success" => match data["file_id"].as_str().filter(|f| !f.is_empty()) {
                Some(file_id) => match self.retrieve_file_url(file_id).await {
                    Ok(url) => (GenerationStatus::Completed, Some(url), None),
                    Err(ProviderError::RequestFailed { retryable: false, message, .. }) => (
                        GenerationStatus::Failed,
                        None,
                        Some(format!("MiniMax finished the job, but its file could not be fetched: {message}")),
                    ),
                    Err(e) => return Err(e),
                },
                None => (GenerationStatus::Failed, None, Some("MiniMax reported Success without a file_id".to_string())),
            },
            "Fail" => (
                GenerationStatus::Failed,
                None,
                Some(data["base_resp"]["status_msg"].as_str().unwrap_or("failed").to_string()),
            ),
            // "Preparing", "Queueing", "Processing", "Success", "Fail".
            "Preparing" | "Queueing" | "Processing" => (GenerationStatus::Processing, None, None),
            other => return Ok(super::unknown_state("minimax", Some(other).filter(|s| !s.is_empty()))),
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
                message: "MiniMax video provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "MiniMax video provider configured".into(), latency_ms: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::{Cleanup, MaterializedRef};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> MiniMaxVideoProvider {
        let mut p = MiniMaxVideoProvider::new();
        let mut cfg = ProviderInstanceConfig { api_key: "mm-key".to_string(), api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.api_key = Some("mm-key".to_string());
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
        VideoExtras { duration_seconds: 6.0, aspect_ratio: None, resolution: Some("1080P".to_string()), fps: None, extra: None, camera: None, audio: None, r#loop: None, source: None, direction: None, time_range: None, replace: None }
    }

    #[tokio::test]
    async fn submits_video_and_polls_to_download_url() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/video_generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "task_id": "mm-task-1", "base_resp": { "status_code": 0, "status_msg": "success" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/query/video_generation"))
            .and(query_param("task_id", "mm-task-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "Success", "file_id": "file-99", "base_resp": { "status_code": 0 }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/files/retrieve"))
            .and(query_param("file_id", "file-99"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "file": { "download_url": "https://cdn.minimax/video.mp4" }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("minimax/MiniMax-Hailuo-02");
        let base = make_base("a drone shot over a canyon", "minimax/MiniMax-Hailuo-02");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "mm-task-1");

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_url.unwrap(), "https://cdn.minimax/video.mp4");
    }

    /// S2V-01's `subject_reference` items are `SubjectReference`
    /// `{type, image: [string]}` with both fields required; `image_file` is the
    /// image API's `ImageSubjectReference` field, not the video one.
    /// @see <https://platform.minimax.io/docs/api-reference/video-generation-s2v.md> (2026-09-11)
    #[tokio::test]
    async fn s2v_sends_subject_reference_as_an_image_array() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/video_generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "task_id": "mm-task-2", "base_resp": { "status_code": 0, "status_msg": "success" }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("minimax/S2V-01");
        let base = make_base("the same character waves", "minimax/S2V-01");
        let extras = VideoExtras { duration_seconds: 6.0, aspect_ratio: None, resolution: None, fps: None, extra: None, camera: None, audio: None, r#loop: None, source: None, direction: None, time_range: None, replace: None };
        let materialized = MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "subject".to_string(),
                form: MaterializedRefForm::Url("https://example.test/face.png".to_string()),
                time_s: None,
            }],
            cleanup: Cleanup::empty(),
        };

        provider.generate(&schema, &base, &extras, &materialized).await.unwrap();

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        let subject = &body["subject_reference"][0];
        assert_eq!(subject["type"], "character");
        assert_eq!(subject["image"], json!(["https://example.test/face.png"]));
        assert!(subject.get("image_file").is_none(), "image_file is the image API's field");
    }

    // ─── V2 (MiniMax-H3 / MiniMax-H3-Max) ──────────────────────────────────
    // @see <https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md> (2026-09-11)
    // @see <https://platform.minimax.io/docs/api-reference/video-generation-v2-query.md> (2026-09-11)

    /// The production base carries `/v1`; V2 must swap it for `/v2`.
    fn make_v2_provider(server: &MockServer) -> MiniMaxVideoProvider {
        make_provider(&format!("{}/v1", server.uri()))
    }

    async fn mount_v2_submit(server: &MockServer, task_id: &str) {
        Mock::given(method("POST"))
            .and(path("/v2/video_generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": task_id })))
            .mount(server)
            .await;
    }

    fn url_ref(role: &str, url: &str) -> crate::proxy::materializer::MaterializedRef {
        crate::proxy::materializer::MaterializedRef {
            role: role.to_string(),
            form: MaterializedRefForm::Url(url.to_string()),
            time_s: None,
        }
    }

    fn default_extras() -> VideoExtras {
        // The gateway's default duration (5 s); no resolution or ratio given.
        VideoExtras { duration_seconds: 5.0, aspect_ratio: None, resolution: None, fps: None, extra: None, camera: None, audio: None, r#loop: None, source: None, direction: None, time_range: None, replace: None }
    }

    async fn sent_body(server: &MockServer) -> Value {
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1, "exactly one submit");
        assert_eq!(received[0].url.path(), "/v2/video_generation");
        serde_json::from_slice(&received[0].body).unwrap()
    }

    fn v2_handle(task_id: &str, model: &str) -> VideoGenerationHandle {
        VideoGenerationHandle { provider_job_id: task_id.to_string(), provider: "minimax".to_string(), model: model.to_string() }
    }

    #[tokio::test]
    async fn h3_text_only_submits_a_v2_content_array_with_the_required_fields() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/video_generation"))
            .and(wiremock::matchers::header("Authorization", "Bearer mm-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": "h3-task-1" })))
            .mount(&server)
            .await;

        let provider = make_v2_provider(&server);
        let schema = ref_schema("minimax/MiniMax-H3");
        let base = make_base("a boy playing basketball by the sea", "minimax/MiniMax-H3");
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &default_extras(), &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "h3-task-1");

        let body = sent_body(&server).await;
        assert_eq!(body["model"], "MiniMax-H3");
        assert_eq!(body["content"], json!([{ "type": "text", "text": "a boy playing basketball by the sea" }]));
        assert_eq!(body["duration"], 5);
        // resolution is required upstream: the catalog default fills it.
        assert_eq!(body["resolution"], "768P");
        // text-to-video requires a concrete ratio (not adaptive).
        assert_eq!(body["ratio"], "16:9");
        assert!(body.get("prompt").is_none(), "V2 carries the prompt in content, not a prompt field");
    }

    /// MiniMax-H3-Highspeed and MiniMax-H3-Lite exist only on the V2 video
    /// API ("Currently available: `MiniMax-H3`, `MiniMax-H3-Highspeed`,
    /// `MiniMax-H3-Lite`, `MiniMax-H3-Max`"): each goes there, by its own id.
    /// @see <https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md> (2026-11-03)
    #[tokio::test]
    async fn the_new_h3_variants_go_to_the_v2_api() {
        for id in ["minimax/MiniMax-H3-Highspeed", "minimax/MiniMax-H3-Lite"] {
            let server = MockServer::start().await;
            mount_v2_submit(&server, "h3-variant").await;
            let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
            make_v2_provider(&server).generate(&ref_schema(id), &make_base("a lighthouse", id), &default_extras(), &materialized).await.unwrap_or_else(|e| panic!("{id}: {e:?}"));
            let body = sent_body(&server).await;
            assert_eq!(body["model"], id.trim_start_matches("minimax/"), "{body}");
            assert_eq!(body["resolution"], "768P", "{body}");
        }
    }

    /// Reference-to-video: "`ratio` is optional and defaults to `adaptive` …
    /// `MiniMax-H3-Lite` does not support `adaptive` yet; specify a concrete
    /// ratio." So an H3-Lite reference request that names no ratio goes with the
    /// row's default; MiniMax-H3's still sends none, so `adaptive` applies.
    /// @see <https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md> (2026-11-09)
    #[tokio::test]
    async fn h3_lite_references_never_ride_on_the_adaptive_default() {
        for (id, want) in [("minimax/MiniMax-H3-Lite", Some("16:9")), ("minimax/MiniMax-H3", None)] {
            let server = MockServer::start().await;
            mount_v2_submit(&server, "h3-ref").await;
            let materialized = MaterializedRequest { refs: vec![url_ref("reference", "https://img.test/r.png")], cleanup: Cleanup::empty() };
            make_v2_provider(&server).generate(&ref_schema(id), &make_base("a lighthouse", id), &default_extras(), &materialized).await.unwrap_or_else(|e| panic!("{id}: {e:?}"));
            let body = sent_body(&server).await;
            assert_eq!(body["ratio"].as_str(), want, "{id}: {body}");
        }
    }

    #[tokio::test]
    async fn h3_max_sends_first_and_last_frames_as_roled_image_items() {
        let server = MockServer::start().await;
        mount_v2_submit(&server, "h3max-task-1").await;

        let provider = make_v2_provider(&server);
        let schema = ref_schema("minimax/MiniMax-H3-Max");
        let base = make_base("a little girl grows up", "minimax/MiniMax-H3-Max");
        let extras = VideoExtras { duration_seconds: 6.0, aspect_ratio: None, resolution: Some("480P".to_string()), fps: None, extra: None, camera: None, audio: None, r#loop: None, source: None, direction: None, time_range: None, replace: None };
        let materialized = MaterializedRequest {
            refs: vec![
                url_ref("first_frame", "https://img.test/start.png"),
                crate::proxy::materializer::MaterializedRef {
                    role: "last_frame".to_string(),
                    form: MaterializedRefForm::Base64("QUJD".to_string()),
                    time_s: None,
                },
            ],
            cleanup: Cleanup::empty(),
        };

        provider.generate(&schema, &base, &extras, &materialized).await.unwrap();

        let body = sent_body(&server).await;
        assert_eq!(body["model"], "MiniMax-H3-Max");
        assert_eq!(
            body["content"],
            json!([
                { "type": "text", "text": "a little girl grows up" },
                { "type": "image_url", "image_url": { "url": "https://img.test/start.png" }, "role": "first_frame" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,QUJD" }, "role": "last_frame" },
            ])
        );
        assert_eq!(body["duration"], 6);
        assert_eq!(body["resolution"], "480P");
        // image-to-video ignores ratio (always adaptive): none is invented.
        assert!(body.get("ratio").is_none());
        assert!(body.get("first_frame_image").is_none() && body.get("last_frame_image").is_none());
    }

    #[tokio::test]
    async fn h3_sends_reference_images_with_the_reference_image_role() {
        let server = MockServer::start().await;
        mount_v2_submit(&server, "h3-task-ref").await;

        let provider = make_v2_provider(&server);
        let schema = ref_schema("minimax/MiniMax-H3");
        let base = make_base("the character follows reference image 1", "minimax/MiniMax-H3");
        let materialized = MaterializedRequest {
            refs: vec![url_ref("reference", "https://img.test/a.png"), url_ref("reference", "https://img.test/b.png")],
            cleanup: Cleanup::empty(),
        };

        provider.generate(&schema, &base, &default_extras(), &materialized).await.unwrap();

        let body = sent_body(&server).await;
        let content = body["content"].as_array().unwrap();
        assert_eq!(content.len(), 3);
        for item in &content[1..] {
            assert_eq!(item["type"], "image_url");
            assert_eq!(item["role"], "reference_image");
        }
        // reference-to-video defaults ratio to adaptive: none is invented.
        assert!(body.get("ratio").is_none());
        assert!(body.get("subject_reference").is_none());
    }

    #[tokio::test]
    async fn h3_submit_error_surfaces_the_vendor_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/video_generation"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "type": "error",
                "error": {
                    "type": "bad_request_error",
                    "message": "invalid params, content must include a non-empty text item (prompt is required) (2013)",
                    "http_code": "400"
                },
                "request_id": "021785229015510a2c883cf675b9804d"
            })))
            .mount(&server)
            .await;

        let provider = make_v2_provider(&server);
        let schema = ref_schema("minimax/MiniMax-H3");
        let base = make_base("x", "minimax/MiniMax-H3");
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
        let err = provider.generate(&schema, &base, &default_extras(), &materialized).await.unwrap_err();
        match err {
            ProviderError::RequestFailed { message, status_code, retryable, .. } => {
                assert!(message.contains("content must include a non-empty text item"), "{message}");
                assert_eq!(status_code, Some(400));
                assert!(!retryable);
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[tokio::test]
    async fn h3_submit_success_without_a_task_id_is_an_error() {
        let server = MockServer::start().await;
        mount_v2_submit(&server, "").await;

        let provider = make_v2_provider(&server);
        let schema = ref_schema("minimax/MiniMax-H3");
        let base = make_base("x", "minimax/MiniMax-H3");
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
        let err = provider.generate(&schema, &base, &default_extras(), &materialized).await.unwrap_err();
        assert!(err.to_string().contains("task_id"), "{err}");
        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].url.path(), "/v2/video_generation");
    }

    #[tokio::test]
    async fn h3_poll_maps_the_v2_task_statuses() {
        let server = MockServer::start().await;
        let tasks = [
            ("t-queued", json!({ "task": { "id": "t-queued", "model": "MiniMax-H3", "status": "queued" } })),
            ("t-running", json!({ "task": { "id": "t-running", "model": "MiniMax-H3", "status": "running" } })),
            ("t-failed", json!({ "task": {
                "id": "t-failed", "model": "MiniMax-H3", "status": "failed",
                "error": { "code": "1026", "message": "video description contains sensitive content" }
            } })),
            ("t-done", json!({ "task": {
                "id": "t-done", "model": "MiniMax-H3", "status": "succeeded",
                "content": { "url": "https://cdn.minimax/h3.mp4" }, "task_type": "generation", "modality": "video"
            } })),
        ];
        for (id, resp) in &tasks {
            Mock::given(method("GET"))
                .and(path(format!("/v2/query/video_generation/{id}")))
                .and(wiremock::matchers::header("Authorization", "Bearer mm-key"))
                .respond_with(ResponseTemplate::new(200).set_body_json(resp.clone()))
                .mount(&server)
                .await;
        }
        let provider = make_v2_provider(&server);

        for id in ["t-queued", "t-running"] {
            let poll = provider.poll_status(&v2_handle(id, "minimax/MiniMax-H3")).await.unwrap();
            assert_eq!(poll.status, GenerationStatus::Processing, "{id}");
            assert!(poll.video_url.is_none());
        }

        let failed = provider.poll_status(&v2_handle("t-failed", "minimax/MiniMax-H3")).await.unwrap();
        assert_eq!(failed.status, GenerationStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("video description contains sensitive content"));
        assert_eq!(failed.progress, 100, "W15: a terminal poll is done");

        let done = provider.poll_status(&v2_handle("t-done", "minimax/MiniMax-H3-Max")).await.unwrap();
        assert_eq!(done.status, GenerationStatus::Completed);
        assert_eq!(done.video_url.as_deref(), Some("https://cdn.minimax/h3.mp4"));
    }

    /// W14: v1 rate limiting comes as HTTP 200 with "`1002`, Rate limit
    /// triggered, please try again later", or as a plain 429; a gateway's 5xx
    /// page is not JSON. Each turned the call away and may be sent again.
    #[tokio::test]
    async fn a_turned_away_v1_submit_may_be_sent_again() {
        let cases = [
            (200, json!({ "base_resp": { "status_code": 1002, "status_msg": "Rate limit triggered, please try again later" } }).to_string()),
            (429, "too many requests".to_string()),
            (502, "<html>bad gateway</html>".to_string()),
        ];
        for (status, body) in cases {
            let server = MockServer::start().await;
            Mock::given(method("POST")).respond_with(ResponseTemplate::new(status).set_body_string(body.clone())).mount(&server).await;
            let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
            let err = make_provider(&server.uri())
                .generate(&ref_schema("minimax/MiniMax-Hailuo-02"), &make_base("x", "minimax/MiniMax-Hailuo-02"), &make_extras(), &materialized)
                .await
                .unwrap_err();
            assert!(err.is_retryable(), "{status} {body}: {err:?}");
        }
    }

    #[tokio::test]
    async fn h3_succeeded_without_a_content_url_fails_instead_of_polling_forever() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/query/video_generation/t-empty"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "task": { "id": "t-empty", "model": "MiniMax-H3", "status": "succeeded", "content": {} }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/query/video_generation/t-notask"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        let provider = make_v2_provider(&server);

        let poll = provider.poll_status(&v2_handle("t-empty", "minimax/MiniMax-H3")).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Failed);
        assert!(poll.error.unwrap().contains("content.url"));
        assert!(poll.video_url.is_none());

        let err = provider.poll_status(&v2_handle("t-notask", "minimax/MiniMax-H3")).await.unwrap_err();
        assert!(err.to_string().contains("missing task"), "{err}");
    }

    // ─── ByteDance/MiniMax/Vidu audit, 2026-10-30: W7, W11–W13, C6 ──────────

    fn v1_handle(id: &str, model: &str) -> VideoGenerationHandle {
        VideoGenerationHandle { provider_job_id: id.into(), provider: "minimax".into(), model: model.into() }
    }

    /// W7: a Success with no file to fetch is a failure; a file lookup that
    /// fails is asked again next tick, never stored as a video with no URL.
    #[tokio::test]
    async fn a_success_without_a_file_fails_and_a_failed_lookup_is_retried() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/query/video_generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "Success", "file_id": "", "base_resp": { "status_code": 0 } })))
            .mount(&server)
            .await;
        let poll = make_provider(&server.uri()).poll_status(&v1_handle("t1", "minimax/MiniMax-Hailuo-02")).await.unwrap();
        assert_eq!((poll.status, poll.progress), (GenerationStatus::Failed, 100));

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/query/video_generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "Success", "file_id": "f1", "base_resp": { "status_code": 0 } })))
            .mount(&server)
            .await;
        Mock::given(method("GET")).and(path("/files/retrieve")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
        let got = make_provider(&server.uri()).poll_status(&v1_handle("t2", "minimax/MiniMax-Hailuo-02")).await;
        assert!(matches!(got, Err(ProviderError::RequestFailed { retryable: true, .. })), "{got:?}");
    }

    /// A job that finished but whose file is gone is a verdict: it fails now,
    /// naming why, so its webhook fires (not a row reaped in silence).
    #[tokio::test]
    async fn a_finished_job_whose_file_is_gone_fails_with_the_reason() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/query/video_generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "Success", "file_id": "f1", "base_resp": { "status_code": 0 } })))
            .mount(&server)
            .await;
        Mock::given(method("GET")).and(path("/files/retrieve")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        let poll = make_provider(&server.uri()).poll_status(&v1_handle("t3", "minimax/MiniMax-Hailuo-02")).await.expect("a verdict");
        assert_eq!(poll.status, GenerationStatus::Failed);
        assert!(poll.error.as_deref().is_some_and(|e| e.contains("file")), "{:?}", poll.error);
    }

    async fn submitted(server: &MockServer) -> Value {
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap()
    }

    async fn accept(server: &MockServer) {
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": "t", "base_resp": { "status_code": 0, "status_msg": "success" } })))
            .mount(server)
            .await;
    }

    /// W11: S2V-01's request has no `duration` or `resolution`.
    #[tokio::test]
    async fn subject_to_video_sends_no_duration() {
        let server = MockServer::start().await;
        accept(&server).await;
        let refs = MaterializedRequest {
            refs: vec![MaterializedRef { role: "subject".into(), form: MaterializedRefForm::Url("https://x/face.png".into()), time_s: None }],
            cleanup: Cleanup::empty(),
        };
        make_provider(&server.uri()).generate(&ref_schema("minimax/S2V-01"), &make_base("x", "minimax/S2V-01"), &make_extras(), &refs).await.unwrap();
        let body = submitted(&server).await;
        assert!(body.get("duration").is_none() && body.get("resolution").is_none(), "{body}");
    }

    /// W12: Hailuo takes 6 or 10 s, and only 6 s at 1080P.
    #[tokio::test]
    async fn a_hailuo_length_it_cannot_make_is_refused() {
        let server = MockServer::start().await;
        accept(&server).await;
        let none = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
        let schema = ref_schema("minimax/MiniMax-Hailuo-02");
        for (secs, res, ok) in [(6.0, "1080P", true), (10.0, "768P", true), (7.0, "768P", false), (10.0, "1080P", false)] {
            let mut extras = make_extras();
            extras.duration_seconds = secs;
            extras.resolution = Some(res.into());
            let got = make_provider(&server.uri()).generate(&schema, &make_base("x", "minimax/MiniMax-Hailuo-02"), &extras, &none).await;
            assert_eq!(got.is_ok(), ok, "{secs} s at {res}: {got:?}");
            if !ok {
                assert!(matches!(got, Err(ProviderError::InvalidRequest(_))), "{got:?}");
            }
        }
    }

    /// W13, C6: a base64 frame goes as a data URI of its own type, and
    /// `fast_pretreatment`, which first-and-last-frame requests do not take,
    /// is not sent with a last frame.
    #[tokio::test]
    async fn frames_go_with_their_own_type_and_only_the_fields_their_mode_takes() {
        use base64::Engine;
        let server = MockServer::start().await;
        accept(&server).await;
        let jpeg = base64::engine::general_purpose::STANDARD.encode([0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0, 1]);
        let refs = MaterializedRequest {
            refs: vec![
                MaterializedRef { role: "first_frame".into(), form: MaterializedRefForm::Base64(jpeg.clone()), time_s: None },
                MaterializedRef { role: "last_frame".into(), form: MaterializedRefForm::Base64(jpeg), time_s: None },
            ],
            cleanup: Cleanup::empty(),
        };
        let mut extras = make_extras();
        extras.extra = Some(json!({ "fast_pretreatment": true }));
        make_provider(&server.uri()).generate(&ref_schema("minimax/MiniMax-Hailuo-02"), &make_base("x", "minimax/MiniMax-Hailuo-02"), &extras, &refs).await.unwrap();
        let body = submitted(&server).await;
        assert!(body["first_frame_image"].as_str().unwrap().starts_with("data:image/jpeg;base64,"), "{body}");
        assert!(body.get("fast_pretreatment").is_none(), "{body}");
    }

    /// E4a: where the result goes is litegen's (it polls, then fires the
    /// key's webhook), so MiniMax's `callback_url` is never sent, on either
    /// endpoint; the endpoint's own fields still are.
    #[tokio::test]
    async fn callback_url_is_never_sent_on_either_endpoint() {
        let none = || MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
        let extra = Some(json!({ "callback_url": "https://attacker.example/cb", "prompt_optimizer": false }));
        let v1 = MockServer::start().await;
        accept(&v1).await;
        let mut extras = make_extras();
        extras.extra = extra.clone();
        make_provider(&v1.uri()).generate(&ref_schema("minimax/MiniMax-Hailuo-2.3"), &make_base("x", "minimax/MiniMax-Hailuo-2.3"), &extras, &none()).await.unwrap();
        let v2 = MockServer::start().await;
        mount_v2_submit(&v2, "t").await;
        let mut extras = default_extras();
        extras.extra = extra;
        make_v2_provider(&v2).generate(&ref_schema("minimax/MiniMax-H3"), &make_base("x", "minimax/MiniMax-H3"), &extras, &none()).await.unwrap();
        for body in [submitted(&v1).await, sent_body(&v2).await] {
            assert!(body.get("callback_url").is_none(), "{body}");
            assert_eq!(body["prompt_optimizer"], false, "{body}");
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
        let err = p.generate(&ref_schema("minimax/MiniMax-Hailuo-02"), &make_base("x", "minimax/MiniMax-Hailuo-02"), &make_extras(), &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
    }

    /// MiniMax answers a query it turned away with HTTP 200 and a non-zero
    /// `base_resp.status_code`, no `status`: a passing code (rate limit,
    /// timeout, internal error) is asked again, never a failed job; any other
    /// code is final. The code is read before the state.
    /// @see <https://platform.minimax.io/docs/api-reference/errorcode> (2026-11-03)
    #[tokio::test]
    async fn a_query_turned_away_is_asked_again_not_failed() {
        for (code, retryable) in [(1002, true), (1001, true), (1013, true), (1004, false)] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "base_resp": { "status_code": code, "status_msg": "turned away" } })))
                .mount(&server)
                .await;
            let handle = VideoGenerationHandle { provider_job_id: "t1".into(), provider: "minimax".into(), model: "minimax/MiniMax-Hailuo-02".into() };
            match make_provider(&server.uri()).poll_status(&handle).await {
                Err(e) => assert_eq!(e.is_retryable(), retryable, "{code}: {e}"),
                Ok(r) => panic!("{code}: a turned-away query is an error, got {:?} {:?}", r.status, r.error),
            }
        }
        // A failed task says why in the same field, and stays a failure.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "Fail", "base_resp": { "status_code": 1027, "status_msg": "sensitive content in the generated video" }
            })))
            .mount(&server)
            .await;
        let handle = VideoGenerationHandle { provider_job_id: "t1".into(), provider: "minimax".into(), model: "minimax/MiniMax-Hailuo-02".into() };
        let r = make_provider(&server.uri()).poll_status(&handle).await.unwrap();
        assert_eq!(r.status, GenerationStatus::Failed);
        assert!(r.error.as_deref().is_some_and(|e| e.contains("sensitive")), "{:?}", r.error);
    }
}
