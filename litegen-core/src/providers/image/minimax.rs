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
/// international vs api.minimaxi.com China).
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
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
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
        let url = format!("{}/image_generation", self.api_base());

        let mut body = json!({
            "model": native,
            "prompt": base.prompt,
            // No `n`: MiniMax makes (and bills) up to 9 images, and litegen
            // returns and bills one; the default is 1.
            "response_format": "base64",
        });
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            body["aspect_ratio"] = Value::String(ar.to_string());
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
        if let (Some(Value::Object(map)), Some(obj)) = (&extras.extra, body.as_object_mut()) {
            for (k, v) in map.iter().filter(|(k, _)| !matches!(k.as_str(), "model" | "prompt" | "n" | "response_format" | "aspect_ratio" | "subject_reference")) {
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
        let data: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse MiniMax response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        // MiniMax returns HTTP 200 with base_resp.status_code != 0 on logical errors.
        let code = data["base_resp"]["status_code"].as_i64().unwrap_or(0);
        if !status.is_success() || code != 0 {
            let msg = data["base_resp"]["status_msg"].as_str().unwrap_or("Unknown error").to_string();
            return Err(ProviderError::RequestFailed {
                message: format!("MiniMax API error ({code}): {msg}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                retryable: status.as_u16() >= 500,
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
}
