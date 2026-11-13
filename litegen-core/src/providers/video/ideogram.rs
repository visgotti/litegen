use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
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

/// Ideogram video: the models Ideogram resells on its v2 API, on the same
/// `Api-Key` as its images (image/ideogram.rs).
///
/// Asynchronous: `POST /v2/video/generate/{model}-{text,image,reference}-to-video`
/// answers {generation_id, created}; `GET /v2/generations/{generation_id}`
/// says pending | completed | failed, and a completed one carries a
/// `video.generation` with "The direct link to the generated video". Text to
/// video is JSON; image and reference to video are `multipart/form-data`
/// (`image` + `end_image`, or `reference_images`).
///
/// @see <https://developer.ideogram.ai/openapi/ideogram-api-v2.json> (2026-11-10)
///   Verbatim: "Upload the first frame as `image`, and optionally a final frame as `end_image`, using `multipart/form-data`. Returns a `generation_id`; poll `GET /v2/generations/{generation_id}` or supply a `webhook_url`."
/// @see <https://developer.ideogram.ai/v2/api-reference/video/generate/kling-3-standard-text-to-video.md> (2026-11-10)
pub struct IdeogramVideoProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

/// A video model Ideogram resells: its endpoints' slug and what they take.
/// @see <https://developer.ideogram.ai/openapi/ideogram-api-v2.json> (2026-11-10)
struct Model {
    /// `/v2/video/generate/{slug}-{mode}-to-video`.
    slug: &'static str,
    /// Has a `…-reference-to-video` endpoint (all but Kling).
    reference: bool,
    /// Its image-to-video takes `aspect_ratio` (Seedance 2.0 only; the others'
    /// frame sets it).
    i2v_aspect: bool,
}

const MODELS: &[(&str, Model)] = &[
    ("ideogram/kling-3-standard", Model { slug: "kling-3-standard", reference: false, i2v_aspect: false }),
    ("ideogram/minimax-h3", Model { slug: "minimax-h3", reference: true, i2v_aspect: false }),
    ("ideogram/seedance-2", Model { slug: "seedance-2", reference: true, i2v_aspect: true }),
    ("ideogram/seedance-2-5", Model { slug: "seedance-2-5", reference: true, i2v_aspect: false }),
];

/// What litegen sends; `extra` cannot replace it. `webhook_url` is never sent
/// (litegen fires its own webhooks), nor `reference_audios`.
const OWNED_FIELDS: &[&str] = &[
    "prompt", "negative_prompt", "aspect_ratio", "resolution", "duration", "generate_audio", "image", "end_image",
    "reference_images", "reference_audios", "webhook_url",
];

/// `prompt_expansion_mode` is in MiniMax H3's reference-to-video schema only.
const REFERENCE_ONLY_EXTRAS: &[&str] = &["prompt_expansion_mode"];

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Text,
    Image,
    Reference,
}

