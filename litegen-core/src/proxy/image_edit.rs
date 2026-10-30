//! Partial image edits (PixelGenie 3D doc §4): one mask convention for every
//! caller, turned into each vendor's own before anything is billed.
//!
//! A caller's mask is an opaque image the size of `init`: white (128 or more)
//! edits, black keeps. A `region` is a rectangle of `init`; with a mask too,
//! the edit area is where they overlap. This module checks both against the
//! init image, grows the edit area by `mask_grow_px` (or leaves that to a
//! vendor that grows masks itself), and hands the vendor either the mask in
//! its own convention or — for a box-native vendor — a window around the
//! region, enlarged to the vendor's minimum, with the exact area as the mask.

use image::{GrayImage, Luma, Rgba, RgbaImage};

pub use crate::capabilities::MaskConvention;
use crate::types::Region;

/// What the vendor takes, from the model's catalog row.
#[derive(Debug, Clone, PartialEq)]
pub struct EditTarget {
    /// The vendor's mask convention.
    pub convention: MaskConvention,
    /// The vendor grows masks itself: the growth goes to it.
    pub native_grow: bool,
    /// Set when the vendor takes a region as its own box.
    pub region: Option<RegionTarget>,
}

/// A box-native vendor's limits.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionTarget {
    /// Its smallest box side, in pixels.
    pub min_window_px: u32,
    /// Its largest box aspect ratio, either way round.
    pub max_aspect: f64,
    /// Whether it takes a mask beside the box — which a region smaller than
    /// its window needs.
    pub takes_mask: bool,
}

/// One edit as the caller sent it.
#[derive(Debug, Clone, Copy)]
pub struct EditRequest<'a> {
    pub init: &'a [u8],
    pub mask: Option<&'a [u8]>,
    pub region: Option<Region>,
    pub grow_px: u32,
}

/// What to send the vendor.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedEdit {
    /// The mask, as a PNG in the vendor's convention; `None` when a box says
    /// it all.
    pub mask_png: Option<Vec<u8>>,
    /// The box, for a box-native vendor.
    pub window: Option<Region>,
    /// The growth for a vendor that grows masks itself — explicit, even 0,
    /// because vendor defaults are not.
    pub vendor_grow_px: Option<u32>,
}

/// Why an edit was refused. Every one is decided before anything is billed.
#[derive(Debug, Clone, PartialEq)]
pub enum EditError {
    ImageUnreadable(String),
    MaskSizeMismatch { init: (u32, u32), mask: (u32, u32) },
    MaskAmbiguous,
    RegionOutOfBounds { region: Region, init: (u32, u32) },
    RegionTooSmall(String),
}

impl EditError {
    pub fn code(&self) -> &'static str {
        match self {
            EditError::ImageUnreadable(_) => "image_unreadable",
            EditError::MaskSizeMismatch { .. } => "mask_size_mismatch",
            EditError::MaskAmbiguous => "mask_ambiguous",
            EditError::RegionOutOfBounds { .. } => "region_out_of_bounds",
            EditError::RegionTooSmall(_) => "region_too_small",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            EditError::RegionTooSmall(_) => 422,
            _ => 400,
        }
    }

    pub fn param(&self) -> &'static str {
        match self {
            EditError::RegionOutOfBounds { .. } | EditError::RegionTooSmall(_) => "region",
            _ => "reference_images",
        }
    }

    pub fn message(&self) -> String {
        match self {
            EditError::ImageUnreadable(m) => m.clone(),
            EditError::MaskSizeMismatch { init, mask } => format!(
                "the mask is {}x{} and the init image {}x{}; a mask is the init image's size",
                mask.0, mask.1, init.0, init.1
            ),
            EditError::MaskAmbiguous => "the mask has a pixel whose alpha is below 255; masks are opaque, white to edit and black to keep".into(),
            EditError::RegionOutOfBounds { region, init } => format!(
                "region {}x{} at {},{} is not inside the {}x{} init image (and must be at least 1x1)",
                region.w, region.h, region.x, region.y, init.0, init.1
            ),
            EditError::RegionTooSmall(m) => m.clone(),
        }
    }
}

