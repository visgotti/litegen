use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
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

/// Upload reference bytes to Leonardo's presigned-S3 init-image endpoint,
/// returning the resulting `imageId`. Shared by the image and video providers.
///
/// Two steps: `POST /init-image {extension}` returns `uploadInitImage{id, url,
/// fields}` (fields is a JSON string of S3 form fields); then POST the file to
/// the returned S3 `url` as multipart (fields + `file`). The `id` is the imageId.
///
/// The upload names the image's own type: `extension` "Has to be png, jpg,
/// jpeg, or webp", and "the image must be in the format of the `extension`
/// provided to the init image method". Anything else is refused before any call.
///
/// @see <https://docs.leonardo.ai/v1.0/reference/uploadinitimage> (2026-10-30)
/// @see <https://docs.leonardo.ai/v1.0/docs/how-to-upload-an-image-using-a-presigned-url.md> (2026-10-30)
pub(crate) async fn upload_init_image(
    client: &Client,
    upload_client: &Client,
    api_base: &str,
    auth: &AuthSpec,
    creds: &ProviderCredentials,
    bytes: &[u8],
) -> Result<String, ProviderError> {
    let (extension, mime) = match crate::proxy::materializer::sniff_image_mime(bytes) {
        Some("image/png") => ("png", "image/png"),
        Some("image/jpeg") => ("jpg", "image/jpeg"),
        Some("image/webp") => ("webp", "image/webp"),
        other => {
            return Err(ProviderError::InvalidRequest(format!(
                "leonardo takes a PNG, JPEG or WebP reference, not {}",
                other.unwrap_or("an image of unknown type")
            )))
        }
    };
    let url = format!("{api_base}/init-image");
    let builder = crate::providers::auth::apply(
        auth,
        creds,
        client.post(&url).header("Content-Type", "application/json").json(&json!({ "extension": extension })),
    )?;
    // An upload is not a billed job, so a failed one may be asked again.
    let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
        message: format!("Leonardo init-image request failed: {e}"),
        status_code: None,
        provider_error: None,
        retryable: true,
    })?;
    let data = read_create(resp, "Leonardo init-image").await?;

    let uii = &data["uploadInitImage"];
    let image_id = uii["id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
        message: "Leonardo init-image missing uploadInitImage.id".to_string(),
        status_code: None,
        provider_error: Some(data.clone()),
        retryable: false,
    })?;
    let s3_url = uii["url"].as_str().ok_or_else(|| ProviderError::RequestFailed {
        message: "Leonardo init-image missing uploadInitImage.url".to_string(),
        status_code: None,
        provider_error: Some(data.clone()),
        retryable: false,
    })?;
    // Leonardo was called by now: an answer litegen can't read is a vendor
    // failure, not a refusal before any call (E9). The presign is free, so it
    // may be asked again.
    let fields: HashMap<String, String> = serde_json::from_str(uii["fields"].as_str().unwrap_or("{}")).map_err(|e| ProviderError::RequestFailed {
        message: format!("Leonardo init-image fields parse: {e}"),
        status_code: None,
        provider_error: Some(data.clone()),
        retryable: true,
    })?;

    // POST the file to S3 with the presigned form fields: storage, not
    // Leonardo's API, so no auth and none of the operator's extra_headers.
    let mut form = Form::new();
    for (k, v) in fields {
        form = form.text(k, v);
    }
    form = form.part(
        "file",
        Part::bytes(bytes.to_vec())
            .file_name(format!("image.{extension}"))
            .mime_str(mime)
            .map_err(|e| ProviderError::RequestFailed { message: format!("leonardo ref mime: {e}"), status_code: None, provider_error: None, retryable: false })?,
    );
    let s3_resp = upload_client.post(s3_url).multipart(form).send().await.map_err(|e| ProviderError::RequestFailed {
        message: format!("Leonardo S3 upload failed: {e}"),
        status_code: None,
        provider_error: None,
        retryable: true,
    })?;
    if !s3_resp.status().is_success() {
        return Err(ProviderError::RequestFailed {
            message: format!("Leonardo S3 upload returned HTTP {}", s3_resp.status()),
            status_code: Some(s3_resp.status().as_u16()),
            provider_error: None,
            retryable: false,
        });
    }
    Ok(image_id.to_string())
}

