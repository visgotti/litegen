use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
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

/// Ideogram 3.0 image generation provider.
///
/// Synchronous: `POST /v1/ideogram-v3/generate` returns `{created, data: [{url,
/// seed, resolution, is_image_safe, style_type}]}`. Auth via the `Api-Key`
/// header (NOT Bearer). Reference images (style/character) are multipart file
/// uploads, so requests with refs use `multipart/form-data`; prompt-only
/// requests use JSON. Image-only. Result URLs expire, so bytes are fetched
/// immediately.
///
/// @see <https://developer.ideogram.ai/api-reference/api-reference/generate-v3>
///   Verbatim: "The model version to use for describing images. Defaults to V_3."
/// @see <https://developer.ideogram.ai/llms-full.txt>
///   Verbatim: "curl -X POST https://api.ideogram.ai/v1/ideogram-v3/generate \
///     -H \"Api-Key: <apiKey>\" -H \"Content-Type: application/json\" \
///     -d '{\"prompt\": \"A picture of a cat\"}'"
pub struct IdeogramProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl IdeogramProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::raw_header("Api-Key"),
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
            .unwrap_or("https://api.ideogram.ai")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("ideogram".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// Map model id → optional rendering_speed (TURBO/QUALITY/DEFAULT).
    fn rendering_speed(model_id: &str) -> Option<&'static str> {
        match model_id.strip_prefix("ideogram/").unwrap_or(model_id) {
            "ideogram-v3-turbo" => Some("TURBO"),
            "ideogram-v3-quality" => Some("QUALITY"),
            _ => None,
        }
    }

    /// Ideogram uses `WxH` aspect-ratio tokens (e.g. "16x9"), not "16:9".
    fn ideogram_aspect(ar: &str) -> String {
        ar.replace(':', "x")
    }

    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let resp = self.client.get(url).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to fetch Ideogram image: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: true,
        })?;
        if !resp.status().is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Ideogram image URL returned HTTP {}", resp.status()),
                status_code: Some(resp.status().as_u16()),
                provider_error: None,
                retryable: false,
            });
        }
        Ok(resp
            .bytes()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Failed to read Ideogram image bytes: {e}"),
                status_code: None,
                provider_error: None,
                retryable: false,
            })?
            .to_vec())
    }
}

impl Default for IdeogramProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for IdeogramProvider {
    fn name(&self) -> &str {
        "ideogram"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.api_keys.is_empty() {
            self.key_pool = Some(ApiKeyPool::new(config.api_keys.clone()));
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
        let url = format!("{}/v1/ideogram-v3/generate", self.api_base());
        let speed = Self::rendering_speed(&model.id);

        // Collect the common string fields once (shared by JSON + multipart).
        let mut fields: Vec<(&str, String)> = vec![("prompt", base.prompt.clone())];
        if let Some(ar) = extras.aspect_ratio.as_deref() {
            fields.push(("aspect_ratio", Self::ideogram_aspect(ar)));
        }
        if let Some(size) = extras.size.as_deref() {
            fields.push(("resolution", size.to_string()));
        }
        if let Some(s) = speed {
            fields.push(("rendering_speed", s.to_string()));
        }
        if let Some(np) = base.negative_prompt.as_deref() {
            fields.push(("negative_prompt", np.to_string()));
        }
        if base.n > 1 {
            fields.push(("num_images", base.n.to_string()));
        }
        if let Some(seed) = base.seed {
            fields.push(("seed", seed.to_string()));
        }
        if let Some(style) = extras.style.as_deref() {
            fields.push(("style_type", style.to_string()));
        }

        let has_refs = materialized
            .refs
            .iter()
            .any(|r| matches!(r.form, MaterializedRefForm::MultipartField { .. }));

        let base_builder = self.client.post(&url);
        let builder = if has_refs {
            // multipart/form-data: text fields + reference-image file parts.
            let mut form = Form::new();
            for (k, v) in &fields {
                form = form.text(k.to_string(), v.clone());
            }
            // extra string params pass through as text fields.
            if let Some(Value::Object(map)) = &extras.extra {
                for (k, v) in map {
                    if let Some(s) = v.as_str() {
                        form = form.text(k.clone(), s.to_string());
                    }
                }
            }
            for r in &materialized.refs {
                if let MaterializedRefForm::MultipartField { field_name, bytes, content_type } = &r.form {
                    let part = Part::bytes(bytes.to_vec())
                        .file_name("ref")
                        .mime_str(content_type)
                        .map_err(|e| ProviderError::InvalidRequest(format!("ideogram ref mime: {e}")))?;
                    form = form.part(field_name.clone(), part);
                }
            }
            base_builder.multipart(form)
        } else {
            // JSON body for prompt-only requests.
            let mut body = serde_json::Map::new();
            for (k, v) in &fields {
                // num_images/seed are numeric in JSON.
                let val = match *k {
                    "num_images" | "seed" => v.parse::<i64>().map(Value::from).unwrap_or(Value::String(v.clone())),
                    _ => Value::String(v.clone()),
                };
                body.insert(k.to_string(), val);
            }
            if let Some(Value::Object(map)) = &extras.extra {
                for (k, v) in map {
                    body.insert(k.clone(), v.clone());
                }
            }
            base_builder.header("Content-Type", "application/json").json(&Value::Object(body))
        };

        let builder = crate::providers::auth::apply(&self.auth, &creds, builder)?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Ideogram request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                retryable: e.is_timeout() || e.is_connect(),
            })?;

        let status = resp.status();
        let data: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Ideogram response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        if !status.is_success() {
            return Err(ProviderError::RequestFailed {
                message: format!("Ideogram API error: {data}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(data),
                retryable: status.as_u16() >= 500,
            });
        }

        let image_url = data["data"][0]["url"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Ideogram response missing data[0].url".to_string(),
            status_code: None,
            provider_error: Some(data.clone()),
            retryable: false,
        })?;

        let bytes = self.fetch_image_bytes(image_url).await?;
        let mut metadata = HashMap::new();
        metadata.insert("url".to_string(), Value::String(image_url.to_string()));
        if let Some(seed) = data["data"][0]["seed"].as_i64() {
            metadata.insert("seed".to_string(), Value::Number(seed.into()));
        }

        Ok(GenerationOutput {
            data: bytes,
            content_type: "image/png".to_string(),
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

    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Ideogram provider not configured".into(),
                latency_ms: None,
            };
        }
        HealthCheckResult {
            healthy: true,
            message: "Ideogram provider configured".into(),
            latency_ms: None,
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

    fn make_provider(api_base: &str) -> IdeogramProvider {
        let mut p = IdeogramProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_key: " id-key".to_string(),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        cfg.credentials.api_key = Some("id-key".to_string());
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
            size: None,
            aspect_ratio: Some("16:9".to_string()),
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
    async fn generates_ideogram_v3_json() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/img.png", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/v1/ideogram-v3/generate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "created": "2026-05-30T00:00:00Z",
                "data": [{ "url": image_url.clone(), "seed": 42, "is_image_safe": true, "resolution": "1344x768" }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"IDEOGRAMPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("ideogram/ideogram-v3");
        let base = make_base("a vintage travel poster of Mars", "ideogram/ideogram-v3");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"IDEOGRAMPNG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("api-key").unwrap(), "id-key");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["prompt"], "a vintage travel poster of Mars");
        assert_eq!(body["aspect_ratio"], "16x9"); // ':' -> 'x'
    }
}