fn decode(bytes: &[u8], what: &str) -> Result<RgbaImage, EditError> {
    image::load_from_memory(bytes)
        .map(|i| i.to_rgba8())
        .map_err(|e| EditError::ImageUnreadable(format!("the {what} image could not be read: {e}")))
}

/// The edit area as a bitmap: `true` edits.
struct Area {
    w: u32,
    h: u32,
    on: Vec<bool>,
}

impl Area {
    fn rect(w: u32, h: u32, r: Region) -> Area {
        let mut on = vec![false; (w * h) as usize];
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                on[(y * w + x) as usize] = true;
            }
        }
        Area { w, h, on }
    }

    fn intersect(&mut self, other: &Area) {
        for (a, b) in self.on.iter_mut().zip(&other.on) {
            *a &= *b;
        }
    }

    /// Grow outward by `r` pixels in every direction (a square neighbourhood,
    /// in two separable passes).
    fn grow(&mut self, r: u32) {
        if r == 0 {
            return;
        }
        let (w, h, r) = (self.w as i64, self.h as i64, r as i64);
        let mut rows = vec![false; self.on.len()];
        for y in 0..h {
            // The distance to the nearest edit pixel on this row, both ways.
            let mut last: i64 = i64::MIN / 2;
            for x in 0..w {
                if self.on[(y * w + x) as usize] {
                    last = x;
                }
                if x - last <= r {
                    rows[(y * w + x) as usize] = true;
                }
            }
            last = i64::MAX / 2;
            for x in (0..w).rev() {
                if self.on[(y * w + x) as usize] {
                    last = x;
                }
                if last - x <= r {
                    rows[(y * w + x) as usize] = true;
                }
            }
        }
        let mut out = vec![false; self.on.len()];
        for x in 0..w {
            let mut last: i64 = i64::MIN / 2;
            for y in 0..h {
                if rows[(y * w + x) as usize] {
                    last = y;
                }
                if y - last <= r {
                    out[(y * w + x) as usize] = true;
                }
            }
            last = i64::MAX / 2;
            for y in (0..h).rev() {
                if rows[(y * w + x) as usize] {
                    last = y;
                }
                if last - y <= r {
                    out[(y * w + x) as usize] = true;
                }
            }
        }
        self.on = out;
    }

    /// The area as a PNG in `convention`.
    fn encode(&self, convention: MaskConvention) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut out);
        let at = |x: u32, y: u32| self.on[(y * self.w + x) as usize];
        let written = match convention {
            MaskConvention::WhiteEdits | MaskConvention::BlackEdits => {
                let edit = if convention == MaskConvention::WhiteEdits { 255 } else { 0 };
                GrayImage::from_fn(self.w, self.h, |x, y| Luma([if at(x, y) { edit } else { 255 - edit }]))
                    .write_to(&mut cursor, image::ImageFormat::Png)
            }
            MaskConvention::TransparentEdits => {
                RgbaImage::from_fn(self.w, self.h, |x, y| if at(x, y) { Rgba([0, 0, 0, 0]) } else { Rgba([0, 0, 0, 255]) })
                    .write_to(&mut cursor, image::ImageFormat::Png)
            }
        };
        written.expect("a PNG encodes into memory");
        out
    }
}

/// A window around `r` on a `w`×`h` image: at least `min` a side, its aspect
/// within `max_aspect` either way, centred on `r` and kept on the image.
fn window(r: Region, w: u32, h: u32, min: u32, max_aspect: f64) -> Option<Region> {
    let mut ww = r.w.max(min);
    let mut wh = r.h.max(min);
    if (ww as f64) > wh as f64 * max_aspect {
        wh = (ww as f64 / max_aspect).ceil() as u32;
    }
    if (wh as f64) > ww as f64 * max_aspect {
        ww = (wh as f64 / max_aspect).ceil() as u32;
    }
    if ww > w || wh > h {
        return None;
    }
    let place = |start: u32, len: u32, win: u32, total: u32| -> u32 {
        let centred = (start as i64 + len as i64 / 2 - win as i64 / 2).max(0) as u32;
        centred.min(total - win)
    };
    Some(Region { x: place(r.x, r.w, ww, w), y: place(r.y, r.h, wh, h), w: ww, h: wh })
}