/// A create's answer, shared by the image and video providers: its JSON on
/// success, otherwise an error with Leonardo's status that may be sent again
/// only when Leonardo turned the call away (429, 5xx).
pub(crate) async fn read_create(resp: reqwest::Response, what: &str) -> Result<Value, ProviderError> {
    let status = resp.status();
    let retry_after = crate::providers::retry_after_ms(resp.headers());
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let snippet: String = text.chars().take(200).collect();
        let body: Option<Value> = serde_json::from_str(&text).ok();
        let message = format!("{what} returned HTTP {}: {snippet}", status.as_u16());
        if let Some(e) = crate::providers::rate_limited("leonardo", status, retry_after, &message, body.as_ref()) {
            return Err(e);
        }
        return Err(ProviderError::RequestFailed {
            message: format!("{what} returned HTTP {}: {snippet}", status.as_u16()),
            status_code: Some(status.as_u16()),
            provider_error: serde_json::from_str(&text).ok(),
            retryable: status.as_u16() == 429 || status.is_server_error(),
        });
    }
    serde_json::from_str(&text).map_err(|e| ProviderError::RequestFailed {
        message: format!("Failed to parse {what} response: {e}"),
        status_code: Some(status.as_u16()),
        provider_error: None,
        retryable: false,
    })
}

/// What litegen itself sends to `POST /generations`; `extra` cannot replace it.
const OWNED_FIELDS: &[&str] = &[
    "prompt", "modelId", "width", "height", "num_images", "negative_prompt", "seed", "init_image_id", "init_strength",
    "guidance_scale", "num_inference_steps",
];

/// Leonardo.Ai image generation provider.
///
/// Async: `POST /generations` returns `sdGenerationJob.generationId`; poll
/// `GET /generations/{id}` until `generations_by_pk.status == "COMPLETE"`, then
/// download `generated_images[0].url`. Bearer auth. Reference images are
/// uploaded via the presigned init-image flow and passed as `init_image_id`.
///
/// @see <https://docs.leonardo.ai/v1.0/reference/creategeneration> (2026-10-30) — POST /generations
/// @see <https://docs.leonardo.ai/v1.0/reference/getgenerationbyid> (2026-10-30) — GET /generations/{id}
pub struct LeonardoImageProvider {
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

/// Leonardo Lightning XL — the `modelId` default of the v1 `POST /generations`
/// reference, used only when a model id resolves to nothing we know.
/// @see <https://docs.leonardo.ai/v1.0/reference/creategeneration>
const DEFAULT_MODEL_ID: &str = "b24e16ff-06e3-43eb-8d33-4416c2d75876";

/// Leonardo Diffusion XL, the model `leonardo/diffusion-xl` advertises.
/// @see <https://docs.leonardo.ai/v1.0/docs/commonly-used-api-values> — "Leonardo Diffusion XL | 1e60896f-3c26-4296-8ecc-53e2afecc132"
const DIFFUSION_XL_MODEL_ID: &str = "1e60896f-3c26-4296-8ecc-53e2afecc132";

impl LeonardoImageProvider {
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

