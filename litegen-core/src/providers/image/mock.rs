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

/// The model that answers slower than any vendor timeout litegen waits for.
const SLOW_MODEL: &str = "mock/slow-image";

/// How long `mock/slow-image` takes: longer than any test's vendor timeout.
const SLOW_MODEL_DELAY: std::time::Duration = std::time::Duration::from_secs(30);

static SLOW_IMAGE_CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// How many times `mock/slow-image` has been called in this process: one per
/// vendor attempt, which is what PixelGenie L1 counts. A timed-out call is
/// never re-sent, so each request that reaches it adds exactly one.
pub fn slow_image_calls() -> u32 {
    SLOW_IMAGE_CALLS.load(std::sync::atomic::Ordering::SeqCst)
}

/// The edit mocks (3D doc §4): `mock/inpainting-image` takes a white-edits
/// mask, `mock/region-image` a box and a black-edits mask.
const INPAINTING_MODEL: &str = "mock/inpainting-image";
const REGION_MODEL: &str = "mock/region-image";

/// What an edit mock paints over its edit area.
const EDIT_FILL: [u8; 4] = [0xff, 0xcd, 0x75, 0xff];

static LAST_REGION_WINDOW: std::sync::Mutex<Option<Region>> = std::sync::Mutex::new(None);

/// The model that takes references only as URLs it can fetch (L4).
const URL_ONLY_MODEL: &str = "mock/url-only-image";

static LAST_REF_URL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The reference URL `mock/url-only-image` was last handed — how a test sees
/// what litegen published.
pub fn last_ref_url() -> Option<String> {
    LAST_REF_URL.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// The box `mock/region-image` was last sent — how a test sees the window
/// litegen chose.
pub fn last_region_window() -> Option<Region> {
    *LAST_REGION_WINDOW.lock().unwrap_or_else(|p| p.into_inner())
}

/// An edit mock's answer: the init image with exactly the edit area painted
/// `EDIT_FILL`, every other pixel as it came. `None` when there is no init
/// image to edit (the request is text-to-image).
fn fill_edit_area(model_id: &str, extras: &ImageExtras, materialized: &MaterializedRequest) -> Result<Option<Vec<u8>>, ProviderError> {
    use crate::proxy::materializer::MaterializedRefForm;
    use base64::Engine;
    let bytes_of = |role: &str| -> Option<Vec<u8>> {
        materialized.refs.iter().find(|r| r.role == role).and_then(|r| match &r.form {
            MaterializedRefForm::MultipartField { bytes, .. } => Some(bytes.to_vec()),
            MaterializedRefForm::Base64(b) => base64::engine::general_purpose::STANDARD.decode(b).ok(),
            MaterializedRefForm::Url(_) => None,
        })
    };
    let Some(init) = bytes_of("init") else { return Ok(None) };
    let mut img = image::load_from_memory(&init)
        .map_err(|e| ProviderError::InvalidRequest(format!("the mock cannot read the init image: {e}")))?
        .to_rgba8();
    let window = extras.edit.as_ref().and_then(|e| e.window);
    if model_id == REGION_MODEL {
        *LAST_REGION_WINDOW.lock().unwrap_or_else(|p| p.into_inner()) = window;
    }
    let mask = match bytes_of("mask") {
        Some(m) => Some(
            image::load_from_memory(&m)
                .map_err(|e| ProviderError::InvalidRequest(format!("the mock cannot read the mask: {e}")))?
                .to_luma8(),
        ),
        None => None,
    };
    let edits = |x: u32, y: u32| -> bool {
        match (&mask, window) {
            // The model's own convention: white edits here, black there.
            (Some(m), _) if model_id == REGION_MODEL => m.get_pixel(x, y)[0] < 128,
            (Some(m), _) => m.get_pixel(x, y)[0] >= 128,
            (None, Some(w)) => x >= w.x && x < w.x + w.w && y >= w.y && y < w.y + w.h,
            (None, None) => false,
        }
    };
    let (w, h) = img.dimensions();
    for y in 0..h {
        for x in 0..w {
            if edits(x, y) {
                img.put_pixel(x, y, image::Rgba(EDIT_FILL));
            }
        }
    }
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| ProviderError::InvalidRequest(format!("the mock could not encode its result: {e}")))?;
    Ok(Some(out))
}

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
        if model.id == SLOW_MODEL {
            SLOW_IMAGE_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(SLOW_MODEL_DELAY).await;
        }
        let mut metadata = HashMap::new();
        metadata.insert("prompt".to_string(), serde_json::Value::String(base.prompt.clone()));
        metadata.insert("provider".to_string(), serde_json::Value::String("mock".to_string()));

        if model.id == URL_ONLY_MODEL {
            let url = _materialized.refs.iter().find_map(|r| match &r.form {
                crate::proxy::materializer::MaterializedRefForm::Url(u) => Some(u.clone()),
                _ => None,
            });
            if url.as_deref().is_some_and(|u| u.starts_with("data:")) {
                return Err(ProviderError::InvalidRequest("mock/url-only-image takes no data: URIs".into()));
            }
            *LAST_REF_URL.lock().unwrap_or_else(|p| p.into_inner()) = url;
        }
        let edited = if model.id == INPAINTING_MODEL || model.id == REGION_MODEL {
            fill_edit_area(&model.id, _extras, _materialized)?
        } else {
            None
        };
        let data = if let Some(edited) = edited {
            edited
        } else if should_render_visual_image(&model.id) {
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
            vendor_job: Default::default(),
            size: None,
            aspect_ratio: None,
            quality: None,
            style: None,
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".to_string(),
            edit: None,
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
            region: None, mask_grow_px: None,
        };
        let cost = provider.estimate_cost(&schema, &req).await.unwrap();
        // mock/test-model has base_cost_usd: 0.0
        assert_eq!(cost.base_cost_usd, 0.0);
    }
}
