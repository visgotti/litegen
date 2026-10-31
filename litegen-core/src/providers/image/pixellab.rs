use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::MaterializedRequest;
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras, ImageProvider, ProviderError,
    ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// PixelLab v2: native pixel art (PixelGenie L16). `create-image-pixflux`
/// answers at once with the image at native size; a forced palette goes as
/// `color_image`, an image of the colours.
///
/// @see <https://api.pixellab.ai/v2/llms.txt> — base URL, `Authorization: Bearer`, the endpoint list
/// @see <https://api.pixellab.ai/v2/openapi.json> — `create-image-pixflux`: `description`, `image_size`
///   (16–400 a side), `color_image` ("Forced color palette, image containing colors used for palette"),
///   `no_background`, `seed`, `text_guidance_scale`; answers `{usage, image: {type, base64, format}}`
pub struct PixelLabProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
}

impl PixelLabProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
            client: Client::builder().timeout(std::time::Duration::from_secs(110)).build().expect("failed to build HTTP client"),
        }
    }

    fn api_base(&self) -> &str {
        self.config.as_ref().and_then(|c| c.api_base.as_deref()).unwrap_or("https://api.pixellab.ai/v2")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("pixellab".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// A palette as PixelLab's `color_image`: a strip one pixel per colour.
    fn palette_image(palette: &[String]) -> Result<Value, ProviderError> {
        let colours: Vec<[u8; 4]> = palette
            .iter()
            .filter_map(|c| u32::from_str_radix(c.trim_start_matches('#'), 16).ok())
            .map(|v| [(v >> 16) as u8, (v >> 8) as u8, v as u8, 0xff])
            .collect();
        let strip = image::RgbaImage::from_fn(colours.len().max(1) as u32, 1, |x, _| image::Rgba(colours.get(x as usize).copied().unwrap_or([0, 0, 0, 0xff])));
        let mut png = Vec::new();
        strip
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| ProviderError::InvalidRequest(format!("the palette could not be drawn: {e}")))?;
        Ok(json!({ "type": "base64", "base64": B64.encode(png), "format": "png" }))
    }
}