    /// Resolve the Leonardo modelId (UUID). Honors config.model_mapping, a
    /// known alias, or an id that already is a UUID; anything else is `None`
    /// and refused rather than billed as another model (L13).
    fn resolve_model_id(&self, model_id: &str) -> Option<String> {
        if let Some(cfg) = &self.config {
            if let Some(m) = cfg.mapped_model(model_id) {
                return Some(m.to_string());
            }
        }
        match model_id.strip_prefix("leonardo/").unwrap_or(model_id) {
            "diffusion-xl" => Some(DIFFUSION_XL_MODEL_ID.to_string()),
            "lightning-xl" => Some(DEFAULT_MODEL_ID.to_string()),
            // Anything that already looks like a UUID is passed through.
            other if other.contains('-') && other.len() >= 32 => Some(other.to_string()),
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
                // The generation is Leonardo's now; a dropped poll is asked again.
                Err(_) => continue,
            };
            // A 5xx or 429 is asked again; any other error status is the answer.
            let data = match crate::providers::video::read_poll_json(resp, "leonardo").await {
                Ok(data) => data,
                Err(ProviderError::RequestFailed { retryable: true, .. }) => continue,
                Err(e) => return Err(e),
            };
            match data["generations_by_pk"]["status"].as_str() {
                Some("COMPLETE") => return Ok(data),
                Some("FAILED") => {
                    return Err(ProviderError::RequestFailed {
                        message: "Leonardo generation failed (status FAILED)".to_string(),
                        status_code: None,
                        provider_error: Some(data),
                        retryable: false,
                    })
                }
                // job_status: PENDING | COMPLETE | FAILED.
                Some("PENDING") => continue,
                other => return Err(super::unknown_state("leonardo", other)),
            }
        }
        Err(ProviderError::Timeout { timeout_ms: budget.total_ms(started) })
    }

    /// A non-2xx (an expired or throttled presigned CDN URL) is an error, not
    /// image bytes: storing its body would bill for a broken asset.
    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        super::fetch_result(&self.asset_client, url, self.public_assets_only, "leonardo image").await
    }
}

impl Default for LeonardoImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl LeonardoImageProvider {
    /// A finished job's result, as `generate` returns it: its image
    /// downloaded, with the vendor's ids.
    async fn finish(&self, gen_id: &str, result: Value) -> Result<GenerationOutput, ProviderError> {
        let image_url = result["generations_by_pk"]["generated_images"][0]["url"]
            .as_str()
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "Leonardo result missing generated_images[0].url".to_string(),
                status_code: None,
                provider_error: Some(result.clone()),
                retryable: false,
            })?;

        let bytes = self.fetch_image_bytes(image_url).await?;
        let mut metadata = HashMap::new();
        metadata.insert("generation_id".to_string(), Value::String(gen_id.to_string()));
        metadata.insert("url".to_string(), Value::String(image_url.to_string()));

        Ok(GenerationOutput { data: bytes, content_type: "image/png".to_string(), metadata, more: Vec::new() })
    }
}

