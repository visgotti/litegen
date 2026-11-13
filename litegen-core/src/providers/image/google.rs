use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRequest, MaterializedRefForm};
use crate::providers::auth::AuthSpec;
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderCredentials, ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// Google Imagen / Gemini image generation provider.
///
/// Routes through the Gemini `generateContent` API with image response modality.
///
/// @see <https://aistudio.google.com/api-reference/generate-content> — `POST /v1beta/models/{model}:generateContent` REST reference
/// @see <https://aistudio.google.com/docs/image-generation?generate-content=true> — image-generation guide,
///   generateContent variant (`responseModalities: ["image"]` and the `inlineData` image part
///   shape); the plain page documents the Interactions API (2026-11-09)
///
/// Authenticated with the `x-goog-api-key` header, as every REST sample in the
/// image guide does (`-H "x-goog-api-key: $GEMINI_API_KEY"`). Never the `?key=`
/// query parameter: reqwest writes the request URL into its error text, and
/// that text reaches the caller's error body, the logs and the database.
pub struct GoogleProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl GoogleProvider {
    /// The client for the vendor's API, carrying the operator's `extra_headers`
    /// (result downloads use their own client and never carry them).
    fn api_client(extra_headers: &std::collections::HashMap<String, String>) -> Client {
        crate::providers::api_client(std::time::Duration::from_secs(120), extra_headers)
    }

    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::raw_header("x-goog-api-key"),
            client: Self::api_client(&Default::default()),
        }
    }

    fn api_key(&self) -> Result<String, ProviderError> {
        if let Some(pool) = &self.key_pool {
            return Ok(pool.next().to_string());
        }
        self.config
            .as_ref()
            .map(|c| c.api_key.clone())
            .ok_or_else(|| ProviderError::NotConfigured("google".into()))
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://generativelanguage.googleapis.com/v1beta")
    }

    /// The credentials to sign one request with: the configured ones, with the
    /// key the weighted pool picked.
    fn creds(&self, api_key: String) -> ProviderCredentials {
        self.config
            .as_ref()
            .map(|c| c.credentials.clone())
            .unwrap_or_default()
            .with_api_key(api_key)
    }

    /// Build the request URL. Imagen models use the `:predict` surface
    /// (`instances`/`parameters` body), while Gemini native image models use
    /// `:generateContent`.
    ///
    /// Google's Imagen reference page is gone (Imagen was shut down 2026-08-17:
    /// "Imagen is shut down and no longer available through the Gemini API.");
    /// the `:predict` surface survives only as `models.predict` in the v1beta
    /// discovery document.
    /// @see <https://aistudio.google.com/docs/image-generation> (2026-11-09)
    /// @see <https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta> — `models.predict` (2026-11-09)
    /// `native` (the row's model) picks the verb; `wire` is the id sent.
    fn api_url(&self, native: &str, wire: &str) -> String {
        let model = wire;
        let verb = if Self::is_imagen_model(native) {
            "predict"
        } else {
            "generateContent"
        };
        format!("{}/models/{model}:{verb}", self.api_base())
    }

    /// True if the resolved native model id belongs to the Imagen family,
    /// which is served on the `:predict` endpoint (distinct from Gemini).
    fn is_imagen_model(model: &str) -> bool {
        model.starts_with("imagen")
    }

    /// Map internal litegen model IDs to actual Gemini/Imagen API model names.
    /// `None` for an id with no arm: refused rather than billed as another
    /// model (L13).
    fn resolve_model(model_id: &str) -> Option<&'static str> {
        // Strip provider prefix first
        let native = if let Some(rest) = model_id.strip_prefix("google/") {
            rest
        } else {
            model_id
        };

        Some(match native {
            "gemini-2.5-flash-image" | "gemini-2.5-flash" => "gemini-2.5-flash-image",
            // GA since 2026-05-28. The `-preview` alias we used to append was
            // shut down on 2026-06-25.
            // @see <https://aistudio.google.com/docs/changelog> (2026-11-09)
            "gemini-3-pro-image" | "gemini-3-pro" => "gemini-3-pro-image",
            "gemini-3.1-flash-image" => "gemini-3.1-flash-image",
            "gemini-3.1-flash-lite-image" => "gemini-3.1-flash-lite-image",
            "gemini-nano-banana-2.1" => "gemini-nano-banana-2.1",
            _ => return None,
        })
    }

}

