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

/// Black Forest Labs (FLUX) direct image provider.
///
/// Asynchronous: `POST /v1/{model}` returns `{id, polling_url}`; poll the
/// returned `polling_url` until `status == "Ready"`, then download
/// `result.sample` (a signed URL valid ~10 min). Authenticated with the `x-key`
/// header. Image-only (BFL has no first-party video API).
///
/// @see <https://docs.bfl.ml/flux_models/flux_1_1_pro> — generate + poll
///   Verbatim: "curl -X POST 'https://api.bfl.ai/v1/flux-pro-1.1' -H \"x-key: ${BFL_API_KEY}\" -H 'Content-Type: application/json' -d '{ \"prompt\": ..., \"width\": 1024, \"height\": 1024 }'"
///   Verbatim: "echo \"Image ready: $(echo $result | jq -r .result.sample)\""
/// @see <https://docs.bfl.ml/kontext/kontext_image_editing> — input_image (base64) editing
/// What litegen itself sends; `extra` cannot replace it.
const OWNED_FIELDS: &[&str] =
    &["prompt", "width", "height", "aspect_ratio", "seed", "output_format", "input_image", "image_prompt"];

pub struct BflProvider {
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

impl BflProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::raw_header("x-key"),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("failed to build HTTP client"),
            asset_client: crate::util::ssrf::no_redirect_download_client(
                std::time::Duration::from_secs(120),
                std::time::Duration::from_secs(30),
            ),
            public_assets_only: true,
        }
    }

    /// Whether `url` may be polled with the key: on the configured base's
    /// origin, or an https host under `bfl.ai` ("polling_url points at the
    /// region that holds your task, so don't build the URL yourself").
    /// @see <https://docs.bfl.ml/flux_3/flux3_image_generate.md> (2026-10-30)
    fn trusted_polling_url(api_base: &str, url: &str) -> bool {
        crate::providers::model3d::fal::same_origin_url(api_base, Some(url)).is_some()
            || reqwest::Url::parse(url).is_ok_and(|u| {
                u.scheme() == "https" && u.host_str().is_some_and(|h| h == "bfl.ai" || h.ends_with(".bfl.ai"))
            })
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.bfl.ai")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("bfl".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// The native model id is the endpoint path segment (e.g. `flux-pro-1.1`).
    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("bfl/").unwrap_or(model_id)
    }

    async fn poll_result(&self, polling_url: &str) -> Result<Value, ProviderError> {
        let creds = self.creds()?;
        let max_attempts = 120; // 120 * 2s = 4 min
        for _ in 0..max_attempts {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.get(polling_url))?;
            let resp = match crate::providers::inject_trace_headers(builder).send().await {
                Ok(resp) => resp,
                // The job is BFL's now; a dropped poll is asked again.
                Err(_) => continue,
            };
            // A 5xx or 429 is asked again; any other error status is the answer.
            let data = match crate::providers::video::read_poll_json(resp, "bfl").await {
                Ok(data) => data,
                Err(ProviderError::RequestFailed { retryable: true, .. }) => continue,
                Err(e) => return Err(e),
            };
            match data["status"].as_str() {
                Some("Ready") => return Ok(data),
                // "`Pending`, `Reasoning`, `Generating` | In progress | Keep polling".
                // @see <https://docs.bfl.ml/flux_3/flux3_image_generate.md> (2026-10-30)
                Some("Pending") | Some("Reasoning") | Some("Generating") => continue,
                // Error, Content/Request Moderated, Task not found, No Card
                // Generated — or a status outside the documented enum.
                other => {
                    return Err(ProviderError::RequestFailed {
                        message: format!("BFL generation failed: {}", other.unwrap_or("no status")),
                        status_code: None,
                        provider_error: Some(data),
                        retryable: false,
                    });
                }
            }
        }
        Err(ProviderError::Timeout { timeout_ms: max_attempts * 2000 })
    }

    /// `result.sample` comes from "region-specific delivery URLs under
    /// `delivery.*.bfl.ai`" and is fetched without the key.
    /// @see <https://docs.bfl.ml/api_integration/integration_guidelines.md> (2026-10-30)
    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        super::fetch_result(&self.asset_client, url, self.public_assets_only, "bfl image").await
    }
}

impl Default for BflProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for BflProvider {
    fn name(&self) -> &str {
        "bfl"
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
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);
        let url = format!("{}/v1/{native}", self.api_base());

