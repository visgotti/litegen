use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRequest, MaterializedRefForm};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// Google Imagen / Gemini image generation provider.
///
/// Routes through the Gemini `generateContent` API with image response modality.
///
/// @see <https://ai.google.dev/api/generate-content> — `models.generateContent` REST reference
/// @see <https://ai.google.dev/gemini-api/docs/image-generation> — image-generation guide
///   (`responseModalities: ["image"]` and the `inlineData` image part shape)
pub struct GoogleProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    client: Client,
}

impl GoogleProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("failed to build HTTP client"),
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

    /// Build the request URL. Imagen models use the `:predict` surface
    /// (`instances`/`parameters` body), while Gemini native image models use
    /// `:generateContent`.
    ///
    /// @see <https://ai.google.dev/api/imagen> — `models.predict` for Imagen
    fn api_url(&self, model: &str) -> String {
        let verb = if Self::is_imagen_model(model) {
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
    fn resolve_model(model_id: &str) -> &'static str {
        // Strip provider prefix first
        let native = if let Some(rest) = model_id.strip_prefix("google/") {
            rest
        } else {
            model_id
        };

        match native {
            "gemini-2.5-flash-image" | "gemini-2.5-flash" => "gemini-2.5-flash-image",
            // GA since 2026-05-28. The `-preview` alias we used to append was
            // shut down on 2026-06-25.
            // @see <https://ai.google.dev/gemini-api/docs/changelog>
            "gemini-3-pro-image" | "gemini-3-pro" => "gemini-3-pro-image",
            "gemini-3.1-flash-image" => "gemini-3.1-flash-image",
            "gemini-3.1-flash-lite-image" => "gemini-3.1-flash-lite-image",
            // Google's migration target for both gemini-2.5-flash-image
            // (shutdown 2026-10-02) and the retired Imagen line.
            _ => "gemini-3.1-flash-image",
        }
    }

    /// Map pixel dimensions to a Google-supported aspect ratio.
    fn aspect_ratio_from_size(size: &str) -> Option<&'static str> {
        let parts: Vec<&str> = size.split('x').collect();
        if parts.len() != 2 {
            return None;
        }
        let w: f64 = parts[0].parse().ok()?;
        let h: f64 = parts[1].parse().ok()?;
        if h == 0.0 {
            return None;
        }
        let ratio = w / h;
        // `ImageConfig.aspect_ratio` supports exactly these eight values.
        // `5:4` and `4:5` are NOT members and are rejected by the API, so the
        // near-square band collapses to `1:1`.
        // @see googleapis/python-genai `types.ImageConfig.aspect_ratio`
        Some(if ratio >= 2.1 {
            "21:9"
        } else if ratio >= 1.7 {
            "16:9"
        } else if ratio >= 1.4 {
            "3:2"
        } else if ratio >= 1.18 {
            "4:3"
        } else if ratio >= 0.85 {
            "1:1"
        } else if ratio >= 0.7 {
            "3:4"
        } else if ratio >= 0.58 {
            "2:3"
        } else {
            "9:16"
        })
    }
}

