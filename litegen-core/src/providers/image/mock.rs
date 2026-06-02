use async_trait::async_trait;
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::MaterializedRequest;
use crate::providers::{
    BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras, ImageProvider,
    ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;
use super::visual_mock::generate_visual_image_png;

fn should_render_visual_image(model_id: &str) -> bool {
    matches!(model_id,
        "mock/visual-image-gen" |
        "mock/all-params-image" |
        "mock/freeform-size-image" |
        "mock/url-refs-image" |
        "mock/base64-refs-image" |
        "mock/multipart-refs-image" |
        "mock/inpainting-image" |
        "mock/passthrough-image" |
        "mock/expensive-image"
    )
}

/// Mock image provider for testing. Returns a 1x1 PNG placeholder.
///
/// No external model API is called — output is fabricated locally, so there is
/// no provider API reference to cite. The only external contract it honours is
/// the PNG byte format of the image it returns.
///
/// @see <https://www.w3.org/TR/png/> — PNG specification (the `placeholder_png` bytes)
/// @see <https://docs.rs/image> — `image` crate, used by the visual-mock variant
pub struct MockProvider {
    configured: bool,
}

impl MockProvider {
    pub fn new() -> Self {
        Self { configured: false }
    }

    /// Minimal valid 1x1 transparent PNG (signature + IHDR + IDAT + IEND chunks).
    ///
    /// @see <https://www.w3.org/TR/png/#5DataRep> — PNG chunk layout these literal bytes follow
    pub fn placeholder_png() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
            0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, // RGBA
            0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, // IDAT chunk
            0x78, 0x9C, 0x62, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE5,
            0x27, 0xDE, 0xFC, // compressed data
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // IEND chunk
            0xAE, 0x42, 0x60, 0x82,
        ]
    }
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn configure(&mut self, _config: ProviderInstanceConfig) {
        self.configured = true;
    }

    fn is_configured(&self) -> bool {
        self.configured
    }

    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        _extras: &ImageExtras,
        _materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let mut metadata = HashMap::new();
        metadata.insert("prompt".to_string(), serde_json::Value::String(base.prompt.clone()));
        metadata.insert("provider".to_string(), serde_json::Value::String("mock".to_string()));

        let data = if should_render_visual_image(&model.id) {
            generate_visual_image_png(&base.prompt)
        } else {
            Self::placeholder_png()
        };

        Ok(GenerationOutput {
            data,
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
            None,
        ))
    }

    async fn health_check(&self) -> HealthCheckResult {
        HealthCheckResult {
            healthy: self.configured,
            message: if self.configured {
                "Mock provider ready".into()
            } else {
                "Mock provider not configured".into()
            },
            latency_ms: Some(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderInstanceConfig;

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
            aspect_ratio: None,
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
    async fn generate_returns_placeholder_png() {
        let schema = ref_schema("mock/image-gen");
        let mut provider = MockProvider::new();
        provider.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: String::new(),
            api_keys: vec![],
            api_base: None,
            model_mapping: Default::default(),
            extra_headers: Default::default(),
            options: None,
        });

        let base = make_base("a beautiful landscape", "mock/image-gen");
        let extras = make_extras();
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok());
        let output = result.unwrap();

        // Must return the deterministic PNG bytes
        assert_eq!(output.data, MockProvider::placeholder_png());
        assert_eq!(output.content_type, "image/png");

        // Metadata should contain the prompt
        assert_eq!(
            output.metadata.get("prompt").and_then(|v| v.as_str()),
            Some("a beautiful landscape")
        );
    }

    #[tokio::test]
    async fn visual_image_gen_returns_real_png() {
        let schema = ref_schema("mock/visual-image-gen");
        let mut provider = MockProvider::new();
        provider.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: String::new(),
            api_keys: vec![],
            api_base: None,
            model_mapping: Default::default(),
            extra_headers: Default::default(),
            options: None,
        });

        let base = make_base("a colorful gradient sky", "mock/visual-image-gen");
        let extras = make_extras();
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let output = result.unwrap();

        // Must start with PNG magic bytes
        assert_eq!(
            &output.data[..8],
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            "output must start with PNG magic"
        );
        // Must be a real image (not the 4-byte placeholder)
        assert!(
            output.data.len() > 1000,
            "expected real PNG > 1000 bytes, got {}",
            output.data.len()
        );
        assert_eq!(output.content_type, "image/png");
    }

    #[tokio::test]
    async fn estimate_cost_uses_model_base_cost() {
        let schema = ref_schema("mock/image-gen");
        let provider = MockProvider::new();
        let req = ImageGenerationRequest {
            base: make_base("hi", "mock/image-gen"),
            size: None,
            aspect_ratio: None,
            quality: None,
            style: None,
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".to_string(),
        };
        let cost = provider.estimate_cost(&schema, &req).await.unwrap();
        // mock/test-model has base_cost_usd: 0.0
        assert_eq!(cost.base_cost_usd, 0.0);
    }
}
