use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// What litegen itself sends to `POST /generations/image`; `extra` cannot replace it.
const OWNED_FIELDS: &[&str] =
    &["model", "prompt", "aspect_ratio", "image_ref", "style_ref", "modify_image_ref", "character_ref", "format"];

/// Luma's spelling of each container litegen offers: "ImageFormat … `jpg`,
/// `png`" ("default": "jpg").
/// @see <https://docs.lumalabs.ai/reference/generateimage.md> (2026-11-03)
pub(crate) const LUMA_IMAGE_FORMATS: [(&str, &str); 2] = [("jpeg", "jpg"), ("png", "png")];

/// Luma Photon image generation provider (Dream Machine API).
///
/// Shares the Luma Dream Machine host and Bearer auth with the Luma video
/// provider. Generation is asynchronous: `POST /generations/image` returns a
/// job id; poll `GET /generations/{id}` until `state == completed`, then read
/// `assets.image` (a CDN URL) and download the bytes.
///
/// Reference images are passed as CDN URLs only (no base64/multipart) — the
/// materializer uploads any base64/blob refs to storage first.
///
/// @see <https://docs.lumalabs.ai/reference/generateimage> — `POST /generations/image`
///   Verbatim: "post https://api.lumalabs.ai/dream-machine/v1/generations/image"
/// @see <https://docs.lumalabs.ai/docs/image-generation> — models + ref roles
///   Verbatim: "You can choose from our two model versions: photon-1 (default) photon-flash-1"
///   Verbatim: "You should upload and use your own cdn image urls, currently this is the only way to pass an image"
pub struct LumaImageProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
    /// Result downloads: no redirects, so a URL that passed the public-address
    /// check cannot bounce into a private one.
    asset_client: Client,
    /// Whether a result URL must resolve to a public address. Only the tests,
    /// whose mock vendor runs on loopback, turn it off.
    pub(crate) public_assets_only: bool,
}

impl LumaImageProvider {
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
            asset_client: crate::util::ssrf::public_only_download_client(
                std::time::Duration::from_secs(120),
                std::time::Duration::from_secs(30),
            ),
            public_assets_only: true,
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.lumalabs.ai/dream-machine/v1")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("luma".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// `None` for an id with no arm: refused, not billed as photon-1 (L13).
    fn resolve_model(model_id: &str) -> Option<&'static str> {
        match model_id.strip_prefix("luma/").unwrap_or(model_id) {
            "photon-flash-1" => Some("photon-flash-1"),
            "photon-1" => Some("photon-1"),
            _ => None,
        }
    }

    async fn poll_generation(&self, id: &str) -> Result<Value, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/generations/{}", self.api_base(), id);
        // As long as litegen waits for this call (the vendor timeout,
        // docs/gateway-settings.md); 180 s when called directly.
        let started = std::time::Instant::now();
        let mut budget = crate::providers::PollBudget::new(std::time::Duration::from_secs(180), std::time::Duration::from_secs(2));
        while budget.next().await {
            let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(&url))?;
            let resp = match crate::providers::inject_trace_headers(builder).send().await {
                Ok(resp) => resp,
                // The job is Luma's now (and billed); a dropped poll is asked again.
                Err(_) => continue,
            };
            // The status first: a gateway's 5xx page, a 408 or a 429 is asked
            // again; any other error status is the answer.
            let data = match crate::providers::video::read_poll_json(resp, "luma").await {
                Ok(data) => data,
                Err(ProviderError::RequestFailed { retryable: true, .. }) => continue,
                Err(e) => return Err(e),
            };
            match data["state"].as_str() {
                Some("completed") => return Ok(data),
                Some("failed") => {
                    let err = data["failure_reason"].as_str().unwrap_or("Unknown error").to_string();
                    return Err(ProviderError::RequestFailed {
                        message: format!("Luma image generation failed: {err}"),
                        status_code: None,
                        provider_error: Some(data),
                        retryable: false,
                    });
                }
                // state: queued | dreaming | completed | failed.
                Some("queued") | Some("dreaming") => continue,
                other => return Err(super::unknown_state("luma", other)),
            }
        }
        Err(ProviderError::Timeout { timeout_ms: budget.total_ms(started) })
    }

    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        super::fetch_result(&self.asset_client, url, self.public_assets_only, "luma image").await
    }
}

