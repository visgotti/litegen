// OpenAI Sora video generation provider
// API Reference: https://developers.openai.com/api/reference/resources/videos/methods/create
// Guide:         https://developers.openai.com/api/docs/guides/video-generation
// API: POST /v1/videos (create) ; GET /v1/videos/{id} (poll) ; GET /v1/videos/{id}/content (download MP4)
// Models: sora-2, sora-2-pro
// Request fields: model, prompt, seconds (4|8|12), size (e.g. 1280x720); image-to-video
//   uploads the first frame as a multipart `input_reference` part.

use crate::providers::auth::Authorize;
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use reqwest::Client;
use serde_json::json;
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRequest, MaterializedRefForm};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig,
    VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// OpenAI Sora video generation provider.
///
/// Supports text-to-video and image-to-video (Sora 2) via the OpenAI Videos API.
///
/// @see <https://developers.openai.com/api/reference/resources/videos/methods/create> — Videos: create (`POST /v1/videos`)
/// @see <https://developers.openai.com/api/docs/guides/video-generation> — video generation guide
pub struct OpenAiVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
    /// How every request to the vendor's API is authenticated.
    auth: crate::providers::auth::AuthSpec,
}

impl OpenAiVideoProvider {
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
            .unwrap_or("https://api.openai.com/v1")
    }

    fn api_key(&self) -> Result<String, ProviderError> {
        if let Some(pool) = &self.key_pool {
            return Ok(pool.next().to_string());
        }
        self.config
            .as_ref()
            .map(|c| c.api_key.clone())
            .ok_or_else(|| ProviderError::NotConfigured("openai".into()))
    }

    /// Map internal model IDs to OpenAI Sora API model names (`sora-2`, `sora-2-pro`).
    /// @see <https://developers.openai.com/api/reference/resources/videos/methods/create> — `model` field
    fn resolve_model(&self, model: &str) -> String {
        if let Some(cfg) = &self.config {
            if let Some(mapped) = cfg.mapped_model(model) {
                return mapped.to_string();
            }
        }
        match model {
            "openai/sora-2-pro" | "openai/sora-pro" | "sora-2-pro" => "sora-2-pro".to_string(),
            "openai/sora" | "openai/sora-2" | "openai-sora" | "sora" | "sora-2" => "sora-2".to_string(),
            other => other.to_string(),
        }
    }

    /// Clamp an arbitrary requested duration to a Sora-supported clip length.
    /// Sora accepts only `seconds` ∈ {4, 8, 12}.
    /// @see <https://developers.openai.com/api/reference/resources/videos/methods/create> — `seconds`
    fn resolve_seconds(duration: f64) -> u64 {
        if duration <= 0.0 {
            return 4;
        }
        [4u64, 8, 12]
            .into_iter()
            .min_by_key(|s| ((*s as f64 - duration).abs() * 1000.0) as u64)
            .unwrap_or(4)
    }

    /// Resolve the Sora output `size` ("WIDTHxHEIGHT") from the requested
    /// aspect ratio / resolution. Sora-2 sizes: 720x1280, 1280x720; Sora-2-pro
    /// additionally supports 1024x1792, 1792x1024.
    /// @see <https://developers.openai.com/api/reference/resources/videos/methods/create> — `size`
    fn resolve_size(extras: &VideoExtras, is_pro: bool) -> &'static str {
        let portrait = extras
            .aspect_ratio
            .as_deref()
            .map(|ar| {
                // portrait if height > width (e.g. "9:16", "3:4")
                let parts: Vec<&str> = ar.split(':').collect();
                parts.len() == 2
                    && parts[0].parse::<f64>().ok().zip(parts[1].parse::<f64>().ok())
                        .is_some_and(|(w, h)| h > w)
            })
            .unwrap_or(false);
        let hi = is_pro
            && extras
                .resolution
                .as_deref()
                .is_some_and(|r| r.contains("1080") || r.contains("1792") || r.contains("hi"));
        match (portrait, hi) {
            (true, true) => "1024x1792",
            (false, true) => "1792x1024",
            (true, false) => "720x1280",
            (false, false) => "1280x720",
        }
    }
}

