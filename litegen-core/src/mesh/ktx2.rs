//! KTX2 textures (mesh-gaps item 3): the container, the one payload that
//! needs no codec (uncompressed `VK_FORMAT_R8G8B8A8_UNORM` / `_SRGB`), the
//! ETC1S/BasisLZ decoder (the private `etc1s` module below), the UASTC LDR 4x4
//! decoder (the private `uastc` module) and Zstandard supercompression
//! (scheme 2, through `ruzstd`).
//!
//! Std only apart from `ruzstd`, which this file alone names, and no I/O:
//! bytes in, RGBA8 out. [`decode_rgba8`] decodes level 0, layer 0, face 0, as
//! stored: no colour conversion, and the Data Format Descriptor is read only to
//! tell a Basis payload's kind, never to change a byte.

//! # The file
//!
//! The Khronos KTX File Format Specification, version 2.0
//! (<https://registry.khronos.org/KTX/specs/2.0/ktxspec.v2.html>), whose
//! sections this file reads in order, cited by their numbers there. Every
//! number is little-endian.
//!
//! - **Identifier** (§3.1, 12 bytes) `«KTX 20»\r\n\x1A\n`, then the **Header**,
//!   nine `u32`s at bytes 12 to 47: `vkFormat` (§3.2), `typeSize` (§3.3),
//!   `pixelWidth`, `pixelHeight` and `pixelDepth` (§3.4), `layerCount`
//!   (§3.5), `faceCount` (§3.6), `levelCount` (§3.7) and
//!   `supercompressionScheme` (§3.8).
//! - **Index** (§3.9, bytes 48 to 79): `dfdByteOffset` and `dfdByteLength`
//!   (§3.9.1, §3.9.2; `u32`), `kvdByteOffset` and `kvdByteLength` (§3.9.3,
//!   §3.9.4; `u32`), `sgdByteOffset` and `sgdByteLength` (§3.9.5, §3.9.6;
//!   `u64`).
//! - **Level Index** (§3.9.7) from byte 80: one entry per level, `byteOffset`,
//!   `byteLength` and `uncompressedByteLength` (`u64` each). `levelCount` 0
//!   means the file holds the base level only, and its Level Index still has
//!   one entry. Level 0 is the largest, listed first; the **Mip Level Array**
//!   (§3.13) itself runs smallest first, so a level's bytes are found by its
//!   entry, never by order.
//! - **Data Format Descriptor** (DFD, §3.10): `dfdTotalSize` (§3.10.4), then
//!   the Khronos Data Format Specification's basic descriptor block, whose
//!   third word starts with the `colorModel` byte. `KHR_DF_MODEL_ETC1S` is 163
//!   and `KHR_DF_MODEL_UASTC` 166 (`khr_df.h`). `vkFormat` 0 says "see the
//!   DFD", which is how a Basis payload is told apart.
//! - **Supercompression Global Data** (SGD, §3.12), for BasisLZ (scheme 1),
//!   laid out in Appendix C: a 20-byte header (`endpointCount` and
//!   `selectorCount` as `u16`, C.1 and C.2, then the `u32` byte lengths of
//!   the endpoints, selectors, tables and extended data, C.3 to C.6), one
//!   20-byte `ImageDesc` per image (C.7: `imageFlags`, then the RGB slice's
//!   and the alpha slice's `byteOffset` and `byteLength`, C.7.2 and C.7.3, as
//!   offsets within their level), then the four blocks (C.8 to C.11). The
//!   blocks' contents are read by the `etc1s` module, from Appendix D.
//!
//! The key/value data (§3.11) is never read, so its offset and length are not
//! checked.
//!
//! # What it refuses, and how
//!
//! `Unsupported`, naming the file's own value: ZLIB (scheme 3, which
//! `KHR_texture_basisu` does not allow either) and any scheme but 0, 1 and 2;
//! cube maps (`faceCount` 6), arrays (`layerCount` past 1), 3D (`pixelDepth`
//! past 0) and 1D (`pixelHeight` 0) textures; any `vkFormat` but 37, 43 and
//! 0; a `vkFormat` 0 colour model but ETC1S and UASTC; a KTX 1.1 file (its
//! identifier is told from 2.0's); and a texture past
//! [`MAX_PIXELS`]. `Malformed`, naming the field: an identifier, a size, a
//! `faceCount`, a Level Index, DFD, SGD or level 0 that does not fit the
//! file, a header that contradicts its DFD, a UASTC block with the reserved
//! mode code or a partition pattern its mode does not define, and a Zstandard
//! level that is not one frame without a dictionary that inflates to exactly
//! the image's size.
//!
//! Every offset and length is summed in `u64` and checked against the file
//! before it is used, and nothing is sized from the header but what the
//! file's own bytes already hold: a level 0 is copied only after its length
//! is known to be exactly the image's. A Zstandard level is the one payload
//! that grows past the bytes it is stored in, so it is inflated under a cap
//! (exactly the image's size, [`inflate`] says how) and its output is what
//! the pixel budget already allows.

use super::MeshError;
use std::ops::Range;

/// The most pixels a texture may have: 64 Mi, 256 MiB of RGBA. It is the one
/// pixel budget of every texture: `normalize::decode_image`'s byte limit for
/// every other format and its packed maps are this too. It is checked from
/// the header, before the Level Index or any data is looked at.
pub const MAX_PIXELS: u64 = 1 << 26;

/// Identifier: `«KTX 20»\r\n\x1A\n`.
const IDENTIFIER: [u8; 12] = [0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A];
/// KTX 1.1's identifier, `«KTX 11»\r\n\x1A\n`: a valid KTX file, named as one.
const KTX1_IDENTIFIER: [u8; 12] = [0xAB, 0x4B, 0x54, 0x58, 0x20, 0x31, 0x31, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A];
/// The identifier, the Header and the Index; the Level Index follows.
const HEADER_LEN: usize = 80;
const LEVEL_ENTRY_LEN: u64 = 24;
const SGD_HEADER_LEN: usize = 20;
/// `dfdTotalSize` and the six words of a basic block before its samples.
const BASIC_BLOCK_MIN: usize = 4 + 24;
const IMAGE_DESC_LEN: u64 = 20;

const VK_FORMAT_UNDEFINED: u32 = 0;
const VK_FORMAT_R8G8B8A8_UNORM: u32 = 37;
const VK_FORMAT_R8G8B8A8_SRGB: u32 = 43;
const KHR_DF_MODEL_ETC1S: u8 = 163;
const KHR_DF_MODEL_UASTC: u8 = 166;

/// An image as RGBA8, 4 bytes a pixel, rows top to bottom with no padding.
#[derive(Clone, PartialEq, Eq)]
pub struct Rgba8 {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for Rgba8 {
    /// The size, not the pixels: an assertion that prints a decode should not
    /// print megabytes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rgba8 {{ {}x{}, {} bytes }}", self.width, self.height, self.pixels.len())
    }
}

/// What the texture's pixels are stored as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ktx2Format {
    /// `VK_FORMAT_R8G8B8A8_UNORM` (`srgb: false`, 37) or `_SRGB` (43).
    Rgba8 { srgb: bool },
    /// Basis Universal ETC1S, always BasisLZ-supercompressed.
    Etc1s,
    /// Basis Universal UASTC LDR 4x4.
    Uastc,
}

/// The file's `supercompressionScheme`, for the three this module reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Supercompression {
    /// Scheme 0.
    None,
    /// Scheme 1, ETC1S's.
    BasisLz,
    /// Scheme 2, Zstandard.
    Zstd,
}

impl Supercompression {
    /// The Header's `supercompressionScheme` value.
    fn scheme(self) -> u32 {
        match self {
            Supercompression::None => 0,
            Supercompression::BasisLz => 1,
            Supercompression::Zstd => 2,
        }
    }
}

/// What [`probe`] reads from a file that parses and is within what this
/// module handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ktx2Info {
    /// The Header's `vkFormat`: 37 or 43, or 0 for a Basis payload.
    pub vk_format: u32,
    pub format: Ktx2Format,
    /// `pixelWidth` and `pixelHeight`: level 0's size.
    pub width: u32,
    pub height: u32,
    /// The Header's `levelCount`, as stored: 0 means the base level only.
    pub level_count: u32,
    pub supercompression: Supercompression,
}

/// A file that parsed: what [`probe`] reports, and the parts of it that the
/// decoders read.
struct Container<'a> {
    info: Ktx2Info,
    /// Level 0's bytes as stored: pixels, or their supercompressed form.
    level0: &'a [u8],
    /// BasisLZ's global data, for scheme 1.
    basislz: Option<BasisLz<'a>>,
}

/// BasisLZ's global data (the SGD), and level 0's image descriptor. The ETC1S
/// decoder reads every field but the extended data, which Appendix C says is
/// "not used for ETC1S": that field's `expect(dead_code)` fails the build with
/// "unfulfilled lint expectation" the day something reads it. The tests read
/// it, hence `not(test)`.
struct BasisLz<'a> {
    endpoint_count: u16,
    selector_count: u16,
    endpoints: &'a [u8],
    selectors: &'a [u8],
    tables: &'a [u8],
    #[cfg_attr(not(test), expect(dead_code, reason = "nothing reads the extended data"))]
    extended: &'a [u8],
    /// Level 0's `imageFlags` (C.7.1). Only `isPFrame` (0x02) is defined; the
    /// decoder refuses an image that has it.
    image_flags: u32,
    /// Level 0's RGB slice, as a range of its bytes.
    rgb: Range<usize>,
    /// Level 0's alpha slice, if it has one (`alphaSliceByteLength` past 0).
    alpha: Option<Range<usize>>,
}

fn malformed(what: String) -> MeshError {
    MeshError::Malformed(format!("KTX2 texture: {what}"))
}

fn unsupported(what: String) -> MeshError {
    MeshError::Unsupported(format!("KTX2 texture: {what}"))
}

/// "1 entry", "2 entries", for the Level Index's messages.
fn entries_of(n: u64) -> String {
    if n == 1 {
        "1 entry".to_string()
    } else {
        format!("{n} entries")
    }
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, MeshError> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| malformed(format!("the file ends before byte {}", at + 2)))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, MeshError> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| malformed(format!("the file ends before byte {}", at + 4)))
}

fn u64_at(b: &[u8], at: usize) -> Result<u64, MeshError> {
    b.get(at..at + 8)
        .map(|s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
        .ok_or_else(|| malformed(format!("the file ends before byte {}", at + 8)))
}

/// `bytes[offset..offset + length]` if it lies inside `bytes`: the sum is made
/// in `u64`, so no pair of values wraps into range.
fn region(bytes: &[u8], offset: u64, length: u64) -> Option<&[u8]> {
    let end = offset.checked_add(length)?;
    bytes.get(usize::try_from(offset).ok()?..usize::try_from(end).ok()?)
}

/// Read the container: the Identifier, Header, Index, Level Index, DFD and, for
/// BasisLZ, the global data. Everything a decoder would size from is checked
/// against the file here.
fn parse(bytes: &[u8]) -> Result<Container<'_>, MeshError> {
    // Identifier. A cut-off identifier is still the identifier, as far as it goes.
    if bytes.starts_with(&KTX1_IDENTIFIER) {
        return Err(unsupported("a KTX 1.1 file is not read, only KTX 2.0".into()));
    }
    if !IDENTIFIER.starts_with(&bytes[..bytes.len().min(IDENTIFIER.len())]) {
        return Err(malformed("the file does not begin with the KTX 2.0 identifier".into()));
    }
    if bytes.len() < HEADER_LEN {
        return Err(malformed(format!("the file is {} bytes, shorter than the {HEADER_LEN} of its identifier, header and index", bytes.len())));
    }

    // Header (bytes 12 to 47) and Index (48 to 79).
    let vk_format = u32_at(bytes, 12)?;
    let (width, height, depth) = (u32_at(bytes, 20)?, u32_at(bytes, 24)?, u32_at(bytes, 28)?);
    let (layers, faces, level_count, scheme) = (u32_at(bytes, 32)?, u32_at(bytes, 36)?, u32_at(bytes, 40)?, u32_at(bytes, 44)?);
    let (dfd_offset, dfd_length) = (u32_at(bytes, 48)?, u32_at(bytes, 52)?);
    let (sgd_offset, sgd_length) = (u64_at(bytes, 64)?, u64_at(bytes, 72)?);

    // The shape: one 2D image per level, or a refusal that names the shape.
    if width == 0 {
        return Err(malformed("pixelWidth is 0".into()));
    }
    match faces {
        1 => {}
        6 => return Err(unsupported("a cube map (faceCount 6) is not decoded, only 2D textures".into())),
        n => return Err(malformed(format!("faceCount is {n}; it is 1, or 6 for a cube map"))),
    }
    // layerCount 0 is "not an array" and 1 a one-element array: both are
    // exactly their layer 0.
    if layers > 1 {
        return Err(unsupported(format!("a texture array (layerCount {layers}) is not decoded, only 2D textures")));
    }
    if depth > 0 {
        return Err(unsupported(format!("a 3D texture (pixelDepth {depth}) is not decoded, only 2D textures")));
    }
    if height == 0 {
        return Err(unsupported("a 1D texture (pixelHeight 0) is not decoded, only 2D textures".into()));
    }
    let supercompression = match scheme {
        0 => Supercompression::None,
        1 => Supercompression::BasisLz,
        2 => Supercompression::Zstd,
        3 => {
            return Err(unsupported(
                "ZLIB supercompression (supercompressionScheme 3) is not decoded; KHR_texture_basisu allows only BasisLZ and Zstandard".into(),
            ))
        }
        n => return Err(unsupported(format!("supercompressionScheme {n} is not decoded"))),
    };

    // The budget, from the header alone: before the Level Index or any byte
    // of data is looked at, and long before any buffer is sized.
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PIXELS {
        return Err(unsupported(format!("a {width}x{height} texture is {pixels} pixels, past the pixel budget of {MAX_PIXELS}")));
    }
    match vk_format {
        VK_FORMAT_R8G8B8A8_UNORM | VK_FORMAT_R8G8B8A8_SRGB | VK_FORMAT_UNDEFINED => {}
        n => {
            return Err(unsupported(format!(
                "vkFormat {n} is not decoded; only R8G8B8A8_UNORM (37), R8G8B8A8_SRGB (43) and Basis Universal textures (vkFormat 0) are"
            )))
        }
    }

    // Level Index: max(levelCount, 1) entries from byte 80, all inside the file.
    let entries = u64::from(level_count.max(1));
    let index_end = HEADER_LEN as u64 + entries * LEVEL_ENTRY_LEN;
    if index_end > bytes.len() as u64 {
        return Err(malformed(format!(
            "levelCount {level_count} needs a level index of {} ending at byte {index_end}, but the file is {} bytes",
            entries_of(entries),
            bytes.len()
        )));
    }

    // DFD and SGD: where the file says they are, if it says they exist.
    let dfd = match dfd_length {
        0 => None,
        n => Some(region(bytes, u64::from(dfd_offset), u64::from(n)).ok_or_else(|| {
            malformed(format!("the data format descriptor (byteOffset {dfd_offset}, byteLength {n}) lies outside the {}-byte file", bytes.len()))
        })?),
    };
    let sgd = match sgd_length {
        0 => None,
        n => Some(region(bytes, sgd_offset, n).ok_or_else(|| {
            malformed(format!("the supercompression global data (byteOffset {sgd_offset}, byteLength {n}) lies outside the {}-byte file", bytes.len()))
        })?),
    };

    // Nothing the index describes may start inside the index: a levelCount
    // larger than the entries the file lists makes the "index" run into the
    // DFD, the global data or the levels.
    let runs_into = |what: &dyn std::fmt::Display, start: u64| {
        malformed(format!(
            "the level index of {} (levelCount {level_count}) ends at byte {index_end}, but the {what} starts at byte {start}",
            entries_of(entries)
        ))
    };
    if dfd.is_some() && u64::from(dfd_offset) < index_end {
        return Err(runs_into(&"data format descriptor", u64::from(dfd_offset)));
    }
    if sgd.is_some() && sgd_offset < index_end {
        return Err(runs_into(&"supercompression global data", sgd_offset));
    }
    for level in 0..entries {
        let at = HEADER_LEN + (level * LEVEL_ENTRY_LEN) as usize;
        let (offset, length) = (u64_at(bytes, at)?, u64_at(bytes, at + 8)?);
        if length > 0 && offset < index_end {
            return Err(runs_into(&format!("level {level}"), offset));
        }
    }

    // Level 0, by its entry's byteOffset and byteLength.
    let (level0_offset, level0_length) = (u64_at(bytes, HEADER_LEN)?, u64_at(bytes, HEADER_LEN + 8)?);
    let level0 = region(bytes, level0_offset, level0_length).ok_or_else(|| {
        malformed(format!("level 0 (byteOffset {level0_offset}, byteLength {level0_length}) lies outside the {}-byte file", bytes.len()))
    })?;

    // What the pixels are stored as.
    let format = match vk_format {
        VK_FORMAT_R8G8B8A8_UNORM => Ktx2Format::Rgba8 { srgb: false },
        VK_FORMAT_R8G8B8A8_SRGB => Ktx2Format::Rgba8 { srgb: true },
        _ => basis_format(dfd, supercompression)?,
    };
    if supercompression == Supercompression::BasisLz && format != Ktx2Format::Etc1s {
        return Err(malformed(format!("BasisLZ supercompression holds ETC1S and needs vkFormat 0, but vkFormat is {vk_format}")));
    }
    let basislz = match supercompression {
        // One image per level: layers, faces and depth are 1 here.
        Supercompression::BasisLz => Some(basislz_global_data(sgd, level0, entries)?),
        _ => None,
    };

    let info = Ktx2Info { vk_format, format, width, height, level_count, supercompression };
    Ok(Container { info, level0, basislz })
}

/// A `vkFormat` 0 file's kind, from the DFD's colour model and the scheme.
fn basis_format(dfd: Option<&[u8]>, scheme: Supercompression) -> Result<Ktx2Format, MeshError> {
    let Some(dfd) = dfd else {
        return Err(malformed("vkFormat 0 names the format in the data format descriptor, which this file lacks".into()));
    };
    // dfdTotalSize (4 bytes), then the basic block's six header words: the
    // third starts with colorModel. A block shorter than that is not one.
    let model = match dfd.get(12) {
        Some(&model) if dfd.len() >= BASIC_BLOCK_MIN => model,
        _ => {
            return Err(malformed(format!(
                "the data format descriptor is {} bytes, shorter than the {BASIC_BLOCK_MIN} of dfdTotalSize and a basic block's header",
                dfd.len()
            )))
        }
    };
    match (model, scheme) {
        (KHR_DF_MODEL_ETC1S, Supercompression::BasisLz) => Ok(Ktx2Format::Etc1s),
        (KHR_DF_MODEL_UASTC, Supercompression::None | Supercompression::Zstd) => Ok(Ktx2Format::Uastc),
        (KHR_DF_MODEL_ETC1S, other) => Err(malformed(format!(
            "the data format descriptor's colour model is ETC1S (163), which needs BasisLZ supercompression (supercompressionScheme 1), but supercompressionScheme is {}",
            other.scheme()
        ))),
        (KHR_DF_MODEL_UASTC, Supercompression::BasisLz) => {
            Err(malformed("BasisLZ supercompression holds ETC1S, but the data format descriptor's colour model is UASTC (166)".into()))
        }
        (other, _) => Err(unsupported(format!("vkFormat 0 with colour model {other} is not decoded; only ETC1S (163) and UASTC (166) are"))),
    }
}

/// BasisLZ's global data: its header, the `images` image descriptors, and the
/// four blocks, each checked against the bytes the SGD holds; then level 0's
/// descriptor (the first), whose slices must lie inside level 0.
fn basislz_global_data<'a>(sgd: Option<&'a [u8]>, level0: &[u8], images: u64) -> Result<BasisLz<'a>, MeshError> {
    let Some(sgd) = sgd else {
        return Err(malformed("BasisLZ supercompression needs supercompression global data, which this file lacks".into()));
    };
    if sgd.len() < SGD_HEADER_LEN {
        return Err(malformed(format!(
            "the supercompression global data is {} bytes, shorter than BasisLZ's {SGD_HEADER_LEN}-byte header",
            sgd.len()
        )));
    }
    let (endpoint_count, selector_count) = (u16_at(sgd, 0)?, u16_at(sgd, 2)?);
    let lengths = [u32_at(sgd, 4)?, u32_at(sgd, 8)?, u32_at(sgd, 12)?, u32_at(sgd, 16)?].map(u64::from);

    let descs_end = SGD_HEADER_LEN as u64 + images * IMAGE_DESC_LEN;
    if descs_end > sgd.len() as u64 {
        return Err(malformed(format!(
            "{images} image descriptors need {descs_end} bytes, but the supercompression global data holds {}",
            sgd.len()
        )));
    }
    let declared = descs_end + lengths.iter().sum::<u64>();
    if declared > sgd.len() as u64 {
        return Err(malformed(format!(
            "the supercompression global data declares {declared} bytes (its header, {images} image descriptors and four blocks of {lengths:?}), but holds {}",
            sgd.len()
        )));
    }
    let mut at = descs_end;
    let mut block = |len: u64| {
        let taken = region(sgd, at, len).ok_or_else(|| malformed("a block of the supercompression global data lies outside it".into()));
        at += len;
        taken
    };
    let [endpoints, selectors, tables, extended] = [block(lengths[0])?, block(lengths[1])?, block(lengths[2])?, block(lengths[3])?];

    // Level 0's descriptor: its flags, then its slices, which are offsets within level 0.
    let image_flags = u32_at(sgd, SGD_HEADER_LEN)?;
    let (rgb_offset, rgb_length) = (u32_at(sgd, SGD_HEADER_LEN + 4)?, u32_at(sgd, SGD_HEADER_LEN + 8)?);
    let (alpha_offset, alpha_length) = (u32_at(sgd, SGD_HEADER_LEN + 12)?, u32_at(sgd, SGD_HEADER_LEN + 16)?);
    let slice = |name: &str, offset: u32, length: u32| {
        let end = u64::from(offset) + u64::from(length);
        if end > level0.len() as u64 {
            return Err(malformed(format!(
                "level 0's {name} slice (byteOffset {offset}, byteLength {length}) lies outside level 0, which is {} bytes",
                level0.len()
            )));
        }
        Ok(offset as usize..end as usize)
    };
    if rgb_length == 0 {
        return Err(malformed("level 0 has no RGB slice (rgbSliceByteLength 0)".into()));
    }
    let rgb = slice("RGB", rgb_offset, rgb_length)?;
    let alpha = if alpha_length == 0 { None } else { Some(slice("alpha", alpha_offset, alpha_length)?) };

    Ok(BasisLz { endpoint_count, selector_count, endpoints, selectors, tables, extended, image_flags, rgb, alpha })
}

/// What a file is, without decoding it: the format, the size, the level count
/// and the supercompression. It refuses what [`decode_rgba8`] would refuse
/// from the container alone (a shape, a scheme, a size past [`MAX_PIXELS`], a
/// `vkFormat`), and finds a file malformed where the container is.
pub fn probe(bytes: &[u8]) -> Result<Ktx2Info, MeshError> {
    parse(bytes).map(|c| c.info)
}

/// Level 0, layer 0, face 0 as RGBA8, the stored values with no colour
/// conversion. Uncompressed `VK_FORMAT_R8G8B8A8_UNORM` and `_SRGB` (also under
/// Zstandard), ETC1S/BasisLZ (an ETC1S P-frame is `Unsupported`, named) and
/// UASTC LDR 4x4 (stored plain or under Zstandard) decode here.
pub fn decode_rgba8(bytes: &[u8]) -> Result<Rgba8, MeshError> {
    let container = parse(bytes)?;
    let Ktx2Info { width, height, format, supercompression, .. } = container.info;
    match (format, supercompression) {
        (Ktx2Format::Rgba8 { .. }, Supercompression::None) => {
            // The budget held, so this is at most 256 MiB, and the level
            // exactly that long is already in the file.
            let expected = u64::from(width) * u64::from(height) * 4;
            if container.level0.len() as u64 != expected {
                return Err(malformed(format!(
                    "level 0 holds {} bytes, but a {width}x{height} RGBA8 image is {expected}",
                    container.level0.len()
                )));
            }
            Ok(Rgba8 { width, height, pixels: container.level0.to_vec() })
        }
        (Ktx2Format::Rgba8 { .. }, Supercompression::Zstd) => {
            let pixels = inflate(container.level0, u64::from(width) * u64::from(height) * 4, &format!("{width}x{height} RGBA8 image"))?;
            Ok(Rgba8 { width, height, pixels })
        }
        (Ktx2Format::Etc1s, _) => match container.basislz.as_ref() {
            // `parse` gives BasisLZ files their global data.
            Some(global) => etc1s::decode(width, height, container.level0, global),
            None => Err(malformed("BasisLZ supercompression needs supercompression global data, which this file lacks".into())),
        },
        (Ktx2Format::Uastc, Supercompression::Zstd) => {
            let blocks = inflate(container.level0, uastc::level_len(width, height), &format!("{width}x{height} UASTC image"))?;
            uastc::decode(width, height, &blocks)
        }
        (Ktx2Format::Uastc, _) => uastc::decode(width, height, container.level0),
        // `parse` refuses BasisLZ with anything but ETC1S.
        (Ktx2Format::Rgba8 { .. }, Supercompression::BasisLz) => Err(malformed("BasisLZ supercompression holds ETC1S, not RGBA8".into())),
    }
}

/// The least window a Zstandard frame may name, whatever the level's size: 8
/// MiB. It is there for frames written without being told the size of what
/// they compress (libzstd's streaming API, `ZSTD_compressStream`): such a
/// frame names its compression level's default window, which is up to 8 MiB
/// for the levels to 19, whatever the content. A frame written knowing the size
/// (`ZSTD_compress`, which libktx and basisu call) is single-segment and names
/// the content's size as its window, so a level bigger than the floor needs
/// the limit to be its own size: [`inflate_reading`] takes the greater.
const ZSTD_WINDOW_FLOOR: u64 = 8 << 20;

/// Level 0 under Zstandard supercompression (`supercompressionScheme` 2, KTX2
/// §3.8.3): one Zstandard frame (RFC 8878), inflated to exactly `expected`
/// bytes, which is what a `image` of that format takes (the caller has checked
/// it against the pixel budget). Anything else is `Malformed`, naming what.
///
/// This is the one place the module uses the `ruzstd` crate (the user's
/// decision D3 in mesh-gaps item 3): a pure-Rust decoder, pinned exactly,
/// which has `unsafe` inside but is used here through its safe, streaming API
/// alone, and which, with the features it is built with, has no dependencies.
fn inflate(frame: &[u8], expected: u64, image: &str) -> Result<Vec<u8>, MeshError> {
    let mut source = frame;
    // The budget held, so `expected` is at most 256 MiB.
    inflate_reading(&mut source, expected as usize, image)
}

/// [`inflate`] over any reader, so a test can see how much of it was taken.
///
/// - **The output is reserved once, at its size.** `out` gets
///   `try_reserve_exact(expected)` after the frame's header is accepted, and
///   is filled by a read capped at `expected` bytes, so it never grows: what
///   is returned is exactly the level (as `Rgba8.pixels` for an RGBA8 level),
///   with nothing kept past it. A size that cannot be set aside is
///   `Unsupported`, naming it, and not a panic in the allocator (an abort on
///   wasm32). That covers `out` only: ruzstd's own ring buffer still panics if
///   its allocation fails. A hostile header can make this reserve `expected`,
///   at most the pixel budget's 256 MiB, but pages are not touched until a
///   byte is written.
/// - **At most `expected` bytes come out, and the cap does not bound ruzstd's
///   work or memory by itself.** After the level's `expected` bytes, one more
///   byte is asked for; a frame that has it goes past the level and is
///   `Malformed`. But ruzstd decodes into a ring buffer that holds a whole
///   window, and gives out nothing until it has decoded past one, so it runs
///   up to a window (and one block) ahead of the cap; a frame that goes on
///   past the level is cut off after about that much, not at once, and the
///   rest of it is not read. ruzstd's ring buffer grows by doubling to a power
///   of two above the window (so up to about twice the window), and holds the
///   old and new halves while it grows. So the peak is `out` (the level) plus
///   that ring: measured with a counting allocator (single-segment frames,
///   window = level) at 2.5× the level for a power-of-two level and up to
///   about 4× for a level just past one (3.98× at 129 MiB; 2.92× at 200 MiB,
///   584.5 MiB). That is ruzstd's design, and it is bounded by the pixel
///   budget; it is not `expected + 1`.
/// - **The window is bounded.** Because of that, the window the header names
///   is limited before a block is read: to [`ZSTD_WINDOW_FLOOR`] or
///   `expected`, whichever is more (the floor is for frames of unknown size,
///   its doc says why). A frame past it is `Malformed`, by its window. A
///   window past the level's size buys a decoder nothing, as nothing can be
///   copied from before the first byte of the level.
/// - **No dictionary.** A KTX2 level is a frame by itself. A frame that names
///   a dictionary (a `Dictionary_ID` other than 0, which means none) cannot be
///   decoded without it, and no dictionary is ever supplied, so it is refused,
///   by its number.
/// - **One frame, to its end.** Bytes after the frame (a second frame, a
///   skippable frame, padding) are `Malformed`: `ZSTD_decompress` takes a
///   second frame as more output, which would break the exact length, and the
///   level's `byteLength` is the frame's. A skippable frame first is not a
///   frame for the same reason. A frame's 4 checksum bytes, if its header says
///   it has them, are part of it.
/// - **The declared size is checked first.** A frame that states its content
///   size (all that `ZSTD_compress` writes) and states another than the
///   level's is refused before a block is decoded.
/// - **The checksum is not checked.** `ruzstd`'s checksum needs its `hash`
///   feature, which is its one dependency and is off here. The frame's end,
///   its block structure and the exact length are checked, and a level
///   corrupted past those decodes to a wrong picture, not to a crash, the same
///   as any codec without a checksum.
fn inflate_reading(source: &mut impl std::io::Read, expected: usize, image: &str) -> Result<Vec<u8>, MeshError> {
    use ruzstd::decoding::errors::FrameDecoderError;
    use ruzstd::decoding::StreamingDecoder;
    use std::io::Read;

    let window_limit = (expected as u64).max(ZSTD_WINDOW_FLOOR);
    let mut decoder = match StreamingDecoder::new_with_max_window_size(&mut *source, window_limit) {
        Ok(decoder) => decoder,
        Err(FrameDecoderError::DictNotProvided { dict_id }) => {
            return Err(malformed(format!("level 0's Zstandard frame names dictionary {dict_id}, and a KTX2 level carries none")));
        }
        Err(FrameDecoderError::WindowSizeTooBig { requested, max }) => {
            return Err(malformed(format!("level 0's Zstandard frame declares a window of {requested} bytes, past the {max} a {image} is allowed")));
        }
        Err(e) => return Err(malformed(format!("level 0 is not a Zstandard frame: {e}"))),
    };
    let declared = decoder.decoder.content_size();
    // 0 is both "not stated" and a stated 0, and a stated 0 is caught by the length.
    if declared != 0 && declared != expected as u64 {
        return Err(malformed(format!("level 0's Zstandard frame declares {declared} bytes of content, but a {image} is {expected}")));
    }

    let mut out = Vec::new();
    out.try_reserve_exact(expected)
        .map_err(|_| unsupported(format!("level 0 inflates to {expected} bytes (a {image}), more than can be set aside")))?;
    // Exactly `expected` bytes fit, so `read_to_end` ends on a probe that finds the cap, never growing `out`.
    decoder
        .by_ref()
        .take(expected as u64)
        .read_to_end(&mut out)
        .map_err(|e| malformed(format!("level 0's Zstandard data does not decode: {e}")))?;
    if out.len() < expected {
        return Err(malformed(format!("level 0 inflates to {} bytes, but a {image} is {expected} (from its Zstandard frame)", out.len())));
    }
    // The level is whole; a frame with one byte more goes past it.
    match decoder.read(&mut [0u8; 1]) {
        Ok(0) => {}
        Ok(_) => return Err(malformed(format!("level 0 inflates to more than the {expected} bytes of a {image} (its Zstandard frame goes on past them)"))),
        Err(e) => return Err(malformed(format!("level 0's Zstandard data does not decode: {e}"))),
    }
    drop(decoder);
    let after = std::io::copy(source, &mut std::io::sink()).map_err(|e| malformed(format!("level 0: {e}")))?;
    if after > 0 {
        return Err(malformed(format!("level 0's Zstandard frame ends, but {after} bytes follow it; the level is one frame")));
    }
    Ok(out)
}

/// ETC1S/BasisLZ to RGBA8: the KTX2 specification's Appendix D (BasisLZ/ETC1S
/// Bitstream Specification), checked against Basis Universal v1.50's
/// `basisu_transcoder.cpp` and `basisu_transcoder_internal.h`, the reference
/// that the committed `.rgba` files come from (the tags `v1_50_0` and
/// `v1_50_0_2` hold identical transcoder sources). Where Appendix D and the
/// transcoder are read differently, the transcoder's output is what is
/// matched, and the difference is said at the place.
///
/// What is decoded, and from where:
///
/// - **Huffman tables** (D.1): `huffman_decoding_table::init` and
///   `bitwise_decoder::read_huffman_table`. A table is complete (its codes
///   fill the code space exactly) or holds one symbol; anything else is
///   `Malformed`.
/// - **The endpoint codebook** (D.2): `basisu_lowlevel_etc1s_transcoder::
///   decode_palettes`, first half. **The selector codebook** (D.3): its
///   second half.
/// - **The slice tables** (D.4): `decode_tables`.
/// - **A slice** (D.5): `transcode_slice`, the part that decodes the two
///   indices of each block, with the ETC1S still-image case only: the
///   P-frame rule for animations (D.5.3's `is_video`) needs the previous
///   frame, so a level 0 whose `imageFlags` has `isPFrame` (C.7.1) is
///   `Unsupported`, named, and the key/value data (`KTXanimData`) is never
///   read. The other `imageFlags` bits are undefined and ignored.
///   A selector run longer than the whole slice is `Malformed`, as in the
///   reference; one that outlasts the blocks left is cut off at the end of the
///   slice, as there.
/// - **RGBA8** (`transcode_slice`'s `cRGB32`, `cRGBA32` and `cA32` cases,
///   as `ktx2_transcoder::transcode_image_level` and
///   `basisu_lowlevel_etc1s_transcoder::transcode_image` call them for
///   `cTFRGBA32`): a block's four colours are its 5-bit base colour
///   expanded to 8 bits (`c << 3 | c >> 2`) plus the four modifiers of its
///   intensity table (`g_etc1_inten_tables`), clamped; each pixel takes
///   the colour its 2-bit selector names. Both ETC1 sub-blocks of an ETC1S
///   block have the same colour, so the sub-block flip bit makes no
///   difference. The alpha slice decodes the same way, and a pixel's alpha is
///   the green channel of the colour its selector names in the alpha slice.
///
/// **Alpha** is decided by the alpha slice alone: a level 0 whose image
/// descriptor has `alphaSliceByteLength` 0 is opaque (255), whatever the DFD
/// says, and one that has an alpha slice takes its alpha from it. The
/// reference transcoder decides by the DFD's byte length instead
/// (`m_has_alpha = (m_header.m_dfd_byte_length == 60)`: 60 bytes is a DFD with
/// an alpha sample, 44 one without). It fails a file whose DFD declares alpha
/// and whose slice is empty, and ignores an alpha slice a DFD does not
/// declare; both agree on every file an encoder writes, and the DFD is never
/// read here to change a byte.
///
/// **Reading past the end is an error.** The reference reads zeros past the
/// end of a stream and carries on; this decoder reports `Malformed`, so a
/// truncated stream never decodes to a picture.
mod etc1s {
    use super::{malformed, unsupported, BasisLz, MeshError, Rgba8};

    /// `g_etc1_inten_tables`: the four modifiers of each of the 8 intensity
    /// tables, in selector order (selector 0 is the darkest).
    const INTEN: [[i32; 4]; 8] =
        [[-8, -2, 2, 8], [-17, -5, 5, 17], [-29, -9, 9, 29], [-42, -13, 13, 42], [-60, -18, 18, 60], [-80, -24, 24, 80], [-106, -33, 33, 106], [-183, -47, 47, 183]];
    /// D.1's `codes_order`: the order in which the code length codes' own
    /// sizes are sent.
    const CODES_ORDER: [usize; 21] = [17, 18, 19, 20, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15, 16];
    /// D.5.3: 4 * 4 * 4 * 4 predictor symbols, then this one: "repeat the last".
    const ENDPOINT_PRED_REPEAT_LAST_SYMBOL: u32 = 256;
    const ENDPOINT_PRED_MIN_REPEAT_COUNT: u32 = 3;
    const SELECTOR_HISTORY_BUF_RLE_COUNT_THRESH: u32 = 3;
    /// D.5.3: `SELECTOR_HISTORY_BUF_RLE_COUNT_TOTAL - 1`: the run symbol that
    /// is followed by a variable-length count.
    const SELECTOR_RLE_LONG_RUN: u32 = 63;
    /// C.7.1: the one flag `imageFlags` defines.
    const IS_P_FRAME: u32 = 0x02;