impl Default for LumaImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl LumaImageProvider {
    /// A finished job's result, as `generate` returns it: its image
    /// downloaded, with the vendor's ids.
    async fn finish(&self, id: &str, final_result: Value) -> Result<GenerationOutput, ProviderError> {
        let image_url = final_result["assets"]["image"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Luma completed result missing assets.image".to_string(),
            status_code: None,
            provider_error: Some(final_result.clone()),
            retryable: false,
        })?;

        let bytes = self.fetch_image_bytes(image_url).await?;
        let mut metadata = HashMap::new();
        metadata.insert("id".to_string(), Value::String(id.to_string()));
        metadata.insert("url".to_string(), Value::String(image_url.to_string()));

        Ok(GenerationOutput {
            data: bytes,
            content_type: "image/jpeg".to_string(),
            metadata,
            more: Vec::new(),
        })
    }
}

#[async_trait]
impl ImageProvider for LumaImageProvider {
    fn name(&self) -> &str {
        "luma"
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

    /// Submit `POST /generations/image`, poll to completion, fetch the bytes.
    ///
    /// @see <https://docs.lumalabs.ai/reference/generateimage> — request body
    ///   (`model`, `prompt`, `aspect_ratio`, `image_ref`/`style_ref`/`character_ref`/
    ///   `modify_image_ref`) and the `assets.image` result field.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id).ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "luma".into() })?;
        let url = format!("{}/generations/image", self.api_base());

        let mut body = json!({
            "model": crate::providers::wire_model(self.config.as_ref(), &model.id, native),
            "prompt": base.prompt,
        });
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }
        // The asked container (PixelGenie L9) in Luma's spelling; none asked,
        // none sent (Luma's default is jpg).
        if let Some(asked) = extras.output_format.as_deref() {
            let format = LUMA_IMAGE_FORMATS
                .iter()
                .find(|(ours, _)| ours.eq_ignore_ascii_case(asked))
                .map(|(_, luma)| *luma)
                .ok_or_else(|| ProviderError::InvalidRequest(format!("{}: Luma returns jpeg or png, not {asked}", model.id)))?;
            body["format"] = Value::String(format.to_string());
        }

        // Reference images (CDN URLs only). Map roles to Luma's ref fields.
        let mut image_ref: Vec<Value> = Vec::new();
        let mut style_ref: Vec<Value> = Vec::new();
        let mut character_images: Vec<Value> = Vec::new();
        for r in &materialized.refs {
            if let MaterializedRefForm::Url(u) = &r.form {
                match r.role.as_str() {
                    "style" | "style_ref" => style_ref.push(json!({ "url": u })),
                    "modify" | "modify_image_ref" => body["modify_image_ref"] = json!({ "url": u }),
                    "character" | "character_ref" => character_images.push(Value::String(u.clone())),
                    _ => image_ref.push(json!({ "url": u })),
                }
            }
        }
        if !image_ref.is_empty() {
            body["image_ref"] = Value::Array(image_ref);
        }
        if !style_ref.is_empty() {
            body["style_ref"] = Value::Array(style_ref);
        }
        // "you can use up to 4 images of the same person to build one
        // identity" — every character ref goes into the one identity.
        // @see <https://docs.lumalabs.ai/docs/image-generation>
        if !character_images.is_empty() {
            body["character_ref"] = json!({ "identity0": { "images": character_images } });
        }

        // Rule 11: `extra` never replaces what litegen sends — an
        // `extra.model` would run a model other than the one billed.
        if let Some(obj) = body.as_object_mut() {
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED_FIELDS.contains(&k)) {
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
                message: format!("Luma image request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: crate::providers::may_resend_after(&e, true),
            })?;

        // The status before the body (read_vendor_json): a 429, a 408 or a
        // 5xx turned the call away and is retried, after any `Retry-After`
        // (docs/gateway-settings.md); any other error status is Luma's refusal.
        let submit = crate::providers::read_vendor_json(resp, "Luma").await?;

