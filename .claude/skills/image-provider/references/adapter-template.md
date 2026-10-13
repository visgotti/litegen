# Image adapter template

This is an annotated skeleton for an async-polling image vendor, the most
involved image flow. If your vendor is synchronous, drop the polling and
download steps and decode the response instead. "Acme" is a placeholder:
replace every URL, field and status with what the vendor's docs actually say,
and quote them in the `@see … Verbatim:` lines. Adapt it rather than pasting
it whole. For a real file in the same style, read `providers/image/bfl.rs`.

## Contents

- [Adapter](#adapter-litegen-coresrcprovidersimageacmers)
- [Tests](#tests-same-file)
- [Catalog row](#catalog-row-modelsacmeyaml)
- [Live test](#live-test-litegen-coretestslive_providersrs)

## Adapter (`litegen-core/src/providers/image/acme.rs`)

```rust
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    build_cost_estimate, ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult,
    ImageExtras, ImageProvider, ProviderError, ProviderInstanceConfig,
};
use crate::types::*;

/// ONE wall-clock budget for the whole `generate()`: the submit, every poll
/// round trip and the download, not just the sleeps. The router times each
/// attempt out at 120 s and re-submits on a timeout, and the vendor bills the
/// second job too.
const GENERATE_BUDGET: std::time::Duration = std::time::Duration::from_secs(90);
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
/// Largest image we'll buffer from a vendor URL.
const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;
/// The vendor's API host. Our key never goes to any other host, except the
/// configured `api_base` host.
const ACME_API_HOST: &str = "api.acme.example";

/// Acme image provider.
///
/// Async: `POST {api_base}/v2/images` returns `{id, polling_url}`. Poll the
/// returned `polling_url` until `status == "ready"`, then download
/// `result.url`, a signed URL that expires after 10 min. Auth is
/// `Authorization: Bearer <key>`.
///
/// @see <https://docs.acme.example/reference/create-image> — submit
///   Verbatim: "POST https://api.acme.example/v2/images"
///   Verbatim: "Authorization: Bearer $ACME_API_KEY"
/// @see <https://docs.acme.example/reference/get-image> — poll
///   Verbatim: "status: one of queued, running, ready, failed, moderated"
/// @see <https://docs.acme.example/errors> — error envelope `{"error": {"code", "message"}}`
pub struct AcmeImageProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
    /// Downloads vendor-returned URLs: no redirects (SSRF), short timeout.
    download_client: Client,
    /// Refuse download URLs that don't resolve to a public address. True in
    /// production. Tests turn it off to download from wiremock on 127.0.0.1,
    /// and one test proves the refusal (`model3d/fal.rs` uses the same switch).
    public_downloads_only: bool,
}

impl Default for AcmeImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AcmeImageProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30)) // per request; GENERATE_BUDGET bounds the whole call
                .build()
                .expect("failed to build HTTP client"),
            download_client: crate::util::ssrf::no_redirect_client_with_timeout(std::time::Duration::from_secs(30)),
            public_downloads_only: true,
        }
    }

    /// Whether our key may be sent to `url`: only Acme's API host or the
    /// configured `api_base` host. A vendor-returned URL is untrusted input.
    fn is_trusted_api_url(&self, url: &str) -> bool {
        let host = |u: &str| reqwest::Url::parse(u).ok().and_then(|u| u.host_str().map(str::to_string));
        match host(url) {
            Some(h) => h == ACME_API_HOST || Some(h) == host(self.api_base()),
            None => false,
        }
    }

    /// The only place a base URL comes from.
    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.acme.example")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("acme".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// Maps a catalog id to the `model` value Acme documents. There is one
    /// arm per image row in `models/acme.yaml`. An unknown id is an error and
    /// never falls back to a default model, which would bill a typo as some
    /// other model. When catalog suffixes equal vendor ids, a verbatim
    /// pass-through (`id.strip_prefix("acme/")`) is fine instead.
    fn resolve_model(id: &str) -> Result<&'static str, ProviderError> {
        match id {
            // @see <https://docs.acme.example/models/photo-2> — API page
            // @see <https://docs.acme.example/openapi.json> (2026-10-12) — CreateImageRequest
            // @see <https://acme.example/pricing.md> (2026-10-12) — "$0.04 per image"
            "acme/photo-2" => Ok("photo-2"),
            other => Err(ProviderError::ModelNotSupported {
                model: other.to_string(),
                provider: "acme".into(),
            }),
        }
    }

    /// Builds the CreateImageRequest body.
    ///
    /// `extra` goes in FIRST. With `strict: false` it arrives unfiltered, so
    /// everything this adapter owns is written afterwards and wins. `n` is
    /// never forwarded: litegen returns and bills one image.
    fn build_body(
        native: &str,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<Value, ProviderError> {
        let mut body = match &extras.extra {
            Some(Value::Object(m)) => Value::Object(m.clone()),
            _ => json!({}),
        };
        body["model"] = json!(native);
        body["prompt"] = json!(base.prompt);
        // We ask for PNG, so `content_type: image/png` below is true.
        body["output_format"] = json!("png");
        if let Some(seed) = base.seed {
            body["seed"] = json!(seed); // CreateImageRequest.seed
        }
        if let Some(neg) = base.negative_prompt.as_deref() {
            body["negative_prompt"] = json!(neg);
        }
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = json!(ar); // the YAML `allowed` list IS the vendor enum
        }

        let mut styles = Vec::new();
        for r in &materialized.refs {
            let MaterializedRefForm::Base64(b64) = &r.form else {
                return Err(ProviderError::InvalidRequest(format!(
                    "{}: reference '{}' did not arrive as base64", model.id, r.role
                )));
            };
            match r.role.as_str() {
                "init" => body["image"] = json!(b64), // CreateImageRequest.image (image-to-image)
                "style" => styles.push(json!(b64)),   // CreateImageRequest.style_images
                other => {
                    return Err(ProviderError::InvalidRequest(format!(
                        "{}: unsupported reference role '{other}'", model.id
                    )))
                }
            }
        }
        if !styles.is_empty() {
            body["style_images"] = Value::Array(styles);
        }
        Ok(body)
    }

    /// Polls until the job is ready. Every error from here on is
    /// non-retryable: Acme has accepted (and will bill) the job, and a router
    /// retry would submit a second one.
    async fn wait_for_result(
        &self,
        creds: &ProviderCredentials,
        polling_url: &str,
        job_id: &str,
        deadline: std::time::Instant,
    ) -> Result<Value, ProviderError> {
        let fail = |message: String, status_code: Option<u16>, body: Option<Value>| {
            ProviderError::RequestFailed { message, status_code, provider_error: body, retryable: false }
        };
        // Stop polling early enough to leave time for the download.
        while std::time::Instant::now() + POLL_INTERVAL + std::time::Duration::from_secs(10) < deadline {
            tokio::time::sleep(POLL_INTERVAL).await;
            let builder = crate::providers::auth::apply(&self.auth, creds, self.client.get(polling_url))?;
            let resp = crate::providers::inject_trace_headers(builder)
                .send()
                .await
                .map_err(|e| fail(format!("Acme poll for job {job_id} failed: {e}"), None, None))?;
            let http = resp.status();
            let text = resp.text().await.unwrap_or_default();
            let data: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if !http.is_success() {
                return Err(fail(
                    format!("Acme poll for job {job_id} returned HTTP {http}: {}", acme_error_message(&data, &text)),
                    Some(http.as_u16()),
                    Some(data),
                ));
            }
            match data["status"].as_str() {
                Some("ready") => return Ok(data),
                Some("queued") | Some("running") => continue,
                Some(s @ ("failed" | "moderated")) => {
                    return Err(fail(
                        format!("Acme job {job_id} {s}: {}", acme_error_message(&data, &text)),
                        None,
                        Some(data),
                    ))
                }
                // A state the docs don't list is an error, not "still running".
                other => {
                    return Err(fail(format!("Acme job {job_id} reported an unknown status {other:?}"), None, Some(data)))
                }
            }
        }
        // Out of budget. Non-retryable: a router retry would submit and bill
        // a second job. Name the vendor job so support can find it.
        Err(fail(format!("Acme job {job_id} was not ready within the generate budget"), None, None))
    }

    /// Downloads the signed result URL with no auth header, because it points
    /// at Acme's CDN, not the API. The URL came from a response, so it is
    /// checked for a public address first and fetched without following
    /// redirects.
    async fn download(&self, url: &str) -> Result<(Vec<u8>, Option<String>), ProviderError> {
        let fail = |message: String, status_code: Option<u16>| ProviderError::RequestFailed {
            message,
            status_code,
            provider_error: None,
            retryable: false,
        };
        if self.public_downloads_only {
            crate::util::ssrf::validate_public_url(url)
                .await
                .map_err(|why| fail(format!("Acme result URL refused: {why}"), None))?;
        }
        let resp = self.download_client.get(url).send().await.map_err(|e| fail(format!("Acme image download failed: {e}"), None))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(fail(format!("Acme image download returned HTTP {status}"), Some(status.as_u16())));
        }
        if resp.content_length().is_some_and(|n| n > MAX_IMAGE_BYTES) {
            return Err(fail("Acme image is over the size cap".into(), None));
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(';').next().unwrap_or(s).trim().to_string());
        let bytes = resp.bytes().await.map_err(|e| fail(format!("Acme image body failed: {e}"), None))?;
        if bytes.len() as u64 > MAX_IMAGE_BYTES {
            return Err(fail("Acme image is over the size cap".into(), None));
        }
        Ok((bytes.to_vec(), content_type))
    }
}

/// Acme returns `{"error": {"message"}}`, and its gateway returns
/// `{"detail": "…"}`. Reading only one shape turns a precise vendor message
/// into a generic one exactly when someone needs it.
/// @see <https://docs.acme.example/errors>
fn acme_error_message(body: &Value, raw: &str) -> String {
    body["error"]["message"]
        .as_str()
        .or_else(|| body["detail"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| raw.chars().take(300).collect())
}

#[async_trait]
impl ImageProvider for AcmeImageProvider {
    fn name(&self) -> &str {
        "acme"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        // Guarded: ApiKeyPool::shared panics on an empty list, and configs
        // without a pool are normal (a single BYO key, or a test default).
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
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let deadline = std::time::Instant::now() + GENERATE_BUDGET;
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id)?;
        let body = Self::build_body(native, model, base, extras, materialized)?;

        let url = format!("{}/v2/images", self.api_base());
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.post(&url).json(&body))?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Acme request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // Only a connect failure proves nothing was sent. After the
                // request went out, a timeout may hide a job Acme already
                // accepted and billed, so retrying would pay twice.
                retryable: e.is_connect(),
            })?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let data: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Acme API error (HTTP {status}): {}", acme_error_message(&data, &text)),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                // Rate limits and server errors on submit are safe to retry; 4xx is not.
                retryable: status.is_server_error() || status.as_u16() == 429,
            });
        }

        let job_id = data["id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Acme response is missing `id`".into(),
            status_code: Some(status.as_u16()),
            provider_error: Some(data.clone()),
            retryable: false,
        })?;
        // Prefer the URL Acme hands back, but only if our key may go there;
        // otherwise use the documented path on our own api_base.
        let polling_url = data["polling_url"]
            .as_str()
            .filter(|u| self.is_trusted_api_url(u))
            .map(str::to_string)
            .unwrap_or_else(|| format!("{}/v2/images/{job_id}", self.api_base()));

        let result = self.wait_for_result(&creds, &polling_url, job_id, deadline).await?;
        let image_url = result["result"]["url"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: format!("Acme job {job_id} is ready but has no result.url"),
            status_code: None,
            provider_error: Some(result.clone()),
            retryable: false,
        })?;
        let (bytes, content_type) = self.download(image_url).await?;

        let mut metadata = HashMap::new();
        metadata.insert("acme_job_id".to_string(), json!(job_id));
        if let Some(rp) = result["result"]["revised_prompt"].as_str() {
            metadata.insert("revised_prompt".to_string(), json!(rp)); // the router surfaces this key
        }
        Ok(GenerationOutput {
            data: bytes,
            content_type: content_type.filter(|c| c.starts_with("image/")).unwrap_or_else(|| "image/png".into()),
            metadata,
        })
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &ImageGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        // Exactly what the router bills: the catalog's flat base price.
        Ok(build_cost_estimate(
            model.pricing.base_cost_usd,
            0.0,
            CostSource::Estimated,
            Some(json!({ "model": model.id })),
        ))
    }

    async fn health_check(&self) -> HealthCheckResult {
        HealthCheckResult {
            healthy: self.is_configured(),
            message: if self.is_configured() { "Acme provider configured" } else { "Acme provider not configured" }.into(),
            latency_ms: None,
        }
    }
}
```

## Tests (same file)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{CapabilityRegistry, MediaType};
    use crate::proxy::materializer::{Cleanup, MaterializedRef};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn registry() -> CapabilityRegistry {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        CapabilityRegistry::from_dir(&p).expect("load models/")
    }

    fn ref_schema(id: &str) -> ModelSchema {
        registry().get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> AcmeImageProvider {
        let mut p = AcmeImageProvider::new();
        let mut cfg = ProviderInstanceConfig { api_base: Some(api_base.into()), ..Default::default() };
        cfg.credentials.api_key = Some("acme-key".into());
        p.configure(cfg);
        p.public_downloads_only = false; // wiremock serves downloads from 127.0.0.1
        p
    }

    fn make_base(model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: "a lighthouse at dusk".into(),
            model: model.into(),
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
            size: None,
            aspect_ratio: None,
            quality: None,
            style: None,
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".into(),
            extra: None,
        }
    }

    fn no_refs() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    /// submit → poll (ready) → download, all on one mock server.
    async fn mount_happy_path(server: &MockServer) {
        let base = server.uri();
        Mock::given(method("POST")).and(path("/v2/images"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "job-1", "polling_url": format!("{base}/v2/images/job-1")
            })))
            .mount(server).await;
        Mock::given(method("GET")).and(path("/v2/images/job-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "ready", "result": { "url": format!("{base}/cdn/out.png") }
            })))
            .mount(server).await;
        Mock::given(method("GET")).and(path("/cdn/out.png"))
            .respond_with(ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(b"\x89PNG-bytes".to_vec()))
            .mount(server).await;
    }

    async fn submitted_body(server: &MockServer) -> Value {
        let reqs = server.received_requests().await.unwrap();
        let post = reqs.iter().find(|r| r.method == wiremock::http::Method::POST).expect("a submit");
        serde_json::from_slice(&post.body).unwrap()
    }

    #[test]
    fn every_catalogued_acme_image_model_resolves() {
        let reg = registry();
        let ids: Vec<String> = reg
            .for_provider("acme")
            .filter(|m| m.media_type == MediaType::Image)
            .map(|m| m.id.clone())
            .collect();
        assert!(!ids.is_empty(), "models/acme.yaml has no image rows");
        for id in &ids {
            assert!(AcmeImageProvider::resolve_model(id).is_ok(), "{id} has no resolution arm");
        }
        assert!(AcmeImageProvider::resolve_model("acme/not-a-model").is_err(), "unknown ids must not fall back");
    }

    #[tokio::test]
    async fn generates_photo_2_with_its_declared_params_on_the_wire() {
        let server = MockServer::start().await;
        mount_happy_path(&server).await;
        let mut base = make_base("acme/photo-2");
        base.seed = Some(7);
        base.negative_prompt = Some("blurry".into());
        let mut extras = make_extras();
        extras.aspect_ratio = Some("16:9".into());

        let out = make_provider(&server.uri())
            .generate(&ref_schema("acme/photo-2"), &base, &extras, &no_refs())
            .await
            .expect("generate");
        assert_eq!(out.data, b"\x89PNG-bytes");
        assert_eq!(out.content_type, "image/png");

        let body = submitted_body(&server).await;
        assert_eq!(body["model"], "photo-2");
        assert_eq!(body["seed"], 7);                 // vendor name AND value
        assert_eq!(body["negative_prompt"], "blurry");
        assert_eq!(body["aspect_ratio"], "16:9");
    }

    #[tokio::test]
    async fn a_pooled_dashboard_key_authenticates_the_submit() {
        let server = MockServer::start().await;
        mount_happy_path(&server).await;
        let mut p = AcmeImageProvider::new();
        p.configure(
            ProviderInstanceConfig { api_base: Some(server.uri()), ..Default::default() }.with_credentials(
                crate::providers::ProviderCredentials::from_json(&json!({ "api_keys": [{ "key": "pooled" }] })),
            ),
        );
        p.public_downloads_only = false;
        assert!(p.is_configured(), "a pool-only BYO credential must configure the adapter");
        p.generate(&ref_schema("acme/photo-2"), &make_base("acme/photo-2"), &make_extras(), &no_refs())
            .await
            .expect("generate");
        let reqs = server.received_requests().await.unwrap();
        let post = reqs.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("authorization").unwrap(), "Bearer pooled");
    }

    #[tokio::test]
    async fn n_is_never_forwarded_and_extra_cannot_override_the_model() {
        let server = MockServer::start().await;
        mount_happy_path(&server).await;
        let mut base = make_base("acme/photo-2");
        base.n = 4; // the input that WOULD produce the field
        let mut extras = make_extras();
        extras.extra = Some(json!({ "model": "photo-ultra", "output_format": "webp" }));
        make_provider(&server.uri())
            .generate(&ref_schema("acme/photo-2"), &base, &extras, &no_refs())
            .await
            .expect("generate");
        let body = submitted_body(&server).await;
        assert!(body.get("n").is_none(), "n must not reach the vendor");
        assert_eq!(body["model"], "photo-2", "extra must not choose the billed model");
        assert_eq!(body["output_format"], "png", "extra must not change the format content_type claims");
    }

    #[tokio::test]
    async fn an_init_reference_is_sent_as_the_image_field() {
        let server = MockServer::start().await;
        mount_happy_path(&server).await;
        let refs = MaterializedRequest {
            refs: vec![MaterializedRef { role: "init".into(), form: MaterializedRefForm::Base64("QUJD".into()) }],
            cleanup: Cleanup::empty(),
        };
        make_provider(&server.uri())
            .generate(&ref_schema("acme/photo-2"), &make_base("acme/photo-2"), &make_extras(), &refs)
            .await
            .expect("generate");
        assert_eq!(submitted_body(&server).await["image"], "QUJD");
    }

    #[tokio::test]
    async fn a_moderated_job_fails_with_the_vendor_reason_and_is_not_retried() {
        let server = MockServer::start().await;
        let base = server.uri();
        Mock::given(method("POST")).and(path("/v2/images"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "job-2", "polling_url": format!("{base}/v2/images/job-2")
            })))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/v2/images/job-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "moderated", "error": { "message": "prompt flagged" }
            })))
            .mount(&server).await;
        let err = make_provider(&server.uri())
            .generate(&ref_schema("acme/photo-2"), &make_base("acme/photo-2"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(!err.is_retryable(), "a retry would re-submit a billed job");
        assert!(err.to_string().contains("prompt flagged"), "{err}");
    }

    #[tokio::test]
    async fn the_key_never_goes_to_an_untrusted_polling_url() {
        let server = MockServer::start().await; // stands in for api_base
        let elsewhere = MockServer::start().await; // a host the response names
        let base = server.uri();
        Mock::given(method("POST")).and(path("/v2/images"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "job-3", "polling_url": "https://evil.example/poll/job-3"
            })))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/v2/images/job-3"))   // the documented fallback on api_base
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "ready", "result": { "url": format!("{base}/cdn/out.png") }
            })))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/cdn/out.png"))
            .respond_with(ResponseTemplate::new(200).insert_header("content-type", "image/png").set_body_bytes(b"\x89PNG".to_vec()))
            .mount(&server).await;
        make_provider(&server.uri())
            .generate(&ref_schema("acme/photo-2"), &make_base("acme/photo-2"), &make_extras(), &no_refs())
            .await
            .expect("falls back to the api_base poll path");
        assert!(elsewhere.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_download_url_on_a_private_address_is_refused() {
        let server = MockServer::start().await;
        mount_happy_path(&server).await; // its result.url points at 127.0.0.1
        let mut p = make_provider(&server.uri());
        p.public_downloads_only = true; // production behaviour
        let err = p
            .generate(&ref_schema("acme/photo-2"), &make_base("acme/photo-2"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("refused"), "{err}");
    }

    #[tokio::test]
    async fn a_4xx_on_submit_carries_the_vendor_message_and_is_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/v2/images"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({
                "error": { "code": "invalid_aspect_ratio", "message": "aspect_ratio 5:4 is not supported" }
            })))
            .mount(&server).await;
        let err = make_provider(&server.uri())
            .generate(&ref_schema("acme/photo-2"), &make_base("acme/photo-2"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(!err.is_retryable());
        assert!(err.to_string().contains("aspect_ratio 5:4"), "{err}");
    }
}
```

Each polling test waits one `POLL_INTERVAL`, which is 2 s. If you lower it
under `#[cfg(test)]` to speed up the suite, keep `GENERATE_BUDGET` in
production well under the router's 120 s.

