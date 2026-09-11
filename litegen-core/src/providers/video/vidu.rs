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
/// @see <https://platform.vidu.com/docs/text-to-video>
///   Verbatim: "POST https://api.vidu.com/ent/v2/text2video"
///   Verbatim: "Authorization: Token {your api key}"
/// @see <https://platform.vidu.com/docs/get-generation>
///   Verbatim: "https://api.vidu.com/ent/v2/tasks/{id}/creations"
///   (state ∈ created|queueing|processing|success|failed; result at creations[].url)
pub struct ViduProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl ViduProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::Header { name: "Authorization".to_string(), value_prefix: "Token ".to_string() },
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
            MaterializedRefForm::Base64(b64) => Some(format!("data:image/png;base64,{b64}")),
            _ => None,
        }
    }

    fn is_last_frame(role: &str) -> bool {
        role == "last_frame" || role == "end_frame"
    }

    /// The `duration` to send. vidu2.0 renders 4s or 8s clips only (4s only on
    /// reference2video), while the gateway defaults an omitted duration to 5s,
    /// so snap to the nearest length the endpoint accepts (ties go to the
    /// shorter clip, like the OpenAI adapter's `resolve_seconds`) rather than
    /// forward a value Vidu rejects. Other models pass through; the catalog's
    /// `duration_seconds` bounds already hold them to what Vidu accepts.
    /// @see <https://platform.vidu.com/docs/image-to-video.md>
    ///   Verbatim: "vidu2.0: default 4s, available: 4, 8"
    /// @see <https://platform.vidu.com/docs/reference-to-video.md>
    ///   Verbatim: "vidu2.0: Default is 4 seconds, available option: 4"
    fn resolve_duration(native: &str, endpoint: &str, requested: f64) -> i64 {
        if native != "vidu2.0" {
            return requested as i64;
        }
        if endpoint == "reference2video" || requested <= 6.0 {
            4
        } else {
            8
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
            // `text_to_video` (viduq1 among ours), and nothing before the
            // adapter checks capability flags — refuse instead of sending a
            // request Vidu rejects.
            // @see <https://platform.vidu.com/docs/text-to-video.md>
            //   Verbatim: "Accepted values:`viduq3-turbo`, `viduq3-pro` , `viduq2` , `viduq1`"
            return Err(ProviderError::InvalidRequest(format!(
                "{} has no text-to-video mode; attach a first_frame or reference image",
                model.id
            )));
        };
        let url = format!("{}/ent/v2/{endpoint}", self.api_base());

        let mut body = json!({ "model": native, "prompt": base.prompt });
        if !images.is_empty() {
            body["images"] = Value::Array(images.into_iter().map(Value::String).collect());
        }
        if extras.duration_seconds > 0.0 {
            let duration = Self::resolve_duration(native, endpoint, extras.duration_seconds);
            body["duration"] = Value::Number(duration.into());
        }
        if let Some(res) = extras.resolution.as_deref() {
            body["resolution"] = Value::String(res.to_string());
        }
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }
        if let Some(Value::Object(map)) = &extras.extra {
            if let Some(obj) = body.as_object_mut() {
                for (k, v) in map {
                    obj.insert(k.clone(), v.clone());
                }
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
                retryable: e.is_timeout() || e.is_connect(),
            })?;

        let status = resp.status();
        let data: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Vidu response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        if !status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Vidu API error: {data}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                retryable: status.as_u16() >= 500,
            });
        }

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
            _ => GenerationStatus::Processing,
        };
        let video_url = data["creations"][0]["url"].as_str().map(String::from);

        Ok(VideoGenerationPollResult {
            status,
            progress: if status == GenerationStatus::Completed { 100 } else { 50 },
            video_url,
            video_data: None,
            content_type: Some("video/mp4".into()),
            error: data["err_code"].as_str().filter(|s| !s.is_empty()).map(String::from),
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
        VideoExtras { duration_seconds: 5.0, aspect_ratio: Some("16:9".to_string()), resolution: Some("720p".to_string()), fps: None, extra: None }
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
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "vd-1");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].headers.get("authorization").unwrap(), "Token vidu-key");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "viduq1");

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_url.unwrap(), "https://cdn.vidu/v.mp4");
    }

    fn url_ref(role: &str, url: &str) -> MaterializedRef {
        MaterializedRef { role: role.to_string(), form: MaterializedRefForm::Url(url.to_string()) }
    }

    async fn mount_submit(server: &MockServer, endpoint: &str) {
        Mock::given(method("POST"))
            .and(path(format!("/ent/v2/{endpoint}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "task_id": "vd-2", "state": "created" })))
            .mount(server)
            .await;
    }

    /// text2video accepts viduq3-turbo, viduq3-pro, viduq2 and viduq1 only, and
    /// nothing upstream of the adapter checks `capabilities.text_to_video`, so a
    /// prompt-only request for an image-only model must never reach text2video.
    /// @see <https://platform.vidu.com/docs/text-to-video.md>
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

    /// start-end2video reads `images` as [start frame, end frame]; the order the
    /// caller listed its refs in must not swap them.
    /// @see <https://platform.vidu.com/docs/start-end-to-video.md>
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

    /// vidu2.0 takes 4s or 8s (4s only on reference2video); the gateway's
    /// default duration of 5s is not one of them.
    /// @see <https://platform.vidu.com/docs/image-to-video.md>
    /// @see <https://platform.vidu.com/docs/reference-to-video.md>
    #[tokio::test]
    async fn vidu2_duration_snaps_to_a_length_the_endpoint_accepts() {
        let server = MockServer::start().await;
        mount_submit(&server, "img2video").await;
        mount_submit(&server, "reference2video").await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("vidu/vidu2.0");
        let base = make_base("a hummingbird in slow motion", "vidu/vidu2.0");
        // (ref role → endpoint, requested seconds, seconds Vidu must receive)
        let cases: [(&str, f64, i64); 3] = [("first_frame", 5.0, 4), ("first_frame", 7.0, 8), ("reference", 8.0, 4)];
        for &(role, requested, _) in &cases {
            let mut extras = make_extras();
            extras.duration_seconds = requested;
            let materialized = MaterializedRequest {
                refs: vec![url_ref(role, "https://img.test/a.png")],
                cleanup: Cleanup::empty(),
            };
            provider.generate(&schema, &base, &extras, &materialized).await.expect("generate");
        }

        let received = server.received_requests().await.unwrap();
        let sent: Vec<i64> = received
            .iter()
            .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap()["duration"].as_i64().unwrap())
            .collect();
        let expected: Vec<i64> = cases.iter().map(|c| c.2).collect();
        assert_eq!(sent, expected);
    }
}
