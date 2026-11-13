use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::image::leonardo::{read_create, upload_init_image};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig,
    VideoExtras, VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;

/// Leonardo.Ai video generation provider (image-to-video / Motion / Veo 3.1 / Kling).
///
/// Async: `POST /generations-image-to-video` (a start frame `imageId` is
/// required) returns a generation id; poll `GET /generations/{id}` until
/// `generations_by_pk.status == "COMPLETE"`, then read
/// `generated_images[0].motionMP4URL`. Bearer auth; start frame uploaded via the
/// shared presigned init-image flow.
///
/// @see <https://docs.leonardo.ai/v1.0/reference/createimagetovideogeneration> (2026-10-30)
///   (required: prompt, imageId, imageType; optional: model [MOTION2, VEO3_1,
///   KLING2_5, …], resolution [RESOLUTION_720, …], duration)
/// @see <https://docs.leonardo.ai/v1.0/docs/veo-31>
///   Verbatim: "\"resolution\": \"RESOLUTION_1080\", \"duration\":8, ... \"model\": \"VEO3_1\""
pub struct LeonardoVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
    /// Start-image uploads to Leonardo's presigned storage URL, without the
    /// operator's extra_headers: a URL from the vendor's answer, so the
    /// client is public-only, follows no redirect and uses no proxy
    /// (SEC-LEONARDO).
    upload_client: Client,
    /// Whether the upload URL must be a public address. Only the tests,
    /// whose mock storage runs on loopback, turn it off.
    pub(crate) public_assets_only: bool,
}

/// What litegen itself sends to `POST /generations-image-to-video`; `extra`
/// cannot replace it (`imageId` and `imageType` in `extra` name the start
/// frame instead of an uploaded reference).
const OWNED_FIELDS: &[&str] = &["prompt", "imageId", "imageType", "model", "resolution", "duration", "seed", "negativePrompt"];

impl LeonardoVideoProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (result downloads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(180), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
            client: Self::api_client(&Default::default()),
            // The process's shared public-only client for this timeout, not
            // one per instance (P8).
            upload_client: crate::util::ssrf::public_only_client_with_timeout(std::time::Duration::from_secs(180)),
            public_assets_only: true,
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://cloud.leonardo.ai/api/rest/v1")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("leonardo".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// Map the litegen model id to a Leonardo motion model enum. Unknown ids
    /// fall back to MOTION2, so every catalog id needs its own arm.
    ///
    /// @see <https://docs.leonardo.ai/v1.0/docs/commonly-used-api-values> — Video Model IDs
    fn resolve_model(model_id: &str) -> Option<&'static str> {
        Some(match model_id.strip_prefix("leonardo/").unwrap_or(model_id) {
            "motion2" | "motion-2" => "MOTION2",
            "motion2-fast" => "MOTION2FAST",
            "veo3.1" | "veo3-1" => "VEO3_1",
            "veo3.1-fast" | "veo3-1-fast" => "VEO3_1FAST",
            // Retired 2026-06-29 and gone from the catalog; kept so a stale
            // model_mapping fails at the vendor instead of billing as MOTION2.
            "veo3" => "VEO3",
            "veo3-fast" => "VEO3FAST",
            // Retired 2026-09-15 and gone from the catalog, kept for the same reason.
            "kling2.1" | "kling-2.1" => "KLING2_1",
            "kling2.5" | "kling-2.5" => "KLING2_5",
            // An id with no arm is refused, not billed as MOTION2 (L13).
            _ => return None,
        })
    }

    /// Map a requested resolution to Leonardo's preset. With none requested,
    /// pick one the model accepts: Kling 2.1 Pro and Kling 2.5 Turbo take only
    /// `RESOLUTION_1080`, so a blanket 720 default made them fail.
    ///
    /// @see <https://docs.leonardo.ai/v1.0/docs/kling-2-1-pro> — "resolution ... Set to `RESOLUTION_1080`."
    /// @see <https://docs.leonardo.ai/v1.0/docs/kling-2-5-turbo> — "resolution ... Set to `RESOLUTION_1080`."
    fn resolve_resolution(leo_model: &str, res: Option<&str>) -> &'static str {
        match res {
            Some("1080p") | Some("1080P") => "RESOLUTION_1080",
            Some("480p") | Some("480P") => "RESOLUTION_480",
            Some(_) => "RESOLUTION_720",
            None if leo_model.starts_with("KLING") => "RESOLUTION_1080",
            None => "RESOLUTION_720",
        }
    }
}

