use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras, ImageProvider,
    ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// Kling (Kuaishou) image generation provider.
///
/// Async: `POST /v1/images/generations` returns `data.task_id`; poll
/// `GET /v1/images/generations/{task_id}` until `data.task_status == "succeed"`,
/// then download `data.task_result.images[0].url`. Auth: the account's API
/// key, `Authorization: Bearer <key>` ([`AuthSpec::bearer`]), from the
/// dashboard's `api_keys` pool. Kling's access/secret-key JWT works only on
/// its legacy design, and no Kling adapter uses it since 2026-11-03.
///
/// @see <https://kling.ai/document-api/api/get-started/authentication.md> (2026-11-03)
///   Verbatim: "API Key (for all models) … API Key authentication must be used to call Kling AI API"
///   Verbatim: "Format: **Authorization = "Bearer XXX"**, where XXX is the **API Key**"
/// @see <https://kling.ai/document-api/api/image/3-0-omni/image-generation.md> (2026-11-03)
///   Verbatim: "- Path: `/v1/images/generations` - Auth: `Authorization: Bearer <API_KEY>`"
///   (response envelope {code, message, data:{task_id, task_status, task_result}})
pub struct KlingImageProvider {
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

impl KlingImageProvider {
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
            // "The API endpoint has been changed from https://api.klingai.com to
            // https://api-singapore.klingai.com."
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
        // An access/secret key pair says to store an API key, as on video.
        Err(crate::providers::video::kling::no_api_key(&base))
    }

    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("kling/").unwrap_or(model_id)
    }

    async fn poll_task(&self, task_id: &str) -> Result<Value, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/v1/images/generations/{}", self.api_base(), task_id);
        // As long as litegen waits for this call (the vendor timeout,
        // docs/gateway-settings.md); 180 s when called directly.
        let started = std::time::Instant::now();
        let mut budget = crate::providers::PollBudget::new(std::time::Duration::from_secs(180), std::time::Duration::from_secs(2));
        while budget.next().await {
            let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(&url))?;
            let resp = match crate::providers::inject_trace_headers(builder).send().await {
                Ok(resp) => resp,
                // The job is Kling's now; a dropped poll is asked again.
                Err(_) => continue,
            };
            // The status, then Kling's code: a passing failure (5xx, 429, a
            // rate-limit or server code) is asked again; any other is final.
            let data = match crate::providers::video::kling::read_kling(resp, "image poll").await {
                Ok(data) => data,
                Err(ProviderError::RequestFailed { retryable: true, .. } | ProviderError::RateLimited { .. }) => continue,
                Err(e) => return Err(e),
            };
            match data["data"]["task_status"].as_str() {
                Some("succeed") => return Ok(data),
                Some("failed") => {
                    return Err(ProviderError::RequestFailed {
                        message: format!("Kling task failed: {}", data["data"]["task_status_msg"].as_str().unwrap_or("")),
                        status_code: None,
                        provider_error: Some(data),
                        retryable: false,
                    })
                }
                // task_status: submitted | processing | succeed | failed.
                Some("submitted") | Some("processing") => continue,
                other => return Err(super::unknown_state("kling", other)),
            }
        }
        Err(ProviderError::Timeout { timeout_ms: budget.total_ms(started) })
    }

    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        super::fetch_result(&self.asset_client, url, self.public_assets_only, "kling image").await
    }
}

impl Default for KlingImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl KlingImageProvider {
    /// A finished job's result, as `generate` returns it: its image
    /// downloaded, with the vendor's ids.
    async fn finish(&self, task_id: &str, result: Value) -> Result<GenerationOutput, ProviderError> {
        let image_url = result["data"]["task_result"]["images"][0]["url"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Kling result missing task_result.images[0].url".to_string(),
            status_code: None,
            provider_error: Some(result.clone()),
            retryable: false,
        })?;

        let bytes = self.fetch_image_bytes(image_url).await?;
        let mut metadata = HashMap::new();
        metadata.insert("task_id".to_string(), Value::String(task_id.to_string()));
        metadata.insert("url".to_string(), Value::String(image_url.to_string()));