    /// A reader of an Appendix D bitstream: bits fill each byte from its least
    /// significant end (`bitwise_decoder`). `what` names the stream in errors.
    pub(super) struct Bits<'a> {
        data: &'a [u8],
        /// The next bit to read, counted from the start of `data`.
        pos: usize,
        what: &'static str,
    }

    impl<'a> Bits<'a> {
        pub(super) fn new(data: &'a [u8], what: &'static str) -> Bits<'a> {
            Bits { data, pos: 0, what }
        }

        fn ended(&self) -> MeshError {
            malformed(format!("{} ends before everything it declares is decoded", self.what))
        }

        /// The next 16 bits, not taken. Zero past the end: [`Bits::skip`] is
        /// what refuses a code that reaches past it.
        fn peek16(&self) -> u32 {
            let byte = self.pos / 8;
            let mut window = 0u32;
            for i in 0..3 {
                window |= u32::from(self.data.get(byte + i).copied().unwrap_or(0)) << (8 * i);
            }
            (window >> (self.pos % 8)) & 0xFFFF
        }

        fn skip(&mut self, n: u32) -> Result<(), MeshError> {
            let end = self.pos + n as usize;
            if end > self.data.len() * 8 {
                return Err(self.ended());
            }
            self.pos = end;
            Ok(())
        }

        /// `get_bits(n)`, up to 32 bits.
        pub(super) fn bits(&mut self, n: u32) -> Result<u32, MeshError> {
            debug_assert!(n <= 32);
            let end = self.pos + n as usize;
            if end > self.data.len() * 8 {
                return Err(self.ended());
            }
            let (byte, shift) = (self.pos / 8, self.pos % 8);
            let mut window = 0u64;
            for i in 0..5 {
                window |= u64::from(self.data.get(byte + i).copied().unwrap_or(0)) << (8 * i);
            }
            self.pos = end;
            Ok(((window >> shift) & ((1u64 << n) - 1)) as u32)
        }

        /// D.5.2's `decode_vlc4` (`chunk_bits` 4) and `decode_vlc7` (7): chunks
        /// of `chunk_bits` + 1 bits whose top bit says another follows. The
        /// specification breaks out of a code that goes on past 32 bits and
        /// carries on; this reports it.
        pub(super) fn vlc(&mut self, chunk_bits: u32) -> Result<u32, MeshError> {
            let (mask, more) = ((1u32 << chunk_bits) - 1, 1u32 << chunk_bits);
            let (mut value, mut offset) = (0u32, 0u32);
            loop {
                let chunk = self.bits(chunk_bits + 1)?;
                value |= (chunk & mask) << offset;
                offset += chunk_bits;
                if chunk & more == 0 {
                    return Ok(value);
                }
                if offset >= 32 {
                    return Err(malformed(format!("{} holds a variable-length code that does not end within 32 bits", self.what)));
                }
            }
        }
    }

    /// A canonical Huffman decoding table (RFC 1951, 3.2.2; D.1), as the
    /// count of codes of each length and the symbols sorted by (length,
    /// symbol), which is all a bit-at-a-time decoder needs. A table with no
    /// symbol is "not there" (`total_used_syms` 0), and `decode` is not
    /// called on it.
    #[derive(Debug)]
    pub(super) struct Huffman {
        /// `counts[l]`: how many symbols have a code `l` bits long (`l` 1 to 16).
        counts: [u32; 17],
        symbols: Vec<u16>,
    }

    impl Huffman {
        /// The table for these code lengths (0: the symbol is unused). The
        /// codes must fill the code space exactly, or there must be a single
        /// used symbol: `huffman_decoding_table::init` accepts nothing else
        /// (an oversubscribed or incomplete code is "can't create a valid
        /// prefix code"). It sums the code space in 32 bits, where six 1-bit
        /// codes wrap to exactly a full space and are caught only later by a
        /// collision; this sums without wrapping, so they are refused at once.
        pub(super) fn from_lengths(lengths: &[u8]) -> Result<Huffman, MeshError> {
            let mut counts = [0u32; 17];
            for &len in lengths {
                if len > 16 {
                    return Err(malformed(format!("a Huffman code length of {len} bits is past the 16 allowed")));
                }
                counts[usize::from(len)] += 1;
            }
            counts[0] = 0;
            if lengths.is_empty() {
                return Ok(Huffman { counts, symbols: Vec::new() });
            }
            let used: u32 = counts.iter().sum();
            // The code space is 2^16 units; a code of length l takes 2^(16 - l).
            let space: u64 = (1..=16).map(|len| u64::from(counts[len]) << (16 - len)).sum();
            if space != 1 << 16 && used != 1 {
                let how = if space > 1 << 16 { "oversubscribed" } else { "incomplete" };
                return Err(malformed(format!("a Huffman table's code lengths are {how}: {used} symbols use {space} of 65536 code units")));
            }
            let mut next = [0usize; 17];
            for len in 1..16 {
                next[len + 1] = next[len] + counts[len] as usize;
            }
            let mut symbols = vec![0u16; used as usize];
            for (symbol, &len) in lengths.iter().enumerate() {
                if len > 0 {
                    symbols[next[usize::from(len)]] = symbol as u16;
                    next[usize::from(len)] += 1;
                }
            }
            Ok(Huffman { counts, symbols })
        }

        /// A Huffman table as D.1 sends it: `total_used_syms` (14 bits), the
        /// sizes of the code length codes, then the code lengths, with runs
        /// of zeros and repeats. `read_huffman_table`.
        pub(super) fn read(bits: &mut Bits) -> Result<Huffman, MeshError> {
            let total = bits.bits(14)? as usize;
            if total == 0 {
                return Ok(Huffman { counts: [0; 17], symbols: Vec::new() });
            }
            let n = bits.bits(5)? as usize;
            if !(1..=CODES_ORDER.len()).contains(&n) {
                return Err(malformed(format!("a Huffman table has {n} code length codes, outside 1 to {}", CODES_ORDER.len())));
            }
            let mut sizes = [0u8; 21];
            for &symbol in &CODES_ORDER[..n] {
                sizes[symbol] = bits.bits(3)? as u8;
            }
            let code_lengths = Huffman::from_lengths(&sizes)?;

            let mut lengths = vec![0u8; total];
            let mut cur = 0usize;
            while cur < total {
                match code_lengths.decode(bits)? {
                    len @ 0..=16 => {
                        lengths[cur] = len as u8;
                        cur += 1;
                    }
                    // A run of zeros may overshoot the table; the length check below refuses it.
                    17 => cur += bits.bits(3)? as usize + 3,
                    18 => cur += bits.bits(7)? as usize + 11,
                    repeat => {
                        let times = if repeat == 19 { bits.bits(2)? + 3 } else { bits.bits(7)? + 7 } as usize;
                        let prev = match cur.checked_sub(1) {
                            Some(at) => lengths[at],
                            None => return Err(malformed("a Huffman table repeats a code length before it has sent one".into())),
                        };
                        if prev == 0 {
                            return Err(malformed("a Huffman table repeats a code length of 0".into()));
                        }
                        if cur + times > total {
                            return Err(malformed(format!("a Huffman table repeats a code length {times} times past its {total} symbols")));
                        }
                        lengths[cur..cur + times].fill(prev);
                        cur += times;
                    }
                }
            }
            if cur != total {
                return Err(malformed(format!("a Huffman table's code lengths run to {cur} symbols, but it declares {total}")));
            }
            Huffman::from_lengths(&lengths)
        }

        /// The next symbol: the code is read one bit at a time, first bit
        /// first (D.1: Deflate's canonical codes, packed starting with their
        /// most significant bit).
        pub(super) fn decode(&self, bits: &mut Bits) -> Result<u32, MeshError> {
            let window = bits.peek16();
            let (mut code, mut first, mut index) = (0u32, 0u32, 0usize);
            for len in 1..=16usize {
                code |= (window >> (len - 1)) & 1;
                let count = self.counts[len];
                if code < first + count {
                    bits.skip(len as u32)?;
                    return match self.symbols.get(index + (code - first) as usize) {
                        Some(&symbol) => Ok(u32::from(symbol)),
                        None => Err(malformed("a Huffman code names a symbol its table does not have".into())),
                    };
                }
                index += count as usize;
                first = (first + count) << 1;
                code <<= 1;
            }
            Err(malformed(format!("{} holds a bit sequence that is no code of its Huffman table", bits.what)))
        }

        pub(super) fn is_empty(&self) -> bool {
            self.symbols.is_empty()
        }
    }

    /// The four Huffman tables and the history size that every slice of a
    /// file is coded with (D.4).
    pub(super) struct Models {
        pub(super) endpoint_pred: Huffman,
        pub(super) endpoint_delta: Huffman,
        pub(super) selector: Huffman,
        pub(super) selector_rle: Huffman,
        /// `selectorHistoryBufSize`: slots in the selector history buffer.
        pub(super) history: usize,
    }

    /// A table the stream needs must have symbols.
    fn nonempty(table: Huffman, what: &str) -> Result<Huffman, MeshError> {
        if table.is_empty() {
            return Err(malformed(format!("{what} is an empty Huffman table")));
        }
        Ok(table)
    }

    /// D.4, `decode_tables`: the endpoint predictor, endpoint delta, selector
    /// and selector run tables, then the history size. The history cannot be
    /// empty: the reference refuses 0.
    fn read_models(tables: &[u8]) -> Result<Models, MeshError> {
        let mut bits = Bits::new(tables, "the slice Huffman tables");
        let endpoint_pred = nonempty(Huffman::read(&mut bits)?, "the endpoint predictor table")?;
        let endpoint_delta = nonempty(Huffman::read(&mut bits)?, "the endpoint delta table")?;
        let selector = nonempty(Huffman::read(&mut bits)?, "the selector table")?;
        let selector_rle = nonempty(Huffman::read(&mut bits)?, "the selector run table")?;
        let history = bits.bits(13)? as usize;
        if history == 0 {
            return Err(malformed("the selector history buffer size is 0".into()));
        }
        Ok(Models { endpoint_pred, endpoint_delta, selector, selector_rle, history })
    }

    /// One entry of the endpoint codebook, as the four colours it gives a
    /// block: the 5-bit base colour expanded to 8 bits, plus each modifier of
    /// its intensity table, clamped (`decoder_etc_block::get_block_colors5`).
    struct Endpoint {
        colors: [[u8; 3]; 4],
    }

    impl Endpoint {
        fn new(color5: [u8; 3], inten: usize) -> Endpoint {
            let base = color5.map(|c| i32::from((c << 3) | (c >> 2)));
            let colors = INTEN[inten].map(|modifier| base.map(|c| (c + modifier).clamp(0, 255) as u8));
            Endpoint { colors }
        }
    }

    /// D.2, `decode_palettes`' first half: four Huffman tables, a grayscale
    /// flag, then for each endpoint an intensity table delta and the colour
    /// deltas, each model chosen by the previous value of its channel.
    fn read_endpoints(count: usize, data: &[u8]) -> Result<Vec<Endpoint>, MeshError> {
        // An endpoint is an intensity delta and at least one colour delta, each
        // at least one bit, so the block holds at most four endpoints a byte.
        if count > data.len() * 4 {
            return Err(malformed(format!("the endpoint codebook holds {} bytes, too few for {count} endpoints", data.len())));
        }
        let mut bits = Bits::new(data, "the endpoint codebook");
        let color_models = [
            nonempty(Huffman::read(&mut bits)?, "the endpoint codebook's first colour delta table")?,
            nonempty(Huffman::read(&mut bits)?, "the endpoint codebook's second colour delta table")?,
            nonempty(Huffman::read(&mut bits)?, "the endpoint codebook's third colour delta table")?,
        ];
        let inten_model = nonempty(Huffman::read(&mut bits)?, "the endpoint codebook's intensity delta table")?;
        let grayscale = bits.bits(1)? == 1;

        let mut endpoints = Vec::with_capacity(count);
        let (mut prev_color, mut prev_inten) = ([16u32; 3], 0u32);
        for _ in 0..count {
            let inten = (inten_model.decode(&mut bits)? + prev_inten) & 7;
            prev_inten = inten;
            let mut color5 = [0u8; 3];
            for channel in 0..if grayscale { 1 } else { 3 } {
                // colorDeltaModel0 for 0 to 9, 1 for 10 to 21, 2 for 22 to 31.
                let model = match prev_color[channel] {
                    0..=9 => &color_models[0],
                    10..=21 => &color_models[1],
                    _ => &color_models[2],
                };
                let value = (prev_color[channel] + model.decode(&mut bits)?) & 31;
                prev_color[channel] = value;
                color5[channel] = value as u8;
            }
            if grayscale {
                color5 = [color5[0]; 3];
            }
            endpoints.push(Endpoint::new(color5, inten as usize));
        }
        Ok(endpoints)
    }

    /// A selector entry: four rows of four 2-bit values, the left pixel in
    /// the least significant bits of each row byte (D.3.2).
    pub(super) type Selector = [u8; 4];

    /// The 2-bit selector of pixel `x` of a row.
    fn selector_at(row: u8, x: usize) -> usize {
        usize::from(row >> (2 * x)) & 3
    }

    /// D.3, `decode_palettes`' second half: two reserved bits that must be 0
    /// (the reference's global and hybrid selector codebooks, which KTX2 does
    /// not have), the raw flag, then either every row byte as is or a delta
    /// table, the first entry's rows, and each later row as an XOR with the
    /// row before it.
    pub(super) fn read_selectors(count: usize, data: &[u8]) -> Result<Vec<Selector>, MeshError> {
        // The first entry takes 32 bits and each later one four delta codes of
        // at least a bit (a raw entry takes 32), so the block holds at most two
        // selectors a byte.
        if count > data.len() * 2 {
            return Err(malformed(format!("the selector codebook holds {} bytes, too few for {count} selectors", data.len())));
        }
        let mut bits = Bits::new(data, "the selector codebook");
        if bits.bits(2)? != 0 {
            return Err(malformed("the selector codebook's two reserved bits are not 0 (a global selector codebook, which KTX2 does not have)".into()));
        }
        let raw = bits.bits(1)? == 1;
        let mut selectors = Vec::with_capacity(count);
        if raw {
            for _ in 0..count {
                let mut rows = [0u8; 4];
                for row in &mut rows {
                    *row = bits.bits(8)? as u8;
                }
                selectors.push(rows);
            }
            return Ok(selectors);
        }
        let delta_model = Huffman::read(&mut bits)?;
        if count > 1 && delta_model.is_empty() {
            return Err(malformed("the selector codebook's delta table is an empty Huffman table".into()));
        }
        let mut rows = [0u8; 4];
        for i in 0..count {
            for row in &mut rows {
                *row = if i == 0 { bits.bits(8)? as u8 } else { *row ^ delta_model.decode(&mut bits)? as u8 };
            }
            selectors.push(rows);
        }
        Ok(selectors)
    }

    /// D.5.1's approximate move to front: `add` puts a value at the rover,
    /// `use_slot` swaps a used slot with the one half way to the front.
    struct History {
        values: Vec<usize>,
        rover: usize,
    }

    impl History {
        fn new(n: usize) -> History {
            History { values: vec![0; n], rover: n / 2 }
        }
        fn add(&mut self, value: usize) {
            self.values[self.rover] = value;
            self.rover += 1;
            if self.rover == self.values.len() {
                self.rover = self.values.len() / 2;
            }
        }
        fn use_slot(&mut self, slot: usize) {
            self.values.swap(slot / 2, slot);
        }
    }

    /// Decode one slice's grid of `blocks` (D.5.3, `transcode_slice`), calling
    /// `emit(block_x, block_y, endpoint_index, selector_index)` for every
    /// block in raster order. The indices are within `endpoints` and
    /// `selectors`, the codebook sizes (`endpoints` is at most 65536: an
    /// endpoint index is 16 bits in the file). A stream that predicts from
    /// outside the image, names an entry that does not exist, has a selector
    /// run longer than the whole slice, leaves an endpoint predictor repeat
    /// unfinished at the end, or ends early is `Malformed`. A selector run
    /// that is no longer than the slice but outlasts the blocks left is cut
    /// off at the end of the slice, as in the reference (it refuses a run
    /// only when `cur_selector_rle_count > total_blocks`).
    pub(super) fn decode_slice(
        data: &[u8],
        (blocks_x, blocks_y): (usize, usize),
        models: &Models,
        endpoints: usize,
        selectors: usize,
        mut emit: impl FnMut(usize, usize, usize, usize),
    ) -> Result<(), MeshError> {
        let mut bits = Bits::new(data, "the slice");
        let total_blocks = blocks_x * blocks_y;
        let rle_symbol = selectors + models.history;
        let mut history = History::new(models.history);

        // The predictor bits for the second row of each 2x2 group, and the
        // endpoint indices of the row above and of this row.
        let mut group_below = vec![0u8; blocks_x.div_ceil(2)];
        debug_assert!(endpoints <= 1 << 16, "an endpoint index is 16 bits in the file");
        let (mut above, mut here) = (vec![0u16; blocks_x], vec![0u16; blocks_x]);
        let (mut pred_bits, mut last_pred_symbol, mut pred_repeat) = (0u32, 0u32, 0u32);
        let (mut prev_endpoint, mut selector_run) = (0usize, 0u32);

        for by in 0..blocks_y {
            for bx in 0..blocks_x {
                // One predictor symbol per 2x2 group, read on its even row and
                // column; the row below uses the half it left.
                if bx % 2 == 0 {
                    if by % 2 == 0 {
                        if pred_repeat > 0 {
                            pred_repeat -= 1;
                            pred_bits = last_pred_symbol;
                        } else {
                            pred_bits = models.endpoint_pred.decode(&mut bits)?;
                            if pred_bits == ENDPOINT_PRED_REPEAT_LAST_SYMBOL {
                                pred_repeat = bits.vlc(4)?.wrapping_add(ENDPOINT_PRED_MIN_REPEAT_COUNT - 1);
                                pred_bits = last_pred_symbol;
                            } else {
                                last_pred_symbol = pred_bits;
                            }
                        }
                        group_below[bx / 2] = (pred_bits >> 4) as u8;
                    } else {
                        pred_bits = u32::from(group_below[bx / 2]);
                    }
                }
                let pred = pred_bits & 3;
                pred_bits >>= 2;

                let outside = |which: &str| malformed(format!("block ({bx}, {by}) predicts its endpoint from the {which} block, which is outside the slice"));
                let endpoint = match pred {
                    0 if bx == 0 => return Err(outside("left")),
                    0 => prev_endpoint,
                    1 if by == 0 => return Err(outside("upper")),
                    1 => usize::from(above[bx]),
                    2 if bx == 0 || by == 0 => return Err(outside("upper-left")),
                    2 => usize::from(above[bx - 1]),
                    _ => {
                        let index = models.endpoint_delta.decode(&mut bits)? as usize + prev_endpoint;
                        if index >= endpoints {
                            index - endpoints
                        } else {
                            index
                        }
                    }
                };
                if endpoint >= endpoints {
                    return Err(malformed(format!("block ({bx}, {by}) has endpoint index {endpoint}, past the {endpoints} endpoints of the codebook")));
                }
                here[bx] = endpoint as u16; // below `endpoints`, at most 65536
                prev_endpoint = endpoint;

                // The selector: a codebook entry, a slot of the history buffer, or a run of slot 0.
                let symbol = if selector_run > 0 {
                    selector_run -= 1;
                    selectors
                } else {
                    let symbol = models.selector.decode(&mut bits)? as usize;
                    if symbol == rle_symbol {
                        let run = models.selector_rle.decode(&mut bits)?;
                        selector_run = if run == SELECTOR_RLE_LONG_RUN { bits.vlc(7)?.wrapping_add(SELECTOR_HISTORY_BUF_RLE_COUNT_THRESH) } else { run + SELECTOR_HISTORY_BUF_RLE_COUNT_THRESH };
                        if selector_run == 0 || selector_run as usize > total_blocks {
                            return Err(malformed(format!("a selector run of {selector_run} blocks is longer than the slice's {total_blocks}")));
                        }
                        selector_run -= 1;
                        selectors
                    } else {
                        symbol
                    }
                };
                let selector = if symbol >= selectors {
                    let slot = symbol - selectors;
                    if slot >= history.values.len() {
                        return Err(malformed(format!("block ({bx}, {by}) names selector history slot {slot}, past the history of {}", history.values.len())));
                    }
                    let selector = history.values[slot];
                    if slot != 0 {
                        history.use_slot(slot);
                    }
                    selector
                } else {
                    history.add(symbol);
                    symbol
                };
                emit(bx, by, endpoint, selector);
            }
            std::mem::swap(&mut above, &mut here);
        }
        if pred_repeat != 0 {
            return Err(malformed(format!("the endpoint predictor repeat runs {pred_repeat} groups past the end of the slice")));
        }
        Ok(())
    }

    /// The extent of block (`bx`, `by`) inside a `width` x `height` image:
    /// (first column, first row, columns, rows). Blocks on the right and bottom
    /// edges of an image whose size is not a multiple of 4 are clipped.
    fn extent(width: usize, height: usize, bx: usize, by: usize) -> (usize, usize, usize, usize) {
        let (x0, y0) = (bx * 4, by * 4);
        (x0, y0, 4.min(width.saturating_sub(x0)), 4.min(height.saturating_sub(y0)))
    }

    /// One decoded 4x4 block (`block[y * 4 + x]`, RGBA) placed into the
    /// `width` x `height` RGBA image at block (`bx`, `by`): pixel (x, y) of
    /// the block is pixel (4 `bx` + x, 4 `by` + y) of the image, whose rows
    /// are exactly `width` pixels apart.
    pub(super) fn put_block(pixels: &mut [u8], width: usize, height: usize, bx: usize, by: usize, block: &[[u8; 4]; 16]) {
        let (x0, y0, columns, rows) = extent(width, height, bx, by);
        for y in 0..rows {
            let row = ((y0 + y) * width + x0) * 4;
            for x in 0..columns {
                pixels[row + x * 4..row + x * 4 + 4].copy_from_slice(&block[y * 4 + x]);
            }
        }
    }

    /// The alpha bytes of one decoded 4x4 block (`block[y * 4 + x]`), placed
    /// into the RGBA image like [`put_block`] but touching byte 3 only.
    pub(super) fn put_alpha(pixels: &mut [u8], width: usize, height: usize, bx: usize, by: usize, block: &[u8; 16]) {
        let (x0, y0, columns, rows) = extent(width, height, bx, by);
        for y in 0..rows {
            let row = ((y0 + y) * width + x0) * 4;
            for x in 0..columns {
                pixels[row + x * 4 + 3] = block[y * 4 + x];
            }
        }
    }

    /// Level 0 of an ETC1S/BasisLZ file as RGBA8: the codebooks and tables of
    /// the global data, then the RGB slice, then the alpha slice if level 0
    /// has one. The slices were checked to lie inside `level0` by `parse`.
    pub(super) fn decode(width: u32, height: u32, level0: &[u8], global: &BasisLz) -> Result<Rgba8, MeshError> {
        if global.image_flags & IS_P_FRAME != 0 {
            return Err(unsupported(format!(
                "level 0 is an ETC1S P-frame (imageFlags {:#x} has isPFrame set), which refers to the previous image of an animation, and a single image has none to decode it against",
                global.image_flags
            )));
        }
        if global.endpoint_count == 0 || global.selector_count == 0 {
            return Err(malformed("the BasisLZ global data declares no endpoints or no selectors".into()));
        }
        if global.endpoints.is_empty() || global.selectors.is_empty() || global.tables.is_empty() {
            return Err(malformed("the BasisLZ global data has an empty endpoint, selector or table block".into()));
        }
        let models = read_models(global.tables)?;
        let endpoints = read_endpoints(usize::from(global.endpoint_count), global.endpoints)?;
        let selectors = read_selectors(usize::from(global.selector_count), global.selectors)?;

        let (w, h) = (width as usize, height as usize);
        let blocks = (w.div_ceil(4), h.div_ceil(4));
        let mut pixels = vec![0u8; w * h * 4];
        decode_slice(&level0[global.rgb.clone()], blocks, &models, endpoints.len(), selectors.len(), |bx, by, e, s| {
            let (colors, rows) = (&endpoints[e].colors, selectors[s]);
            let mut block = [[0u8; 4]; 16];
            for (i, texel) in block.iter_mut().enumerate() {
                let [r, g, b] = colors[selector_at(rows[i / 4], i % 4)];
                *texel = [r, g, b, 255];
            }
            put_block(&mut pixels, w, h, bx, by, &block);
        })?;
        if let Some(alpha) = &global.alpha {
            decode_slice(&level0[alpha.clone()], blocks, &models, endpoints.len(), selectors.len(), |bx, by, e, s| {
                let (colors, rows) = (&endpoints[e].colors, selectors[s]);
                let mut block = [0u8; 16];
                for (i, texel) in block.iter_mut().enumerate() {
                    *texel = colors[selector_at(rows[i / 4], i % 4)][1];
                }
                put_alpha(&mut pixels, w, h, bx, by, &block);
            })?;
        }
        Ok(Rgba8 { width, height, pixels })
    }
}

/// UASTC LDR 4x4 to RGBA8, as Basis Universal v1.50 unpacks it to its `RGBA32`
/// target: the file's blocks are 16 bytes each, one per 4x4 pixels, decoded
/// one by one with no colour conversion (`srgb` false in the reference).
///
/// The reference is `basisu_transcoder.cpp` of the tags `v1_50_0` and
/// `v1_50_0_2` (three.js r183's transcoder, the oracle of the committed
/// `.rgba` files), read, never built or run by us:
///
/// - **Modes** (`g_uastc_mode_huff_codes`, `g_uastc_huff_modes` and the
///   `g_uastc_mode_*` arrays, cpp:11674-11721): the first 7 bits of a block are
///   a prefix code naming one of 19 modes; mode 8 is a solid colour, and the 20th
///   code is reserved ("future expansion"), which the reference fails. [`MODES`]
///   holds the columns of those arrays that decoding needs.
/// - **Unpacking** (`unpack_uastc(const uastc_block&, unpacked_uastc_block&, ...)`,
///   cpp:12576): after the code come the ETC1/BC1 hint bits, which only the
///   transcoders to those formats use and which are skipped; then a common
///   partition pattern (5 bits for modes 2, 4, 7, 9 and 16, 4 for mode 3), a
///   colour component selector (2 bits for modes 6, 11 and 13; mode 17's is 3),
///   the endpoints, then the weights, which run to the end of the used bits.
///   The endpoints of ranges with a trit or a quint (an ASTC "bounded integer
///   sequence") are sent as one base-3 or base-5 number per 5 or 3 values and
///   then their low bits (the reference's `tq_values` and `accum % mul`); the
///   last bundle is shorter. Each subset's first pixel (its "anchor") has a
///   weight one bit shorter, because its top bit is implied to be 0.
/// - **Unquantizing the endpoints** (`unquant_astc_endpoint`, cpp:11801): the ASTC
///   colour endpoint unquantization of the ASTC specification, "Color Unquantization
///   Procedure", for a range with a trit or a quint, and bit replication to 8 bits
///   for the others.
/// - **Pixels** (`unpack_uastc(uint32_t mode, ...)`, cpp:13037, with
///   `astc_interpolate` of `basisu_transcoder_uastc.h`): each subset's colours are
///   its two endpoints blended by the ASTC weights, in 16-bit precision, rounded
///   (`(l * (64 - w) + h * w + 32) >> 6`, where `l` and `h` are the 8-bit
///   endpoints replicated to 16 bits), then the top 8 bits. A mode with no alpha
///   has alpha 255, the luminance-alpha modes (15 to 17) copy luminance into
///   R, G and B, and a dual-plane block takes the component its selector names
///   from the second weight.
/// - **Partitions** (`g_astc_bc7_patterns2` and `3`, `g_bc7_3_astc2_patterns2`):
///   the reference lists the 60 common patterns as 16-entry tables and says in
///   debug builds (cpp:12173, `astc_compute_texel_partition`) that each is the ASTC
///   partition function of a seed. To keep the code small this computes the
///   function (ASTC specification, "Partition pattern generation") of the 60
///   seeds instead of holding the tables, and
///   `the_partition_function_gives_the_tables_of_the_reference` checks every row.
///   An anchor is the first pixel of its subset, which is what the reference's
///   anchor tables list (the same debug assertion).
///
/// `weights` and the `end` of the used bits are whole-block facts that the tests
/// pin for every mode.
mod uastc {
    use super::etc1s::put_block;
    use super::{malformed, MeshError, Rgba8};

    /// Bytes of one block.
    const BLOCK_LEN: usize = 16;
    /// The solid-colour mode.
    const SOLID: usize = 8;

    /// One mode's columns of the reference's `g_uastc_mode_*` arrays.
    struct Mode {
        /// The prefix code, read from bit 0, and its length.
        code: u8,
        code_len: u8,
        /// ETC1/BC1 hint bits after the code (`g_uastc_mode_total_hint_bits`).
        hint_bits: u8,
        subsets: u8,
        /// 2 for dual-plane modes: a second set of weights for one component.
        planes: u8,
        /// Components per endpoint: 3 RGB, 4 RGBA, 2 luminance-alpha.
        comps: u8,
        /// Bits of one weight.
        weight_bits: u8,
        /// The ASTC endpoint range (BISE index): 7 is 2 bits and a trit, 8 four
        /// bits, 11 five, 12 three and a quint, 13 four and a trit, 18 five and a
        /// quint, 19 six and a trit, 20 eight.
        range: u8,
    }

    const MODES: [Mode; 19] = [
        Mode { code: 0x01, code_len: 4, hint_bits: 15, subsets: 1, planes: 1, comps: 3, weight_bits: 4, range: 19 },
        Mode { code: 0x35, code_len: 6, hint_bits: 15, subsets: 1, planes: 1, comps: 3, weight_bits: 2, range: 20 },
        Mode { code: 0x1D, code_len: 5, hint_bits: 15, subsets: 2, planes: 1, comps: 3, weight_bits: 3, range: 8 },
        Mode { code: 0x03, code_len: 5, hint_bits: 15, subsets: 3, planes: 1, comps: 3, weight_bits: 2, range: 7 },
        Mode { code: 0x13, code_len: 5, hint_bits: 15, subsets: 2, planes: 1, comps: 3, weight_bits: 2, range: 12 },
        Mode { code: 0x0B, code_len: 5, hint_bits: 15, subsets: 1, planes: 1, comps: 3, weight_bits: 3, range: 20 },
        Mode { code: 0x1B, code_len: 5, hint_bits: 15, subsets: 1, planes: 2, comps: 3, weight_bits: 2, range: 18 },
        Mode { code: 0x07, code_len: 5, hint_bits: 15, subsets: 2, planes: 1, comps: 3, weight_bits: 2, range: 12 },
        Mode { code: 0x17, code_len: 5, hint_bits: 0, subsets: 0, planes: 0, comps: 4, weight_bits: 0, range: 0 },
        Mode { code: 0x0F, code_len: 5, hint_bits: 23, subsets: 2, planes: 1, comps: 4, weight_bits: 2, range: 8 },
        Mode { code: 0x02, code_len: 3, hint_bits: 17, subsets: 1, planes: 1, comps: 4, weight_bits: 4, range: 13 },
        Mode { code: 0x00, code_len: 2, hint_bits: 17, subsets: 1, planes: 2, comps: 4, weight_bits: 2, range: 13 },
        Mode { code: 0x06, code_len: 3, hint_bits: 17, subsets: 1, planes: 1, comps: 4, weight_bits: 3, range: 19 },
        Mode { code: 0x1F, code_len: 5, hint_bits: 23, subsets: 1, planes: 2, comps: 4, weight_bits: 1, range: 20 },
        Mode { code: 0x0D, code_len: 5, hint_bits: 23, subsets: 1, planes: 1, comps: 4, weight_bits: 2, range: 20 },
        Mode { code: 0x05, code_len: 7, hint_bits: 23, subsets: 1, planes: 1, comps: 2, weight_bits: 4, range: 20 },
        Mode { code: 0x15, code_len: 6, hint_bits: 23, subsets: 2, planes: 1, comps: 2, weight_bits: 2, range: 20 },
        Mode { code: 0x25, code_len: 6, hint_bits: 23, subsets: 1, planes: 2, comps: 2, weight_bits: 2, range: 20 },
        Mode { code: 0x09, code_len: 4, hint_bits: 15, subsets: 1, planes: 1, comps: 3, weight_bits: 5, range: 11 },
    ];

    /// The ASTC partition seeds of the common patterns (`m_astc` of
    /// `g_astc_bc7_common_partitions2` and `3`, `m_astc2` of
    /// `g_bc7_3_astc2_common_partitions`): the two-subset patterns of modes 2, 4, 9
    /// and 16, the three-subset patterns of mode 3 and mode 7's own.
    const SEEDS_2: [u16; 30] = [
        28, 20, 16, 29, 91, 9, 107, 72, 149, 204, 50, 114, 496, 17, 78, 39, 252, 828, 43, 156, 116, 210, 476, 273, 684, 359, 246, 195, 694, 524,
    ];
    const SEEDS_3: [u16; 11] = [260, 74, 32, 156, 183, 15, 745, 0, 335, 902, 254];
    const SEEDS_7: [u16; 19] = [36, 48, 61, 137, 161, 183, 226, 281, 302, 307, 479, 495, 593, 594, 605, 799, 812, 988, 993];

    /// The mode a block's first byte names: its low 7 bits are a prefix code
    /// (bit 7 belongs to the next field). `None` is the reserved code.
    pub(super) fn mode_of(first: u8) -> Option<usize> {
        MODES.iter().position(|m| u32::from(first) & ((1 << m.code_len) - 1) == u32::from(m.code))
    }

    /// An endpoint range (the ASTC "BISE" index, `g_astc_bise_range_table`): its
    /// bits, whether it adds a trit (3) or a quint (5) or neither (0), and the
    /// constants C and B of the unquantization (`g_astc_endpoint_unquant_params`,
    /// B as the nine bit sources, most significant first: `a` is bit 0 of the
    /// stored bits, `b` bit 1, and so on; `0` is a zero).
    fn range(index: u32) -> (u32, u32, u32, &'static [u8; 9]) {
        match index {
            7 => (2, 3, 93, b"b000b0bb0"),
            8 => (4, 0, 0, b"000000000"),
            11 => (5, 0, 0, b"000000000"),
            12 => (3, 5, 26, b"cb0000cbc"),
            13 => (4, 3, 22, b"dcb000dcb"),
            18 => (5, 5, 6, b"edcb0000e"),
            19 => (6, 3, 5, b"fedcb000f"),
            // 20, the only other range a mode names.
            _ => (8, 0, 0, b"000000000"),
        }
    }

    /// An endpoint value as stored (its low bits, with the trit or quint above
    /// them) unquantized to 0..=255 (`unquant_astc_endpoint_val`).
    pub(super) fn unquant(index: u32, value: u32) -> u8 {
        let (bits, base, c, sources) = range(index);
        let low = value & ((1 << bits) - 1);
        if base == 0 {
            // The bits repeated down to 8 bits.
            let (mut out, mut left) = (0u32, 8u32);
            while left > 0 && bits > 0 {
                let take = left.min(bits);
                out |= (low >> (bits - take)) << (left - take);
                left -= take;
            }
            return out as u8;
        }
        let a = if low & 1 == 1 { 511 } else { 0 };
        let mut b = 0;
        for &source in sources {
            b <<= 1;
            if source != b'0' {
                b |= (low >> (source - b'a')) & 1;
            }
        }
        let val = ((value >> bits) * c + b) ^ a;
        ((a & 0x80) | (val >> 2)) as u8
    }

    /// The weight (0 to 64) of an index of `bits` bits: the ASTC unquantization,
    /// the index's bits repeated to 6 bits and 1 added if that is over 32
    /// (`g_bc7_weights1` to `3` and `g_astc_weights4` and `5`).
    pub(super) fn weight(index: u32, bits: u32) -> u32 {
        let (mut out, mut filled) = (0u32, 0u32);
        while filled < 6 && bits > 0 {
            let take = bits.min(6 - filled);
            out = (out << take) | (index >> (bits - take));
            filled += take;
        }
        if out > 32 {
            out + 1
        } else {
            out
        }
    }

    /// `astc_interpolate` with `srgb` false: the endpoints `l` and `h` at weight
    /// `w`, in 16-bit precision, down to 8 bits.
    fn blend(l: u32, h: u32, w: u32) -> u8 {
        let (l, h) = ((l << 8) | l, (h << 8) | h);
        (((l * (64 - w) + h * w + 32) >> 6) >> 8) as u8
    }

    /// The subset (0 to `subsets` - 1) of each pixel of a 4x4 block in the
    /// pattern of an ASTC partition `seed`: the ASTC partition function
    /// (`astc_compute_texel_partition` with `small_block` and z = 0).
    pub(super) fn partition(seed: u32, subsets: u32) -> [u8; 16] {
        let seed = seed + (subsets - 1) * 1024;
        // hash52
        let mut r = seed;
        r ^= r >> 15;
        r = r.wrapping_sub(r << 17);
        r = r.wrapping_add(r << 7);
        r = r.wrapping_add(r << 4);
        r ^= r >> 5;
        r = r.wrapping_add(r << 16);
        r ^= r >> 7;
        r ^= r >> 3;
        r ^= r << 6;
        r ^= r >> 17;
        // Eight 4-bit numbers of the hash, squared; the odd ones are shifted by
        // `sh1` and the even ones by `sh2`.
        let square = |n: u32| {
            let s = (r >> (4 * n)) & 0xF;
            s * s
        };
        let (sh1, sh2) = if seed & 1 == 1 {
            (if seed & 2 == 2 { 4 } else { 5 }, if subsets == 3 { 6 } else { 5 })
        } else {
            (if subsets == 3 { 6 } else { 5 }, if seed & 2 == 2 { 4 } else { 5 })
        };
        let mut out = [0u8; 16];
        for (i, subset) in out.iter_mut().enumerate() {
            // The pixel's coordinates, doubled: a block under 31 texels uses the small-block scale.
            let (x, y) = (2 * (i as u32 & 3), 2 * (i as u32 >> 2));
            let term = |n: u32, shift: u32| ((square(2 * n) >> sh1) * x + (square(2 * n + 1) >> sh2) * y + (r >> shift)) & 0x3F;
            let (a, b) = (term(0, 14), term(1, 10));
            let c = if subsets < 3 { 0 } else { term(2, 6) };
            let d = if subsets < 4 { 0 } else { term(3, 2) };
            *subset = if a >= b && a >= c && a >= d {
                0
            } else if b >= c && b >= d {
                1
            } else if c >= d {
                2
            } else {
                3
            };
        }
        out
    }

    /// A little-endian bit reader over one block, the way the reference reads it
    /// (`read_bits1_to_9_fst`): bit 0 is the least significant bit of byte 0.
    struct Bits {
        block: u128,
        at: u32,
    }