impl Default for PixelLabProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for PixelLabProvider {
    fn name(&self) -> &str {
        "pixellab"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.api_keys.is_empty() {
            self.key_pool = Some(ApiKeyPool::shared(config.api_keys.clone()));
        }
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config.as_ref().is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.key_pool.is_some())
    }

    /// `POST /create-image-pixflux`: `extra`'s style hints first, then the
    /// fields litegen owns, so `extra` cannot replace them.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        _materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        if model.id != "pixellab/pixflux" {
            return Err(ProviderError::ModelNotSupported { model: model.id.clone(), provider: "pixellab".into() });
        }
        let creds = self.creds()?;
        let (width, height) = extras
            .size
            .as_deref()
            .and_then(|s| s.split_once('x'))
            .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
            .unwrap_or((64, 64));
        let mut body = match &extras.extra {
            Some(Value::Object(extra)) => Value::Object(extra.clone()),
            _ => json!({}),
        };
        body["description"] = json!(base.prompt);
        body["image_size"] = json!({ "width": width, "height": height });
        if let Some(palette) = extras.palette.as_deref() {
            body["color_image"] = Self::palette_image(palette)?;
        }
        if extras.background.as_deref() == Some("transparent") {
            body["no_background"] = json!(true);
        }
        if let Some(seed) = base.seed {
            body["seed"] = json!(seed);
        }
        if let Some(g) = extras.guidance_scale {
            body["text_guidance_scale"] = json!(g);
        }
        let url = format!("{}/create-image-pixflux", self.api_base());
        let builder = crate::providers::auth::apply(&self.auth, &creds, self.client.post(&url).json(&body))?;
        let resp = crate::providers::inject_trace_headers(builder).send().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("PixelLab request failed: {e}"),
            status_code: e.status().map(|s| s.as_u16()),
            provider_error: None,
            retryable: e.is_timeout() || e.is_connect(),
        })?;
        let status = resp.status();
        let answer: Value = resp.json().await.map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse PixelLab response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;
        if !status.is_success() {
            let detail = answer["detail"]
                .as_str()
                .map(str::to_string)
                .or_else(|| answer["detail"][0]["msg"].as_str().map(str::to_string))
                .unwrap_or_else(|| answer.to_string());
            return Err(ProviderError::RequestFailed {
                message: format!("PixelLab API error: {detail}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(answer),
                retryable: status.as_u16() >= 500 || status.as_u16() == 429,
            });
        }
        let b64 = answer["image"]["base64"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "PixelLab response has no image".into(),
            status_code: None,
            provider_error: Some(answer.clone()),
            retryable: false,
        })?;
        let data = B64.decode(b64).map_err(|e| ProviderError::RequestFailed {
            message: format!("PixelLab image is not base64: {e}"),
            status_code: None,
            provider_error: None,
            retryable: false,
        })?;
        let mut metadata = HashMap::new();
        if let Some(usd) = answer["usage"]["usd"].as_f64() {
            metadata.insert("vendor_usd".to_string(), json!(usd));
        }
        Ok(GenerationOutput { data, content_type: "image/png".into(), metadata, more: Vec::new() })
    }

    async fn estimate_cost(&self, model: &ModelSchema, _request: &ImageGenerationRequest) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, Some(json!({ "model": model.id }))))
    }

    async fn health_check(&self) -> HealthCheckResult {
        HealthCheckResult { healthy: self.is_configured(), message: "PixelLab provider".into(), latency_ms: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load").get(id).expect("model").clone()
    }

    fn provider(api_base: &str) -> PixelLabProvider {
        let mut p = PixelLabProvider::new();
        let mut cfg = ProviderInstanceConfig { api_key: "pl-key".into(), api_base: Some(api_base.into()), ..Default::default() };
        cfg.credentials.api_key = Some("pl-key".into());
        p.configure(cfg);
        p
    }

    fn base(prompt: &str) -> BaseGenerationRequest {
        serde_json::from_value(json!({ "model": "pixellab/pixflux", "prompt": prompt, "seed": 7 })).unwrap()
    }

    fn extras() -> ImageExtras {
        ImageExtras {
            vendor_job: Default::default(),
            expand: None, layout: None, mask_prompt: None, preserve_unmasked: None, feather_px: None,
            palette: Some(vec!["#081820".into(), "#346856".into(), "#88c070".into(), "#e0f8d0".into()]),
            directions: None,
            size: Some("64x48".into()),
            aspect_ratio: None, quality: None, style: None, steps: None,
            guidance_scale: Some(9.5),
            strength: None,
            response_format: "b64_json".into(),
            edit: None, views: Vec::new(),
            extra: Some(json!({ "outline": "single color black outline" })),
            output_format: None,
            background: Some("transparent".into()),
        }
    }

    /// The palette goes as `color_image`, a PNG of its colours; transparency
    /// as `no_background`; the image comes back at native size.
    /// @see <https://api.pixellab.ai/v2/openapi.json> — `create-image-pixflux`
    #[tokio::test]
    async fn pixflux_takes_the_palette_as_a_colour_image_and_answers_native_pixels() {
        let server = MockServer::start().await;
        let art = {
            let mut out = Vec::new();
            image::RgbaImage::from_pixel(64, 48, image::Rgba([8, 24, 32, 255])).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
            out
        };
        Mock::given(method("POST"))
            .and(path("/create-image-pixflux"))
            .and(header("authorization", "Bearer pl-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "usage": { "type": "usd", "usd": 0.009 }, "image": { "type": "base64", "base64": B64.encode(&art), "format": "png" } })))
            .mount(&server)
            .await;
        let out = provider(&server.uri())
            .generate(&ref_schema("pixellab/pixflux"), &base("a knight"), &extras(), &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() })
            .await
            .expect("generate");
        assert_eq!(out.data, art);
        let body: Value = serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(
            (&body["description"], &body["image_size"], &body["no_background"], &body["seed"], &body["text_guidance_scale"], &body["outline"]),
            (&json!("a knight"), &json!({ "width": 64, "height": 48 }), &json!(true), &json!(7), &json!(9.5), &json!("single color black outline"))
        );
        assert_eq!(body["color_image"]["type"], "base64");
        let swatch = image::load_from_memory(&B64.decode(body["color_image"]["base64"].as_str().unwrap()).unwrap()).unwrap().to_rgba8();
        let colours: Vec<[u8; 4]> = swatch.pixels().map(|p| p.0).collect();
        assert_eq!(colours, [[0x08, 0x18, 0x20, 255], [0x34, 0x68, 0x56, 255], [0x88, 0xc0, 0x70, 255], [0xe0, 0xf8, 0xd0, 255]], "one pixel per colour");
    }

    #[tokio::test]
    async fn a_refusal_is_pixellabs_own_answer() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/create-image-pixflux"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({ "detail": [{ "msg": "image_size.width must be at most 400" }] })))
            .mount(&server)
            .await;
        let err = provider(&server.uri())
            .generate(&ref_schema("pixellab/pixflux"), &base("x"), &extras(), &MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() })
            .await
            .unwrap_err();
        assert!(matches!(&err, ProviderError::RequestFailed { status_code: Some(422), retryable: false, .. }), "{err:?}");
        assert!(err.to_string().contains("at most 400"), "{err}");
    }
}
