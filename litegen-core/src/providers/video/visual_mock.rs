use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, ImageBuffer, Rgba};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::OnceLock;
use tokio::sync::RwLock;

// ─── In-memory GIF store ─────────────────────────────────────────────────────

const MAX_ENTRIES: usize = 100;

/// Simple FIFO store (capacity-capped) for mock video GIF bytes.
/// Shared between the mock video provider and the route handler.
pub struct MockVideoStore {
    /// (id, bytes) pairs in insertion order.
    inner: RwLock<VecDeque<(String, Vec<u8>)>>,
}

impl MockVideoStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(VecDeque::with_capacity(MAX_ENTRIES)),
        }
    }

    pub async fn put(&self, id: String, bytes: Vec<u8>) {
        let mut guard = self.inner.write().await;
        if guard.len() >= MAX_ENTRIES {
            guard.pop_front();
        }
        guard.push_back((id, bytes));
    }

    pub async fn get(&self, id: &str) -> Option<Vec<u8>> {
        let guard = self.inner.read().await;
        guard.iter().find(|(k, _)| k == id).map(|(_, v)| v.clone())
    }
}

impl Default for MockVideoStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Global singleton store — shared between provider and handler.
pub fn global_store() -> &'static MockVideoStore {
    static STORE: OnceLock<MockVideoStore> = OnceLock::new();
    STORE.get_or_init(MockVideoStore::new)
}

/// Generate an animated GIF of a colored square moving along a hash-derived path.
///
/// 8 frames × 250 ms = 2-second loop. 256×256 px. The background and square
/// colors are both derived from the prompt hash, so every prompt produces a
/// visually distinct animation.
///
/// No external model API — frames are encoded locally with the `image` crate.
///
/// @see <https://docs.rs/image/latest/image/codecs/gif/struct.GifEncoder.html> — `GifEncoder`
/// @see <https://www.w3.org/Graphics/GIF/spec-gif89a.txt> — GIF89a specification
pub fn generate_visual_video_gif(prompt: &str) -> Vec<u8> {
    let hash = Sha256::digest(prompt.as_bytes());

    let bg = Rgba([hash[0], hash[1], hash[2], 255]);
    let fg = Rgba([
        255u8.wrapping_sub(hash[0]),
        255u8.wrapping_sub(hash[1]),
        255u8.wrapping_sub(hash[2]),
        255,
    ]);

    // 8 waypoints from hash bytes (indices 3..=18 → 16 bytes for 8 (x,y) pairs)
    let waypoints: Vec<(u32, u32)> = (0..8)
        .map(|i| {
            let x = (hash[3 + i * 2] as u32 % 192) + 16;
            let y = (hash[4 + i * 2] as u32 % 192) + 16;
            (x, y)
        })
        .collect();

    let mut bytes: Vec<u8> = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        encoder.set_repeat(Repeat::Infinite).expect("set repeat");

        for &(x, y) in &waypoints {
            let mut frame_buf: ImageBuffer<Rgba<u8>, Vec<u8>> =
                ImageBuffer::from_pixel(256, 256, bg);

            // Draw a 32×32 fg square at (x, y)
            for dy in 0..32u32 {
                for dx in 0..32u32 {
                    let px = (x + dx).min(255);
                    let py = (y + dy).min(255);
                    frame_buf.put_pixel(px, py, fg);
                }
            }

            encoder
                .encode_frame(Frame::from_parts(
                    frame_buf,
                    0,
                    0,
                    Delay::from_numer_denom_ms(250, 1),
                ))
                .expect("encode frame");
        }
    }
    bytes
}

