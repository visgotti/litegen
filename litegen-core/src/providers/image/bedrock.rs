use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{sigv4, AuthSpec, ProviderCredentials};
use crate::providers::{
    BaseGenerationRequest, CredentialPool, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// Amazon Bedrock image provider: Stability AI's text-to-image models.
///
/// Synchronous: `POST /model/{modelId}/invoke` with Stability's own JSON body
/// (Nova Canvas's `taskType` body went with Nova's end of life on
/// 2026-09-30); the answer holds base64 image(s) at `images[]`. Authenticated
/// with AWS Signature V4 (service `bedrock`) over the exact request body,
/// using the access key id (key_id) and secret access key (key_secret). Every
/// model here runs In-Region in us-west-2 only, so the call goes to (and is
/// signed for) us-west-2 whatever region the credential names.
///
/// @see <https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_InvokeModel.html>
///   Verbatim: "POST /model/{modelId}/invoke HTTP/1.1"
/// @see <https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-stability-diffusion.md> (2026-11-04)
///   Verbatim: "Amazon Bedrock supports the following Stability AI models and image services. … Support for all other Stability AI models are in the process of being deprecated."
///   Verbatim: "finish_reasons – Enum indicating whether the request was filtered or not. `null` will indicate that the request was successful. Current possible values: \"Filter reason: prompt\", \"Filter reason: output image\", \"Filter reason: input image\", \"Inference error\", null."
/// @see <https://docs.aws.amazon.com/bedrock/latest/userguide/model-card-stability-ai-stable-diffusion-3-5-large.md> — "us-west-2 (Oregon)" In-Region; no Geo or Global id (2026-11-04)
pub struct BedrockImageProvider {
    config: Option<ProviderInstanceConfig>,
    cred_pool: Option<CredentialPool>,
    auth: AuthSpec,
    client: Client,
}

impl BedrockImageProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (result downloads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(180), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            cred_pool: None,
            auth: AuthSpec::AwsSigV4 { service: "bedrock".into(), default_region: "us-east-1".into() },
            client: Self::api_client(&Default::default()),
        }
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("bedrock".into()))?;
        if let Some(pool) = &self.cred_pool {
            return Ok(base.with_signing(pool.next()));
        }
        Ok(base)
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

/// The Stability models Bedrock hosts, by the request body each takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stability {
    /// SD3.5 Large: `mode` text-to-image | image-to-image; image-to-image
    /// "requires the prompt, image, and strength parameters".
    Sd35Large,
    /// Stable Image Core: text-to-image only.
    Core,
    /// Stable Image Ultra: text-to-image, or `image` + `strength`
    /// ("Default: 0.35").
    Ultra,
}

/// The Stability model a Bedrock model id names; `None` for any other id
/// (never a default model: a typo would bill as another model).
///
/// @see <https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-diffusion-3-5-large.md> — "modelId='stability.sd3-5-large-v1:0'" (2026-11-09)
/// @see <https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-diffusion-stable-image-core-text-image-request-response.md> — "modelId='stability.stable-image-core-v1:1'" (2026-11-09)
/// @see <https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-diffusion-stable-ultra-text-image-request-response.md> — "modelId='stability.stable-image-ultra-v1:1'" (2026-11-09)
/// @see <https://aws.amazon.com/bedrock/pricing/> — "Stability AI model | Price per generated image": $0.08 (Stable Diffusion
///   3.5 Large), $0.04 (Stable Image Core), $0.14 (Stable Image Ultra), read from AWS's metered unit map for us-west-2,
///   <https://b0.p.awsstatic.com/pricing/2.0/meteredUnitMaps/bedrockfoundationmodels/USD/current/bedrockfoundationmodels.json>
///   (2026-11-09; the request pages carry no price)
fn stability_model(native: &str) -> Option<Stability> {
    match native {
        "stability.sd3-5-large-v1:0" => Some(Stability::Sd35Large),
        "stability.stable-image-core-v1:1" => Some(Stability::Core),
        "stability.stable-image-ultra-v1:1" => Some(Stability::Ultra),
        _ => None,
    }
}