        let mut body = json!({ "prompt": base.prompt });
        // Width/height from size; else aspect_ratio for ultra/kontext models.
        if let Some(size) = extras.size.as_deref() {
            if let Some((w, h)) = size.split_once('x') {
                if let (Ok(w), Ok(h)) = (w.parse::<u64>(), h.parse::<u64>()) {
                    body["width"] = Value::Number(w.into());
                    body["height"] = Value::Number(h.into());
                }
            }
        } else if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }
        // The asked container (PixelGenie L9); png when none is asked.
        body["output_format"] = Value::String(extras.output_format.clone().unwrap_or_else(|| "png".to_string()));

        // Reference images: input_image (Kontext edit base) or image_prompt (Redux).
        for r in &materialized.refs {
            if let MaterializedRefForm::Base64(b64) = &r.form {
                match r.role.as_str() {
                    "redux" | "image_prompt" => body["image_prompt"] = Value::String(b64.clone()),
                    _ => body["input_image"] = Value::String(b64.clone()),
                }
            } else if let MaterializedRefForm::Url(u) = &r.form {
                body["input_image"] = Value::String(u.clone());
            }
        }

        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(obj) = body.as_object_mut() {
                for (k, v) in extra_map.iter().filter(|(k, _)| !OWNED_FIELDS.contains(&k.as_str())) {
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
                message: format!("BFL request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // Only a connection that never opened is safe to retry: a
                // request that timed out may have been accepted and billed.
                retryable: e.is_connect(),
            })?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let submit: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let said = if submit.is_null() { text.chars().take(200).collect() } else { submit.to_string() };
            return Err(ProviderError::RequestFailed {
                message: format!("BFL API error: {said}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(submit),
                // Turned away, not run.
                retryable: status.as_u16() == 429 || status.is_server_error(),
            });
        }
        let submit: Value = serde_json::from_str(&text).map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse BFL response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        let id = submit["id"].as_str().unwrap_or_default().to_string();
        // Accepted: from here on a failure must not re-send the job (L1).
        extras.vendor_job.record(id.clone());
        // Prefer the returned polling_url; fall back to the documented path.
        let polling_url = submit["polling_url"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("{}/v1/get_result?id={id}", self.api_base()));
        // The poll carries the key, so it goes only to BFL.
        if !Self::trusted_polling_url(self.api_base(), &polling_url) {
            return Err(ProviderError::RequestFailed {
                message: format!("BFL named a polling URL off its own hosts: {polling_url}"),
                status_code: None,
                provider_error: Some(submit),
                retryable: false,
            });
        }

        let result = self.poll_result(&polling_url).await?;
        let sample = result["result"]["sample"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "BFL result missing result.sample".to_string(),
            status_code: None,
            provider_error: Some(result.clone()),
            retryable: false,
        })?;

        let bytes = self.fetch_image_bytes(sample).await?;
        let mut metadata = HashMap::new();
        metadata.insert("id".to_string(), Value::String(id));
        metadata.insert("url".to_string(), Value::String(sample.to_string()));

        Ok(GenerationOutput {
            data: bytes,
            content_type: "image/png".to_string(),
            metadata,
            more: Vec::new(),
        })
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
                message: "BFL provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult {
            healthy: true,
            message: "BFL provider configured".into(),
            latency_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> BflProvider {
        let mut p = BflProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_key: "bfl-key".to_string(),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        cfg.credentials.api_key = Some("bfl-key".to_string());
        p.configure(cfg);
        p.public_assets_only = false;
        p
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(),
            model: model.to_string(),
            n: 1,
            negative_prompt: None,
            seed: Some(7),
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        }
    }

    fn make_extras() -> ImageExtras {
        ImageExtras {
            vendor_job: Default::default(),
            size: Some("1024x1024".to_string()),
            aspect_ratio: None,
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
    async fn generates_flux_pro_via_polling_url() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/sample.png", image_server.uri());
        let polling_url = format!("{}/v1/get_result", server.uri());

        Mock::given(method("POST"))
            .and(path("/v1/flux-pro-1.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "bfl-1", "polling_url": format!("{}?id=bfl-1", polling_url)
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/get_result"))
            .and(query_param("id", "bfl-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "Ready", "result": { "sample": image_url.clone() }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"FLUXPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("bfl/flux-pro-1.1");
        let base = make_base("a cyberpunk fox", "bfl/flux-pro-1.1");
        let mut extras = make_extras();
        extras.output_format = Some("webp".into());
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        // The vendor accepted the job before the poll: the router must never re-send it.
        assert_eq!(extras.vendor_job.get().as_deref(), Some("bfl-1"));
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"FLUXPNG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("x-key").unwrap(), "bfl-key");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["prompt"], "a cyberpunk fox");
        assert_eq!(body["width"], 1024);
        assert_eq!(body["output_format"], "webp", "PixelGenie L9: the asked container");
        assert_eq!(body["height"], 1024);
        assert_eq!(body["seed"], 7);
    }

    // ─── BFL audit, 2026-10-30 ──────────────────────────────────────────────

    fn no_refs() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    /// F2: `extra` adds BFL's own knobs but never replaces what litegen sends.
    #[tokio::test]
    async fn extra_cannot_replace_what_litegen_sends() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({ "detail": "stop" })))
            .mount(&server)
            .await;
        let mut extras = make_extras();
        extras.extra = Some(json!({
            "prompt": "other", "width": 64, "seed": 9, "output_format": "jpeg", "input_image": "x", "safety_tolerance": 2
        }));
        let _ = make_provider(&server.uri()).generate(&ref_schema("bfl/flux-pro-1.1"), &make_base("a fern", "bfl/flux-pro-1.1"), &extras, &no_refs()).await;
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body["prompt"], "a fern");
        assert_eq!(body["width"], 1024);
        assert_eq!(body["seed"], 7);
        assert_eq!(body["output_format"], "png");
        assert!(body.get("input_image").is_none(), "{body}");
        assert_eq!(body["safety_tolerance"], 2);
    }

    /// F3: "`Pending`, `Reasoning`, `Generating` | In progress | Keep polling";
    /// every other status but `Ready` (`No Card Generated` among them) is an
    /// ending, not a job to poll until the cap.
    /// @see <https://docs.bfl.ml/flux_3/flux3_image_generate.md> (2026-10-30)
    #[tokio::test]
    async fn only_pending_reasoning_or_generating_is_still_running() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "j1", "polling_url": format!("{}/v1/get_result?id=j1", server.uri())
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/get_result"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "j1", "status": "No Card Generated" })))
            .mount(&server)
            .await;
        let started = std::time::Instant::now();
        let err = make_provider(&server.uri())
            .generate(&ref_schema("bfl/flux-pro-1.1"), &make_base("x", "bfl/flux-pro-1.1"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ProviderError::RequestFailed { retryable: false, message, .. } if message.contains("No Card Generated")),
            "{err:?}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "polled past an ending");
    }

    /// F13: the key goes only to BFL: a `polling_url` on another origin is
    /// not polled with it.
    #[tokio::test]
    async fn the_key_goes_only_to_bfl() {
        let elsewhere = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "Pending" })))
            .mount(&elsewhere)
            .await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "j1", "polling_url": format!("{}/v1/get_result?id=j1", elsewhere.uri())
            })))
            .mount(&server)
            .await;
        let err = make_provider(&server.uri())
            .generate(&ref_schema("bfl/flux-pro-1.1"), &make_base("x", "bfl/flux-pro-1.1"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
        assert!(elsewhere.received_requests().await.unwrap().is_empty(), "the key went to another host");
        assert!(BflProvider::trusted_polling_url("https://api.bfl.ai", "https://api.eu1.bfl.ai/v1/get_result?id=a"));
        assert!(!BflProvider::trusted_polling_url("https://api.bfl.ai", "https://bfl.ai.example.com/v1/get_result"));
        assert!(!BflProvider::trusted_polling_url("https://api.bfl.ai", "http://api.eu1.bfl.ai/v1/get_result"));
    }

    /// F13: a submit that timed out may have been accepted and billed, so it
    /// is never sent again; a 429 was turned away and may be.
    #[tokio::test]
    async fn a_timed_out_submit_is_never_sent_again_and_a_rate_limited_one_may_be() {
        let slow = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "j" })).set_delay(std::time::Duration::from_secs(5)))
            .mount(&slow)
            .await;
        let mut p = make_provider(&slow.uri());
        p.client = reqwest::Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        let schema = ref_schema("bfl/flux-pro-1.1");
        let base = make_base("x", "bfl/flux-pro-1.1");
        let err = p.generate(&schema, &base, &make_extras(), &no_refs()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");

        let limited = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({ "detail": "Too many active tasks" })))
            .mount(&limited)
            .await;
        let err = make_provider(&limited.uri()).generate(&schema, &base, &make_extras(), &no_refs()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: true, status_code: Some(429), .. }), "{err:?}");
    }

    /// F13: the result is fetched without the key, only from a public address,
    /// and at most 64 MiB.
    #[tokio::test]
    async fn a_result_is_fetched_only_from_a_public_address() {
        let server = MockServer::start().await;
        let cdn = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "j1", "polling_url": format!("{}/v1/get_result?id=j1", server.uri())
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/get_result"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "Ready", "result": { "sample": format!("{}/sample.png", cdn.uri()) }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"\x89PNG\r\n\x1a\n".to_vec())).mount(&cdn).await;
        let mut provider = make_provider(&server.uri());
        provider.public_assets_only = true;
        let err = provider
            .generate(&ref_schema("bfl/flux-pro-1.1"), &make_base("x", "bfl/flux-pro-1.1"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
        assert!(cdn.received_requests().await.unwrap().is_empty(), "a private address was fetched");
    }
}