/// Check an edit against its init image and turn it into what the vendor
/// takes.
pub fn prepare(req: &EditRequest, target: &EditTarget) -> Result<PreparedEdit, EditError> {
    let init = decode(req.init, "init")?;
    let (w, h) = init.dimensions();
    let mut area: Option<Area> = None;
    if let Some(m) = req.mask {
        let m = decode(m, "mask")?;
        if m.dimensions() != (w, h) {
            return Err(EditError::MaskSizeMismatch { init: (w, h), mask: m.dimensions() });
        }
        if m.pixels().any(|p| p[3] < 255) {
            return Err(EditError::MaskAmbiguous);
        }
        // Luma by the usual weights; 128 or more edits.
        let on = m
            .pixels()
            .map(|p| (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000 >= 128)
            .collect();
        area = Some(Area { w, h, on });
    }
    if let Some(r) = req.region {
        let inside = r.w > 0 && r.h > 0 && r.x.checked_add(r.w).is_some_and(|e| e <= w) && r.y.checked_add(r.h).is_some_and(|e| e <= h);
        if !inside {
            return Err(EditError::RegionOutOfBounds { region: r, init: (w, h) });
        }
        let rect = Area::rect(w, h, r);
        area = Some(match area {
            Some(mut a) => {
                a.intersect(&rect);
                a
            }
            None => rect,
        });
    }
    let Some(mut area) = area else {
        return Ok(PreparedEdit { mask_png: None, window: None, vendor_grow_px: None });
    };
    let vendor_grow_px = if target.native_grow {
        Some(req.grow_px)
    } else {
        area.grow(req.grow_px);
        None
    };
    match (&target.region, req.region) {
        (Some(t), Some(r)) => {
            let win = window(r, w, h, t.min_window_px, t.max_aspect).ok_or_else(|| {
                EditError::RegionTooSmall(format!(
                    "this model's box is at least {} px a side within 1:{} and the init image is {w}x{h}",
                    t.min_window_px, t.max_aspect
                ))
            })?;
            // The box alone says it all when it IS the edit area.
            let exact = win == r && req.mask.is_none() && req.grow_px == 0;
            if !exact && !t.takes_mask {
                return Err(EditError::RegionTooSmall(format!(
                    "this model edits boxes of at least {} px a side and takes no mask to confine a smaller edit",
                    t.min_window_px
                )));
            }
            Ok(PreparedEdit { mask_png: (!exact).then(|| area.encode(target.convention)), window: Some(win), vendor_grow_px })
        }
        _ => Ok(PreparedEdit { mask_png: Some(area.encode(target.convention)), window: None, vendor_grow_px }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, ImageBuffer, Luma, Rgba, RgbaImage};

    fn png_rgba(img: &RgbaImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    fn init(w: u32, h: u32) -> Vec<u8> {
        png_rgba(&ImageBuffer::from_fn(w, h, |x, y| Rgba([(x % 256) as u8, (y % 256) as u8, 7, 255])))
    }

    /// An opaque white-edits mask: white inside `r`, black outside.
    fn mask(w: u32, h: u32, r: Region) -> Vec<u8> {
        png_rgba(&ImageBuffer::from_fn(w, h, |x, y| {
            let inside = x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
            if inside { Rgba([255, 255, 255, 255]) } else { Rgba([0, 0, 0, 255]) }
        }))
    }

    fn region(x: u32, y: u32, w: u32, h: u32) -> Region {
        Region { x, y, w, h }
    }

    fn white_mask_target() -> EditTarget {
        EditTarget { convention: MaskConvention::WhiteEdits, native_grow: false, region: None }
    }

    fn decoded(png: &[u8]) -> RgbaImage {
        image::load_from_memory(png).unwrap().to_rgba8()
    }

    fn edited(m: &RgbaImage, x: u32, y: u32, convention: MaskConvention) -> bool {
        let p = m.get_pixel(x, y);
        match convention {
            MaskConvention::WhiteEdits => p[0] >= 128,
            MaskConvention::BlackEdits => p[0] < 128,
            MaskConvention::TransparentEdits => p[3] == 0,
        }
    }

    #[test]
    fn a_white_edits_mask_becomes_each_vendors_convention() {
        let r = region(2, 3, 4, 5);
        let (i, m) = (init(16, 16), mask(16, 16, r));
        for convention in [MaskConvention::WhiteEdits, MaskConvention::BlackEdits, MaskConvention::TransparentEdits] {
            let target = EditTarget { convention, native_grow: false, region: None };
            let out = prepare(&EditRequest { init: &i, mask: Some(&m), region: None, grow_px: 0 }, &target).unwrap();
            let sent = decoded(out.mask_png.as_ref().unwrap());
            assert_eq!(sent.dimensions(), (16, 16));
            for (x, y) in [(2, 3), (5, 7), (1, 3), (6, 3), (2, 8), (0, 0)] {
                let inside = x >= 2 && x < 6 && y >= 3 && y < 8;
                assert_eq!(edited(&sent, x, y, convention), inside, "{convention:?} at {x},{y}");
            }
            if convention == MaskConvention::TransparentEdits {
                assert_eq!(sent.get_pixel(0, 0)[3], 255, "kept pixels opaque");
            }
        }
    }

    #[test]
    fn a_translucent_mask_is_ambiguous() {
        let mut m = decoded(&mask(8, 8, region(0, 0, 4, 4)));
        m.put_pixel(7, 7, Rgba([0, 0, 0, 254]));
        let err = prepare(&EditRequest { init: &init(8, 8), mask: Some(&png_rgba(&m)), region: None, grow_px: 0 }, &white_mask_target()).unwrap_err();
        assert_eq!(err.code(), "mask_ambiguous");
    }

    #[test]
    fn a_mask_of_another_size_is_refused() {
        let err = prepare(&EditRequest { init: &init(8, 8), mask: Some(&mask(8, 9, region(0, 0, 2, 2))), region: None, grow_px: 0 }, &white_mask_target()).unwrap_err();
        assert_eq!(err.code(), "mask_size_mismatch");
    }

    #[test]
    fn a_region_off_the_image_or_empty_is_out_of_bounds() {
        for r in [region(6, 0, 3, 2), region(0, 7, 1, 2), region(0, 0, 0, 3)] {
            let err = prepare(&EditRequest { init: &init(8, 8), mask: None, region: Some(r), grow_px: 0 }, &white_mask_target()).unwrap_err();
            assert_eq!(err.code(), "region_out_of_bounds", "{r:?}");
        }
    }

    #[test]
    fn a_region_on_a_mask_model_arrives_drawn_as_its_mask() {
        let out = prepare(&EditRequest { init: &init(10, 10), mask: None, region: Some(region(1, 2, 3, 4)), grow_px: 0 }, &white_mask_target()).unwrap();
        let m = decoded(out.mask_png.as_ref().unwrap());
        let white: Vec<(u32, u32)> = (0..10).flat_map(|y| (0..10).map(move |x| (x, y))).filter(|&(x, y)| m.get_pixel(x, y)[0] >= 128).collect();
        assert_eq!(white.len(), 12);
        assert!(white.iter().all(|&(x, y)| (1..4).contains(&x) && (2..6).contains(&y)));
        assert_eq!(out.window, None);
    }

    #[test]
    fn a_region_and_a_mask_edit_only_where_they_overlap() {
        let out = prepare(
            &EditRequest { init: &init(10, 10), mask: Some(&mask(10, 10, region(0, 0, 5, 10))), region: Some(region(3, 0, 5, 2)), grow_px: 0 },
            &white_mask_target(),
        ).unwrap();
        let m = decoded(out.mask_png.as_ref().unwrap());
        let white = (0..10).flat_map(|y| (0..10).map(move |x| (x, y))).filter(|&(x, y)| m.get_pixel(x, y)[0] >= 128).count();
        assert_eq!(white, 4, "x 3..5 × y 0..2");
    }

    #[test]
    fn litegen_grows_the_edit_area_where_the_vendor_cannot() {
        let out = prepare(&EditRequest { init: &init(10, 10), mask: None, region: Some(region(4, 4, 1, 1)), grow_px: 2 }, &white_mask_target()).unwrap();
        let m = decoded(out.mask_png.as_ref().unwrap());
        assert!(m.get_pixel(2, 4)[0] >= 128 && m.get_pixel(4, 6)[0] >= 128, "2 px out");
        assert!(m.get_pixel(1, 4)[0] < 128, "not 3");
        assert_eq!(out.vendor_grow_px, None);
    }

    #[test]
    fn a_vendor_that_grows_masks_gets_the_exact_mask_and_an_explicit_growth() {
        let target = EditTarget { convention: MaskConvention::WhiteEdits, native_grow: true, region: None };
        for grow in [0, 3] {
            let out = prepare(&EditRequest { init: &init(10, 10), mask: None, region: Some(region(4, 4, 1, 1)), grow_px: grow }, &target).unwrap();
            let m = decoded(out.mask_png.as_ref().unwrap());
            assert_eq!((0..10).flat_map(|y| (0..10).map(move |x| (x, y))).filter(|&(x, y)| m.get_pixel(x, y)[0] >= 128).count(), 1);
            assert_eq!(out.vendor_grow_px, Some(grow), "explicit, even 0: the vendor's default is not");
        }
    }

    fn box_target(takes_mask: bool) -> EditTarget {
        EditTarget {
            convention: MaskConvention::BlackEdits,
            native_grow: false,
            region: Some(RegionTarget { min_window_px: 256, max_aspect: 6.0, takes_mask }),
        }
    }

    #[test]
    fn a_region_the_vendor_takes_as_a_box_is_sent_exactly_when_it_is_big_enough() {
        let out = prepare(&EditRequest { init: &init(1024, 768), mask: None, region: Some(region(384, 256, 300, 260)), grow_px: 0 }, &box_target(true)).unwrap();
        assert_eq!(out.window, Some(region(384, 256, 300, 260)));
        assert!(out.mask_png.is_none(), "the box says it all");
    }

    #[test]
    fn a_small_region_gets_the_vendors_minimum_window_and_its_exact_area_as_the_mask() {
        let out = prepare(&EditRequest { init: &init(1024, 768), mask: None, region: Some(region(384, 256, 128, 64)), grow_px: 0 }, &box_target(true)).unwrap();
        // Centred on the region, 256 a side.
        assert_eq!(out.window, Some(region(320, 160, 256, 256)));
        let m = decoded(out.mask_png.as_ref().unwrap());
        assert!(edited(&m, 384, 256, MaskConvention::BlackEdits) && edited(&m, 511, 319, MaskConvention::BlackEdits));
        assert!(!edited(&m, 383, 256, MaskConvention::BlackEdits) && !edited(&m, 512, 300, MaskConvention::BlackEdits));
    }

    #[test]
    fn a_window_is_kept_on_the_image_and_within_the_vendors_aspect_ratio() {
        // A thin strip in a corner: enlarged to 256 high, wider than 6:1 → taller.
        let out = prepare(&EditRequest { init: &init(2000, 400), mask: None, region: Some(region(0, 380, 1800, 20)), grow_px: 0 }, &box_target(true)).unwrap();
        let w = out.window.unwrap();
        assert!(w.x + w.w <= 2000 && w.y + w.h <= 400, "{w:?}");
        assert!(w.w as f64 / w.h as f64 <= 6.0 + 1e-9 && w.h >= 256, "{w:?}");
        assert!(w.x <= 0 && w.y <= 380 && w.y + w.h >= 400, "covers the region: {w:?}");
    }

    #[test]
    fn a_region_too_small_for_a_vendor_that_takes_no_mask_is_refused() {
        let err = prepare(&EditRequest { init: &init(1024, 768), mask: None, region: Some(region(10, 10, 40, 40)), grow_px: 0 }, &box_target(false)).unwrap_err();
        assert_eq!(err.code(), "region_too_small");
        let err = prepare(&EditRequest { init: &init(200, 200), mask: None, region: Some(region(10, 10, 40, 40)), grow_px: 0 }, &box_target(true)).unwrap_err();
        assert_eq!(err.code(), "region_too_small", "no 256 px window fits a 200 px image");
    }

    #[test]
    fn an_unreadable_init_says_so() {
        let err = prepare(&EditRequest { init: b"not an image", mask: None, region: Some(region(0, 0, 1, 1)), grow_px: 0 }, &white_mask_target()).unwrap_err();
        assert_eq!(err.code(), "image_unreadable");
        let _ = GrayImage::new(1, 1);
        let _: Luma<u8> = Luma([0]);
    }
}
