//! Opt-in pixel preservation (PixelGenie 3D doc §4, P2): litegen pastes the
//! init image back outside the edit area, so untouched pixels come back
//! byte-identical from any vendor; `feather_px` blends from the init at the
//! area's edge to the vendor's pixels that far inside it.

/// What preservation needs, fixed when the edit was prepared.
#[derive(Debug, Clone, PartialEq)]
pub struct Preserve {
    /// The init image as sent.
    pub init: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// The edit area at the init's size, row by row: true where the vendor's
    /// pixels stand.
    pub area: Vec<bool>,
    pub feather_px: u32,
    /// Encode the result as lossless WebP rather than PNG.
    pub webp: bool,
}

/// The vendor's image with the init pasted back outside the edit area. One of
/// another size is first resized to the init's.
pub fn composite(vendor: &[u8], p: &Preserve) -> Result<Vec<u8>, image::ImageError> {
    let init = image::load_from_memory(&p.init)?.to_rgba8();
    let mut out = image::load_from_memory(vendor)?.to_rgba8();
    if out.dimensions() != (p.width, p.height) {
        out = image::imageops::resize(&out, p.width, p.height, image::imageops::FilterType::Lanczos3);
    }
    let weights = feather(&p.area, p.width, p.height, p.feather_px);
    for ((o, s), w) in out.pixels_mut().zip(init.pixels()).zip(weights) {
        if w <= 0.0 {
            *o = *s;
        } else if w < 1.0 {
            for k in 0..4 {
                o.0[k] = (w * f32::from(o.0[k]) + (1.0 - w) * f32::from(s.0[k])).round() as u8;
            }
        }
    }
    let mut bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut bytes);
    if p.webp {
        out.write_with_encoder(image::codecs::webp::WebPEncoder::new_lossless(&mut cursor))?;
    } else {
        out.write_to(&mut cursor, image::ImageFormat::Png)?;
    }
    Ok(bytes)
}

/// How much of the vendor's pixel stands, per pixel: 0 outside the area, 1
/// inside it at least `px` from its edge, `d / px` at `d` (chessboard) in.
fn feather(area: &[bool], w: u32, h: u32, px: u32) -> Vec<f32> {
    let (w, h) = (w as usize, h as usize);
    if px == 0 {
        return area.iter().map(|&a| if a { 1.0 } else { 0.0 }).collect();
    }
    // Distance from the nearest pixel outside the area, by breadth-first steps.
    let mut dist = vec![u32::MAX; w * h];
    let mut queue = std::collections::VecDeque::new();
    for (i, &a) in area.iter().enumerate() {
        if !a {
            dist[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        if dist[i] >= px {
            continue;
        }
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                let n = ny as usize * w + nx as usize;
                if dist[n] == u32::MAX {
                    dist[n] = dist[i] + 1;
                    queue.push_back(n);
                }
            }
        }
    }
    dist.into_iter().map(|d| (d as f32 / px as f32).min(1.0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn png(img: &RgbaImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    fn plan(feather_px: u32) -> (RgbaImage, Preserve) {
        let init = RgbaImage::from_fn(12, 8, |x, y| Rgba([(x * 20) as u8, (y * 30) as u8, 5, 255]));
        let area = (0..8).flat_map(|y| (0..12).map(move |x| (2..10).contains(&x) && (1..7).contains(&y))).collect();
        (init.clone(), Preserve { init: png(&init), width: 12, height: 8, area, feather_px, webp: false })
    }

    fn decode(bytes: &[u8]) -> RgbaImage {
        image::load_from_memory(bytes).unwrap().to_rgba8()
    }

    #[test]
    fn outside_the_area_is_the_init_exactly_and_inside_is_the_vendors() {
        let (init, p) = plan(0);
        let vendor = RgbaImage::from_pixel(12, 8, Rgba([9, 9, 9, 255]));
        let out = decode(&composite(&png(&vendor), &p).unwrap());
        for (x, y, px) in out.enumerate_pixels() {
            let inside = (2..10).contains(&x) && (1..7).contains(&y);
            assert_eq!(*px, if inside { Rgba([9, 9, 9, 255]) } else { *init.get_pixel(x, y) }, "{x},{y}");
        }
    }

    #[test]
    fn a_feather_ramps_from_the_init_at_the_edge_to_the_vendor_inside() {
        let (init, p) = plan(2);
        let vendor = RgbaImage::from_pixel(12, 8, Rgba([250, 250, 250, 255]));
        let out = decode(&composite(&png(&vendor), &p).unwrap());
        assert_eq!(out.get_pixel(0, 0), init.get_pixel(0, 0));
        let edge = out.get_pixel(2, 3).0[0];
        assert!(edge > init.get_pixel(2, 3).0[0] && edge < 250, "half way at the edge: {edge}");
        assert_eq!(out.get_pixel(5, 3).0, [250, 250, 250, 255], "2 px in it is all vendor");
    }

    #[test]
    fn a_vendor_image_of_another_size_is_scaled_to_the_init_first() {
        let (init, p) = plan(0);
        let out = decode(&composite(&png(&RgbaImage::from_pixel(24, 16, Rgba([1, 2, 3, 255]))), &p).unwrap());
        assert_eq!(out.dimensions(), (12, 8));
        assert_eq!((out.get_pixel(0, 0), out.get_pixel(5, 3).0), (init.get_pixel(0, 0), [1, 2, 3, 255]));
    }
}
