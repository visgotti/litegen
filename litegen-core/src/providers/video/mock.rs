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
use super::visual_mock::{generate_keyframe_blend_gif, generate_sprite_gif, generate_timed_keyframes_gif, generate_visual_video_gif, global_store};
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

/// The video operation mocks (3D doc §5).
const EXTEND_MODEL: &str = "mock/extend-video";
const RETAKE_MODEL: &str = "mock/retake-video";
const EDIT_MODEL: &str = "mock/edit-video";
/// Stays processing until cancelled: the cancel path, without a race.
const HELD_MODEL: &str = "mock/held-video";
/// Reports its video finished, with neither the bytes nor a URL: nothing
/// litegen can keep, so the generation fails at the first poll.
const EMPTY_MODEL: &str = "mock/empty-video";

/// A GIF's frames as the `gif` crate reads them — indexed, each with the
/// palette it was drawn with — so they can be written back exactly.
fn gif_frames(bytes: &[u8]) -> Result<(u16, u16, Vec<gif::Frame<'static>>), ProviderError> {
    let unreadable = |e: gif::DecodingError| ProviderError::InvalidRequest(format!("the mock reads GIF sources only: {e}"));
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::Indexed);
    let mut decoder = opts.read_info(std::io::Cursor::new(bytes)).map_err(unreadable)?;
    let (w, h) = (decoder.width(), decoder.height());
    let global = decoder.global_palette().map(<[u8]>::to_vec);
    let mut frames = Vec::new();
    while let Some(frame) = decoder.read_next_frame().map_err(unreadable)? {
        let mut frame = frame.clone();
        if frame.palette.is_none() {
            frame.palette = global.clone();
        }
        frames.push(frame);
    }
    Ok((w, h, frames))
}

/// `n` new full frames, each a solid colour no source frame of the mocks
/// has, `delay` hundredths of a second apiece.
fn generated_frames(w: u16, h: u16, n: usize, delay: u16) -> Vec<gif::Frame<'static>> {
    (0..n)
        .map(|i| {
            let shade = (40 + (i * 37) % 180) as u8;
            let mut rgba: Vec<u8> = std::iter::repeat_n([shade, 0x10, 0xd0, 0xff], usize::from(w) * usize::from(h)).flatten().collect();
            let mut frame = gif::Frame::from_rgba_speed(w, h, &mut rgba, 10);
            frame.delay = delay;
            frame
        })
        .collect()
}

fn write_gif(w: u16, h: u16, frames: &[gif::Frame<'static>]) -> Result<Vec<u8>, ProviderError> {
    let failed = |e: gif::EncodingError| ProviderError::InvalidRequest(format!("the mock could not write its clip: {e}"));
    let mut out = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut out, w, h, &[]).map_err(failed)?;
        encoder.set_repeat(gif::Repeat::Infinite).map_err(failed)?;
        for frame in frames {
            encoder.write_frame(frame).map_err(failed)?;
        }
    }
    Ok(out)
}

/// `mock/extend-video`, `mock/retake-video` and `mock/edit-video`: the
/// source's frames copied exactly where the plan keeps them, new frames where
/// it generates. An extend adds `duration_seconds` of frames after
/// (`forward`, the default) or before (`backward`) the source and returns the
/// whole clip; a retake regenerates every frame starting inside `time_range`;
/// an edit regenerates every frame.
fn operate_on_source(model_id: &str, extras: &VideoExtras) -> Result<Vec<u8>, ProviderError> {
    let source = extras.source.as_ref().ok_or_else(|| ProviderError::InvalidRequest(format!("{model_id} needs a source_video")))?;
    let (w, h, frames) = gif_frames(&source.bytes)?;
    let delay = frames.first().map_or(10, |f| f.delay.max(1));
    let out = if model_id == EDIT_MODEL {
        frames.iter().flat_map(|f| generated_frames(w, h, 1, f.delay)).collect()
    } else if model_id == EXTEND_MODEL {
        let added = generated_frames(w, h, (extras.duration_seconds * 100.0 / f64::from(delay)).round() as usize, delay);
        match extras.direction.as_deref() {
            Some("backward") => [added, frames].concat(),
            _ => [frames, added].concat(),
        }
    } else {
        let r = extras.time_range.ok_or_else(|| ProviderError::InvalidRequest(format!("{model_id} needs a time_range")))?;
        let mut t = 0.0;
        let mut out = Vec::with_capacity(frames.len());
        for frame in frames {
            let starts = t;
            t += f64::from(frame.delay) / 100.0;
            if starts + 1e-9 >= r.start_s && starts + 1e-9 < r.end_s {
                out.extend(generated_frames(w, h, 1, frame.delay));
            } else {
                out.push(frame);
            }
        }
        out
    };
    write_gif(w, h, &out)
}

