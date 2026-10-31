use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    BaseGenerationRequest, CredentialPool, GenerationOutput, HealthCheckResult, ImageExtras, ImageProvider,
    ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// Kling (Kuaishou) image generation provider.
///
/// Async: `POST /v1/images/generations` returns `data.task_id`; poll
/// `GET /v1/images/generations/{task_id}` until `data.task_status == "succeed"`,
/// then download `data.task_result.images[0].url`. Auth: a per-request HS256
/// JWT minted from the account access key (key_id) + secret key (key_secret),
/// sent as `Authorization: Bearer <jwt>` (the [`AuthSpec::KlingJwt`] scheme).
///
/// @see <https://kling.ai/document-api/api/get-started/authentication.md> (2026-10-30)
///   (auth: HS256 JWT `"iss": ak`, `"exp": int(time.time()) + 1800`,
///   `"nbf": int(time.time()) - 5`)
/// @see <https://kling.ai/document-api/api/image/2-1/image-generation.md> (2026-10-30)
///   (`/v1/images/generations`; response envelope {code, message, data:{task_id,
///   task_status, task_result}})
pub struct KlingImageProvider {
    config: Option<ProviderInstanceConfig>,
    cred_pool: Option<CredentialPool>,
    auth: AuthSpec,
    client: Client,
}

impl KlingImageProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            cred_pool: None,
            auth: AuthSpec::KlingJwt,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("failed to build HTTP client"),
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
        if let Some(pool) = &self.cred_pool {
            return Ok(base.with_signing(pool.next()));
        }
        Ok(base)
    }

    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("kling/").unwrap_or(model_id)
    }

    async fn poll_task(&self, task_id: &str) -> Result<Value, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/v1/images/generations/{}", self.api_base(), task_id);
        let max_attempts = 90;
        for _ in 0..max_attempts {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(&url))?;
            let data: Value = crate::providers::inject_trace_headers(builder)
                .send()
                .await
                .map_err(|e| ProviderError::RequestFailed {
                    message: e.to_string(),
                    status_code: None,
                    provider_error: None,
                    retryable: true,
                })?
                .json()
                .await
                .unwrap_or_default();
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
                _ => continue,
            }
        }
        Err(ProviderError::Timeout { timeout_ms: max_attempts * 2000 })
    }

    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let resp = self.client.get(url).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to fetch Kling image: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: true,
        })?;
        if !resp.status().is_success() {
            // A non-2xx (e.g. an expired/throttled presigned CDN URL) delivers a
            // short error body, not image bytes. Returning it would store an
            // error blob as the generated image and bill for a broken asset.
            return Err(ProviderError::RequestFailed {
                message: format!("Kling image URL returned HTTP {}", resp.status()),
                status_code: Some(resp.status().as_u16()),
                provider_error: None,
                retryable: false,
            });
        }
        Ok(resp
            .bytes()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to read Kling image: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
            .to_vec())
    }
}

impl Default for KlingImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for KlingImageProvider {
    fn name(&self) -> &str {
        "kling"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.credentials.credential_sets.is_empty() {
            self.cred_pool = Some(CredentialPool::shared(config.credentials.credential_sets.clone()));
        }
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.cred_pool.is_some())
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
            "model_name": native,
            "prompt": base.prompt,
            "n": 1,
        });
        if let Some(np) = base.negative_prompt.as_deref() {
            body["negative_prompt"] = Value::String(np.to_string());
        }
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }
        // Reference image as base64 (no data: prefix) or URL. A `subject` is
        // Kling's subject reference ("`image_reference` … `subject`, `face`");
        // without it the image is plain image-to-image.
        for r in &materialized.refs {
            match &r.form {
                MaterializedRefForm::Base64(b64) => body["image"] = Value::String(b64.clone()),
                MaterializedRefForm::Url(u) => body["image"] = Value::String(u.clone()),
                _ => continue,
            }
            if r.role == "subject" {
                body["image_reference"] = Value::String("subject".into());
            }
        }
        // `extra` adds the endpoint's own fields; it cannot replace one litegen
        // sets (rule 11).
        if let (Some(Value::Object(map)), Some(obj)) = (&extras.extra, body.as_object_mut()) {
            for (k, v) in map.iter().filter(|(k, _)| !matches!(k.as_str(), "model_name" | "prompt" | "n" | "negative_prompt" | "aspect_ratio" | "image" | "image_reference")) {
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

    fn make_provider(api_base: &str) -> KlingImageProvider {
        let mut p = KlingImageProvider::new();
        let mut cfg = ProviderInstanceConfig { api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.key_id = Some("ak-test".to_string());
        cfg.credentials.key_secret = Some("sk-test".to_string());
        p.configure(cfg);
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
            size: None, aspect_ratio: Some("16:9".to_string()), quality: None, style: None,
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
    async fn generates_with_jwt_auth_and_poll() {
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
        // JWT Bearer auth present.
        let auth = post.headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("Bearer ey"), "expected JWT bearer, got {auth}");
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
}
