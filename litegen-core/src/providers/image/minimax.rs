use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
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

/// MiniMax image generation provider (image-01).
///
/// Synchronous: `POST /v1/image_generation` returns `{data: {image_base64: [..]},
/// base_resp: {status_code, status_msg}}`. We request `response_format: "base64"`
/// to get bytes inline. Bearer auth. Region split via api_base (api.minimax.io
/// international vs api.minimax.cn China: the platform.minimax.cn specs say
/// `servers: - url: https://api.minimax.cn`, 2026-11-09).
///
/// @see <https://platform.minimax.io/docs/guides/image-generation>
///   Verbatim: "headers = {\"Authorization\": f\"Bearer {api_key}\"} ... payload = {\"model\": \"image-01\", \"prompt\": ..., \"aspect_ratio\": \"16:9\", \"response_format\": \"base64\"}; endpoint https://api.minimax.io/v1/image_generation"
pub struct MiniMaxImageProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl MiniMaxImageProvider {
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
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.minimax.io/v1")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("minimax".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("minimax/").unwrap_or(model_id)
    }
}

impl Default for MiniMaxImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for MiniMaxImageProvider {
    fn name(&self) -> &str {
        "minimax"
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
        let native = crate::providers::wire_model(self.config.as_ref(), &model.id, Self::resolve_model(&model.id));
        let url = format!("{}/image_generation", self.api_base());

        let mut body = json!({
            "model": native,
            "prompt": base.prompt,
            // No `n`: MiniMax makes (and bills) up to 9 images, and litegen
            // returns and bills one; the default is 1.
            "response_format": "base64",
        });
        // "width … height … Range [512, 2048], must be divisible by 8. If both
        // `width/height` and `aspect_ratio` are provided, `aspect_ratio` takes
        // priority": an exact size goes alone, so the ratio cannot override it.
        // @see https://platform.minimax.io/docs/api-reference/image-generation-t2i.md (2026-11-03)
        let size = extras.size.as_deref().and_then(|s| s.split_once('x')).and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)));
        if let Some((w, h)) = size {
            body["width"] = Value::Number(w.into());
            body["height"] = Value::Number(h.into());
        } else if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
        }
        // "seed … Using the same seed and parameters produces reproducible images."
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }
        // subject_reference: [{type: "character", image_file: <url or data-uri>}]
        let mut subject_ref: Vec<Value> = Vec::new();
        for r in &materialized.refs {
            let image_file = match &r.form {
                MaterializedRefForm::Url(u) => Some(u.clone()),
                MaterializedRefForm::Base64(b64) => Some(crate::proxy::materializer::base64_data_uri(b64, Some("image/png"))),
                _ => None,
            };
            if let Some(f) = image_file {
                subject_ref.push(json!({ "type": "character", "image_file": f }));
            }
        }
        if !subject_ref.is_empty() {
            body["subject_reference"] = Value::Array(subject_ref);
        }
        // `extra` adds the endpoint's own fields; it cannot replace one litegen
        // sets (rule 11).
        if let Some(obj) = body.as_object_mut() {
            let owned = |k: &str| matches!(k, "model" | "prompt" | "n" | "response_format" | "aspect_ratio" | "subject_reference" | "width" | "height" | "seed");
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
                message: format!("MiniMax request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // A call that timed out may still run, and bill: only a
                // connection that never opened is sent again.
                retryable: e.is_connect(),
            })?;

        let status = resp.status();
        let data = crate::providers::read_vendor_json(resp, "MiniMax").await?;

        // MiniMax returns HTTP 200 with base_resp.status_code != 0 on logical errors.
        let code = data["base_resp"]["status_code"].as_i64().unwrap_or(0);
        if code != 0 {
            let msg = data["base_resp"]["status_msg"].as_str().unwrap_or("Unknown error").to_string();
            return Err(ProviderError::RequestFailed {
                message: format!("MiniMax API error ({code}): {msg}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                retryable: false,
            });
        }

        let b64 = data["data"]["image_base64"][0]
            .as_str()
            .or_else(|| data["data"]["image_base64"].as_str())
            .ok_or_else(|| ProviderError::RequestFailed {
                message: "MiniMax response missing data.image_base64".to_string(),
                status_code: None,
                provider_error: Some(data.clone()),
                retryable: false,
            })?;
        let bytes = B64.decode(b64).map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to decode MiniMax image: {e}"),
            status_code: None,
            provider_error: None,
            retryable: false,
        })?;

        let mut metadata = HashMap::new();
        metadata.insert("model".to_string(), Value::String(native.to_string()));

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
                message: "MiniMax provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "MiniMax provider configured".into(), latency_ms: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> MiniMaxImageProvider {
        let mut p = MiniMaxImageProvider::new();
        let mut cfg = ProviderInstanceConfig { api_key: "mm-key".to_string(), api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.api_key = Some("mm-key".to_string());
        p.configure(cfg);
        p
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
    async fn generates_image_01() {
        let server = MockServer::start().await;
        let png = b"MINIMAXPNG";
        let b64 = B64.encode(png);
        Mock::given(method("POST"))
            .and(path("/image_generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "image_base64": [b64] },
                "base_resp": { "status_code": 0, "status_msg": "success" }
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("minimax/image-01");
        let base = make_base("a koi pond at dusk", "minimax/image-01");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, png);

        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].headers.get("authorization").unwrap(), "Bearer mm-key");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "image-01");
        assert_eq!(body["response_format"], "base64");
        assert_eq!(body["aspect_ratio"], "16:9");
    }

    // ─── ByteDance/MiniMax/Vidu audit, 2026-10-30: W6 ───────────────────────

    /// W6: `n` is never sent: MiniMax generates (and bills) up to 9 images,
    /// while litegen returns and bills one.
    #[tokio::test]
    async fn one_image_is_asked_for() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/image_generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": { "image_base64": [B64.encode(b"IMG")] }, "base_resp": { "status_code": 0 } })))
            .mount(&server)
            .await;
        let mut base = make_base("a fox", "minimax/image-01");
        base.n = 4;
        make_provider(&server.uri()).generate(&ref_schema("minimax/image-01"), &base, &make_extras(), &crate::proxy::materializer::MaterializedRequest::default()).await.unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert!(body.get("n").is_none(), "{body}");
    }

    /// "width … height … Range [512, 2048], must be divisible by 8. If both
    /// `width/height` and `aspect_ratio` are provided, `aspect_ratio` takes
    /// priority": an exact size goes as width/height without the ratio that
    /// would override it, and `seed` goes as MiniMax's own; `extra` cannot
    /// replace any of them.
    /// @see <https://platform.minimax.io/docs/api-reference/image-generation-t2i.md> (2026-11-03)
    #[tokio::test]
    async fn a_size_goes_as_width_and_height_and_a_seed_goes_too() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/image_generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": { "image_base64": [B64.encode(b"IMG")] }, "base_resp": { "status_code": 0 } })))
            .mount(&server)
            .await;
        let mut base = make_base("a fox", "minimax/image-01");
        base.seed = Some(42);
        let mut extras = make_extras();
        extras.size = Some("1024x768".into());
        extras.extra = Some(json!({ "width": 512, "height": 512, "seed": 1 }));
        make_provider(&server.uri()).generate(&ref_schema("minimax/image-01"), &base, &extras, &crate::proxy::materializer::MaterializedRequest::default()).await.unwrap();
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!((body["width"].as_u64(), body["height"].as_u64(), body["seed"].as_i64()), (Some(1024), Some(768), Some(42)), "{body}");
        assert!(body.get("aspect_ratio").is_none(), "the ratio would take priority over the size: {body}");
    }

    /// W14: a submit that timed out may still run, and bill, at the vendor:
    /// it is never sent again.
    #[tokio::test]
    async fn a_timed_out_submit_is_never_sent_again() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})).set_delay(std::time::Duration::from_secs(5)))
            .mount(&server)
            .await;
        let mut p = make_provider(&server.uri());
        p.client = reqwest::Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        let err = p.generate(&ref_schema("minimax/image-01"), &make_base("x", "minimax/image-01"), &make_extras(), &crate::proxy::materializer::MaterializedRequest::default()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
    }

    /// W14: a gateway's 5xx page is not JSON and a 429 turned the call away;
    /// both are judged by their status before any parse and may be sent again.
    #[tokio::test]
    async fn a_gateway_page_or_a_rate_limit_may_be_sent_again() {
        for (status, body) in [(502, "<html>bad gateway</html>"), (429, "too many requests")] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).respond_with(ResponseTemplate::new(status).set_body_string(body)).mount(&server).await;
            let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };
            let err = make_provider(&server.uri())
                .generate(&ref_schema("minimax/image-01"), &make_base("x", "minimax/image-01"), &make_extras(), &materialized)
                .await
                .unwrap_err();
            assert!(err.is_retryable(), "{status}: {err:?}");
        }
    }
}