    impl Bits {
        /// The next `n` (at most 8) bits; zeros past the block's end, which
        /// the layout of the modes never reaches.
        fn take(&mut self, n: u32) -> u32 {
            let value = (self.block.checked_shr(self.at).unwrap_or(0) & ((1 << n) - 1)) as u32;
            self.at += n;
            value
        }
    }

    /// One block, unpacked to the fields its mode lays out (the reference's
    /// `unpacked_uastc_block` without its hints).
    pub(super) struct Block {
        pub(super) mode: usize,
        /// The common partition pattern (modes 2, 3, 4, 7, 9 and 16), else 0.
        /// Decoding looks its seed up by this number, and an error names it;
        /// the tests also read it, to see that every pattern is reached.
        pub(super) pattern: usize,
        /// The component that has the second weight plane (dual-plane modes), else 0.
        pub(super) ccs: usize,
        /// The endpoint values as stored, `comps * 2 * subsets` of them in the
        /// order R low, R high, G low, ..., subset after subset. A solid block
        /// holds its R, G, B, A here.
        endpoints: [u8; 18],
        /// The weights, one per pixel, or a pair per pixel for dual plane.
        weights: [u8; 32],
        /// The subset of each pixel.
        subsets: [u8; 16],
        /// The bit after the block's last used field.
        pub(super) end: u32,
    }

    /// Unpack the fields of a block (`unpack_uastc`, `blue_contract_check` and
    /// `read_hints` false). The error names what in the block is not valid.
    pub(super) fn parse_block(bytes: &[u8; BLOCK_LEN]) -> Result<Block, String> {
        let Some(mode) = mode_of(bytes[0]) else {
            return Err("it uses the reserved mode code (the 20th prefix code, 1000101)".into());
        };
        let m = &MODES[mode];
        let mut bits = Bits { block: u128::from_le_bytes(*bytes), at: u32::from(m.code_len) };
        let mut block = Block { mode, pattern: 0, ccs: 0, endpoints: [0; 18], weights: [0; 32], subsets: [0; 16], end: 0 };
        if mode == SOLID {
            for value in &mut block.endpoints[..4] {
                *value = bits.take(8) as u8;
            }
            block.end = bits.at;
            return Ok(block);
        }
        bits.at += u32::from(m.hint_bits);

        // The common partition pattern, and the seed of its ASTC partition.
        let (pattern_bits, seeds): (u32, &[u16]) = match mode {
            2 | 4 | 9 | 16 => (5, &SEEDS_2),
            3 => (4, &SEEDS_3),
            7 => (5, &SEEDS_7),
            _ => (0, &[]),
        };
        let subsets = u32::from(m.subsets);
        if pattern_bits > 0 {
            block.pattern = bits.take(pattern_bits) as usize;
            let Some(&seed) = seeds.get(block.pattern) else {
                return Err(format!("its partition pattern {} is past the {} that mode {mode} defines", block.pattern, seeds.len()));
            };
            block.subsets = partition(u32::from(seed), subsets);
        }
        match mode {
            6 | 11 | 13 => block.ccs = bits.take(2) as usize,
            17 => block.ccs = 3,
            _ => {}
        }

        // The endpoints: for a range with a trit or a quint, the packed bundles
        // first (8 bits for 5 trits, 7 for 3 quints, fewer for the last bundle),
        // then each value's low bits with its digit of its bundle above them.
        let values = usize::from(m.comps) * 2 * usize::from(m.subsets);
        let (low_bits, base, _, _) = range(u32::from(m.range));
        let per_bundle: usize = if base == 3 { 5 } else { 3 };
        let bundles = if base == 0 { 0 } else { values.div_ceil(per_bundle) };
        let mut packed = [0u32; 8];
        for (i, slot) in packed.iter_mut().enumerate().take(bundles) {
            let mut n = if base == 3 { 8 } else { 7 };
            if i == bundles - 1 {
                n = match (base, values - i * per_bundle) {
                    (3, 1) => 2,
                    (3, 2) => 4,
                    (3, 3) => 5,
                    (3, 4) => 7,
                    (5, 1) => 3,
                    (5, 2) => 5,
                    _ => n,
                };
            }
            *slot = bits.take(n);
        }
        let (mut digits, mut left, mut next) = (0u32, 0usize, 0usize);
        for value in block.endpoints.iter_mut().take(values) {
            let mut v = bits.take(low_bits);
            if bundles > 0 {
                if left == 0 {
                    digits = packed[next];
                    next += 1;
                    left = per_bundle;
                }
                v |= (digits % base) << low_bits;
                digits /= base;
                left -= 1;
            }
            *value = v as u8;
        }

        // The weights, from here to the end of the used bits: a subset's first
        // pixel has one bit less (both weights of the first pixel, for dual plane).
        let weight_bits = u32::from(m.weight_bits);
        let mut first = [usize::MAX; 3];
        for (i, &s) in block.subsets.iter().enumerate() {
            let slot = &mut first[usize::from(s)];
            if *slot == usize::MAX {
                *slot = i;
            }
        }
        let count = if m.planes == 2 { 32 } else { 16 };
        for i in 0..count {
            let anchor = if m.planes == 2 { i < 2 } else { first[usize::from(block.subsets[i])] == i };
            let n = if anchor { weight_bits - 1 } else { weight_bits };
            block.weights[i] = bits.take(n) as u8;
        }
        block.end = bits.at;
        Ok(block)
    }

    /// The 16 pixels of a block, RGBA, `[y * 4 + x]`.
    pub(super) fn pixels(block: &Block) -> [[u8; 4]; 16] {
        let m = &MODES[block.mode];
        if block.mode == SOLID {
            let [r, g, b, a, ..] = block.endpoints;
            return [[r, g, b, a]; 16];
        }
        let (comps, subsets, weight_bits) = (usize::from(m.comps), usize::from(m.subsets), u32::from(m.weight_bits));
        // Each subset's two endpoint colours, a component without a stored value 255.
        let mut ends = [[[255u32; 4]; 2]; 3];
        for (s, subset) in ends.iter_mut().enumerate().take(subsets) {
            let value = |k: usize| u32::from(unquant(u32::from(m.range), u32::from(block.endpoints[s * comps * 2 + k])));
            if comps == 2 {
                // Luminance and alpha: the luminance fills R, G and B.
                subset[0] = [value(0), value(0), value(0), value(2)];
                subset[1] = [value(1), value(1), value(1), value(3)];
            } else {
                for c in 0..comps {
                    subset[0][c] = value(c * 2);
                    subset[1][c] = value(c * 2 + 1);
                }
            }
        }
        // Every colour a weight can pick, per subset.
        let mut colors = [[[0u8; 4]; 32]; 3];
        for (s, subset) in colors.iter_mut().enumerate().take(subsets) {
            for (index, color) in subset.iter_mut().enumerate().take(1 << weight_bits) {
                let w = weight(index as u32, weight_bits);
                for c in 0..4 {
                    color[c] = blend(ends[s][0][c], ends[s][1][c], w);
                }
            }
        }
        let mut out = [[0u8; 4]; 16];
        for (i, pixel) in out.iter_mut().enumerate() {
            if m.planes == 2 {
                let (first, second) = (usize::from(block.weights[2 * i]), usize::from(block.weights[2 * i + 1]));
                for (c, value) in pixel.iter_mut().enumerate() {
                    *value = colors[0][if c == block.ccs { second } else { first }][c];
                }
            } else {
                *pixel = colors[usize::from(block.subsets[i])][usize::from(block.weights[i])];
            }
        }
        out
    }

    /// The bytes of a `width` x `height` UASTC level: 16 for each 4x4 block,
    /// the blocks on the right and bottom edges counted whole.
    pub(super) fn level_len(width: u32, height: u32) -> u64 {
        u64::from(width.div_ceil(4)) * u64::from(height.div_ceil(4)) * BLOCK_LEN as u64
    }