        Ok(GenerationOutput { data: bytes, content_type: "image/png".to_string(), metadata, more: Vec::new() })
    }
}

/// litegen's tier as Kling's: "resolution: 1k (default) | 2k" (Image 2.1 and
/// 3.0, the same price).
/// @see <https://kling.ai/document-api/api/image/2-1/image-generation.md> (2026-11-03)
fn kling_resolution(resolution: &str) -> Option<&'static str> {
    match resolution {
        "1K" => Some("1k"),
        "2K" => Some("2k"),
        _ => None,
    }
}

#[async_trait]
impl ImageProvider for KlingImageProvider {
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
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);
        let url = format!("{}/v1/images/generations", self.api_base());

        // "In the Image-to-Image scenario (when the "image" field is not empty),
        // negative prompts are not supported."
        // @see <https://kling.ai/document-api/api/image/2-1/image-generation.md>
        if base.negative_prompt.is_some() && !materialized.refs.is_empty() {
            return Err(ProviderError::InvalidRequest(format!("{}: a negative_prompt cannot go with a reference image", model.id)));
        }
        // One image: litegen returns and bills one, whatever `n` was.
        let mut body = json!({
            "model_name": crate::providers::wire_model(self.config.as_ref(), &model.id, native),
            "prompt": base.prompt,
            "n": 1,
        });
        if let Some(np) = base.negative_prompt.as_deref() {
            body["negative_prompt"] = Value::String(np.to_string());
        }
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }
        if let Some(tier) = extras.resolution.as_deref() {
            let r = kling_resolution(tier).ok_or_else(|| ProviderError::InvalidRequest(format!("{}: Kling has no image resolution {tier}", model.id)))?;
            body["resolution"] = Value::String(r.into());
        }
        // Reference image as base64 (no data: prefix) or URL. A `subject` is
        // Kling's subject reference ("`image_reference` … `subject`, `face`");
        // without it the image is plain image-to-image. The type the caller
        // names (`extra.image_reference`, allowlisted) is the one sent.
        let named = extras.extra.as_ref().and_then(|e| e.get("image_reference")).and_then(Value::as_str);
        for r in &materialized.refs {
            match &r.form {
                MaterializedRefForm::Base64(b64) => body["image"] = Value::String(b64.clone()),
                MaterializedRefForm::Url(u) => body["image"] = Value::String(u.clone()),
                _ => continue,
            }
            if let Some(kind) = named.or((r.role == "subject").then_some("subject")) {
                body["image_reference"] = Value::String(kind.into());
            }
        }
        // `extra` adds the endpoint's own fields; it cannot replace one litegen
        // sets (rule 11).
        if let Some(obj) = body.as_object_mut() {
            let owned = |k: &str| matches!(k, "model_name" | "prompt" | "n" | "negative_prompt" | "aspect_ratio" | "resolution" | "image" | "image_reference");
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
                message: format!("Kling request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // A call that timed out may still run, and bill.
                retryable: e.is_connect(),
            })?;
        let submit = crate::providers::video::kling::read_kling(resp, "image submit").await?;

        let task_id = submit["data"]["task_id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Kling response missing data.task_id".to_string(),
            status_code: None,
            provider_error: Some(submit.clone()),
            retryable: false,
        })?;
        // Accepted: from here on a failure must not re-send the job (L1).
        extras.vendor_job.record(task_id);

        let result = self.poll_task(task_id).await?;
        self.finish(task_id, result).await
    }

    fn resumable(&self, _model: &ModelSchema) -> bool {
        true
    }

    /// One look at an accepted job, read as `generate`'s own poll loop reads it.
    async fn poll_job(&self, _model: &str, job: &str, _resume: Option<&Value>) -> Result<crate::providers::ImageJobPoll, ProviderError> {
        match super::poll_once(self.poll_task(job)).await {
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
                message: "Kling provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Kling provider configured".into(), latency_ms: None }
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

    /// "API Key authentication must be used to call Kling AI API": the
    /// dashboard's pool (`api_keys`), as a BYO credential arrives.
    fn make_provider(api_base: &str) -> KlingImageProvider {
        let mut p = KlingImageProvider::new();
        let cfg = ProviderInstanceConfig {
            api_base: Some(api_base.to_string()),
            api_keys: vec![crate::types::ApiKeyEntry { key: "kling-key".into(), weight: 1, label: None }],
            ..Default::default()
        };
        p.configure(cfg);
        // The mock vendor serves results from loopback.
        p.public_assets_only = false;
        p
    }

    #[tokio::test]
    async fn fetch_image_bytes_errors_on_non_success_status() {
        // An expired/throttled presigned CDN URL returns a non-2xx with a short
        // error body. Draining it as if it were the image would store an XML/JSON
        // error blob to S3 and bill the customer for a broken asset, so the fetch
        // must surface an error instead of returning the body as bytes.
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
            size: None, resolution: None, aspect_ratio: Some("16:9".to_string()), quality: None, style: None,
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
    async fn generates_with_api_key_auth_and_poll() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/k.png", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/v1/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0, "message": "SUCCEED", "data": { "task_id": "kl-1", "task_status": "submitted" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/v1/images/generations/kl-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0, "data": { "task_status": "succeed",
                    "task_result": { "images": [{ "index": 0, "url": image_url.clone() }] } }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"KLINGPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("kling/kling-v2-1");
        let base = make_base("a jade dragon", "kling/kling-v2-1");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        // The vendor accepted the job before the poll: the router must never re-send it.
        assert_eq!(extras.vendor_job.get().as_deref(), Some("kl-1"));
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"KLINGPNG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        // "Authorization = "Bearer XXX"" with the pooled API key, not a JWT.
        // @see <https://kling.ai/document-api/api/get-started/authentication.md> (2026-11-03)
        let auth = post.headers.get("authorization").unwrap().to_str().unwrap();
        assert_eq!(auth, "Bearer kling-key");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["model_name"], "kling-v2-1");
        assert_eq!(body["prompt"], "a jade dragon");
    }

    // ─── Kling audit, 2026-10-30: K8–K10, K12 ───────────────────────────────

    fn subject_ref() -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef { role: "subject".into(), form: MaterializedRefForm::Url("https://x/face.png".into()), time_s: None }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    /// K8, K10: one image is asked for, and a `subject` reference is sent as
    /// Kling's subject reference ("`image_reference` … `subject`, `face`").
    #[tokio::test]
    async fn one_image_is_asked_for_and_a_subject_is_sent_as_one() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 1, "message": "stop here" })))
            .mount(&server)
            .await;
        let mut base = make_base("a knight", "kling/kling-v2-1");
        base.n = 4;
        let _ = make_provider(&server.uri()).generate(&ref_schema("kling/kling-v2-1"), &base, &make_extras(), &subject_ref()).await;
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!((body["n"].as_u64(), body["image_reference"].as_str()), (Some(1), Some("subject")), "{body}");
    }

    /// K10: a caller who names the reference type (`extra.image_reference`,
    /// allowlisted: "`subject`, `face`") gets it; `subject` is only the
    /// default for a `subject` reference that names none.
    #[tokio::test]
    async fn a_face_reference_the_caller_names_is_sent_as_one() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 1, "message": "stop here" })))
            .mount(&server)
            .await;
        let mut extras = make_extras();
        extras.extra = Some(json!({ "image_reference": "face" }));
        let base = make_base("a knight", "kling/kling-v2-1");
        let _ = make_provider(&server.uri()).generate(&ref_schema("kling/kling-v2-1"), &base, &extras, &subject_ref()).await;
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body["image_reference"].as_str(), Some("face"), "{body}");
    }

    /// K9: "In the Image-to-Image scenario … negative prompts are not
    /// supported" — refused before any call.
    #[tokio::test]
    async fn a_negative_prompt_with_a_reference_is_refused() {
        let server = MockServer::start().await;
        let mut base = make_base("a knight", "kling/kling-v2-1");
        base.negative_prompt = Some("blur".into());
        let err = make_provider(&server.uri()).generate(&ref_schema("kling/kling-v2-1"), &base, &make_extras(), &subject_ref()).await.unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// K12: a submit Kling turned away (rate limit, server error) may be
    /// sent again; one it refused may not.
    #[tokio::test]
    async fn a_turned_away_submit_may_be_sent_again() {
        for (status, code, retryable) in [(429, 1302, true), (500, 5000, true), (400, 1201, false)] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!({ "code": code, "message": "no" })))
                .mount(&server)
                .await;
            let err = make_provider(&server.uri())
                .generate(&ref_schema("kling/kling-v2-1"), &make_base("x", "kling/kling-v2-1"), &make_extras(), &crate::proxy::materializer::MaterializedRequest::default())
                .await
                .unwrap_err();
            assert!(matches!(err, ProviderError::RequestFailed { retryable: r, .. } if r == retryable), "{code}: {err:?}");
        }
    }

    /// "API Key authentication must be used to call Kling AI API"; an access /
    /// secret key pair is "only applicable to legacy version design
    /// standards". A credential that is only such a pair (an org's stored
    /// pair) is refused before any call, saying what to store instead.
    /// @see <https://kling.ai/document-api/api/get-started/authentication.md> (2026-11-03)
    #[tokio::test]
    async fn a_stored_key_pair_is_refused_naming_the_api_key_kling_needs() {
        let server = MockServer::start().await;
        let mut p = KlingImageProvider::new();
        p.configure(ProviderInstanceConfig {
            api_base: Some(server.uri()),
            credentials: ProviderCredentials { key_id: Some("ak".into()), key_secret: Some("sk".into()), ..Default::default() },
            ..Default::default()
        });
        let schema = ref_schema("kling/kling-v2-1");
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
        let err = p.generate(&schema, &make_base("x", "kling/kling-v2-1"), &make_extras(), &materialized).await.unwrap_err();
        assert!(matches!(&err, ProviderError::NotConfigured(m) if m.contains("API key") && m.contains("access/secret key pair")), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing was sent");
    }

    /// Kling's image tier: "resolution: 1k (default) | 2k" — litegen's `1K`
    /// and `2K`. Every tier a Kling image row declares maps; nothing else does.
    /// @see <https://kling.ai/document-api/api/image/2-1/image-generation.md> (2026-11-03)
    #[test]
    fn every_declared_resolution_is_one_of_klings() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let reg = crate::capabilities::CapabilityRegistry::from_dir(&dir).expect("load");
        let mut seen = 0;
        for m in reg.for_provider("kling").filter(|m| m.media_type == crate::capabilities::MediaType::Image) {
            if let Some(crate::capabilities::ParamSpec::String(s)) = m.params.get("resolution") {
                for v in &s.enum_values {
                    seen += 1;
                    assert!(matches!(kling_resolution(v), Some("1k" | "2k")), "{}: {v}", m.id);
                }
            }
        }
        assert!(seen > 0, "no Kling image row declares a resolution");
        assert_eq!((kling_resolution("4K"), kling_resolution("1k")), (None, None));
    }

    /// The tier goes as Kling's own `resolution`, and `extra` cannot replace it.
    #[tokio::test]
    async fn the_tier_goes_as_klings_resolution_and_extra_cannot_replace_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "code": 1201, "message": "stop" })))
            .mount(&server)
            .await;
        let mut extras = make_extras();
        extras.resolution = Some("2K".into());
        extras.extra = Some(json!({ "resolution": "1k" }));
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
        let _ = make_provider(&server.uri()).generate(&ref_schema("kling/kling-v3"), &make_base("x", "kling/kling-v3"), &extras, &materialized).await;
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body["resolution"], "2k", "{body}");
    }
}
