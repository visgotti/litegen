use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::MaterializedRequest;
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// Recraft image generation provider (raster + vector).
///
/// Synchronous, OpenAI-SDK-compatible: `POST /v1/images/generations` returns
/// `{created, data: [{url | b64_json}]}`. We request `b64_json` so the bytes
/// come back inline (Recraft result URLs expire). Bearer auth. Image-only.
///
/// @see <https://www.recraft.ai/docs/api-reference/getting-started.md> — base + auth
///   Verbatim: "'https://external.api.recraft.ai/v1'"
///   Verbatim: "Authorization: Bearer RECRAFT_API_TOKEN"
/// @see <https://www.recraft.ai/docs/api-reference/endpoints.md>
///   Verbatim: "POST https://external.api.recraft.ai/v1/images/generations"
pub struct RecraftProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl RecraftProvider {
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
            .unwrap_or("https://external.api.recraft.ai/v1")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("recraft".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    fn resolve_model(model_id: &str) -> &str {
        model_id.strip_prefix("recraft/").unwrap_or(model_id)
    }
}

impl RecraftProvider {
    /// Remove background (PixelGenie L8): the image uploaded as `file`, the
    /// result asked as base64 in `image_format` png (Recraft's default is
    /// lossless WebP) unless webp is asked. It answers one `image`, not a
    /// `data` array.
    ///
    /// @see <https://www.recraft.ai/docs/api-reference/tools/remove-background.md> — `file` (multipart),
    ///   `response_format`, `image_format` webp | png, and the `{created, credits, image}` response
    async fn remove_background(&self, extras: &ImageExtras, materialized: &MaterializedRequest) -> Result<GenerationOutput, ProviderError> {
        use crate::proxy::materializer::MaterializedRefForm;
        let creds = self.creds()?;
        let file = materialized.refs.iter().find(|r| r.role == "init").and_then(|r| match &r.form {
            MaterializedRefForm::MultipartField { bytes, content_type, .. } => Some((bytes.to_vec(), content_type.clone())),
            _ => None,
        });
        let (bytes, content_type) = file.ok_or_else(|| ProviderError::InvalidRequest("background removal needs an init image".into()))?;
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name("image")
            .mime_str(&content_type)
            .map_err(|e| ProviderError::InvalidRequest(format!("init image type {content_type}: {e}")))?;
        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("response_format", "b64_json")
            .text("image_format", extras.output_format.clone().unwrap_or_else(|| "png".into()));
        let url = format!("{}/images/removeBackground", self.api_base());
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.post(&url).multipart(form))?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Recraft request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_timeout() || e.is_connect(),
        })?;
        let status = resp.status();
        let data: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Recraft response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        if !status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Recraft API error: {data}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                retryable: status.as_u16() >= 500,
            });
        }
        let failed = |message: String| ProviderError::RequestFailed { message, status_code: None, provider_error: None, retryable: false };
        let bytes = if let Some(b64) = data["image"]["b64_json"].as_str() {
            B64.decode(b64).map_err(|e| failed(format!("Failed to decode Recraft b64_json: {e}")))?
        } else if let Some(at) = data["image"]["url"].as_str() {
            let r = self.client.get(at).send().await.map_err(|e| failed(format!("Failed to fetch Recraft image: {e}")))?;
            r.bytes().await.map(|b| b.to_vec()).map_err(|e| failed(format!("Failed to read Recraft image: {e}")))?
        } else {
            return Err(ProviderError::RequestFailed {
                message: "Recraft response has no image".into(),
                status_code: None,
                provider_error: Some(data),
                retryable: false,
            });
        };
        Ok(GenerationOutput { data: bytes, content_type: "image/png".into(), metadata: HashMap::new(), more: Vec::new() })
    }
}

