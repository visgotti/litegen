use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{sigv4, AuthSpec, ProviderCredentials};
use crate::providers::{
    BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras, ImageProvider,
    ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// Amazon Bedrock Nova Canvas image provider.
///
/// Synchronous: `POST /model/amazon.nova-canvas-v1:0/invoke` with a JSON body
/// (`taskType` + `imageGenerationConfig`); the response JSON contains base64
/// image(s) at `images[]`. Authenticated with AWS Signature V4 (service
/// `bedrock`) over the exact request body, using access key id (key_id),
/// secret access key (key_secret), and region.
///
/// @see <https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_InvokeModel.html>
///   Verbatim: "POST /model/{modelId}/invoke HTTP/1.1"
/// @see <https://docs.aws.amazon.com/nova/latest/userguide/image-gen-access.html>
///   Verbatim: "Amazon Nova Canvas is available through the Bedrock InvokeModel API ..."
pub struct BedrockImageProvider {
    config: Option<ProviderInstanceConfig>,
    auth: AuthSpec,
    client: Client,
}

impl BedrockImageProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            auth: AuthSpec::AwsSigV4 { service: "bedrock".into(), default_region: "us-east-1".into() },
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        self.config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("bedrock".into()))
    }

    fn region(&self, creds: &ProviderCredentials) -> String {
        creds
            .region
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| match &self.auth {
                AuthSpec::AwsSigV4 { default_region, .. } => default_region.clone(),
                _ => "us-east-1".to_string(),
            })
    }

    /// Endpoint host. api_base overrides; otherwise the regional Bedrock host.
    fn endpoint(&self, region: &str, model: &str) -> String {
        if let Some(base) = self.config.as_ref().and_then(|c| c.api_base.as_deref()) {
            format!("{base}/model/{model}/invoke")
        } else {
            format!("https://bedrock-runtime.{region}.amazonaws.com/model/{model}/invoke")
        }
    }

    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("bedrock/").unwrap_or(model_id)
    }
}

impl Default for BedrockImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for BedrockImageProvider {
    fn name(&self) -> &str {
        "bedrock"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config.as_ref().is_some_and(|c| self.auth.is_satisfied_by(&c.credentials))
    }

    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let region = self.region(&creds);
        let native = Self::resolve_model(&model.id);
        let endpoint = self.endpoint(&region, native);

        let (mut w, mut h) = (1024u64, 1024u64);
        if let Some(size) = extras.size.as_deref() {
            if let Some((sw, sh)) = size.split_once('x') {
                if let (Ok(a), Ok(b)) = (sw.parse(), sh.parse()) {
                    w = a;
                    h = b;
                }
            }
        }

        let mut text_params = json!({ "text": base.prompt });
        if let Some(np) = base.negative_prompt.as_deref() {
            text_params["negativeText"] = Value::String(np.to_string());
        }
        // Optional conditioning image (base64), controlMode via extra.
        for r in &materialized.refs {
            if let MaterializedRefForm::Base64(b64) = &r.form {
                text_params["conditionImage"] = Value::String(b64.clone());
                let mode = extras
                    .extra
                    .as_ref()
                    .and_then(|e| e.get("controlMode"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("CANNY_EDGE");
                text_params["controlMode"] = Value::String(mode.to_string());
                break;
            }
        }

        let mut image_config = json!({
            "numberOfImages": base.n.max(1),
            "width": w,
            "height": h,
        });
        if let Some(seed) = base.seed {
            image_config["seed"] = Value::Number(seed.max(0).into());
        }
        if let Some(cfg) = extras.guidance_scale {
            image_config["cfgScale"] = json!(cfg);
        }
        if let Some(q) = extras.quality.as_deref() {
            image_config["quality"] = Value::String(q.to_string());
        }

        let body = json!({
            "taskType": "TEXT_IMAGE",
            "textToImageParams": text_params,
            "imageGenerationConfig": image_config,
        });
        let body_bytes = serde_json::to_vec(&body).map_err(|e| ProviderError::InvalidRequest(e.to_string()))?;

        let url = reqwest::Url::parse(&endpoint).map_err(|e| ProviderError::InvalidRequest(format!("bad bedrock url: {e}")))?;
        let signed = sigv4::sign(
            &creds,
            "bedrock",
            &region,
            "POST",
            &url,
            &[("content-type".to_string(), "application/json".to_string())],
            &body_bytes,
        )?;

        let mut req = self.client.post(url).header("content-type", "application/json");
        for (k, v) in &signed {
            req = req.header(k, v);
        }
        let resp = crate::providers::inject_trace_headers(req.body(body_bytes))
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Bedrock request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?;

        let status = resp.status();
        let data: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Bedrock response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        if !status.is_success() {
            let msg = data["message"].as_str().or_else(|| data["Message"].as_str()).unwrap_or("Unknown error");
            return Err(ProviderError::RequestFailed {
                message: format!("Bedrock API error: {msg}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                retryable: status.as_u16() >= 500,
            });
        }

        let b64 = data["images"][0].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Bedrock response missing images[0]".to_string(),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        })?;
        let bytes = B64.decode(b64).map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to decode Bedrock image: {e}"),
            status_code: None,
            provider_error: None,
            retryable: false,
        })?;

        let mut metadata = HashMap::new();
        metadata.insert("model".to_string(), Value::String(native.to_string()));

        Ok(GenerationOutput { data: bytes, content_type: "image/png".to_string(), metadata })
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
                message: "Bedrock provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult { healthy: true, message: "Bedrock provider configured".into(), latency_ms: None }
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

    fn make_provider(api_base: &str) -> BedrockImageProvider {
        let mut p = BedrockImageProvider::new();
        let mut cfg = ProviderInstanceConfig { api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.key_id = Some("AKIDEXAMPLE".to_string());
        cfg.credentials.key_secret = Some("secretkey".to_string());
        cfg.credentials.region = Some("us-east-1".to_string());
        p.configure(cfg);
        p
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(), model: model.to_string(), n: 1, negative_prompt: Some("blurry".into()),
            seed: Some(7), reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    fn make_extras() -> ImageExtras {
        ImageExtras {
            size: Some("1024x1024".to_string()), aspect_ratio: None, quality: Some("standard".to_string()), style: None,
            steps: None, guidance_scale: Some(6.5), strength: None, response_format: "url".to_string(), extra: None,
        }
    }

    #[tokio::test]
    async fn generates_nova_canvas_with_sigv4() {
        let server = MockServer::start().await;
        let png = b"NOVAPNG";
        let b64 = B64.encode(png);
        Mock::given(method("POST"))
            .and(path("/model/amazon.nova-canvas-v1:0/invoke"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "images": [b64] })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("bedrock/amazon.nova-canvas-v1:0");
        let base = make_base("a desert at golden hour", "bedrock/amazon.nova-canvas-v1:0");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, png);

        let received = server.received_requests().await.unwrap();
        let post = &received[0];
        // SigV4 signature present.
        let auth = post.headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"), "auth: {auth}");
        assert!(post.headers.get("x-amz-date").is_some());
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["taskType"], "TEXT_IMAGE");
        assert_eq!(body["textToImageParams"]["text"], "a desert at golden hour");
        assert_eq!(body["textToImageParams"]["negativeText"], "blurry");
        assert_eq!(body["imageGenerationConfig"]["width"], 1024);
        assert_eq!(body["imageGenerationConfig"]["cfgScale"], 6.5);
    }
}