fn should_render_visual_video(model_id: &str) -> bool {
    matches!(model_id,
        "mock/visual-video-gen" |
        "mock/keyframe-video" |
        "mock/passthrough-video" |
        "mock/strict-duration-video" |
        "mock/expensive-video" |
        "mock/priced-video" |
        "mock/sprite-video" |
        EXTEND_MODEL |
        RETAKE_MODEL |
        EDIT_MODEL
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
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationHandle, ProviderError> {
        if [EXTEND_MODEL, RETAKE_MODEL, EDIT_MODEL].contains(&model.id.as_str()) {
            let job_id = uuid::Uuid::new_v4().to_string();
            global_store().put(job_id.clone(), operate_on_source(&model.id, extras)?).await;
            return Ok(VideoGenerationHandle { provider_job_id: job_id, provider: "mock".to_string(), model: model.id.clone() });
        }
        if should_render_visual_video(&model.id) {
            let job_id = uuid::Uuid::new_v4().to_string();

            // Keyframe model with both first_frame + last_frame refs supplied:
            // cross-fade between the two real images so the dev sees that the
            // ref-image flow actually delivered bytes to the provider.
            let timed = materialized.refs.iter().any(|r| r.role == "keyframe");
            let gif_bytes = if model.id == "mock/sprite-video" {
                generate_sprite_gif(extras.duration_seconds, 10, extras.r#loop == Some(true))
            } else if model.id == "mock/keyframe-video" && timed {
                // Timed keyframes: first_frame is t = 0 and last_frame the end.
                let duration = extras.duration_seconds;
                let at = |r: &crate::proxy::materializer::MaterializedRef| match r.role.as_str() {
                    "first_frame" => Some(0.0),
                    "last_frame" => Some(duration),
                    "keyframe" => r.time_s,
                    _ => None,
                };
                let keyframes: Vec<(f64, Vec<u8>)> = materialized
                    .refs
                    .iter()
                    .filter_map(|r| match &r.form {
                        MaterializedRefForm::Base64(b64) => Some((at(r)?, B64.decode(b64).ok()?)),
                        _ => None,
                    })
                    .collect();
                generate_timed_keyframes_gif(&keyframes, duration, extras.fps.unwrap_or(10), &base.prompt)
            } else if model.id == "mock/keyframe-video" {
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
        if handle.model == EMPTY_MODEL {
            return Ok(VideoGenerationPollResult {
                status: GenerationStatus::Completed,
                progress: 100,
                video_url: None,
                video_data: None,
                content_type: None,
                error: None,
                metadata: Default::default(),
            });
        }
        if handle.model == HELD_MODEL {
            return Ok(VideoGenerationPollResult {
                status: GenerationStatus::Processing,
                progress: 50,
                video_url: None,
                video_data: None,
                content_type: None,
                error: None,
                metadata: HashMap::new(),
            });
        }
        if should_render_visual_video(&handle.model) {
            // Return a local URL pointing to the stashed GIF bytes, and the
            // bytes themselves, which litegen keeps (as with a vendor whose
            // result litegen downloads).
            let result_url = format!("/mock/video/{}", handle.provider_job_id);
            return Ok(VideoGenerationPollResult {
                status: GenerationStatus::Completed,
                progress: 100,
                video_url: Some(result_url),
                video_data: global_store().get(&handle.provider_job_id).await,
                content_type: Some("image/gif".into()),
                error: None,
                metadata: HashMap::from([
                    ("mock".to_string(), serde_json::json!(true)),
                    ("job_id".to_string(), serde_json::json!(handle.provider_job_id)),
                    ("visual".to_string(), serde_json::json!(true)),
                ]),
            });
        }
        // Always return completed with a placeholder: a tiny animated GIF,
        // inline, so litegen keeps a real file without any download.
        Ok(VideoGenerationPollResult {
            status: GenerationStatus::Completed,
            progress: 100,
            video_url: Some("https://example.com/mock-video.mp4".into()),
            video_data: Some(generate_visual_video_gif(&handle.model)),
            content_type: Some("image/gif".into()),
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

    /// A mock job is still running until a poll collects it, and nothing polls
    /// a cancelled one, so every cancel is taken.
    async fn cancel(&self, _handle: &VideoGenerationHandle) -> Result<crate::providers::CancelOutcome, ProviderError> {
        Ok(crate::providers::CancelOutcome::Requested)
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
            camera: None, audio: None, r#loop: None,
            source: None, direction: None, time_range: None, replace: None,
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
                    time_s: None,
                },
                MaterializedRef {
                    role: "last_frame".to_string(),
                    form: MaterializedRefForm::Base64(solid_png_b64(0, 0, 255)),
                    time_s: None,
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
    async fn the_held_mock_stays_in_flight_until_it_is_cancelled() {
        let provider = MockVideoProvider::new();
        let schema = ref_schema(HELD_MODEL);
        let base = crate::types::BaseGenerationRequest {
            prompt: "a held clip".into(), model: HELD_MODEL.into(), n: 1, negative_prompt: None, seed: None,
            reference_images: vec![], strict: true, extra: None, metadata: None,
        };
        let extras = VideoExtras {
            duration_seconds: 5.0, aspect_ratio: None, resolution: None, fps: None, extra: None, camera: None,
            audio: None, r#loop: None, source: None, direction: None, time_range: None, replace: None,
        };
        let materialized = MaterializedRequest { refs: vec![], cleanup: crate::proxy::materializer::Cleanup::empty() };
        let handle = provider.generate(&schema, &base, &extras, &materialized).await.unwrap();
        for _ in 0..3 {
            let poll = provider.poll_status(&handle).await.unwrap();
            assert_eq!((poll.status, poll.progress), (GenerationStatus::Processing, 50));
            assert!(poll.video_url.is_none() && poll.video_data.is_none());
        }
        assert_eq!(provider.cancel(&handle).await.unwrap(), crate::providers::CancelOutcome::Requested);
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