/// Blend two keyframe images into an animated GIF that cross-fades from
/// `first` to `last` over 10 frames. Both inputs are raw image bytes (PNG/JPEG
/// — anything `image` can decode). They get resized to a common 256×256 canvas
/// so any pair of inputs works. Outputs the GIF bytes ready to serve.
///
/// If either decode fails, falls back to a prompt-derived random GIF so the
/// flow still completes (mock provider should never block a request).
///
/// No external model API — inputs are decoded/resized and frames encoded
/// locally with the `image` crate.
///
/// @see <https://docs.rs/image/latest/image/fn.load_from_memory.html> — `image::load_from_memory` (decode keyframes)
/// @see <https://docs.rs/image/latest/image/codecs/gif/struct.GifEncoder.html> — `GifEncoder` (encode blend)
pub fn generate_keyframe_blend_gif(first: &[u8], last: &[u8], prompt: &str) -> Vec<u8> {
    use image::{imageops::FilterType, GenericImageView};

    let canvas = (256u32, 256u32);
    let decoded_first = image::load_from_memory(first);
    let decoded_last = image::load_from_memory(last);
    let (first_img, last_img) = match (decoded_first, decoded_last) {
        (Ok(a), Ok(b)) => (
            a.resize_exact(canvas.0, canvas.1, FilterType::Triangle).to_rgba8(),
            b.resize_exact(canvas.0, canvas.1, FilterType::Triangle).to_rgba8(),
        ),
        _ => return generate_visual_video_gif(prompt),
    };

    const FRAMES: u32 = 10;
    let mut bytes: Vec<u8> = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        encoder.set_repeat(Repeat::Infinite).expect("set repeat");
        for i in 0..FRAMES {
            // alpha goes 0 → 1 across the loop; first dominates at i=0, last at i=FRAMES-1
            let alpha = i as f32 / (FRAMES - 1) as f32;
            let mut frame_buf: ImageBuffer<Rgba<u8>, Vec<u8>> =
                ImageBuffer::new(canvas.0, canvas.1);
            for (x, y, _) in first_img.view(0, 0, canvas.0, canvas.1).pixels() {
                let a = first_img.get_pixel(x, y).0;
                let b = last_img.get_pixel(x, y).0;
                let blend = |ca: u8, cb: u8| -> u8 {
                    ((1.0 - alpha) * ca as f32 + alpha * cb as f32).round().clamp(0.0, 255.0) as u8
                };
                frame_buf.put_pixel(
                    x,
                    y,
                    Rgba([blend(a[0], b[0]), blend(a[1], b[1]), blend(a[2], b[2]), 255]),
                );
            }
            encoder
                .encode_frame(Frame::from_parts(
                    frame_buf,
                    0,
                    0,
                    Delay::from_numer_denom_ms(200, 1),
                ))
                .expect("encode frame");
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_png(r: u8, g: u8, b: u8) -> Vec<u8> {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_pixel(64, 64, Rgba([r, g, b, 255]));
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        bytes
    }

    #[test]
    fn keyframe_blend_produces_valid_gif_with_intermediate_frames() {
        let first = solid_png(255, 0, 0);   // red
        let last = solid_png(0, 0, 255);    // blue
        let bytes = generate_keyframe_blend_gif(&first, &last, "ignored");
        assert_eq!(&bytes[..6], b"GIF89a");
        // 10 frames at 256×256 must be substantially larger than the 8-frame random one.
        assert!(bytes.len() > 1000, "keyframe blend should be a real multi-frame GIF, got {} bytes", bytes.len());
    }

    #[test]
    fn keyframe_blend_falls_back_when_inputs_undecodable() {
        let garbage = vec![0u8, 1, 2, 3];
        let bytes = generate_keyframe_blend_gif(&garbage, &garbage, "fallback test prompt");
        // Should still produce a valid GIF via the prompt-derived fallback.
        assert_eq!(&bytes[..6], b"GIF89a");
    }

    #[test]
    fn different_keyframes_produce_different_gifs() {
        let red = solid_png(255, 0, 0);
        let blue = solid_png(0, 0, 255);
        let green = solid_png(0, 255, 0);
        let a = generate_keyframe_blend_gif(&red, &blue, "");
        let b = generate_keyframe_blend_gif(&red, &green, "");
        assert_ne!(a, b, "Different last-frames must produce different GIFs");
    }

    #[test]
    fn generates_valid_gif_bytes() {
        let bytes = generate_visual_video_gif("a cinematic timelapse of clouds");
        // GIF89a magic
        assert_eq!(&bytes[..6], b"GIF89a", "Output must start with GIF89a magic");
        assert!(
            bytes.len() > 500,
            "Expected real GIF > 500 bytes, got {}",
            bytes.len()
        );
    }

    #[test]
    fn different_prompts_produce_different_gifs() {
        let a = generate_visual_video_gif("prompt alpha");
        let b = generate_visual_video_gif("prompt beta");
        assert_ne!(a, b, "Different prompts must produce different GIFs");
    }

    #[test]
    fn same_prompt_is_deterministic() {
        let a = generate_visual_video_gif("deterministic video test");
        let b = generate_visual_video_gif("deterministic video test");
        assert_eq!(a, b, "Same prompt must produce identical GIF");
    }
}