## Catalog row (`models/acme.yaml`)

```yaml
models:
  # Acme (api.acme.example). Auth: Authorization: Bearer <ACME_API_KEY>. Images
  # are async (submit + poll). Video is planned; see video-provider path B.
  # ─── Image ────────────────────────────────────────────────────────────────
  # Acme Photo 2 on POST /v2/images (model `photo-2`).
  # @see https://docs.acme.example/openapi.json (2026-10-12) — CreateImageRequest
  - id: acme/photo-2
    provider: acme
    media_type: image
    display_name: Acme Photo 2
    description: Acme Photo 2 text-to-image and image-to-image with style references.
    docs_url: https://docs.acme.example/models/photo-2
    # "$0.04 per image" at any aspect ratio; no size tiers.
    # @see https://acme.example/pricing.md (2026-10-12)
    pricing: { base_cost_usd: 0.04 }
    capabilities: { text_to_image: true, image_to_image: true }
    # CreateImageRequest.prompt: "maxLength: 2000"
    prompt: { required: true, max_length: 2000 }
    params:
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1", "16:9", "9:16", "4:3", "3:4"]
        default: "1:1"
      # CreateImageRequest.seed: "minimum: 0, maximum: 2147483647"
      seed: { kind: seed, min: 0, max: 2147483647 }
      negative_prompt: { kind: string, max_length: 1000 }
    ref_inputs:
      max_total: 4
      default_role: init
      provider_format: { form: base64 }
      roles:
        init:  { required: false, min_count: 0, max_count: 1 }
        style: { required: false, min_count: 0, max_count: 3 }
    # CreateImageRequest also takes `safety_tolerance` (0–6). It has no
    # canonical param, so it is a passthrough.
    extra_allowlist: [safety_tolerance]
    tags: [text-to-image, image-to-image]
```

## Live test (`litegen-core/tests/live_providers.rs`)

```rust
#[tokio::test]
#[ignore = "live: set ACME_API_KEY"]
async fn live_acme_image() {
    skip_if_missing!("ACME_API_KEY");
    let mut p = litegen::providers::image::acme::AcmeImageProvider::new();
    p.configure(api_key_cfg(&env("ACME_API_KEY").unwrap()));
    let out = p
        .generate(&schema("acme/photo-2"), &base("a lighthouse at dusk", "acme/photo-2"), &img_extras(), &empty_mat())
        .await
        .expect("acme image");
    // Prove the bytes are what content_type claims, not just non-empty.
    assert!(out.data.starts_with(b"\x89PNG"), "expected PNG, got {} bytes as {}", out.data.len(), out.content_type);
}
```

Also add `ACME_API_KEY=` to `litegen-core/.env.example`, and map the env var
in `load_provider_env_overrides`.