impl IdeogramVideoProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`.
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(180), extra_headers)
    }

    pub fn new() -> Self {
        Self { config: None, key_pool: None, auth: AuthSpec::raw_header("Api-Key"), client: Self::api_client(&Default::default()) }
    }

    /// `api_base`, else Ideogram's API host (as image/ideogram.rs).
    fn api_base(&self) -> &str {
        self.config.as_ref().and_then(|c| c.api_base.as_deref()).unwrap_or("https://api.ideogram.ai").trim_end_matches('/')
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self.config.as_ref().map(|c| c.credentials.clone()).ok_or_else(|| ProviderError::NotConfigured("ideogram".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// The model's endpoints for a litegen id; `None` for an id with no arm,
    /// which is refused rather than run as another model (L13).
    fn resolve_model(model_id: &str) -> Option<&'static Model> {
        MODELS.iter().find(|(id, _)| *id == model_id).map(|(_, m)| m)
    }

    /// The submit's body as text fields (JSON or multipart alike) for `mode`.
    fn fields(model: &ModelSchema, m: &Model, mode: Mode, base: &BaseGenerationRequest, extras: &VideoExtras) -> Result<Vec<(String, Value)>, ProviderError> {
        use crate::capabilities::schema::ParamSpec;
        let default = |param: &str| match model.params.get(param) {
            Some(ParamSpec::AspectRatio(a)) => a.default.clone(),
            Some(ParamSpec::String(s)) => s.default.clone(),
            _ => None,
        };
        let mut out: Vec<(String, Value)> = vec![("prompt".into(), json!(base.prompt))];
        if let Some(n) = base.negative_prompt.as_deref().filter(|n| !n.is_empty()) {
            out.push(("negative_prompt".into(), json!(n)));
        }
        // "16:9" → Ideogram's "16x9", where this endpoint takes one.
        let takes_aspect = mode != Mode::Image || m.i2v_aspect;
        if let Some(aspect) = extras.aspect_ratio.clone().or_else(|| default("aspect_ratio")).filter(|_| takes_aspect) {
            out.push(("aspect_ratio".into(), json!(aspect.replace(':', "x"))));
        }
        if let Some(tier) = extras.resolution.clone().or_else(|| default("resolution")) {
            out.push(("resolution".into(), json!(tier)));
        }
        // Always: Seedance's "When omitted, the model picks the best duration",
        // and litegen bills the duration it asks for.
        out.push(("duration".into(), json!(extras.duration_seconds.round() as u64)));
        if let Some(audio) = extras.audio {
            out.push(("generate_audio".into(), json!(audio)));
        }
        for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED_FIELDS.contains(&k)) {
            if REFERENCE_ONLY_EXTRAS.contains(&k.as_str()) && mode != Mode::Reference {
                return Err(ProviderError::InvalidRequest(format!("{}: `{k}` is taken by reference-to-video only (attach reference images)", model.id)));
            }
            out.push((k.clone(), v.clone()));
        }
        Ok(out)
    }

    /// A refs's bytes as a multipart file part named after its field.
    fn file_part(field: &str, bytes: &[u8], content_type: &str) -> Result<Part, ProviderError> {
        let ct = crate::util::content::image_content_type(bytes).unwrap_or(content_type);
        let ext = match ct {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "bin",
        };
        Part::bytes(bytes.to_vec())
            .file_name(format!("{field}.{ext}"))
            .mime_str(ct)
            .map_err(|e| ProviderError::InvalidRequest(format!("ideogram image part: {e}")))
    }
}

impl Default for IdeogramVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for IdeogramVideoProvider {
    fn name(&self) -> &str {
        "ideogram"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.api_keys.is_empty() {
            self.key_pool = Some(ApiKeyPool::shared(config.api_keys.clone()));
        }
        self.client = Self::api_client(&config.extra_headers);
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config.as_ref().is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.key_pool.is_some())
    }

    /// Submit to the endpoint the request's images choose: reference images →
    /// reference to video; a start frame (and optional last) → image to
    /// video; none → text to video.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        let m = Self::resolve_model(&model.id)
            .ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "ideogram".into() })?;
        let refuse = |why: String| Err(ProviderError::InvalidRequest(format!("{}: {why}", model.id)));
        let files: Vec<(&str, &[u8], &str)> = materialized
            .refs
            .iter()
            .filter_map(|r| match &r.form {
                MaterializedRefForm::MultipartField { field_name, bytes, content_type } => Some((field_name.as_str(), bytes.as_ref(), content_type.as_str())),
                _ => None,
            })
            .collect();
        let has = |field: &str| files.iter().any(|(f, ..)| *f == field);
        let mode = match (has("image") || has("end_image"), has("reference_images")) {
            (true, true) => return refuse("frames and reference images go to different endpoints: send one or the other".into()),
            (false, true) if !m.reference => return refuse("takes no reference images (it has no reference-to-video endpoint)".into()),
            (false, true) => Mode::Reference,
            (true, false) if !has("image") => return refuse("a last frame needs a first frame (`image` is required)".into()),
            (true, false) => Mode::Image,
            (false, false) => Mode::Text,
        };
        let fields = Self::fields(model, m, mode, base, extras)?;
        let path = match mode {
            Mode::Text => "text",
            Mode::Image => "image",
            Mode::Reference => "reference",
        };
        let url = format!("{}/v2/video/generate/{}-{path}-to-video", self.api_base(), m.slug);
        let creds = self.creds()?;
        let builder = if mode == Mode::Text {
            let body: serde_json::Map<String, Value> = fields.into_iter().collect();
            self.client.post(&url).json(&Value::Object(body))
        } else {
            let mut form = Form::new();
            for (k, v) in fields {
                form = form.text(k, v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()));
            }
            // In order: image, then end_image; reference_images in prompt order.
            for field in ["image", "end_image", "reference_images"] {
                for (f, bytes, ct) in files.iter().filter(|(f, ..)| *f == field) {
                    form = form.part(f.to_string(), Self::file_part(f, bytes, ct)?);
                }
            }
            self.client.post(&url).multipart(form)
        };
        let builder = crate::providers::auth::apply(&self.auth, &creds, builder)?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| {
            let retryable = crate::providers::may_resend_after(&e, true);
            ProviderError::RequestFailed {
                message: format!("Ideogram video request failed: {}", e.without_url()),
                status_code: None,
                provider_error: None,
                retryable,
            }
        })?;
        // The status first (a 429's Retry-After reaches the retry policy).
        let data = crate::providers::read_vendor_json(resp, "Ideogram").await?;
        let id = data["generation_id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Ideogram answered without a generation_id".to_string(),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        })?;
        Ok(VideoGenerationHandle { provider_job_id: id.to_string(), provider: "ideogram".to_string(), model: model.id.clone() })
    }

    /// `GET /v2/generations/{generation_id}`.
    async fn poll_status(&self, handle: &VideoGenerationHandle) -> Result<VideoGenerationPollResult, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/v2/generations/{}", self.api_base(), handle.provider_job_id);
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(&url))?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
            message: e.to_string(),
            status_code: None,
            provider_error: None,
            retryable: true,
        })?;
        // A 4xx is the answer (a 404 is a generation that can't be found); a
        // 5xx or 429 ("a 429 response carries a `Retry-After`") is asked again.
        let data = super::read_poll_json(resp, "ideogram").await?;
        let result = |status, progress, video_url: Option<String>, error: Option<String>| VideoGenerationPollResult {
            status,
            progress,
            video_url,
            video_data: None,
            content_type: (status == GenerationStatus::Completed).then(|| "video/mp4".to_string()),
            error,
            metadata: HashMap::new(),
        };
        // status "pending" | "completed" | "failed".
        Ok(match data["status"].as_str() {
            Some("pending") => result(GenerationStatus::Processing, 50, None, None),
            Some("failed") => result(
                GenerationStatus::Failed,
                100,
                None,
                Some(data["failure_reason"].as_str().unwrap_or("Ideogram failed the generation without a reason").to_string()),
            ),
            Some("completed") => {
                let video = data["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|d| d["object_type"] == "video.generation")
                    .and_then(|d| d["url"].as_str());
                match video {
                    Some(url) => result(GenerationStatus::Completed, 100, Some(url.to_string()), None),
                    // A null url is an output that didn't pass Ideogram's safety checks.
                    None => result(GenerationStatus::Failed, 100, None, Some(format!("Ideogram finished without a video link: {}", data["data"]))),
                }
            }
            other => super::unknown_state("ideogram", other),
        })
    }

    async fn estimate_cost(&self, model: &ModelSchema, _request: &VideoGenerationRequest) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, Some(json!({ "model": model.id }))))
    }

    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult { healthy: false, message: "Ideogram video provider not configured".into(), latency_ms: None };
        }
        HealthCheckResult { healthy: true, message: "Ideogram video provider configured".into(), latency_ms: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::{Cleanup, MaterializedRef, MaterializedRefForm};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load").get(id).expect("model").clone()
    }

    /// A pooled key, as the dashboard stores it.
    fn make_provider(api_base: &str) -> IdeogramVideoProvider {
        let mut p = IdeogramVideoProvider::new();
        p.configure(ProviderInstanceConfig {
            api_base: Some(api_base.to_string()),
            api_keys: vec![crate::types::ApiKeyEntry { key: "id-key".into(), weight: 1, label: None }],
            ..Default::default()
        });
        p
    }

    fn base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.into(), model: model.into(), n: 1, negative_prompt: None, seed: None,
            reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    fn extras(aspect: Option<&str>, resolution: Option<&str>, duration: f64, audio: Option<bool>, extra: Option<Value>) -> VideoExtras {
        VideoExtras {
            duration_seconds: duration, aspect_ratio: aspect.map(str::to_string), resolution: resolution.map(str::to_string),
            fps: None, extra, camera: None, audio, r#loop: None, source: None, direction: None, time_range: None, replace: None,
        }
    }

    /// Refs as the materializer hands them over for the rows' multipart
    /// field_map: first_frame → image, last_frame → end_image, reference →
    /// reference_images.
    fn refs(roles: &[&str]) -> MaterializedRequest {
        let field = |role: &str| match role {
            "first_frame" => "image",
            "last_frame" => "end_image",
            _ => "reference_images",
        };
        MaterializedRequest {
            refs: roles
                .iter()
                .enumerate()
                .map(|(i, role)| MaterializedRef {
                    role: role.to_string(),
                    form: MaterializedRefForm::MultipartField {
                        field_name: field(role).into(),
                        bytes: format!("\u{89}PNG\r\n\u{1a}\n{role}{i}").into_bytes().into(),
                        content_type: "application/octet-stream".into(),
                    },
                    time_s: None,
                })
                .collect(),
            cleanup: Cleanup::empty(),
        }
    }

    async fn accepting() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "generation_id": "gen_1", "created": "2026-11-10T00:00:00Z" })))
            .mount(&server)
            .await;
        server
    }

    /// The multipart parts of a request: (name, value) for text, (name, "<file>") for files.
    fn parts(r: &wiremock::Request) -> Vec<(String, String)> {
        let ct = r.headers.get("content-type").unwrap().to_str().unwrap().to_string();
        let boundary = ct.split("boundary=").nth(1).unwrap().trim_matches('"').to_string();
        let body = String::from_utf8_lossy(&r.body).to_string();
        body.split(&format!("--{boundary}"))
            .filter_map(|part| {
                let name = part.split("name=\"").nth(1)?.split('"').next()?.to_string();
                let file = part.contains("filename=");
                let value = part.split("\r\n\r\n").nth(1)?.trim_end_matches("\r\n").to_string();
                Some((name, if file { "<file>".to_string() } else { value }))
            })
            .collect()
    }

    /// L13: every catalog row has its own arm, and an id with none is refused.
    #[test]
    fn every_catalog_row_resolves_and_an_unknown_id_does_not() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        let rows: Vec<_> = catalog.for_provider("ideogram").filter(|m| m.media_type == crate::capabilities::MediaType::Video).collect();
        assert_eq!(rows.len(), 4);
        for m in rows {
            assert!(IdeogramVideoProvider::resolve_model(&m.id).is_some(), "{} has no arm", m.id);
        }
        assert!(IdeogramVideoProvider::resolve_model("ideogram/kling-4").is_none());
    }

    /// Text to video is JSON at `/v2/video/generate/{model}-text-to-video`,
    /// with the Api-Key header from the pool: the prompt, duration (always),
    /// aspect_ratio as "16x9", the tier spelled Ideogram's way, generate_audio
    /// and negative_prompt where the row has them, allowlisted extras; never
    /// webhook_url.
    /// @see <https://developer.ideogram.ai/openapi/ideogram-api-v2.json> (2026-11-10)
    #[tokio::test]
    async fn text_to_video_is_json_with_the_fields_its_endpoint_takes() {
        let server = accepting().await;
        let p = make_provider(&server.uri());
        let mut b = base("a fox in snow", "ideogram/kling-3-standard");
        b.negative_prompt = Some("blur".into());
        let h = p
            .generate(&ref_schema("ideogram/kling-3-standard"), &b, &extras(Some("9:16"), None, 10.0, Some(true), Some(json!({ "shot_type": "intelligent", "webhook_url": "https://x", "prompt": "other" }))), &refs(&[]))
            .await
            .unwrap();
        assert_eq!((h.provider_job_id.as_str(), h.provider.as_str()), ("gen_1", "ideogram"));
        let _ = p.generate(&ref_schema("ideogram/minimax-h3"), &base("a fox", "ideogram/minimax-h3"), &extras(None, None, 5.0, None, None), &refs(&[])).await.unwrap();
        let _ = p.generate(&ref_schema("ideogram/seedance-2-5"), &base("a fox", "ideogram/seedance-2-5"), &extras(None, Some("1080p"), 30.0, None, None), &refs(&[])).await.unwrap();

        let sent = server.received_requests().await.unwrap();
        assert_eq!(sent[0].url.path(), "/v2/video/generate/kling-3-standard-text-to-video");
        assert_eq!(sent[0].headers.get("api-key").unwrap(), "id-key");
        let body: Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert_eq!(body, json!({ "prompt": "a fox in snow", "duration": 10, "aspect_ratio": "9x16", "generate_audio": true, "negative_prompt": "blur", "shot_type": "intelligent" }));
        assert_eq!(sent[1].url.path(), "/v2/video/generate/minimax-h3-text-to-video");
        let body: Value = serde_json::from_slice(&sent[1].body).unwrap();
        assert_eq!(body, json!({ "prompt": "a fox", "duration": 5, "aspect_ratio": "16x9", "resolution": "2K" }));
        // Seedance's aspect defaults to the vendor's "auto": none is sent unless asked.
        let body: Value = serde_json::from_slice(&sent[2].body).unwrap();
        assert_eq!(body, json!({ "prompt": "a fox", "duration": 30, "resolution": "1080p" }));
    }

    /// A start frame (and an optional last one) goes multipart to
    /// `…-image-to-video` as `image` and `end_image`; reference images go to
    /// `…-reference-to-video` as `reference_images`. Image to video takes no
    /// aspect_ratio on Kling, MiniMax and Seedance 2.5, so none is sent there.
    #[tokio::test]
    async fn frames_and_references_go_multipart_to_their_own_endpoints() {
        let server = accepting().await;
        let p = make_provider(&server.uri());
        let _ = p.generate(&ref_schema("ideogram/minimax-h3"), &base("a fox", "ideogram/minimax-h3"), &extras(Some("1:1"), Some("768P"), 6.0, None, None), &refs(&["last_frame", "first_frame"])).await.unwrap();
        let _ = p
            .generate(&ref_schema("ideogram/minimax-h3"), &base("a fox", "ideogram/minimax-h3"), &extras(Some("1:1"), None, 5.0, None, Some(json!({ "prompt_expansion_mode": "fast" }))), &refs(&["reference", "reference"]))
            .await
            .unwrap();
        let _ = p.generate(&ref_schema("ideogram/seedance-2"), &base("a fox", "ideogram/seedance-2"), &extras(Some("21:9"), None, 4.0, Some(false), None), &refs(&["first_frame"])).await.unwrap();

        let sent = server.received_requests().await.unwrap();
        assert_eq!(sent[0].url.path(), "/v2/video/generate/minimax-h3-image-to-video");
        let mut got = parts(&sent[0]);
        got.sort();
        assert_eq!(got, vec![
            ("duration".into(), "6".into()), ("end_image".into(), "<file>".into()), ("image".into(), "<file>".into()),
            ("prompt".into(), "a fox".into()), ("resolution".into(), "768P".into()),
        ]);
        assert_eq!(sent[1].url.path(), "/v2/video/generate/minimax-h3-reference-to-video");
        let mut got = parts(&sent[1]);
        got.sort();
        assert_eq!(got, vec![
            ("aspect_ratio".into(), "1x1".into()), ("duration".into(), "5".into()), ("prompt".into(), "a fox".into()),
            ("prompt_expansion_mode".into(), "fast".into()), ("reference_images".into(), "<file>".into()), ("reference_images".into(), "<file>".into()),
            ("resolution".into(), "2K".into()),
        ]);
        // Seedance 2.0's image to video does take an aspect.
        assert_eq!(sent[2].url.path(), "/v2/video/generate/seedance-2-image-to-video");
        let got = parts(&sent[2]);
        assert!(got.contains(&("aspect_ratio".into(), "21x9".into())) && got.contains(&("generate_audio".into(), "false".into())), "{got:?}");
    }

    /// Requests no endpoint takes are refused before any call: frames with
    /// references, a last frame alone ("`image` … required"), references on
    /// Kling (no reference endpoint), prompt_expansion_mode off reference
    /// to video.
    #[tokio::test]
    async fn a_request_no_endpoint_takes_is_refused_before_any_call() {
        let server = accepting().await;
        let p = make_provider(&server.uri());
        for (id, materialized, extra) in [
            ("ideogram/minimax-h3", refs(&["first_frame", "reference"]), None),
            ("ideogram/seedance-2", refs(&["last_frame"]), None),
            ("ideogram/kling-3-standard", refs(&["reference"]), None),
            ("ideogram/minimax-h3", refs(&[]), Some(json!({ "prompt_expansion_mode": "fast" }))),
        ] {
            let err = p.generate(&ref_schema(id), &base("x", id), &extras(None, None, 5.0, None, extra), &materialized).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{id}: {err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// `GET /v2/generations/{generation_id}`: pending → processing; completed
    /// → the `video.generation` url; failed → its `failure_reason`; a
    /// completed one without a url (unsafe output) fails; an undocumented
    /// state fails; a 401 / 404 is final, a 503 or 429 asked again.
    /// @see <https://developer.ideogram.ai/openapi/ideogram-api-v2.json> (2026-11-10)
    #[tokio::test]
    async fn a_poll_says_what_ideogram_said() {
        let handle = VideoGenerationHandle { provider_job_id: "gen_1".into(), provider: "ideogram".into(), model: "ideogram/seedance-2".into() };
        let done = json!({ "generation_id": "gen_1", "status": "completed", "created": "2026-11-10T00:00:00Z", "response_type": "url",
            "data": [{ "object_type": "video.generation", "url": "https://ideogram.ai/api/videos/v.mp4", "prompt": "x", "resolution": "720p", "aspect_ratio": "16:9", "duration": 5 }] });
        for (reply, want) in [
            (ResponseTemplate::new(200).set_body_json(json!({ "generation_id": "gen_1", "status": "pending", "created": "x" })), Ok((GenerationStatus::Processing, None))),
            (ResponseTemplate::new(200).set_body_json(done.clone()), Ok((GenerationStatus::Completed, Some("https://ideogram.ai/api/videos/v.mp4")))),
            (ResponseTemplate::new(200).set_body_json(json!({ "generation_id": "gen_1", "status": "failed", "created": "x", "failure_reason": "content_policy_violation" })), Ok((GenerationStatus::Failed, None))),
            (ResponseTemplate::new(200).set_body_json(json!({ "generation_id": "gen_1", "status": "completed", "data": [{ "object_type": "video.generation", "url": null }] })), Ok((GenerationStatus::Failed, None))),
            (ResponseTemplate::new(200).set_body_json(json!({ "generation_id": "gen_1", "status": "cancelled" })), Ok((GenerationStatus::Failed, None))),
            (ResponseTemplate::new(404).set_body_json(json!({ "error": "not found" })), Err(false)),
            (ResponseTemplate::new(401).set_body_json(json!({ "error": "bad key" })), Err(false)),
            (ResponseTemplate::new(503).set_body_string("busy"), Err(true)),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET")).and(path("/v2/generations/gen_1")).respond_with(reply).mount(&server).await;
            match (make_provider(&server.uri()).poll_status(&handle).await, want) {
                (Ok(p), Ok((status, url))) => {
                    assert_eq!((p.status, p.video_url.as_deref()), (status, url));
                    if status == GenerationStatus::Failed {
                        assert!(p.error.is_some());
                    }
                    if status == GenerationStatus::Completed {
                        assert_eq!((p.progress, p.content_type.as_deref()), (100, Some("video/mp4")));
                    }
                }
                (Err(ProviderError::RequestFailed { retryable, .. }), Err(want)) => assert_eq!(retryable, want),
                (got, want) => panic!("{got:?} for {want:?}"),
            }
        }
    }

    /// A submit Ideogram turns away says why; a 429 or 5xx may be sent again,
    /// any other 4xx not; an answer without a generation_id is final.
    #[tokio::test]
    async fn a_refused_submit_says_why() {
        for (reply, retryable) in [
            (ResponseTemplate::new(422).set_body_json(json!({ "error": "duration out of range" })), Some(false)),
            (ResponseTemplate::new(503).set_body_string("busy"), Some(true)),
            (ResponseTemplate::new(200).set_body_json(json!({ "created": "x" })), Some(false)),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).respond_with(reply).mount(&server).await;
            let err = make_provider(&server.uri())
                .generate(&ref_schema("ideogram/seedance-2"), &base("x", "ideogram/seedance-2"), &extras(None, None, 5.0, None, None), &refs(&[]))
                .await
                .unwrap_err();
            match err {
                ProviderError::RequestFailed { retryable: r, .. } => assert_eq!(Some(r), retryable),
                ProviderError::RateLimited { .. } => assert_eq!(retryable, Some(true)),
                other => panic!("{other:?}"),
            }
        }
    }
}