/// The one region every Stability model on Bedrock runs in ("us-west-2
/// (Oregon)": In-Region only, no Geo or Global inference id).
const STABILITY_REGION: &str = "us-west-2";

/// The SD3.5 image-to-image strength sent when none is asked: the field is
/// required there, and 0.5 is what the direct `stability` rows send.
const DEFAULT_STRENGTH: f64 = 0.5;

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
        if !config.credentials.credential_sets.is_empty() {
            self.cred_pool = Some(CredentialPool::shared(config.credentials.credential_sets.clone()));
        }
        self.client = Self::api_client(&config.extra_headers);
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
        let native = Self::resolve_model(&model.id);
        let kind = stability_model(native).ok_or_else(|| ProviderError::ModelNotSupported {
            model: model.id.clone(),
            provider: "bedrock".into(),
        })?;
        let creds = self.creds()?;
        // Not the credential's region: these models run in us-west-2 only (a
        // fixed host, so no tenant value reaches it).
        let region = STABILITY_REGION.to_string();
        let endpoint = self.endpoint(&region, crate::providers::wire_model(self.config.as_ref(), &model.id, native));

        // One image per call: Stability's body has no count field, so `n`
        // never goes out (rule 12).
        let mut body = json!({ "prompt": base.prompt });
        // "negative_prompt … Maximum 10,000 characters."
        if let Some(np) = base.negative_prompt.as_deref().filter(|n| !n.is_empty()) {
            body["negative_prompt"] = json!(np);
        }
        // "seed … Range 0 to 4294967294. Default 0 (random seed)." (Core/Ultra: to 4294967295)
        if let Some(seed) = base.seed {
            body["seed"] = json!(seed);
        }
        // "output_format … Enum: jpeg, png, webp. Default png." (Core/Ultra: JPEG, PNG)
        if let Some(format) = extras.output_format.as_deref() {
            body["output_format"] = json!(format);
        }
        let image = materialized.refs.iter().find_map(|r| match &r.form {
            MaterializedRefForm::Base64(b64) if r.role == "init" => Some(b64.clone()),
            _ => None,
        });
        match (kind, image) {
            (Stability::Core, Some(_)) => {
                return Err(ProviderError::InvalidRequest("bedrock: Stable Image Core does text-to-image only".into()));
            }
            // "image-to-image requires the prompt, image, and strength parameters".
            (Stability::Sd35Large, Some(image)) => {
                body["mode"] = json!("image-to-image");
                body["image"] = json!(image);
                body["strength"] = json!(extras.strength.unwrap_or(DEFAULT_STRENGTH));
            }
            // "strength … Default: 0.35": sent only when asked.
            (Stability::Ultra, Some(image)) => {
                body["image"] = json!(image);
                if let Some(strength) = extras.strength {
                    body["strength"] = json!(strength);
                }
            }
            // "aspect_ratio … Valid for text-to-image requests only."
            (_, None) => {
                if let Some(ratio) = extras.aspect_ratio.as_deref() {
                    body["aspect_ratio"] = json!(ratio);
                }
            }
        }
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
                retryable: crate::providers::may_resend_after(&e, true),
            })?;

        // The status first (a gateway's 5xx page is retried), and a 429's
        // `Retry-After` reaches the retry policy.
        let data = crate::providers::read_vendor_json(resp, "Bedrock").await?;

        // "`null` will indicate that the request was successful": anything else
        // is the vendor's refusal ("Filter reason: prompt", …), not sent again.
        if let Some(reason) = data["finish_reasons"][0].as_str() {
            return Err(ProviderError::RequestFailed {
                message: format!("Bedrock {native} refused the request: {reason}"),
                status_code: None,
                provider_error: Some(data),
                retryable: false,
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

        if let Some(seed) = data["seeds"][0].as_u64() {
            metadata.insert("seed".to_string(), json!(seed));
        }
        // The bytes say what they are; else the format asked for (png by default).
        let content_type = crate::util::content::image_content_type(&bytes).map(str::to_string).unwrap_or_else(|| {
            match extras.output_format.as_deref() {
                Some("jpeg") => "image/jpeg",
                Some("webp") => "image/webp",
                _ => "image/png",
            }
            .to_string()
        });
        Ok(GenerationOutput { data: bytes, content_type, metadata, more: Vec::new() })
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
        crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load").get(id).expect("model").clone()
    }

    /// A credential naming us-east-1, the adapter's default region: the
    /// Stability rows still go to (and sign for) us-west-2.
    fn make_provider(api_base: &str) -> BedrockImageProvider {
        let mut p = BedrockImageProvider::new();
        let mut cfg = ProviderInstanceConfig { api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.key_id = Some("AKIDEXAMPLE".to_string());
        cfg.credentials.key_secret = Some("secretkey".to_string());
        cfg.credentials.region = Some("us-east-1".to_string());
        p.configure(cfg);
        p
    }

    #[test]
    fn configure_with_credential_pool_cycles_and_is_configured() {
        let mut p = BedrockImageProvider::new();
        let cfg = ProviderInstanceConfig {
            credentials: ProviderCredentials {
                credential_sets: vec![
                    CredentialEntry { key_id: "AKIA1".into(), key_secret: "s1".into(), region: Some("us-east-1".into()), weight: 1, label: None },
                    CredentialEntry { key_id: "AKIA2".into(), key_secret: "s2".into(), region: Some("eu-west-1".into()), weight: 1, label: None },
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        p.configure(cfg);
        assert!(p.is_configured(), "a credential pool alone configures the provider");

        let c1 = p.creds().unwrap();
        let c2 = p.creds().unwrap();
        let c3 = p.creds().unwrap();
        assert_ne!(c1.key_id, c2.key_id, "consecutive credential sets differ");
        assert_eq!(c1.key_id, c3.key_id, "round-robin wraps");
        // The matching secret + region travel with the selected set.
        assert_eq!(c1.key_id.as_deref(), Some("AKIA1"));
        assert_eq!(c1.key_secret.as_deref(), Some("s1"));
        assert_eq!(c1.region.as_deref(), Some("us-east-1"));
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(), model: model.to_string(), n: 3, negative_prompt: Some("blurry".into()),
            seed: Some(7), reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    fn make_extras() -> ImageExtras {
        ImageExtras {
            vendor_job: Default::default(),
            size: None, resolution: None, aspect_ratio: None, quality: None, style: None,
            steps: None, guidance_scale: None, strength: None, response_format: "url".to_string(), extra: None,
            edit: None, views: Vec::new(),
            output_format: None, background: None,
            expand: None,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        }
    }

    const SD35: &str = "bedrock/stability.sd3-5-large-v1:0";
    const CORE: &str = "bedrock/stability.stable-image-core-v1:1";
    const ULTRA: &str = "bedrock/stability.stable-image-ultra-v1:1";
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nSTABILITY";

    async fn answering(model: &str, answer: Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/model/{model}/invoke")))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .mount(&server)
            .await;
        server
    }

    fn ok_answer() -> Value {
        json!({ "seeds": [7], "finish_reasons": [null], "images": [B64.encode(PNG)] })
    }

    fn no_refs() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    fn init_image() -> MaterializedRequest {
        MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef { role: "init".into(), form: MaterializedRefForm::Base64(B64.encode(b"SOURCE")), time_s: None }],
            cleanup: Cleanup::empty(),
        }
    }

    /// Text-to-image: Stability's own body ("prompt", "negative_prompt",
    /// "seed", "aspect_ratio", "output_format") to InvokeModel, SigV4-signed for
    /// us-west-2 (the only region that serves it) although the credential
    /// names us-east-1; one image is asked for whatever `n` says; the answer's
    /// images[0] is the image.
    /// @see <https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-diffusion-3-5-large.md> (2026-11-04)
    #[tokio::test]
    async fn sd3_5_large_sends_stabilitys_body_signed_for_us_west_2() {
        let server = answering("stability.sd3-5-large-v1:0", ok_answer()).await;
        let mut extras = make_extras();
        extras.aspect_ratio = Some("16:9".into());
        extras.output_format = Some("webp".into());
        let out = make_provider(&server.uri())
            .generate(&ref_schema(SD35), &make_base("a car made out of vegetables", SD35), &extras, &no_refs())
            .await
            .expect("generated");
        assert_eq!(out.data, PNG);
        assert_eq!(out.content_type, "image/png", "the bytes say what they are");

        let received = server.received_requests().await.unwrap();
        let auth = received[0].headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/") && auth.contains("/us-west-2/bedrock/aws4_request"), "auth: {auth}");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(
            body,
            json!({ "prompt": "a car made out of vegetables", "negative_prompt": "blurry", "seed": 7, "aspect_ratio": "16:9", "output_format": "webp" }),
            "no count, no mode for text-to-image"
        );
    }

    /// Image-to-image: SD3.5 Large takes `mode: image-to-image` with `image`
    /// and `strength` ("image-to-image requires the prompt, image, and strength
    /// parameters": 0.5 when none is asked); Ultra takes `image` and `strength`
    /// ("Default: 0.35": left out when none is asked). Neither gets
    /// `aspect_ratio` ("Valid for text-to-image requests only").
    #[tokio::test]
    async fn image_to_image_sends_the_image_and_strength_and_no_ratio() {
        let mut extras = make_extras();
        extras.aspect_ratio = Some("1:1".into());
        let server = answering("stability.sd3-5-large-v1:0", ok_answer()).await;
        make_provider(&server.uri()).generate(&ref_schema(SD35), &make_base("in watercolor", SD35), &extras, &init_image()).await.expect("generated");
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!((body["mode"].as_str(), body["image"].as_str(), body["strength"].as_f64()), (Some("image-to-image"), Some(B64.encode(b"SOURCE").as_str()), Some(0.5)));
        assert!(body.get("aspect_ratio").is_none(), "{body}");

        let server = answering("stability.stable-image-ultra-v1:1", ok_answer()).await;
        make_provider(&server.uri()).generate(&ref_schema(ULTRA), &make_base("in watercolor", ULTRA), &extras, &init_image()).await.expect("generated");
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body["image"].as_str(), Some(B64.encode(b"SOURCE").as_str()));
        assert!(body.get("strength").is_none() && body.get("mode").is_none() && body.get("aspect_ratio").is_none(), "{body}");
        extras.strength = Some(0.8);
        let server = answering("stability.stable-image-ultra-v1:1", ok_answer()).await;
        make_provider(&server.uri()).generate(&ref_schema(ULTRA), &make_base("in watercolor", ULTRA), &extras, &init_image()).await.expect("generated");
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(body["strength"], 0.8);
    }

    /// A finish reason that is not null ("Filter reason: prompt", "Filter
    /// reason: output image", "Filter reason: input image", "Inference error")
    /// is the vendor's refusal, not sent again.
    #[tokio::test]
    async fn a_filtered_answer_is_a_refusal_with_its_reason() {
        let server = answering("stability.stable-image-core-v1:1", json!({ "finish_reasons": ["Filter reason: prompt"] })).await;
        match make_provider(&server.uri()).generate(&ref_schema(CORE), &make_base("x", CORE), &make_extras(), &no_refs()).await {
            Err(ProviderError::RequestFailed { message, retryable: false, .. }) => assert!(message.contains("Filter reason: prompt"), "{message}"),
            other => panic!("{other:?}"),
        }
    }

    /// Every Bedrock image row is a Stability model this adapter speaks; any
    /// other id is refused before a call (never a default model).
    #[tokio::test]
    async fn every_bedrock_row_is_a_model_this_adapter_speaks() {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let reg = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        let rows: Vec<_> = reg.for_provider("bedrock").filter(|m| m.media_type == crate::capabilities::MediaType::Image).collect();
        assert_eq!(rows.len(), 3);
        for m in rows {
            assert!(stability_model(BedrockImageProvider::resolve_model(&m.id)).is_some(), "{}", m.id);
        }
        let server = MockServer::start().await;
        let mut gone = ref_schema(CORE);
        gone.id = "bedrock/amazon.nova-canvas-v1:0".into();
        assert!(make_provider(&server.uri()).generate(&gone, &make_base("x", &gone.id), &make_extras(), &no_refs()).await.is_err());
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