        let id = submit["id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Luma response missing id".to_string(),
            status_code: None,
            provider_error: Some(submit.clone()),
            retryable: false,
        })?;
        // Accepted: from here on a failure must not re-send the job (L1).
        extras.vendor_job.record(id);

        let final_result = self.poll_generation(id).await?;
        self.finish(id, final_result).await
    }

    fn resumable(&self, _model: &ModelSchema) -> bool {
        true
    }

    /// One look at an accepted job, read as `generate`'s own poll loop reads it.
    async fn poll_job(&self, _model: &str, job: &str, _resume: Option<&Value>) -> Result<crate::providers::ImageJobPoll, ProviderError> {
        match super::poll_once(self.poll_generation(job)).await {
            Ok(Some(result)) => self.finish(job, result).await.map(crate::providers::ImageJobPoll::Done),
            Ok(None) => Ok(crate::providers::ImageJobPoll::Running),
            Err(e) => super::job_failed(e),
        }
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &ImageGenerationRequest,
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
                message: "Luma image provider not configured".into(),
                latency_ms: None,
            };
        }
        let start = std::time::Instant::now();
        let creds = match self.creds() {
            Ok(c) => c,
            Err(_) => return HealthCheckResult { healthy: false, message: "No API key".into(), latency_ms: None },
        };
        let builder = match crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.get(format!("{}/generations?limit=1", self.api_base())).timeout(std::time::Duration::from_secs(10)),
        ) {
            Ok(b) => b,
            Err(e) => return HealthCheckResult { healthy: false, message: e.to_string(), latency_ms: None },
        };
        match builder.send().await {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "Luma API key valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("Luma returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("Luma health check failed: {e}"),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
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
        for m in catalog.for_provider("luma").filter(|m| m.media_type == crate::capabilities::MediaType::Image) {
            assert!(LumaImageProvider::resolve_model(&m.id).is_some(), "{} has no arm", m.id);
        }
        assert!(LumaImageProvider::resolve_model("luma/not-a-model").is_none());
    }

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> LumaImageProvider {
        let mut p = LumaImageProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_key: "test-key".to_string(),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        cfg.credentials.api_key = Some("test-key".to_string());
        p.configure(cfg);
        // The mock vendor serves results from loopback.
        p.public_assets_only = false;
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

    fn make_extras() -> ImageExtras {
        ImageExtras {
            vendor_job: Default::default(),
            size: None,
            resolution: None, aspect_ratio: Some("16:9".to_string()),
            quality: None,
            style: None,
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".to_string(),
            edit: None, views: Vec::new(),
            extra: None,
            output_format: None, background: None,
            expand: None,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        }
    }

    #[tokio::test]
    async fn generates_photon_image_via_polling() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/img.jpg", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/generations/image"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": "luma_img_1", "state": "queued"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/generations/luma_img_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "luma_img_1", "state": "completed", "assets": { "image": image_url.clone() }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"LUMAJPEG".to_vec()).insert_header("content-type", "image/jpeg"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("luma/photon-1");
        let base = make_base("a serene alpine lake", "luma/photon-1");
        let extras = make_extras();
        let materialized = MaterializedRequest {
            refs: vec![MaterializedRef { role: "init".into(), form: MaterializedRefForm::Url("https://cdn/x.jpg".into()), time_s: None }],
            cleanup: Cleanup::empty(),
        };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        // The vendor accepted the job before the poll: the router must never re-send it.
        assert_eq!(extras.vendor_job.get().as_deref(), Some("luma_img_1"));
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"LUMAJPEG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("authorization").unwrap(), "Bearer test-key");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["model"], "photon-1");
        assert_eq!(body["prompt"], "a serene alpine lake");
        assert_eq!(body["aspect_ratio"], "16:9");
        assert_eq!(body["image_ref"][0]["url"], "https://cdn/x.jpg");
    }

    /// The catalog allows up to 4 `character` refs and Luma builds one identity
    /// from "up to 4 images of the same person" — but each ref used to
    /// overwrite `character_ref`, so only the last image was ever sent.
    /// @see <https://docs.lumalabs.ai/docs/image-generation>
    #[tokio::test]
    async fn all_character_refs_go_into_one_identity() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/img.jpg", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/generations/image"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": "luma_img_2", "state": "queued"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/generations/luma_img_2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "luma_img_2", "state": "completed", "assets": { "image": image_url.clone() }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"LUMAJPEG".to_vec()))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("luma/photon-1");
        let base = make_base("the same man as a knight", "luma/photon-1");
        let materialized = MaterializedRequest {
            refs: vec![
                MaterializedRef { role: "character".into(), form: MaterializedRefForm::Url("https://cdn/face1.jpg".into()), time_s: None },
                MaterializedRef { role: "character".into(), form: MaterializedRefForm::Url("https://cdn/face2.jpg".into()), time_s: None },
            ],
            cleanup: Cleanup::empty(),
        };

        provider.generate(&schema, &base, &make_extras(), &materialized).await.unwrap();

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(
            body["character_ref"]["identity0"]["images"],
            json!(["https://cdn/face1.jpg", "https://cdn/face2.jpg"])
        );
    }

    /// A 429 turned the call away and is retried, after the `Retry-After`
    /// Luma asked for when it sent one (docs/gateway-settings.md).
    #[tokio::test]
    async fn a_429_is_retryable() {
        let body = json!({ "detail": "Too many requests" });
        for (answer, wait_ms) in [
            (ResponseTemplate::new(429).insert_header("retry-after", "2").set_body_json(body.clone()), Some(2000)),
            (ResponseTemplate::new(429).set_body_json(body.clone()), None),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).respond_with(answer).mount(&server).await;
            let err = make_provider(&server.uri())
                .generate(&ref_schema("luma/photon-1"), &make_base("x", "luma/photon-1"), &make_extras(), &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() })
                .await
                .unwrap_err();
            match wait_ms {
                Some(ms) => assert!(matches!(&err, ProviderError::RateLimited { retry_after_ms, provider_error: Some(_), .. } if *retry_after_ms == ms), "{err:?}"),
                None => assert!(matches!(&err, ProviderError::RequestFailed { retryable: true, status_code: Some(429), .. }), "{err:?}"),
            }
        }
    }

    /// A gateway's 5xx page is not JSON: the status decides, and the call is
    /// retried rather than failed as an unreadable answer.
    #[tokio::test]
    async fn a_5xx_page_that_is_not_json_is_retryable() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(503).set_body_string("<html>Service Unavailable</html>")).mount(&server).await;
        let err = make_provider(&server.uri())
                .generate(&ref_schema("luma/photon-1"), &make_base("x", "luma/photon-1"), &make_extras(), &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() })
                .await
                .unwrap_err();
        assert!(matches!(&err, ProviderError::RequestFailed { retryable: true, status_code: Some(503), .. }), "{err:?}");
    }

    /// `extra` passes vendor knobs through, never over a field litegen sets.
    #[tokio::test]
    async fn extra_never_overwrites_an_owned_field() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(400).set_body_json(json!({ "detail": "stop" }))).mount(&server).await;
        let p = make_provider(&server.uri());
        let mut extras = make_extras();
        extras.aspect_ratio = Some("16:9".into());
        extras.extra = Some(json!({ "model": "photon-flash-1", "prompt": "overwritten", "aspect_ratio": "1:1", "sync_timeout": 30 }));
        let _ = p.generate(&ref_schema("luma/photon-1"), &make_base("a lighthouse", "luma/photon-1"), &extras, &MaterializedRequest::default()).await;
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(
            (body["model"].as_str(), body["prompt"].as_str(), body["aspect_ratio"].as_str()),
            (Some("photon-1"), Some("a lighthouse"), Some("16:9")),
            "{body}"
        );
        assert_eq!(body["sync_timeout"], 30, "a knob litegen doesn't set still passes: {body}");
    }

    /// "format … The format of the image": ImageFormat `jpg | png` ("default":
    /// "jpg"). The asked container goes as Luma's own spelling (jpeg → jpg),
    /// `extra` cannot replace it, and none is sent when none is asked.
    /// @see <https://docs.lumalabs.ai/reference/generateimage.md> (2026-11-03)
    #[tokio::test]
    async fn the_output_format_goes_as_lumas_format() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(400).set_body_json(json!({ "detail": "stop" }))).mount(&server).await;
        let p = make_provider(&server.uri());
        let id = "luma/photon-1";
        for (asked, extra) in [(Some("jpeg"), json!({ "format": "png" })), (Some("png"), json!({})), (None, json!({}))] {
            let mut extras = make_extras();
            extras.output_format = asked.map(str::to_string);
            extras.extra = Some(extra);
            let _ = p.generate(&ref_schema(id), &make_base("x", id), &extras, &MaterializedRequest::default()).await;
        }
        let formats: Vec<Value> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap()["format"].clone())
            .collect();
        assert_eq!(formats, [json!("jpg"), json!("png"), Value::Null]);
        for (ours, luma) in LUMA_IMAGE_FORMATS {
            assert!(["jpg", "png"].contains(&luma), "{ours} → {luma}");
        }
    }
}