impl Default for GoogleProvider {
    fn default() -> Self {
        Self::new()
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
    /// @see <https://ai.google.dev/api/generate-content> — `models.generateContent`.
    ///   Proves the `contents[].parts[]` request structure (text + `inlineData`/`fileData`),
    ///   the `generationConfig` (`responseModalities`, `seed`, nested
    ///   `imageConfig.aspectRatio`), `?key=` auth, and the
    ///   `candidates[].content.parts[].inlineData` response this method reads.
    /// @see <https://ai.google.dev/api/imagen> — `models.predict` for Imagen.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let api_key = self.api_key()?;
        let gemini_model = Self::resolve_model(&model.id);
        let url = self.api_url(gemini_model);
        let is_imagen = Self::is_imagen_model(gemini_model);

        // Resolve the requested aspect ratio (explicit, else derived from size).
        let aspect_ratio: Option<String> = extras
            .aspect_ratio
            .as_deref()
            .map(|s| s.to_string())
            .or_else(|| {
                extras
                    .size
                    .as_deref()
                    .and_then(Self::aspect_ratio_from_size)
                    .map(|s| s.to_string())
            });

        let body = if is_imagen {
            // ── Imagen :predict surface ───────────────────────────────────
            // {"instances":[{"prompt":...}],"parameters":{"sampleCount":...,
            //  "aspectRatio":...}}
            // @see <https://ai.google.dev/api/imagen> — models.predict
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

            // Shallow-merge extra fields into parameters (e.g. personGeneration).
            if let Some(Value::Object(extra_map)) = &extras.extra {
                if let Some(p_obj) = parameters.as_object_mut() {
                    for (k, v) in extra_map {
                        p_obj.insert(k.clone(), v.clone());
                    }
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
                        parts.push(json!({
                            "inlineData": {
                                "mimeType": "image/png",
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

            // Number of images — the generateContent surface uses
            // candidateCount; `numberOfImages` belongs to the Imagen :predict
            // API and Google 400s on unknown generationConfig fields. Omit
            // entirely for n<=1 (the default) to stay off models that reject
            // candidateCount for image output.
            if base.n > 1 {
                gen_config["candidateCount"] = Value::Number(base.n.into());
            }

            // Merge extra fields. `imageSize` and `personGeneration` are
            // members of `generationConfig.imageConfig`, not of
            // `generationConfig` itself, so they have to be nested.
            // @see googleapis/python-genai `types.ImageConfig`
            const IMAGE_CONFIG_KEYS: [&str; 2] = ["imageSize", "personGeneration"];
            if let Some(Value::Object(extra_map)) = &extras.extra {
                for (k, v) in extra_map {
                    if IMAGE_CONFIG_KEYS.contains(&k.as_str()) {
                        if !gen_config["imageConfig"].is_object() {
                            gen_config["imageConfig"] = json!({});
                        }
                        gen_config["imageConfig"][k] = v.clone();
                    } else if let Some(cfg_obj) = gen_config.as_object_mut() {
                        cfg_obj.insert(k.clone(), v.clone());
                    }
                }
            }

            json!({
                "contents": [{
                    "parts": parts
                }],
                "generationConfig": gen_config
            })
        };

        let resp = crate::providers::inject_trace_headers(
            self.client
                .post(&url)
                .query(&[("key", &api_key)])
                .header("Content-Type", "application/json")
                .json(&body),
        )
        .send()
        .await
        .map_err(|e| ProviderError::RequestFailed {
            message: format!("Google request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_timeout() || e.is_connect(),
        })?;

        let status = resp.status();
        let resp_json: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Google response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        if !status.is_success() {
            let err = resp_json["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error")
                .to_string();
            return Err(ProviderError::RequestFailed {
                message: format!("Google API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(resp_json),
                retryable: status.as_u16() >= 500,
            });
        }

        let mut metadata = HashMap::new();
        metadata.insert("model".to_string(), Value::String(gemini_model.to_string()));

        let (b64_data, mime_type): (String, String) = if is_imagen {
            // Imagen :predict returns:
            //   predictions[0].{bytesBase64Encoded, mimeType}
            // @see <https://ai.google.dev/api/imagen> — models.predict response
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

            let parts_arr = first_candidate["content"]["parts"].as_array().ok_or_else(|| {
                ProviderError::RequestFailed {
                    message: "Google response missing parts".to_string(),
                    status_code: None,
                    provider_error: None,
                    retryable: false,
                }
            })?;

            // Include any text parts (e.g. revised prompt)
            for part in parts_arr {
                if let Some(text) = part["text"].as_str() {
                    metadata.insert("text".to_string(), Value::String(text.to_string()));
                    break;
                }
            }

            // Find the part with inlineData (image)
            let image_part = parts_arr
                .iter()
                .find(|p| p["inlineData"].is_object())
                .ok_or_else(|| ProviderError::RequestFailed {
                    message: "Google response missing image inlineData part".to_string(),
                    status_code: None,
                    provider_error: Some(resp_json.clone()),
                    retryable: false,
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

    /// Validate the API key via `GET {generativelanguage}/v1beta/models?key=…`.
    ///
    /// @see <https://ai.google.dev/api/models#method:-models.list> — `models.list`
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
        match self
            .client
            .get(format!(
                "https://generativelanguage.googleapis.com/v1beta/models?key={api_key}"
            ))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
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
                message: format!("Google health check failed: {e}"),
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
            size: None,
            aspect_ratio: Some("1:1".to_string()),
            quality: None,
            style: None,
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".to_string(),
            extra: None,
        }
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
    /// @see <https://ai.google.dev/gemini-api/docs/changelog>
    #[test]
    fn gemini_3_pro_image_is_not_rewritten_to_the_retired_preview_id() {
        let resolved = GoogleProvider::resolve_model("google/gemini-3-pro-image");
        assert_eq!(resolved, "gemini-3-pro-image");
    }

    /// Imagen 3 was shut down 2025-11-10; the whole Imagen surface (Imagen 4)
    /// shuts down 2026-08-17. @see <https://ai.google.dev/gemini-api/docs/deprecations>
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

    /// `ImageConfig.aspect_ratio` supports exactly
    /// `1:1, 2:3, 3:2, 3:4, 4:3, 9:16, 16:9, 21:9`. The size-derivation helper
    /// used to emit `5:4` and `4:5`, which are rejected.
    #[test]
    fn every_derived_aspect_ratio_is_supported_by_image_config() {
        const SUPPORTED: [&str; 8] =
            ["1:1", "2:3", "3:2", "3:4", "4:3", "9:16", "16:9", "21:9"];
        // Sweep a wide range of plausible sizes, including the ratios that used
        // to fall into the 5:4 and 4:5 buckets.
        for w in (256..=3072).step_by(16) {
            for h in [256u32, 512, 768, 832, 1024, 1200, 1280, 1536, 2048] {
                if let Some(ar) = GoogleProvider::aspect_ratio_from_size(&format!("{w}x{h}")) {
                    assert!(
                        SUPPORTED.contains(&ar),
                        "{w}x{h} derived unsupported aspect ratio {ar:?}"
                    );
                }
            }
        }
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

    /// `imageSize` and `personGeneration` are members of
    /// `generationConfig.imageConfig`, not of `generationConfig` itself.
    /// @see googleapis/python-genai `types.ImageConfig`
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
        extras.extra = Some(json!({ "imageSize": "2K", "personGeneration": "ALLOW_ADULT" }));

        provider
            .generate(&schema, &base, &extras, &empty_materialized())
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        let cfg = &body["generationConfig"];
        assert_eq!(cfg["imageConfig"]["imageSize"], "2K");
        assert_eq!(cfg["imageConfig"]["personGeneration"], "ALLOW_ADULT");
        assert!(cfg.get("imageSize").is_none(), "imageSize must not be top-level");
        assert!(
            cfg.get("personGeneration").is_none(),
            "personGeneration must not be top-level"
        );
        // The aspect ratio still nests under imageConfig alongside them.
        assert_eq!(cfg["imageConfig"]["aspectRatio"], "1:1");
    }
}