#[async_trait]
impl ImageProvider for LeonardoImageProvider {
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
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let model_id = self.resolve_model_id(&model.id).ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "leonardo".into() })?;

        // Upload the first reference image (if any) -> init_image_id.
        let mut init_image_id: Option<String> = None;
        for r in &materialized.refs {
            if let MaterializedRefForm::MultipartField { bytes, .. } = &r.form {
                init_image_id = Some(upload_init_image(&self.client, &self.asset_client, self.api_base(), &self.auth, &creds, bytes).await?);
                break;
            }
        }

        let (mut w, mut h) = (1024u64, 768u64);
        if let Some(size) = extras.size.as_deref() {
            if let Some((sw, sh)) = size.split_once('x') {
                if let (Ok(a), Ok(b)) = (sw.parse(), sh.parse()) {
                    w = a;
                    h = b;
                }
            }
        }

        // One image: `num_images` defaults to 4, and litegen returns and bills
        // one. Alchemy off unless asked for: it defaults on, and "when using
        // Alchemy v2, the output dimension is 1.75 times bigger" than `size`.
        // @see <https://docs.leonardo.ai/v1.0/reference/creategeneration> (2026-11-09)
        // @see <https://docs.leonardo.ai/docs/legacy-faq.md> (2026-11-09)
        let mut body = json!({
            "prompt": base.prompt,
            "modelId": model_id,
            "width": w,
            "height": h,
            "num_images": 1,
            "alchemy": false,
        });
        if let Some(np) = base.negative_prompt.as_deref() {
            body["negative_prompt"] = Value::String(np.to_string());
        }
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }
        // "guidance_scale … Must be between 1 and 20"; "num_inference_steps …
        // Must be between 10 and 60. Default is 15."
        // @see <https://docs.leonardo.ai/v1.0/reference/creategeneration> (2026-11-03)
        if let Some(g) = extras.guidance_scale {
            body["guidance_scale"] = json!(g);
        }
        if let Some(steps) = extras.steps {
            body["num_inference_steps"] = json!(steps);
        }
        if let Some(id) = init_image_id {
            body["init_image_id"] = Value::String(id);
            if let Some(strength) = extras.strength {
                body["init_strength"] = json!(strength);
            }
        }
        if let Some(obj) = body.as_object_mut() {
            for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED_FIELDS.contains(&k)) {
                obj.insert(k.clone(), v.clone());
            }
        }

        let url = format!("{}/generations", self.api_base());
        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Leonardo request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // Only a connection that never opened is safe to retry: a
                // request that timed out may have been accepted and billed.
                retryable: e.is_connect(),
            })?;
        let submit = read_create(resp, "Leonardo").await?;

        let gen_id = submit["sdGenerationJob"]["generationId"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Leonardo response missing sdGenerationJob.generationId".to_string(),
            status_code: None,
            provider_error: Some(submit.clone()),
            retryable: false,
        })?;
        // Accepted: from here on a failure must not re-send the job (L1).
        extras.vendor_job.record(gen_id);

        let result = self.poll_generation(gen_id).await?;
        self.finish(gen_id, result).await
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
                message: "Leonardo provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Leonardo provider configured".into(), latency_ms: None }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::Cleanup;
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
        for m in catalog.for_provider("leonardo").filter(|m| m.media_type == crate::capabilities::MediaType::Image) {
            assert!(LeonardoImageProvider::new().resolve_model_id(&m.id).is_some(), "{} has no arm", m.id);
        }
        assert!(LeonardoImageProvider::new().resolve_model_id("leonardo/not-a-model").is_none());
    }

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> LeonardoImageProvider {
        let mut p = LeonardoImageProvider::new();
        let mut cfg = ProviderInstanceConfig { api_key: "leo-key".to_string(), api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.api_key = Some("leo-key".to_string());
        p.configure(cfg);
        p.public_assets_only = false;
        p
    }

    #[tokio::test]
    async fn fetch_image_bytes_errors_on_non_success_status() {
        // An expired/throttled presigned CDN URL returns a non-2xx with a short
        // error body. Draining it as if it were the image would store an error
        // blob and bill the customer for a broken asset, so the fetch must error.
        let cdn = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/expired.png"))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_string("<?xml version=\"1.0\"?><Error><Code>AccessDenied</Code></Error>"),
            )
            .mount(&cdn)
            .await;

        let p = make_provider("http://unused.invalid");
        let result = p.fetch_image_bytes(&format!("{}/expired.png", cdn.uri())).await;
        assert!(
            result.is_err(),
            "fetch_image_bytes must error on HTTP 403, got Ok with {:?} bytes",
            result.map(|b| b.len())
        );
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(), model: model.to_string(), n: 1, negative_prompt: None,
            seed: None, reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    fn make_extras() -> ImageExtras {
        ImageExtras {
            vendor_job: Default::default(),
            size: Some("1024x768".to_string()), resolution: None, aspect_ratio: None, quality: None, style: None,
            steps: None, guidance_scale: None, strength: None, response_format: "url".to_string(), extra: None,
            edit: None, views: Vec::new(),
            output_format: None, background: None,
            expand: None,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        }
    }

    #[tokio::test]
    async fn generates_via_job_and_poll() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/img.png", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sdGenerationJob": { "generationId": "leo-gen-1", "apiCreditCost": 8 }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/generations/leo-gen-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "generations_by_pk": { "id": "leo-gen-1", "status": "COMPLETE",
                    "generated_images": [{ "id": "img1", "url": image_url.clone() }] }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"LEOPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("leonardo/diffusion-xl");
        let base = make_base("an enchanted forest", "leonardo/diffusion-xl");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        // The vendor accepted the job before the poll: the router must never re-send it.
        assert_eq!(extras.vendor_job.get().as_deref(), Some("leo-gen-1"));
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"LEOPNG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("authorization").unwrap(), "Bearer leo-key");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["prompt"], "an enchanted forest");
        assert_eq!(body["modelId"], DIFFUSION_XL_MODEL_ID);
        assert_eq!(body["width"], 1024);
    }

    /// Leonardo's own table: Diffusion XL is 1e60896f-…; b24e16ff-… (what
    /// `leonardo/diffusion-xl` used to send) is Leonardo Lightning XL.
    /// @see <https://docs.leonardo.ai/v1.0/docs/commonly-used-api-values>
    #[test]
    fn diffusion_xl_resolves_to_the_diffusion_xl_uuid() {
        let p = make_provider("http://unused.invalid");
        assert_eq!(
            p.resolve_model_id("leonardo/diffusion-xl").as_deref(),
            Some("1e60896f-3c26-4296-8ecc-53e2afecc132")
        );
    }

    // ─── Leonardo audit, 2026-10-30 ─────────────────────────────────────────

    fn no_refs() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    /// Leonardo answers every create with a 400, so only the outbound body matters.
    async fn refusing_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/generations"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": "stop" })))
            .mount(&server)
            .await;
        server
    }

    async fn created_body(server: &MockServer) -> Value {
        let received = server.received_requests().await.unwrap();
        let create = received.iter().find(|r| r.url.path() == "/generations").expect("no create");
        serde_json::from_slice(&create.body).unwrap()
    }

    /// #4, #10, #11, #16: `num_images` defaults to 4 ("Must be between 1 and
    /// 8"), so one is asked for; `alchemy` defaults to true and makes the
    /// output 1.75 times the requested size, so it is sent off; `extra` cannot
    /// replace the model litegen bills or anything else litegen sends.
    /// @see <https://docs.leonardo.ai/v1.0/reference/creategeneration> (2026-11-09)
    /// @see <https://docs.leonardo.ai/docs/legacy-faq.md> (2026-11-09)
    #[tokio::test]
    async fn one_plain_image_is_asked_for_and_extra_cannot_replace_what_litegen_sends() {
        let server = refusing_server().await;
        let mut base = make_base("a quiet harbour", "leonardo/diffusion-xl");
        base.n = 4;
        let mut extras = make_extras();
        extras.extra = Some(json!({
            "modelId": "aa77f04e-3eec-4034-9c07-d0f619684628", "prompt": "other", "num_images": 8,
            "width": 64, "height": 64, "init_image_id": "x", "presetStyle": "CINEMATIC"
        }));
        let _ = make_provider(&server.uri()).generate(&ref_schema("leonardo/diffusion-xl"), &base, &extras, &no_refs()).await;
        let body = created_body(&server).await;
        assert_eq!(body["modelId"], DIFFUSION_XL_MODEL_ID);
        assert_eq!(body["prompt"], "a quiet harbour");
        assert_eq!(body["num_images"], 1);
        assert_eq!((body["width"].clone(), body["height"].clone()), (json!(1024), json!(768)));
        assert!(body.get("init_image_id").is_none(), "{body}");
        assert_eq!(body["alchemy"], false);
        assert_eq!(body["presetStyle"], "CINEMATIC");

        // A caller may still ask for Alchemy.
        let server = refusing_server().await;
        let mut extras = make_extras();
        extras.extra = Some(json!({ "alchemy": true }));
        let _ = make_provider(&server.uri()).generate(&ref_schema("leonardo/diffusion-xl"), &base, &extras, &no_refs()).await;
        assert_eq!(created_body(&server).await["alchemy"], true);
    }

    /// E4a: whether the images show in Leonardo's community feed (`public`)
    /// is litegen's: never sent, so the account's own default applies; the
    /// endpoint's other fields still go.
    #[tokio::test]
    async fn public_is_never_sent() {
        let server = refusing_server().await;
        let mut extras = make_extras();
        extras.extra = Some(json!({ "public": true, "alchemy": true }));
        let base = make_base("a quiet harbour", "leonardo/diffusion-xl");
        let _ = make_provider(&server.uri()).generate(&ref_schema("leonardo/diffusion-xl"), &base, &extras, &no_refs()).await;
        let body = created_body(&server).await;
        assert!(body.get("public").is_none(), "{body}");
        assert_eq!(body["alchemy"], true, "{body}");
    }

    /// "guidance_scale … How strongly the generation should reflect the
    /// prompt. 7 is recommended. Must be between 1 and 20" and
    /// "num_inference_steps … The Step Count to use for the generation. Must be
    /// between 10 and 60. Default is 15" carry the canonical `guidance_scale`
    /// and `steps`; `extra` cannot replace either.
    /// @see <https://docs.leonardo.ai/v1.0/reference/creategeneration> (2026-11-03)
    #[tokio::test]
    async fn guidance_and_steps_go_as_leonardos_own_fields() {
        let server = refusing_server().await;
        let mut extras = make_extras();
        extras.guidance_scale = Some(9.5);
        extras.steps = Some(40);
        extras.extra = Some(json!({ "guidance_scale": 2, "num_inference_steps": 10 }));
        let _ = make_provider(&server.uri()).generate(&ref_schema("leonardo/diffusion-xl"), &make_base("a quiet harbour", "leonardo/diffusion-xl"), &extras, &no_refs()).await;
        let body = created_body(&server).await;
        assert_eq!((body["guidance_scale"].as_f64(), body["num_inference_steps"].as_u64()), (Some(9.5), Some(40)), "{body}");
    }

    /// #12: `extension` "Has to be png, jpg, jpeg, or webp", and "the image
    /// must be in the format of the `extension` provided"; anything else is
    /// refused before any call.
    /// @see <https://docs.leonardo.ai/v1.0/reference/uploadinitimage> (2026-10-30)
    #[tokio::test]
    async fn a_reference_goes_up_as_its_own_type_and_a_gif_is_refused() {
        let server = refusing_server().await;
        let s3 = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/init-image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "uploadInitImage": { "id": "init-1", "url": format!("{}/upload", s3.uri()), "fields": "{\"key\":\"k\"}" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST")).and(path("/upload")).respond_with(ResponseTemplate::new(204)).mount(&s3).await;
        let one = |bytes: &[u8]| MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".into(),
                form: MaterializedRefForm::MultipartField {
                    field_name: "file".into(),
                    bytes: bytes::Bytes::copy_from_slice(bytes),
                    content_type: "application/octet-stream".into(),
                },
                time_s: None,
            }],
            cleanup: Cleanup::empty(),
        };
        let provider = make_provider(&server.uri());
        let schema = ref_schema("leonardo/diffusion-xl");
        let base = make_base("x", "leonardo/diffusion-xl");
        let _ = provider.generate(&schema, &base, &make_extras(), &one(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 16])).await;
        let received = server.received_requests().await.unwrap();
        let init = received.iter().find(|r| r.url.path() == "/init-image").expect("no init-image call");
        assert_eq!(serde_json::from_slice::<Value>(&init.body).unwrap()["extension"], "jpg");
        let upload = String::from_utf8_lossy(&s3.received_requests().await.unwrap()[0].body).to_string();
        assert!(upload.contains("Content-Type: image/jpeg") && upload.contains("image.jpg"), "{upload}");
        assert_eq!(created_body(&server).await["init_image_id"], "init-1");

        let before = server.received_requests().await.unwrap().len();
        let err = provider.generate(&schema, &base, &make_extras(), &one(b"GIF89a\x01\x00\x01\x00")).await.unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        assert_eq!(server.received_requests().await.unwrap().len(), before, "a GIF reached Leonardo");
    }

    /// E9: a presign answer litegen can't read (`uploadInitImage.fields`)
    /// comes after a call to Leonardo: a vendor failure that may be asked
    /// again, not the adapter's refusal before any call.
    #[tokio::test]
    async fn a_presign_answer_that_cannot_be_read_is_a_vendor_failure() {
        let server = refusing_server().await;
        Mock::given(method("POST"))
            .and(path("/init-image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "uploadInitImage": { "id": "init-1", "url": "http://127.0.0.1:9/upload", "fields": "not json" }
            })))
            .mount(&server)
            .await;
        let jpeg = MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".into(),
                form: MaterializedRefForm::MultipartField {
                    field_name: "file".into(),
                    bytes: bytes::Bytes::from_static(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 16]),
                    content_type: "image/jpeg".into(),
                },
                time_s: None,
            }],
            cleanup: Cleanup::empty(),
        };
        let schema = ref_schema("leonardo/diffusion-xl");
        let err = make_provider(&server.uri()).generate(&schema, &make_base("x", "leonardo/diffusion-xl"), &make_extras(), &jpeg).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: true, .. }), "{err:?}");
    }

    /// #17, #19: a create Leonardo turned away says so, with its status — a
    /// 429 or 5xx may be sent again, another 4xx may not — and one that timed
    /// out may have been accepted and billed, so it is never sent again.
    #[tokio::test]
    async fn a_turned_away_create_says_so_and_a_timed_out_one_is_never_sent_again() {
        let schema = ref_schema("leonardo/diffusion-xl");
        let base = make_base("x", "leonardo/diffusion-xl");
        for (status, retryable) in [(429, true), (503, true), (400, false)] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!({ "error": "no" })))
                .mount(&server)
                .await;
            let err = make_provider(&server.uri()).generate(&schema, &base, &make_extras(), &no_refs()).await.unwrap_err();
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
        let err = p.generate(&schema, &base, &make_extras(), &no_refs()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
    }

    /// #13: a poll Leonardo answers with an error is that error, not a
    /// generation still running.
    #[tokio::test]
    async fn a_poll_error_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "sdGenerationJob": { "generationId": "g1" } })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/generations/g1"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({ "error": "Invalid token" })))
            .mount(&server)
            .await;
        let err = make_provider(&server.uri())
            .generate(&ref_schema("leonardo/diffusion-xl"), &make_base("x", "leonardo/diffusion-xl"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { status_code: Some(401), retryable: false, .. }), "{err:?}");
    }

    /// #23: the result is fetched without the key, only from a public address,
    /// and at most 64 MiB.
    #[tokio::test]
    async fn a_result_is_fetched_only_from_a_public_address() {
        let server = MockServer::start().await;
        let cdn = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "sdGenerationJob": { "generationId": "g1" } })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/generations/g1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "generations_by_pk": { "status": "COMPLETE", "generated_images": [{ "url": format!("{}/out.jpg", cdn.uri()) }] }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0xFF, 0xD8, 0xFF])).mount(&cdn).await;
        let mut provider = make_provider(&server.uri());
        provider.public_assets_only = true;
        let err = provider
            .generate(&ref_schema("leonardo/diffusion-xl"), &make_base("x", "leonardo/diffusion-xl"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
        assert!(cdn.received_requests().await.unwrap().is_empty(), "a private address was fetched");
    }
}
