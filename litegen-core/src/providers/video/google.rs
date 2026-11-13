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

/// Google Veo video generation provider (Gemini Developer API).
///
/// Text-to-video and image-to-video (first/last keyframe + reference images).
/// Generation is asynchronous: the create call returns a long-running operation
/// `name`; poll the operation until `done`, then read the sample `video.uri`.
/// Authenticated with the same Google API key used by the image provider, sent
/// as the `x-goog-api-key` header.
///
/// NOTE: the returned `video.uri` must be downloaded with the API key (it is not
/// a public CDN URL). This provider returns the uri as `video_url`; re-hosting
/// auth-gated outputs to litegen storage is a tracked cross-provider follow-up
/// (shared with Bedrock Nova Reel).
///
/// @see <https://aistudio.google.com/docs/veo> — Veo generate + poll (2026-11-09;
///   `/docs/video` is now an Omni overview with no Veo samples)
///   Verbatim:
///     curl -s "${BASE_URL}/models/veo-3.1-generate-preview:predictLongRunning" \
///     -H "x-goog-api-key: $GEMINI_API_KEY" \
///     status_response=$(curl -s -H "x-goog-api-key: $GEMINI_API_KEY" "${BASE_URL}/${operation_name}")
pub struct GoogleVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl GoogleVideoProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (result downloads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(120), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::raw_header("x-goog-api-key"),
            client: Self::api_client(&Default::default()),
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://generativelanguage.googleapis.com/v1beta")
    }

    /// Resolve the per-request credentials, honoring the weighted key pool.
    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("google".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// Strip the `google/` prefix; the remainder is the native Veo model id.
    /// Veo's finished video, downloaded with the key: `video.uri` is a Files
    /// API link on Google's API host that answers only an authenticated call.
    /// The key goes only there (the configured base's origin, or Google's own
    /// host); a transient failure is asked again next tick.
    /// @see <https://aistudio.google.com/docs/veo> (2026-11-09)
    async fn download_video(&self, creds: &crate::providers::ProviderCredentials, uri: &str) -> Result<(Vec<u8>, String), ProviderError> {
        const MAX_VIDEO_BYTES: usize = 1024 * 1024 * 1024;
        let googles = reqwest::Url::parse(uri)
            .is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("generativelanguage.googleapis.com"));
        if !googles && crate::providers::model3d::fal::same_origin_url(self.api_base(), Some(uri)).is_none() {
            return Err(ProviderError::RequestFailed {
                message: format!("Veo named a video off Google's API host: {uri}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            });
        }
        let builder = crate::providers::auth::apply(&self.auth, creds, self.client.get(uri))?;
        let mut resp = builder.send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Veo video download failed: {}", e.without_url()),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Veo video download answered HTTP {}", status.as_u16()),
                status_code: Some(status.as_u16()),
                provider_error: None,
                retryable: status.is_server_error() || matches!(status.as_u16(), 408 | 429),
            });
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or(v).trim().to_string())
            .filter(|v| v.starts_with("video/"))
            .unwrap_or_else(|| "video/mp4".to_string());
        let mut bytes = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Veo video download broke off: {}", e.without_url()),
            status_code: None,
            provider_error: None,
            retryable: true,
        })? {
            if bytes.len() + chunk.len() > MAX_VIDEO_BYTES {
                return Err(ProviderError::RequestFailed {
                    message: format!("Veo video is over {MAX_VIDEO_BYTES} bytes"),
                    status_code: None,
                    provider_error: None,
                    retryable: false,
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((bytes, content_type))
    }

    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("google/").unwrap_or(model_id)
    }
}

impl Default for GoogleVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for GoogleVideoProvider {
    fn name(&self) -> &str {
        "google"
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

    /// Submit a generation via `POST {base}/models/{model}:predictLongRunning`,
    /// returning the operation `name` as the job handle.
    ///
    /// @see <https://aistudio.google.com/docs/veo> — `instances[].prompt`,
    ///   `instances[].image.inlineData`, `instances[].lastFrame`, and
    ///   `parameters.{aspectRatio,durationSeconds,resolution}`.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);
        let url = format!(
            "{}/models/{}:predictLongRunning",
            self.api_base(),
            crate::providers::wire_model(self.config.as_ref(), &model.id, native)
        );

        // Build the instance (prompt + optional keyframes as base64 inlineData).
        let mut instance = json!({ "prompt": base.prompt });
        let mut reference_images: Vec<Value> = Vec::new();
        for r in &materialized.refs {
            if let MaterializedRefForm::Base64(b64) = &r.form {
                // The `:predictLongRunning` instances take the Vertex-style
                // `Image` shape — `{bytesBase64Encoded, mimeType}`. `inlineData`
                // is the `generateContent` shape and is not understood here.
                // @see googleapis/python-genai `_Image_to_mldev`
                let img = json!({ "bytesBase64Encoded": b64, "mimeType": "image/png" });
                match r.role.as_str() {
                    "first_frame" | "init" => instance["image"] = img,
                    "last_frame" => instance["lastFrame"] = img,
                    // Veo 3.1 reference images.
                    // @see `_GenerateVideosConfig_to_mldev` → instances[0].referenceImages
                    "reference" => reference_images.push(img),
                    _ => {}
                }
            }
        }
        if !reference_images.is_empty() {
            instance["referenceImages"] = Value::Array(reference_images);
        }

        // Build the parameters block.
        let mut parameters = json!({});
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            parameters["aspectRatio"] = Value::String(ar.to_string());
        }
        if extras.duration_seconds > 0.0 {
            parameters["durationSeconds"] = Value::Number((extras.duration_seconds as i64).into());
        }
        if let Some(res) = extras.resolution.as_deref() {
            parameters["resolution"] = Value::String(res.to_string());
        }
        if let Some(np) = base.negative_prompt.as_deref() {
            parameters["negativePrompt"] = Value::String(np.to_string());
        }
        // `extra` adds Veo's own parameters (personGeneration); it cannot
        // replace what litegen sets (the billed length, shape and tier), nor
        // ask for more clips than litegen returns and bills (rule 11, N1).
        if let Some(obj) = parameters.as_object_mut() {
            let owned = |k: &str| matches!(k, "aspectRatio" | "durationSeconds" | "resolution" | "negativePrompt" | "sampleCount" | "numberOfVideos");
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), owned) {
                obj.insert(k.clone(), v.clone());
            }
        }

        let body = json!({ "instances": [instance], "parameters": parameters });

        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Google Veo request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: crate::providers::may_resend_after(&e, true),
            })?;

        // The status first (a gateway's 5xx page is retried), and a 429's
        // `Retry-After` reaches the retry policy.
        let resp_json = crate::providers::read_vendor_json(resp, "Google Veo").await?;

        let name = resp_json["name"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Google Veo response missing operation name".to_string(),
            status_code: None,
            provider_error: Some(resp_json.clone()),
            retryable: false,
        })?;

        Ok(VideoGenerationHandle {
            provider_job_id: name.to_string(),
            provider: "google".to_string(),
            model: model.id.clone(),
        })
    }

    /// Poll the long-running operation via `GET {base}/{operation_name}`.
    ///
    /// @see <https://aistudio.google.com/docs/veo> — operation `done` +
    ///   `response.generateVideoResponse.generatedSamples[0].video.uri`.
    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        self.poll_status_gated(handle, &crate::providers::AlwaysFetch).await
    }

    /// The video is downloaded only once `gate` says this observer keeps it.
    async fn poll_status_gated(
        &self,
        handle: &VideoGenerationHandle,
        gate: &dyn crate::providers::FetchGate,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/{}", self.api_base(), handle.provider_job_id);

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

        let data = super::read_poll_json(resp, "google").await?;

        // Operation-level error → failed.
        if let Some(err) = data["error"]["message"].as_str() {
            return Ok(VideoGenerationPollResult {
                status: GenerationStatus::Failed,
                progress: 0,
                video_url: None,
                video_data: None,
                content_type: Some("video/mp4".into()),
                error: Some(err.to_string()),
                metadata: HashMap::new(),
            });
        }

        // A long-running operation: `done` false is still running.
        if !data["done"].as_bool().unwrap_or(false) {
            return Ok(VideoGenerationPollResult {
                status: GenerationStatus::Processing,
                progress: 50,
                video_url: None,
                video_data: None,
                content_type: None,
                error: None,
                metadata: HashMap::new(),
            });
        }
        let answer = &data["response"]["generateVideoResponse"];
        let Some(uri) = answer["generatedSamples"][0]["video"]["uri"].as_str() else {
            // Finished with nothing to show: say why, in Google's words.
            let why = answer["raiMediaFilteredReasons"]
                .as_array()
                .map(|r| r.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; "))
                .filter(|r| !r.is_empty())
                .unwrap_or_else(|| "Veo finished without a video".to_string());
            return Ok(VideoGenerationPollResult {
                status: GenerationStatus::Failed,
                progress: 100,
                video_url: None,
                video_data: None,
                content_type: None,
                error: Some(why),
                metadata: HashMap::new(),
            });
        };
        // `video.uri` answers only a call with the key, so litegen could never
        // keep it later: the video is downloaded now, and only its bytes go on.
        // The link is the file's own, so it holds however long the gate took.
        if gate.ask().await == crate::providers::Fetch::StandAside {
            return Ok(super::standing_aside());
        }
        let (bytes, content_type) = self.download_video(&creds, uri).await?;
        Ok(VideoGenerationPollResult {
            status: GenerationStatus::Completed,
            progress: 100,
            video_url: None,
            video_data: Some(bytes),
            content_type: Some(content_type),
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

    /// Validate the API key via `GET {generativelanguage}/v1beta/models`.
    /// Veo's `video.uri` answers only a call with the key: the poll downloads it, so there is no link to hand out.
    fn hands_out_links(&self, _model: &ModelSchema) -> bool {
        false
    }

    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Google video provider not configured".into(),
                latency_ms: None,
            };
        }
        let start = std::time::Instant::now();
        let creds = match self.creds() {
            Ok(c) => c,
            Err(_) => {
                return HealthCheckResult { healthy: false, message: "No API key".into(), latency_ms: None }
            }
        };
        let builder = match crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.get(format!("{}/models", self.api_base())).timeout(std::time::Duration::from_secs(10)),
        ) {
            Ok(b) => b,
            Err(e) => {
                return HealthCheckResult { healthy: false, message: e.to_string(), latency_ms: None }
            }
        };
        match builder.send().await {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "Google API key valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("Google returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("Google health check failed: {e}"),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::{Cleanup, MaterializedRef};
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// The Gemini API's Veo 3.1 previews left the catalog at their shutdown
    /// (2026-10-22); their last catalog rows are kept as test fixtures.
    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let yaml = include_str!("../../../tests/fixtures/retired-models.yaml");
        let r = crate::capabilities::CapabilityRegistry::from_yaml_strs(&[("retired-models.yaml", yaml)]).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> GoogleVideoProvider {
        let mut p = GoogleVideoProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_key: "test-key".to_string(),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        cfg.credentials.api_key = Some("test-key".to_string());
        p.configure(cfg);
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
            duration_seconds: 8.0,
            aspect_ratio: Some("16:9".to_string()),
            resolution: Some("720p".to_string()),
            fps: None,
            extra: None,
            camera: None, audio: None, r#loop: None,
            source: None, direction: None, time_range: None, replace: None,
        }
    }

    fn materialized_first_frame(b64: &str) -> MaterializedRequest {
        MaterializedRequest {
            refs: vec![MaterializedRef {
                role: "first_frame".to_string(),
                form: MaterializedRefForm::Base64(b64.to_string()),
                time_s: None,
            }],
            cleanup: Cleanup::empty(),
        }
    }

    #[tokio::test]
    async fn submits_veo_job_and_sends_api_key_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/models/veo-3\.1-generate-preview:predictLongRunning"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "models/veo-3.1-generate-preview/operations/abc123"
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&format!("{}/v1beta", server.uri()));
        let schema = ref_schema("google/veo-3.1-generate-preview");
        let base = make_base("a cat surfing a wave", "google/veo-3.1-generate-preview");
        let extras = make_extras();
        let materialized = materialized_first_frame("Zm9vYmFy");

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let handle = result.unwrap();
        assert_eq!(handle.provider_job_id, "models/veo-3.1-generate-preview/operations/abc123");
        assert_eq!(handle.provider, "google");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        // Auth header present.
        assert_eq!(received[0].headers.get("x-goog-api-key").unwrap(), "test-key");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["instances"][0]["prompt"], "a cat surfing a wave");
        assert_eq!(body["instances"][0]["image"]["bytesBase64Encoded"], "Zm9vYmFy");
        assert_eq!(body["instances"][0]["image"]["mimeType"], "image/png");
        assert_eq!(body["parameters"]["aspectRatio"], "16:9");
        assert_eq!(body["parameters"]["durationSeconds"], 8);
        assert_eq!(body["parameters"]["resolution"], "720p");
    }

    /// N1 (rule 11): `extra` adds Veo's own parameters but cannot replace
    /// what litegen sets (the billed length, shape and tier) or ask for more
    /// clips than litegen returns and bills; and E4a: where the clip is
    /// stored (`storageUri`) is litegen's.
    #[tokio::test]
    async fn extra_cannot_replace_what_litegen_sets_or_say_where_the_clip_goes() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "name": "ops/1" }))).mount(&server).await;
        let mut extras = make_extras();
        extras.extra = Some(json!({
            "durationSeconds": 4, "aspectRatio": "9:16", "resolution": "1080p", "sampleCount": 4,
            "storageUri": "gs://elsewhere/out", "personGeneration": "allow_adult"
        }));
        let schema = ref_schema("google/veo-3.1-generate-preview");
        let base = make_base("a cat surfing a wave", "google/veo-3.1-generate-preview");
        make_provider(&format!("{}/v1beta", server.uri())).generate(&schema, &base, &extras, &materialized_first_frame("Zm9vYmFy")).await.unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        let p = &body["parameters"];
        assert_eq!((&p["durationSeconds"], &p["aspectRatio"], &p["resolution"]), (&json!(8), &json!("16:9"), &json!("720p")), "{p}");
        assert!(p.get("sampleCount").is_none() && p.get("storageUri").is_none(), "{p}");
        assert_eq!(p["personGeneration"], "allow_adult", "{p}");
    }

    /// `lastFrame` takes the same `Image` shape as `image`. `_Image_to_mldev`
    /// in googleapis/python-genai emits `{bytesBase64Encoded, mimeType}` for the
    /// Gemini Developer API; `inlineData` is the `generateContent` shape and is
    /// not understood by `:predictLongRunning`.
    #[tokio::test]
    async fn last_frame_uses_bytes_base64_encoded_shape() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/models/veo-3\.1-generate-preview:predictLongRunning"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "name": "ops/1" })))
            .mount(&server)
            .await;

        let provider = make_provider(&format!("{}/v1beta", server.uri()));
        let schema = ref_schema("google/veo-3.1-generate-preview");
        let base = make_base("a slow dissolve", "google/veo-3.1-generate-preview");
        let materialized = MaterializedRequest {
            refs: vec![MaterializedRef {
                role: "last_frame".to_string(),
                form: MaterializedRefForm::Base64("bGFzdA==".to_string()),
                time_s: None,
            }],
            cleanup: Cleanup::empty(),
        };

        provider
            .generate(&schema, &base, &make_extras(), &materialized)
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["instances"][0]["lastFrame"]["bytesBase64Encoded"], "bGFzdA==");
        assert_eq!(body["instances"][0]["lastFrame"]["mimeType"], "image/png");
        assert!(
            body["instances"][0]["lastFrame"].get("inlineData").is_none(),
            "inlineData is the generateContent shape, not the predict shape"
        );
    }

    /// Veo 2.0 / 3.0 / 3.0-fast were shut down on 2026-06-30, and the Veo 3.1
    /// previews on 2026-10-22.
    /// @see <https://aistudio.google.com/docs/deprecations> (2026-11-09)
    #[test]
    fn catalog_ships_no_retired_veo_models() {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        for dead in [
            "google/veo-2.0-generate-001",
            "google/veo-3.0-generate-001",
            "google/veo-3.0-fast-generate-001",
            "google/veo-3.1-generate-preview",
            "google/veo-3.1-fast-generate-preview",
            "google/veo-3.1-lite-generate-preview",
        ] {
            assert!(
                r.get(dead).is_none(),
                "{dead} was shut down and must not be advertised"
            );
        }
    }

    fn operation(done: bool, uri: Option<&str>) -> Value {
        let mut op = json!({ "name": "models/veo-3.0-generate-001/operations/op1", "done": done });
        if let Some(u) = uri {
            op["response"] = json!({ "generateVideoResponse": { "generatedSamples": [{ "video": { "uri": u } }] } });
        }
        op
    }

    fn handle() -> VideoGenerationHandle {
        VideoGenerationHandle {
            provider_job_id: "models/veo-3.0-generate-001/operations/op1".into(),
            provider: "google".into(),
            model: "google/veo-3.0-generate-001".into(),
        }
    }

    /// Veo's `video.uri` answers only a call with the key, so litegen could
    /// never re-host it: the poll downloads the video then, with the key, and
    /// hands the bytes on (known-gaps: "a video URL that needs the vendor's
    /// key").
    #[tokio::test]
    async fn a_finished_video_is_downloaded_with_the_key() {
        let server = MockServer::start().await;
        let uri = format!("{}/v1beta/files/abc:download?alt=media", server.uri());
        Mock::given(method("GET"))
            .and(wiremock::matchers::path("/v1beta/files/abc:download"))
            .and(wiremock::matchers::header("x-goog-api-key", "test-key"))
            .respond_with(ResponseTemplate::new(200).insert_header("content-type", "video/mp4").set_body_bytes(b"MP4BYTES".to_vec()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(operation(true, Some(&uri))))
            .mount(&server)
            .await;
        let poll = make_provider(&server.uri()).poll_status(&handle()).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_data.as_deref(), Some(b"MP4BYTES".as_slice()));
        assert!(poll.video_url.is_none(), "the key-gated link is never handed on");
    }

    /// A gate that always says another observer keeps the video.
    struct Refuse;

    #[async_trait::async_trait]
    impl crate::providers::FetchGate for Refuse {
        async fn ask(&self) -> crate::providers::Fetch {
            crate::providers::Fetch::StandAside
        }
    }

    /// Review 2026-11-11 P4: an observer that does not keep the video
    /// downloads none of it.
    #[tokio::test]
    async fn a_gate_that_says_stand_aside_downloads_nothing() {
        let server = MockServer::start().await;
        let uri = format!("{}/v1beta/files/abc:download?alt=media", server.uri());
        Mock::given(method("GET"))
            .and(wiremock::matchers::path("/v1beta/files/abc:download"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"MP4BYTES".to_vec()))
            .mount(&server)
            .await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(operation(true, Some(&uri)))).mount(&server).await;
        let poll = make_provider(&server.uri()).poll_status_gated(&handle(), &Refuse).await.unwrap();
        assert_eq!((poll.status, poll.video_data.is_none(), poll.error.is_none()), (GenerationStatus::Processing, true, true));
        assert!(poll.progress < 100);
        let downloads = server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == "/v1beta/files/abc:download").count();
        assert_eq!(downloads, 0);
    }

    /// The key goes only to Google's API host: a video named anywhere else is
    /// not fetched with it.
    #[tokio::test]
    async fn a_video_named_off_googles_host_is_not_fetched_with_the_key() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(operation(true, Some("https://elsewhere.example/v.mp4"))))
            .mount(&server)
            .await;
        let err = make_provider(&server.uri()).poll_status(&handle()).await.unwrap_err();
        assert!(!err.is_retryable() && err.to_string().contains("elsewhere.example"), "{err}");
    }

    /// A run that ends with no video fails with Google's reason, not silently.
    #[tokio::test]
    async fn a_finished_run_with_no_video_fails_with_googles_reason() {
        let server = MockServer::start().await;
        let mut op = operation(true, None);
        op["response"] = json!({ "generateVideoResponse": { "raiMediaFilteredCount": 1, "raiMediaFilteredReasons": ["the prompt was filtered"] } });
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(op)).mount(&server).await;
        let poll = make_provider(&server.uri()).poll_status(&handle()).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Failed);
        assert!(poll.error.as_deref().is_some_and(|e| e.contains("filtered")), "{:?}", poll.error);
    }
}