impl Default for LeonardoVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for LeonardoVideoProvider {
    fn name(&self) -> &str {
        "leonardo"
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
        let leo_model = Self::resolve_model(&model.id).ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "leonardo".into() })?;

        // A start-frame image is required for image-to-video.
        let mut image_id: Option<String> = None;
        let mut image_type = "UPLOADED";
        for r in &materialized.refs {
            if let MaterializedRefForm::MultipartField { bytes, .. } = &r.form {
                image_id = Some(
                    upload_init_image(&self.client, &self.upload_client, self.public_assets_only, self.api_base(), &self.auth, &creds, bytes).await?,
                );
                break;
            }
        }
        // Allow passing a pre-existing generated/uploaded image id via extra.
        if image_id.is_none() {
            if let Some(Value::Object(map)) = &extras.extra {
                if let Some(id) = map.get("imageId").and_then(|v| v.as_str()) {
                    image_id = Some(id.to_string());
                    if let Some(t) = map.get("imageType").and_then(|v| v.as_str()) {
                        image_type = if t == "GENERATED" { "GENERATED" } else { "UPLOADED" };
                    }
                }
            }
        }
        let image_id = image_id.ok_or_else(|| {
            ProviderError::InvalidRequest("leonardo video requires a start-frame reference image".into())
        })?;

        let mut body = json!({
            "prompt": base.prompt,
            "imageId": image_id,
            "imageType": image_type,
            "model": crate::providers::wire_model(self.config.as_ref(), &model.id, leo_model),
            "resolution": Self::resolve_resolution(leo_model, extras.resolution.as_deref()),
        });
        // `duration` is documented for Veo and Kling only; Motion 2.0 (and
        // Motion 2.0 Fast) take no duration parameter at all.
        // @see <https://docs.leonardo.ai/v1.0/reference/createimagetovideogeneration>
        if extras.duration_seconds > 0.0 && !leo_model.starts_with("MOTION2") {
            body["duration"] = Value::Number((extras.duration_seconds as i64).into());
        }
        // Motion 2.0's `seed` and `negativePrompt` (the model list: "seed
        // {…maximum":2147483637}", "negative_prompt {…maxLength":1000}"); only
        // its row declares them, and no other model is sent either.
        // @see <https://docs.leonardo.ai/v1.0/reference/createimagetovideogeneration> (2026-11-03)
        if leo_model.starts_with("MOTION2") {
            if let Some(seed) = base.seed {
                body["seed"] = Value::Number(seed.into());
            }
            if let Some(np) = base.negative_prompt.as_deref() {
                body["negativePrompt"] = Value::String(np.to_string());
            }
        }
        if let Some(obj) = body.as_object_mut() {
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED_FIELDS.contains(&k)) {
                obj.insert(k.clone(), v.clone());
            }
        }

        let url = format!("{}/generations-image-to-video", self.api_base());
        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Leonardo video request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // Only a connection that never opened is safe to retry: a
                // request that timed out may have been accepted and billed.
                retryable: e.is_connect(),
            })?;
        let submit = read_create(resp, "Leonardo video").await?;

        // The generation id appears under one of these job keys depending on model.
        let gen_id = submit["motionVideoGenerationJob"]["generationId"]
            .as_str()
            .or_else(|| submit["sdGenerationJob"]["generationId"].as_str())
            .or_else(|| submit["motionSvdGenerationJob"]["generationId"].as_str())
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "Leonardo video response missing generationId".to_string(),
                status_code: None,
                provider_error: Some(submit.clone()),
                retryable: false,
            })?;

        Ok(VideoGenerationHandle {
            provider_job_id: gen_id.to_string(),
            provider: "leonardo".to_string(),
            model: model.id.clone(),
        })
    }

    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/generations/{}", self.api_base(), handle.provider_job_id);
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
        let data = super::read_poll_json(resp, "leonardo").await?;

        let pk = &data["generations_by_pk"];
        let mut status = match pk["status"].as_str() {
            Some("COMPLETE") => GenerationStatus::Completed,
            Some("FAILED") => GenerationStatus::Failed,
            // job_status: PENDING | COMPLETE | FAILED.
            Some("PENDING") => GenerationStatus::Processing,
            other => return Ok(super::unknown_state("leonardo", other)),
        };
        // The video is the generated image's `motionMP4URL` ("The URL of the
        // motion MP4"); its `url` is not a video, so a finished generation
        // without one fails.
        // @see <https://docs.leonardo.ai/v1.0/reference/getgenerationbyid> (2026-10-30)
        let video_url = pk["generated_images"][0]["motionMP4URL"].as_str().map(String::from);
        let error = match status {
            GenerationStatus::Failed => Some("leonardo generation failed (status FAILED)".to_string()),
            GenerationStatus::Completed if video_url.is_none() => {
                status = GenerationStatus::Failed;
                Some("leonardo generation finished with no motionMP4URL".to_string())
            }
            _ => None,
        };

        Ok(VideoGenerationPollResult {
            status,
            progress: match status {
                GenerationStatus::Completed => 100,
                GenerationStatus::Failed => 0,
                _ => 50,
            },
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
                message: "Leonardo video provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Leonardo video provider configured".into(), latency_ms: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::{Cleanup, MaterializedRef};
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// L13: every catalog row has its own arm, and an id with none is refused
    /// rather than billed as another model.
    #[test]
    fn every_catalog_row_resolves_and_an_unknown_id_does_not() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        for m in catalog.for_provider("leonardo").filter(|m| m.media_type == crate::capabilities::MediaType::Video) {
            assert!(LeonardoVideoProvider::resolve_model(&m.id).is_some(), "{} has no arm", m.id);
        }
        assert!(LeonardoVideoProvider::resolve_model("leonardo/not-a-model").is_none());
    }

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> LeonardoVideoProvider {
        let mut p = LeonardoVideoProvider::new();
        let mut cfg = ProviderInstanceConfig { api_key: "leo-key".to_string(), api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.api_key = Some("leo-key".to_string());
        p.configure(cfg);
        p
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(), model: model.to_string(), n: 1, negative_prompt: None,
            seed: None, reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    fn extras_with_image_id() -> VideoExtras {
        VideoExtras {
            duration_seconds: 8.0,
            aspect_ratio: None,
            resolution: Some("720p".to_string()),
            fps: None,
            // Provide a pre-existing imageId so the test skips the upload step.
            extra: Some(json!({ "imageId": "init-123", "imageType": "UPLOADED" })),
            camera: None, audio: None, r#loop: None,
            source: None, direction: None, time_range: None, replace: None,
        }
    }

    /// Veo 3.1 is `VEO3_1`; without its arm the id fell through to MOTION2
    /// and billed as a different model.
    /// @see <https://docs.leonardo.ai/v1.0/docs/veo-31> (2026-09-11)
    #[test]
    fn catalog_video_ids_resolve_to_their_own_leonardo_model() {
        assert_eq!(LeonardoVideoProvider::resolve_model("leonardo/veo3.1"), Some("VEO3_1"));
        assert_eq!(LeonardoVideoProvider::resolve_model("leonardo/kling2.5"), Some("KLING2_5"));
        assert_eq!(LeonardoVideoProvider::resolve_model("leonardo/kling2.1"), Some("KLING2_1"));
        assert_eq!(LeonardoVideoProvider::resolve_model("leonardo/motion2"), Some("MOTION2"));
    }

    /// Veo 3.1 Fast is `VEO3_1FAST` ("model ... Set to VEO3_1, VEO3_1FAST");
    /// without its arm the id fell through to MOTION2 and billed as a
    /// different model.
    /// @see <https://docs.leonardo.ai/v1.0/docs/veo-31> (2026-09-11)
    #[test]
    fn veo3_1_fast_resolves_to_veo3_1fast() {
        assert_eq!(LeonardoVideoProvider::resolve_model("leonardo/veo3.1-fast"), Some("VEO3_1FAST"));
    }

    /// The Veo 3.1 Fast submit carries its own model and a duration (Motion
    /// 2.0, the fallback, would have dropped the duration).
    /// @see <https://docs.leonardo.ai/v1.0/docs/veo-31> (2026-09-11)
    #[tokio::test]
    async fn veo3_1_fast_submits_veo3_1fast_with_duration() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/generations-image-to-video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "motionVideoGenerationJob": { "generationId": "leo-vid-5" }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("leonardo/veo3.1-fast");
        let base = make_base("a hummingbird hovering over a flower", "leonardo/veo3.1-fast");
        let mut extras = extras_with_image_id();
        extras.duration_seconds = 6.0;
        let materialized = MaterializedRequest { refs: vec![] as Vec<MaterializedRef>, cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "leo-vid-5");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "VEO3_1FAST");
        assert_eq!(body["resolution"], "RESOLUTION_720");
        assert_eq!(body["duration"], 6);
    }

    #[tokio::test]
    async fn submits_image_to_video_and_polls() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/generations-image-to-video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "motionVideoGenerationJob": { "generationId": "leo-vid-1", "apiCreditCost": 2500 }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/generations/leo-vid-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "generations_by_pk": { "status": "COMPLETE",
                    "generated_images": [{ "id": "i1", "motionMP4URL": "https://cdn.leo/v.mp4" }] }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("leonardo/veo3.1");
        let base = make_base("a slow zoom over mountains", "leonardo/veo3.1");
        let extras = extras_with_image_id();
        let materialized = MaterializedRequest { refs: vec![] as Vec<MaterializedRef>, cleanup: Cleanup::empty() };

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider_job_id, "leo-vid-1");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "VEO3_1");
        assert_eq!(body["imageId"], "init-123");
        assert_eq!(body["resolution"], "RESOLUTION_720");
        assert_eq!(body["duration"], 8);

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.video_url.unwrap(), "https://cdn.leo/v.mp4");
    }

    /// Leonardo, whose presign names an upload URL on `s3` (a loopback IP
    /// literal) that redirects to `s3`'s `/landed`, which would accept it.
    async fn leonardo_with_a_redirecting_upload() -> (MockServer, MockServer) {
        let server = MockServer::start().await;
        let s3 = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/init-image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "uploadInitImage": { "id": "init-1", "url": format!("{}/upload", s3.uri()), "fields": "{\"key\":\"k\"}" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/generations-image-to-video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "motionVideoGenerationJob": { "generationId": "leo-vid-1" } })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/upload"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", format!("{}/landed", s3.uri())))
            .mount(&s3)
            .await;
        Mock::given(path("/landed")).respond_with(ResponseTemplate::new(200)).mount(&s3).await;
        (server, s3)
    }

    fn one_start_frame() -> MaterializedRequest {
        MaterializedRequest {
            refs: vec![MaterializedRef {
                role: "start".into(),
                form: MaterializedRefForm::MultipartField {
                    field_name: "file".into(),
                    bytes: bytes::Bytes::from_static(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 16]),
                    content_type: "image/jpeg".into(),
                },
                time_s: None,
            }],
            cleanup: Cleanup::empty(),
        }
    }

    /// SEC-LEONARDO: the upload URL comes from Leonardo's answer, so the start
    /// frame goes only to a public address, and nothing is created when it
    /// can't.
    #[tokio::test]
    async fn a_start_frame_is_never_uploaded_to_an_internal_address() {
        let (server, s3) = leonardo_with_a_redirecting_upload().await;
        let mut extras = extras_with_image_id();
        extras.extra = None;
        let got = make_provider(&server.uri())
            .generate(&ref_schema("leonardo/veo3.1"), &make_base("x", "leonardo/veo3.1"), &extras, &one_start_frame())
            .await;
        let seen: Vec<String> = s3.received_requests().await.unwrap().iter().map(|r| format!("{} {}", r.method, r.url.path())).collect();
        assert!(seen.is_empty(), "the start frame went to an internal address: {seen:?}");
        let err = got.expect_err("refused before the upload");
        assert!(
            matches!(&err, ProviderError::RequestFailed { retryable: false, status_code: None, message, .. } if message.contains("upload URL")),
            "{err:?}"
        );
        let received = server.received_requests().await.unwrap();
        assert!(received.iter().all(|r| r.url.path() != "/generations-image-to-video"), "a job was created after a refused upload");
    }

    /// The upload client follows no redirect: the storage URL Leonardo named
    /// gets the upload, and a 3xx from it is a failed upload, never a second
    /// request to wherever it points.
    #[tokio::test]
    async fn an_upload_url_that_redirects_is_not_followed() {
        let (server, s3) = leonardo_with_a_redirecting_upload().await;
        let mut provider = make_provider(&server.uri());
        provider.public_assets_only = false;
        let mut extras = extras_with_image_id();
        extras.extra = None;
        let got = provider.generate(&ref_schema("leonardo/veo3.1"), &make_base("x", "leonardo/veo3.1"), &extras, &one_start_frame()).await;
        let seen: Vec<String> = s3.received_requests().await.unwrap().iter().map(|r| format!("{} {}", r.method, r.url.path())).collect();
        assert_eq!(seen, vec!["POST /upload".to_string()], "the redirect was followed");
        assert!(matches!(got, Err(ProviderError::RequestFailed { status_code: Some(302), retryable: false, .. })), "{got:?}");
        let received = server.received_requests().await.unwrap();
        assert!(received.iter().all(|r| r.url.path() != "/generations-image-to-video"), "a job was created after a failed upload");
    }

    /// Kling 2.5 Turbo takes `RESOLUTION_1080` ("resolution ... Set to
    /// RESOLUTION_1080"); a request that names no resolution used to go out
    /// as `RESOLUTION_720`.
    /// @see <https://docs.leonardo.ai/v1.0/docs/kling-2-5-turbo>
    #[tokio::test]
    async fn kling_without_a_resolution_sends_1080() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/generations-image-to-video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "motionVideoGenerationJob": { "generationId": "leo-vid-2" }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("leonardo/kling2.5");
        let base = make_base("a paper boat on a river", "leonardo/kling2.5");
        let mut extras = extras_with_image_id();
        extras.resolution = None;
        extras.duration_seconds = 5.0;
        let materialized = MaterializedRequest { refs: vec![] as Vec<MaterializedRef>, cleanup: Cleanup::empty() };

        provider.generate(&schema, &base, &extras, &materialized).await.unwrap();

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "KLING2_5");
        assert_eq!(body["resolution"], "RESOLUTION_1080");
        assert_eq!(body["duration"], 5);
    }

    /// Motion 2.0 has no duration parameter — the v1 reference documents
    /// `duration` for Veo and Kling only — so none is sent.
    /// @see <https://docs.leonardo.ai/v1.0/reference/createimagetovideogeneration>
    #[tokio::test]
    async fn motion2_sends_no_duration() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/generations-image-to-video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "motionVideoGenerationJob": { "generationId": "leo-vid-3" }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("leonardo/motion2");
        let base = make_base("leaves drifting in the wind", "leonardo/motion2");
        let mut extras = extras_with_image_id();
        extras.duration_seconds = 5.0;
        let materialized = MaterializedRequest { refs: vec![] as Vec<MaterializedRef>, cleanup: Cleanup::empty() };

        provider.generate(&schema, &base, &extras, &materialized).await.unwrap();

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "MOTION2");
        assert!(body.get("duration").is_none(), "Motion 2.0 got a duration: {body}");
    }

    /// Motion 2.0 takes a seed and a negative prompt: the v1 body's `seed` and
    /// `negativePrompt`, which Leonardo's model list gives `motion_2.0` as
    /// "seed {…"maximum":2147483637}" and "negative_prompt {…"maxLength":1000}".
    /// `extra` cannot replace either.
    /// @see <https://docs.leonardo.ai/v1.0/reference/createimagetovideogeneration> (2026-11-03)
    #[tokio::test]
    async fn motion2_sends_its_seed_and_negative_prompt() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/generations-image-to-video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "motionVideoGenerationJob": { "generationId": "leo-vid-4" }
            })))
            .mount(&server)
            .await;
        let mut base = make_base("leaves drifting in the wind", "leonardo/motion2");
        base.seed = Some(77);
        base.negative_prompt = Some("blur".into());
        let mut extras = extras_with_image_id();
        if let Some(Value::Object(map)) = extras.extra.as_mut() {
            map.insert("seed".into(), json!(1));
            map.insert("negativePrompt".into(), json!("other"));
        }
        let materialized = MaterializedRequest { refs: vec![] as Vec<MaterializedRef>, cleanup: Cleanup::empty() };
        make_provider(&server.uri()).generate(&ref_schema("leonardo/motion2"), &base, &extras, &materialized).await.unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!((body["seed"].as_i64(), body["negativePrompt"].as_str()), (Some(77), Some("blur")), "{body}");
    }

    /// Kling 2.5 Turbo: `KLING2_5`, "resolution ... Set to RESOLUTION_1080",
    /// "duration ... Set to 5 or 10".
    /// @see <https://docs.leonardo.ai/v1.0/docs/kling-2-5-turbo>
    #[tokio::test]
    async fn kling2_5_sends_kling2_5_at_1080() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/generations-image-to-video"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "motionVideoGenerationJob": { "generationId": "leo-vid-4" }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("leonardo/kling2.5");
        let base = make_base("a lantern floating over a lake", "leonardo/kling2.5");
        let mut extras = extras_with_image_id();
        extras.resolution = None;
        extras.duration_seconds = 10.0;
        let materialized = MaterializedRequest { refs: vec![] as Vec<MaterializedRef>, cleanup: Cleanup::empty() };

        provider.generate(&schema, &base, &extras, &materialized).await.unwrap();

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "KLING2_5");
        assert_eq!(body["resolution"], "RESOLUTION_1080");
        assert_eq!(body["duration"], 10);
    }

    // ─── Leonardo audit, 2026-10-30 ─────────────────────────────────────────

    fn no_refs() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    /// #16: `extra` passes a start frame by `imageId` and Leonardo's own
    /// knobs, but never replaces the model litegen bills or anything else it
    /// sends. E4a: nor does it publish the clip (`isPublic`); the account's
    /// own default applies.
    #[tokio::test]
    async fn extra_cannot_replace_what_litegen_sends() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": "stop" })))
            .mount(&server)
            .await;
        let mut extras = extras_with_image_id();
        extras.duration_seconds = 8.0;
        extras.extra = Some(json!({
            "imageId": "img-1", "model": "MOTION2", "prompt": "other", "resolution": "RESOLUTION_480",
            "duration": 4, "isPublic": true, "promptEnhance": true
        }));
        let _ = make_provider(&server.uri())
            .generate(&ref_schema("leonardo/veo3.1"), &make_base("a fox runs", "leonardo/veo3.1"), &extras, &no_refs())
            .await;
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body["model"], "VEO3_1");
        assert_eq!(body["prompt"], "a fox runs");
        assert_eq!(body["resolution"], "RESOLUTION_720");
        assert_eq!(body["duration"], 8);
        assert_eq!(body["imageId"], "img-1");
        assert_eq!(body["promptEnhance"], true);
        assert!(body.get("isPublic").is_none(), "{body}");
    }

    /// #17, #19: a create Leonardo turned away says so, with its status, and
    /// one that timed out may have been accepted and billed, so it is never
    /// sent again.
    #[tokio::test]
    async fn a_turned_away_create_says_so_and_a_timed_out_one_is_never_sent_again() {
        let schema = ref_schema("leonardo/motion2");
        let base = make_base("x", "leonardo/motion2");
        for (status, retryable) in [(429, true), (502, true), (400, false)] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!({ "error": "no" })))
                .mount(&server)
                .await;
            let err = make_provider(&server.uri()).generate(&schema, &base, &extras_with_image_id(), &no_refs()).await.unwrap_err();
            assert!(
                matches!(err, ProviderError::RequestFailed { status_code: Some(s), retryable: r, .. } if s == status && r == retryable),
                "{status}: {err:?}"
            );
        }
        let slow = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})).set_delay(std::time::Duration::from_secs(5)))
            .mount(&slow)
            .await;
        let mut p = make_provider(&slow.uri());
        p.client = reqwest::Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        let err = p.generate(&schema, &base, &extras_with_image_id(), &no_refs()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
    }

    /// #13, #25: a poll Leonardo answers with an error is that error; a
    /// failed generation says so; a finished one with no `motionMP4URL` ("The
    /// URL of the motion MP4") fails rather than delivering a still as a video.
    /// @see <https://docs.leonardo.ai/v1.0/reference/getgenerationbyid> (2026-10-30)
    #[tokio::test]
    async fn a_poll_says_what_leonardo_said() {
        let server = MockServer::start().await;
        for (id, status, body) in [
            ("gone", 404, json!({ "error": "not found" })),
            ("failed", 200, json!({ "generations_by_pk": { "status": "FAILED", "generated_images": [] } })),
            ("still", 200, json!({ "generations_by_pk": { "status": "COMPLETE", "generated_images": [{ "url": "https://cdn.leonardo.ai/a.jpg" }] } })),
        ] {
            Mock::given(method("GET"))
                .and(path(format!("/generations/{id}")))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .mount(&server)
                .await;
        }
        let provider = make_provider(&server.uri());
        let handle = |id: &str| VideoGenerationHandle { provider_job_id: id.into(), provider: "leonardo".into(), model: "leonardo/motion2".into() };

        let err = provider.poll_status(&handle("gone")).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { status_code: Some(404), retryable: false, .. }), "{err:?}");

        let failed = provider.poll_status(&handle("failed")).await.unwrap();
        assert_eq!(failed.status, GenerationStatus::Failed);
        assert!(failed.error.is_some());

        let still = provider.poll_status(&handle("still")).await.unwrap();
        assert_eq!(still.status, GenerationStatus::Failed);
        assert!(still.video_url.is_none());
        assert!(still.error.as_deref().is_some_and(|e| e.contains("motionMP4URL")), "{:?}", still.error);
    }
}
