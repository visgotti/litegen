//! What a vendor's segmentation masks become (PixelGenie L7): a grayscale
//! PNG the size of the image, white where selected and black elsewhere, with
//! the box litegen measured around the white — whatever container, colours,
//! alpha or size the vendor used.

use crate::providers::VendorMask;
use crate::types::{Region, SegmentationMask};

/// The image a segmentation request sent (base64 or a `data:` URI): its
/// bytes, content type and size. PNG, JPEG, WebP and GIF are read whole, so a
/// truncated file is refused here rather than at the vendor.
pub fn read_image(value: &str) -> Result<(bytes::Bytes, &'static str, u32, u32), String> {
    use base64::Engine;
    let b64 = match value.strip_prefix("data:") {
        Some(uri) => uri.split_once(";base64,").map(|(_, b)| b).ok_or("a data: URI must be base64")?,
        None => value,
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("image is not base64: {e}"))?;
    let content_type = match image::guess_format(&bytes) {
        Ok(image::ImageFormat::Png) => "image/png",
        Ok(image::ImageFormat::Jpeg) => "image/jpeg",
        Ok(image::ImageFormat::WebP) => "image/webp",
        Ok(image::ImageFormat::Gif) => "image/gif",
        _ => return Err("image is not a PNG, JPEG, WebP or GIF".into()),
    };
    let decoded = image::load_from_memory(&bytes).map_err(|e| format!("image cannot be read: {e}"))?;
    Ok((bytes.into(), content_type, decoded.width(), decoded.height()))
}

/// The masks a caller gets: each normalised, empty ones left out, best
/// score first (a mask with no score after those with one), at most `max`.
pub fn masks(vendor: Vec<VendorMask>, width: u32, height: u32, max: usize) -> Result<Vec<SegmentationMask>, image::ImageError> {
    let mut out = Vec::with_capacity(vendor.len());
    for m in vendor {
        if let Some(mask) = normalise(&m.bytes, width, height, m.score)? {
            out.push(mask);
        }
    }
    out.sort_by(|a, b| b.score.unwrap_or(f64::NEG_INFINITY).total_cmp(&a.score.unwrap_or(f64::NEG_INFINITY)));
    out.truncate(max);
    Ok(out)
}

/// One mask as litegen serves it; `None` when it selects nothing. A pixel is
/// selected when it is opaque and bright in any channel.
pub fn normalise(bytes: &[u8], width: u32, height: u32, score: Option<f64>) -> Result<Option<SegmentationMask>, image::ImageError> {
    use base64::Engine;
    let mut vendor = image::load_from_memory(bytes)?.to_rgba8();
    if vendor.dimensions() != (width, height) {
        vendor = image::imageops::resize(&vendor, width, height, image::imageops::FilterType::Nearest);
    }
    let mask = image::GrayImage::from_fn(width, height, |x, y| {
        let [r, g, b, a] = vendor.get_pixel(x, y).0;
        image::Luma([if a >= 128 && r.max(g).max(b) >= 128 { 255 } else { 0 }])
    });
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, _) in mask.enumerate_pixels().filter(|(_, _, p)| p.0[0] == 255) {
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    if x0 == u32::MAX {
        return Ok(None);
    }
    let mut png = Vec::new();
    mask.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    Ok(Some(SegmentationMask {
        png_base64: base64::engine::general_purpose::STANDARD.encode(png),
        score,
        bounds: Region { x: x0, y: y0, w: x1 - x0 + 1, h: y1 - y0 + 1 },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use image::{GrayImage, Rgba, RgbaImage};

    fn encode(img: &RgbaImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    fn pixels(m: &SegmentationMask) -> GrayImage {
        let png = base64::engine::general_purpose::STANDARD.decode(&m.png_base64).unwrap();
        assert_eq!(image::guess_format(&png).unwrap(), image::ImageFormat::Png);
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!(img.color(), image::ColorType::L8, "grayscale");
        img.to_luma8()
    }

    #[test]
    fn a_coloured_mask_on_transparency_becomes_white_on_black_with_its_box() {
        // Blue where selected, transparent elsewhere; one translucent pixel.
        let mut img = RgbaImage::from_fn(20, 10, |x, y| if (4..9).contains(&x) && (2..5).contains(&y) { Rgba([0, 0, 255, 255]) } else { Rgba([255, 255, 255, 0]) });
        img.put_pixel(15, 8, Rgba([255, 255, 255, 40]));
        let m = normalise(&encode(&img), 20, 10, Some(0.7)).unwrap().unwrap();
        assert_eq!((m.bounds, m.score), (Region { x: 4, y: 2, w: 5, h: 3 }, Some(0.7)));
        for (x, y, p) in pixels(&m).enumerate_pixels() {
            assert_eq!(p.0[0], if (4..9).contains(&x) && (2..5).contains(&y) { 255 } else { 0 }, "{x},{y}");
        }
    }

    #[test]
    fn a_mask_of_another_size_is_scaled_to_the_image() {
        let img = RgbaImage::from_fn(10, 5, |x, _| if x >= 5 { Rgba([255, 255, 255, 255]) } else { Rgba([0, 0, 0, 255]) });
        let m = normalise(&encode(&img), 20, 10, None).unwrap().unwrap();
        assert_eq!(m.bounds, Region { x: 10, y: 0, w: 10, h: 10 });
        assert_eq!(pixels(&m).dimensions(), (20, 10));
    }

    #[test]
    fn empty_masks_are_left_out_and_the_rest_come_best_first() {
        let blank = encode(&RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 255])));
        let dot = |x| encode(&RgbaImage::from_fn(8, 8, move |px, py| if (px, py) == (x, 1) { Rgba([255; 4]) } else { Rgba([0, 0, 0, 255]) }));
        let out = masks(
            vec![
                VendorMask { bytes: dot(1), score: None },
                VendorMask { bytes: blank, score: Some(0.99) },
                VendorMask { bytes: dot(2), score: Some(0.5) },
                VendorMask { bytes: dot(3), score: Some(0.8) },
            ],
            8,
            8,
            2,
        )
        .unwrap();
        assert_eq!(out.iter().map(|m| (m.bounds.x, m.score)).collect::<Vec<_>>(), vec![(3, Some(0.8)), (2, Some(0.5))]);
        assert!(normalise(b"not an image", 8, 8, None).is_err());
    }
}