impl Default for OpenAiVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for OpenAiVideoProvider {
    fn name(&self) -> &str {
        "openai-video"
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

    /// Submit a video generation job (`POST {api_base}/videos`).
    ///
    /// @see <https://developers.openai.com/api/reference/resources/videos/methods/create> — Videos: create.
    ///   Proves the `model`/`prompt`/`seconds`/`size` request fields and the job object (`id`, `status`)
    ///   response. Text-to-video sends a JSON body; image-to-video uploads the first frame as a
    ///   multipart `input_reference` part.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let api_key = self.api_key()?;
        let model_name = self.resolve_model(&model.id);
        let is_pro = model_name.contains("pro");
        let url = format!("{}/videos", self.api_base());

        // Sora accepts only seconds ∈ {4,8,12} and a fixed set of WxH sizes.
        let seconds = Self::resolve_seconds(extras.duration_seconds);
        let size = Self::resolve_size(extras, is_pro);

        // First-frame reference for image-to-video. Sora wants the raw bytes as a
        // multipart `input_reference` part (not a URL/data-URI field), so we only
        // use base64-form refs here.
        let init_bytes: Option<Vec<u8>> = materialized.refs.iter().find_map(|r| {
            if r.role == "init" || r.role == "first_frame" {
                if let MaterializedRefForm::Base64(b64) = &r.form {
                    return B64.decode(b64).ok();
                }
            }
            None
        });

        let request = if let Some(bytes) = init_bytes {
            // Image-to-video: multipart form with the reference frame.
            let part = reqwest::multipart::Part::bytes(bytes)
                .file_name("input_reference.png")
                .mime_str("image/png")
                .unwrap_or_else(|_| reqwest::multipart::Part::bytes(Vec::new()));
            let form = reqwest::multipart::Form::new()
                .text("model", model_name.clone())
                .text("prompt", base.prompt.clone())
                .text("seconds", seconds.to_string())
                .text("size", size.to_string())
                .part("input_reference", part);
            self.client
                .post(&url)
                .authorized(&self.auth, &api_key)?
                .multipart(form)
        } else {
            // Text-to-video: JSON body.
            // `CreateVideoJsonBody.seconds` is `VideoSeconds`, a *string* enum
            // ("4" | "8" | "12") — not a number. The multipart branch above
            // stringifies via `.text(...)`; this branch has to do it explicitly.
            let mut body = json!({
                "model": model_name,
                "prompt": base.prompt,
                "seconds": seconds.to_string(),
                "size": size,
            });
            // `extra` cannot replace what litegen sends: the model it bills and
            // the length and size it prices (rule 11, N1).
            if let Some(body_obj) = body.as_object_mut() {
                let owned = |k: &str| matches!(k, "model" | "prompt" | "seconds" | "size" | "input_reference");
                for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), owned) {
                    body_obj.insert(k.clone(), v.clone());
                }
            }
            self.client
                .post(&url)
                .authorized(&self.auth, &api_key)?
                .header("Content-Type", "application/json")
                .json(&body)
        };

        let resp = crate::providers::inject_trace_headers(request)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Sora request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: crate::providers::may_resend_after(&e, true),
            })?;

        // The status first (a gateway's 5xx page is retried), and a 429's
        // `Retry-After` reaches the retry policy.
        let resp_json = crate::providers::read_vendor_json(resp, "Sora").await?;

        let job_id = resp_json["id"]
            .as_str()
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "Sora response missing id".to_string(),
                status_code: None,
                provider_error: Some(resp_json.clone()),
                retryable: false,
            })?
            .to_string();

        Ok(VideoGenerationHandle {
            provider_job_id: job_id,
            provider: "openai".to_string(),
            model: model.id.clone(),
        })
    }

    /// Poll the job (`GET {api_base}/videos/{id}`). When complete, the finished
    /// MP4 is downloaded from `GET {api_base}/videos/{id}/content`, which is the
    /// URL surfaced as `video_url` (it requires the same bearer auth to fetch).
    ///
    /// @see <https://developers.openai.com/api/reference/resources/videos/methods/retrieve> — Videos: retrieve
    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        self.poll_status_gated(handle, &crate::providers::AlwaysFetch).await
    }

    /// The MP4 is downloaded only once `gate` says this observer keeps it.
    async fn poll_status_gated(
        &self,
        handle: &VideoGenerationHandle,
        gate: &dyn crate::providers::FetchGate,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let api_key = self.api_key()?;

        // GET /v1/videos/{id}
        let resp = crate::providers::inject_trace_headers(
            self.client
                .get(format!(
                    "{}/videos/{}",
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

        let data = super::read_poll_json(resp, "openai").await?;

        // Sora job status: queued | in_progress | completed | failed
        let status = match data["status"].as_str() {
            Some("completed") | Some("succeeded") => GenerationStatus::Completed,
            Some("failed") => GenerationStatus::Failed,
            Some("in_progress") | Some("processing") => GenerationStatus::Processing,
            Some("queued") => GenerationStatus::Pending,
            other => return Ok(super::unknown_state("openai", other)),
        };

        // On completion the MP4 lives at the /content sub-resource, which is an
        // auth-gated OpenAI REST endpoint (NOT a public/pre-signed URL): it must be
        // fetched with the same `Authorization: Bearer` header as every other call.
        // Handing the bare URL to the customer leaves them unable to download it, so
        // we download the bytes here and surface them as `video_data`.
        let content_url = format!(
            "{}/videos/{}/content",
            self.api_base(),
            handle.provider_job_id
        );
        let mut video_url = None;
        let mut video_data = None;
        if status == GenerationStatus::Completed {
            if gate.ask().await == crate::providers::Fetch::StandAside {
                return Ok(super::standing_aside());
            }
            let mut content_resp = crate::providers::inject_trace_headers(
                self.client
                    .get(&content_url)
                    .authorized(&self.auth, &api_key)?,
            )
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to fetch Sora video content: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: crate::providers::may_resend_after(&e, false),
            })?;
            if !content_resp.status().is_success() {
                return Err(ProviderError::RequestFailed {
                    message: format!(
                        "Sora video content returned HTTP {}",
                        content_resp.status()
                    ),
                    status_code: Some(content_resp.status().as_u16()),
                    provider_error: None,
                    retryable: content_resp.status().is_server_error() || matches!(content_resp.status().as_u16(), 408 | 429),
                });
            }
            // At most a video's cap, as Veo's and Bedrock's downloads: a
            // declared length past it is refused before reading, and the body
            // is counted as it arrives. Read into one buffer of the declared
            // length, as `download_capped` reads theirs, so it never grows
            // and is copied (payloads review #5).
            const MAX_VIDEO_BYTES: u64 = 1024 * 1024 * 1024;
            let too_large = || ProviderError::RequestFailed {
                message: format!("Sora video is over {MAX_VIDEO_BYTES} bytes"),
                status_code: None,
                provider_error: None,
                retryable: false,
            };
            if content_resp.content_length().is_some_and(|n| n > MAX_VIDEO_BYTES) {
                return Err(too_large());
            }
            let mut bytes = Vec::with_capacity(content_resp.content_length().unwrap_or(0) as usize);
            while let Some(chunk) = content_resp.chunk().await.map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to read Sora video bytes: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })? {
                if (bytes.len() + chunk.len()) as u64 > MAX_VIDEO_BYTES {
                    return Err(too_large());
                }
                bytes.extend_from_slice(&chunk);
            }
            video_data = Some(bytes);
            video_url = Some(content_url);
        }

        let error = data["error"]
            .as_str()
            .or_else(|| data["error"]["message"].as_str())
            .map(String::from);

        Ok(VideoGenerationPollResult {
            status,
            progress: match status {
                GenerationStatus::Completed => 100,
                GenerationStatus::Processing => 60,
                _ => 10,
            },
            video_url,
            video_data,
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

    /// Validate the API key via `GET {api_base}/models`.
    ///
    /// @see <https://developers.openai.com/api/reference/resources/models/methods/list> — Models: list (2026-11-09)
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "OpenAI Video provider not configured".into(),
                latency_ms: None,
            };
        }
        // Validate key via GET /v1/models
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
        let request = match self.client.get(format!("{}/models", self.api_base())).authorized(&self.auth, &api_key) {
            Ok(request) => request,
            Err(e) => return HealthCheckResult { healthy: false, message: format!("OpenAI health check failed: {e}"), latency_ms: None },
        };
        match request.timeout(std::time::Duration::from_secs(10)).send().await {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "OpenAI API key valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("OpenAI returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("OpenAI health check failed: {e}"),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::{method, path};

    /// Sora left the catalog when OpenAI shut it down (2026-09-24); its last catalog row is kept as a test fixture.
    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let yaml = include_str!("../../../tests/fixtures/retired-models.yaml");
        let r = crate::capabilities::CapabilityRegistry::from_yaml_strs(&[("retired-models.yaml", yaml)]).expect("load");
        r.get(id).expect("model").clone()
    }

    fn empty_materialized() -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: vec![],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    fn make_provider(api_base: &str) -> OpenAiVideoProvider {
        let mut p = OpenAiVideoProvider::new();
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

    /// A gate that always says another observer keeps the video.
    struct Refuse;

    #[async_trait::async_trait]
    impl crate::providers::FetchGate for Refuse {
        async fn ask(&self) -> crate::providers::Fetch {
            crate::providers::Fetch::StandAside
        }
    }

    fn sora_handle() -> VideoGenerationHandle {
        VideoGenerationHandle { provider_job_id: "video_1".into(), provider: "openai".into(), model: "openai/sora-2".into() }
    }

    /// Review 2026-11-11 P4: an observer that does not keep the video
    /// downloads none of it.
    #[tokio::test]
    async fn a_gate_that_says_stand_aside_downloads_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/videos/video_1/content"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"MP4BYTES".to_vec()))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/videos/video_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": "video_1", "status": "completed" })))
            .mount(&server).await;
        let poll = make_provider(&server.uri()).poll_status_gated(&sora_handle(), &Refuse).await.unwrap();
        assert_eq!((poll.status, poll.video_data.is_none(), poll.error.is_none()), (GenerationStatus::Processing, true, true));
        let content = server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == "/videos/video_1/content").count();
        assert_eq!(content, 0);
    }

    /// Review 2026-11-11 P4, NEW 1: Sora's video is capped at 1 GiB as Veo's
    /// and Bedrock's are. One that says it is larger is refused before any of
    /// it is read, not read to the end into memory.
    #[tokio::test]
    async fn a_sora_video_over_a_gib_is_refused_before_it_is_read() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut seen = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        let Ok(n) = sock.read(&mut buf).await else { return };
                        if n == 0 {
                            return;
                        }
                        seen.extend_from_slice(&buf[..n]);
                        let Some(end) = seen.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                        let head = String::from_utf8_lossy(&seen[..end]).to_string();
                        seen.drain(..end + 4);
                        if head.starts_with("GET /videos/video_1/content") {
                            let big = format!("HTTP/1.1 200 OK\r\ncontent-type: video/mp4\r\ncontent-length: {}\r\n\r\n", 1024u64 * 1024 * 1024 + 1);
                            let _ = sock.write_all(big.as_bytes()).await;
                            let _ = sock.write_all(&[0u8; 65536]).await;
                            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                            return;
                        }
                        let body = r#"{"id":"video_1","status":"completed"}"#;
                        let ok = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len());
                        if sock.write_all(ok.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        let provider = make_provider(&format!("http://{addr}"));
        let got = tokio::time::timeout(std::time::Duration::from_secs(10), provider.poll_status(&sora_handle()))
            .await
            .expect("a video over the cap is refused at once, not read to the end");
        let err = got.expect_err("over the cap");
        assert!(err.to_string().contains("over 1073741824 bytes") && !err.is_retryable(), "{err}");
    }

    /// Payloads review #5: a Sora video is read into one buffer of the
    /// length the vendor declared, as `download_capped` reads Veo's and
    /// Bedrock's. Grown as it arrives, the buffer doubled and was copied at
    /// each step, and ended up to twice the video's size.
    #[tokio::test]
    async fn a_sora_video_is_read_into_a_buffer_of_its_declared_length() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/videos/video_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "video_1", "status": "completed" })))
            .mount(&server)
            .await;
        let video: Vec<u8> = (0..3 * 1024 * 1024 + 1).map(|i| (i % 251) as u8).collect();
        Mock::given(method("GET"))
            .and(path("/videos/video_1/content"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(video.clone(), "video/mp4"))
            .mount(&server)
            .await;
        let polled = make_provider(&server.uri()).poll_status(&sora_handle()).await.unwrap();
        let data = polled.video_data.expect("the video's bytes");
        assert!(data == video, "the video arrived whole ({} of {} bytes)", data.len(), video.len());
        assert_eq!(data.capacity(), video.len(), "the buffer is sized from the declared length");
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
    async fn submits_sora_job_and_returns_handle() {
        let server = MockServer::start().await;

        // Real Sora create endpoint: POST /v1/videos (NOT /v1/videos/generations)
        Mock::given(method("POST"))
            .and(path("/v1/videos"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "sora_job_1",
                "status": "queued",
                "model": "sora-2"
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&format!("{}/v1", server.uri()));
        let schema = ref_schema("openai/sora");
        let base = make_base("a spaceship launching from Earth", "openai/sora");
        let extras = make_extras();
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let handle = result.unwrap();

        assert_eq!(handle.provider_job_id, "sora_job_1");
        assert_eq!(handle.provider, "openai");
        assert_eq!(handle.model, "openai/sora");

        // Verify outbound request body uses the real Sora fields.
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "sora-2");
        assert_eq!(body["prompt"], "a spaceship launching from Earth");
        // duration 5.0 → nearest supported clip length (4s); size defaults to landscape.
        // VideoSeconds is a string enum ("4" | "8" | "12"), not a number.
        assert_eq!(body["seconds"], json!("4"));
        assert_eq!(body["size"], "1280x720");
    }

    /// N1 (rule 11): `extra` cannot replace what litegen sends, the model it
    /// bills and the length and size it prices included; other keys still go.
    #[tokio::test]
    async fn extra_cannot_replace_what_litegen_sends() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/videos"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "video_1", "status": "queued" })))
            .mount(&server)
            .await;
        let mut extras = make_extras();
        extras.extra = Some(json!({ "model": "sora-2-pro", "seconds": "12", "size": "1792x1024", "prompt": "other", "knob": 1 }));
        let base = make_base("a kite over dunes", "openai/sora");
        make_provider(&server.uri()).generate(&ref_schema("openai/sora"), &base, &extras, &empty_materialized()).await.expect("generate");
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        // What litegen sends with no `extra` (`submits_sora_job_and_returns_handle`):
        // the model it bills, the clip length it priced and the default size.
        assert_eq!(
            (&body["model"], &body["seconds"], &body["size"], &body["prompt"], &body["knob"]),
            (&json!("sora-2"), &json!("4"), &json!("1280x720"), &json!("a kite over dunes"), &json!(1)),
            "{body}"
        );
    }

    /// `CreateVideoJsonBody.seconds` is `$ref: VideoSeconds`, which is
    /// `{type: string, enum: ["4","8","12"]}`. The multipart path already
    /// stringifies; the JSON (text-to-video) path sent a bare number.
    /// @see openai/openai-openapi `VideoSeconds`
    #[tokio::test]
    async fn seconds_is_sent_as_a_string_in_the_json_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/videos"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "video_1", "status": "queued"
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("openai/sora");
        let base = make_base("a kite over dunes", "openai/sora");
        provider
            .generate(&schema, &base, &make_extras(), &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(
            body["seconds"],
            json!("4"),
            "seconds must be the string form, got {}",
            body["seconds"]
        );
    }
}