impl Default for GoogleProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// litegen's tier as Google's `imageSize`: "Supported values are `512`, `1K`,
/// `2K`, `4K`. If not specified, the model will use default value `1K`." Each
/// row declares the ones its model takes (3.1 Flash Image "adds the smaller
/// 512px (0.5K) resolution").
/// @see <https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta> — `ImageConfig` (2026-11-03)
fn image_size(resolution: &str) -> Option<&'static str> {
    match resolution {
        "0.5K" => Some("512"),
        "1K" => Some("1K"),
        "2K" => Some("2K"),
        "4K" => Some("4K"),
        _ => None,
    }
}

#[async_trait]
impl ImageProvider for GoogleProvider {
    fn name(&self) -> &str {
        "google"
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
            .is_some_and(|c| !c.api_key.is_empty() || self.key_pool.is_some())
    }

    /// Generate an image. Imagen models route through `POST
    /// {api_base}/models/{model}:predict` (`instances`/`parameters` body,
    /// `predictions[].bytesBase64Encoded` response); Gemini native image models
    /// route through `POST {api_base}/models/{model}:generateContent`.
    ///
    /// @see <https://aistudio.google.com/api-reference/generate-content> — `POST /v1beta/models/{model}:generateContent`.
    ///   Proves the `contents[].parts[]` request structure (text + `inlineData`/`fileData`),
    ///   the `generationConfig` (`responseModalities`, `seed`, nested
    ///   `imageConfig.aspectRatio`) and the `candidates[].content.parts[].inlineData`
    ///   response this method reads. The `x-goog-api-key` header is how every
    ///   REST sample in the image guide authenticates (the reference's own
    ///   samples use `?key=`).
    /// @see <https://aistudio.google.com/docs/image-generation?generate-content=true> (2026-11-09)
    /// @see <https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta> — `models.predict`
    ///   (`PredictRequest.instances`/`parameters`, `PredictResponse.predictions`) for Imagen,
    ///   whose reference page is gone (2026-11-09).
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let api_key = self.api_key()?;
        let gemini_model = Self::resolve_model(&model.id).ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "google".into() })?;
        let url = self.api_url(gemini_model, crate::providers::wire_model(self.config.as_ref(), &model.id, gemini_model));
        let is_imagen = Self::is_imagen_model(gemini_model);

        // The requested aspect ratio. No Google row declares `size`, so none
        // reaches here to derive one from.
        let aspect_ratio: Option<String> = extras.aspect_ratio.clone();

        let body = if is_imagen {
            // ── Imagen :predict surface ───────────────────────────────────
            // {"instances":[{"prompt":...}],"parameters":{"sampleCount":...,
            //  "aspectRatio":...}}
            // @see <https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta> — models.predict (PredictRequest, 2026-11-09)
            let mut instance = json!({ "prompt": base.prompt });
            if let Some(np) = base.negative_prompt.as_deref() {
                instance["negativePrompt"] = Value::String(np.to_string());
            }

            let mut parameters = json!({
                "sampleCount": base.n.max(1)
            });
            if let Some(ar) = &aspect_ratio {
                parameters["aspectRatio"] = Value::String(ar.clone());
            }
            if let Some(seed) = base.seed {
                parameters["seed"] = Value::Number(seed.into());
            }

            // `extra` adds Imagen's own parameters (personGeneration); it
            // cannot replace what litegen sets, nor ask for more images than
            // litegen returns and bills (rule 11, N1).
            if let Some(p_obj) = parameters.as_object_mut() {
                for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| matches!(k, "sampleCount" | "aspectRatio" | "seed")) {
                    p_obj.insert(k.clone(), v.clone());
                }
            }

            json!({
                "instances": [instance],
                "parameters": parameters
            })
        } else {
            // ── Gemini :generateContent surface ───────────────────────────
            // Build the parts array for the content
            let mut parts: Vec<Value> = vec![json!({
                "text": base.prompt
            })];

            // Append reference images as inlineData parts
            for r in &materialized.refs {
                match &r.form {
                    MaterializedRefForm::Base64(b64) => {
                        // The type the bytes are: `Blob.mimeType` is "the IANA
                        // standard MIME type of the source data".
                        let head = B64.decode(b64.get(..b64.len().min(64)).unwrap_or("")).unwrap_or_default();
                        let mime = crate::proxy::materializer::sniff_image_mime(&head).unwrap_or("image/png");
                        parts.push(json!({
                            "inlineData": {
                                "mimeType": mime,
                                "data": b64
                            }
                        }));
                    }
                    MaterializedRefForm::Url(url) => {
                        // Gemini can handle file URIs — include as fileData
                        parts.push(json!({
                            "fileData": {
                                "fileUri": url,
                                "mimeType": "image/png"
                            }
                        }));
                    }
                    _ => {}
                }
            }

            // Gemini doesn't have a direct negative_prompt; append as a text part
            if let Some(np) = base.negative_prompt.as_deref() {
                parts.push(json!({ "text": format!("Do not include: {np}") }));
            }

            // Build generationConfig
            // The wire value is the proto enum name — `Modality.IMAGE` in the
            // Google GenAI SDK. Lowercase is undocumented.
            let mut gen_config = json!({
                "responseModalities": ["IMAGE"]
            });

            // Aspect ratio must be nested under imageConfig for the
            // generateContent image surface — Google ignores a top-level
            // generationConfig.aspectRatio key.
            if let Some(ar) = &aspect_ratio {
                gen_config["imageConfig"] = json!({ "aspectRatio": ar });
            }

            if let Some(seed) = base.seed {
                gen_config["seed"] = Value::Number(seed.into());
            }

            // No `candidateCount`: it defaults to 1, and litegen returns (and
            // bills) one image.

            // The tier is `imageConfig.imageSize`, beside `aspectRatio`: the REST
            // `ImageConfig` has exactly those two members.
            // @see <https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta> — `ImageConfig` (2026-11-03)
            if let Some(tier) = extras.resolution.as_deref() {
                let size = image_size(tier).ok_or_else(|| ProviderError::InvalidRequest(format!("{}: Google has no image size {tier}", model.id)))?;
                if !gen_config["imageConfig"].is_object() {
                    gen_config["imageConfig"] = json!({});
                }
                gen_config["imageConfig"]["imageSize"] = Value::String(size.into());
            }

            // Merge extra fields; nothing litegen sets is replaced, the tier
            // (`imageSize`, from `resolution`) included.
            const OWNED: [&str; 5] = ["responseModalities", "candidateCount", "seed", "imageConfig", "imageSize"];
            if let Some(cfg_obj) = gen_config.as_object_mut() {
                for (k, v) in crate::providers::extra_fields(extras.extra.as_ref(), |k| OWNED.contains(&k)) {
                    cfg_obj.insert(k.clone(), v.clone());
                }
            }

            json!({
                "contents": [{
                    "parts": parts
                }],
                "generationConfig": gen_config
            })
        };

        let builder = crate::providers::auth::apply(
            &self.auth,
            &self.creds(api_key),
            self.client.post(&url).header("Content-Type", "application/json").json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder)
        .send()
        .await
        .map_err(|e| {
            let status_code = e.status().map(|s| s.as_u16());
            // Only a connection that never opened was surely not received:
            // a call that timed out may still run, and bill, at Google.
            let retryable = e.is_connect();
            ProviderError::RequestFailed {
                // No URL in the text, whatever it carries: errors reach callers.
                message: format!("Google request failed: {}", e.without_url()),
                status_code,
                provider_error: None,
                retryable,
            }
        })?;

        // The status before the body (read_vendor_json): a 429, a 408 or a
        // 5xx turned the call away and is retried, after any `Retry-After`
        // (docs/gateway-settings.md); any other error status is Google's refusal.
        let resp_json = crate::providers::read_vendor_json(resp, "Google").await?;

        let mut metadata = HashMap::new();
        metadata.insert("model".to_string(), Value::String(gemini_model.to_string()));

        let (b64_data, mime_type): (String, String) = if is_imagen {
            // Imagen :predict returns:
            //   predictions[0].{bytesBase64Encoded, mimeType}
            // @see <https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta> — PredictResponse.predictions (2026-11-09)
            let predictions = resp_json["predictions"].as_array().ok_or_else(|| {
                ProviderError::RequestFailed {
                    message: "Google response missing predictions".to_string(),
                    status_code: None,
                    provider_error: Some(resp_json.clone()),
                    retryable: false,
                }
            })?;

            let first = predictions.first().ok_or_else(|| ProviderError::RequestFailed {
                message: "Google response has empty predictions".to_string(),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?;

            let data = first["bytesBase64Encoded"].as_str().ok_or_else(|| {
                ProviderError::RequestFailed {
                    message: "Google prediction missing bytesBase64Encoded".to_string(),
                    status_code: None,
                    provider_error: Some(resp_json.clone()),
                    retryable: false,
                }
            })?;

            let mime = first["mimeType"].as_str().unwrap_or("image/png").to_string();
            (data.to_string(), mime)
        } else {
            // Gemini returns:
            //   candidates[0].content.parts[{inlineData.data, inlineData.mimeType}]
            let candidates = resp_json["candidates"].as_array().ok_or_else(|| {
                ProviderError::RequestFailed {
                    message: "Google response missing candidates".to_string(),
                    status_code: None,
                    provider_error: Some(resp_json.clone()),
                    retryable: false,
                }
            })?;

            let first_candidate = candidates.first().ok_or_else(|| {
                ProviderError::RequestFailed {
                    message: "Google response has empty candidates".to_string(),
                    status_code: None,
                    provider_error: None,
                    retryable: false,
                }
            })?;

            // A refused image may come with no content at all; the search
            // below then answers with Google's reason.
            let no_parts = Vec::new();
            let parts_arr = first_candidate["content"]["parts"].as_array().unwrap_or(&no_parts);

            // Include any text parts (e.g. revised prompt)
            for part in parts_arr {
                if let Some(text) = part["text"].as_str() {
                    metadata.insert("text".to_string(), Value::String(text.to_string()));
                    break;
                }
            }

            // The final image is the last one that is not a thought: "The
            // model generates up to two interim images to test composition
            // and logic. The last image within Thinking is also the final
            // rendered image." With none, Google's reason is the error.
            // @see <https://aistudio.google.com/docs/image-generation?generate-content=true> (2026-11-09)
            let image_part = parts_arr
                .iter()
                .rev()
                .find(|p| p["inlineData"].is_object() && p["thought"] != Value::Bool(true))
                .ok_or_else(|| {
                    let why = first_candidate["finishReason"]
                        .as_str()
                        .or_else(|| resp_json["promptFeedback"]["blockReason"].as_str())
                        .unwrap_or("no reason given");
                    ProviderError::RequestFailed {
                        message: format!("Google returned no image ({why})"),
                        status_code: None,
                        provider_error: Some(resp_json.clone()),
                        retryable: false,
                    }
                })?;

            let mime = image_part["inlineData"]["mimeType"]
                .as_str()
                .unwrap_or("image/png")
                .to_string();

            let data = image_part["inlineData"]["data"].as_str().ok_or_else(|| {
                ProviderError::RequestFailed {
                    message: "Google inlineData missing data".to_string(),
                    status_code: None,
                    provider_error: None,
                    retryable: false,
                }
            })?;

            (data.to_string(), mime)
        };

        let image_bytes = B64.decode(&b64_data).map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to decode Google image data: {e}"),
            status_code: None,
            provider_error: None,
            retryable: false,
        })?;

        Ok(GenerationOutput {
            data: image_bytes,
            content_type: mime_type,
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

    /// Validate the API key via `GET {api_base}/models`, key in the header.
    ///
    /// @see <https://aistudio.google.com/api-reference/models/list-models> — `models.list` (`GET /v1beta/models`, 2026-11-09)
    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Google provider not configured".into(),
                latency_ms: None,
            };
        }
        let start = std::time::Instant::now();
        let api_key = match self.api_key() {
            Ok(k) => k,
            Err(_) => {
                return HealthCheckResult {
                    healthy: false,
                    message: "No API key".into(),
                    latency_ms: None,
                }
            }
        };
        let request = match crate::providers::auth::apply(
            &self.auth,
            &self.creds(api_key),
            self.client
                .get(format!("{}/models", self.api_base()))
                .timeout(std::time::Duration::from_secs(10)),
        ) {
            Ok(r) => r,
            Err(e) => {
                return HealthCheckResult {
                    healthy: false,
                    message: e.to_string(),
                    latency_ms: None,
                }
            }
        };
        match request.send().await {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "Google API key valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("Google returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("Google health check failed: {}", e.without_url()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::{method, path_regex};

    /// L13: every catalog row has its own arm, and an id with none is refused
    /// rather than billed as another model.
    #[test]
    fn every_catalog_row_resolves_and_an_unknown_id_does_not() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        for m in catalog.for_provider("google").filter(|m| m.media_type == crate::capabilities::MediaType::Image) {
            assert!(GoogleProvider::resolve_model(&m.id).is_some(), "{} has no arm", m.id);
        }
        assert!(GoogleProvider::resolve_model("google/not-a-model").is_none());
    }

    #[cfg(test)]
    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    #[cfg(test)]
    fn empty_materialized() -> crate::proxy::materializer::MaterializedRequest {
        crate::proxy::materializer::MaterializedRequest {
            refs: vec![],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        }
    }

    fn make_provider(api_base: &str) -> GoogleProvider {
        let mut p = GoogleProvider::new();
        p.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: "test-api-key".to_string(),
            api_keys: vec![],
            api_base: Some(api_base.to_string()),
            model_mapping: Default::default(),
            extra_headers: Default::default(),
            options: None,
        });
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
            resolution: None, aspect_ratio: Some("1:1".to_string()),
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

    /// The key goes in the `x-goog-api-key` header, as every image sample in
    /// Google's docs sends it. In the URL query it rode along in reqwest's
    /// error text ("… for url (…?key=…)") into the 502 body, logs and the DB.
    /// @see <https://aistudio.google.com/docs/image-generation?generate-content=true> (`-H "x-goog-api-key: $GEMINI_API_KEY"`, 2026-11-09)
    #[tokio::test]
    async fn the_api_key_travels_in_a_header_never_in_the_url() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex("/models/gemini-2.5-flash-image:generateContent"))
            .and(wiremock::matchers::header("x-goog-api-key", "test-api-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{ "content": { "parts": [
                    { "inlineData": { "mimeType": "image/png", "data": B64.encode(b"png") } }
                ] } }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("google/gemini-2.5-flash-image");
        let base = make_base("a lighthouse", "google/gemini-2.5-flash-image");
        provider.generate(&schema, &base, &make_extras(), &empty_materialized()).await.expect("generate");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert!(received[0].url.query().is_none_or(|q| !q.contains("key")), "{}", received[0].url);
    }

    #[tokio::test]
    async fn a_transport_error_never_carries_the_api_key() {
        // Nothing listens on port 1: a connect error, the case whose text used
        // to include the full URL.
        let mut provider = GoogleProvider::new();
        provider.configure(ProviderInstanceConfig {
            api_key: "AIza-SECRET-TEST-KEY".to_string(),
            api_base: Some("http://127.0.0.1:1/v1beta".to_string()),
            ..Default::default()
        });
        let schema = ref_schema("google/gemini-2.5-flash-image");
        let base = make_base("a lighthouse", "google/gemini-2.5-flash-image");
        let err = provider
            .generate(&schema, &base, &make_extras(), &empty_materialized())
            .await
            .expect_err("nothing listens there");
        assert!(!format!("{err} {err:?}").contains("AIza-SECRET-TEST-KEY"), "{err:?}");

        let health = provider.health_check().await;
        assert!(!health.healthy);
        assert!(!health.message.contains("AIza-SECRET-TEST-KEY"), "{}", health.message);
    }

    #[tokio::test]
    async fn the_health_check_uses_the_configured_base_and_the_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wiremock::matchers::path("/v1beta/models"))
            .and(wiremock::matchers::header("x-goog-api-key", "test-api-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "models": [] })))
            .mount(&server)
            .await;
        let provider = make_provider(&format!("{}/v1beta", server.uri()));
        let health = provider.health_check().await;
        assert!(health.healthy, "{}", health.message);
        let received = server.received_requests().await.unwrap();
        assert!(received[0].url.query().is_none_or(|q| !q.contains("key")), "{}", received[0].url);
    }

    #[tokio::test]
    async fn generates_gemini_flash_image() {
        let server = MockServer::start().await;
        let fake_png = b"FAKEPNGDATA";
        let fake_b64 = B64.encode(fake_png);

        Mock::given(method("POST"))
            .and(path_regex("/models/gemini-2.5-flash-image:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{
                    "content": {
                        "parts": [
                            {
                                "inlineData": {
                                    "mimeType": "image/png",
                                    "data": fake_b64
                                }
                            }
                        ]
                    }
                }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("google/gemini-2.5-flash-image");
        let base = make_base("a beautiful mountain landscape", "google/gemini-2.5-flash-image");
        let extras = make_extras();
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let output = result.unwrap();
        assert_eq!(output.data, fake_png);

        // Verify outbound request shape
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["contents"][0]["parts"][0]["text"], "a beautiful mountain landscape");
        assert!(body["generationConfig"]["responseModalities"].as_array().is_some());
    }

    fn registry() -> crate::capabilities::CapabilityRegistry {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load")
    }

    /// `gemini-3-pro-image-preview` was shut down on 2026-06-25 when
    /// `gemini-3-pro-image` went GA. Appending `-preview` targets the dead id.
    /// @see <https://aistudio.google.com/docs/changelog> (2026-11-09)
    #[test]
    fn gemini_3_pro_image_is_not_rewritten_to_the_retired_preview_id() {
        let resolved = GoogleProvider::resolve_model("google/gemini-3-pro-image");
        assert_eq!(resolved, Some("gemini-3-pro-image"));
    }

    /// Imagen 3 was shut down 2025-11-10; the whole Imagen surface (Imagen 4)
    /// on 2026-08-17. @see <https://aistudio.google.com/docs/deprecations> (2026-11-09)
    #[test]
    fn catalog_ships_no_imagen_models() {
        let r = registry();
        assert!(
            r.get("google/imagen-3").is_none(),
            "imagen-3.0-generate-002 was shut down on 2025-11-10"
        );
    }

    #[test]
    fn catalog_ships_the_current_nano_banana_2_model() {
        let r = registry();
        assert!(
            r.get("google/gemini-3.1-flash-image").is_some(),
            "gemini-3.1-flash-image (GA 2026-05-28) is the migration target for \
             both gemini-2.5-flash-image and Imagen"
        );
    }

    /// The canonical wire value is the proto enum name. `Modality.IMAGE` in the
    /// Google GenAI SDK serialises as `"IMAGE"`; lowercase is undocumented.
    #[tokio::test]
    async fn response_modalities_uses_the_canonical_uppercase_enum_name() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex("/models/.*:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{ "content": { "parts": [
                    { "inlineData": { "mimeType": "image/png", "data": B64.encode(b"x") } }
                ] } }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("google/gemini-3.1-flash-image");
        let base = make_base("a fern", "google/gemini-3.1-flash-image");
        provider
            .generate(&schema, &base, &make_extras(), &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(
            body["generationConfig"]["responseModalities"],
            json!(["IMAGE"])
        );
    }

    /// `imageSize` is a member of `generationConfig.imageConfig`, not of
    /// `generationConfig` itself; the REST `ImageConfig` has exactly
    /// `aspectRatio` and `imageSize` (no `personGeneration`).
    /// @see <https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta> (2026-10-30)
    #[tokio::test]
    async fn image_config_keys_are_nested_under_image_config() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex("/models/.*:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{ "content": { "parts": [
                    { "inlineData": { "mimeType": "image/png", "data": B64.encode(b"x") } }
                ] } }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("google/gemini-3-pro-image");
        let base = make_base("a lighthouse", "google/gemini-3-pro-image");
        let mut extras = make_extras();
        extras.resolution = Some("2K".into());

        provider
            .generate(&schema, &base, &extras, &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        let cfg = &body["generationConfig"];
        assert_eq!(cfg["imageConfig"]["imageSize"], "2K");
        assert!(cfg.get("imageSize").is_none(), "imageSize must not be top-level");
        // The aspect ratio still nests under imageConfig alongside them.
        assert_eq!(cfg["imageConfig"]["aspectRatio"], "1:1");
    }

    // ─── Google audit, 2026-10-30: W3–W7 ────────────────────────────────────

    fn gemini_answer(parts: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({ "candidates": [{ "content": { "parts": parts } }] }))
    }

    async fn sent_body(server: &MockServer) -> Value {
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap()
    }

    /// W3, W4: `candidateCount` is never sent (litegen returns and bills one
    /// image), `extra` cannot replace what litegen sets, and only `imageSize`
    /// nests in `imageConfig`, whose only other member is `aspectRatio`.
    #[tokio::test]
    async fn one_image_is_asked_for_and_extra_cannot_replace_what_litegen_sets() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(gemini_answer(json!([{ "inlineData": { "mimeType": "image/png", "data": B64.encode(b"PNG") } }])))
            .mount(&server)
            .await;
        let mut base = make_base("a fox", "google/gemini-3.1-flash-image");
        base.n = 4;
        let mut extras = make_extras();
        // The tier is litegen's (`resolution`): an `extra.imageSize` neither
        // replaces it nor goes anywhere else.
        extras.resolution = Some("2K".into());
        extras.extra = Some(json!({ "responseModalities": ["TEXT"], "candidateCount": 3, "imageSize": "4K", "personGeneration": "allow_all" }));
        make_provider(&server.uri()).generate(&ref_schema("google/gemini-3.1-flash-image"), &base, &extras, &empty_materialized()).await.unwrap();
        let cfg = sent_body(&server).await["generationConfig"].clone();
        assert_eq!(cfg["responseModalities"], json!(["IMAGE"]), "{cfg}");
        assert!(cfg.get("candidateCount").is_none(), "{cfg}");
        assert_eq!(cfg["imageConfig"], json!({ "aspectRatio": "1:1", "imageSize": "2K" }), "{cfg}");
        assert!(cfg.get("imageSize").is_none(), "{cfg}");
    }

    /// W5: the final image is the last non-thought one, and a reply with none
    /// says why, with Google's own answer.
    #[tokio::test]
    async fn the_last_finished_image_is_kept_and_a_refusal_says_why() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(gemini_answer(json!([
                { "thought": true, "inlineData": { "mimeType": "image/png", "data": B64.encode(b"DRAFT") } },
                { "inlineData": { "mimeType": "image/png", "data": B64.encode(b"FIRST") } },
                { "inlineData": { "mimeType": "image/png", "data": B64.encode(b"FINAL") } },
            ])))
            .mount(&server)
            .await;
        let schema = ref_schema("google/gemini-3.1-flash-image");
        let out = make_provider(&server.uri())
            .generate(&schema, &make_base("x", "google/gemini-3.1-flash-image"), &make_extras(), &empty_materialized())
            .await
            .unwrap();
        assert_eq!(out.data, b"FINAL");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "candidates": [{ "finishReason": "IMAGE_SAFETY", "content": { "parts": [] } }] })))
            .mount(&server)
            .await;
        let err = make_provider(&server.uri())
            .generate(&schema, &make_base("x", "google/gemini-3.1-flash-image"), &make_extras(), &empty_materialized())
            .await
            .unwrap_err();
        match err {
            ProviderError::RequestFailed { message, provider_error, retryable, .. } => {
                assert!(message.contains("IMAGE_SAFETY"), "{message}");
                assert!(provider_error.is_some() && !retryable);
            }
            other => panic!("{other:?}"),
        }
    }

    /// W6: a reference goes with the type its bytes are.
    #[tokio::test]
    async fn a_reference_is_labelled_with_its_real_type() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(gemini_answer(json!([{ "inlineData": { "mimeType": "image/png", "data": B64.encode(b"PNG") } }])))
            .mount(&server)
            .await;
        let jpeg = B64.encode([0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0, 1]);
        let materialized = crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".into(),
                form: MaterializedRefForm::Base64(jpeg),
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };
        make_provider(&server.uri())
            .generate(&ref_schema("google/gemini-3.1-flash-image"), &make_base("x", "google/gemini-3.1-flash-image"), &make_extras(), &materialized)
            .await
            .unwrap();
        assert_eq!(sent_body(&server).await["contents"][0]["parts"][1]["inlineData"]["mimeType"], "image/jpeg");
    }

    /// W7: a call that timed out may still have run, and been billed, at
    /// Google: it is never sent again.
    #[tokio::test]
    async fn a_timed_out_call_is_never_sent_again() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(gemini_answer(json!([])).set_delay(std::time::Duration::from_secs(5)))
            .mount(&server)
            .await;
        let mut p = make_provider(&server.uri());
        p.client = Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        let err = p
            .generate(&ref_schema("google/gemini-3.1-flash-image"), &make_base("x", "google/gemini-3.1-flash-image"), &make_extras(), &empty_materialized())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
    }

    /// A 429 turned the call away and is retried, after the `Retry-After`
    /// Google asked for when it sent one (docs/gateway-settings.md).
    #[tokio::test]
    async fn a_429_is_retryable() {
        let body = json!({ "error": { "code": 429, "message": "Resource has been exhausted (e.g. check quota).", "status": "RESOURCE_EXHAUSTED" } });
        for (answer, wait_ms) in [
            (ResponseTemplate::new(429).insert_header("retry-after", "2").set_body_json(body.clone()), Some(2000)),
            (ResponseTemplate::new(429).set_body_json(body.clone()), None),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).respond_with(answer).mount(&server).await;
            let err = make_provider(&server.uri())
                .generate(&ref_schema("google/gemini-3.1-flash-image"), &make_base("x", "google/gemini-3.1-flash-image"), &make_extras(), &empty_materialized())
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
                .generate(&ref_schema("google/gemini-3.1-flash-image"), &make_base("x", "google/gemini-3.1-flash-image"), &make_extras(), &empty_materialized())
                .await
                .unwrap_err();
        assert!(matches!(&err, ProviderError::RequestFailed { retryable: true, status_code: Some(503), .. }), "{err:?}");
    }

    /// litegen's tier goes as `imageConfig.imageSize`: "Supported values are
    /// `512`, `1K`, `2K`, `4K`" — litegen's `0.5K` is Google's `512`. Every
    /// tier a Google row declares maps; nothing else does.
    /// @see <https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta> (ImageConfig, 2026-11-03)
    #[test]
    fn every_declared_resolution_is_one_of_googles_image_sizes() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let reg = crate::capabilities::CapabilityRegistry::from_dir(&dir).expect("load");
        let mut seen = 0;
        for m in reg.for_provider("google") {
            if let Some(crate::capabilities::ParamSpec::String(s)) = m.params.get("resolution") {
                for v in &s.enum_values {
                    seen += 1;
                    assert!(matches!(image_size(v), Some("512" | "1K" | "2K" | "4K")), "{}: {v}", m.id);
                }
            }
        }
        assert!(seen > 0, "no Google row declares a resolution");
        assert_eq!((image_size("0.5K"), image_size("1.5K"), image_size("2k")), (Some("512"), None, None));
    }
}
