use async_trait::async_trait;
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::MaterializedRequest;
use crate::providers::{
    BaseGenerationRequest, HealthCheckResult, ProviderError, ProviderInstanceConfig, VideoExtras,
    VideoGenerationHandle, VideoGenerationPollResult, VideoProvider, build_cost_estimate,
};
use crate::types::*;
use crate::proxy::materializer::MaterializedRefForm;
use super::visual_mock::{generate_keyframe_blend_gif, generate_visual_video_gif, global_store};
use base64::{Engine, engine::general_purpose::STANDARD as B64};

/// Pull the raw decoded bytes for a ref with the given role, regardless of
/// whether the materializer handed us base64 (decoded here) or a URL form
/// (the mock provider can't fetch URLs, so URL-form refs return None and the
/// caller falls back).
fn ref_bytes_for_role(materialized: &MaterializedRequest, role: &str) -> Option<Vec<u8>> {
    materialized.refs.iter()
        .find(|r| r.role == role)
        .and_then(|r| match &r.form {
            MaterializedRefForm::Base64(b64) => B64.decode(b64).ok(),
            _ => None,
        })
}

fn should_render_visual_video(model_id: &str) -> bool {
    matches!(model_id,
        "mock/visual-video-gen" |
        "mock/keyframe-video" |
        "mock/passthrough-video" |
        "mock/strict-duration-video" |
        "mock/expensive-video"
    )
}

/// Mock video provider for testing and development.
///
/// No external model API is called — `generate`/`poll_status` fabricate output
/// locally (a stored GIF, or a placeholder URL), so there is no provider API
/// reference to cite. The only external contract honoured is the GIF byte
/// format produced by the visual-mock helpers.
///
/// @see <https://www.w3.org/Graphics/GIF/spec-gif89a.txt> — GIF89a specification
/// @see <https://docs.rs/image> — `image` crate used to encode the frames
pub struct MockVideoProvider {
    configured: bool,
}

impl MockVideoProvider {
    pub fn new() -> Self {
        Self { configured: false }
    }
}

