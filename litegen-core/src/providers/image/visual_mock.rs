use image::{ImageBuffer, ImageFormat, Rgba};
use sha2::{Digest, Sha256};

/// Generate a deterministic colored PNG derived from the prompt.
///
/// Hashes the prompt to derive a background color, accent color, and circle
/// position/radius. Produces a 512×512 RGBA PNG that is visually distinct per
/// prompt and clearly renderable in any browser.
///
/// No external model API — the PNG is encoded locally with the `image` crate.
///
/// @see <https://docs.rs/image/latest/image/struct.ImageBuffer.html#method.write_to> — `ImageBuffer::write_to`
/// @see <https://docs.rs/image/latest/image/enum.ImageFormat.html> — `ImageFormat::Png` encoder used here
pub fn generate_visual_image_png(prompt: &str) -> Vec<u8> {
    let hash = Sha256::digest(prompt.as_bytes());

    let r = hash[0];
    let g = hash[1];
    let b = hash[2];
    let accent_r = 255u8.wrapping_sub(r);
    let accent_g = 255u8.wrapping_sub(g);
    let accent_b = 255u8.wrapping_sub(b);

    // Circle center in [64, 448) and radius in [32, 128)
    let cx = (hash[3] as u32 % 384) + 64;
    let cy = (hash[4] as u32 % 384) + 64;
    let radius = (hash[5] as u32 % 96) + 32;

    // Second smaller circle for visual interest
    let cx2 = (hash[6] as u32 % 384) + 64;
    let cy2 = (hash[7] as u32 % 384) + 64;
    let r2 = (hash[8] as u32 % 48) + 16;

    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(512, 512, Rgba([r, g, b, 255]));

    // Draw main accent circle
    for y in 0..512u32 {
        for x in 0..512u32 {
            let dx = x as i64 - cx as i64;
            let dy = y as i64 - cy as i64;
            if (dx * dx + dy * dy) as u64 <= (radius as u64 * radius as u64) {
                img.put_pixel(x, y, Rgba([accent_r, accent_g, accent_b, 255]));
            }
        }
    }

    // Draw second smaller circle (original bg color, creating a "ring" effect)
    for y in 0..512u32 {
        for x in 0..512u32 {
            let dx = x as i64 - cx2 as i64;
            let dy = y as i64 - cy2 as i64;
            if (dx * dx + dy * dy) as u64 <= (r2 as u64 * r2 as u64) {
                img.put_pixel(x, y, Rgba([r, g, b, 255]));
            }
        }
    }

    let mut bytes: Vec<u8> = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("png encode");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_valid_png_bytes() {
        let bytes = generate_visual_image_png("a beautiful sunset over the ocean");
        // PNG magic bytes: 0x89 P N G \r \n 0x1a \n
        assert_eq!(&bytes[..8], &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            "Output must start with PNG magic bytes");
        assert!(bytes.len() > 1000,
            "Expected real PNG > 1000 bytes, got {}", bytes.len());
    }

    #[test]
    fn different_prompts_produce_different_images() {
        let a = generate_visual_image_png("prompt one");
        let b = generate_visual_image_png("prompt two");
        assert_ne!(a, b, "Different prompts must produce different images");
    }

    #[test]
    fn same_prompt_is_deterministic() {
        let a = generate_visual_image_png("deterministic test");
        let b = generate_visual_image_png("deterministic test");
        assert_eq!(a, b, "Same prompt must produce identical output");
    }
}