    /// Level 0 of a UASTC file as RGBA8: `level` is exactly 16 bytes for each
    /// 4x4 block of the `width` x `height` image, left to right then top to
    /// bottom; blocks on the right and bottom edges are cut to the image.
    pub(super) fn decode(width: u32, height: u32, level: &[u8]) -> Result<Rgba8, MeshError> {
        let (w, h) = (width as usize, height as usize);
        let columns = w.div_ceil(4);
        let expected = level_len(width, height);
        if level.len() as u64 != expected {
            return Err(malformed(format!("level 0 holds {} bytes, but a {width}x{height} UASTC image is {expected}", level.len())));
        }
        // The level is there in full, so this is at most four times its size.
        let mut out = vec![0u8; w * h * 4];
        let (blocks, _) = level.as_chunks::<BLOCK_LEN>();
        for (i, bytes) in blocks.iter().enumerate() {
            let (bx, by) = (i % columns, i / columns);
            let block = parse_block(bytes).map_err(|why| malformed(format!("UASTC block ({bx}, {by}): {why}")))?;
            debug_assert!(block.end <= 8 * BLOCK_LEN as u32, "a block's fields end inside the block");
            put_block(&mut out, w, h, bx, by, &pixels(&block));
        }
        Ok(Rgba8 { width, height, pixels: out })
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::sweeps::layer_sweep;
    use super::*;

    /// The 12 identifier bytes, `«KTX 20»\r\n\x1A\n`, as the KTX2 specification
    /// lists them: written out here, not read from the module under test.
    const ID: [u8; 12] = [0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A];

    // Vulkan's VkFormat values (vulkan_core.h) and the DFD colour models
    // (khr_df.h) that these tests build files from.
    const R8G8B8A8_UNORM: u32 = 37;
    const R8G8B8A8_SRGB: u32 = 43;
    const B8G8R8A8_UNORM: u32 = 44;
    const BC7_UNORM_BLOCK: u32 = 145;
    const MODEL_RGBSDA: u8 = 1;
    const MODEL_ETC1S: u8 = 163;
    const MODEL_UASTC: u8 = 166;
    const TRANSFER_LINEAR: u8 = 1;
    const TRANSFER_SRGB: u8 = 2;

    /// A KTX2 file, field by field (KTX File Format Specification v2.0:
    /// Identifier, Header, Index, Level Index, DFD, Supercompression Global
    /// Data, Mip Level Array). Every field is the test's to set, so a test
    /// makes the exact shape it needs, including shapes no encoder writes:
    /// levels in any order, a `levelCount` apart from the entries listed,
    /// any scheme, faces, layers and depth.
    #[derive(Clone)]
    struct Ktx2 {
        vk_format: u32,
        type_size: u32,
        width: u32,
        height: u32,
        depth: u32,
        layers: u32,
        faces: u32,
        /// The header's `levelCount`, which need not match `levels`.
        level_count: u32,
        scheme: u32,
        /// Level data, `levels[0]` the largest. One Level Index entry each.
        levels: Vec<Vec<u8>>,
        /// The order the levels' bytes are laid out in the file. The spec
        /// lays them smallest first, so that is the default.
        layout: Vec<usize>,
        dfd: Vec<u8>,
        kvd: Vec<u8>,
        sgd: Vec<u8>,
    }

    impl Ktx2 {
        /// The Header's fields for `levels`, with the spec's defaults: a 2D
        /// texture, one face, no layers, `levelCount` as many as listed,
        /// smallest level laid out first.
        fn new(vk_format: u32, width: u32, height: u32, levels: Vec<Vec<u8>>) -> Ktx2 {
            Ktx2 {
                vk_format,
                type_size: 1,
                width,
                height,
                depth: 0,
                layers: 0,
                faces: 1,
                level_count: levels.len() as u32,
                scheme: 0,
                layout: (0..levels.len()).rev().collect(),
                levels,
                dfd: Vec::new(),
                kvd: Vec::new(),
                sgd: Vec::new(),
            }
        }

        /// An uncompressed RGBA8 file: `vkFormat` as given, and the DFD an
        /// encoder writes for it (four 8-bit samples R, G, B, A).
        fn rgba8(vk_format: u32, width: u32, height: u32, levels: Vec<Vec<u8>>) -> Ktx2 {
            let transfer = if vk_format == R8G8B8A8_SRGB { TRANSFER_SRGB } else { TRANSFER_LINEAR };
            let mut k = Ktx2::new(vk_format, width, height, levels);
            k.dfd = dfd(MODEL_RGBSDA, transfer, 1, 4, &[(0, 7, 0), (8, 7, 1), (16, 7, 2), (24, 7, 15)]);
            k
        }

        /// A BasisLZ file: `vkFormat` 0, scheme 1, the ETC1S colour model.
        fn etc1s(width: u32, height: u32, levels: Vec<Vec<u8>>, sgd: Vec<u8>) -> Ktx2 {
            let mut k = Ktx2::new(0, width, height, levels);
            k.scheme = 1;
            k.dfd = dfd(MODEL_ETC1S, TRANSFER_SRGB, 4, 0, &[(0, 63, 0)]);
            k.sgd = sgd;
            k
        }

        /// A UASTC file: `vkFormat` 0, the UASTC colour model, `scheme` 0 or 2.
        fn uastc(width: u32, height: u32, scheme: u32, levels: Vec<Vec<u8>>) -> Ktx2 {
            let mut k = Ktx2::new(0, width, height, levels);
            k.scheme = scheme;
            k.dfd = dfd(MODEL_UASTC, TRANSFER_LINEAR, 4, 16, &[(0, 127, 3)]);
            k
        }

        /// The file's bytes: Identifier, Header, Index, Level Index, DFD, KVD,
        /// SGD (8-byte aligned), then the levels in `layout` order, each
        /// padded to 8 bytes.
        fn build(&self) -> Vec<u8> {
            let align8 = |x: usize| (x + 7) & !7;
            let n = self.levels.len();
            let dfd_at = 80 + 24 * n;
            let kvd_at = dfd_at + self.dfd.len();
            let sgd_at = align8(kvd_at + self.kvd.len());
            let mut cursor = align8(sgd_at + self.sgd.len());
            let mut level_at = vec![0usize; n];
            for &i in &self.layout {
                level_at[i] = cursor;
                cursor = align8(cursor + self.levels[i].len());
            }
            let present = |bytes: &Vec<u8>, at: usize| if bytes.is_empty() { (0u64, 0u64) } else { (at as u64, bytes.len() as u64) };

            let mut out = ID.to_vec();
            for field in [
                self.vk_format,
                self.type_size,
                self.width,
                self.height,
                self.depth,
                self.layers,
                self.faces,
                self.level_count,
                self.scheme,
            ] {
                out.extend(field.to_le_bytes());
            }
            let (dfd_off, dfd_len) = present(&self.dfd, dfd_at);
            let (kvd_off, kvd_len) = present(&self.kvd, kvd_at);
            let (sgd_off, sgd_len) = present(&self.sgd, sgd_at);
            for field in [dfd_off as u32, dfd_len as u32, kvd_off as u32, kvd_len as u32] {
                out.extend(field.to_le_bytes());
            }
            out.extend(sgd_off.to_le_bytes());
            out.extend(sgd_len.to_le_bytes());
            assert_eq!(out.len(), 80);
            for (i, level) in self.levels.iter().enumerate() {
                out.extend((level_at[i] as u64).to_le_bytes()); // byteOffset
                out.extend((level.len() as u64).to_le_bytes()); // byteLength
                out.extend((level.len() as u64).to_le_bytes()); // uncompressedByteLength
            }
            out.extend(&self.dfd);
            out.extend(&self.kvd);
            out.resize(sgd_at, 0);
            out.extend(&self.sgd);
            for &i in &self.layout {
                out.resize(level_at[i], 0);
                out.extend(&self.levels[i]);
            }
            out
        }
    }

    /// A basic Data Format Descriptor (Khronos Data Format Specification,
    /// "Khronos Basic Data Format Descriptor Block"; KTX2 prefixes it with its
    /// `dfdTotalSize`). `samples` are (bitOffset, bitLength - 1, channelType),
    /// each an unsigned 0..255 sample.
    fn dfd(model: u8, transfer: u8, block_dim: u8, bytes_plane0: u8, samples: &[(u16, u8, u8)]) -> Vec<u8> {
        let block_size = 24 + 16 * samples.len() as u32;
        let mut d = Vec::new();
        d.extend((4 + block_size).to_le_bytes()); // dfdTotalSize
        d.extend(0u32.to_le_bytes()); // vendorId 0 (Khronos), descriptorType 0 (basic)
        d.extend((2 | (block_size << 16)).to_le_bytes()); // versionNumber 2, descriptorBlockSize
        d.extend([model, 1, transfer, 0]); // colorModel, colorPrimaries (BT.709), transferFunction, flags
        d.extend([block_dim - 1, block_dim - 1, 0, 0]); // texelBlockDimension0..3, each one less
        d.extend([bytes_plane0, 0, 0, 0, 0, 0, 0, 0]); // bytesPlane0..7
        for &(bit_offset, bit_length, channel) in samples {
            d.extend((u32::from(bit_offset) | (u32::from(bit_length) << 16) | (u32::from(channel) << 24)).to_le_bytes());
            d.extend(0u32.to_le_bytes()); // samplePosition0..3
            d.extend(0u32.to_le_bytes()); // sampleLower
            d.extend(255u32.to_le_bytes()); // sampleUpper
        }
        d
    }

    /// One BasisLZ image descriptor: imageFlags, then the RGB slice's and the
    /// alpha slice's (byteOffset, byteLength) within its level.
    struct Desc {
        rgb: (u32, u32),
        alpha: (u32, u32),
    }

    /// BasisLZ global data (Supercompression Global Data, BasisLZ): the
    /// header, one descriptor per image, then the four data blocks.
    fn basislz_sgd(counts: (u16, u16), blocks: [&[u8]; 4], descs: &[Desc]) -> Vec<u8> {
        let mut g = Vec::new();
        g.extend(counts.0.to_le_bytes()); // endpointCount
        g.extend(counts.1.to_le_bytes()); // selectorCount
        for block in blocks {
            g.extend((block.len() as u32).to_le_bytes()); // endpoints, selectors, tables, extended byte lengths
        }
        for d in descs {
            g.extend(0u32.to_le_bytes()); // imageFlags
            for v in [d.rgb.0, d.rgb.1, d.alpha.0, d.alpha.1] {
                g.extend(v.to_le_bytes());
            }
        }
        for block in blocks {
            g.extend(block);
        }
        g
    }

    /// `n` bytes that differ from each other and from other `seed`s.
    fn pattern(n: usize, seed: u8) -> Vec<u8> {
        (0..n).map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed)).collect()
    }

    /// Overwrite a header or index field of a built file (offsets are the
    /// specification's: 12 vkFormat, 20 pixelWidth, 24 pixelHeight, 52
    /// dfdByteLength, 64 sgdByteOffset, 72 sgdByteLength, 80 the first Level
    /// Index entry's byteOffset, 88 its byteLength).
    fn put32(file: &mut [u8], at: usize, v: u32) {
        file[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn put64(file: &mut [u8], at: usize, v: u64) {
        file[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }

    fn decoded(file: &[u8]) -> Rgba8 {
        decode_rgba8(file).unwrap_or_else(|e| panic!("expected a decode, got {e:?}"))
    }
    fn malformed(result: Result<impl std::fmt::Debug, MeshError>) -> String {
        match result {
            Err(MeshError::Malformed(why)) => why,
            other => panic!("expected Malformed, got {other:?}"),
        }
    }
    fn unsupported(result: Result<impl std::fmt::Debug, MeshError>) -> String {
        match result {
            Err(MeshError::Unsupported(why)) => why,
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    // ── the container ───────────────────────────────────────────────────

    /// The Mip Level Array runs smallest level first, so level 0's bytes are
    /// the last in the file, not the first after the index. They are found by
    /// the Level Index's byteOffset and byteLength.
    #[test]
    fn levels_are_taken_by_offset_not_by_order() {
        let (base, small) = (pattern(2 * 2 * 4, 10), pattern(4, 200));
        for layout in [vec![1, 0], vec![0, 1]] {
            let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![base.clone(), small.clone()]);
            k.layout = layout.clone();
            let img = decoded(&k.build());
            assert_eq!((img.width, img.height), (2, 2), "layout {layout:?}");
            assert_eq!(img.pixels, base, "layout {layout:?}: level 0's bytes, not the smaller level's");
        }
        // The decoy: the bytes right after the Level Index and the DFD are level 1's.
        let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![base.clone(), small.clone()]);
        k.layout = vec![1, 0];
        let file = k.build();
        let first_data = u64::from_le_bytes(file[104..112].try_into().unwrap()); // level 1's byteOffset
        assert!(first_data < u64::from_le_bytes(file[80..88].try_into().unwrap()), "level 1 comes first in the file");
        assert_eq!(probe(&file).unwrap().level_count, 2);
    }

    /// `levelCount` 0 means the file holds the base level only, and its Level
    /// Index has one entry (Header: levelCount).
    #[test]
    fn level_count_zero_means_the_base_level() {
        let pixels = pattern(3 * 5 * 4, 3);
        let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 3, 5, vec![pixels.clone()]);
        k.level_count = 0;
        let file = k.build();
        let img = decoded(&file);
        assert_eq!((img.width, img.height, img.pixels), (3, 5, pixels));
        let info = probe(&file).unwrap();
        assert_eq!(info.level_count, 0, "the header's own value");
    }

    /// `levelCount` 0 is one image, not none (the mutant `level_count.max(1)`
    /// to `level_count` made it none). A BasisLZ file with one image
    /// descriptor keeps its four blocks after that descriptor, at byte 40 of
    /// the global data and not at 20, and keeps level 0's slices.
    #[test]
    fn level_count_zero_counts_one_image_in_the_basislz_global_data() {
        let level0 = pattern(100, 1);
        let sgd = basislz_sgd((258, 515), [&[1, 2, 3], &[4, 5], &[6], &[7, 8, 9, 10]], &[Desc { rgb: (4, 60), alpha: (64, 30) }]);
        let mut k = Ktx2::etc1s(8, 8, vec![level0.clone()], sgd);
        k.level_count = 0;
        let file = k.build();
        let c = parse(&file).unwrap();
        assert_eq!(c.info.level_count, 0, "the header's own value");
        assert_eq!(c.level0, &level0[..]);
        let g = c.basislz.expect("BasisLZ global data");
        assert_eq!((g.endpoint_count, g.selector_count), (258, 515));
        assert_eq!((g.endpoints, g.selectors, g.tables, g.extended), (&[1u8, 2, 3][..], &[4u8, 5][..], &[6u8][..], &[7u8, 8, 9, 10][..]));
        assert_eq!((g.rgb, g.alpha), (4..64, Some(64..94)));
    }

    /// And its Level Index has one entry, which ends at byte 104: a DFD that
    /// starts at 90 starts inside it. With no entries counted it would end at
    /// 80, and the DFD would pass.
    #[test]
    fn level_count_zero_has_a_level_index_of_one_entry() {
        let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]);
        k.level_count = 0;
        let mut file = k.build();
        put32(&mut file, 48, 90); // dfdByteOffset
        let why = malformed(probe(&file));
        assert!(why.contains("level index") && why.contains("data format descriptor"), "{why}");
    }

    #[test]
    fn zlib_supercompression_is_refused_by_name() {
        let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]);
        k.scheme = 3;
        let file = k.build();
        for why in [unsupported(probe(&file)), unsupported(decode_rgba8(&file))] {
            assert!(why.contains("ZLIB") && why.contains('3'), "{why}");
        }
    }

    #[test]
    fn an_unknown_supercompression_scheme_is_refused_with_its_number() {
        for scheme in [4u32, 0x10001, u32::MAX] {
            let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]);
            k.scheme = scheme;
            let why = unsupported(probe(&k.build()));
            assert!(why.contains(&scheme.to_string()), "scheme {scheme}: {why}");
        }
    }

    #[test]
    fn cube_and_array_textures_are_refused_by_name() {
        let mut cube = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]);
        cube.faces = 6;
        let why = unsupported(decode_rgba8(&cube.build()));
        assert!(why.contains("cube map") && why.contains('6'), "{why}");

        for layers in [2u32, 4, u32::MAX] {
            let mut array = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]);
            array.layers = layers;
            let file = array.build();
            let why = unsupported(decode_rgba8(&file));
            assert!(why.contains("array") && why.contains(&layers.to_string()), "layerCount {layers}: {why}");
            unsupported(probe(&file));
        }
        // A cube array is a cube first.
        cube.layers = 3;
        assert!(unsupported(probe(&cube.build())).contains("cube map"));
    }

    /// One layer is `layerCount` 1 (a one-element array): layer 0 is all of
    /// it, so it decodes. Zero is the usual "not an array".
    #[test]
    fn a_one_element_array_is_its_layer_zero() {
        let pixels = pattern(16, 9);
        for layers in [0u32, 1] {
            let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pixels.clone()]);
            k.layers = layers;
            assert_eq!(decoded(&k.build()).pixels, pixels, "layerCount {layers}");
        }
    }

    #[test]
    fn three_d_and_one_d_textures_are_refused_by_name() {
        let mut volume = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]);
        volume.depth = 8;
        let why = unsupported(decode_rgba8(&volume.build()));
        assert!(why.contains("3D") && why.contains('8'), "{why}");

        let line = Ktx2::rgba8(R8G8B8A8_UNORM, 4, 0, vec![pattern(16, 0)]);
        let why = unsupported(decode_rgba8(&line.build()));
        assert!(why.contains("1D"), "{why}");
    }

    #[test]
    fn dimensions_and_faces_the_header_cannot_have_are_malformed() {
        let base = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]);
        let mut no_width = base.clone();
        no_width.width = 0;
        assert!(malformed(probe(&no_width.build())).contains("pixelWidth"));
        for faces in [0u32, 2, 5, 7] {
            let mut k = base.clone();
            k.faces = faces;
            assert!(malformed(probe(&k.build())).contains("faceCount"), "faceCount {faces}");
        }
    }

    /// A texture's size is checked from the header, before the Level Index or
    /// any data is looked at: the file here is a few hundred bytes whose
    /// header and Level Index claim a 16 GiB level.
    #[test]
    fn a_texture_past_the_pixel_budget_is_refused_before_allocating() {
        for (w, h) in [(65_536u32, 65_536u32), (u32::MAX, u32::MAX), (8193, 8192), (1, 67_108_865)] {
            let mut file = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]).build();
            put32(&mut file, 20, w);
            put32(&mut file, 24, h);
            put64(&mut file, 88, (u64::from(w) * u64::from(h)).saturating_mul(4)); // byteLength to match
            assert!(file.len() < 1024);
            for why in [unsupported(probe(&file)), unsupported(decode_rgba8(&file))] {
                assert!(why.contains("pixel budget") && why.contains(&w.to_string()), "{w}x{h}: {why}");
            }
        }
    }

    /// The budget is 64 Mi pixels (256 MiB of RGBA), and exactly that many
    /// are within it: 8192 x 8192 gets past the budget and is stopped by its
    /// level not being in the file.
    #[test]
    fn the_pixel_budget_is_exactly_64m_pixels() {
        for (w, h) in [(8192u32, 8192u32), (67_108_864, 1), (1, 67_108_864)] {
            let mut file = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]).build();
            put32(&mut file, 20, w);
            put32(&mut file, 24, h);
            put64(&mut file, 88, u64::from(w) * u64::from(h) * 4);
            let why = malformed(decode_rgba8(&file));
            assert!(why.contains("level 0"), "{w}x{h}: {why}");
        }
    }

    /// Basic BasisLZ file for the slice tests: one 8x8 image with a 100-byte
    /// level 0, whose RGB slice is the first 60 bytes and alpha the rest.
    fn etc1s_with(desc: Desc) -> Vec<u8> {
        let sgd = basislz_sgd((2, 3), [&[1, 2, 3], &[4, 5], &[6], &[]], &[desc]);
        Ktx2::etc1s(8, 8, vec![pattern(100, 1)], sgd).build()
    }

    #[test]
    fn slice_offsets_outside_level_zero_are_malformed() {
        // The control: slices inside level 0 (100 bytes) parse.
        probe(&etc1s_with(Desc { rgb: (0, 60), alpha: (60, 40) })).expect("slices inside level 0");
        probe(&etc1s_with(Desc { rgb: (0, 100), alpha: (0, 0) })).expect("no alpha slice");

        let cases = [
            ("RGB slice ends one byte past level 0", Desc { rgb: (41, 60), alpha: (0, 0) }),
            ("RGB slice starts past level 0", Desc { rgb: (101, 1), alpha: (0, 0) }),
            ("alpha slice ends past level 0", Desc { rgb: (0, 60), alpha: (60, 41) }),
            ("alpha slice starts past level 0", Desc { rgb: (0, 60), alpha: (200, 4) }),
            ("RGB offset + length wraps a u32", Desc { rgb: (0xFFFF_FFF0, 0x20), alpha: (0, 0) }),
            ("alpha offset + length wraps a u32", Desc { rgb: (0, 60), alpha: (0xFFFF_FFF0, 0x20) }),
            ("an empty RGB slice", Desc { rgb: (0, 0), alpha: (0, 0) }),
        ];
        for (what, desc) in cases {
            let file = etc1s_with(desc);
            for why in [malformed(probe(&file)), malformed(decode_rgba8(&file))] {
                assert!(why.contains("slice") && why.contains("level 0"), "{what}: {why}");
            }
        }
    }

    /// A Level Index shorter than `levelCount` says (Review Focus 5): the
    /// header says 11 levels, and the file lists fewer.
    #[test]
    fn a_short_level_index_is_malformed() {
        // Eleven levels claimed, one listed: the 11-entry index would end at
        // byte 344 of a file 216 bytes long.
        let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]);
        k.level_count = 11;
        let file = k.build();
        for why in [malformed(probe(&file)), malformed(decode_rgba8(&file))] {
            assert!(why.contains("level index") && why.contains("11"), "{why}");
        }

        // The file ends inside the Level Index.
        let mut ends = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]);
        ends.level_count = 3;
        ends.dfd.clear();
        let whole = ends.build();
        let cut = &whole[..80 + 24]; // the header and one entry
        assert!(malformed(probe(cut)).contains("level index"));

        // A count no file could hold: 4,294,967,295 entries of 24 bytes.
        let mut huge = k.clone();
        huge.level_count = u32::MAX;
        assert!(malformed(probe(&huge.build())).contains("level index"));

        // And the file ends inside the header.
        let clean = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]).build();
        for len in [0, 11, 12, 47, 79] {
            malformed(probe(&clean[..len]));
        }
    }

    #[test]
    fn a_file_that_is_not_ktx2_is_malformed() {
        let mut file = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]).build();
        file[2] ^= 0x20; // "KTX 20" -> "KtX 20"
        assert!(malformed(probe(&file)).contains("identifier"));
        assert!(malformed(probe(b"PNG? no")).contains("KTX"));
    }

    /// A KTX 1.1 file is a valid KTX file this module does not read, and is
    /// named as one: its identifier differs from 2.0's in two bytes.
    #[test]
    fn a_ktx_1_1_file_is_named_as_one() {
        let mut file = vec![0xAB, 0x4B, 0x54, 0x58, 0x20, 0x31, 0x31, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A];
        file.resize(200, 0);
        let why = unsupported(probe(&file));
        assert!(why.contains("KTX 1.1"), "{why}");
        assert!(unsupported(decode_rgba8(&file)).contains("KTX 1.1"));
        // Only the whole identifier is KTX 1.1's: a cut-off one is not named.
        assert!(malformed(probe(&file[..11])).contains("identifier"));
    }

    /// Level 0, the DFD and the SGD must lie inside the file, however their
    /// offsets and lengths add up.
    #[test]
    fn structures_outside_the_file_are_malformed() {
        let sgd = basislz_sgd((1, 1), [&[1], &[2], &[3], &[]], &[Desc { rgb: (0, 4), alpha: (0, 0) }]);
        let rgba = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]).build();
        let basis = Ktx2::etc1s(4, 4, vec![pattern(8, 0)], sgd).build();
        let oversize = rgba.len() as u64;
        let sgd_at = u64::from_le_bytes(basis[64..72].try_into().unwrap());

        // Level 0: byteOffset and byteLength at 80 and 88.
        for (what, offset, length) in [
            ("level 0 runs one byte past the end", oversize - 15, 16u64),
            ("level 0 starts past the end", oversize + 1, 1),
            ("level 0's offset + length wraps a u64", u64::MAX - 7, 16),
            ("level 0's length is a u64 maximum", 200, u64::MAX),
            ("level 0 starts inside the level index", 80, 16),
        ] {
            let mut file = rgba.clone();
            put64(&mut file, 80, offset);
            put64(&mut file, 88, length);
            assert!(malformed(decode_rgba8(&file)).contains("level 0"), "{what}");
        }
        // DFD: dfdByteOffset at 48, dfdByteLength at 52.
        for (what, offset, length) in [
            ("the DFD runs past the end", 104u32, rgba.len() as u32),
            ("the DFD starts past the end", rgba.len() as u32 + 1, 4),
            ("the DFD's offset + length wraps a u32", u32::MAX - 3, 16),
        ] {
            let mut file = rgba.clone();
            put32(&mut file, 48, offset);
            put32(&mut file, 52, length);
            assert!(malformed(probe(&file)).contains("data format descriptor"), "{what}");
        }
        // SGD: sgdByteOffset at 64, sgdByteLength at 72.
        for (what, offset, length) in [
            ("the SGD runs past the end", sgd_at, basis.len() as u64),
            ("the SGD starts past the end", basis.len() as u64 + 1, 8),
            ("the SGD's offset + length wraps a u64", u64::MAX - 7, 16),
            ("the SGD's length is a u64 maximum", sgd_at, u64::MAX),
        ] {
            let mut file = basis.clone();
            put64(&mut file, 64, offset);
            put64(&mut file, 72, length);
            assert!(malformed(probe(&file)).contains("global data"), "{what}");
        }
    }

    #[test]
    fn a_basislz_file_without_global_data_is_malformed() {
        let mut k = Ktx2::etc1s(4, 4, vec![pattern(8, 0)], Vec::new());
        assert!(malformed(probe(&k.build())).contains("global data"));
        // 19 bytes are one short of the header.
        k.sgd = vec![0; 19];
        assert!(malformed(probe(&k.build())).contains("global data"));
    }

    /// The SGD's header says how long its image descriptors and four data
    /// blocks are. Each is checked against the bytes present.
    #[test]
    fn global_data_that_does_not_hold_what_it_declares_is_malformed() {
        let good = basislz_sgd((2, 3), [&[1, 2, 3], &[4, 5], &[6], &[7, 8]], &[Desc { rgb: (0, 4), alpha: (0, 0) }]);
        probe(&Ktx2::etc1s(4, 4, vec![pattern(8, 0)], good.clone()).build()).expect("the control parses");

        // endpointsByteLength, selectorsByteLength, tablesByteLength and
        // extendedByteLength are at 4, 8, 12 and 16 of the SGD.
        for at in [4usize, 8, 12, 16] {
            for bump in [1u32, 1000, u32::MAX] {
                let mut sgd = good.clone();
                let v = u32::from_le_bytes(sgd[at..at + 4].try_into().unwrap());
                put32(&mut sgd, at, v.saturating_add(bump));
                let file = Ktx2::etc1s(4, 4, vec![pattern(8, 0)], sgd).build();
                assert!(malformed(probe(&file)).contains("global data"), "block at {at} grown by {bump}");
            }
        }
        // Two levels listed but one image descriptor present.
        let two = Ktx2::etc1s(8, 8, vec![pattern(8, 0), pattern(8, 1)], good).build();
        assert!(malformed(probe(&two)).contains("image descriptor"));
    }

    /// The parse reads the SGD into its blocks and level 0's slices.
    #[test]
    fn the_basislz_global_data_and_level_zero_slices_are_read() {
        let level0 = pattern(100, 1);
        let sgd = basislz_sgd(
            (258, 515),
            [&[1, 2, 3], &[4, 5], &[6], &[7, 8, 9, 10]],
            &[Desc { rgb: (4, 60), alpha: (64, 30) }, Desc { rgb: (0, 8), alpha: (0, 0) }],
        );
        let file = Ktx2::etc1s(8, 8, vec![level0.clone(), pattern(16, 2)], sgd).build();
        let c = parse(&file).unwrap();
        let g = c.basislz.expect("BasisLZ global data");
        assert_eq!((g.endpoint_count, g.selector_count), (258, 515));
        assert_eq!((g.endpoints, g.selectors, g.tables, g.extended), (&[1u8, 2, 3][..], &[4u8, 5][..], &[6u8][..], &[7u8, 8, 9, 10][..]));
        assert_eq!((g.rgb, g.alpha), (4..64, Some(64..94)));
        assert_eq!(c.level0, &level0[..]);

        // An absent alpha slice (length 0) is None.
        let sgd = basislz_sgd((1, 1), [&[1], &[2], &[3], &[]], &[Desc { rgb: (0, 100), alpha: (u32::MAX, 0) }]);
        let file = Ktx2::etc1s(8, 8, vec![level0], sgd).build();
        let c = parse(&file).unwrap();
        assert_eq!(c.basislz.unwrap().alpha, None);
    }

    /// Level data and the DFD, SGD and every level start at or after the end
    /// of the Level Index. A `levelCount` that makes the index run into them
    /// means the header lies, caught before any level is read.
    #[test]
    fn a_level_index_that_runs_into_what_follows_it_is_malformed() {
        let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0), pattern(4, 1)]);
        k.level_count = 3; // the index would end at 152, and the DFD starts at 128
        let why = malformed(probe(&k.build()));
        assert!(why.contains("level index") && why.contains("data format descriptor"), "{why}");
    }

    /// The same for the global data (the DFD's case is above, a level's below
    /// in `structures_outside_the_file_are_malformed`).
    #[test]
    fn global_data_that_starts_inside_the_level_index_is_malformed() {
        let mut file = etc1s_with(Desc { rgb: (0, 60), alpha: (60, 40) });
        put64(&mut file, 64, 90); // sgdByteOffset: the one entry ends at byte 104
        let why = malformed(probe(&file));
        assert!(why.contains("level index") && why.contains("supercompression global data"), "{why}");
    }

    /// The key/value data is never read, so wherever the header says it is,
    /// and however long, the file parses as it did.
    #[test]
    fn the_key_value_data_is_never_read() {
        let pixels = pattern(16, 3);
        let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pixels.clone()]);
        k.kvd = vec![0xFF; 12];
        let clean = k.build();
        for (offset, length) in [(u32::MAX, u32::MAX), (0, 40), (u32::MAX, 0), (80, 1)] {
            let mut file = clean.clone();
            put32(&mut file, 56, offset); // kvdByteOffset
            put32(&mut file, 60, length); // kvdByteLength
            assert_eq!(probe(&file), probe(&clean), "kvd at {offset}, {length} long");
            assert_eq!(decoded(&file).pixels, pixels, "kvd at {offset}, {length} long");
        }
    }

    /// The messages count in the singular for one entry.
    #[test]
    fn the_level_index_messages_count_their_entries() {
        let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]);
        k.level_count = 0;
        let whole = k.build();
        let mut overlap = whole.clone();
        put32(&mut overlap, 48, 90);
        let ends = &whole[..100]; // inside the one entry
        for why in [malformed(probe(&overlap)), malformed(probe(ends))] {
            assert!(why.contains("level index of 1 entry") && !why.contains("1 entries"), "{why}");
        }
        k.level_count = 11;
        assert!(malformed(probe(&k.build())).contains("level index of 11 entries"));
    }

    // ── format and shape ────────────────────────────────────────────────

    #[test]
    fn probe_names_the_format_dimensions_levels_and_scheme() {
        let rgba = |vk| Ktx2::rgba8(vk, 4, 2, vec![pattern(32, 0), pattern(8, 1)]).build();
        let info = probe(&rgba(R8G8B8A8_UNORM)).unwrap();
        assert_eq!(
            info,
            Ktx2Info {
                vk_format: R8G8B8A8_UNORM,
                format: Ktx2Format::Rgba8 { srgb: false },
                width: 4,
                height: 2,
                level_count: 2,
                supercompression: Supercompression::None
            }
        );
        let info = probe(&rgba(R8G8B8A8_SRGB)).unwrap();
        assert_eq!((info.vk_format, info.format), (R8G8B8A8_SRGB, Ktx2Format::Rgba8 { srgb: true }));

        let sgd = basislz_sgd((1, 1), [&[1], &[2], &[3], &[]], &[Desc { rgb: (0, 8), alpha: (0, 0) }]);
        let info = probe(&Ktx2::etc1s(8, 8, vec![pattern(8, 0)], sgd).build()).unwrap();
        assert_eq!((info.vk_format, info.format, info.supercompression), (0, Ktx2Format::Etc1s, Supercompression::BasisLz));

        for (scheme, want) in [(0, Supercompression::None), (2, Supercompression::Zstd)] {
            let info = probe(&Ktx2::uastc(8, 8, scheme, vec![pattern(64, 0)]).build()).unwrap();
            assert_eq!((info.vk_format, info.format, info.supercompression), (0, Ktx2Format::Uastc, want), "scheme {scheme}");
        }
    }

    #[test]
    fn a_format_this_item_does_not_decode_is_refused_by_number() {
        for vk in [B8G8R8A8_UNORM, BC7_UNORM_BLOCK, 9, 0xFFFF_FFFF] {
            let why = unsupported(probe(&Ktx2::new(vk, 4, 4, vec![pattern(64, 0)]).build()));
            assert!(why.contains(&format!("vkFormat {vk}")), "{why}");
        }
        // vkFormat 0 with a colour model that is neither ETC1S nor UASTC.
        let mut k = Ktx2::new(0, 4, 4, vec![pattern(16, 0)]);
        k.dfd = dfd(167, TRANSFER_LINEAR, 4, 16, &[(0, 127, 3)]);
        let why = unsupported(probe(&k.build()));
        assert!(why.contains("167"), "{why}");
    }

    /// vkFormat 0 says "see the DFD", so a file with no readable DFD, or a
    /// colour model that contradicts its scheme, is not a Basis texture.
    #[test]
    fn basis_files_whose_dfd_contradicts_the_header_are_malformed() {
        let sgd = || basislz_sgd((1, 1), [&[1], &[2], &[3], &[]], &[Desc { rgb: (0, 8), alpha: (0, 0) }]);
        let mut no_dfd = Ktx2::etc1s(8, 8, vec![pattern(8, 0)], sgd());
        no_dfd.dfd.clear();
        assert!(malformed(probe(&no_dfd.build())).contains("data format descriptor"));
        for len in [1, 15, 27] {
            no_dfd.dfd = vec![0; len];
            assert!(malformed(probe(&no_dfd.build())).contains("data format descriptor"), "{len} bytes");
        }

        // BasisLZ with the UASTC model, and UASTC data with the ETC1S model.
        let mut k = Ktx2::etc1s(8, 8, vec![pattern(8, 0)], sgd());
        k.dfd = dfd(MODEL_UASTC, TRANSFER_LINEAR, 4, 16, &[(0, 127, 3)]);
        assert!(malformed(probe(&k.build())).contains("BasisLZ"));
        let mut k = Ktx2::uastc(8, 8, 0, vec![pattern(64, 0)]);
        k.dfd = dfd(MODEL_ETC1S, TRANSFER_SRGB, 4, 0, &[(0, 63, 0)]);
        assert!(malformed(probe(&k.build())).contains("ETC1S"));
        // The message gives the file's scheme by its number.
        for scheme in [0u32, 2] {
            k.scheme = scheme;
            let why = malformed(probe(&k.build()));
            assert!(why.contains(&format!("supercompressionScheme is {scheme}")), "{why}");
        }
        // BasisLZ needs vkFormat 0.
        let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 0)]);
        k.scheme = 1;
        assert!(malformed(probe(&k.build())).contains("BasisLZ"));
    }

    // ── the uncompressed RGBA8 payload ──────────────────────────────────

    #[test]
    fn an_uncompressed_rgba8_texture_decodes_to_its_pixels() {
        for (w, h) in [(1u32, 1u32), (3, 5), (4, 4), (17, 2)] {
            let pixels = pattern((w * h * 4) as usize, w as u8);
            let unorm = decoded(&Ktx2::rgba8(R8G8B8A8_UNORM, w, h, vec![pixels.clone()]).build());
            let srgb = decoded(&Ktx2::rgba8(R8G8B8A8_SRGB, w, h, vec![pixels.clone()]).build());
            assert_eq!((unorm.width, unorm.height), (w, h));
            assert_eq!(unorm.pixels, pixels, "{w}x{h} UNORM");
            // No colour conversion: the stored values come out whatever the tag.
            assert_eq!(srgb.pixels, pixels, "{w}x{h} SRGB");
            assert_eq!((srgb.width, srgb.height), (w, h));
        }
        // The DFD is not needed for it.
        let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(16, 4)]);
        k.dfd.clear();
        assert_eq!(decoded(&k.build()).pixels, pattern(16, 4));
    }

    #[test]
    fn an_uncompressed_level_of_the_wrong_length_is_malformed() {
        for len in [0usize, 15, 17, 32] {
            let file = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![pattern(len, 0)]).build();
            let why = malformed(decode_rgba8(&file));
            assert!(why.contains("level 0") && why.contains("16"), "{len} bytes: {why}");
        }
    }

    // ── Zstandard supercompression (scheme 2) ───────────────────────────

    /// One block of a hand-built Zstandard frame (RFC 8878 §3.1.1.2).
    enum Zb {
        /// `Raw_Block`: the bytes, as they are.
        Raw(Vec<u8>),
        /// `RLE_Block`: one stored byte, repeated `count` times.
        Rle(u8, usize),
    }

    /// A Zstandard frame (RFC 8878 §3.1.1) written field by field: the magic
    /// number, a header with no content size, no checksum and the smallest
    /// power-of-two window that holds the biggest block (a block may not
    /// exceed the window, §3.1.1.2.3), a 4-byte `Dictionary_ID` when
    /// `dictionary` is set, then `blocks`, the last marked as the last. No
    /// compressor is involved, so what a frame inflates to is what the test
    /// says it is.
    fn zstd_frame(dictionary: Option<u32>, blocks: &[Zb]) -> Vec<u8> {
        let size_of = |block: &Zb| match block {
            Zb::Raw(bytes) => bytes.len(),
            Zb::Rle(_, count) => *count,
        };
        let biggest = blocks.iter().map(size_of).max().unwrap_or(0);
        // Window_Descriptor: the exponent in the top 5 bits, window 1 << (10 + exponent), at least 1 KiB.
        let exponent = (0u8..).find(|e| 1usize << (10 + e) >= biggest).unwrap();
        let mut frame = vec![0x28, 0xB5, 0x2F, 0xFD];
        // Frame_Header_Descriptor: Dictionary_ID_flag 3 is a 4-byte ID, 0 none.
        frame.push(if dictionary.is_some() { 3 } else { 0 });
        frame.push(exponent << 3);
        if let Some(id) = dictionary {
            frame.extend(id.to_le_bytes());
        }
        for (i, block) in blocks.iter().enumerate() {
            let last = u32::from(i + 1 == blocks.len());
            let size = size_of(block);
            let (kind, stored): (u32, Vec<u8>) = match block {
                Zb::Raw(bytes) => (0, bytes.clone()),
                Zb::Rle(byte, _) => (1, vec![*byte]),
            };
            assert!(size <= 1 << 17, "a block holds at most 128 KiB");
            // Block_Header: Last_Block, Block_Type (2 bits), Block_Size (21 bits).
            frame.extend(&(last | kind << 1 | (size as u32) << 3).to_le_bytes()[..3]);
            frame.extend(stored);
        }
        frame
    }

    /// `source`, counting the bytes taken from it.
    struct Counting<'a> {
        rest: &'a [u8],
        taken: usize,
    }

    impl std::io::Read for Counting<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = std::io::Read::read(&mut self.rest, buf)?;
            self.taken += n;
            Ok(n)
        }
    }

    /// A UASTC file whose level 0 is `level0`, under supercompression `scheme`.
    fn uastc_file(scheme: u32, width: u32, height: u32, level0: Vec<u8>) -> Vec<u8> {
        Ktx2::uastc(width, height, scheme, vec![level0]).build()
    }

    /// The Zstandard scheme's file for `level0`, a frame.
    fn uastc_zstd(width: u32, height: u32, level0: Vec<u8>) -> Vec<u8> {
        uastc_file(2, width, height, level0)
    }

    /// A UASTC block that decodes: mode 8, a solid colour, every field 0.
    fn solid_block() -> Vec<u8> {
        let mut block = vec![0u8; 16];
        block[0] = 0x17;
        block
    }

    #[test]
    fn a_zstd_rgba8_texture_decodes_to_its_pixels() {
        // Stored in a raw block, and as one repeated byte (an RLE block).
        let pixels = pattern(3 * 5 * 4, 9);
        let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 3, 5, vec![zstd_frame(None, &[Zb::Raw(pixels.clone())])]);
        k.scheme = 2;
        assert_eq!(decoded(&k.build()).pixels, pixels);

        let mut k = Ktx2::rgba8(R8G8B8A8_SRGB, 2, 2, vec![zstd_frame(None, &[Zb::Rle(0xAB, 16)])]);
        k.scheme = 2;
        let img = decoded(&k.build());
        assert_eq!((img.width, img.height, img.pixels), (2, 2, vec![0xAB; 16]));

        // A frame of several blocks is one level.
        let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![zstd_frame(None, &[Zb::Raw(pattern(8, 1)), Zb::Rle(7, 5), Zb::Raw(pattern(3, 2))])]);
        k.scheme = 2;
        let want: Vec<u8> = [pattern(8, 1), vec![7; 5], pattern(3, 2)].concat();
        assert_eq!(decoded(&k.build()).pixels, want);
    }

    #[test]
    fn a_zstd_rgba8_level_of_the_wrong_length_is_malformed() {
        for (len, phrase) in [(15usize, "15 bytes"), (17, "more than the 16 bytes")] {
            let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, 2, 2, vec![zstd_frame(None, &[Zb::Raw(pattern(len, 0))])]);
            k.scheme = 2;
            let why = malformed(decode_rgba8(&k.build()));
            assert!(why.contains("level 0") && why.contains("RGBA8") && why.contains(phrase), "{len}: {why}");
        }
    }

    /// The Zstandard level inflates to the UASTC blocks and no further: the
    /// all-modes texture's blocks, in a frame of two raw blocks and under
    /// scheme 2, give the picture the same blocks give stored plain, which is
    /// the committed reference.
    #[test]
    fn a_zstd_uastc_level_decodes_like_the_same_blocks_stored_plain() {
        let s = sample("uastc_all_modes");
        let blocks = parse(s.ktx2).unwrap().level0.to_vec();
        assert_eq!(blocks.len(), 240 * 16);
        let (first, rest) = blocks.split_at(1000);
        let frame = zstd_frame(None, &[Zb::Raw(first.to_vec()), Zb::Raw(rest.to_vec())]);
        let packed = decoded(&uastc_zstd(s.size.0, s.size.1, frame));
        assert_eq!(first_difference(&packed, s.rgba(), s.size), None);
        assert_eq!(packed, decoded(&uastc_file(0, s.size.0, s.size.1, blocks)));
    }

    /// Ruling K11: the real Zstandard textures' level 0, inflated and put back
    /// as scheme 0, is UASTC blocks an encoder made and must give the
    /// reference, so the UASTC decoder is held to it without the Zstandard
    /// decoder's part in the result.
    #[test]
    fn the_real_zstd_textures_decode_to_the_reference_as_plain_uastc_blocks() {
        for stem in ["FlightHelmet_Materials_LensesMat_Normal", "Rib_N"] {
            let s = sample(stem);
            let c = parse(s.ktx2).unwrap();
            assert_eq!(c.info.supercompression, Supercompression::Zstd, "{stem}");
            let blocks = zstd_bytes(c.level0);
            assert_eq!(blocks.len() as u64, uastc::level_len(s.size.0, s.size.1), "{stem}: the frame inflates to the level");
            let img = decoded(&uastc_file(0, s.size.0, s.size.1, blocks));
            assert_eq!(first_difference(&img, s.rgba(), s.size), None, "{stem} as scheme 0");
        }
    }

    #[test]
    fn a_zstd_payload_longer_than_its_level_is_malformed() {
        // A 4x4 UASTC level is one block, 16 bytes: a frame of 17 is past it.
        let file = uastc_zstd(4, 4, zstd_frame(None, &[Zb::Raw(pattern(17, 0))]));
        let why = malformed(decode_rgba8(&file));
        assert!(why.contains("level 0") && why.contains("Zstandard") && why.contains("more than the 16 bytes"), "{why}");
        // And the exact length is the control: the same frame, 16 bytes.
        assert!(decode_rgba8(&uastc_zstd(4, 4, zstd_frame(None, &[Zb::Raw(solid_block())]))).is_ok());
    }

    #[test]
    fn a_zstd_payload_shorter_than_its_level_is_malformed() {
        for len in [0usize, 1, 15] {
            let file = uastc_zstd(4, 4, zstd_frame(None, &[Zb::Raw(pattern(len, 0))]));
            let why = malformed(decode_rgba8(&file));
            assert!(why.contains("level 0") && why.contains("Zstandard") && why.contains(&format!("to {len} bytes, but")) && why.contains("is 16"), "{len}: {why}");
        }
    }

    /// A frame that would inflate to a quarter of a gigabyte is read only as
    /// far as ruzstd's window lets the cap see it: here the window is the
    /// smallest that holds a block, so the decoder takes a few bytes of the
    /// frame, not all, and the result is `Malformed`. The frame is 2,000
    /// blocks of 128 KiB, each stored as one byte. (With a bigger window the
    /// decoder reads more first: the next test.)
    #[test]
    fn a_zstd_frame_past_the_cap_stops_at_the_cap() {
        let bomb = zstd_frame(None, &(0..2_000).map(|_| Zb::Rle(0, 1 << 17)).collect::<Vec<_>>());
        assert!(bomb.len() > 8_000, "the frame is {} bytes, thousands of blocks", bomb.len());
        let mut source = Counting { rest: &bomb, taken: 0 };
        let why = malformed(inflate_reading(&mut source, 16, "UASTC image"));
        assert!(why.contains("more than the 16 bytes"), "{why}");
        // The header is 6 bytes and a block 4; the 128 KiB window holds one
        // block, so the decoder needs a second before it gives a byte: it
        // stopped within a few blocks.
        assert!(source.taken < 40, "it took {} of the frame's {} bytes", source.taken, bomb.len());
        // The same through the file.
        let why = malformed(decode_rgba8(&uastc_zstd(4, 4, bomb)));
        assert!(why.contains("more than the 16 bytes"), "{why}");
    }

    #[test]
    fn a_zstd_dictionary_frame_is_refused() {
        let level = |dictionary| uastc_zstd(4, 4, zstd_frame(dictionary, &[Zb::Raw(solid_block())]));
        // The control: the same frame without a dictionary decodes.
        assert!(decode_rgba8(&level(None)).is_ok());
        for id in [1u32, 0x1234_5678, u32::MAX] {
            let why = malformed(decode_rgba8(&level(Some(id))));
            assert!(why.contains("dictionary") && why.contains(&id.to_string()), "dictionary {id}: {why}");
        }
        // Dictionary_ID 0 means no dictionary (RFC 8878 §3.1.1.1.2).
        assert!(decode_rgba8(&level(Some(0))).is_ok());
    }

    #[test]
    fn a_zstd_level_that_is_not_one_frame_is_malformed() {
        let frame = zstd_frame(None, &[Zb::Raw(solid_block())]);
        let cases: [(&str, Vec<u8>, &str); 6] = [
            ("not a frame", pattern(32, 5), "Zstandard"),
            ("empty", Vec::new(), "Zstandard"),
            ("cut off", frame[..frame.len() - 3].to_vec(), "Zstandard"),
            ("trailing bytes", [frame.clone(), vec![0; 3]].concat(), "3 bytes follow"),
            ("a second frame", [frame.clone(), frame.clone()].concat(), "bytes follow"),
            // Skippable frames (magic 0x184D2A50 to 5F) are not the frame.
            ("a skippable frame", vec![0x50, 0x2A, 0x4D, 0x18, 4, 0, 0, 0, 0, 0, 0, 0], "Zstandard"),
        ];
        for (name, level0, phrase) in cases {
            let why = malformed(decode_rgba8(&uastc_zstd(4, 4, level0)));
            assert!(why.contains("level 0") && why.contains(phrase), "{name}: {why}");
        }
    }

    /// ruzstd sizes its decode buffer from the window the frame's header names
    /// and decodes up to a window before it gives a byte, so a header naming
    /// more window than a level could use is refused before a block is read:
    /// the limit is the level's size, or 8 MiB if that is more (the floor, for
    /// frames written without being told the size).
    #[test]
    fn a_zstd_window_past_the_limit_is_refused() {
        let level = |window_descriptor: u8| {
            let mut frame = zstd_frame(None, &[Zb::Raw(solid_block())]);
            frame[5] = window_descriptor;
            decode_rgba8(&uastc_zstd(4, 4, frame))
        };
        // Window_Descriptor: exponent in the top 5 bits, window 1 << (10 + exponent).
        let why = malformed(level(14 << 3)); // 16 MiB
        assert!(why.contains("window") && why.contains("16777216"), "{why}");
        let why = malformed(level(17 << 3)); // 128 MiB
        assert!(why.contains("window"), "{why}");
        // At the limit it is read.
        assert!(level(13 << 3).is_ok(), "8 MiB");
    }

    /// A frame that declares its content size (RFC 8878 §3.1.1.1.4) is
    /// checked against the level before any block is decoded.
    #[test]
    fn a_zstd_frame_that_declares_another_size_is_malformed() {
        // Single_Segment (0x20) with a 1-byte Frame_Content_Size: no
        // Window_Descriptor, no Dictionary_ID.
        let frame = |declared: u8, content: Vec<u8>| {
            let mut f = vec![0x28, 0xB5, 0x2F, 0xFD, 0x20, declared];
            f.extend(&(1u32 | (content.len() as u32) << 3).to_le_bytes()[..3]);
            f.extend(content);
            f
        };
        assert!(decode_rgba8(&uastc_zstd(4, 4, frame(16, solid_block()))).is_ok(), "the control: 16 declared, 16 stored");
        let why = malformed(decode_rgba8(&uastc_zstd(4, 4, frame(200, solid_block()))));
        assert!(why.contains("declares 200") && why.contains("16"), "{why}");
    }

    /// The cap cannot stop ruzstd before it has decoded a window's worth, so
    /// the window is what bounds the work a frame costs. A frame naming the
    /// floor window (8 MiB) and going on with 128 KiB RLE blocks for a
    /// 16-byte level is refused once ruzstd has decoded 64 blocks to fill the
    /// window and one more to have a byte to give: 6 header bytes and 4 a
    /// block, 266 of the frame's 1,206.
    #[test]
    fn a_zstd_frame_with_an_8_mib_window_is_refused_after_a_bounded_part_of_its_source() {
        let mut frame = zstd_frame(None, &(0..300).map(|_| Zb::Rle(0, 1 << 17)).collect::<Vec<_>>());
        frame[5] = 13 << 3; // Window_Descriptor: 1 << 23
        assert_eq!(frame.len(), 6 + 4 * 300);
        let mut source = Counting { rest: &frame, taken: 0 };
        let why = malformed(inflate_reading(&mut source, 16, "UASTC image"));
        assert!(why.contains("more than the 16 bytes"), "{why}");
        assert_eq!(source.taken, 6 + 4 * 65, "the 64 blocks of the window and one more, of {} bytes", frame.len());
    }

    /// A level of 9 MiB (2048 x 1152 RGBA8) is bigger than the 8 MiB floor,
    /// so the window limit is the level's own size: a single-segment frame,
    /// the kind `ZSTD_compress` writes, has the content size for its window and
    /// is read. One byte more window than the level is refused. (The limit is
    /// `max(level, floor)`; with the floor alone, this level would be refused.)
    #[test]
    fn a_single_segment_zstd_frame_of_a_level_past_the_floor_is_read_and_one_byte_more_is_refused() {
        let (width, height) = (2048u32, 1152u32);
        let size = width as usize * height as usize * 4;
        assert!(size as u64 > ZSTD_WINDOW_FLOOR && size == 72 << 17, "a level of 72 blocks of 128 KiB, past the floor");
        // Frame_Header_Descriptor 0xA0: a 4-byte Frame_Content_Size and Single_Segment, so no Window_Descriptor.
        let frame = |declared: usize| {
            let mut f = vec![0x28, 0xB5, 0x2F, 0xFD, 0xA0];
            f.extend((declared as u32).to_le_bytes());
            for i in 0..72u32 {
                let last = u32::from(i == 71);
                f.extend(&(last | 1 << 1 | (1u32 << 17) << 3).to_le_bytes()[..3]); // an RLE block of 128 KiB
                f.push(0xAB);
            }
            f
        };
        let file = |declared: usize| {
            let mut k = Ktx2::rgba8(R8G8B8A8_UNORM, width, height, vec![frame(declared)]);
            k.scheme = 2;
            k.build()
        };
        assert!(frame(size).len() < 100_000, "{} bytes of frame", frame(size).len());
        let img = decoded(&file(size));
        assert!(img.pixels.len() == size && img.pixels.iter().all(|&b| b == 0xAB), "the level is 9 MiB of 0xAB");
        assert_eq!(img.pixels.capacity(), size, "the pixels are exactly the level, not a grown buffer");
        // One byte more than the level: its window is past the limit.
        let why = malformed(decode_rgba8(&file(size + 1)));
        assert!(why.contains("window") && why.contains(&(size + 1).to_string()), "{why}");
    }

    /// A frame with `Content_Checksum_flag` set ends with 4 more bytes, which
    /// are part of it: they are taken, and nothing is left. A checksum cut
    /// short is `Malformed`. (ruzstd is built without its `hash` feature, so
    /// the value is read and not checked.)
    #[test]
    fn a_zstd_frame_with_a_content_checksum_has_its_four_bytes_taken() {
        let mut frame = zstd_frame(None, &[Zb::Raw(solid_block())]);
        frame[4] |= 0x04; // Content_Checksum_flag
        frame.extend([0xDE, 0xAD, 0xBE, 0xEF]);
        let mut source = Counting { rest: &frame, taken: 0 };
        let out = inflate_reading(&mut source, 16, "UASTC image").unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(out, solid_block());
        assert_eq!((source.taken, source.rest.len()), (frame.len(), 0), "the whole frame, checksum included, was taken");
        assert!(decode_rgba8(&uastc_zstd(4, 4, frame.clone())).is_ok(), "and nothing follows it in the file");
        for kept in 0..4 {
            let cut = frame[..frame.len() - 4 + kept].to_vec();
            let why = malformed(decode_rgba8(&uastc_zstd(4, 4, cut)));
            assert!(why.contains("level 0") && why.contains("Zstandard"), "{kept} of 4 checksum bytes: {why}");
        }
    }

    /// The level is reserved once, at its size, before any byte is inflated;
    /// a size that cannot be set aside is `Unsupported` with that size, not a
    /// panic in the allocator (which is an abort on wasm32).
    #[test]
    fn a_level_that_cannot_be_set_aside_is_unsupported_not_a_panic() {
        let frame = zstd_frame(None, &[Zb::Raw(solid_block())]);
        let mut source: &[u8] = &frame;
        let why = unsupported(inflate_reading(&mut source, usize::MAX, "huge image"));
        assert!(why.contains(&usize::MAX.to_string()) && why.contains("huge image"), "{why}");
    }

    // ── hostile input ───────────────────────────────────────────────────

    /// `layer_sweep` over `file` (cut at every length, every bit of it flipped
    /// in turn, and 10,000 seeded cases of 2 to 8 flipped bits), reading each
    /// case with `probe` and `decode_rgba8`. What a hostile file may give is a
    /// decode whose pixels match its size, `Malformed` or `Unsupported`:
    /// anything else, or a panic, fails with the case. The sweep also counts
    /// what `probe` gave, and each of the three must have happened, or the
    /// sweep never reached the code it is there for.
    fn sweep(name: &str, file: &[u8]) {
        sweep_layers(name, file, 0..file.len());
    }

    /// [`sweep`], with `layers` as the span of the file whose every bit is
    /// flipped past its first 4 KiB, for a file too big to flip all of.
    fn sweep_layers(name: &str, file: &[u8], layers: std::ops::Range<usize>) {
        sweep_reading(name, file, layers, true);
    }

    /// [`sweep_layers`], reading each case with `probe` alone when `decode` is
    /// false: for a file whose decode is too slow to run on every case of a
    /// sweep (a 1024x1024 ETC1S image, tens of milliseconds a case in a debug
    /// build). The counts are (`probe`'s, `decode_rgba8`'s) as (Ok, Malformed,
    /// Unsupported); the decode counts stay 0 when `decode` is false.
    fn sweep_reading(name: &str, file: &[u8], layers: std::ops::Range<usize>, decode: bool) -> ([usize; 3], [usize; 3]) {
        let counts = Outcomes::default();
        layer_sweep(name, file, layers, |bytes| counts.read(bytes, decode));
        counts.report(name, decode)
    }

    /// What a sweep's readings gave, as (Ok, Malformed, Unsupported) for
    /// `probe` and for `decode_rgba8`.
    #[derive(Default)]
    struct Outcomes {
        probed: [std::cell::Cell<usize>; 3],
        decoded: [std::cell::Cell<usize>; 3],
    }

    impl Outcomes {
        /// One reading of `bytes`: what a hostile file may give is a decode
        /// whose pixels match its size, `Malformed` or `Unsupported`, and
        /// anything else fails.
        fn read(&self, bytes: &[u8], decode: bool) {
            let count = |cells: &[std::cell::Cell<usize>; 3], at: usize| cells[at].set(cells[at].get() + 1);
            match probe(bytes) {
                Ok(_) => count(&self.probed, 0),
                Err(MeshError::Malformed(_)) => count(&self.probed, 1),
                Err(MeshError::Unsupported(_)) => count(&self.probed, 2),
                Err(other) => panic!("probe gave {other:?}"),
            }
            if !decode {
                return;
            }
            match decode_rgba8(bytes) {
                Ok(img) => {
                    assert_eq!(img.pixels.len() as u64, u64::from(img.width) * u64::from(img.height) * 4);
                    count(&self.decoded, 0);
                }
                Err(MeshError::Malformed(_)) => count(&self.decoded, 1),
                Err(MeshError::Unsupported(_)) => count(&self.decoded, 2),
                Err(other) => panic!("decode gave {other:?}"),
            }
        }

        /// Print the counts, and check that `probe` gave all three outcomes.
        fn report(&self, name: &str, decode: bool) -> ([usize; 3], [usize; 3]) {
            let probed = self.probed.clone().map(|c| c.get());
            let decoded = self.decoded.clone().map(|c| c.get());
            eprintln!("{name}: probe gave {} Ok, {} Malformed, {} Unsupported", probed[0], probed[1], probed[2]);
            if decode {
                eprintln!("{name}: decode gave {} Ok, {} Malformed, {} Unsupported", decoded[0], decoded[1], decoded[2]);
            }
            assert!(probed.iter().all(|&n| n > 0), "{name}: the sweep reached only some outcomes");
            (probed, decoded)
        }
    }

    /// The uncompressed file, with two levels laid smallest first.
    fn sweep_rgba8() -> Vec<u8> {
        Ktx2::rgba8(R8G8B8A8_SRGB, 4, 4, vec![pattern(64, 5), pattern(16, 6)]).build()
    }

    /// A BasisLZ file with an alpha slice, two levels and a full SGD.
    fn sweep_etc1s() -> Vec<u8> {
        let sgd = basislz_sgd(
            (4, 8),
            [&pattern(12, 1), &pattern(20, 2), &pattern(9, 3), &pattern(5, 4)],
            &[Desc { rgb: (0, 40), alpha: (40, 24) }, Desc { rgb: (0, 8), alpha: (8, 8) }],
        );
        Ktx2::etc1s(16, 8, vec![pattern(64, 7), pattern(16, 8)], sgd).build()
    }

    /// UASTC with Zstandard, eleven levels listed under a count of 11 (the
    /// Review Focus 5 shape, whole).
    fn sweep_uastc() -> Vec<u8> {
        Ktx2::uastc(1024, 1024, 2, (0..11).map(|i| pattern(20 + i, i as u8)).collect()).build()
    }

    #[test]
    fn the_uncompressed_container_survives_truncation_and_bit_flips() {
        sweep("uncompressed RGBA8 KTX2", &sweep_rgba8());
    }

    #[test]
    fn the_basislz_container_survives_truncation_and_bit_flips() {
        sweep("ETC1S/BasisLZ KTX2", &sweep_etc1s());
    }

    #[test]
    fn the_uastc_zstd_container_survives_truncation_and_bit_flips() {
        sweep("UASTC + Zstandard KTX2", &sweep_uastc());
    }

    // ── the committed corpus: real textures from Khronos (testdata/ktx2/) ──

    /// A real KTX2 texture and its reference. The reference is level 0, layer
    /// 0, face 0 as raw RGBA8, written by `make_reference.mjs` with the Basis
    /// Universal transcoder bundled in three.js (see the folder's README), and
    /// committed Zstandard-compressed by the `zstd` command line tool as
    /// `<stem>.rgba.zst` (Ruling K10: the raw files were 12.7 MB). The decoders
    /// decode `ktx2` and compare with [`Sample::rgba`], the reference inflated
    /// by `ruzstd` itself. What each file should be is written here from the
    /// transcoder's own report and the file's header read by a script, not
    /// copied from `probe`.
    struct Sample {
        /// The upstream file name without `.ktx2`; the reference is `<stem>.rgba.zst`.
        stem: &'static str,
        ktx2: &'static [u8],
        /// The reference, one Zstandard frame.
        rgba_zst: &'static [u8],
        /// The file's size in bytes, as upstream has it.
        len: usize,
        format: Ktx2Format,
        scheme: Supercompression,
        size: (u32, u32),
        levels: u32,
        /// Level 0 has an alpha slice (`alphaSliceByteLength` past 0), which
        /// only ETC1S/BasisLZ has. For UASTC it is whether the reference has
        /// any alpha under 255.
        alpha: bool,
    }

    /// `frame` inflated by `ruzstd` directly, not through [`inflate`], so a
    /// reference is read by something the code under test does not wrap: the
    /// frame is whole, whatever its window, and it ends where the data does.
    fn zstd_bytes(frame: &[u8]) -> Vec<u8> {
        let mut decoder = ruzstd::decoding::StreamingDecoder::new(frame).expect("a Zstandard frame");
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut out).expect("a frame that inflates");
        out
    }

    impl Sample {
        /// The reference as raw RGBA8, width x height x 4 bytes, inflated on the
        /// first call and kept for the rest of the run (four of them are 4 MiB,
        /// and a dozen tests read them).
        fn rgba(&self) -> &'static [u8] {
            static INFLATED: std::sync::Mutex<Vec<(&'static str, &'static [u8])>> = std::sync::Mutex::new(Vec::new());
            let mut kept = INFLATED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((_, bytes)) = kept.iter().find(|(stem, _)| *stem == self.stem) {
                return bytes;
            }
            let bytes: &'static [u8] = Box::leak(zstd_bytes(self.rgba_zst).into_boxed_slice());
            kept.push((self.stem, bytes));
            bytes
        }
    }

    macro_rules! sample {
        ($stem:literal, $len:expr, $format:expr, $scheme:expr, $size:expr, $levels:expr, $alpha:expr) => {
            Sample {
                stem: $stem,
                ktx2: include_bytes!(concat!("testdata/ktx2/", $stem, ".ktx2")),
                rgba_zst: include_bytes!(concat!("testdata/ktx2/", $stem, ".rgba.zst")),
                len: $len,
                format: $format,
                scheme: $scheme,
                size: $size,
                levels: $levels,
                alpha: $alpha,
            }
        };
    }

    /// FlightHelmet is CC0; CarConcept and StainedGlassLamp are CC BY 4.0
    /// (the README gives the attribution); `uastc_all_modes` is ours.
    const CORPUS: [Sample; 6] = [
        sample!("FlightHelmet_Materials_LensesMat_Normal", 1_685, Ktx2Format::Uastc, Supercompression::Zstd, (1024, 1024), 11, false),
        sample!("FlightHelmet_Materials_LensesMat_BaseColor", 93_420, Ktx2Format::Etc1s, Supercompression::BasisLz, (1024, 1024), 11, false),
        sample!("Rib_N", 622, Ktx2Format::Uastc, Supercompression::Zstd, (32, 32), 6, false),
        sample!("Dot_C", 1_374, Ktx2Format::Etc1s, Supercompression::BasisLz, (128, 128), 8, false),
        sample!("StainedGlassLamp_grill_basecolor-alpha", 217_120, Ktx2Format::Etc1s, Supercompression::BasisLz, (1024, 1024), 11, true),
        sample!("uastc_all_modes", 4_000, Ktx2Format::Uastc, Supercompression::None, (62, 59), 1, true),
    ];

    /// The folder's README, embedded like every other fixture. It documents
    /// each file's origin and licence, so it must list the same files as
    /// `CORPUS`: a file that is embedded but undocumented, or documented but
    /// not embedded, is caught without reading the folder.
    const README: &str = include_str!("testdata/ktx2/README.md");

    /// The `.ktx2` and `.rgba.zst` names that `README`'s tables list: the first
    /// cell of a table row, in backticks, such as ``| `Rib_N.ktx2` | ...``.
    /// Prose that mentions a name generically (`<name>.rgba`) is not a row.
    fn readme_files() -> Vec<&'static str> {
        let mut names: Vec<&str> = README
            .lines()
            .filter_map(|line| line.strip_prefix("| `"))
            .filter_map(|rest| rest.split('`').next())
            .filter(|name| name.ends_with(".ktx2") || name.ends_with(".rgba.zst"))
            .collect();
        names.sort_unstable();
        names
    }

    /// The README's tables list exactly the files `CORPUS` embeds, each once.
    /// Then each file is the one upstream published (its size), parses as the
    /// format, size, levels and scheme the sample says, and has a reference
    /// of the right length whose alpha is what the sample says.
    #[test]
    fn the_corpus_parses() {
        let mut embedded: Vec<String> = CORPUS.iter().flat_map(|s| [format!("{}.ktx2", s.stem), format!("{}.rgba.zst", s.stem)]).collect();
        embedded.sort_unstable();
        let listed = readme_files();
        let missing: Vec<&String> = embedded.iter().filter(|n| !listed.contains(&n.as_str())).collect();
        let unexpected: Vec<&&str> = listed.iter().filter(|n| !embedded.iter().any(|e| e == **n)).collect();
        assert!(
            missing.is_empty() && unexpected.is_empty() && listed.len() == embedded.len(),
            "README.md's tables do not list the embedded corpus: they lack {missing:?}, list {unexpected:?} that is not embedded, and name {} files for {} embedded",
            listed.len(),
            embedded.len(),
        );

        // Every sample is checked, and every disagreement reported, so a
        // failure names all that is wrong, not the first.
        let mut problems: Vec<String> = Vec::new();
        for s in &CORPUS {
            let name = s.stem;
            if s.ktx2.len() != s.len {
                problems.push(format!("{name}.ktx2 is {} bytes, not the {} upstream publishes", s.ktx2.len(), s.len));
            }
            let want = Ktx2Info {
                vk_format: 0,
                format: s.format,
                width: s.size.0,
                height: s.size.1,
                level_count: s.levels,
                supercompression: s.scheme,
            };
            match probe(s.ktx2) {
                Ok(info) if info == want => {}
                other => problems.push(format!("{name}.ktx2 probes as {other:?}, not {want:?}")),
            }
            // ETC1S: whether level 0's image descriptor has an alpha slice.
            // Anything else has no BasisLZ global data to ask.
            let alpha_slice = parse(s.ktx2).ok().and_then(|c| c.basislz).map(|g| g.alpha.is_some());
            let want_slice = (s.format == Ktx2Format::Etc1s).then_some(s.alpha);
            if alpha_slice != want_slice {
                problems.push(format!("{name}.ktx2: level 0's alpha slice is {alpha_slice:?}, not {want_slice:?}"));
            }
            let pixels = u64::from(s.size.0) * u64::from(s.size.1);
            if s.rgba().len() as u64 != pixels * 4 {
                problems.push(format!("{name}.rgba.zst inflates to {} bytes, not width x height x 4 = {}", s.rgba().len(), pixels * 4));
            }
            let translucent = s.rgba().chunks_exact(4).filter(|p| p[3] != 255).count();
            if (translucent > 0) != s.alpha {
                problems.push(format!("{name}.rgba.zst has {translucent} of {pixels} pixels with alpha under 255, but the sample says alpha: {}", s.alpha));
            }
        }
        assert!(problems.is_empty(), "the corpus does not hold:\n{}", problems.join("\n"));
    }

    /// `sweep_layers` over a sample: every cut and seeded case, and every bit
    /// of the first 4 KiB (the header, the Index, the Level Index, the DFD and
    /// the start of the global data) and of the whole supercompression global
    /// data, which the 93 KB and 217 KB files hold past 4 KiB. The span is read
    /// from the Index at the offsets §3.9 gives (`sgdByteOffset` at 64,
    /// `sgdByteLength` at 72), not from `parse`. The level data past it is
    /// not flipped bit by bit.
    ///
    /// A 1024x1024 file is read with `probe` alone: with a decoder, a case that
    /// keeps the container valid decodes a whole 1024x1024 picture (tens of
    /// milliseconds in a debug build for ETC1S, a second for UASTC with
    /// Zstandard, and tens of thousands of cases do), so the decoders' own
    /// hostile-input sweeps are on the strips of [`etc1s_strip_sweep`] and
    /// [`uastc_strip_sweep`] instead, which keep the file's real tables or
    /// blocks. The same goes for the all-modes file ([`all_modes_sweep`]).
    fn corpus_sweep(stem: &str) {
        let s = CORPUS.iter().find(|s| s.stem == stem).expect("a sample of the corpus");
        assert_eq!(s.ktx2.len(), s.len, "{stem}.ktx2 is not the size upstream publishes");
        let at = |offset: usize| u64::from_le_bytes(s.ktx2[offset..offset + 8].try_into().unwrap()) as usize;
        let sgd = at(64)..at(64) + at(72);
        assert!(sgd.end <= s.ktx2.len(), "{stem}: the global data {sgd:?} is outside the file");
        // UASTC decodes in microseconds a block, so a file of 64 blocks is read
        // whole by every case. The 240 blocks of the all-modes file cost half a
        // millisecond a case (46,000 cases, forty seconds in a debug build), and
        // the 1024x1024 one 65,536 blocks and a megabyte of Zstandard output a
        // case (a second): their decoders are swept on their own, below.
        let decode = s.format == Ktx2Format::Uastc && u64::from(s.size.0) * u64::from(s.size.1) <= 1024;
        sweep_reading(stem, s.ktx2, sgd, decode);
    }

    #[test]
    fn the_flighthelmet_lenses_normal_texture_survives_truncation_and_bit_flips() {
        corpus_sweep("FlightHelmet_Materials_LensesMat_Normal");
    }

    #[test]
    fn the_flighthelmet_lenses_basecolor_texture_survives_truncation_and_bit_flips() {
        corpus_sweep("FlightHelmet_Materials_LensesMat_BaseColor");
    }

    #[test]
    fn the_carconcept_rib_normal_texture_survives_truncation_and_bit_flips() {
        corpus_sweep("Rib_N");
    }

    #[test]
    fn the_carconcept_dot_colour_texture_survives_truncation_and_bit_flips() {
        corpus_sweep("Dot_C");
    }

    #[test]
    fn the_stainedglass_grill_alpha_texture_survives_truncation_and_bit_flips() {
        corpus_sweep("StainedGlassLamp_grill_basecolor-alpha");
    }

    #[test]
    fn the_uastc_all_modes_texture_survives_truncation_and_bit_flips() {
        corpus_sweep("uastc_all_modes");
    }

    // ── ETC1S/BasisLZ decoding (KTX2 Appendix D) ────────────────────────

    use super::etc1s::{decode_slice, put_alpha, put_block, read_selectors, Bits, Huffman, Models};

    /// A corpus sample by its stem.
    fn sample(stem: &str) -> &'static Sample {
        CORPUS.iter().find(|s| s.stem == stem).unwrap_or_else(|| panic!("no sample {stem}"))
    }

    fn get32(file: &[u8], at: usize) -> usize {
        u32::from_le_bytes(file[at..at + 4].try_into().unwrap()) as usize
    }
    fn get64(file: &[u8], at: usize) -> usize {
        u64::from_le_bytes(file[at..at + 8].try_into().unwrap()) as usize
    }

    /// `None` if `got` is `want` byte for byte; otherwise the first differing
    /// pixel (in raster order), the 4x4 block it is in, and how many differ.
    fn first_difference(got: &Rgba8, want: &[u8], size: (u32, u32)) -> Option<String> {
        if (got.width, got.height) != size {
            return Some(format!("decoded {}x{}, expected {}x{}", got.width, got.height, size.0, size.1));
        }
        if got.pixels.len() != want.len() {
            return Some(format!("decoded {} bytes, expected {}", got.pixels.len(), want.len()));
        }
        let first = got.pixels.iter().zip(want).position(|(a, b)| a != b)? / 4;
        let (x, y) = (first % size.0 as usize, first / size.0 as usize);
        let differing = got.pixels.chunks_exact(4).zip(want.chunks_exact(4)).filter(|(a, b)| a != b).count();
        Some(format!(
            "first differing pixel ({x}, {y}), in block ({}, {}) at ({}, {}) within it: got {:?}, want {:?}; {differing} of {} pixels differ",
            x / 4,
            y / 4,
            x % 4,
            y % 4,
            &got.pixels[first * 4..first * 4 + 4],
            &want[first * 4..first * 4 + 4],
            want.len() / 4
        ))
    }

    /// Level 0 of every input of the corpus (three ETC1S, two UASTC with
    /// Zstandard, and the UASTC all-modes texture), each transcoded by Basis
    /// Universal v1.50 (three.js r183's bundled transcoder, `make_reference.mjs`
    /// and `make_uastc_modes.mjs`): the decoder must give the same bytes, every
    /// pixel. ETC1S and UASTC to RGBA are deterministic, so any difference is a
    /// decoder bug, not a tolerance.
    #[test]
    fn every_reference_texture_transcodes_bit_exactly() {
        let mut problems = Vec::new();
        let (mut etc1s, mut uastc) = (0, 0);
        for s in &CORPUS {
            match s.format {
                Ktx2Format::Etc1s => etc1s += 1,
                _ => uastc += 1,
            }
            match decode_rgba8(s.ktx2) {
                Ok(img) => {
                    if let Some(why) = first_difference(&img, s.rgba(), s.size) {
                        problems.push(format!("{}: {why}", s.stem));
                    }
                }
                Err(e) => problems.push(format!("{}: {e:?}", s.stem)),
            }
        }
        assert_eq!((etc1s, uastc), (3, 3), "the corpus holds three ETC1S and three UASTC inputs");
        assert!(problems.is_empty(), "not bit-exact against the Basis Universal v1.50 reference:\n{}", problems.join("\n"));
    }

    /// The grill's alpha slice is its own ETC1S stream, and its pixels' alpha
    /// is the alpha slice's green channel (what the transcoder takes), so
    /// translucent pixels come out as translucent, not opaque and not as
    /// their colour.
    #[test]
    fn an_etc1s_alpha_slice_becomes_alpha() {
        let s = sample("StainedGlassLamp_grill_basecolor-alpha");
        assert!(parse(s.ktx2).unwrap().basislz.unwrap().alpha.is_some(), "the grill has an alpha slice");
        let img = decoded(s.ktx2);
        let alpha = |pixels: &[u8]| pixels.iter().skip(3).step_by(4).copied().collect::<Vec<u8>>();
        let (got, want) = (alpha(&img.pixels), alpha(s.rgba()));
        let translucent = want.iter().filter(|&&a| a != 255).count();
        assert!(translucent > 500_000, "the reference has {translucent} translucent pixels");
        assert_eq!(got.iter().filter(|&&a| a != 255).count(), translucent, "translucent pixels");
        assert!(got == want, "the alpha channel is not the reference's");
        assert_eq!(first_difference(&img, s.rgba(), s.size), None);
    }

    /// Review Focus 2: a file whose DFD declares alpha but whose level 0 has
    /// no alpha slice (`alphaSliceByteLength` 0) decodes opaque, and its
    /// colours are the reference's. Built from the grill by zeroing the alpha
    /// slice's length in level 0's image descriptor.
    #[test]
    fn an_absent_alpha_slice_is_opaque() {
        let s = sample("StainedGlassLamp_grill_basecolor-alpha");
        let mut file = s.ktx2.to_vec();
        assert_eq!(get32(&file, 52), 60, "the grill's DFD declares two samples (RGB and alpha)");
        let desc0 = get64(&file, 64) + 20; // the first ImageDesc follows the 20-byte global data header
        assert!(get32(&file, desc0 + 16) > 0, "the grill has an alpha slice to remove");
        put32(&mut file, desc0 + 16, 0); // alphaSliceByteLength
        let img = decoded(&file);
        assert_eq!((img.width, img.height), s.size);
        assert!(img.pixels.iter().skip(3).step_by(4).all(|&a| a == 255), "alpha is 255 everywhere");
        let rgb = |pixels: &[u8]| pixels.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect::<Vec<u8>>();
        assert!(rgb(&img.pixels) == rgb(s.rgba()), "the colours are the reference's");
    }

    /// The tags in the DFD say how a renderer should read the colours; the
    /// decoder reads none of them. Flipping the transfer function between
    /// sRGB and linear, or changing the primaries or the flags, leaves the
    /// bytes as they were.
    #[test]
    fn dfd_tags_never_change_the_bytes() {
        let s = sample("Dot_C");
        let base = decoded(s.ktx2);
        assert_eq!(first_difference(&base, s.rgba(), s.size), None);
        let dfd = get32(s.ktx2, 48); // dfdByteOffset
        // The basic block's fourth word: colorModel, colorPrimaries, transferFunction, flags.
        assert_eq!(s.ktx2[dfd + 12], 163, "Dot_C is ETC1S");
        assert_eq!(s.ktx2[dfd + 14], 2, "Dot_C is tagged sRGB");
        for (what, at, value) in [
            ("transfer function linear", 14, 1u8),
            ("transfer function unspecified", 14, 0),
            ("primaries BT.2020", 13, 2),
            ("primaries unspecified", 13, 0),
            ("alpha premultiplied", 15, 1),
        ] {
            let mut file = s.ktx2.to_vec();
            file[dfd + at] = value;
            assert_eq!(decoded(&file), base, "{what}");
        }
    }

    /// Review Focus 1: the block grid is ceil(width/4) x ceil(height/4) and
    /// the output stride is exactly `width` pixels, no padding. No
    /// non-power-of-two input exists in the corpus (nothing in the Khronos
    /// sample set, Ruling K11), so the arithmetic is pinned where the decoded
    /// blocks are placed: a 1020 x 516 image of 255 x 129 distinct
    /// constant-colour blocks, every pixel checked, and an image whose size
    /// is not a multiple of four, where the last blocks are clipped. A real
    /// NPOT input waits for an encoder (the corpus README says so).
    #[test]
    fn npot_multiples_of_four_decode_exactly() {
        let (w, h) = (1020usize, 516usize);
        let (blocks_x, blocks_y) = (w.div_ceil(4), h.div_ceil(4));
        assert_eq!((blocks_x, blocks_y), (255, 129));
        // 32,895 colours, no two the same: the block index in two bytes.
        let colour = |bx: usize, by: usize| {
            let i = by * blocks_x + bx;
            [i as u8, (i >> 8) as u8, 0x5A, 0xFF]
        };
        let mut rgba = vec![0u8; w * h * 4];
        for by in 0..blocks_y {
            for bx in 0..blocks_x {
                put_block(&mut rgba, w, h, bx, by, &[colour(bx, by); 16]);
            }
        }
        assert_eq!(rgba.len(), 1020 * 516 * 4, "no padding columns or rows");
        let wrong: Vec<(usize, usize)> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| rgba[(y * w + x) * 4..(y * w + x) * 4 + 4] != colour(x / 4, y / 4))
            .take(5)
            .collect();
        assert!(wrong.is_empty(), "pixels in the wrong place, first: {wrong:?}");

        // Inside one block, pixel (x, y) is block[y * 4 + x], and lands at (4 bx + x, 4 by + y).
        let block: [[u8; 4]; 16] = std::array::from_fn(|i| [(i % 4) as u8, (i / 4) as u8, 7, 0xFF]);
        let mut rgba = vec![0u8; 12 * 12 * 4];
        put_block(&mut rgba, 12, 12, 2, 1, &block);
        for y in 0..12 {
            for x in 0..12 {
                let px = &rgba[(y * 12 + x) * 4..(y * 12 + x) * 4 + 4];
                let want = if (8..12).contains(&x) && (4..8).contains(&y) { block[(y - 4) * 4 + (x - 8)] } else { [0; 4] };
                assert_eq!(px, want, "pixel ({x}, {y}) of a 12x12 image with one block at (2, 1)");
            }
        }

        // The alpha plane touches byte 3 only.
        let mut rgba = vec![0x11u8; w * h * 4];
        let alpha = |bx: usize, by: usize| (by * blocks_x + bx) as u8;
        for by in 0..blocks_y {
            for bx in 0..blocks_x {
                put_alpha(&mut rgba, w, h, bx, by, &[alpha(bx, by); 16]);
            }
        }
        let wrong: Vec<(usize, usize)> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| rgba[(y * w + x) * 4..(y * w + x) * 4 + 4] != [0x11, 0x11, 0x11, alpha(x / 4, y / 4)])
            .take(5)
            .collect();
        assert!(wrong.is_empty(), "alpha in the wrong place, first: {wrong:?}");

        // A size that is not a multiple of four: 6 x 5 has 2 x 2 blocks, and
        // the right column and the bottom row are clipped to the image.
        for (w, h) in [(6usize, 5usize), (1, 1), (5, 2), (4, 7)] {
            let (blocks_x, blocks_y) = (w.div_ceil(4), h.div_ceil(4));
            let mut rgba = vec![0u8; w * h * 4];
            for by in 0..blocks_y {
                for bx in 0..blocks_x {
                    put_block(&mut rgba, w, h, bx, by, &[[bx as u8 + 1, by as u8 + 1, 9, 0xFF]; 16]);
                }
            }
            for y in 0..h {
                for x in 0..w {
                    assert_eq!(rgba[(y * w + x) * 4..(y * w + x) * 4 + 4], [x as u8 / 4 + 1, y as u8 / 4 + 1, 9, 0xFF], "pixel ({x}, {y}) of {w}x{h}");
                }
            }
        }
    }

    // ── bits, Huffman tables and the slice (Appendix D.1 to D.5) ────────

    /// Writes a bitstream the way Appendix D reads one: each byte fills from
    /// its least significant bit.
    #[derive(Default)]
    struct BitWriter {
        bytes: Vec<u8>,
        len: usize,
    }

    impl BitWriter {
        fn bits(&mut self, value: u32, n: u32) -> &mut Self {
            for k in 0..n {
                if self.len % 8 == 0 {
                    self.bytes.push(0);
                }
                if (value >> k) & 1 == 1 {
                    *self.bytes.last_mut().unwrap() |= 1 << (self.len % 8);
                }
                self.len += 1;
            }
            self
        }

        /// A Huffman code starts with its most significant bit (RFC 1951,
        /// 3.1.1), so that bit is the first one read.
        fn code(&mut self, code: u32, len: u32) -> &mut Self {
            for k in (0..len).rev() {
                self.bits((code >> k) & 1, 1);
            }
            self
        }
    }

    /// RFC 1951, 3.2.2: the (code, length) of every symbol, length 0 for a
    /// symbol that is not used.
    fn canonical_codes(lengths: &[u8]) -> Vec<(u32, u32)> {
        let mut count = [0u32; 17];
        for &l in lengths {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let (mut next, mut code) = ([0u32; 17], 0u32);
        for bits in 1..=16 {
            code = (code + count[bits - 1]) << 1;
            next[bits] = code;
        }
        lengths
            .iter()
            .map(|&l| {
                if l == 0 {
                    return (0, 0);
                }
                let c = next[l as usize];
                next[l as usize] += 1;
                (c, u32::from(l))
            })
            .collect()
    }

    /// A Huffman table the test writes: its code lengths, and so its codes.
    struct Table {
        lengths: Vec<u8>,
        codes: Vec<(u32, u32)>,
    }

    impl Table {
        /// `used` are the (symbol, code length) pairs of an `alphabet`-symbol table.
        fn new(alphabet: usize, used: &[(usize, u8)]) -> Table {
            let mut lengths = vec![0u8; alphabet];
            for &(symbol, len) in used {
                lengths[symbol] = len;
            }
            let codes = canonical_codes(&lengths);
            Table { lengths, codes }
        }
        /// A table whose one used symbol has the code `0`.
        fn only(alphabet: usize, symbol: usize) -> Table {
            Table::new(alphabet, &[(symbol, 1)])
        }
        fn huffman(&self) -> Huffman {
            Huffman::from_lengths(&self.lengths).expect("a valid table")
        }
        fn put(&self, w: &mut BitWriter, symbol: usize) {
            let (code, len) = self.codes[symbol];
            assert!(len > 0, "symbol {symbol} has no code");
            w.code(code, len);
        }
    }

    /// `codes_order` (Appendix D.1), written out from the specification.
    const CODES_ORDER: [usize; 21] = [17, 18, 19, 20, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15, 16];

    /// A compressed Huffman table (D.1) for `lengths`, each length sent as a
    /// literal through a fixed code-length code: symbols 0 to 14 have 4-bit
    /// codes and 15 and 16 have 5-bit codes, which is a complete code.
    fn put_table(w: &mut BitWriter, lengths: &[u8]) {
        w.bits(lengths.len() as u32, 14); // total_used_syms
        if lengths.is_empty() {
            return;
        }
        w.bits(21, 5); // num_codelength_codes
        let mut sizes = [0u8; 21];
        for (symbol, size) in sizes.iter_mut().enumerate().take(17) {
            *size = if symbol <= 14 { 4 } else { 5 };
        }
        for symbol in CODES_ORDER {
            w.bits(u32::from(sizes[symbol]), 3);
        }
        let codes = canonical_codes(&sizes);
        for &len in lengths {
            let (code, n) = codes[len as usize];
            w.code(code, n);
        }
    }

    /// Overwrite `n` bits of `bytes` from bit `at` (least significant bit first).
    fn set_bits(bytes: &mut [u8], at: usize, n: usize, value: u32) {
        for k in 0..n {
            let (byte, mask) = ((at + k) / 8, 1u8 << ((at + k) % 8));
            if (value >> k) & 1 == 1 {
                bytes[byte] |= mask;
            } else {
                bytes[byte] &= !mask;
            }
        }
    }

    /// The endpoint, selector and table blocks of a BasisLZ file's global
    /// data, from the offsets Appendix C gives (not through `parse`): the
    /// 20-byte header, one 20-byte ImageDesc per level, then the blocks.
    fn etc1s_blocks(file: &[u8]) -> [std::ops::Range<usize>; 3] {
        let sgd = get64(file, 64);
        let start = sgd + 20 + 20 * get32(file, 40).max(1);
        let (endpoints, selectors, tables) = (get32(file, sgd + 4), get32(file, sgd + 8), get32(file, sgd + 12));
        [start..start + endpoints, start + endpoints..start + endpoints + selectors, start + endpoints + selectors..start + endpoints + selectors + tables]
    }

    #[test]
    fn bits_are_read_least_significant_bit_first() {
        let mut b = Bits::new(&[0b1011_0001, 0b0000_0011, 0xFF], "test");
        assert_eq!(b.bits(3).unwrap(), 0b001);
        assert_eq!(b.bits(5).unwrap(), 0b10110);
        assert_eq!(b.bits(0).unwrap(), 0);
        assert_eq!(b.bits(10).unwrap(), 0x303, "the second byte, then two bits of the third");
        assert_eq!(Bits::new(&[0x78, 0x56, 0x34, 0x12, 0xFF], "test").bits(32).unwrap(), 0x1234_5678);
        // Reading exactly the data is fine, one bit more is not: no zero padding.
        let mut b = Bits::new(&[0xFF, 0x01], "the test stream");
        assert_eq!(b.bits(9).unwrap(), 0x1FF);
        assert_eq!(b.bits(7).unwrap(), 0);
        let why = malformed(b.bits(1));
        assert!(why.contains("the test stream"), "{why}");
    }

    #[test]
    fn variable_length_codes_chain_chunks_until_one_lacks_a_continue_bit() {
        // decode_vlc4: 5-bit chunks, bit 4 means another follows. 0x123 is 3, 2, 1.
        let mut w = BitWriter::default();
        w.bits(0x10 | 3, 5).bits(0x10 | 2, 5).bits(1, 5);
        assert_eq!(Bits::new(&w.bytes, "t").vlc(4).unwrap(), 0x123);
        // decode_vlc7: 8-bit chunks, bit 7 means another follows. 300 is 0x2C, 2.
        let mut w = BitWriter::default();
        w.bits(0x80 | 0x2C, 8).bits(2, 8);
        assert_eq!(Bits::new(&w.bytes, "t").vlc(7).unwrap(), 300);
        // Five 7-bit chunks are the most that fit 32 bits (offsets 0, 7, 14, 21, 28).
        let mut w = BitWriter::default();
        for payload in [1u32, 0, 0, 0] {
            w.bits(0x80 | payload, 8);
        }
        w.bits(1, 8);
        assert_eq!(Bits::new(&w.bytes, "t").vlc(7).unwrap(), 0x1000_0001);
        let mut w = BitWriter::default();
        for _ in 0..5 {
            w.bits(0x80, 8);
        }
        w.bits(0, 8);
        assert!(malformed(Bits::new(&w.bytes, "t").vlc(7)).contains("variable-length"));
        // Eight 4-bit chunks are the most; the stream that still asks for a ninth is invalid.
        let mut w = BitWriter::default();
        for _ in 0..9 {
            w.bits(0x10, 5);
        }
        assert!(malformed(Bits::new(&w.bytes, "t").vlc(4)).contains("variable-length"));
        // And a stream that ends mid-code.
        assert!(malformed(Bits::new(&[0x80], "t").vlc(7)).contains('t'));
    }

    /// A table is complete (every code is used) or holds one symbol, as the
    /// reference transcoder demands. Anything else cannot be decoded.
    #[test]
    fn huffman_tables_must_be_complete_or_hold_one_symbol() {
        let ok: [&[u8]; 7] = [&[1, 1], &[2, 1, 3, 3], &[1, 2, 3, 3, 0], &[1, 0, 1], &[0, 0, 5, 0], &[1], &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 16]];
        for lengths in ok {
            assert!(Huffman::from_lengths(lengths).is_ok(), "{lengths:?} is a table");
        }
        let bad: [(&str, &[u8]); 7] = [
            ("oversubscribed", &[1, 1, 1]),
            ("incomplete", &[1, 2]),
            ("incomplete, three symbols", &[2, 2, 2]),
            ("no symbol used", &[0, 0, 0]),
            ("a length past 16", &[17]),
            ("oversubscribed past 16", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 16, 16]),
            ("a 16-bit code short of complete", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
        ];
        for (what, lengths) in bad {
            let why = malformed(Huffman::from_lengths(lengths));
            assert!(why.contains("Huffman"), "{what}: {why}");
        }
        // Six one-bit codes wrap a 32-bit Kraft sum to exactly 2^31: still oversubscribed.
        assert!(malformed(Huffman::from_lengths(&[1; 6])).contains("Huffman"));
    }

    /// RFC 1951 3.2.2's example: lengths (3,3,3,3,3,2,4,4) for A to H give
    /// F=00, A=010, B=011, C=100, D=101, E=110, G=1110, H=1111, and each code
    /// is read starting with its first (most significant) bit.
    #[test]
    fn huffman_codes_are_canonical_and_read_most_significant_bit_first() {
        let lengths = [3u8, 3, 3, 3, 3, 2, 4, 4];
        let table = Huffman::from_lengths(&lengths).unwrap();
        let codes = canonical_codes(&lengths);
        assert_eq!(codes[5], (0b00, 2), "F");
        assert_eq!(codes[6], (0b1110, 4), "G");
        let symbols = [5usize, 0, 7, 2, 6, 1, 4, 3, 6];
        let mut w = BitWriter::default();
        for &s in &symbols {
            w.code(codes[s].0, codes[s].1);
        }
        let mut bits = Bits::new(&w.bytes, "test");
        let got: Vec<u32> = symbols.iter().map(|_| table.decode(&mut bits).unwrap()).collect();
        assert_eq!(got, symbols.map(|s| s as u32));

        // One byte, 0b0000_1111: H (1111), then two F (00), and then nothing: a
        // code read past the end is an error, not a symbol from zero padding.
        let mut bits = Bits::new(&[0b0000_1111], "the stream");
        assert_eq!(table.decode(&mut bits).unwrap(), 7);
        assert_eq!(table.decode(&mut bits).unwrap(), 5);
        assert_eq!(table.decode(&mut bits).unwrap(), 5);
        assert!(malformed(table.decode(&mut bits)).contains("the stream"));

        // A one-symbol table: its code is a single 0 bit; a 1 is no code at all.
        let one = Huffman::from_lengths(&[0, 0, 1]).unwrap();
        let mut bits = Bits::new(&[0b0000_0100], "test");
        assert_eq!(one.decode(&mut bits).unwrap(), 2);
        assert_eq!(one.decode(&mut bits).unwrap(), 2);
        assert!(malformed(one.decode(&mut bits)).contains("Huffman"));
    }

    /// A table as D.1 sends it, with runs: lengths [2,2,0,0,0,3,3,3,3] sent as
    /// the literals 2, 2, symbol 17 (a run of 3 zeros), the literal 3, and
    /// symbol 19 (repeat it 3 times), through a code-length code of the four
    /// symbols 2, 3, 17 and 19 (each 2 bits: 00, 01, 10, 11).
    #[test]
    fn a_compressed_huffman_table_reads_and_decodes() {
        let mut w = BitWriter::default();
        w.bits(9, 14); // total_used_syms
        w.bits(17, 5); // num_codelength_codes: through position 16 (symbol 2)
        for position in 0..17 {
            // codes_order[0] = 17, [2] = 19, [14] = 3, [16] = 2
            w.bits(if matches!(position, 0 | 2 | 14 | 16) { 2 } else { 0 }, 3);
        }
        w.code(0b00, 2).code(0b00, 2); // 2, 2
        w.code(0b10, 2).bits(0, 3); // symbol 17, extra 0: a run of 3 zeros
        w.code(0b01, 2); // 3
        w.code(0b11, 2).bits(0, 2); // symbol 19, extra 0: three more
        // The payload: symbols 5, 0, 8, 1, 6, 7 under the canonical codes of
        // [2,2,0,0,0,3,3,3,3]: 0=00, 1=01, 5=100, 6=101, 7=110, 8=111.
        for (code, len) in [(0b100, 3), (0b00, 2), (0b111, 3), (0b01, 2), (0b101, 3), (0b110, 3)] {
            w.code(code, len);
        }
        let mut bits = Bits::new(&w.bytes, "test");
        let table = Huffman::read(&mut bits).unwrap();
        assert!(!table.is_empty());
        let got: Vec<u32> = (0..6).map(|_| table.decode(&mut bits).unwrap()).collect();
        assert_eq!(got, [5, 0, 8, 1, 6, 7]);

        // Zero symbols is a table that is not there.
        let mut w = BitWriter::default();
        w.bits(0, 14);
        assert!(Huffman::read(&mut Bits::new(&w.bytes, "test")).unwrap().is_empty());
    }

    /// A damaged Huffman table in the global data is `Malformed`, wherever it
    /// is: the endpoint codebook's four tables, the selector codebook's delta
    /// table and the slice tables. Each corruption is made at a bit offset the
    /// test works out from Appendix C and D.1: `total_used_syms` is 14 bits,
    /// then `num_codelength_codes` 5 bits, then 3 bits per code length code.
    #[test]
    fn a_malformed_huffman_table_is_malformed() {
        let dot = sample("Dot_C").ktx2;
        decoded(dot); // the control: the file as published decodes
        let [endpoints, selectors, tables] = etc1s_blocks(dot);
        // The selector codebook's table follows its three flag bits (D.3):
        // two zeros and "isUncompressed", which must be 0 here for a table to follow.
        assert_eq!(dot[selectors.start] & 0b111, 0, "Dot_C's selectors are Huffman coded, not raw");

        // (what, the corrupted file, the phrase its error must carry)
        let mut cases: Vec<(String, Vec<u8>, &str)> = Vec::new();
        for (name, block, table_at) in [("endpoints", endpoints, 0usize), ("selectors", selectors, 3), ("tables", tables.clone(), 0)] {
            let corrupt = |what: &str, phrase: &'static str, edit: &dyn Fn(&mut [u8])| {
                let mut file = dot.to_vec();
                edit(&mut file[block.clone()]);
                (format!("{name}: {what}"), file, phrase)
            };
            for n in [0u32, 22, 31] {
                cases.push(corrupt(&format!("num_codelength_codes {n}"), "code length codes, outside 1 to 21", &|b| set_bits(b, table_at + 14, 5, n)));
            }
            // A table with no symbol is "not there", which a table the stream needs may not be.
            cases.push(corrupt("total_used_syms 0", "is an empty Huffman table", &|b| set_bits(b, table_at, 14, 0)));
            // 21 code length codes whose first three are sized 1: three 1-bit codes.
            cases.push(corrupt("an oversubscribed code length code", "code lengths are oversubscribed", &|b| {
                set_bits(b, table_at + 14, 5, 21);
                for k in 0..3 {
                    set_bits(b, table_at + 19 + 3 * k, 3, 1);
                }
            }));
        }
        // Tables the test writes over the start of the slice tables block. A
        // table of three symbols that use no code is incomplete: 0 of 65536 code units.
        for (what, lengths, phrase) in [
            ("oversubscribed", vec![1u8, 1, 1], "code lengths are oversubscribed"),
            ("incomplete", vec![1, 2], "code lengths are incomplete"),
            ("without a used symbol", vec![0, 0, 0], "use 0 of 65536 code units"),
        ] {
            let mut w = BitWriter::default();
            put_table(&mut w, &lengths);
            let mut file = dot.to_vec();
            file[tables.start..tables.start + w.bytes.len()].copy_from_slice(&w.bytes);
            cases.push((format!("tables: {what} code lengths"), file, phrase));
        }
        for (what, file, phrase) in cases {
            let why = malformed(decode_rgba8(&file));
            assert!(why.contains("Huffman") && why.contains(phrase), "{what}: expected {phrase:?}, got {why}");
        }
    }

    /// Slice decoding as Appendix D.5.3 gives it, on a 4 x 2 grid of blocks
    /// with a stream the test writes. The endpoint predictor symbols are one
    /// per 2x2 blocks (two bits per block: 0 left, 1 upper, 2 upper-left, 3
    /// a delta), the selectors go through the history buffer (size 4, with
    /// the move-to-front `use`), and a run of three covers the middle of the
    /// second row.
    #[test]
    fn endpoint_and_selector_indices_follow_appendix_d() {
        // Group (0,0): (0,0) delta, (1,0) left, below: (0,1) upper, (1,1) upper-left.
        let group0 = 3 | (1 << 4) | (2 << 6);
        // Group (2,0): (2,0) delta, (3,0) left, below: (2,1) upper, (3,1) left.
        let group1 = 3 | (1 << 4);
        assert_eq!((group0, group1), (147, 19));
        let pred = Table::new(257, &[(group0, 1), (group1, 1)]);
        let delta = Table::new(5, &[(2, 1), (4, 1)]);
        // 3 selectors and a history of 4: symbols 3 to 6 are history slots, 7 starts a run.
        let selector = Table::new(8, &[(0, 2), (1, 3), (2, 3), (3, 2), (5, 3), (7, 3)]);
        let rle = Table::only(64, 0);
        let models = Models {
            endpoint_pred: pred.huffman(),
            endpoint_delta: delta.huffman(),
            selector: selector.huffman(),
            selector_rle: rle.huffman(),
            history: 4,
        };
        let mut w = BitWriter::default();
        // Row 0.
        pred.put(&mut w, group0);
        delta.put(&mut w, 2); // endpoint 0 + 2 = 2
        selector.put(&mut w, 2); // codebook entry 2; history becomes [0,0,2,0]
        selector.put(&mut w, 0); // entry 0 (history wraps its rover: [0,0,2,0])
        pred.put(&mut w, group1);
        delta.put(&mut w, 4); // 2 + 4 = 6, past the 5 endpoints: 6 - 5 = 1
        selector.put(&mut w, 3); // history slot 0 (entry 0)
        selector.put(&mut w, 5); // history slot 2 (entry 2); slot 2 swaps with slot 1
        // Row 1: no predictor symbols. A run of 3 (symbol 7, run symbol 0 + 3), then entry 1.
        selector.put(&mut w, 7);
        rle.put(&mut w, 0);
        selector.put(&mut w, 1);
        let mut got = Vec::new();
        let models = &models;
        decode_slice(&w.bytes, (4, 2), models, 5, 3, |x, y, e, s| got.push((x, y, e, s))).expect("a valid stream");
        let want = [(0, 0, 2, 2), (1, 0, 2, 0), (2, 0, 1, 0), (3, 0, 1, 2), (0, 1, 2, 0), (1, 1, 2, 0), (2, 1, 1, 0), (3, 1, 1, 1)];
        assert_eq!(got, want);

        // Every shorter stream runs out of bits somewhere.
        for len in 0..w.bytes.len() {
            let why = malformed(decode_slice(&w.bytes[..len], (4, 2), models, 5, 3, |_, _, _, _| {}));
            assert!(!why.is_empty(), "cut to {len} bytes");
        }
    }

    /// An endpoint predictor symbol 256 repeats the previous one for the
    /// groups that follow: `vlc4 + 3` groups in all, this one included.
    #[test]
    fn a_repeated_endpoint_prediction_covers_the_groups_after_it() {
        // (0,0) delta, (1,0) left, below: (0,1) upper, (1,1) upper.
        let group = 3 | (1 << 4) | (1 << 6);
        let pred = Table::new(257, &[(group, 1), (256, 1)]);
        let delta = Table::only(2, 1);
        let selector = Table::only(3, 0); // one selector, history of 1: the only symbol is entry 0
        let rle = Table::only(64, 0);
        let models = Models {
            endpoint_pred: pred.huffman(),
            endpoint_delta: delta.huffman(),
            selector: selector.huffman(),
            selector_rle: rle.huffman(),
            history: 1,
        };
        let stream = |repeat: u32| {
            let mut w = BitWriter::default();
            for y in 0..4 {
                for x in 0..4 {
                    // The first block of each 2x2 group on an even row predicts by delta.
                    if x % 2 == 0 && y % 2 == 0 {
                        match (x, y) {
                            (0, 0) => pred.put(&mut w, group),
                            (2, 0) => {
                                pred.put(&mut w, 256);
                                w.bits(repeat, 5); // vlc4: a single chunk
                            }
                            _ => {} // inside the run: no symbol
                        }
                        delta.put(&mut w, 1);
                    }
                    selector.put(&mut w, 0);
                }
            }
            w
        };
        let mut got = Vec::new();
        decode_slice(&stream(0).bytes, (4, 4), &models, 8, 1, |x, y, e, s| got.push((x, y, e, s))).expect("three groups after the first symbol");
        let rows: Vec<Vec<usize>> = (0..4).map(|y| got.iter().filter(|b| b.1 == y).map(|b| b.2).collect()).collect();
        assert_eq!(rows, [[1, 1, 2, 2], [1, 1, 2, 2], [3, 3, 4, 4], [3, 3, 4, 4]]);
        assert!(got.iter().all(|b| b.3 == 0));
        // One more repeat than there are groups left: the run never ends.
        let why = malformed(decode_slice(&stream(1).bytes, (4, 4), &models, 8, 1, |_, _, _, _| {}));
        assert!(why.contains("repeat"), "{why}");
    }

    /// Every way a slice stream can contradict itself is `Malformed`, and
    /// names what: a prediction that reaches outside the image, an index past
    /// its codebook, a selector run longer than the whole slice. (A run that
    /// is no longer than the slice but outlasts the blocks left is not one of
    /// them: it is cut off at the end of the slice, as the reference does.
    /// `a_selector_run_that_outlasts_the_blocks_left_is_cut_off_at_the_end`.)
    #[test]
    fn slice_streams_that_break_the_rules_are_malformed() {
        // 3 endpoints, 2 selectors and a history of 2: selector symbols 0 and 1
        // are entries, 2 and 3 history slots, 4 starts a run, 5 is past the history.
        let selector = Table::new(6, &[(0, 1), (1, 2), (4, 3), (5, 3)]);
        let case = |group: usize, delta: usize, run: usize, script: &dyn Fn(&mut BitWriter, &Table, &Table, &Table)| {
            let pred = Table::only(257, group);
            let delta = Table::only(delta + 1, delta);
            let rle = Table::only(64, run);
            let models = Models {
                endpoint_pred: pred.huffman(),
                endpoint_delta: delta.huffman(),
                selector: selector.huffman(),
                selector_rle: rle.huffman(),
                history: 2,
            };
            let mut w = BitWriter::default();
            script(&mut w, &pred, &delta, &rle);
            malformed(decode_slice(&w.bytes, (2, 2), &models, 3, 2, |_, _, _, _| {}))
        };
        let blocks = |w: &mut BitWriter, n: usize| {
            for _ in 0..n {
                selector.put(w, 0);
            }
        };
        // The first block (0,0) can only use a delta: left, upper and upper-left reach outside the image.
        for (what, group, mention) in [("left", 0usize, "left"), ("upper", 1, "upper"), ("upper-left", 2, "upper-left")] {
            let why = case(group, 0, 0, &|w, pred, _, _| {
                pred.put(w, group);
                blocks(w, 4);
            });
            assert!(why.contains(mention), "{what} at (0,0): {why}");
        }
        // Upper-left on the second row's first column: (0,0) delta, (1,0) left, (0,1) upper-left.
        let group = 3 | (2 << 4);
        let why = case(group, 0, 0, &|w, pred, delta, _| {
            pred.put(w, group);
            delta.put(w, 0);
            blocks(w, 4);
        });
        assert!(why.contains("upper-left"), "{why}");
        // A delta of 7 from endpoint 0, past the 3 endpoints even after one wrap: 7 - 3 = 4.
        let why = case(3, 7, 0, &|w, pred, delta, _| {
            pred.put(w, 3);
            delta.put(w, 7);
            blocks(w, 4);
        });
        assert!(why.contains("endpoint"), "{why}");
        // Selector symbol 5 is history slot 3, but the history holds 2.
        let why = case(3, 0, 0, &|w, pred, delta, _| {
            pred.put(w, 3);
            delta.put(w, 0);
            selector.put(w, 5);
        });
        assert!(why.contains("selector history"), "{why}");
        // A run of 5 + 3 = 8 selectors in a slice of 4 blocks: longer than the whole slice.
        let why = case(3, 0, 5, &|w, pred, delta, rle| {
            pred.put(w, 3);
            delta.put(w, 0);
            selector.put(w, 4);
            rle.put(w, 5);
        });
        assert!(why.contains("selector run of 8 blocks is longer than the slice's 4"), "{why}");
    }

    /// The reference refuses a selector run only when it is longer than the
    /// whole slice (`cur_selector_rle_count > total_blocks`). One that starts
    /// late and outlasts the blocks left is cut off at the end of the slice,
    /// and the slice decodes. Here a run of 4 starts at the third of 4 blocks,
    /// so two of its blocks are past the end.
    #[test]
    fn a_selector_run_that_outlasts_the_blocks_left_is_cut_off_at_the_end() {
        // Block (0,0) a delta, (1,0) left, (0,1) upper, (1,1) left.
        let group = 3 | (1 << 4);
        let pred = Table::only(257, group);
        let delta = Table::only(1, 0);
        let selector = Table::new(6, &[(0, 1), (1, 2), (4, 3), (5, 3)]);
        let run_of = |run_symbol: usize| {
            let rle = Table::only(64, run_symbol);
            let models = Models {
                endpoint_pred: pred.huffman(),
                endpoint_delta: delta.huffman(),
                selector: selector.huffman(),
                selector_rle: rle.huffman(),
                history: 2,
            };
            let mut w = BitWriter::default();
            pred.put(&mut w, group);
            delta.put(&mut w, 0);
            selector.put(&mut w, 0); // block (0,0): entry 0
            selector.put(&mut w, 0); // block (1,0): entry 0
            selector.put(&mut w, 4); // block (0,1): a run of run_symbol + 3, of which 2 blocks are left
            rle.put(&mut w, run_symbol);
            let mut got = Vec::new();
            let result = decode_slice(&w.bytes, (2, 2), &models, 3, 2, |x, y, e, s| got.push((x, y, e, s)));
            (result, got)
        };
        let (result, got) = run_of(1); // 1 + 3 = 4 blocks: all of the slice, 2 of them past the end
        result.expect("a run no longer than the slice decodes");
        assert_eq!(got, [(0, 0, 0, 0), (1, 0, 0, 0), (0, 1, 0, 0), (1, 1, 0, 0)]);
        let (result, _) = run_of(2); // 2 + 3 = 5 blocks, longer than the slice's 4
        let why = malformed(result);
        assert!(why.contains("selector run of 5 blocks is longer than the slice's 4"), "{why}");
    }

    // ── hostile input to the ETC1S decoder, on the corpus's own tables ──

    /// Where `inner` lies in `outer`, for a slice that `parse` took from it.
    fn offset_in(outer: &[u8], inner: &[u8]) -> usize {
        let at = (inner.as_ptr() as usize).checked_sub(outer.as_ptr() as usize).expect("a slice of the file");
        assert!(at + inner.len() <= outer.len(), "not a slice of the file");
        at
    }

    /// The top `block_rows` rows of blocks of an ETC1S corpus sample as a
    /// KTX2 file of its own: the sample's real global data (its codebooks and
    /// Huffman tables, whole) and the shortest prefix of each of its slices
    /// that still decodes those rows, which are then the reference's top
    /// rows, byte for byte. That is a real picture of 1/64 the blocks, so a
    /// sweep can decode it tens of thousands of times. The width is the
    /// sample's, so the slice streams run exactly as they do in the whole
    /// file.
    fn etc1s_strip(stem: &str, block_rows: u32) -> Vec<u8> {
        let s = sample(stem);
        let whole = parse(s.ktx2).unwrap();
        let global = whole.basislz.as_ref().expect("an ETC1S sample has global data");
        let (width, height) = (s.size.0, (4 * block_rows).min(s.size.1));
        let want = &s.rgba()[..width as usize * height as usize * 4];
        let rgb_all = &whole.level0[global.rgb.clone()];
        let alpha_all = global.alpha.clone().map(|a| &whole.level0[a]);
        let build = |rgb: usize, alpha: usize| {
            let mut level0 = rgb_all[..rgb].to_vec();
            let desc = match alpha_all {
                Some(a) => {
                    level0.extend(&a[..alpha]);
                    Desc { rgb: (0, rgb as u32), alpha: (rgb as u32, alpha as u32) }
                }
                None => Desc { rgb: (0, rgb as u32), alpha: (0, 0) },
            };
            let sgd = basislz_sgd((global.endpoint_count, global.selector_count), [global.endpoints, global.selectors, global.tables, global.extended], &[desc]);
            Ktx2::etc1s(width, height, vec![level0], sgd).build()
        };
        let good = |file: &[u8]| matches!(decode_rgba8(file), Ok(img) if img.pixels == want);
        // `good` holds from some length on and not before: a shorter stream
        // runs out, a longer one only has bytes it never reads.
        let shortest = |len: usize, with: &dyn Fn(usize) -> Vec<u8>| (0..=len).collect::<Vec<usize>>().partition_point(|&n| !good(&with(n)));
        let alpha_len = alpha_all.map_or(0, |a| a.len());
        let rgb = shortest(rgb_all.len(), &|n| build(n, alpha_len));
        let alpha = shortest(alpha_len, &|n| build(rgb, n));
        let file = build(rgb, alpha);
        assert!(good(&file), "{stem}: no prefix of its slices decodes its top {block_rows} rows of blocks to the reference");
        file
    }

    /// A hostile-input sweep with a budget, for a file whose every reading
    /// costs about a millisecond (a decode has to read the whole codebook),
    /// where `layer_sweep`'s every bit of the first 4 KiB and 10,000 seeded
    /// cases come to a minute and a half. Every cut of `file`, then single
    /// bit flips of each span of `spans` (every bit of it, or if it holds
    /// more than `per_span` bits, every `n`th, the same ones every run), then
    /// `seeded` cases of 2 to 8 flipped bits drawn like `layer_sweep`'s, with
    /// the same xorshift64 and seed. `read` must return every time; the
    /// readings, the slowest and the wall time are printed.
    fn bounded_sweep(name: &str, file: &[u8], spans: &[(&str, std::ops::Range<usize>)], per_span: usize, seeded: usize, read: impl Fn(&[u8])) {
        const FLIP_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
        let started = std::time::Instant::now();
        let (mut readings, mut slowest) = (0usize, (std::time::Duration::ZERO, String::new()));
        let mut survives = |bytes: &[u8], case: &dyn Fn() -> String| {
            let at = std::time::Instant::now();
            let returned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read(bytes))).is_ok();
            let took = at.elapsed();
            assert!(returned, "{name}: {}", case());
            readings += 1;
            if took > slowest.0 {
                slowest = (took, case());
            }
        };
        for cut in 0..file.len() {
            survives(&file[..cut], &|| format!("cut to {cut} bytes"));
        }
        let mut flipped = file.to_vec();
        let mut singles = Vec::new();
        for (label, span) in spans {
            let bits = span.len() * 8;
            let stride = bits.div_ceil(per_span).max(1);
            let mut taken = 0;
            for bit in (0..bits).step_by(stride) {
                let at = span.start * 8 + bit;
                flipped[at / 8] ^= 1 << (at % 8);
                survives(&flipped, &|| format!("{label}: bit {at} flipped"));
                flipped[at / 8] ^= 1 << (at % 8);
                taken += 1;
            }
            singles.push(format!("{taken} of {bits} bits of the {label} (one in {stride})"));
        }
        let mut state = FLIP_SEED;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let bits = file.len() as u64 * 8;
        for case in 0..seeded {
            let flips = 2 + next() % 7;
            let mut chosen: Vec<u64> = Vec::new();
            while (chosen.len() as u64) < flips {
                let bit = next() % bits;
                if !chosen.contains(&bit) {
                    chosen.push(bit);
                }
            }
            let mut flipped = file.to_vec();
            for &bit in &chosen {
                flipped[(bit / 8) as usize] ^= 1 << (bit % 8);
            }
            survives(&flipped, &|| format!("case {case}: bits {chosen:?} flipped"));
        }
        eprintln!(
            "{name}: {} bytes; {readings} readings: {} cuts, flips of {}, {seeded} seeded cases (seed {FLIP_SEED:#x}); slowest {:?} ({}); all {:?}",
            file.len(),
            file.len(),
            singles.join(", "),
            slowest.0,
            slowest.1,
            started.elapsed()
        );
    }

    /// The decoder's hostile-input sweep on a corpus sample, run on the
    /// sample's strip (the top 2 rows of blocks, with its real codebooks and
    /// tables), then eight single-bit flips in each slice of the whole
    /// picture at evenly spaced places, to read the whole-size path too.
    /// Every reading gives pixels of the right size, `Malformed` or
    /// `Unsupported`, and the sweep must reach a decode and a `Malformed`.
    ///
    /// `budget` `None` runs the item's whole bar on the strip: `layer_sweep`,
    /// with every cut, every bit of the first 4 KiB and of the Huffman tables
    /// flipped, and 10,000 seeded cases. That is for a strip whose codebooks
    /// are small (Dot_C's: a reading costs a tenth of a millisecond), and for
    /// the release runs of the two big files' ignored tests. `Some(n)` is for
    /// a big codebook in a debug build, where a reading costs a millisecond or
    /// more (the 1024x1024 files: 1.5 and 6 ms): [`bounded_sweep`], every cut
    /// and `n` flips of each part of the file (its header, the SGD header and
    /// descriptor; the endpoint and selector codebooks; the Huffman tables;
    /// the slice streams) and `n` seeded cases.
    fn etc1s_strip_sweep(stem: &str, budget: Option<usize>) {
        let started = std::time::Instant::now();
        let strip = etc1s_strip(stem, 2);
        let container = parse(&strip).unwrap();
        let global = container.basislz.as_ref().unwrap();
        let span = |from: &[u8], to: &[u8]| offset_in(&strip, from)..offset_in(&strip, to) + to.len();
        let spans = [
            ("header, index, descriptors and SGD header", 0..offset_in(&strip, global.endpoints)),
            ("endpoint and selector codebooks", span(global.endpoints, global.selectors)),
            ("Huffman tables", span(global.tables, global.tables)),
            ("slice streams", span(container.level0, container.level0)),
        ];
        let name = format!("{stem} (top 2 block rows)");
        let counts = Outcomes::default();
        match budget {
            None => layer_sweep(&name, &strip, span(global.tables, global.tables), |bytes| counts.read(bytes, true)),
            Some(n) => bounded_sweep(&name, &strip, &spans, n, n, |bytes| counts.read(bytes, true)),
        }
        let (probed, decoded) = counts.report(&name, true);
        assert!(probed[0] > 0 && decoded[0] > 0 && decoded[1] > 0, "{stem}: probe gave {probed:?} and decode {decoded:?}");

        let s = sample(stem);
        let whole = parse(s.ktx2).unwrap();
        let global = whole.basislz.as_ref().unwrap();
        let level0 = offset_in(s.ktx2, whole.level0);
        let slices = std::iter::once(&global.rgb).chain(global.alpha.iter());
        let (mut slowest, mut shown) = (std::time::Duration::ZERO, 0);
        let (mut ok, mut bad) = (0, 0);
        for slice in slices {
            for k in 0..8 {
                let mut file = s.ktx2.to_vec();
                let byte = level0 + slice.start + slice.len() * (2 * k + 1) / 16;
                file[byte] ^= 1 << (k % 8);
                let at = std::time::Instant::now();
                match decode_rgba8(&file) {
                    Ok(img) => {
                        assert_eq!(img.pixels.len(), s.rgba().len());
                        ok += 1;
                    }
                    Err(MeshError::Malformed(_)) => bad += 1,
                    Err(other) => panic!("{stem}: bit {k} of byte {byte} flipped gave {other:?}"),
                }
                slowest = slowest.max(at.elapsed());
                shown += 1;
            }
        }
        eprintln!("{stem}: {shown} whole-size slice flips gave {ok} Ok, {bad} Malformed; slowest {slowest:?}; the strip sweep and these took {:?}", started.elapsed());
    }

    #[test]
    fn the_flighthelmet_lenses_basecolor_decoder_survives_hostile_slices_and_tables() {
        etc1s_strip_sweep("FlightHelmet_Materials_LensesMat_BaseColor", Some(400));
    }

    #[test]
    fn the_carconcept_dot_colour_decoder_survives_hostile_slices_and_tables() {
        etc1s_strip_sweep("Dot_C", None);
    }

    #[test]
    fn the_stainedglass_grill_alpha_decoder_survives_hostile_slices_and_tables() {
        etc1s_strip_sweep("StainedGlassLamp_grill_basecolor-alpha", Some(100));
    }

    /// The overview's whole bar on the strips of the two 1024x1024 files, which
    /// the budgeted sweeps above do not reach: `layer_sweep` (every cut, every
    /// bit of the first 4 KiB and of the Huffman tables, 10,000 seeded cases).
    /// A reading costs milliseconds in a debug build, so these run in the
    /// release profile, behind `--ignored`:
    /// `cargo test --release --lib -- --ignored the_flighthelmet_lenses_basecolor_decoder_survives_the_whole_sweep_bar the_stainedglass_grill_alpha_decoder_survives_the_whole_sweep_bar`.
    #[test]
    #[ignore = "the whole sweep bar: run in release with --ignored"]
    fn the_flighthelmet_lenses_basecolor_decoder_survives_the_whole_sweep_bar() {
        etc1s_strip_sweep("FlightHelmet_Materials_LensesMat_BaseColor", None);
    }

    #[test]
    #[ignore = "the whole sweep bar: run in release with --ignored"]
    fn the_stainedglass_grill_alpha_decoder_survives_the_whole_sweep_bar() {
        etc1s_strip_sweep("StainedGlassLamp_grill_basecolor-alpha", None);
    }

    // ── hostile input to the UASTC and Zstandard paths ───────────────────

    /// The top `block_rows` rows of blocks of a UASTC corpus sample as a KTX2
    /// file of its own, `scheme` 0 (the blocks as they are) or 2 (the blocks in
    /// one hand-built raw-block frame): the sample's real blocks, inflated if
    /// need be, and the sample's width, so they are the reference's top rows
    /// byte for byte. That is a real picture of 1/256 the blocks (1/128 for
    /// two rows), so a sweep can decode it tens of thousands of times.
    fn uastc_strip(stem: &str, block_rows: u32, scheme: u32) -> Vec<u8> {
        let s = sample(stem);
        let whole = parse(s.ktx2).unwrap();
        let blocks = match whole.info.supercompression {
            Supercompression::Zstd => inflate(whole.level0, uastc::level_len(s.size.0, s.size.1), "UASTC image").unwrap(),
            _ => whole.level0.to_vec(),
        };
        let (width, height) = (s.size.0, (4 * block_rows).min(s.size.1));
        let strip = blocks[..uastc::level_len(width, height) as usize].to_vec();
        let level0 = if scheme == 2 { zstd_frame(None, &[Zb::Raw(strip)]) } else { strip };
        let file = uastc_file(scheme, width, height, level0);
        let want = &s.rgba()[..width as usize * height as usize * 4];
        let got = decoded(&file);
        assert_eq!(got.pixels, want, "{stem}: its top {block_rows} rows of blocks are not the reference's top rows");
        file
    }

    /// The UASTC decoder's hostile-input sweep on a corpus sample's strip
    /// (`scheme` 0, or 2 for the Zstandard glue around the same blocks), with
    /// the real blocks an encoder wrote. `budget` `None` is the item's whole
    /// bar (`layer_sweep`: every cut, every bit of the first 4 KiB and of the
    /// blocks, 10,000 seeded cases); `Some(n)` is [`bounded_sweep`], every cut
    /// and `n` flips of each part of the file and `n` seeded cases, for the
    /// debug build. Every reading gives pixels of the right size, `Malformed`
    /// or `Unsupported`, and the sweep must reach a decode and a `Malformed`.
    fn uastc_strip_sweep(stem: &str, scheme: u32, budget: Option<usize>) {
        let strip = uastc_strip(stem, 1, scheme);
        let container = parse(&strip).unwrap();
        let level0 = offset_in(&strip, container.level0)..offset_in(&strip, container.level0) + container.level0.len();
        let name = format!("{stem} (top block row, scheme {scheme})");
        let counts = Outcomes::default();
        match budget {
            None => layer_sweep(&name, &strip, level0, |bytes| counts.read(bytes, true)),
            Some(n) => {
                let spans = [("header, index and DFD", 0..level0.start), (if scheme == 2 { "Zstandard frame" } else { "blocks" }, level0)];
                bounded_sweep(&name, &strip, &spans, n, n, |bytes| counts.read(bytes, true));
            }
        }
        let (probed, decoded) = counts.report(&name, true);
        assert!(probed[0] > 0 && decoded[0] > 0 && decoded[1] > 0, "{stem}: probe gave {probed:?} and decode {decoded:?}");
    }

    #[test]
    fn the_flighthelmet_lenses_normal_decoder_survives_hostile_blocks() {
        uastc_strip_sweep("FlightHelmet_Materials_LensesMat_Normal", 0, Some(2_000));
    }

    #[test]
    fn the_flighthelmet_lenses_normal_zstd_glue_survives_hostile_frames() {
        uastc_strip_sweep("FlightHelmet_Materials_LensesMat_Normal", 2, Some(2_000));
    }

    /// The overview's whole bar on the Normal strips, which the budgeted
    /// sweeps above do not reach: `cargo test --release --lib -- --ignored
    /// the_flighthelmet_lenses_normal_decoder_survives_the_whole_sweep_bar`.
    #[test]
    #[ignore = "the whole sweep bar: run in release with --ignored"]
    fn the_flighthelmet_lenses_normal_decoder_survives_the_whole_sweep_bar() {
        uastc_strip_sweep("FlightHelmet_Materials_LensesMat_Normal", 0, None);
        uastc_strip_sweep("FlightHelmet_Materials_LensesMat_Normal", 2, None);
    }

    /// The UASTC decoder's hostile-input sweep on the all-modes texture, whose
    /// 240 blocks cover every mode: `budget` `None` is the item's whole bar
    /// (`layer_sweep`: every cut, every bit of the first 4 KiB, which is the
    /// whole file, and 10,000 seeded cases; 40 s in a debug build);
    /// `Some(n)` is [`bounded_sweep`], every cut and `n` flips of each part of
    /// the file and `n` seeded cases.
    fn all_modes_sweep(budget: Option<usize>) {
        let s = sample("uastc_all_modes");
        let container = parse(s.ktx2).unwrap();
        let level0 = offset_in(s.ktx2, container.level0)..offset_in(s.ktx2, container.level0) + container.level0.len();
        let counts = Outcomes::default();
        match budget {
            None => layer_sweep("uastc_all_modes", s.ktx2, level0, |bytes| counts.read(bytes, true)),
            Some(n) => {
                let spans = [("header, index and DFD", 0..level0.start), ("blocks", level0)];
                bounded_sweep("uastc_all_modes", s.ktx2, &spans, n, n, |bytes| counts.read(bytes, true));
            }
        }
        let (probed, decoded) = counts.report("uastc_all_modes", true);
        assert!(probed[0] > 0 && decoded[0] > 0 && decoded[1] > 0, "probe gave {probed:?} and decode {decoded:?}");
    }

    #[test]
    fn the_uastc_all_modes_decoder_survives_hostile_blocks() {
        all_modes_sweep(Some(1_000));
    }

    /// `cargo test --release --lib -- --ignored the_uastc_all_modes_decoder_survives_the_whole_sweep_bar`.
    #[test]
    #[ignore = "the whole sweep bar: run in release with --ignored"]
    fn the_uastc_all_modes_decoder_survives_the_whole_sweep_bar() {
        all_modes_sweep(None);
    }

    /// The real frame of the Normal texture (365 bytes, a megabyte out), which
    /// the strips cannot cut: every cut of it, `flips` of its 2,920 bits
    /// flipped in turn (at an even stride, every bit if `flips` is 2,920 or
    /// more) and `seeded` cases of 2 to 8 flipped bits, through `inflate` alone
    /// (the UASTC decode of a case that survives would be a second of debug
    /// build). A reading gives the megabyte or `Malformed`; a case that
    /// inflates to a megabyte costs about 15 ms in a debug build.
    fn normal_frame_sweep(flips: usize, seeded: usize) {
        let s = sample("FlightHelmet_Materials_LensesMat_Normal");
        let frame = parse(s.ktx2).unwrap().level0;
        let want = uastc::level_len(s.size.0, s.size.1);
        let (inflated, refused) = (std::cell::Cell::new(0usize), std::cell::Cell::new(0usize));
        bounded_sweep("Normal's Zstandard frame", frame, &[("frame", 0..frame.len())], flips, seeded, |bytes| match inflate(bytes, want, "1024x1024 UASTC image") {
            Ok(out) => {
                assert_eq!(out.len() as u64, want);
                inflated.set(inflated.get() + 1);
            }
            Err(MeshError::Malformed(_)) => refused.set(refused.get() + 1),
            Err(other) => panic!("inflate gave {other:?}"),
        });
        eprintln!("Normal's Zstandard frame: {} inflated to the megabyte, {} Malformed", inflated.get(), refused.get());
        assert!(inflated.get() > 0 && refused.get() > 0, "the sweep reached only one outcome");
    }

    #[test]
    fn the_flighthelmet_lenses_normal_real_frame_survives_truncation_and_bit_flips() {
        normal_frame_sweep(1_460, 500);
    }

    /// Every bit, and 2,000 seeded cases: `cargo test --release --lib --
    /// --ignored the_flighthelmet_lenses_normal_real_frame_survives_the_whole_sweep_bar`.
    #[test]
    #[ignore = "the whole sweep bar: run in release with --ignored"]
    fn the_flighthelmet_lenses_normal_real_frame_survives_the_whole_sweep_bar() {
        normal_frame_sweep(2_920, 2_000);
    }

    /// The Zstandard glue on a frame small enough for the item's whole bar in
    /// a debug build: a UASTC 8x8 texture (four blocks of the all-modes
    /// texture, then a 32-byte run of one byte, a solid-colour block's mode
    /// code twice over) in a frame of a raw block and an RLE block.
    #[test]
    fn a_small_zstd_frame_survives_truncation_and_bit_flips() {
        let blocks = parse(sample("uastc_all_modes").ktx2).unwrap().level0[..32].to_vec();
        let frame = zstd_frame(None, &[Zb::Raw(blocks), Zb::Rle(0x17, 32)]);
        let file = uastc_zstd(8, 8, frame);
        assert_eq!(decoded(&file).pixels.len(), 8 * 8 * 4, "the unbroken file decodes");
        let whole = 0..file.len();
        let counts = Outcomes::default();
        layer_sweep("UASTC 8x8 in a raw and an RLE Zstandard block", &file, whole, |bytes| counts.read(bytes, true));
        let (probed, decoded) = counts.report("UASTC 8x8 in a Zstandard frame", true);
        assert!(probed[0] > 0 && decoded[0] > 0 && decoded[1] > 0, "probe gave {probed:?} and decode {decoded:?}");
    }

    // ── real data through the codebook, size and flag paths the corpus does not take ──

    /// `file` (an ETC1S file of one level: a strip or a whole sample) with its
    /// global data rebuilt, `edit` replacing any of the endpoint, selector,
    /// table and extended blocks (in that order). The size, level 0 and the
    /// slices stay.
    fn rebuilt(file: &[u8], edit: &dyn Fn(&mut [Vec<u8>; 4])) -> Vec<u8> {
        let c = parse(file).unwrap();
        let g = c.basislz.as_ref().unwrap();
        let mut blocks = [g.endpoints.to_vec(), g.selectors.to_vec(), g.tables.to_vec(), g.extended.to_vec()];
        edit(&mut blocks);
        let span = |r: &std::ops::Range<usize>| (r.start as u32, r.len() as u32);
        let desc = Desc { rgb: span(&g.rgb), alpha: g.alpha.as_ref().map_or((0, 0), span) };
        let sgd = basislz_sgd((g.endpoint_count, g.selector_count), [&blocks[0], &blocks[1], &blocks[2], &blocks[3]], &[desc]);
        Ktx2::etc1s(c.info.width, c.info.height, vec![c.level0.to_vec()], sgd).build()
    }

    /// D.3: the selector codebook has a raw form (its second flag bit set:
    /// `0b100` with the two reserved bits), where each entry is its four row
    /// bytes as they are. Every corpus file uses the delta form, so this
    /// writes Dot_C's own selectors again in the raw form and the picture
    /// must still be the reference's.
    #[test]
    fn a_raw_selector_codebook_decodes_to_the_reference() {
        let s = sample("Dot_C");
        let strip = etc1s_strip("Dot_C", 2);
        let c = parse(&strip).unwrap();
        let g = c.basislz.as_ref().unwrap();
        let selectors = read_selectors(usize::from(g.selector_count), g.selectors).unwrap();
        let mut w = BitWriter::default();
        w.bits(0b100, 3); // two reserved zero bits, then isUncompressed
        for rows in &selectors {
            for &row in rows {
                w.bits(u32::from(row), 8);
            }
        }
        assert_ne!(w.bytes, g.selectors, "the raw form is a different encoding of the same entries");
        let raw = rebuilt(&strip, &|b| b[1] = w.bytes.clone());
        assert_eq!(first_difference(&decoded(&raw), &s.rgba()[..128 * 8 * 4], (128, 8)), None);

        // A raw codebook short of its last byte runs out.
        let short = rebuilt(&strip, &|b| b[1] = w.bytes[..w.bytes.len() - 1].to_vec());
        let why = malformed(decode_rgba8(&short));
        assert!(why.contains("the selector codebook ends before everything it declares is decoded"), "{why}");
    }

    /// A texture whose size is not a multiple of 4 has the block grid of the
    /// size rounded up, and its edge blocks are clipped. No corpus file is
    /// like that, but the header of a strip can be patched to a size that
    /// keeps the block grid (128 wide by 8 high is 32 x 2 blocks, as is 125 x
    /// 5): the same blocks decode, and the picture is the reference's rows
    /// cropped. That puts real data through the `div_ceil` grid and the
    /// clipping of both edges, and the alpha slice's clipping too.
    #[test]
    fn real_data_at_sizes_that_keep_the_block_grid_decodes_to_the_cropped_reference() {
        let sizes = [(125u32, 5u32), (126, 6), (127, 7), (128, 5), (125, 8)];
        for (stem, width) in [("Dot_C", 128u32), ("StainedGlassLamp_grill_basecolor-alpha", 1024)] {
            let s = sample(stem);
            let strip = etc1s_strip(stem, 2);
            for (cut_w, h) in sizes.map(|(w, h)| (width - (128 - w), h)) {
                let mut file = strip.clone();
                put32(&mut file, 20, cut_w);
                put32(&mut file, 24, h);
                let img = decoded(&file);
                let stride = width as usize * 4;
                let want: Vec<u8> = (0..h as usize).flat_map(|y| s.rgba()[y * stride..y * stride + cut_w as usize * 4].iter().copied()).collect();
                assert_eq!(first_difference(&img, &want, (cut_w, h)), None, "{stem} at {cut_w}x{h}");
            }
        }
    }

    /// C.7.1: `isPFrame` (0x02) says an image refers to the previous image of
    /// an animation, which a single image does not have. The reference only
    /// honours it for an animation; here it is refused by name, so a still
    /// image is never decoded as if its blocks could be skipped. The other
    /// bits are undefined and ignored, as the reference ignores them.
    #[test]
    fn a_p_frame_is_unsupported_and_named() {
        let dot = sample("Dot_C").ktx2;
        let flags_at = get64(dot, 64) + 20; // the SGD's header, then level 0's imageFlags
        let want = decoded(dot);
        for flags in [2u32, 3, 0xFFFF_FFFF] {
            let mut file = dot.to_vec();
            put32(&mut file, flags_at, flags);
            let why = unsupported(decode_rgba8(&file));
            assert!(why.contains("P-frame") && why.contains("isPFrame"), "imageFlags {flags:#x}: {why}");
            probe(&file).expect("the container is fine; only the image is refused");
        }
        for flags in [0u32, 1, 0xFFFF_FFFD] {
            let mut file = dot.to_vec();
            put32(&mut file, flags_at, flags);
            assert!(decoded(&file) == want, "imageFlags {flags:#x} without isPFrame decodes as usual");
        }
    }

    /// The codebook counts are `u16`s in the file, and a codebook costs at
    /// least a couple of bits an entry, so a count that its block's bytes
    /// cannot hold is refused before anything is sized from it.
    #[test]
    fn a_codebook_count_its_block_cannot_hold_is_refused_up_front() {
        let strip = etc1s_strip("Dot_C", 2);
        let sgd = get64(&strip, 64);
        decoded(&strip);
        for (at, what) in [(sgd, "endpoints"), (sgd + 2, "selectors")] {
            let mut file = strip.clone();
            file[at..at + 2].copy_from_slice(&u16::MAX.to_le_bytes());
            let why = malformed(decode_rgba8(&file));
            assert!(why.contains(&format!("too few for 65535 {what}")), "{why}");
        }
    }

    /// Alpha comes from the alpha slice, not from the DFD: the reference takes
    /// the DFD's byte length (60 with alpha, 44 without), and `Ktx2::etc1s`
    /// writes the 44-byte DFD of one sample, so the grill's strip is an alpha
    /// slice under a DFD that declares none. The slice decides, and the top
    /// rows are the reference's, translucent pixels included. (The converse,
    /// a DFD with alpha over a file without a slice, is
    /// `an_absent_alpha_slice_is_opaque`.)
    #[test]
    fn an_alpha_slice_decides_even_when_the_dfd_declares_no_alpha() {
        let stem = "StainedGlassLamp_grill_basecolor-alpha";
        let s = sample(stem);
        assert_eq!(get32(s.ktx2, 52), 60, "the published file's DFD declares alpha");
        let strip = etc1s_strip(stem, 2);
        assert_eq!(get32(&strip, 52), 44, "the strip's DFD declares none");
        assert!(parse(&strip).unwrap().basislz.unwrap().alpha.is_some());
        let img = decoded(&strip);
        let want = &s.rgba()[..1024 * 8 * 4];
        assert!(want.chunks_exact(4).any(|p| p[3] != 255), "the top rows have translucent pixels");
        assert_eq!(first_difference(&img, want, (1024, 8)), None);
    }

    /// A compressed Huffman table (D.1) of `total` symbols whose code length
    /// code gives `sizes` (the (symbol, size) pairs of its 21 symbols), with
    /// its code lengths written by `body`.
    fn code_length_stream(total: u32, sizes: &[(usize, u8)], body: &dyn Fn(&mut BitWriter, &Table)) -> Vec<u8> {
        let code = Table::new(21, sizes);
        let mut w = BitWriter::default();
        w.bits(total, 14).bits(21, 5);
        for symbol in CODES_ORDER {
            w.bits(u32::from(code.lengths[symbol]), 3);
        }
        body(&mut w, &code);
        w.bytes
    }

    fn read_table(bytes: &[u8]) -> Result<Huffman, MeshError> {
        Huffman::read(&mut Bits::new(bytes, "test"))
    }

    /// The symbols `table` decodes from `codes`, each a (code, length) pair.
    fn decode_all(table: &Huffman, codes: &[(u32, u32)]) -> Vec<u32> {
        let mut w = BitWriter::default();
        for &(code, len) in codes {
            w.code(code, len);
        }
        let mut bits = Bits::new(&w.bytes, "test");
        codes.iter().map(|_| table.decode(&mut bits).unwrap()).collect()
    }

    /// Each branch of `Huffman::read` (D.1): the two zero runs and the two
    /// repeats work, and everything the reference's `read_huffman_table`
    /// refuses is refused here with a message of its own: a repeat before any
    /// length, a repeat of a zero length, a repeat that goes past the
    /// table, and a zero run that goes past it.
    #[test]
    fn each_branch_of_the_compressed_huffman_table_reads_or_names_its_error() {
        // Repeat code 19 (3 + 2 bits): one length of 2, then 3 more: four 2-bit codes.
        let table = read_table(&code_length_stream(4, &[(2, 1), (19, 1)], &|w, code| {
            code.put(w, 2);
            code.put(w, 19);
            w.bits(0, 2);
        }))
        .unwrap();
        assert_eq!(decode_all(&table, &[(0, 2), (1, 2), (2, 2), (3, 2)]), [0, 1, 2, 3]);
        // Repeat code 20 (7 + 7 bits): one length of 4, then 7 + 8 = 15 more: sixteen 4-bit codes.
        let table = read_table(&code_length_stream(16, &[(4, 1), (20, 1)], &|w, code| {
            code.put(w, 4);
            code.put(w, 20);
            w.bits(8, 7);
        }))
        .unwrap();
        assert_eq!(decode_all(&table, &[(0, 4), (5, 4), (15, 4)]), [0, 5, 15]);
        // Zero run 17 (3 + 3 bits): a length of 1, three zeros, a length of 1: symbols 0 and 4.
        let table = read_table(&code_length_stream(5, &[(1, 1), (17, 1)], &|w, code| {
            code.put(w, 1);
            code.put(w, 17);
            w.bits(0, 3);
            code.put(w, 1);
        }))
        .unwrap();
        assert_eq!(decode_all(&table, &[(0, 1), (1, 1)]), [0, 4]);
        // Zero run 18 (11 + 7 bits): a length of 1, eleven zeros, a length of 1: symbols 0 and 12.
        let table = read_table(&code_length_stream(13, &[(1, 1), (18, 1)], &|w, code| {
            code.put(w, 1);
            code.put(w, 18);
            w.bits(0, 7);
            code.put(w, 1);
        }))
        .unwrap();
        assert_eq!(decode_all(&table, &[(1, 1), (0, 1)]), [12, 0]);

        let refused = |total: u32, sizes: &[(usize, u8)], body: &dyn Fn(&mut BitWriter, &Table)| malformed(read_table(&code_length_stream(total, sizes, body)));
        // A repeat code as the very first symbol.
        let why = refused(4, &[(1, 1), (19, 1)], &|w, code| {
            code.put(w, 19);
            w.bits(0, 2);
        });
        assert!(why.contains("repeats a code length before it has sent one"), "{why}");
        // A repeat of a length that is 0.
        let why = refused(8, &[(0, 1), (19, 1)], &|w, code| {
            code.put(w, 0);
            code.put(w, 19);
            w.bits(0, 2);
        });
        assert!(why.contains("repeats a code length of 0"), "{why}");
        // A repeat of 3 + 3 = 6 with 3 places left, by code 19 and by code 20 (7 times).
        let why = refused(4, &[(1, 1), (19, 1)], &|w, code| {
            code.put(w, 1);
            code.put(w, 19);
            w.bits(3, 2);
        });
        assert!(why.contains("repeats a code length 6 times past its 4 symbols"), "{why}");
        let why = refused(4, &[(1, 1), (20, 1)], &|w, code| {
            code.put(w, 1);
            code.put(w, 20);
            w.bits(0, 7);
        });
        assert!(why.contains("repeats a code length 7 times past its 4 symbols"), "{why}");
        // A zero run past the end: the table declares 4 symbols, and the run takes cur to 11 and to 12.
        let why = refused(4, &[(1, 1), (17, 1)], &|w, code| {
            code.put(w, 1);
            code.put(w, 17);
            w.bits(7, 3);
        });
        assert!(why.contains("code lengths run to 11 symbols, but it declares 4"), "{why}");
        let why = refused(4, &[(1, 1), (18, 1)], &|w, code| {
            code.put(w, 1);
            code.put(w, 18);
            w.bits(0, 7);
        });
        assert!(why.contains("code lengths run to 12 symbols, but it declares 4"), "{why}");
    }

    // ── UASTC LDR 4x4 decoding (the `uastc` module) ─────────────────────

    use super::uastc;

    /// `g_uastc_huff_modes[128]` of Basis Universal v1.50
    /// (`basisu_transcoder.cpp`), copied from its source by a script: the mode
    /// that each value of a block's first 7 bits names. 19 is the reserved
    /// "future expansion" code.
    const REFERENCE_MODES: [u8; 128] = [
        11, 0, 10, 3, 11, 15, 12, 7, 11, 18, 10, 5, 11, 14, 12, 9, //
        11, 0, 10, 4, 11, 16, 12, 8, 11, 18, 10, 6, 11, 2, 12, 13, //
        11, 0, 10, 3, 11, 17, 12, 7, 11, 18, 10, 5, 11, 14, 12, 9, //
        11, 0, 10, 4, 11, 1, 12, 8, 11, 18, 10, 6, 11, 2, 12, 13, //
        11, 0, 10, 3, 11, 19, 12, 7, 11, 18, 10, 5, 11, 14, 12, 9, //
        11, 0, 10, 4, 11, 16, 12, 8, 11, 18, 10, 6, 11, 2, 12, 13, //
        11, 0, 10, 3, 11, 17, 12, 7, 11, 18, 10, 5, 11, 14, 12, 9, //
        11, 0, 10, 4, 11, 1, 12, 8, 11, 18, 10, 6, 11, 2, 12, 13, //
    ];

    /// `g_astc_bc7_patterns2` (30 rows), `g_astc_bc7_patterns3` (11) and
    /// `g_bc7_3_astc2_patterns2` (19), copied by the same script: the
    /// subset (0, 1 or 2) of each of a block's 16 pixels, row by row, for each
    /// common partition pattern.
    const REFERENCE_PATTERNS_2: [&str; 30] = [
        "0011001100110011",
        "0001000100010001",
        "1000100010001000",
        "0001001100110111",
        "1111111011101100",
        "0011011101111111",
        "1110110010000000",
        "1111111011001000",
        "0000000000010011",
        "1100100000000000",
        "0000000101111111",
        "1111111111101000",
        "1110100000000000",
        "1111111100000000",
        "0000111111111111",
        "1111111111110000",
        "1000111011111111",
        "1111111101110001",
        "0111001100010000",
        "0011000100000000",
        "0000100011001110",
        "1111111101110011",
        "1000110011001110",
        "0011000100010000",
        "1111011101110011",
        "0110011001100110",
        "1111000000001111",
        "1010101010101010",
        "1111000011110000",
        "1001001101101100",
    ];
    const REFERENCE_PATTERNS_3: [&str; 11] = [
        "0000000011221122",
        "1111111100002222",
        "1111000000002222",
        "1111222200000000",
        "1120112011201120",
        "0112011201120112",
        "0211021102110211",
        "2000200021112111",
        "2012201220122012",
        "1111000022221111",
        "0022001100110022",
    ];
    const REFERENCE_PATTERNS_MODE_7: [&str; 19] = [
        "0000111100000000",
        "0010001000100010",
        "1100110010000000",
        "0000000100110011",
        "1111111100001111",
        "0100010001000100",
        "0001001111111111",
        "0111001100110011",
        "1100000000111100",
        "0111011100000000",
        "0000000011101110",
        "1100000000001100",
        "0111001100000000",
        "0000000111111111",
        "1111111111110110",
        "1100110011001000",
        "1111111110001000",
        "0011011011001000",
        "1111011100000000",
    ];
    /// The ASTC partition seeds of those patterns (the `m_astc` and `m_astc2`
    /// columns of `g_astc_bc7_common_partitions2`, `..._partitions3` and
    /// `g_bc7_3_astc2_common_partitions`). The pattern is the ASTC partition
    /// function of the seed (ASTC specification, "Partition pattern
    /// generation") at block size 4x4.
    const SEEDS_2: [u32; 30] = [
        28, 20, 16, 29, 91, 9, 107, 72, 149, 204, 50, 114, 496, 17, 78, 39, 252, 828, 43, 156, 116, 210, 476, 273, 684, 359, 246, 195, 694, 524,
    ];
    const SEEDS_3: [u32; 11] = [260, 74, 32, 156, 183, 15, 745, 0, 335, 902, 254];
    const SEEDS_MODE_7: [u32; 19] = [36, 48, 61, 137, 161, 183, 226, 281, 302, 307, 479, 495, 593, 594, 605, 799, 812, 988, 993];

    /// `g_bc7_weights1`, `2`, `3`, `g_astc_weights4` and `5`: the interpolation
    /// weight (0 to 64) of each weight index of 1 to 5 bits.
    const REFERENCE_WEIGHTS: [&[u32]; 5] = [
        &[0, 64],
        &[0, 21, 43, 64],
        &[0, 9, 18, 27, 37, 46, 55, 64],
        &[0, 4, 8, 12, 17, 21, 25, 29, 35, 39, 43, 47, 52, 56, 60, 64],
        &[0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 64],
    ];

    /// The all-modes texture's blocks (`make_uastc_modes.mjs`): 240 blocks, level 0.
    fn all_modes_blocks() -> Vec<[u8; 16]> {
        let level0 = parse(sample("uastc_all_modes").ktx2).unwrap().level0;
        level0.chunks_exact(16).map(|b| <[u8; 16]>::try_from(b).unwrap()).collect()
    }

    /// A block's bytes from a bit writer, padded with `fill` bits to 128.
    fn block_of(w: &BitWriter, fill: u32) -> [u8; 16] {
        let mut w = BitWriter { bytes: w.bytes.clone(), len: w.len };
        while w.len < 128 {
            let n = (128 - w.len).min(32) as u32;
            w.bits(fill, n);
        }
        <[u8; 16]>::try_from(w.bytes).expect("a block of 16 bytes")
    }

    /// The first 7 bits of a block pick its mode, by the codes of `g_uastc_mode_huff_codes`.
    #[test]
    fn the_uastc_mode_codes_pick_the_modes_of_the_reference_table() {
        for first in 0..=255u8 {
            let want = REFERENCE_MODES[usize::from(first & 127)];
            let got = uastc::mode_of(first);
            let want = (want < 19).then_some(usize::from(want));
            assert_eq!(got, want, "first byte {first:#04x}: bit 7 is not part of any code, the low 7 bits are");
        }
    }

    /// The compact partition function reproduces every row of the reference's tables.
    #[test]
    fn the_partition_function_gives_the_tables_of_the_reference() {
        let sets: [(&str, &[u32], &[&str], u32); 3] = [
            ("two-subset", &SEEDS_2, &REFERENCE_PATTERNS_2, 2),
            ("three-subset", &SEEDS_3, &REFERENCE_PATTERNS_3, 3),
            ("mode 7's two-subset", &SEEDS_MODE_7, &REFERENCE_PATTERNS_MODE_7, 2),
        ];
        for (name, seeds, rows, subsets) in sets {
            assert_eq!(seeds.len(), rows.len());
            for (i, (&seed, row)) in seeds.iter().zip(rows).enumerate() {
                let got: String = uastc::partition(seed, subsets).iter().map(|&s| char::from(b'0' + s)).collect();
                assert_eq!(&got, row, "{name} pattern {i} (seed {seed})");
            }
        }
    }

    /// The weight of an index, from the ASTC rule (replicate the index's bits to
    /// 6 bits, add 1 if over 32), is each entry of the reference's tables.
    #[test]
    fn the_uastc_weights_are_the_astc_weights() {
        for (bits, table) in (1..=5u32).zip(REFERENCE_WEIGHTS) {
            let got: Vec<u32> = (0..1u32 << bits).map(|index| uastc::weight(index, bits)).collect();
            assert_eq!(got, table, "{bits}-bit weights");
        }
    }

    /// Every endpoint range a mode uses unquantizes its values to 0..=255 without
    /// a repeat, in the numbers of levels the range says. Range 7 (0 to 11) is
    /// pinned as a whole: its twelve values are what the procedure gives (not
    /// written from a table; the proof that they are right is that the all-modes
    /// texture, whose mode 3 blocks use range 7, decodes to the reference's bytes).
    #[test]
    fn uastc_endpoints_unquantize_onto_a_rising_scale() {
        // (range, levels) for the ranges UASTC's modes use: 2 bits + trit, 4 bits, 5 bits,
        // 3 bits + quint, 4 bits + trit, 5 bits + quint, 6 bits + trit, 8 bits.
        let ranges = [(7u32, 12u32), (8, 16), (11, 32), (12, 40), (13, 48), (18, 160), (19, 192), (20, 256)];
        for (range, levels) in ranges {
            let mut values: Vec<u8> = (0..levels).map(|v| uastc::unquant(range, v)).collect();
            values.sort_unstable();
            assert_eq!(values.windows(2).filter(|w| w[0] == w[1]).count(), 0, "range {range}: a value repeats");
            assert_eq!((values[0], values[values.len() - 1]), (0, 255), "range {range}");
        }
        let mut twelve: Vec<u8> = (0..12).map(|v| uastc::unquant(7, v)).collect();
        twelve.sort_unstable();
        assert_eq!(twelve, [0, 23, 46, 69, 92, 116, 139, 163, 186, 209, 232, 255]);
        // Without a trit or quint, the bits are replicated to 8: 4 bits 0xA -> 0xAA, 5 bits 0b10110 -> 0b10110101.
        assert_eq!(uastc::unquant(8, 0xA), 0xAA);
        assert_eq!(uastc::unquant(11, 0b10110), 0b1011_0101);
        assert_eq!(uastc::unquant(20, 0x9C), 0x9C);
    }

    /// A solid-colour block (mode 8) is its 4 bytes R, G, B, A after the 5-bit code,
    /// whatever follows.
    #[test]
    fn a_uastc_solid_colour_block_is_one_colour() {
        for junk in [0, 0xFFFF_FFFF, 0x5A5A_5A5A] {
            let mut w = BitWriter::default();
            w.bits(0x17, 5).bits(0x12, 8).bits(0x34, 8).bits(0x56, 8).bits(0xF0, 8);
            let img = decoded(&Ktx2::uastc(4, 4, 0, vec![block_of(&w, junk).to_vec()]).build());
            assert_eq!((img.width, img.height), (4, 4));
            assert!(img.pixels.chunks_exact(4).all(|p| p == [0x12, 0x34, 0x56, 0xF0]), "junk {junk:#x}: {:?}", &img.pixels[..8]);
        }
    }

    /// Mode 1, written out field by field from the layout: the 6-bit code, 15 hint
    /// bits (ignored), six 8-bit endpoint values (R low, R high, G low, ...), then
    /// sixteen 2-bit weights, the first of them one bit (an anchor); 28 bits stay
    /// unused. Pixel i is each component's endpoints blended by weight {0, 21, 43, 64}.
    #[test]
    fn a_uastc_mode_1_block_is_its_endpoints_blended_by_its_weights() {
        let (lo, hi) = ([10u32, 100, 250], [200u32, 20, 250]);
        let weights = [1u32, 3, 2, 0, 1, 2, 3, 0, 2, 2, 1, 3, 0, 1, 3, 2];
        let mut w = BitWriter::default();
        w.bits(0x35, 6).bits(0x5A5A5, 15);
        for c in 0..3 {
            w.bits(lo[c], 8).bits(hi[c], 8);
        }
        w.bits(weights[0], 1);
        for &weight in &weights[1..] {
            w.bits(weight, 2);
        }
        assert_eq!(w.len, 100);
        let img = decoded(&Ktx2::uastc(4, 4, 0, vec![block_of(&w, 0xFFFF_FFFF).to_vec()]).build());
        let blend = |l: u32, h: u32, weight: u32| (((l * 257) * (64 - weight) + (h * 257) * weight + 32) >> 6) >> 8;
        let steps = [0u32, 21, 43, 64];
        for (i, pixel) in img.pixels.chunks_exact(4).enumerate() {
            let step = steps[weights[i] as usize];
            let want = [blend(lo[0], hi[0], step), blend(lo[1], hi[1], step), blend(lo[2], hi[2], step), 255];
            assert_eq!(pixel.iter().map(|&b| u32::from(b)).collect::<Vec<_>>(), want, "pixel {i}");
        }
    }

    /// The reserved mode code, and a partition pattern past the ones UASTC defines,
    /// are `Malformed` and name the block.
    #[test]
    fn a_uastc_block_with_a_reserved_mode_or_a_missing_pattern_is_malformed() {
        let one = |w: &BitWriter| decode_rgba8(&Ktx2::uastc(8, 4, 0, vec![[block_of(&BitWriter::default(), 0), block_of(w, 0)].concat()]).build());
        let mut w = BitWriter::default();
        w.bits(0x45, 7);
        let why = malformed(one(&w));
        assert!(why.contains("UASTC block (1, 0)") && why.contains("reserved mode code"), "{why}");
        // Pattern fields follow the code and the hint bits: modes 2, 4, 9 and 16 hold 5 bits (30 patterns), mode 3 4 bits (11), mode 7 5 bits (19).
        for (code, code_len, hints, bits, value, defined) in
            [(0x1D, 5, 15, 5, 30, 30), (0x13, 5, 15, 5, 31, 30), (0x03, 5, 15, 4, 11, 11), (0x07, 5, 15, 5, 19, 19), (0x0F, 5, 23, 5, 30, 30), (0x15, 6, 23, 5, 30, 30)]
        {
            let mut w = BitWriter::default();
            w.bits(code, code_len).bits(0, hints).bits(value, bits);
            let why = malformed(one(&w));
            assert!(why.contains("UASTC block (1, 0)") && why.contains(&format!("partition pattern {value}")) && why.contains(&format!("{defined}")), "{why}");
            // The last defined pattern is fine.
            let mut w = BitWriter::default();
            w.bits(code, code_len).bits(0, hints).bits(defined - 1, bits);
            decoded(&Ktx2::uastc(8, 4, 0, vec![[block_of(&BitWriter::default(), 0), block_of(&w, 0)].concat()]).build());
        }
    }

    /// A level that is not exactly 16 bytes a block is `Malformed`, for the file's size.
    #[test]
    fn a_uastc_level_of_the_wrong_length_is_malformed() {
        for len in [0usize, 48, 63, 65, 80] {
            let why = malformed(decode_rgba8(&Ktx2::uastc(8, 8, 0, vec![vec![0x17; len]]).build()));
            assert!(why.contains(&format!("level 0 holds {len} bytes")) && why.contains("a 8x8 UASTC image is 64"), "{why}");
        }
    }

    /// How many bits of its 128 each mode's fields use, worked out by hand from
    /// the layout: the mode code, the hint bits, a pattern (5 or 4 bits) or a colour
    /// component selector (2), the endpoint values (an ASTC trit is 8 bits for 5
    /// values, a quint 7 for 3), and 16 weights (32 for dual plane) of the mode's
    /// width, less one bit for each anchor. Many modes leave bits unused.
    const MODE_END_BITS: [u32; 19] = [128, 100, 119, 118, 119, 115, 128, 119, 37, 127, 128, 128, 128, 124, 123, 125, 128, 123, 128];

    #[test]
    fn every_block_of_the_all_modes_texture_ends_where_the_layout_of_its_mode_says() {
        let blocks = all_modes_blocks();
        assert_eq!(blocks.len(), 240);
        for (i, block) in blocks.iter().enumerate() {
            let parsed = uastc::parse_block(block).unwrap_or_else(|why| panic!("block {i}: {why}"));
            assert_eq!(parsed.end, MODE_END_BITS[parsed.mode], "block {i}, mode {}", parsed.mode);
        }
    }

    /// The texture would prove nothing about a path it does not reach: every mode but
    /// the reserved code, every common partition pattern and every colour component
    /// selector of the dual-plane modes appears in it.
    #[test]
    fn the_all_modes_texture_reaches_every_mode_pattern_and_selector() {
        use std::collections::BTreeSet;
        let (mut modes, mut patterns, mut selectors) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        for block in all_modes_blocks() {
            let b = uastc::parse_block(&block).unwrap();
            modes.insert(b.mode);
            if [2, 3, 4, 7, 9, 16].contains(&b.mode) {
                patterns.insert((b.mode, b.pattern));
            }
            if [6, 11, 13, 17].contains(&b.mode) {
                selectors.insert((b.mode, b.ccs));
            }
        }
        assert_eq!(modes.into_iter().collect::<Vec<_>>(), (0..19).collect::<Vec<_>>());
        let want_patterns: BTreeSet<(usize, usize)> =
            [(2, 30), (4, 30), (9, 30), (16, 30), (3, 11), (7, 19)].iter().flat_map(|&(mode, n)| (0..n).map(move |p| (mode, p))).collect();
        assert_eq!(patterns, want_patterns);
        let want_selectors: BTreeSet<(usize, usize)> = [6, 11, 13].iter().flat_map(|&mode| (0..4).map(move |c| (mode, c))).chain([(17, 3)]).collect();
        assert_eq!(selectors, want_selectors);
    }

    /// Any 16 bytes are a block: it unpacks and its pixels come out, or it is refused
    /// for the reserved mode code or a partition pattern UASTC does not define,
    /// and never anything else (no panic). Every one of the 128 bits of each of
    /// the all-modes texture's 240 blocks is flipped on its own (30,720 blocks),
    /// then the all-zero and all-one blocks, then 200,000 pseudo-random blocks
    /// from the sweeps' xorshift64 and seed. Each mode must be reached by the
    /// random ones, and each outcome must happen.
    #[test]
    fn uastc_blocks_survive_every_bit_flip_and_random_bytes() {
        /// How many blocks unpacked, by mode, and how many were refused (for the reserved mode code, for a missing pattern).
        struct Tally {
            modes: [usize; 19],
            refused: [usize; 2],
        }
        fn survive(tally: &mut Tally, block: &[u8; 16]) {
            match uastc::parse_block(block) {
                Ok(parsed) => {
                    assert!(parsed.end <= 128, "{block:02x?}: fields end at bit {}", parsed.end);
                    assert_eq!(uastc::pixels(&parsed).len(), 16);
                    tally.modes[parsed.mode] += 1;
                }
                Err(why) if why.contains("reserved mode code") => tally.refused[0] += 1,
                Err(why) if why.contains("partition pattern") => tally.refused[1] += 1,
                Err(why) => panic!("{block:02x?}: refused for another reason: {why}"),
            }
        }
        let started = std::time::Instant::now();
        let mut tally = Tally { modes: [0; 19], refused: [0; 2] };
        let mut flips = 0;
        for block in all_modes_blocks() {
            for bit in 0..128 {
                let mut flipped = block;
                flipped[bit / 8] ^= 1 << (bit % 8);
                survive(&mut tally, &flipped);
                flips += 1;
            }
        }
        survive(&mut tally, &[0; 16]);
        survive(&mut tally, &[0xFF; 16]);
        let before_random = tally.modes;
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..200_000 {
            let mut block = [0u8; 16];
            block[..8].copy_from_slice(&next().to_le_bytes());
            block[8..].copy_from_slice(&next().to_le_bytes());
            survive(&mut tally, &block);
        }
        let random: Vec<usize> = (0..19).map(|m| tally.modes[m] - before_random[m]).collect();
        eprintln!(
            "UASTC blocks: {flips} single-bit flips + 2 + 200000 random; per mode (random) {random:?}; refused: {} reserved mode code, {} missing pattern; {:?}",
            tally.refused[0],
            tally.refused[1],
            started.elapsed()
        );
        assert!(random.iter().all(|&n| n > 0), "the random blocks missed a mode: {random:?}");
        assert!(tally.refused.iter().all(|&n| n > 0), "the sweep reached only some refusals: {:?}", tally.refused);
    }

    /// 240 pseudo-random blocks of all 19 modes (0 to 18) in a 62 x 59 image (the last
    /// column of blocks 2 pixels wide, the last row 3 high), against what Basis
    /// Universal v1.50 makes of them: every mode, partition pattern, endpoint range,
    /// weight range, dual plane and cut block, bit for bit.
    #[test]
    fn the_uastc_all_modes_texture_transcodes_bit_exactly() {
        let s = sample("uastc_all_modes");
        assert_eq!((s.format, s.scheme), (Ktx2Format::Uastc, Supercompression::None));
        let img = decoded(s.ktx2);
        if let Some(why) = first_difference(&img, s.rgba(), s.size) {
            panic!("not bit-exact against the Basis Universal v1.50 reference: {why}");
        }
    }

    // ── ruzstd against the reference zstd (decision D3): testdata/ktx2/zstd/ ──

    /// Mesh-gaps item 3, decision D3: `ruzstd` is "fully tested", for a decoder
    /// we did not write. The folder `testdata/ktx2/zstd/` holds Zstandard streams
    /// that the reference tool (`zstd` 1.5.7) made, and the bytes its `zstd -d`
    /// printed for them; its README lists every command and the sha256 of each
    /// output. These tests decode the streams with litegen's own Zstandard path
    /// and compare. Everything is embedded: no `std::fs`, no manifest path.
    ///
    /// Under Miri (`cargo +nightly miri test --lib mesh::ktx2::tests::zstd_corpus`)
    /// the files whose output is 64 KiB or more, and the two that name a 128 MiB
    /// window, are skipped, and the sweeps shrink.
    mod zstd_corpus {
        use super::super::{inflate, MeshError, ZSTD_WINDOW_FLOOR};
        use super::{malformed, zstd_bytes};
        use std::io::Read;
        use std::time::{Duration, Instant};

        /// The folder's README: the tables in it are the reference, and
        /// `the_readme_lists_the_embedded_corpus` keeps them in step with `FILES`.
        const README: &str = include_str!("testdata/ktx2/zstd/README.md");

        macro_rules! embed {
            ($($name:literal),* $(,)?) => {
                /// Every `.zst` of the folder, by name.
                const FILES: &[(&str, &[u8])] = &[$(($name, include_bytes!(concat!("testdata/ktx2/zstd/", $name)))),*];
            };
        }
        embed![
            "uastc_normal.l1.check.zst", "uastc_normal.l1.nocheck.zst", "uastc_normal.l3.check.zst", "uastc_normal.l3.nocheck.zst",
            "uastc_normal.l9.check.zst", "uastc_normal.l9.nocheck.zst", "uastc_normal.l19.check.zst", "uastc_normal.l19.nocheck.zst",
            "uastc_normal.l22.check.zst", "uastc_normal.l22.nocheck.zst", "uastc_normal.l19.long27.zst", "uastc_normal.stream.l3.zst",
            "uastc_normal.stream.l19.zst", "uastc_all_modes.l1.check.zst", "uastc_all_modes.l1.nocheck.zst", "uastc_all_modes.l3.check.zst",
            "uastc_all_modes.l3.nocheck.zst", "uastc_all_modes.l9.check.zst", "uastc_all_modes.l9.nocheck.zst", "uastc_all_modes.l19.check.zst",
            "uastc_all_modes.l19.nocheck.zst", "uastc_all_modes.l22.check.zst", "uastc_all_modes.l22.nocheck.zst", "uastc_all_modes.l19.long27.zst",
            "uastc_all_modes.stream.l3.zst", "uastc_all_modes.stream.l19.zst", "rle.l1.check.zst", "rle.l1.nocheck.zst", "rle.l3.check.zst",
            "rle.l3.nocheck.zst", "rle.l9.check.zst", "rle.l9.nocheck.zst", "rle.l19.check.zst", "rle.l19.nocheck.zst", "rle.l22.check.zst",
            "rle.l22.nocheck.zst", "rle.l19.long27.zst", "rle.stream.l3.zst", "rle.stream.l19.zst", "random.l1.check.zst", "random.l1.nocheck.zst",
            "random.l3.check.zst", "random.l3.nocheck.zst", "random.l9.check.zst", "random.l9.nocheck.zst", "random.l19.check.zst",
            "random.l19.nocheck.zst", "random.l22.check.zst", "random.l22.nocheck.zst", "random.l19.long27.zst", "random.stream.l3.zst",
            "random.stream.l19.zst", "structured.l1.check.zst", "structured.l1.nocheck.zst", "structured.l3.check.zst", "structured.l3.nocheck.zst",
            "structured.l9.check.zst", "structured.l9.nocheck.zst", "structured.l19.check.zst", "structured.l19.nocheck.zst",
            "structured.l22.check.zst", "structured.l22.nocheck.zst", "structured.l19.long27.zst", "structured.stream.l3.zst",
            "structured.stream.l19.zst", "uastc_normal.stream.l22.long27.zst", "structured.stream.l22.long27.zst", "mixed3.frames3.zst",
            "uastc_normal.w10.l3.check.zst", "uastc_normal.w10.l19.nocheck.zst", "uastc_all_modes.w10.l3.check.zst",
            "uastc_all_modes.w10.l19.nocheck.zst", "rle.w10.l3.check.zst", "rle.w10.l19.nocheck.zst", "random.w10.l3.check.zst",
            "random.w10.l19.nocheck.zst", "structured.w10.l3.check.zst", "structured.w10.l19.nocheck.zst",
            "unit.l3.check.zst", "unit.l19.nocheck.zst", "unit.w10.l9.check.zst", "unit.expected.zst",
            "rle.fast1000.zst", "structured_long.l3.check.zst", "structured_long.w10.l19.nocheck.zst", "structured_long.w12.l9.check.zst",
            "structured_long.w15.l22.check.zst", "structured.skippable-first.zst", "far_repeats.l3.check.zst", "far_repeats.l19.check.zst",
            "uastc_normal.expected.zst", "uastc_all_modes.expected.zst", "rle.expected.zst", "random.expected.zst", "structured.expected.zst",
            "structured_long.expected.zst", "far_repeats.expected.zst", "mixed3.expected.zst",
        ];

        /// The nine inputs, which every `<stem>.*.zst` decodes to.
        const STEMS: [&str; 9] = ["uastc_normal", "uastc_all_modes", "rle", "random", "structured", "structured_long", "unit", "far_repeats", "mixed3"];

        /// The sweeps' seed (xorshift64), the one every sweep in this file uses.
        const FLIP_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

        /// An input whose output is this big or bigger costs a debug build
        /// milliseconds a reading, and Miri minutes.
        const BIG: u64 = 64 << 10;

        fn stem(name: &str) -> &str {
            name.split('.').next().expect("a name")
        }

        fn file(name: &str) -> &'static [u8] {
            FILES.iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("{name} is not embedded")).1
        }

        /// The files that are encodings, not expected outputs.
        fn encodings() -> impl Iterator<Item = (&'static str, &'static [u8])> {
            FILES.iter().copied().filter(|(name, _)| !name.ends_with(".expected.zst"))
        }

        // ── SHA-256 (FIPS 180-4), because the README's hashes are sha256 and mesh may not use a crate for it ──

        fn sha256(data: &[u8]) -> String {
            const K: [u32; 64] = [
                0x428a_2f98, 0x7137_4491, 0xb5c0_fbcf, 0xe9b5_dba5, 0x3956_c25b, 0x59f1_11f1, 0x923f_82a4, 0xab1c_5ed5,
                0xd807_aa98, 0x1283_5b01, 0x2431_85be, 0x550c_7dc3, 0x72be_5d74, 0x80de_b1fe, 0x9bdc_06a7, 0xc19b_f174,
                0xe49b_69c1, 0xefbe_4786, 0x0fc1_9dc6, 0x240c_a1cc, 0x2de9_2c6f, 0x4a74_84aa, 0x5cb0_a9dc, 0x76f9_88da,
                0x983e_5152, 0xa831_c66d, 0xb003_27c8, 0xbf59_7fc7, 0xc6e0_0bf3, 0xd5a7_9147, 0x06ca_6351, 0x1429_2967,
                0x27b7_0a85, 0x2e1b_2138, 0x4d2c_6dfc, 0x5338_0d13, 0x650a_7354, 0x766a_0abb, 0x81c2_c92e, 0x9272_2c85,
                0xa2bf_e8a1, 0xa81a_664b, 0xc24b_8b70, 0xc76c_51a3, 0xd192_e819, 0xd699_0624, 0xf40e_3585, 0x106a_a070,
                0x19a4_c116, 0x1e37_6c08, 0x2748_774c, 0x34b0_bcb5, 0x391c_0cb3, 0x4ed8_aa4a, 0x5b9c_ca4f, 0x682e_6ff3,
                0x748f_82ee, 0x78a5_636f, 0x84c8_7814, 0x8cc7_0208, 0x90be_fffa, 0xa450_6ceb, 0xbef9_a3f7, 0xc671_78f2,
            ];
            let mut h: [u32; 8] = [0x6a09_e667, 0xbb67_ae85, 0x3c6e_f372, 0xa54f_f53a, 0x510e_527f, 0x9b05_688c, 0x1f83_d9ab, 0x5be0_cd19];
            let mut message = data.to_vec();
            message.push(0x80);
            while message.len() % 64 != 56 {
                message.push(0);
            }
            message.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());
            for block in message.chunks_exact(64) {
                let mut w = [0u32; 64];
                for (word, bytes) in w.iter_mut().zip(block.chunks_exact(4)) {
                    *word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                }
                for i in 16..64 {
                    let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                    let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                    w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
                }
                let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
                for (k, wi) in K.iter().zip(&w) {
                    let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                    let ch = (e & f) ^ (!e & g);
                    let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(*k).wrapping_add(*wi);
                    let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                    let maj = (a & b) ^ (a & c) ^ (b & c);
                    let t2 = s0.wrapping_add(maj);
                    (hh, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
                }
                for (state, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
                    *state = state.wrapping_add(v);
                }
            }
            h.iter().map(|word| format!("{word:08x}")).collect()
        }

        // ── the README's tables ──

        /// One row of the README's table of `.zst` files.
        struct Row {
            name: &'static str,
            frames: usize,
            skippable: usize,
            window: u64,
            bytes: usize,
            sha: &'static str,
        }

        fn number(cell: &str) -> u64 {
            cell.replace(',', "").parse().unwrap_or_else(|_| panic!("README.md: {cell:?} is not a number"))
        }

        /// The README's rows for `.zst` files: `| name | command | frames | skippable | window | bytes | sha256 |`.
        /// The command has no pipe in it, so a row splits on `|`.
        fn rows() -> Vec<Row> {
            README
                .lines()
                .filter(|line| line.starts_with("| `"))
                .filter_map(|line| {
                    let cells: Vec<&str> = line.split('|').map(str::trim).collect();
                    let name = cells[1].trim_matches('`');
                    name.ends_with(".zst").then(|| {
                        assert_eq!(cells.len(), 9, "README.md: the row of {name} has {} cells, not 7", cells.len() - 2);
                        Row {
                            name,
                            frames: number(cells[3]) as usize,
                            skippable: number(cells[4]) as usize,
                            window: number(cells[5]),
                            bytes: number(cells[6]) as usize,
                            sha: cells[7].trim_matches('`'),
                        }
                    })
                })
                .collect()
        }

        fn row(name: &str) -> Row {
            rows().into_iter().find(|r| r.name == name).unwrap_or_else(|| panic!("README.md has no row for {name}"))
        }

        /// The README's row for an input, `| name.bin | origin | bytes | sha256 |`: its length and the sha256 of its bytes.
        fn input(stem: &str) -> (u64, &'static str) {
            let key = format!("| `{stem}.bin` |");
            let line = README.lines().find(|line| line.starts_with(&key)).unwrap_or_else(|| panic!("README.md has no row for {stem}.bin"));
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            assert_eq!(cells.len(), 6, "README.md: the row of {stem}.bin has {} cells, not 4", cells.len() - 2);
            (number(cells[3]), cells[4].trim_matches('`'))
        }

        fn big(stem: &str) -> bool {
            input(stem).0 >= BIG
        }

        /// An input as `<stem>.expected.zst` decodes, read by `ruzstd` and not
        /// trusted: its sha256 must be what `zstd -d` printed (the README's).
        /// Inflated on the first call and kept.
        fn expected(stem: &str) -> &'static [u8] {
            static KEPT: std::sync::Mutex<Vec<(String, &'static [u8])>> = std::sync::Mutex::new(Vec::new());
            let mut kept = KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((_, bytes)) = kept.iter().find(|(s, _)| s == stem) {
                return bytes;
            }
            let name = format!("{stem}.expected.zst");
            let bytes = zstd_bytes(file(&name));
            assert_eq!(sha256(&bytes), row(&name).sha, "{name} does not decode to what zstd -d printed");
            let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
            kept.push((stem.to_string(), bytes));
            bytes
        }

        // ── what a stream holds, read from its bytes (RFC 8878, sections 3.1, 3.1.1.2 to 3.1.1.3.2 and 4) ──

        /// A Zstandard stream's frames, windows, blocks, literals and sequence
        /// modes, counted from the bytes with no decoding. Given the output as
        /// well ([`Census::replayed`]), it also reads the sequences themselves
        /// and holds every match to the output.
        #[derive(Default, Debug, Clone, PartialEq)]
        struct Census {
            frames: usize,
            skippable: usize,
            /// The largest window a frame names.
            window: u64,
            /// Raw, RLE and compressed blocks.
            blocks: [usize; 3],
            /// Literals sections: raw, RLE, compressed (with a Huffman tree of
            /// its own) and treeless (the previous tree again).
            literals: [usize; 4],
            /// Compressed blocks with no sequences at all.
            no_sequences: usize,
            /// Huffman literals (compressed, then treeless) whose size format
            /// is 3: 18-bit sizes and a header of 5 bytes, which a literals
            /// section of 16 KiB or more needs.
            format3: [usize; 2],
            /// Huffman trees written as direct 4-bit weights, and as
            /// FSE-compressed weights.
            direct_trees: usize,
            fse_trees: usize,
            /// The largest compressed block, in bytes.
            largest_block: usize,
            /// For literal lengths, offsets and match lengths, how many
            /// compressed blocks used each mode: predefined, RLE, FSE
            /// compressed, repeat.
            sequences: [[usize; 4]; 3],
            /// Only from `replayed`: the sequences read, the bytes they and
            /// the literals make (all of the output), and the farthest back a
            /// match reached.
            sequence_count: usize,
            produced: usize,
            largest_offset: u64,
            /// Matches that named a repeat offset, by which: the previous
            /// offset, the one before, the one before that, and the previous
            /// minus one (an offset value of 3 after no literals).
            repeat_offsets: [usize; 4],
        }

        /// `bytes` as a little-endian integer.
        fn le(bytes: &[u8]) -> u64 {
            bytes.iter().rev().fold(0, |acc, &b| (acc << 8) | u64::from(b))
        }

        /// `n` bits of `bytes` from bit `at`, the first the least significant.
        fn forward_bits(bytes: &[u8], at: usize, n: u32) -> u32 {
            (0..n as usize).fold(0, |value, i| value | (u32::from((bytes[(at + i) / 8] >> ((at + i) % 8)) & 1) << i))
        }

        /// One FSE decoding table (RFC 8878, section 4.1.1): for each state, the
        /// symbol it stands for, the bits to read next and the base they add to.
        #[derive(Clone)]
        struct Fse {
            log: u32,
            cells: Vec<(u8, u8, u16)>,
        }

        impl Fse {
            fn rle(symbol: u8) -> Fse {
                Fse { log: 0, cells: vec![(symbol, 0, 0)] }
            }

            /// The table of a normalized distribution, `-1` a probability below one.
            fn of(counts: &[i16], log: u32) -> Fse {
                let size = 1usize << log;
                assert_eq!(counts.iter().map(|&c| usize::from(c.unsigned_abs())).sum::<usize>(), size, "a distribution adds up to the size of its table");
                let mut symbols = vec![0u8; size];
                let mut next: Vec<u32> = counts.iter().map(|&c| u32::from(c.unsigned_abs())).collect();
                let mut high = size as i64 - 1;
                for (symbol, _) in counts.iter().enumerate().filter(|(_, &c)| c == -1) {
                    symbols[high as usize] = symbol as u8;
                    high -= 1;
                }
                let (step, mask) = ((size >> 1) + (size >> 3) + 3, size - 1);
                let mut at = 0;
                for (symbol, &count) in counts.iter().enumerate() {
                    for _ in 0..count.max(0) {
                        symbols[at] = symbol as u8;
                        at = (at + step) & mask;
                        while at as i64 > high {
                            at = (at + step) & mask;
                        }
                    }
                }
                assert_eq!(at, 0, "the symbols spread over the whole table");
                let cells = symbols
                    .iter()
                    .map(|&symbol| {
                        let state = next[usize::from(symbol)];
                        next[usize::from(symbol)] += 1;
                        let bits = log - state.ilog2();
                        (symbol, bits as u8, ((state << bits) - size as u32) as u16)
                    })
                    .collect::<Vec<(u8, u8, u16)>>();
                // The next states of one symbol's cells tile the table.
                for (symbol, &count) in counts.iter().enumerate() {
                    let tiled: usize = cells.iter().filter(|c| usize::from(c.0) == symbol).map(|c| 1usize << c.1).sum();
                    assert_eq!(tiled, if count == 0 { 0 } else { size }, "symbol {symbol}: its states reach {tiled} of the {size}");
                }
                Fse { log, cells }
            }

            /// The table that an FSE-compressed description writes, and the bytes it took.
            fn described(bytes: &[u8], max_symbol: usize, max_log: u32) -> (Fse, usize) {
                let log = 5 + forward_bits(bytes, 0, 4);
                assert!(log <= max_log, "an accuracy log of {log}");
                let mut bit = 4;
                let (mut remaining, mut threshold, mut width) = ((1i32 << log) + 1, 1i32 << log, log + 1);
                let mut counts: Vec<i16> = Vec::new();
                let mut previous_zero = false;
                while remaining > 1 && counts.len() <= max_symbol {
                    if previous_zero {
                        // A run of symbols with no probability: 2-bit counts, each 3 for "and more".
                        loop {
                            let run = forward_bits(bytes, bit, 2);
                            bit += 2;
                            counts.extend(std::iter::repeat_n(0, run as usize));
                            if run != 3 {
                                break;
                            }
                        }
                    }
                    let max = 2 * threshold - 1 - remaining;
                    let low = forward_bits(bytes, bit, width - 1) as i32;
                    let value = if low < max {
                        bit += (width - 1) as usize;
                        low
                    } else {
                        let all = forward_bits(bytes, bit, width) as i32;
                        bit += width as usize;
                        if all >= threshold { all - max } else { all }
                    };
                    let count = value - 1;
                    remaining -= count.abs();
                    counts.push(count as i16);
                    previous_zero = count == 0;
                    while remaining < threshold {
                        width -= 1;
                        threshold >>= 1;
                    }
                }
                assert_eq!(remaining, 1, "the description adds up to the whole table");
                (Fse::of(&counts, log), bit.div_ceil(8))
            }
        }

        /// A bitstream read backward (RFC 8878, section 4.1): from just under
        /// the end mark, the highest set bit of the last byte, down; the first
        /// bit read is the most significant of what `take` returns.
        struct Back<'a> {
            bytes: &'a [u8],
            left: usize,
        }

        impl<'a> Back<'a> {
            fn new(bytes: &'a [u8]) -> Back<'a> {
                let last = *bytes.last().expect("a bitstream has a byte");
                assert!(last != 0, "the last byte of a bitstream holds its end mark");
                Back { bytes, left: 8 * (bytes.len() - 1) + 7 - last.leading_zeros() as usize }
            }

            fn take(&mut self, n: u8) -> u32 {
                assert!(self.left >= usize::from(n), "the bitstream ends early");
                let mut value = 0;
                for _ in 0..n {
                    self.left -= 1;
                    value = (value << 1) | u32::from((self.bytes[self.left / 8] >> (self.left % 8)) & 1);
                }
                value
            }
        }

        /// The baseline and the extra bits of each literal length or match length code (RFC 8878, section 3.1.1.3.2.1.1).
        fn length_codes(first: u32, plain: usize, extra: &[u8]) -> Vec<(u32, u8)> {
            let mut base = first;
            std::iter::repeat_n(&0, plain)
                .chain(extra)
                .map(|&bits| {
                    let here = base;
                    base += 1 << bits;
                    (here, bits)
                })
                .collect()
        }

        /// The default distributions of the literal length, offset and match length tables (RFC 8878, section 3.1.1.3.2.2).
        const PREDEFINED: [(&[i16], u32); 3] = [
            (&[4, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 2, 1, 1, 1, 1, 1, -1, -1, -1, -1], 6),
            (&[1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1], 5),
            (&[1, 4, 3, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1, -1, -1], 6),
        ];

        /// What reading the sequences of a frame's blocks carries from one
        /// block to the next, and the output that every match is held to.
        struct Replay<'a> {
            output: &'a [u8],
            /// Where the output stands, and where this frame's began.
            at: usize,
            start: usize,
            /// The three repeat offsets.
            rep: [u64; 3],
            /// The literal length, offset and match length tables last used.
            tables: [Option<Fse>; 3],
        }

        impl Replay<'_> {
            fn new_frame(&mut self) {
                self.start = self.at;
                self.rep = [1, 4, 8];
                self.tables = Default::default();
            }

            /// One compressed block whose literals made `literals` bytes and
            /// which holds `count` sequences; `sections` starts at the
            /// Symbol_Compression_Modes byte. Every match must copy what the
            /// output says, and the bitstream must end where the last sequence does.
            fn block(&mut self, census: &mut Census, literals: usize, count: usize, sections: &[u8]) {
                let mut rest = &sections[1..];
                for (which, shift) in [6, 4, 2].into_iter().enumerate() {
                    let (max_symbol, max_log) = [(35, 9), (31, 8), (52, 9)][which];
                    let table = match (sections[0] >> shift) & 3 {
                        0 => Fse::of(PREDEFINED[which].0, PREDEFINED[which].1),
                        1 => {
                            let table = Fse::rle(rest[0]);
                            rest = &rest[1..];
                            table
                        }
                        2 => {
                            let (table, used) = Fse::described(rest, max_symbol, max_log);
                            rest = &rest[used..];
                            table
                        }
                        _ => self.tables[which].clone().expect("a table is repeated only after one in its frame"),
                    };
                    self.tables[which] = Some(table);
                }
                let [Some(ll), Some(of), Some(ml)] = &self.tables else { unreachable!("the three tables were just set") };
                let ll_codes = length_codes(0, 16, &[1, 1, 1, 1, 2, 2, 3, 3, 4, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
                let ml_codes = length_codes(3, 32, &[1, 1, 1, 1, 2, 2, 3, 3, 4, 4, 5, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
                let mut back = Back::new(rest);
                // The three states, then per sequence: the offset's extra bits, the match length's, the literal length's, and the next states.
                let mut ll_state = back.take(ll.log as u8) as usize;
                let mut of_state = back.take(of.log as u8) as usize;
                let mut ml_state = back.take(ml.log as u8) as usize;
                let mut used = 0;
                for index in 0..count {
                    let (ll_cell, of_cell, ml_cell) = (ll.cells[ll_state], of.cells[of_state], ml.cells[ml_state]);
                    let offset_value = (1u64 << of_cell.0) + u64::from(back.take(of_cell.0));
                    let (ml_base, ml_bits) = ml_codes[usize::from(ml_cell.0)];
                    let match_length = (ml_base + back.take(ml_bits)) as usize;
                    let (ll_base, ll_bits) = ll_codes[usize::from(ll_cell.0)];
                    let literal_length = (ll_base + back.take(ll_bits)) as usize;
                    if index + 1 < count {
                        ll_state = usize::from(ll_cell.2) + back.take(ll_cell.1) as usize;
                        ml_state = usize::from(ml_cell.2) + back.take(ml_cell.1) as usize;
                        of_state = usize::from(of_cell.2) + back.take(of_cell.1) as usize;
                    }
                    // Offset values 1 to 3 name a repeat offset (shifted by one when there are no literals); more is the offset plus 3.
                    let offset = if offset_value > 3 {
                        let offset = offset_value - 3;
                        self.rep = [offset, self.rep[0], self.rep[1]];
                        offset
                    } else {
                        let slot = (offset_value - 1) as usize + usize::from(literal_length == 0);
                        census.repeat_offsets[slot] += 1;
                        let offset = if slot == 3 { self.rep[0] - 1 } else { self.rep[slot] };
                        let [first, second, third] = self.rep;
                        self.rep = match slot {
                            0 => [first, second, third],
                            1 => [second, first, third],
                            2 => [third, first, second],
                            _ => [offset, first, second],
                        };
                        offset
                    };
                    self.at += literal_length;
                    used += literal_length;
                    let (at, back_by) = (self.at, offset as usize);
                    assert!(back_by > 0 && back_by <= at - self.start, "a match at byte {at} reaches back {back_by}, past the start of its frame at {}", self.start);
                    assert!(at + match_length <= self.output.len(), "the sequences run past the {} bytes of output", self.output.len());
                    let copies = if back_by >= match_length {
                        self.output[at - back_by..at - back_by + match_length] == self.output[at..at + match_length]
                    } else {
                        (0..match_length).all(|i| self.output[at + i - back_by] == self.output[at + i])
                    };
                    assert!(copies, "the match of {match_length} bytes at byte {at}, {back_by} back, does not copy what the output holds there");
                    self.at += match_length;
                    census.largest_offset = census.largest_offset.max(offset);
                }
                assert_eq!(back.left, 0, "the sequences use the whole of their bitstream");
                assert!(used <= literals, "the sequences take {used} literals of the {literals}");
                self.at += literals - used;
                census.sequence_count += count;
            }
        }

        impl Census {
            /// Panics on a stream that is not whole: it is for the committed files.
            fn of(stream: &[u8]) -> Census {
                Census::scan(stream, None)
            }

            /// As `of`, and reads every sequence, holding each match to `output`,
            /// the bytes the stream inflates to; panics at the first that differs.
            fn replayed(stream: &[u8], output: &[u8]) -> Census {
                Census::scan(stream, Some(output))
            }

            fn scan(stream: &[u8], output: Option<&[u8]>) -> Census {
                let mut census = Census::default();
                let mut replay = output.map(|output| Replay { output, at: 0, start: 0, rep: [1, 4, 8], tables: Default::default() });
                let mut at = 0;
                while at < stream.len() {
                    let magic = le(&stream[at..at + 4]);
                    at += 4;
                    if (0x184D_2A50..=0x184D_2A5F).contains(&magic) {
                        at += 4 + le(&stream[at..at + 4]) as usize;
                        census.skippable += 1;
                        continue;
                    }
                    assert_eq!(magic, 0xFD2F_B528, "byte {} is neither a frame nor a skippable frame", at - 4);
                    census.frames += 1;
                    if let Some(replay) = replay.as_mut() {
                        replay.new_frame();
                    }
                    // Frame_Header_Descriptor: size flag, single segment, checksum, dictionary id flag.
                    let descriptor = stream[at];
                    at += 1;
                    let (size_flag, single) = (descriptor >> 6, descriptor & 0x20 != 0);
                    let mut window = 0;
                    if !single {
                        let d = stream[at];
                        at += 1;
                        let base = 1u64 << (10 + (d >> 3));
                        window = base + base / 8 * u64::from(d & 7);
                    }
                    at += [0, 1, 2, 4][usize::from(descriptor & 3)];
                    let size_len = match size_flag {
                        0 => usize::from(single),
                        1 => 2,
                        2 => 4,
                        _ => 8,
                    };
                    if single {
                        // The window is the content: the size field, which is 256 more than it says in two bytes.
                        window = le(&stream[at..at + size_len]) + if size_flag == 1 { 256 } else { 0 };
                    }
                    at += size_len;
                    census.window = census.window.max(window);
                    loop {
                        let header = le(&stream[at..at + 3]);
                        at += 3;
                        let (last, size) = (header & 1 == 1, (header >> 3) as usize);
                        match (header >> 1) & 3 {
                            0 => {
                                census.blocks[0] += 1;
                                at += size;
                                if let Some(replay) = replay.as_mut() {
                                    replay.at += size;
                                }
                            }
                            1 => {
                                census.blocks[1] += 1;
                                at += 1;
                                if let Some(replay) = replay.as_mut() {
                                    replay.at += size;
                                }
                            }
                            2 => {
                                census.blocks[2] += 1;
                                census.compressed_block(&stream[at..at + size], replay.as_mut());
                                at += size;
                            }
                            _ => panic!("a reserved block type at byte {}", at - 3),
                        }
                        if last {
                            break;
                        }
                    }
                    if descriptor & 4 != 0 {
                        at += 4;
                    }
                }
                assert_eq!(at, stream.len(), "the stream ends where its last frame does");
                if let Some(replay) = replay {
                    assert_eq!(replay.at, replay.output.len(), "the blocks make as many bytes as the output holds");
                    census.produced = replay.at;
                }
                census
            }

            /// A compressed block: its Literals_Section_Header, then its sequences.
            fn compressed_block(&mut self, block: &[u8], replay: Option<&mut Replay>) {
                let (kind, format) = (usize::from(block[0] & 3), (block[0] >> 2) & 3);
                self.literals[kind] += 1;
                self.largest_block = self.largest_block.max(block.len());
                if kind >= 2 && format == 3 {
                    self.format3[kind - 2] += 1;
                }
                // The header's length, the bytes of literals the section makes and the bytes that hold them.
                let (header, regenerated, stored) = if kind < 2 {
                    let (header, regenerated) = match format {
                        0 | 2 => (1, usize::from(block[0] >> 3)),
                        1 => (2, (le(&block[..2]) >> 4) as usize),
                        _ => (3, (le(&block[..3]) >> 4) as usize),
                    };
                    (header, regenerated, if kind == 0 { regenerated } else { 1 })
                } else {
                    // Regenerated_Size, then Compressed_Size: 10 bits each in a header of 3 bytes (formats 0 and 1), 14 in 4, 18 in 5.
                    let (header, bits) = match format {
                        0 | 1 => (3, 10),
                        2 => (4, 14),
                        _ => (5, 18),
                    };
                    let fields = le(&block[..header]) >> 4;
                    let mask = (1u64 << bits) - 1;
                    (header, (fields & mask) as usize, ((fields >> bits) & mask) as usize)
                };
                if kind == 2 {
                    // The Huffman_Tree_Description: under 128, FSE-compressed weights in that many bytes; from 128, direct 4-bit weights.
                    if block[header] < 128 {
                        self.fse_trees += 1;
                    } else {
                        self.direct_trees += 1;
                    }
                }
                let sequences = &block[header + stored..];
                let (count, count_len) = match sequences[0] {
                    0 => (0, 1),
                    n @ 1..=127 => (usize::from(n), 1),
                    255 => (usize::from(sequences[1]) + (usize::from(sequences[2]) << 8) + 0x7F00, 3),
                    n => (((usize::from(n) - 128) << 8) + usize::from(sequences[1]), 2),
                };
                if count == 0 {
                    self.no_sequences += 1;
                } else {
                    // Symbol_Compression_Modes: literal lengths, offsets, match lengths.
                    for (modes, shift) in self.sequences.iter_mut().zip([6, 4, 2]) {
                        modes[usize::from((sequences[count_len] >> shift) & 3)] += 1;
                    }
                }
                if let Some(replay) = replay {
                    if count == 0 {
                        replay.at += regenerated;
                    } else {
                        replay.block(self, regenerated, count, &sequences[count_len..]);
                    }
                }
            }
        }

        // ── decoding ──

        /// A stream of several frames, or with a skippable frame, which
        /// `inflate` refuses and `zstd -d` takes: one `StreamingDecoder` per
        /// frame over the same source, a skippable frame skipped by its length.
        /// That is how the type's documentation says to read several frames. The
        /// output is held to `want` bytes, as `inflate`'s is.
        fn inflate_frames(bytes: &[u8], window_limit: u64, want: usize) -> Result<Vec<u8>, MeshError> {
            use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};
            use ruzstd::decoding::StreamingDecoder;
            let bad = |what: String| MeshError::Malformed(format!("zstd corpus: {what}"));
            let mut rest = bytes;
            let mut out: Vec<u8> = Vec::new();
            while !rest.is_empty() {
                let frame = match StreamingDecoder::new_with_max_window_size(&mut rest, window_limit) {
                    Ok(mut decoder) => {
                        let room = (want - out.len()) as u64;
                        decoder.by_ref().take(room + 1).read_to_end(&mut out).map_err(|e| bad(format!("a frame does not decode: {e}")))?;
                        if out.len() > want {
                            return Err(bad(format!("more than the {want} bytes")));
                        }
                        Ok(())
                    }
                    Err(e) => Err(e),
                };
                match frame {
                    Ok(()) => {}
                    Err(FrameDecoderError::ReadFrameHeaderError(ReadFrameHeaderError::SkipFrame { length, .. })) => {
                        rest = rest.get(length as usize..).ok_or_else(|| bad("a skippable frame runs past the end".into()))?;
                    }
                    Err(e) => return Err(bad(format!("not a Zstandard frame: {e}"))),
                }
            }
            if out.len() != want {
                return Err(bad(format!("inflates to {} bytes, not {want}", out.len())));
            }
            Ok(out)
        }

        /// How litegen reads a file: one frame through `inflate`; several, or a
        /// skippable first, through [`inflate_frames`]; a window past the limit
        /// has to be raised.
        #[derive(Debug, Clone, Copy, PartialEq)]
        enum Shape {
            One,
            /// One frame whose window (in bytes) is past the limit.
            Wide(u64),
            Many,
        }

        /// How a good file, with this `Census`, is read to give `want` bytes.
        fn shape_of(census: &Census, want: usize) -> Shape {
            if census.frames != 1 || census.skippable != 0 {
                Shape::Many
            } else if census.window > (want as u64).max(ZSTD_WINDOW_FLOOR) {
                Shape::Wide(census.window)
            } else {
                Shape::One
            }
        }

        /// One reading of `bytes` the way its shape is read, as the sweep does:
        /// a window past the limit is read with the limit raised to it, as
        /// `ruzstd_matches_reference_zstd` reads the intact file.
        fn reads(shape: Shape, bytes: &[u8], want: usize) -> Result<Vec<u8>, MeshError> {
            match shape {
                Shape::One => inflate(bytes, want as u64, "corpus file"),
                Shape::Wide(window) => inflate_frames(bytes, window, want),
                Shape::Many => inflate_frames(bytes, (want as u64).max(ZSTD_WINDOW_FLOOR), want),
            }
        }

        /// `file`, decoded the way its `shape` is read, must give `want`'s bytes
        /// and the sha256 `zstd -d` printed. `Err` says what differs.
        fn check(file: &[u8], shape: Shape, want: &[u8], sha: &str) -> Result<(), String> {
            if let Shape::Wide(_) = shape {
                // litegen refuses a window past max(level, 8 MiB) by its size ...
                match inflate(file, want.len() as u64, "corpus file") {
                    Err(MeshError::Malformed(why)) if why.contains("window") => {}
                    other => return Err(format!("expected a refusal by window, got {other:?}")),
                }
            }
            // ... and ruzstd, told to take the window, must still agree with libzstd.
            let got = reads(shape, file, want.len()).map_err(|e| format!("litegen's path refused it: {e:?}"))?;
            if got != want {
                let at = got.iter().zip(want).position(|(a, b)| a != b);
                return Err(format!("decoded to {} bytes where zstd -d gave {}; the first byte that differs is {at:?}", got.len(), want.len()));
            }
            let actual = sha256(&got);
            if actual != sha {
                return Err(format!("decoded to sha256 {actual}, but zstd -d printed {sha}"));
            }
            Ok(())
        }

        // ── the sweep ──

        /// How much of one file a sweep reads: this many cuts and single-bit
        /// flips, evenly spaced, and `edge` bytes at the start (the frame
        /// header) and half as many at the end besides, with every cut and every
        /// bit of them; and this many seeded cases of 2 to 8 flipped bits.
        #[derive(Clone, Copy)]
        struct Budget {
            cuts: usize,
            flips: usize,
            edge: usize,
            seeded: usize,
        }

        #[derive(Default)]
        struct Tally {
            readings: usize,
            decoded: usize,
            malformed: usize,
            slowest: (Duration, String),
        }

        /// `file` cut short, with bits flipped, and with 2 to 8 random bits
        /// flipped, each read by `read`. What a hostile file may give is
        /// `want` bytes or `Malformed`: another error, another length or a
        /// panic fails, and says which case.
        fn sweep(name: &str, file: &[u8], budget: Budget, want: usize, read: &dyn Fn(&[u8]) -> Result<Vec<u8>, MeshError>, tally: &mut Tally) {
            use std::collections::BTreeSet;
            let mut reading = |bytes: &[u8], case: &dyn Fn() -> String| {
                let at = Instant::now();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read(bytes)));
                let took = at.elapsed();
                match result {
                    Err(_) => panic!("{name}: the decoder panicked on {}", case()),
                    Ok(Ok(out)) => {
                        assert_eq!(out.len(), want, "{name}: {} decoded to {} bytes, not the {want} of the level", case(), out.len());
                        tally.decoded += 1;
                    }
                    Ok(Err(MeshError::Malformed(_))) => tally.malformed += 1,
                    Ok(Err(other)) => panic!("{name}: {} gave {other:?}, and only Malformed is allowed", case()),
                }
                tally.readings += 1;
                if took > tally.slowest.0 {
                    tally.slowest = (took, format!("{name}, {}", case()));
                }
            };
            // Cuts: evenly spaced, and every length that leaves the edges.
            let len = file.len();
            let mut cuts: BTreeSet<usize> = (0..len).step_by(len.div_ceil(budget.cuts.max(1)).max(1)).collect();
            cuts.extend(0..len.min(budget.edge));
            cuts.extend(len.saturating_sub(budget.edge / 2)..len);
            for cut in cuts {
                reading(&file[..cut], &|| format!("cut to {cut} bytes"));
            }
            // One flipped bit: evenly spaced, and every bit of the edges.
            let bits = len * 8;
            let mut flips: BTreeSet<usize> = (0..bits).step_by(bits.div_ceil(budget.flips.max(1)).max(1)).collect();
            flips.extend(0..bits.min(budget.edge * 8));
            flips.extend(bits.saturating_sub(budget.edge / 2 * 8)..bits);
            let mut flipped = file.to_vec();
            for bit in flips {
                flipped[bit / 8] ^= 1 << (bit % 8);
                reading(&flipped, &|| format!("bit {bit} flipped"));
                flipped[bit / 8] ^= 1 << (bit % 8);
            }
            // 2 to 8 flipped bits, from a xorshift64 stream: the same cases every run.
            let mut state = FLIP_SEED;
            let mut next = move || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            };
            for case in 0..budget.seeded {
                let count = 2 + next() % 7;
                let mut chosen: Vec<u64> = Vec::new();
                while (chosen.len() as u64) < count {
                    let bit = next() % bits as u64;
                    if !chosen.contains(&bit) {
                        chosen.push(bit);
                    }
                }
                let mut hostile = file.to_vec();
                for &bit in &chosen {
                    hostile[(bit / 8) as usize] ^= 1 << (bit % 8);
                }
                reading(&hostile, &|| format!("case {case}: bits {chosen:?} flipped"));
            }
        }

        /// The sweep over the corpus: `small` for each file whose output is
        /// small, `big` for the rest. Prints what it read.
        fn sweep_corpus(small: Budget, big_files: Budget) -> Tally {
            let started = Instant::now();
            let mut tally = Tally::default();
            // For each input: its files, readings and the time they took.
            let mut inputs: Vec<(&str, usize, usize, Duration)> = STEMS.iter().map(|&stem| (stem, 0, 0, Duration::ZERO)).collect();
            for (name, bytes) in encodings() {
                let stem = stem(name);
                if big(stem) && cfg!(miri) {
                    continue;
                }
                let want = expected(stem).len();
                let shape = shape_of(&Census::of(bytes), want);
                if cfg!(miri) && matches!(shape, Shape::Wide(_)) {
                    continue;
                }
                let budget = if big(stem) { big_files } else { small };
                let (before, at) = (tally.readings, Instant::now());
                sweep(name, bytes, budget, want, &|cut| reads(shape, cut, want), &mut tally);
                let entry = inputs.iter_mut().find(|(s, ..)| *s == stem).expect("an input");
                (entry.1, entry.2, entry.3) = (entry.1 + 1, entry.2 + tally.readings - before, entry.3 + at.elapsed());
            }
            for (input, files, readings, took) in &inputs {
                eprintln!("  {input}: {files} files, {readings} readings, {took:?}");
            }
            eprintln!(
                "zstd corpus sweep: {} files, {} readings ({} decoded, {} Malformed), seed {FLIP_SEED:#x}, slowest {:?} ({}), wall {:?}",
                inputs.iter().map(|i| i.1).sum::<usize>(),
                tally.readings,
                tally.decoded,
                tally.malformed,
                tally.slowest.0,
                tally.slowest.1,
                started.elapsed()
            );
            assert!(tally.readings > 0, "the sweep read nothing");
            assert!(tally.decoded > 0 && tally.malformed > 0, "the sweep reached only one outcome: {} decoded, {} Malformed", tally.decoded, tally.malformed);
            tally
        }

        // ── the tests ──

        /// The FIPS 180-4 / NIST example values for SHA-256, and the 56-byte
        /// message that needs a second padding block.
        #[test]
        fn sha256_matches_the_published_vectors() {
            assert_eq!(sha256(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
            assert_eq!(sha256(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
            assert_eq!(sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
            // 112 bytes, two blocks whose padding fits in the second, and the million-`a` vector; Miri
            // runs the first and leaves the second, which is 15,625 blocks.
            assert_eq!(
                sha256(b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"),
                "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1"
            );
            if !cfg!(miri) {
                assert_eq!(sha256(&vec![b'a'; 1_000_000]), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
            }
        }

        /// The README's tables list exactly the files embedded, each once; the
        /// sizes are the files' own; and the frames, skippable frames and
        /// windows that `zstd -lv` printed are what the bytes say.
        #[test]
        fn the_readme_lists_the_embedded_corpus() {
            let mut embedded: Vec<&str> = FILES.iter().map(|(name, _)| *name).collect();
            embedded.sort_unstable();
            let mut listed: Vec<&str> = rows().iter().map(|r| r.name).collect();
            listed.sort_unstable();
            let missing: Vec<&&str> = embedded.iter().filter(|n| !listed.contains(n)).collect();
            let unexpected: Vec<&&str> = listed.iter().filter(|n| !embedded.contains(n)).collect();
            assert!(
                missing.is_empty() && unexpected.is_empty() && listed.len() == embedded.len(),
                "README.md's tables do not list the embedded corpus: they lack {missing:?}, list {unexpected:?} that is not embedded, and name {} files for {} embedded",
                listed.len(),
                embedded.len(),
            );
            let mut problems = Vec::new();
            for (name, bytes) in FILES {
                let row = row(name);
                if row.bytes != bytes.len() {
                    problems.push(format!("{name} is {} bytes, the README says {}", bytes.len(), row.bytes));
                }
                let census = Census::of(bytes);
                if (census.frames, census.skippable, census.window) != (row.frames, row.skippable, row.window) {
                    problems.push(format!(
                        "{name}: the bytes hold {} frames, {} skippable, window {}; zstd -lv printed {}, {}, {}",
                        census.frames, census.skippable, census.window, row.frames, row.skippable, row.window
                    ));
                }
            }
            for stem in STEMS {
                input(stem);
            }
            assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
        }

        /// Each expected output decodes to the bytes whose sha256 `zstd -d`
        /// printed, and three of the inputs are also made again from their
        /// recorded generators, by code that has no Zstandard in it.
        #[test]
        fn the_expected_outputs_are_what_zstd_printed() {
            for stem in STEMS {
                if cfg!(miri) && big(stem) {
                    continue;
                }
                let (len, sha) = input(stem);
                let bytes = expected(stem);
                assert_eq!(bytes.len() as u64, len, "{stem}: the README gives the input's length");
                assert_eq!(sha256(bytes), sha, "{stem}: the README gives the input's sha256");
            }
            let rle = vec![0xA5u8; 4096];
            // xorshift64 (13, 7, 17) from the recorded seed, each output as 8 little-endian bytes.
            let mut state = 0xC0FF_EE12_3456_7890u64;
            let mut random = Vec::new();
            for _ in 0..512 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                random.extend_from_slice(&state.to_le_bytes());
            }
            assert!(expected("rle") == rle, "rle is 4,096 bytes of 0xA5");
            assert!(expected("random") == random, "random is the recorded xorshift64 stream");
            assert!(expected("mixed3") == [rle, random, expected("structured").to_vec()].concat(), "mixed3 is rle, random and structured end to end");
        }

        /// The item's bar: every file decodes, the way litegen reads it, to the
        /// bytes of the reference `zstd -d`.
        #[test]
        fn ruzstd_matches_reference_zstd() {
            let started = Instant::now();
            let (mut files, mut bytes, mut shapes) = (0, 0, [0usize; 3]);
            for (name, frame) in encodings() {
                let stem = stem(name);
                if cfg!(miri) && big(stem) {
                    continue;
                }
                let want = expected(stem);
                let shape = shape_of(&Census::of(frame), want.len());
                if cfg!(miri) && matches!(shape, Shape::Wide(_)) {
                    continue;
                }
                check(frame, shape, want, row(name).sha).unwrap_or_else(|why| panic!("{name}: {why}"));
                shapes[match shape {
                    Shape::One => 0,
                    Shape::Wide(_) => 1,
                    Shape::Many => 2,
                }] += 1;
                files += 1;
                bytes += want.len();
            }
            eprintln!(
                "ruzstd matches zstd -d on {files} files ({} one frame through inflate, {} with a window past the limit, {} of several frames or a skippable first), {bytes} bytes, {:?}",
                shapes[0],
                shapes[1],
                shapes[2],
                started.elapsed()
            );
            assert!(cfg!(miri) || shapes.iter().all(|&n| n > 0), "the corpus lacks a shape: {shapes:?}");
        }

        /// `inflate` takes a level as one frame to its end, so the two files
        /// that `zstd -d` takes as several frames are refused, each for its reason.
        #[test]
        fn inflate_refuses_what_zstd_takes_as_several_frames() {
            let three = file("mixed3.frames3.zst");
            let first = input("rle").0;
            let why = malformed(inflate(three, first, "corpus file"));
            assert!(why.contains("bytes follow it"), "three frames, the first inflated: {why}");
            let skippable = file("structured.skippable-first.zst");
            let why = malformed(inflate(skippable, input("structured").0, "corpus file"));
            assert!(why.contains("not a Zstandard frame"), "a skippable frame first: {why}");
        }

        /// The comparison is a control of its own: a different byte, a wrong
        /// hash, and a stream that does not decode each fail it.
        #[test]
        fn the_comparison_catches_a_wrong_byte_a_wrong_hash_and_a_cut_stream() {
            let name = "random.l3.nocheck.zst";
            let (frame, want, sha) = (file(name), expected("random"), row(name).sha);
            assert_eq!(check(frame, Shape::One, want, sha), Ok(()), "the control: the intact file");
            // Random data is stored in raw blocks and this frame has no checksum, so
            // a flipped bit in the payload decodes, to a different byte, unnoticed.
            let mut damaged = frame.to_vec();
            damaged[2000] ^= 1;
            assert_eq!(inflate(&damaged, want.len() as u64, "corpus file").map(|out| out.len()).ok(), Some(4096), "it still decodes");
            let why = check(&damaged, Shape::One, want, sha).unwrap_err();
            assert!(why.contains("the first byte that differs is Some("), "{why}");
            let mut wrong = sha.to_string();
            wrong.replace_range(..1, if sha.starts_with('0') { "1" } else { "0" });
            let why = check(frame, Shape::One, want, &wrong).unwrap_err();
            assert!(why.contains("but zstd -d printed"), "{why}");
            let why = check(&frame[..frame.len() - 1], Shape::One, want, sha).unwrap_err();
            assert!(why.contains("litegen's path refused it"), "a cut stream: {why}");
        }

        /// The sweep reads a file that names a window past litegen's limit the
        /// way the matching test does, with the limit raised to the window: that
        /// is shown on `structured.stream.l22.long27.zst`, which `inflate` refuses
        /// by its window and the raised reading decodes whole, and on a cut copy,
        /// which the raised reading refuses as `Malformed` and `inflate` also does.
        #[test]
        fn a_window_past_the_limit_is_swept_with_the_limit_raised() {
            if cfg!(miri) {
                return;
            }
            let name = "structured.stream.l22.long27.zst";
            let (frame, want) = (file(name), expected("structured"));
            let shape = shape_of(&Census::of(frame), want.len());
            assert_eq!(shape, Shape::Wide(128 << 20), "{name} names a window of 128 MiB");
            let why = malformed(reads(Shape::One, frame, want.len()));
            assert!(why.contains("window"), "read as one frame, inflate refuses the window: {why}");
            assert!(reads(shape, frame, want.len()).is_ok_and(|out| out == want), "with the limit raised the frame decodes to the input");
            let why = malformed(reads(shape, &frame[..frame.len() - 8], want.len()));
            assert!(!why.is_empty(), "a cut frame is Malformed even with the limit raised");
        }

        /// What the files exercise in the decoder, read from their bytes (block and
        /// literals headers, and a replay of every sequence held to the output):
        /// raw, RLE and compressed blocks; raw, RLE, Huffman-compressed
        /// and treeless literals; and, for the literal length, offset and match
        /// length tables, each of the four modes (predefined, RLE, FSE compressed,
        /// repeat). A corpus made again with other settings must still reach them.
        #[test]
        fn the_corpus_reaches_every_block_kind() {
            let mut total = Census::default();
            let mut by_input: Vec<(&str, u64)> = Vec::new();
            for (name, bytes) in encodings() {
                // Every sequence is read and every match held to the input, except under Miri, which would take too long on the big ones.
                let census = std::panic::catch_unwind(|| if cfg!(miri) && big(stem(name)) { Census::of(bytes) } else { Census::replayed(bytes, expected(stem(name))) })
                    .unwrap_or_else(|payload| {
                        let why = payload.downcast_ref::<String>().cloned().or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                        panic!("{name}: {why}")
                    });
                total.frames += census.frames;
                total.skippable += census.skippable;
                total.window = total.window.max(census.window);
                total.no_sequences += census.no_sequences;
                total.direct_trees += census.direct_trees;
                total.fse_trees += census.fse_trees;
                total.largest_block = total.largest_block.max(census.largest_block);
                total.sequence_count += census.sequence_count;
                total.produced += census.produced;
                total.largest_offset = total.largest_offset.max(census.largest_offset);
                for (sum, n) in total.repeat_offsets.iter_mut().zip(census.repeat_offsets) {
                    *sum += n;
                }
                match by_input.iter_mut().find(|(input, _)| *input == stem(name)) {
                    Some(entry) => entry.1 = entry.1.max(census.largest_offset),
                    None => by_input.push((stem(name), census.largest_offset)),
                }
                for (sum, n) in total.format3.iter_mut().zip(census.format3) {
                    *sum += n;
                }
                for (sum, n) in total.blocks.iter_mut().zip(census.blocks) {
                    *sum += n;
                }
                for (sum, n) in total.literals.iter_mut().zip(census.literals) {
                    *sum += n;
                }
                for (sum, n) in total.sequences.iter_mut().flatten().zip(census.sequences.iter().flatten()) {
                    *sum += n;
                }
            }
            eprintln!("{total:?}");
            eprintln!("the farthest match, by input: {by_input:?}");
            assert!(total.blocks.iter().all(|&n| n > 0), "every block kind (raw, RLE, compressed): {total:?}");
            assert!(total.literals.iter().all(|&n| n > 0), "every literals kind (raw, RLE, compressed, treeless): {total:?}");
            for (modes, what) in total.sequences.iter().zip(["literal lengths", "offsets", "match lengths"]) {
                assert!(modes.iter().all(|&n| n > 0), "every table mode (predefined, RLE, FSE compressed, repeat) for {what}: {modes:?}");
            }
            // What only an input of a few hundred KB reaches.
            let mut missing = Vec::new();
            if total.format3[0] == 0 {
                missing.push("a Huffman literals section with size format 3 (18-bit sizes, a 5-byte header)".to_string());
            }
            if total.direct_trees == 0 {
                missing.push("a Huffman tree written as direct 4-bit weights".to_string());
            }
            if total.fse_trees == 0 {
                missing.push("a Huffman tree written as FSE-compressed weights".to_string());
            }
            if total.largest_block < 64 << 10 {
                missing.push(format!("a compressed block of 64 KiB or more (the largest is {} bytes)", total.largest_block));
            }
            // Under Miri only the small files are replayed (the big ones skip the replay), so
            // no far offset is seen there; the far-offset requirement holds outside Miri.
            if !cfg!(miri) && total.largest_offset <= 128 << 10 {
                missing.push(format!("a match that reaches back more than 128 KiB (the farthest is {} bytes)", total.largest_offset));
            }
            assert!(missing.is_empty(), "the corpus lacks {}", missing.join("; and "));
        }

        /// The census reads the sequences with a decoder of its own, so it is
        /// held to account: the stream is replayed against the output, and a
        /// stream replayed against another output, or against a short one, is
        /// refused by name.
        #[test]
        fn the_replay_holds_every_match_to_the_output() {
            let (name, frame, want) = ("structured.l19.check.zst", file("structured.l19.check.zst"), expected("structured"));
            let census = Census::replayed(frame, want);
            assert_eq!((census.produced, census.frames), (want.len(), 1), "the control: the right output is accepted whole");
            assert!(census.sequence_count > 0 && census.largest_offset > 0, "{name}: {census:?}");
            let refusal = |output: &[u8]| {
                let tried = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Census::replayed(frame, output)));
                match tried {
                    Ok(_) => String::new(),
                    Err(payload) => payload.downcast_ref::<String>().cloned().or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default(),
                }
            };
            let mut reversed = want.to_vec();
            reversed.reverse();
            let why = refusal(&reversed);
            assert!(why.contains("does not copy what the output holds"), "an output the matches do not fit: {why}");
            let why = refusal(&want[..want.len() - 1]);
            assert!(why.contains("run past") || why.contains("as many bytes"), "an output a byte short: {why}");
            let mut longer = want.to_vec();
            longer.push(0);
            let why = refusal(&longer);
            assert!(why.contains("as many bytes"), "an output a byte long: {why}");
        }

        /// The sweep must name the case it fails on and must fail on a panic,
        /// on an error that is not `Malformed` and on a wrong length: it is
        /// shown on readers that do each.
        #[test]
        fn the_sweep_fails_on_a_panic_another_error_or_a_wrong_length() {
            let (name, frame, want) = ("rle.l3.check.zst", file("rle.l3.check.zst"), 4096);
            let budget = Budget { cuts: 50, flips: 50, edge: 16, seeded: 10 };
            let outcome = |read: &dyn Fn(&[u8]) -> Result<Vec<u8>, MeshError>| {
                let tried = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sweep(name, frame, budget, want, read, &mut Tally::default())));
                match tried {
                    Ok(()) => String::new(),
                    Err(payload) => payload.downcast_ref::<String>().cloned().or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default(),
                }
            };
            let real = |bytes: &[u8]| inflate(bytes, want as u64, "corpus file");
            assert_eq!(outcome(&real), "", "the control: litegen's own path passes");
            let why = outcome(&|bytes| if bytes.len() == 5 { panic!("boom") } else { real(bytes) });
            assert!(why.contains("panicked") && why.contains("cut to 5 bytes"), "a panic on a cut: {why}");
            let why = outcome(&|bytes| if bytes.len() == frame.len() { Err(MeshError::Unsupported("x".into())) } else { real(bytes) });
            assert!(why.contains("only Malformed is allowed") && why.contains("flipped"), "another error on a flip: {why}");
            let why = outcome(&|bytes| if bytes.len() == 7 { Ok(vec![0; 3]) } else { real(bytes) });
            assert!(why.contains("decoded to 3 bytes") && why.contains("cut to 7 bytes"), "a wrong length: {why}");
        }

        /// The budgeted sweep: every file cut short and bit-flipped, in a few
        /// seconds of a debug build. A file whose output is small gets 150 cuts,
        /// 250 flipped bits and 60 seeded cases besides its edges; one whose
        /// output is big (a megabyte costs a debug build milliseconds a
        /// reading) gets 12, 12 and 4.
        #[test]
        fn the_zstd_corpus_survives_truncation_and_bit_flips() {
            let small = if cfg!(miri) { Budget { cuts: 12, flips: 12, edge: 8, seeded: 4 } } else { Budget { cuts: 150, flips: 250, edge: 16, seeded: 60 } };
            sweep_corpus(small, Budget { cuts: 12, flips: 12, edge: 6, seeded: 4 });
        }

        /// The item's whole bar: every cut, every bit and 10,000 seeded cases of
        /// each file whose output is small; of each that is big, 2,000 cuts and
        /// 3,000 bits evenly spaced (its edges whole) and 1,000 seeded cases,
        /// since a frame of 12 KB that inflates to a megabyte has 100,000 bits
        /// and a debug build takes milliseconds to read one: `cargo test --lib --
        /// --ignored the_zstd_corpus_survives_the_whole_sweep_bar`.
        #[test]
        #[ignore = "the whole sweep bar: a few minutes in a debug build"]
        fn the_zstd_corpus_survives_the_whole_sweep_bar() {
            sweep_corpus(
                Budget { cuts: usize::MAX, flips: usize::MAX, edge: 16, seeded: 10_000 },
                Budget { cuts: 2_000, flips: 3_000, edge: 16, seeded: 1_000 },
            );
        }
    }
}