impl Default for MockVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VideoProvider for MockVideoProvider {
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
        _extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        if should_render_visual_video(&model.id) {
            let job_id = uuid::Uuid::new_v4().to_string();

            // Keyframe model with both first_frame + last_frame refs supplied:
            // cross-fade between the two real images so the dev sees that the
            // ref-image flow actually delivered bytes to the provider.
            let gif_bytes = if model.id == "mock/keyframe-video" {
                let first = ref_bytes_for_role(materialized, "first_frame");
                let last = ref_bytes_for_role(materialized, "last_frame");
                match (first, last) {
                    (Some(a), Some(b)) => generate_keyframe_blend_gif(&a, &b, &base.prompt),
                    _ => generate_visual_video_gif(&base.prompt),
                }
            } else {
                generate_visual_video_gif(&base.prompt)
            };

            global_store().put(job_id.clone(), gif_bytes).await;
            return Ok(VideoGenerationHandle {
                provider_job_id: job_id,
                provider: "mock".to_string(),
                model: model.id.clone(),
            });
        }
        Ok(VideoGenerationHandle {
            provider_job_id: "mock-video-job-1".to_string(),
            provider: "mock".to_string(),
            model: model.id.clone(),
        })
    }

    async fn poll_status(
        &self,
        handle: &VideoGenerationHandle,
    ) -> Result<VideoGenerationPollResult, ProviderError> {
        if should_render_visual_video(&handle.model) {
            // Return a local URL pointing to the stashed GIF bytes.
            let result_url = format!("/mock/video/{}", handle.provider_job_id);
            return Ok(VideoGenerationPollResult {
                status: GenerationStatus::Completed,
                progress: 100,
                video_url: Some(result_url),
                video_data: None,
                content_type: Some("image/gif".into()),
                error: None,
                metadata: HashMap::from([
                    ("mock".to_string(), serde_json::json!(true)),
                    ("job_id".to_string(), serde_json::json!(handle.provider_job_id)),
                    ("visual".to_string(), serde_json::json!(true)),
                ]),
            });
        }
        // Always return completed with a placeholder
        Ok(VideoGenerationPollResult {
            status: GenerationStatus::Completed,
            progress: 100,
            video_url: Some("https://example.com/mock-video.mp4".into()),
            video_data: None,
            content_type: Some("video/mp4".into()),
            error: None,
            metadata: HashMap::from([
                ("mock".to_string(), serde_json::json!(true)),
                ("job_id".to_string(), serde_json::json!(handle.provider_job_id)),
            ]),
        })
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &VideoGenerationRequest,
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
            healthy: true,
            message: "Mock video provider always healthy".into(),
            latency_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderInstanceConfig;

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

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

    fn make_extras() -> VideoExtras {
        VideoExtras {
            duration_seconds: 5.0,
            aspect_ratio: None,
            resolution: None,
            fps: None,
            extra: None,
        }
    }

    #[tokio::test]
    async fn visual_video_gen_returns_gif_url() {
        let schema = ref_schema("mock/visual-video-gen");
        let mut provider = MockVideoProvider::new();
        provider.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: String::new(),
            api_keys: vec![],
            api_base: None,
            model_mapping: Default::default(),
            extra_headers: Default::default(),
            options: None,
        });

        let base = make_base("a colorful sunset timelapse", "mock/visual-video-gen");
        let extras = make_extras();
        let materialized = empty_materialized();

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        assert_eq!(handle.provider, "mock");
        assert_eq!(handle.model, "mock/visual-video-gen");

        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.progress, 100);

        let url = poll.video_url.expect("expected video_url");
        assert!(url.contains("/mock/video/"), "url should contain /mock/video/, got: {}", url);
        assert_eq!(poll.content_type.as_deref(), Some("image/gif"));

        // Verify the GIF bytes are actually stored in the global store
        let job_id = url.trim_start_matches("/mock/video/");
        let stored = crate::providers::video::visual_mock::global_store().get(job_id).await;
        assert!(stored.is_some(), "GIF bytes must be in the store");
        let bytes = stored.unwrap();
        assert_eq!(&bytes[..6], b"GIF89a", "stored bytes must start with GIF89a magic");
        assert!(bytes.len() > 500, "expected real GIF > 500 bytes, got {}", bytes.len());
    }

    #[tokio::test]
    async fn keyframe_video_blends_first_and_last_frames() {
        use crate::proxy::materializer::{MaterializedRef, MaterializedRefForm, MaterializedRequest};
        use image::{ImageBuffer, Rgba};

        let schema = ref_schema("mock/keyframe-video");
        let mut provider = MockVideoProvider::new();
        provider.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: String::new(),
            api_keys: vec![],
            api_base: None,
            model_mapping: Default::default(),
            extra_headers: Default::default(),
            options: None,
        });

        // Build two distinct solid PNGs as base64 → simulates the materializer
        // having handed us decoded ref bytes for the form: base64 ref_inputs.
        fn solid_png_b64(r: u8, g: u8, b: u8) -> String {
            let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
                ImageBuffer::from_pixel(64, 64, Rgba([r, g, b, 255]));
            let mut bytes = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
            B64.encode(&bytes)
        }

        let materialized = MaterializedRequest {
            refs: vec![
                MaterializedRef {
                    role: "first_frame".to_string(),
                    form: MaterializedRefForm::Base64(solid_png_b64(255, 0, 0)),
                },
                MaterializedRef {
                    role: "last_frame".to_string(),
                    form: MaterializedRefForm::Base64(solid_png_b64(0, 0, 255)),
                },
            ],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };

        let base = make_base("blend red to blue", "mock/keyframe-video");
        let extras = make_extras();

        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        let poll = provider.poll_status(&handle).await.unwrap();
        let url = poll.video_url.expect("video_url");
        let job_id = url.trim_start_matches("/mock/video/");
        let stored = crate::providers::video::visual_mock::global_store()
            .get(job_id).await.expect("stored bytes");

        assert_eq!(&stored[..6], b"GIF89a");
        // 10-frame 256x256 blend is meaningfully larger than the 8-frame
        // random-pattern fallback (4-byte placeholder dims).
        assert!(stored.len() > 2000,
            "expected real 10-frame blend GIF > 2000 bytes, got {}", stored.len());

        // Compare against the no-refs fallback for the same prompt — they must differ.
        let no_refs = MaterializedRequest {
            refs: vec![],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };
        let handle2 = provider.generate(&schema, &base, &extras, &no_refs).await.unwrap();
        let stored2 = crate::providers::video::visual_mock::global_store()
            .get(&handle2.provider_job_id).await.expect("stored bytes");
        assert_ne!(stored, stored2,
            "keyframe-blend output must differ from prompt-only fallback");
    }

    #[tokio::test]
    async fn generate_returns_handle_and_poll_returns_completed() {
        let schema = ref_schema("mock/video-gen");
        let mut provider = MockVideoProvider::new();
        provider.configure(ProviderInstanceConfig {
            credentials: Default::default(),
            api_key: String::new(),
            api_keys: vec![],
            api_base: None,
            model_mapping: Default::default(),
            extra_headers: Default::default(),
            options: None,
        });

        let base = make_base("a cinematic timelapse", "mock/video-gen");
        let extras = make_extras();
        let materialized = empty_materialized();

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        let handle = result.unwrap();

        assert_eq!(handle.model, "mock/video-gen");
        assert_eq!(handle.provider, "mock");

        // Poll and assert Completed
        let poll = provider.poll_status(&handle).await.unwrap();
        assert_eq!(poll.status, GenerationStatus::Completed);
        assert_eq!(poll.progress, 100);
        assert!(poll.video_url.is_some());
    }
}