impl Default for RecraftProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for RecraftProvider {
    fn name(&self) -> &str {
        "recraft"
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

    /// @see <https://www.recraft.ai/docs/api-reference/endpoints.md> — request fields
    ///   (`prompt`, `model`, `style`/`style_id`, `n`, `size`, `negative_prompt`,
    ///   `response_format`, `controls`) and the `data[].url`/`data[].b64_json` response.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        _materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        if model.capabilities.background_removal {
            return self.remove_background(extras, _materialized).await;
        }
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id);
        let url = format!("{}/images/generations", self.api_base());
        let is_vector = native.contains("vector");

        let mut body = json!({
            "prompt": base.prompt,
            "model": native,
            "n": base.n.max(1),
            "response_format": "b64_json",
        });
        if let Some(size) = extras.size.as_deref() {
            body["size"] = Value::String(size.to_string());
        }
        // Recraft only accepts `style` (and `style_id`) on V2/V3 models. V4/V4.1
        // models reject it with a 400; for those, style selection is implicit in
        // the model id (e.g. the `_vector` suffix). So only forward style for
        // recraftv2/recraftv3* native ids.
        let supports_style = native.starts_with("recraftv2") || native.starts_with("recraftv3");
        if supports_style {
            if let Some(style) = extras.style.as_deref() {
                body["style"] = Value::String(style.to_string());
            }
        }
        if let Some(np) = base.negative_prompt.as_deref() {
            body["negative_prompt"] = Value::String(np.to_string());
        }
        // Recraft's seed parameter is `random_seed`, supported on all models.
        // @see <https://www.recraft.ai/docs/api-reference/endpoints.md>
        if let Some(seed) = base.seed {
            body["random_seed"] = Value::Number(seed.into());
        }
        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(obj) = body.as_object_mut() {
                for (k, v) in extra_map {
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
                message: format!("Recraft request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?;

        let status = resp.status();
        let data: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Recraft response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        if !status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Recraft API error: {data}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                retryable: status.as_u16() >= 500,
            });
        }

        let first = data["data"].get(0).ok_or_else(|| ProviderError::RequestFailed {
            message: "Recraft response missing data[0]".to_string(),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        })?;

        let bytes = if let Some(b64) = first["b64_json"].as_str() {
            B64.decode(b64).map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to decode Recraft b64_json: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
        } else if let Some(url) = first["url"].as_str() {
            // Fallback if the account forces url responses.
            let r = self.client.get(url).send().await.map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to fetch Recraft image: {e}"),
                status_code: None,
                provider_error: None,
                retryable: true,
            })?;
            r.bytes().await.map(|b| b.to_vec()).map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to read Recraft image: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
        } else {
            return Err(ProviderError::RequestFailed {
                message: "Recraft data[0] has neither b64_json nor url".to_string(),
                status_code: None,
                provider_error: Some(data.clone()),
                retryable: false,
            });
        };

        let mut metadata = HashMap::new();
        metadata.insert("model".to_string(), Value::String(native.to_string()));

        Ok(GenerationOutput {
            data: bytes,
            content_type: if is_vector { "image/svg+xml".to_string() } else { "image/png".to_string() },
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
                message: "Recraft provider not configured".into(),
                latency_ms: None,
            };
        }
        let start = std::time::Instant::now();
        let creds = match self.creds() {
            Ok(c) => c,
            Err(_) => return HealthCheckResult { healthy: false, message: "No API key".into(), latency_ms: None },
        };
        let builder = match crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client.get(format!("{}/users/me", self.api_base())).timeout(std::time::Duration::from_secs(10)),
        ) {
            Ok(b) => b,
            Err(e) => return HealthCheckResult { healthy: false, message: e.to_string(), latency_ms: None },
        };
        match builder.send().await {
            Ok(resp) if resp.status().is_success() => HealthCheckResult {
                healthy: true,
                message: "Recraft API token valid".into(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(resp) => HealthCheckResult {
                healthy: false,
                message: format!("Recraft returned HTTP {}", resp.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => HealthCheckResult {
                healthy: false,
                message: format!("Recraft health check failed: {e}"),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
        }
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

    fn make_provider(api_base: &str) -> RecraftProvider {
        let mut p = RecraftProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_key: "rc-token".to_string(),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        cfg.credentials.api_key = Some("rc-token".to_string());
        p.configure(cfg);
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
            size: Some("1024x1024".to_string()),
            aspect_ratio: None,
            quality: None,
            style: Some("digital_illustration".to_string()),
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".to_string(),
            edit: None, views: Vec::new(),
            extra: None,
            output_format: None, background: None,
        }
    }

    #[tokio::test]
    async fn generates_recraftv3_image() {
        let server = MockServer::start().await;
        let fake_png = b"RECRAFTPNG";
        let b64 = B64.encode(fake_png);

        Mock::given(method("POST"))
            .and(path("/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "created": 1700000000,
                "data": [{ "b64_json": b64 }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("recraft/recraftv3");
        let base = make_base("a friendly robot mascot", "recraft/recraftv3");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, fake_png);

        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].headers.get("authorization").unwrap(), "Bearer rc-token");
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["model"], "recraftv3");
        assert_eq!(body["prompt"], "a friendly robot mascot");
        assert_eq!(body["size"], "1024x1024");
        assert_eq!(body["style"], "digital_illustration");
        assert_eq!(body["response_format"], "b64_json");
    }

    /// Recraft's seed parameter is `random_seed`, not `seed`.
    /// @see <https://www.recraft.ai/docs/api-reference/endpoints.md>
    #[tokio::test]
    async fn seed_is_sent_as_random_seed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "b64_json": B64.encode(b"png") }]
            })))
            .mount(&server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("recraft/recraftv3");
        let mut base = make_base("a race car", "recraft/recraftv3");
        base.seed = Some(4242);

        provider
            .generate(
                &schema,
                &base,
                &make_extras(),
                &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() },
            )
            .await
            .expect("generate");

        let received = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["random_seed"], 4242);
        assert!(body.get("seed").is_none(), "Recraft has no `seed` field");
    }

    fn multipart_field(body: &[u8], name: &str) -> Option<String> {
        let s = String::from_utf8_lossy(body).into_owned();
        let marker = format!("name=\"{name}\"");
        let at = s.find(&marker)? + marker.len();
        let rest = &s[at..];
        let value = &rest[rest.find("\r\n\r\n")? + 4..];
        Some(value[..value.find("\r\n")?].to_string())
    }

    /// Remove background uploads the image as `file` and answers one `image`,
    /// not a `data` array; litegen asks for PNG (Recraft's default is WebP)
    /// as base64.
    /// @see <https://www.recraft.ai/docs/api-reference/tools/remove-background.md>
    #[tokio::test]
    async fn remove_background_uploads_the_file_and_reads_the_one_image() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/images/removeBackground"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "created": 1, "credits": 10, "image": { "image_id": "i", "b64_json": B64.encode(b"CUTOUT") }
            })))
            .mount(&server)
            .await;
        let materialized = MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".to_string(),
                form: crate::proxy::materializer::MaterializedRefForm::MultipartField {
                    field_name: "file".to_string(),
                    bytes: bytes::Bytes::from_static(b"src"),
                    content_type: "image/png".to_string(),
                },
                time_s: None,
            }],
            cleanup: Cleanup::empty(),
        };
        let extras = ImageExtras { size: None, style: None, ..make_extras() };
        let out = make_provider(&server.uri())
            .generate(&ref_schema("recraft/remove-background"), &make_base("", "recraft/remove-background"), &extras, &materialized)
            .await
            .expect("generate");
        assert_eq!(out.data, b"CUTOUT");
        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].headers.get("authorization").unwrap(), "Bearer rc-token");
        let body = &received[0].body;
        assert!(String::from_utf8_lossy(body).contains("name=\"file\""), "the file part");
        assert_eq!(multipart_field(body, "response_format").as_deref(), Some("b64_json"));
        assert_eq!(multipart_field(body, "image_format").as_deref(), Some("png"));
        for absent in ["prompt", "model", "n", "size", "style"] {
            assert!(multipart_field(body, absent).is_none(), "{absent} is not a removeBackground field");
        }
    }
}
