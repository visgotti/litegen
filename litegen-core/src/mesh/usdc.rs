//! USDC — USD's binary "crate" encoding — parsed into a [`Layer`] ([`parse`])
//! and written from one ([`write`]).
//!
//! # Why read crate at all
//!
//! Apple's tools write it: `usdzip --arkitAsset`, `usdcrush`, Reality
//! Converter and every iOS export produce a usdz whose root layer is a crate
//! file, and so do Blender's and most DCCs' USD exporters. A USD reader that
//! only understands text reads none of what vendors and users actually ship.
//! The writer (below) serves the pipelines that take only binary USD, and a
//! USDZ whose root is a crate, as Apple's own packagers write it.
//!
//! # The format, as this reader decodes it
//!
//! An 88-byte bootstrap (`PXR-USDC`, version, offset of the table of
//! contents), then sections named by the TOC: TOKENS (NUL-separated strings,
//! LZ4-compressed), STRINGS (token indices), FIELDS (a token and a 64-bit
//! `ValueRep` each), FIELDSETS (runs of field indices), PATHS (a compressed
//! pre-order tree of path elements) and SPECS (path, fieldset and spec type
//! per spec). Integers are delta- and width-coded and LZ4-compressed
//! (`Sdf_IntegerCompression`), LZ4 is wrapped in `TfFastCompression`'s chunk
//! framing, and a `ValueRep` either inlines a small value in its own bits or
//! points at it. Versions 0.4.0 onward are read (everything written since
//! 2017; `usdcat` writes 0.8.0); newer versions are read too, because the
//! writer only bumps the version when a newer feature is used, and such a
//! feature (splines, array edits) is refused by name when met.
//!
//! @see <https://github.com/PixarAnimationStudios/OpenUSD/blob/release/pxr/usd/sdf/crateFile.cpp>
//!
//! # Hostile input
//!
//! Every offset and count comes from the file. Each is checked against the
//! bytes that exist before anything is allocated or read; decompression is
//! bounded by the size the caller expects; the path tree is decoded with an
//! explicit stack and refuses revisits (a jump that points backwards would
//! otherwise loop forever); nested values (dictionaries) are depth-limited;
//! and the number of decoded values is charged to the same budget as the text
//! reader's.
//!
//! # Writing
//!
//! [`write`] encodes a [`Layer`] as version 0.8.0, the version `usdcat` writes,
//! and [`parse`] reads the result back as an equal layer. The layout is
//! Pixar's: the bootstrap, then the values that live out of line, then the six
//! sections (TOKENS, STRINGS, FIELDS, FIELDSETS, PATHS, SPECS), then the table
//! of contents. Each spec kind carries the fields `usdcat` gives it (a prim:
//! specifier, type, its metadata, then its children and properties; an
//! attribute: `custom`, its type, its variability, then its default,
//! connections, metadata and samples; a relationship: `variability` and its
//! targets), and a value is inlined in its `ValueRep` where OpenUSD inlines it.
//!
//! What a crate needs and the layer does not say is never guessed. An
//! attribute's crate type comes from its declared `type_name`, a metadata or
//! dictionary value's from its `Value` variant (a float is a double: the layer
//! does not keep the difference), and a type or value this cannot write is
//! [`MeshError::Unsupported`], naming it. The cheapest valid choices are made
//! where the format leaves one: arrays are uncompressed (the compressed bit
//! clear, which the reader takes), TOKENS is one chunk of literal-only LZ4,
//! STRINGS is raw, and FIELDS, FIELDSETS, PATHS and SPECS are Sdf
//! integer-coded (with FIELDS' value reps in literal-only LZ4 too). Literal-only
//! LZ4 is valid LZ4, which any decoder takes, and needs no match finder.
//! Output is deterministic: no hash map's order reaches the bytes.
//!
//! The layer is what [`super::usda::parse`] makes of litegen's own USDA text
//! (`usd::write_usdc`), so the crate holds exactly what the text does. The
//! writer's output was checked by hand with `usdcat` and `usdchecker` (Apple
//! USD Tools 0.25.2), and the reader's truncation and bit-flip sweeps run on
//! it too.

use super::usd_stage::{Layer, ListOp, PrimSpec, Reference, RelSpec, Specifier, Value, Variability, MAX_LIST_OP_ITEMS, MAX_PATH_LEN};
use super::usda::MAX_VALUES;
use super::{MeshError, MeshFormat};
use std::collections::{BTreeSet, HashMap, HashSet};

const MAGIC: &[u8; 8] = b"PXR-USDC";
const BOOTSTRAP_LEN: usize = 88;
/// Arrays shorter than this are never compressed, whatever their flag says
/// (`_MinCompressedArraySize` in crateFile.cpp).
const MIN_COMPRESSED_ARRAY: usize = 16;
/// LZ4's largest single block (`LZ4_MAX_INPUT_SIZE`), which bounds each chunk
/// of `TfFastCompression` output.
const LZ4_MAX_BLOCK: usize = 0x7E00_0000;
/// Sections the TOC may list. The format has six; a few more are tolerated
/// for forward compatibility.
const MAX_SECTIONS: u64 = 64;
/// Nesting of dictionaries inside values.
const MAX_VALUE_DEPTH: usize = 16;
/// Largest structural table, in entries. Well past any real layer; it stops a
/// count field from requesting a giant allocation before the data is checked.
const MAX_TABLE: usize = 64 * 1024 * 1024;

fn malformed(detail: impl Into<String>) -> MeshError {
    MeshError::Malformed(format!("usdc: {}", detail.into()))
}

fn unsupported(detail: impl Into<String>) -> MeshError {
    MeshError::Unsupported(format!("usdc: {}", detail.into()))
}

pub fn looks_like_crate(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

// ─── byte reading ───────────────────────────────────────────────────────────

#[derive(Clone)]
struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn at(b: &'a [u8], pos: usize) -> Cursor<'a> {
        Cursor { b, pos }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], MeshError> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.b.len()).ok_or_else(|| malformed("data runs past the end of the file"))?;
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, MeshError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, MeshError> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn i32(&mut self) -> Result<i32, MeshError> {
        Ok(self.u32()? as i32)
    }

    fn u64(&mut self) -> Result<u64, MeshError> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Ok(u64::from_le_bytes(a))
    }

    fn i64(&mut self) -> Result<i64, MeshError> {
        Ok(self.u64()? as i64)
    }

    fn f64(&mut self) -> Result<f64, MeshError> {
        Ok(f64::from_bits(self.u64()?))
    }

    /// A count that will be followed by `count * elem` bytes: checked against
    /// what remains before anyone allocates for it.
    fn count(&mut self, elem: usize, what: &str) -> Result<usize, MeshError> {
        let n = self.u64()?;
        let remaining = self.b.len().saturating_sub(self.pos);
        let n = usize::try_from(n).map_err(|_| malformed(format!("{what} count is absurd")))?;
        if elem > 0 && n.checked_mul(elem).is_none_or(|bytes| bytes > remaining) {
            return Err(malformed(format!("{what} declares {n} entries but the file holds fewer")));
        }
        Ok(n)
    }
}

// ─── LZ4 and integer coding ─────────────────────────────────────────────────

/// Decode one raw LZ4 block (no frame) into `out`, which must not grow past
/// `limit` bytes in total.
///
/// LZ4 is a sequence of `[token][literal length ext][literals][offset][match
/// length ext]`; the last sequence stops after its literals. Matches copy from
/// earlier output of the same block, possibly overlapping the bytes being
/// written (that is how runs are encoded), so the copy is byte by byte.
fn lz4_block(src: &[u8], out: &mut Vec<u8>, limit: usize) -> Result<(), MeshError> {
    let block_start = out.len();
    let mut i = 0usize;
    let ext = |i: &mut usize, mut n: usize| -> Result<usize, MeshError> {
        loop {
            let b = *src.get(*i).ok_or_else(|| malformed("lz4: a length runs past the block"))?;
            *i += 1;
            n = n.checked_add(b as usize).ok_or_else(|| malformed("lz4: a length overflows"))?;
            if b != 255 {
                return Ok(n);
            }
        }
    };
    loop {
        let token = *src.get(i).ok_or_else(|| malformed("lz4: the block ends inside a sequence"))?;
        i += 1;
        let mut lit = (token >> 4) as usize;
        if lit == 15 {
            lit = ext(&mut i, lit)?;
        }
        let lit_end = i.checked_add(lit).filter(|e| *e <= src.len()).ok_or_else(|| malformed("lz4: literals run past the block"))?;
        if out.len() + lit > limit {
            return Err(malformed("lz4: the block decompresses past its declared size"));
        }
        out.extend_from_slice(&src[i..lit_end]);
        i = lit_end;
        if i == src.len() {
            return Ok(());
        }
        let off = src.get(i..i + 2).ok_or_else(|| malformed("lz4: a match offset runs past the block"))?;
        let offset = u16::from_le_bytes([off[0], off[1]]) as usize;
        i += 2;
        let mut mlen = (token & 15) as usize;
        if mlen == 15 {
            mlen = ext(&mut i, mlen)?;
        }
        mlen += 4;
        if offset == 0 || offset > out.len() - block_start {
            return Err(malformed("lz4: a match points before the start of the block"));
        }
        if out.len() + mlen > limit {
            return Err(malformed("lz4: the block decompresses past its declared size"));
        }
        let from = out.len() - offset;
        for k in 0..mlen {
            let b = out[from + k];
            out.push(b);
        }
    }
}

/// `TfFastCompression`: byte 0 is 0 for one LZ4 block, else the number of
/// chunks, each `[i32 size][block]`.
fn fast_decompress(src: &[u8], limit: usize) -> Result<Vec<u8>, MeshError> {
    let (&chunks, rest) = src.split_first().ok_or_else(|| malformed("an empty compressed blob"))?;
    // Capacity is bounded by the input too: LZ4 cannot expand a byte into more
    // than ~255, so a small blob claiming a huge size reserves nothing.
    let mut out = Vec::with_capacity(limit.min(src.len().saturating_mul(255)).min(1 << 26));
    if chunks == 0 {
        lz4_block(rest, &mut out, limit)?;
        return Ok(out);
    }
    let mut c = Cursor::at(rest, 0);
    for _ in 0..chunks {
        let size = c.i32()?;
        let size = usize::try_from(size).map_err(|_| malformed("a negative compressed chunk size"))?;
        let chunk = c.take(size)?;
        let chunk_limit = limit.min(out.len().saturating_add(LZ4_MAX_BLOCK));
        lz4_block(chunk, &mut out, chunk_limit)?;
    }
    Ok(out)
}

/// `Sdf_IntegerCompression`: decode `count` integers from their compressed
/// encoding (a common value, 2-bit width codes, then the odd-sized deltas),
/// each a running sum. `wide` selects the 64-bit variant.
fn decode_ints(data: &[u8], count: usize, wide: bool) -> Result<Vec<i64>, MeshError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut c = Cursor::at(data, 0);
    let common = if wide { c.i64()? } else { c.i32()? as i64 };
    let codes_len = (count * 2).div_ceil(8);
    let codes = c.take(codes_len)?;
    let mut out = Vec::with_capacity(count);
    let mut prev: i64 = 0;
    for i in 0..count {
        let code = (codes[i / 4] >> (2 * (i % 4))) & 3;
        let delta = match (code, wide) {
            (0, _) => common,
            (1, false) => c.take(1)?[0] as i8 as i64,
            (2, false) => {
                let s = c.take(2)?;
                i16::from_le_bytes([s[0], s[1]]) as i64
            }
            (3, false) => c.i32()? as i64,
            (1, true) => {
                let s = c.take(2)?;
                i16::from_le_bytes([s[0], s[1]]) as i64
            }
            (2, true) => c.i32()? as i64,
            _ => c.i64()?,
        };
        prev = if wide { prev.wrapping_add(delta) } else { (prev as i32).wrapping_add(delta as i32) as i64 };
        out.push(prev);
    }
    Ok(out)
}

/// `[u64 compressed size][TfFastCompression blob]` decoding to `count`
/// integers.
fn compressed_ints(c: &mut Cursor<'_>, count: usize, wide: bool) -> Result<Vec<i64>, MeshError> {
    if count > MAX_TABLE {
        return Err(unsupported(format!("a table of {count} integers")));
    }
    let size = c.count(1, "compressed integers")?;
    let blob = c.take(size)?;
    if count == 0 {
        return Ok(Vec::new());
    }
    let w = if wide { 8 } else { 4 };
    let limit = w + (count * 2).div_ceil(8) + count * w;
    let raw = fast_decompress(blob, limit)?;
    decode_ints(&raw, count, wide)
}

// ─── the file ───────────────────────────────────────────────────────────────

/// SdfSpecType.
const SPEC_ATTRIBUTE: i64 = 1;
const SPEC_PRIM: i64 = 6;
const SPEC_PSEUDO_ROOT: i64 = 7;
const SPEC_RELATIONSHIP: i64 = 8;
const SPEC_VARIANT: i64 = 10;
const SPEC_VARIANT_SET: i64 = 11;

/// Value type codes (`crateDataTypes.h`). The table never reorders, so the
/// numbers are the format.
mod ty {
    pub const BOOL: u8 = 1;
    pub const UCHAR: u8 = 2;
    pub const INT: u8 = 3;
    pub const UINT: u8 = 4;
    pub const INT64: u8 = 5;
    pub const UINT64: u8 = 6;
    pub const HALF: u8 = 7;
    pub const FLOAT: u8 = 8;
    pub const DOUBLE: u8 = 9;
    pub const STRING: u8 = 10;
    pub const TOKEN: u8 = 11;
    pub const ASSET_PATH: u8 = 12;
    pub const MATRIX2D: u8 = 13;
    pub const MATRIX3D: u8 = 14;
    pub const MATRIX4D: u8 = 15;
    pub const QUATD: u8 = 16;
    pub const QUATF: u8 = 17;
    pub const QUATH: u8 = 18;
    pub const VEC2D: u8 = 19;
    pub const VEC2F: u8 = 20;
    pub const VEC2H: u8 = 21;
    pub const VEC2I: u8 = 22;
    pub const VEC3D: u8 = 23;
    pub const VEC3F: u8 = 24;
    pub const VEC3H: u8 = 25;
    pub const VEC3I: u8 = 26;
    pub const VEC4D: u8 = 27;
    pub const VEC4F: u8 = 28;
    pub const VEC4H: u8 = 29;
    pub const VEC4I: u8 = 30;
    pub const DICTIONARY: u8 = 31;
    pub const TOKEN_LIST_OP: u8 = 32;
    pub const STRING_LIST_OP: u8 = 33;
    pub const PATH_LIST_OP: u8 = 34;
    pub const REFERENCE_LIST_OP: u8 = 35;
    pub const INT_LIST_OP: u8 = 36;
    pub const INT64_LIST_OP: u8 = 37;
    pub const UINT_LIST_OP: u8 = 38;
    pub const UINT64_LIST_OP: u8 = 39;
    pub const PATH_VECTOR: u8 = 40;
    pub const TOKEN_VECTOR: u8 = 41;
    pub const SPECIFIER: u8 = 42;
    pub const PERMISSION: u8 = 43;
    pub const VARIABILITY: u8 = 44;
    pub const VARIANT_SELECTION_MAP: u8 = 45;
    pub const TIME_SAMPLES: u8 = 46;
    pub const PAYLOAD: u8 = 47;
    pub const DOUBLE_VECTOR: u8 = 48;
    pub const LAYER_OFFSET_VECTOR: u8 = 49;
    pub const STRING_VECTOR: u8 = 50;
    pub const VALUE_BLOCK: u8 = 51;
    pub const VALUE: u8 = 52;
    pub const PAYLOAD_LIST_OP: u8 = 55;
    pub const TIME_CODE: u8 = 56;
}

/// Scalar layout of the numeric types: (components, bytes per component,
/// kind) where kind is 'f' float, 'h' half, 'd' double, 'i' signed and 'u'
/// unsigned integer. Quaternions are stored (x, y, z, w), the stage's order
/// already, so they need no flag of their own.
fn numeric_layout(t: u8) -> Option<(usize, usize, u8)> {
    Some(match t {
        ty::BOOL | ty::UCHAR => (1, 1, b'u'),
        ty::INT => (1, 4, b'i'),
        ty::UINT => (1, 4, b'u'),
        ty::INT64 => (1, 8, b'i'),
        ty::UINT64 => (1, 8, b'u'),
        ty::HALF => (1, 2, b'h'),
        ty::FLOAT => (1, 4, b'f'),
        ty::DOUBLE | ty::TIME_CODE => (1, 8, b'd'),
        ty::MATRIX2D => (4, 8, b'd'),
        ty::MATRIX3D => (9, 8, b'd'),
        ty::MATRIX4D => (16, 8, b'd'),
        ty::QUATD => (4, 8, b'd'),
        ty::QUATF => (4, 4, b'f'),
        ty::QUATH => (4, 2, b'h'),
        ty::VEC2D => (2, 8, b'd'),
        ty::VEC2F => (2, 4, b'f'),
        ty::VEC2H => (2, 2, b'h'),
        ty::VEC2I => (2, 4, b'i'),
        ty::VEC3D => (3, 8, b'd'),
        ty::VEC3F => (3, 4, b'f'),
        ty::VEC3H => (3, 2, b'h'),
        ty::VEC3I => (3, 4, b'i'),
        ty::VEC4D => (4, 8, b'd'),
        ty::VEC4F => (4, 4, b'f'),
        ty::VEC4H => (4, 2, b'h'),
        ty::VEC4I => (4, 4, b'i'),
        _ => return None,
    })
}

/// IEEE 754 half to f32, exactly.
pub fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0f32 } else { 1.0 };
    let exp = ((h >> 10) & 0x1F) as i32;
    let frac = (h & 0x3FF) as f32;
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        31 => {
            if frac == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => sign * (1.0 + frac / 1024.0) * 2f32.powi(e - 15),
    }
}

/// f32 to IEEE 754 half, rounded to nearest even: the partner of
/// [`half_to_f32`], so that `f32_to_half(half_to_f32(h)) == h` for every half
/// that is not a NaN. The crate writer encodes every half with it.
pub fn f32_to_half(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let frac = bits & 0x007F_FFFF;
    if exp == 0xFF {
        // Infinity, or a NaN (kept one by its quiet bit).
        return sign | 0x7C00 | if frac != 0 { 0x0200 } else { 0 };
    }
    let e = exp - 127;
    if e > 15 {
        return sign | 0x7C00;
    }
    // Round the dropped low bits to nearest, a tie to the even result. A carry
    // out of the fraction rolls into the exponent, and out of the top exponent
    // into infinity, which is what IEEE rounding says.
    let round = |kept: u32, rest: u32, half: u32| if rest > half || (rest == half && kept & 1 == 1) { kept + 1 } else { kept };
    if e >= -14 {
        // A normal half: 10 fraction bits, so the low 13 are dropped.
        let h = round((((e + 15) as u32) << 10) | (frac >> 13), frac & 0x1FFF, 0x1000);
        return sign | h as u16;
    }
    if e < -25 {
        // Under half the smallest subnormal: zero.
        return sign;
    }
    // A subnormal half is a multiple of 2^-24: shift the 24-bit significand
    // down to it.
    let significand = frac | 0x0080_0000;
    let shift = (-e - 1) as u32;
    let h = round(significand >> shift, significand & ((1 << shift) - 1), 1 << (shift - 1));
    sign | h as u16
}

fn component(s: &[u8], size: usize, kind: u8) -> f64 {
    match (size, kind) {
        (1, _) => s[0] as f64,
        (2, _) => half_to_f32(u16::from_le_bytes([s[0], s[1]])) as f64,
        (4, b'f') => f32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64,
        (4, b'i') => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64,
        (4, _) => u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64,
        (8, b'd') => {
            let mut a = [0u8; 8];
            a.copy_from_slice(&s[..8]);
            f64::from_le_bytes(a)
        }
        (8, b'i') => {
            let mut a = [0u8; 8];
            a.copy_from_slice(&s[..8]);
            i64::from_le_bytes(a) as f64
        }
        _ => {
            let mut a = [0u8; 8];
            a.copy_from_slice(&s[..8]);
            u64::from_le_bytes(a) as f64
        }
    }
}

struct Crate<'a> {
    b: &'a [u8],
    minor: u8,
    tokens: Vec<String>,
    strings: Vec<u32>,
    paths: PathTable,
    values: usize,
}

/// A decoded field: its name and value.
type Field = (String, Value);

/// How [`Crate::value`] ends its message for a value it does not decode.
const UNREAD: &str = "which is not read";

/// A field's value, or `None` for one of a crate type this reader does not
/// decode (newer USD writes path expressions, splines, array edits): that
/// field is skipped and its type named in `unread`, for the layer's warning,
/// instead of failing the layer. Any other failure is the layer's.
fn field_value(cr: &mut Crate, rep: u64, unread: &mut BTreeSet<String>) -> Result<Option<Value>, MeshError> {
    match cr.value(rep, 0) {
        Ok(v) => Ok(Some(v)),
        Err(MeshError::Unsupported(m)) if m.ends_with(UNREAD) => {
            unread.insert(if rep >> 60 & 1 == 1 { "array edit".to_string() } else { ((rep >> 48) & 0xFF).to_string() });
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// The crate format, OpenUSD's own definition (openusd.org documents no byte
/// layout), v26.11.1: the bootstrap, table of contents and sections
/// (https://github.com/PixarAnimationStudios/OpenUSD/blob/v26.11.1/pxr/usd/sdf/crateFile.h#L477-L499, crateFile.cpp#L3454-L3619), the ValueRep bits
/// (crateFile.h#L65-L146) and type numbers (crateDataTypes.h#L14-L109, which
/// never change), compressed integers (crateFile.cpp#L2188-L2263,
/// integerCoding.cpp#L21-L79) and LZ4 blocks.
pub fn parse(bytes: &[u8], id: &str) -> Result<Layer, MeshError> {
    if !looks_like_crate(bytes) {
        return Err(MeshError::NotThisFormat {
            expected: MeshFormat::Usda,
            detail: "a USDC layer starts with `PXR-USDC`".into(),
        });
    }
    if bytes.len() < BOOTSTRAP_LEN {
        return Err(malformed("the file is shorter than its header"));
    }
    let (major, minor) = (bytes[8], bytes[9]);
    if major != 0 || minor < 4 {
        return Err(unsupported(format!(
            "crate version {major}.{minor}.{}; versions 0.4.0 and later are read",
            bytes[10]
        )));
    }
    let toc = Cursor::at(bytes, 16).u64()?;
    let toc = usize::try_from(toc).ok().filter(|t| *t >= BOOTSTRAP_LEN && *t < bytes.len()).ok_or_else(|| malformed("the table of contents lies outside the file"))?;
    let mut c = Cursor::at(bytes, toc);
    let n = c.u64()?;
    if n > MAX_SECTIONS {
        return Err(malformed(format!("{n} sections in the table of contents")));
    }
    let mut sections: Vec<(String, usize, usize)> = Vec::new();
    for _ in 0..n {
        let name = c.take(16)?;
        let end = name.iter().position(|b| *b == 0).unwrap_or(16);
        let name = String::from_utf8_lossy(&name[..end]).into_owned();
        let start = c.i64()?;
        let size = c.i64()?;
        let (start, size) = match (usize::try_from(start), usize::try_from(size)) {
            (Ok(s), Ok(z)) if s.checked_add(z).is_some_and(|e| e <= bytes.len()) => (s, z),
            _ => return Err(malformed(format!("section {name} lies outside the file"))),
        };
        sections.push((name, start, size));
    }
    let section = |name: &str| -> Result<&[u8], MeshError> {
        sections
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, s, z)| &bytes[*s..*s + *z])
            .ok_or_else(|| malformed(format!("no {name} section")))
    };

    let mut cr = Crate { b: bytes, minor, tokens: Vec::new(), strings: Vec::new(), paths: PathTable::default(), values: 0 };

    // TOKENS
    {
        let mut c = Cursor::at(section("TOKENS")?, 0);
        let count = c.u64()?;
        let usize_ = c.u64()?;
        let csize = c.count(1, "token blob")?;
        let count = usize::try_from(count).ok().filter(|n| *n <= MAX_TABLE).ok_or_else(|| unsupported("too many tokens"))?;
        let uncompressed = usize::try_from(usize_).ok().filter(|n| *n <= 1 << 28).ok_or_else(|| unsupported("the token table is too large"))?;
        let blob = c.take(csize)?;
        let raw = fast_decompress(blob, uncompressed)?;
        if raw.len() != uncompressed {
            return Err(malformed("the token table decompresses to the wrong size"));
        }
        let mut toks: Vec<String> = Vec::with_capacity(count.min(raw.len() + 1));
        for t in raw.split(|b| *b == 0) {
            if toks.len() == count {
                break;
            }
            toks.push(String::from_utf8_lossy(t).into_owned());
        }
        if toks.len() != count {
            return Err(malformed(format!("{count} tokens declared, {} present", toks.len())));
        }
        cr.tokens = toks;
    }
    // STRINGS
    {
        let mut c = Cursor::at(section("STRINGS")?, 0);
        let n = c.count(4, "strings")?;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let t = c.u32()?;
            if t as usize >= cr.tokens.len() {
                return Err(malformed("a string names a token that does not exist"));
            }
            v.push(t);
        }
        cr.strings = v;
    }
    // FIELDS
    let fields: Vec<(u32, u64)> = {
        let mut c = Cursor::at(section("FIELDS")?, 0);
        let n = c.u64()?;
        let n = usize::try_from(n).ok().filter(|n| *n <= MAX_TABLE).ok_or_else(|| unsupported("too many fields"))?;
        let toks = compressed_ints(&mut c, n, false)?;
        let rsize = c.count(1, "field values")?;
        let blob = c.take(rsize)?;
        let reps = fast_decompress(blob, n * 8)?;
        if reps.len() != n * 8 {
            return Err(malformed("the field values decompress to the wrong size"));
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let t = u32::try_from(toks[i]).ok().filter(|t| (*t as usize) < cr.tokens.len()).ok_or_else(|| malformed("a field names a token that does not exist"))?;
            let mut a = [0u8; 8];
            a.copy_from_slice(&reps[i * 8..i * 8 + 8]);
            out.push((t, u64::from_le_bytes(a)));
        }
        out
    };
    // FIELDSETS
    let fieldsets: Vec<i64> = {
        let mut c = Cursor::at(section("FIELDSETS")?, 0);
        let n = c.u64()?;
        let n = usize::try_from(n).ok().filter(|n| *n <= MAX_TABLE).ok_or_else(|| unsupported("too many field sets"))?;
        compressed_ints(&mut c, n, false)?
    };
    // PATHS
    {
        let mut c = Cursor::at(section("PATHS")?, 0);
        let total = c.u64()?;
        let total = usize::try_from(total).ok().filter(|n| *n <= MAX_TABLE).ok_or_else(|| unsupported("too many paths"))?;
        let encoded = c.u64()?;
        let encoded = usize::try_from(encoded).ok().filter(|n| *n <= total).ok_or_else(|| malformed("more encoded paths than paths"))?;
        let idx = compressed_ints(&mut c, encoded, false)?;
        let elem = compressed_ints(&mut c, encoded, false)?;
        let jumps = compressed_ints(&mut c, encoded, false)?;
        cr.paths = build_paths(&cr.tokens, total, &idx, &elem, &jumps)?;
    }
    // SPECS
    let specs: Vec<(usize, usize, i64)> = {
        let mut c = Cursor::at(section("SPECS")?, 0);
        let n = c.u64()?;
        let n = usize::try_from(n).ok().filter(|n| *n <= MAX_TABLE).ok_or_else(|| unsupported("too many specs"))?;
        let pi = compressed_ints(&mut c, n, false)?;
        let fs = compressed_ints(&mut c, n, false)?;
        let st = compressed_ints(&mut c, n, false)?;
        (0..n)
            .map(|i| {
                let p = usize::try_from(pi[i]).ok().filter(|p| *p < cr.paths.len()).ok_or_else(|| malformed("a spec names a path that does not exist"))?;
                let f = usize::try_from(fs[i]).ok().filter(|f| *f < fieldsets.len()).ok_or_else(|| malformed("a spec names a field set that does not exist"))?;
                Ok((p, f, st[i]))
            })
            .collect::<Result<_, MeshError>>()?
    };

    // Specs to layer.
    let mut layer = Layer { id: id.to_string(), ..Default::default() };
    let mut unread: BTreeSet<String> = BTreeSet::new();
    // Properties arrive in any order; they attach to their prim after, and are
    // then put in the prim's authored `properties` order.
    let mut props: Vec<(String, String, i64, Vec<Field>)> = Vec::new();
    let mut orders: Vec<(String, Vec<String>)> = Vec::new();
    let mut variant_sets: Vec<(String, String, Vec<String>)> = Vec::new();
    for (path_index, fieldset, spec_type) in specs {
        let path = cr.paths.get(path_index).flatten().cloned().ok_or_else(|| malformed("a spec's path was never decoded"))?;
        let mut fields_here: Vec<Field> = Vec::new();
        let mut i = fieldset;
        loop {
            let f = *fieldsets.get(i).ok_or_else(|| malformed("a field set runs past its table"))?;
            if f < 0 || f == 0xFFFF_FFFF {
                break;
            }
            let (tok, rep) = *fields.get(f as usize).ok_or_else(|| malformed("a field set names a field that does not exist"))?;
            // Every visit is charged: specs may legally share one field set, so
            // without this a single long run shared by a million specs costs
            // specs x run decodes (an inlined value charges nothing itself).
            cr.charge(1)?;
            let name = cr.tokens[tok as usize].clone();
            if let Some(v) = field_value(&mut cr, rep, &mut unread)? {
                fields_here.push((name, v));
            }
            i += 1;
        }
        match spec_type {
            SPEC_PSEUDO_ROOT => {
                for (k, v) in fields_here {
                    match k.as_str() {
                        "primChildren" => layer.root_children = strs(&v),
                        "subLayers" => {
                            for s in strs(&v) {
                                layer.sublayers.push((s, 0.0, 1.0));
                            }
                        }
                        "subLayerOffsets" => {
                            if let Value::Tuple(t) = &v {
                                for (i, pair) in t.chunks_exact(2).enumerate() {
                                    if let Some(s) = layer.sublayers.get_mut(i) {
                                        s.1 = pair[0];
                                        s.2 = pair[1];
                                    }
                                }
                            }
                        }
                        _ => layer.meta.push((k, v)),
                    }
                }
            }
            SPEC_PRIM | SPEC_VARIANT => {
                let mut spec = PrimSpec {
                    // A variant's own spec carries no specifier: it is an
                    // opinion on the prim that owns the set.
                    specifier: if spec_type == SPEC_VARIANT { Specifier::Over } else { Specifier::Def },
                    ..Default::default()
                };
                let mut prop_order: Vec<String> = Vec::new();
                for (k, v) in fields_here {
                    apply_prim_field(&mut spec, &k, v, &mut prop_order)?;
                }
                orders.push((path.clone(), prop_order));
                if layer.specs.insert(path.clone(), spec).is_some() {
                    return Err(malformed(format!("two specs for {}", super::usd_stage::short(&path))));
                }
            }
            SPEC_ATTRIBUTE | SPEC_RELATIONSHIP => {
                let (prim, name) = super::usd_stage::split_property(&path).ok_or_else(|| malformed(format!("property spec at a prim path {}", super::usd_stage::short(&path))))?;
                props.push((prim.to_string(), name.to_string(), spec_type, fields_here));
            }
            SPEC_VARIANT_SET => {
                // `/Prim{set=}` lists the set's variants.
                let Some(open) = path.rfind('{') else { continue };
                let prim = path[..open].to_string();
                let set = path[open + 1..].trim_end_matches('}').trim_end_matches('=').to_string();
                let variants = fields_here.iter().find(|(k, _)| k == "variantChildren").map(|(_, v)| strs(v)).unwrap_or_default();
                variant_sets.push((prim, set, variants));
            }
            // Connection and relationship-target specs, expressions, mappers:
            // nothing a scene reads.
            _ => {}
        }
    }
    for (prim, name, kind, fields_here) in props {
        let Some(spec) = layer.specs.get_mut(&prim) else { continue };
        let prop = if kind == SPEC_ATTRIBUTE {
            super::usd_stage::Prop::Attr(attr_from_fields(fields_here)?)
        } else {
            let mut r = RelSpec::default();
            for (k, v) in fields_here {
                if k == "targetPaths" {
                    if let FieldValue::PathOp(op) = path_op(v) {
                        r.targets = Some(op);
                    }
                }
            }
            super::usd_stage::Prop::Rel(r)
        };
        if spec.prop(&name).is_some() {
            return Err(malformed(format!("two specs for property {}", super::usd_stage::short(&name))));
        }
        spec.props.push((name, prop));
    }
    for (path, order) in orders {
        if let Some(spec) = layer.specs.get_mut(&path) {
            let rank = |n: &str| order.iter().position(|o| o == n).unwrap_or(usize::MAX);
            spec.props.sort_by_key(|(n, _)| rank(n));
        }
    }
    for (prim, set, variants) in variant_sets {
        if let Some(spec) = layer.specs.get_mut(&prim) {
            spec.variant_sets.push((set, variants));
        }
    }
    if !unread.is_empty() {
        layer.warnings.push(format!(
            "layer {}: fields holding values of crate type(s) {} were skipped (a newer USD than this reader wrote them)",
            super::usd_stage::short(id),
            unread.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    Ok(layer)
}

fn strs(v: &Value) -> Vec<String> {
    match v {
        Value::Strs(s) | Value::Tokens(s) | Value::Assets(s) => s.clone(),
        Value::Str(s) | Value::Token(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// Field values that are list ops come back as `Value::List` of tagged
/// entries; this unwraps them.
enum FieldValue {
    PathOp(ListOp<String>),
    Other,
}

fn path_op(v: Value) -> FieldValue {
    match list_op_strings(&v) {
        Some(op) => FieldValue::PathOp(op),
        None => FieldValue::Other,
    }
}

/// A list op is encoded as `Dict` with keys explicit/add/prepend/append/delete
/// (see `Crate::list_op`).
fn list_op_strings(v: &Value) -> Option<ListOp<String>> {
    let d = v.dict()?;
    let mut op = ListOp::default();
    let mut any = false;
    for (k, items) in d {
        let items: Vec<String> = match items {
            Value::Strs(s) => s.clone(),
            _ => continue,
        };
        any = true;
        match k.as_str() {
            "explicit" => op.explicit = Some(items),
            "add" => op.add = items,
            "prepend" => op.prepend = items,
            "append" => op.append = items,
            "delete" => op.delete = items,
            _ => {}
        }
    }
    any.then_some(op)
}

fn list_op_refs(v: &Value) -> Option<ListOp<Reference>> {
    let d = v.dict()?;
    let mut op = ListOp::default();
    for (k, items) in d {
        let Value::List(items) = items else { continue };
        let refs: Vec<Reference> = items
            .iter()
            .filter_map(|it| {
                let t = it.dict()?;
                let get = |key: &str| t.iter().find(|(k, _)| k == key).map(|(_, v)| v);
                Some(Reference {
                    asset: get("asset").and_then(Value::as_str).unwrap_or("").to_string(),
                    prim: get("prim").and_then(Value::as_str).unwrap_or("").to_string(),
                    offset: get("offset").and_then(Value::as_f64).unwrap_or(0.0),
                    scale: get("scale").and_then(Value::as_f64).unwrap_or(1.0),
                })
            })
            .collect();
        match k.as_str() {
            "explicit" => op.explicit = Some(refs),
            "add" => op.add = refs,
            "prepend" => op.prepend = refs,
            "append" => op.append = refs,
            "delete" => op.delete = refs,
            _ => {}
        }
    }
    Some(op)
}

fn apply_prim_field(spec: &mut PrimSpec, k: &str, v: Value, prop_order: &mut Vec<String>) -> Result<(), MeshError> {
    match k {
        "specifier" => {
            spec.specifier = match v.as_i64() {
                Some(0) => Specifier::Def,
                Some(1) => Specifier::Over,
                Some(2) => Specifier::Class,
                _ => return Err(malformed("an unknown specifier")),
            }
        }
        "typeName" => spec.type_name = v.as_str().unwrap_or("").to_string(),
        "primChildren" => spec.children = strs(&v),
        "properties" => *prop_order = strs(&v),
        "apiSchemas" => spec.api_schemas = list_op_strings(&v),
        // The text format's `inherits` is Sdf's `inheritPaths` field.
        "inheritPaths" | "inherits" => spec.inherits = list_op_strings(&v),
        "specializes" => spec.specializes = list_op_strings(&v),
        "variantSetNames" => spec.variant_set_names = list_op_strings(&v),
        "references" => spec.references = list_op_refs(&v),
        "payload" => {
            spec.payloads = match &v {
                // A single payload (crate < 0.8 writes one, not a list op).
                Value::List(items) if items.len() == 1 && items[0].dict().is_some_and(|d| d.iter().any(|(k, _)| k == "asset")) => {
                    list_op_refs(&Value::Dict(vec![("explicit".into(), Value::List(items.clone()))]))
                }
                _ => list_op_refs(&v),
            }
        }
        "variantSelection" => {
            if let Value::Dict(d) = v {
                spec.variant_selection = d.into_iter().filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string()))).collect();
            }
        }
        "active" => spec.active = v.as_bool(),
        "kind" => spec.kind = v.as_str().map(str::to_string),
        "customData" => {
            if let Value::Dict(d) = v {
                spec.custom_data = Some(d);
            }
        }
        _ => {}
    }
    Ok(())
}

fn attr_from_fields(fields: Vec<Field>) -> Result<super::usd_stage::AttrSpec, MeshError> {
    let mut a = super::usd_stage::AttrSpec::default();
    for (k, v) in fields {
        match k.as_str() {
            "typeName" => a.type_name = v.as_str().unwrap_or("").to_string(),
            "default" => a.default = Some(v),
            "timeSamples" => {
                if let Value::List(items) = v {
                    let mut samples = Vec::with_capacity(items.len());
                    for it in items {
                        if let Value::List(mut pair) = it {
                            if pair.len() == 2 {
                                let val = pair.pop().unwrap_or(Value::Block);
                                let t = pair.pop().and_then(|t| t.as_f64()).unwrap_or(f64::NAN);
                                if !t.is_finite() {
                                    return Err(malformed("a time sample at a non-finite time"));
                                }
                                samples.push((t, val));
                            }
                        }
                    }
                    samples.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                    a.samples = Some(samples);
                }
            }
            "connectionPaths" => a.connections = list_op_strings(&v),
            "variability" => {
                a.variability = match v.as_i64() {
                    Some(0) => Variability::Varying,
                    Some(1) => Variability::Uniform,
                    Some(2) => Variability::Config,
                    _ => return Err(malformed("an unknown variability")),
                }
            }
            "interpolation" => a.interpolation = v.as_str().map(str::to_string),
            "elementSize" => a.element_size = v.as_i64(),
            // The text reader drops `custom` too: nothing reads it.
            "custom" => {}
            _ => a.metadata.push((k, v)),
        }
    }
    // Quaternion attributes are stored x, y, z, w — already the stage's order.
    Ok(a)
}

/// The PATHS table: the `count` slots the file declares, holding only the
/// paths its encoded entries name. Stored by entry rather than by slot: the
/// count is a number nothing in the file backs (OpenUSD's own `comp.usdc`
/// declares 56 and encodes 55), and a 2 KB file declaring 64Mi slots sized a
/// 1.6 GB table (2026-10-12 review, finding 8).
#[derive(Debug, Default)]
struct PathTable {
    count: usize,
    slots: HashMap<usize, String>,
}

impl PathTable {
    fn len(&self) -> usize {
        self.count
    }

    /// `None` past the declared table; `Some(None)` for a slot no entry fills.
    fn get(&self, i: usize) -> Option<Option<&String>> {
        (i < self.count).then(|| self.slots.get(&i))
    }
}

/// Decode the PATHS tree: a pre-order walk where each entry carries its
/// element token and a jump saying whether a child, a sibling or both follow.
fn build_paths(tokens: &[String], total: usize, idx: &[i64], elem: &[i64], jumps: &[i64]) -> Result<PathTable, MeshError> {
    let mut paths = PathTable { count: total, slots: HashMap::with_capacity(idx.len()) };
    if idx.is_empty() {
        return Ok(paths);
    }
    let mut visited = vec![false; idx.len()];
    // (entry to read next, parent path of that entry)
    let mut stack: Vec<(usize, Option<String>)> = vec![(0, None)];
    while let Some((mut cur, mut parent)) = stack.pop() {
        loop {
            let this = cur;
            cur += 1;
            if this >= idx.len() {
                return Err(malformed("the path tree jumps past its end"));
            }
            if std::mem::replace(&mut visited[this], true) {
                return Err(malformed("the path tree revisits an entry"));
            }
            let path = match &parent {
                None => "/".to_string(),
                Some(p) => {
                    let e = elem[this];
                    let tok = tokens.get(e.unsigned_abs() as usize).ok_or_else(|| malformed("a path element names a token that does not exist"))?;
                    let s = if e < 0 { format!("{p}.{tok}") } else { super::usd_stage::child_path(p, tok) };
                    if s.len() > MAX_PATH_LEN {
                        return Err(malformed("a path is longer than any real hierarchy"));
                    }
                    s
                }
            };
            let slot = usize::try_from(idx[this]).ok().filter(|i| *i < total).ok_or_else(|| malformed("a path index is out of range"))?;
            paths.slots.insert(slot, path.clone());
            let j = jumps[this];
            let has_child = j > 0 || j == -1;
            let has_sibling = j >= 0;
            if has_child {
                if has_sibling {
                    let sib = usize::try_from(j).ok().and_then(|j| this.checked_add(j)).ok_or_else(|| malformed("a path jump overflows"))?;
                    stack.push((sib, parent.clone()));
                }
                parent = Some(path);
            }
            if !(has_child || has_sibling) {
                break;
            }
        }
    }
    Ok(paths)
}

impl<'a> Crate<'a> {
    fn charge(&mut self, n: usize) -> Result<(), MeshError> {
        self.values = self.values.saturating_add(n);
        if self.values > MAX_VALUES {
            return Err(unsupported(format!("the layer holds more than {MAX_VALUES} values")));
        }
        Ok(())
    }

    fn token(&self, i: u64) -> Result<String, MeshError> {
        self.tokens.get(i as usize).cloned().ok_or_else(|| malformed("a value names a token that does not exist"))
    }

    fn string(&self, i: u64) -> Result<String, MeshError> {
        let t = *self.strings.get(i as usize).ok_or_else(|| malformed("a value names a string that does not exist"))?;
        self.token(t as u64)
    }

    fn path(&self, i: u64) -> Result<String, MeshError> {
        self.paths.get(i as usize).flatten().cloned().ok_or_else(|| malformed("a value names a path that does not exist"))
    }

    fn at(&self, payload: u64) -> Result<Cursor<'a>, MeshError> {
        let p = usize::try_from(payload).ok().filter(|p| *p < self.b.len()).ok_or_else(|| malformed("a value points outside the file"))?;
        Ok(Cursor::at(self.b, p))
    }

    /// Decode a `ValueRep`.
    fn value(&mut self, rep: u64, depth: usize) -> Result<Value, MeshError> {
        if depth > MAX_VALUE_DEPTH {
            return Err(malformed("values nest deeper than any real file"));
        }
        let is_array = rep >> 63 & 1 == 1;
        let inlined = rep >> 62 & 1 == 1;
        let compressed = rep >> 61 & 1 == 1;
        let array_edit = rep >> 60 & 1 == 1;
        let t = ((rep >> 48) & 0xFF) as u8;
        let payload = rep & ((1u64 << 48) - 1);
        if array_edit {
            return Err(unsupported("an array-edit value (crate 0.14), which is not read"));
        }
        if t == ty::TIME_SAMPLES {
            return self.time_samples(payload, depth);
        }
        if is_array {
            return self.array(t, payload, compressed);
        }
        if inlined {
            return self.inlined(t, (payload & 0xFFFF_FFFF) as u32);
        }
        let mut c = self.at(payload)?;
        self.scalar(&mut c, t, depth)
    }

    fn inlined(&mut self, t: u8, v: u32) -> Result<Value, MeshError> {
        let b = v.to_le_bytes();
        Ok(match t {
            ty::BOOL => Value::Bool(v != 0),
            ty::UCHAR => Value::Int((v & 0xFF) as i64),
            ty::INT | ty::INT64 => Value::Int(v as i32 as i64),
            ty::UINT | ty::UINT64 => Value::Int(v as i64),
            // Doubles that are exactly a float are stored as that float.
            ty::FLOAT | ty::DOUBLE | ty::TIME_CODE => Value::Float(f32::from_bits(v) as f64),
            ty::HALF => Value::Float(half_to_f32(v as u16) as f64),
            ty::TOKEN => Value::Token(self.token(v as u64)?),
            ty::STRING => Value::Str(self.string(v as u64)?),
            ty::ASSET_PATH => Value::Asset(self.token(v as u64)?),
            ty::SPECIFIER | ty::PERMISSION | ty::VARIABILITY => Value::Int(v as i64),
            ty::VEC2H => Value::Tuple(vec![half_to_f32(u16::from_le_bytes([b[0], b[1]])) as f64, half_to_f32(u16::from_le_bytes([b[2], b[3]])) as f64]),
            // Larger vectors with all-int8 components inline those components.
            ty::VEC2D | ty::VEC2F | ty::VEC2I => Value::Tuple(b[..2].iter().map(|x| *x as i8 as f64).collect()),
            ty::VEC3D | ty::VEC3F | ty::VEC3H | ty::VEC3I => Value::Tuple(b[..3].iter().map(|x| *x as i8 as f64).collect()),
            ty::VEC4D | ty::VEC4F | ty::VEC4H | ty::VEC4I => Value::Tuple(b[..4].iter().map(|x| *x as i8 as f64).collect()),
            // Diagonal matrices inline their int8 diagonal.
            ty::MATRIX2D | ty::MATRIX3D | ty::MATRIX4D => {
                let n = match t {
                    ty::MATRIX2D => 2,
                    ty::MATRIX3D => 3,
                    _ => 4,
                };
                let mut m = vec![0.0f64; n * n];
                for i in 0..n {
                    m[i * n + i] = b[i] as i8 as f64;
                }
                Value::Tuple(m)
            }
            ty::DICTIONARY => Value::Dict(Vec::new()),
            ty::VALUE_BLOCK => Value::Block,
            other => return Err(unsupported(format!("inlined value of type {other}"))),
        })
    }

    fn numeric_scalar(&mut self, c: &mut Cursor<'_>, t: u8) -> Result<Option<Value>, MeshError> {
        let Some((w, size, kind)) = numeric_layout(t) else { return Ok(None) };
        let s = c.take(w * size)?;
        if w == 1 {
            return Ok(Some(match kind {
                b'i' | b'u' if t != ty::BOOL => Value::Int(component(s, size, kind) as i64),
                _ if t == ty::BOOL => Value::Bool(s[0] != 0),
                _ => Value::Float(component(s, size, kind)),
            }));
        }
        let v: Vec<f64> = (0..w).map(|i| component(&s[i * size..], size, kind)).collect();
        Ok(Some(Value::Tuple(v)))
    }

    fn scalar(&mut self, c: &mut Cursor<'_>, t: u8, depth: usize) -> Result<Value, MeshError> {
        if let Some(v) = self.numeric_scalar(c, t)? {
            return Ok(v);
        }
        Ok(match t {
            ty::TOKEN => Value::Token(self.token(c.u32()? as u64)?),
            ty::STRING => Value::Str(self.string(c.u32()? as u64)?),
            ty::ASSET_PATH => Value::Asset(self.string(c.u32()? as u64)?),
            ty::SPECIFIER | ty::PERMISSION | ty::VARIABILITY => Value::Int(c.i32()? as i64),
            ty::TOKEN_VECTOR => {
                let n = c.count(4, "token vector")?;
                self.charge(n)?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.token(c.u32()? as u64)?);
                }
                Value::Tokens(v)
            }
            ty::STRING_VECTOR => {
                let n = c.count(4, "string vector")?;
                self.charge(n)?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.string(c.u32()? as u64)?);
                }
                Value::Strs(v)
            }
            ty::PATH_VECTOR => {
                let n = c.count(4, "path vector")?;
                self.charge(n)?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.path(c.u32()? as u64)?);
                }
                Value::Strs(v)
            }
            ty::DOUBLE_VECTOR => {
                let n = c.count(8, "double vector")?;
                self.charge(n)?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(c.f64()?);
                }
                Value::Doubles(v)
            }
            ty::LAYER_OFFSET_VECTOR => {
                let n = c.count(16, "layer offsets")?;
                self.charge(n * 2)?;
                let mut v = Vec::with_capacity(n * 2);
                for _ in 0..n * 2 {
                    v.push(c.f64()?);
                }
                Value::Tuple(v)
            }
            ty::VARIANT_SELECTION_MAP => {
                let n = c.count(8, "variant selections")?;
                self.charge(n)?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    let k = self.string(c.u32()? as u64)?;
                    let s = self.string(c.u32()? as u64)?;
                    v.push((k, Value::Str(s)));
                }
                Value::Dict(v)
            }
            ty::DICTIONARY => self.dictionary(c, depth)?,
            ty::TOKEN_LIST_OP | ty::STRING_LIST_OP | ty::PATH_LIST_OP => self.list_op(c, t)?,
            ty::REFERENCE_LIST_OP | ty::PAYLOAD_LIST_OP => self.ref_list_op(c, t, depth)?,
            ty::INT_LIST_OP | ty::INT64_LIST_OP | ty::UINT_LIST_OP | ty::UINT64_LIST_OP => {
                // Integer list ops (no scene meaning): consume nothing more.
                Value::Block
            }
            ty::PAYLOAD => {
                let asset = self.string(c.u32()? as u64)?;
                let prim = self.path_or_empty(c.u32()?)?;
                let (offset, scale) = if self.minor >= 8 { (c.f64()?, c.f64()?) } else { (0.0, 1.0) };
                Value::List(vec![reference_value(asset, prim, offset, scale)])
            }
            ty::VALUE_BLOCK => Value::Block,
            ty::VALUE => {
                // A VtValue: [i64 offset][...][ValueRep at start + offset].
                let start = c.pos;
                let off = c.i64()?;
                let at = (start as i64).checked_add(off).and_then(|p| usize::try_from(p).ok()).ok_or_else(|| malformed("a nested value offset is out of range"))?;
                let mut c2 = Cursor::at(self.b, at);
                let rep = c2.u64()?;
                self.value(rep, depth + 1)?
            }
            other => return Err(unsupported(format!("a value of crate type {other}, which is not read"))),
        })
    }

    /// Path index 0 names the empty path in list items that need none.
    fn path_or_empty(&self, i: u32) -> Result<String, MeshError> {
        match self.paths.get(i as usize) {
            Some(Some(p)) if p.as_str() != "/" => Ok(p.clone()),
            Some(_) => Ok(String::new()),
            None => Err(malformed("a value names a path that does not exist")),
        }
    }

    fn dictionary(&mut self, c: &mut Cursor<'_>, depth: usize) -> Result<Value, MeshError> {
        let n = c.count(12, "dictionary")?;
        self.charge(n)?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let key = self.string(c.u32()? as u64)?;
            // Each value is a VtValue: an offset to its ValueRep, after which
            // the next entry begins.
            let start = c.pos;
            let off = c.i64()?;
            let at = (start as i64).checked_add(off).and_then(|p| usize::try_from(p).ok()).ok_or_else(|| malformed("a dictionary value offset is out of range"))?;
            let mut rc = Cursor::at(self.b, at);
            let rep = rc.u64()?;
            let v = self.value(rep, depth + 1)?;
            c.pos = rc.pos;
            out.push((key, v));
        }
        Ok(Value::Dict(out))
    }

    /// Token, string and path list ops, as a `Dict` of item vectors keyed
    /// explicit/add/prepend/append/delete.
    fn list_op(&mut self, c: &mut Cursor<'_>, t: u8) -> Result<Value, MeshError> {
        let header = c.u8()?;
        let mut out: Vec<(String, Value)> = Vec::new();
        // Item vectors appear in this order when their bit is set.
        for (bit, name) in [(2u8, "explicit"), (4, "add"), (32, "prepend"), (64, "append"), (8, "delete"), (16, "reorder")] {
            if header & bit == 0 {
                continue;
            }
            let n = c.count(4, "list op")?;
            if n > MAX_LIST_OP_ITEMS {
                return Err(unsupported(format!("a list op of {n} items")));
            }
            self.charge(n)?;
            let mut items = Vec::with_capacity(n);
            for _ in 0..n {
                let i = c.u32()? as u64;
                items.push(match t {
                    ty::TOKEN_LIST_OP => self.token(i)?,
                    ty::STRING_LIST_OP => self.string(i)?,
                    _ => self.path(i)?,
                });
            }
            out.push((name.to_string(), Value::Strs(items)));
        }
        if header & 1 != 0 && !out.iter().any(|(k, _)| k == "explicit") {
            // Explicit with no items: an explicit empty list.
            out.push(("explicit".into(), Value::Strs(Vec::new())));
        }
        Ok(Value::Dict(out))
    }

    fn ref_list_op(&mut self, c: &mut Cursor<'_>, t: u8, depth: usize) -> Result<Value, MeshError> {
        let header = c.u8()?;
        let mut out: Vec<(String, Value)> = Vec::new();
        for (bit, name) in [(2u8, "explicit"), (4, "add"), (32, "prepend"), (64, "append"), (8, "delete"), (16, "reorder")] {
            if header & bit == 0 {
                continue;
            }
            let n = c.count(8, "reference list")?;
            if n > MAX_LIST_OP_ITEMS {
                return Err(unsupported(format!("a reference list of {n} items")));
            }
            self.charge(n)?;
            let mut items = Vec::with_capacity(n);
            for _ in 0..n {
                let asset = self.string(c.u32()? as u64)?;
                let prim = self.path_or_empty(c.u32()?)?;
                let (offset, scale) = if t == ty::PAYLOAD_LIST_OP && self.minor < 8 { (0.0, 1.0) } else { (c.f64()?, c.f64()?) };
                if t == ty::REFERENCE_LIST_OP {
                    // customData: a dictionary, skipped.
                    self.dictionary(c, depth + 1)?;
                }
                items.push(reference_value(asset, prim, offset, scale));
            }
            out.push((name.to_string(), Value::List(items)));
        }
        if header & 1 != 0 && !out.iter().any(|(k, _)| k == "explicit") {
            out.push(("explicit".into(), Value::List(Vec::new())));
        }
        Ok(Value::Dict(out))
    }

    fn time_samples(&mut self, payload: u64, depth: usize) -> Result<Value, MeshError> {
        let mut c = self.at(payload)?;
        let start = c.pos;
        let off = c.i64()?;
        let times_at = (start as i64).checked_add(off).and_then(|p| usize::try_from(p).ok()).ok_or_else(|| malformed("time samples point outside the file"))?;
        let mut c = Cursor::at(self.b, times_at);
        let times_rep = c.u64()?;
        let times = self.value(times_rep, depth + 1)?;
        let times: Vec<f64> = match times {
            Value::Doubles(t) => t,
            Value::Tuple(t) => t,
            Value::Float(f) => vec![f],
            _ => return Err(malformed("time sample times are not a list of doubles")),
        };
        let start = c.pos;
        let off = c.i64()?;
        let values_at = (start as i64).checked_add(off).and_then(|p| usize::try_from(p).ok()).ok_or_else(|| malformed("time sample values point outside the file"))?;
        let mut c = Cursor::at(self.b, values_at);
        let n = c.count(8, "time sample values")?;
        if n != times.len() {
            return Err(malformed(format!("{} sample times but {n} values", times.len())));
        }
        self.charge(n)?;
        let mut items = Vec::with_capacity(n);
        for t in times {
            let rep = c.u64()?;
            let v = self.value(rep, depth + 1)?;
            items.push(Value::List(vec![Value::Float(t), v]));
        }
        Ok(Value::List(items))
    }

    fn array(&mut self, t: u8, payload: u64, compressed: bool) -> Result<Value, MeshError> {
        if payload == 0 {
            return Ok(empty_array(t));
        }
        let mut c = self.at(payload)?;
        if self.minor < 5 {
            c.u32()?; // rank
        }
        let n = if self.minor < 7 { c.u32()? as usize } else {
            usize::try_from(c.u64()?).map_err(|_| malformed("an array count is absurd"))?
        };
        if n > MAX_VALUES {
            return Err(unsupported(format!("an array of {n} elements")));
        }
        // Integer arrays.
        if matches!(t, ty::INT | ty::UINT | ty::INT64 | ty::UINT64) {
            self.charge(n)?;
            let wide = matches!(t, ty::INT64 | ty::UINT64);
            if compressed && n >= MIN_COMPRESSED_ARRAY {
                let v = compressed_ints(&mut c, n, wide)?;
                return Ok(Value::Ints(v));
            }
        }
        // Float arrays: compressed as exact integers ('i') or through a
        // lookup table ('t').
        if matches!(t, ty::HALF | ty::FLOAT | ty::DOUBLE) && compressed && n >= MIN_COMPRESSED_ARRAY && self.minor >= 6 {
            self.charge(n)?;
            let code = c.u8()?;
            let vals: Vec<f64> = match code {
                b'i' => compressed_ints(&mut c, n, false)?.into_iter().map(|i| i as f64).collect(),
                b't' => {
                    let lut_n = c.u32()? as usize;
                    let (_, size, kind) = numeric_layout(t).unwrap_or((1, 4, b'f'));
                    let raw = c.take(lut_n.checked_mul(size).ok_or_else(|| malformed("a lookup table overflows"))?)?;
                    let lut: Vec<f64> = (0..lut_n).map(|i| component(&raw[i * size..], size, kind)).collect();
                    let ids = compressed_ints(&mut c, n, false)?;
                    ids.into_iter()
                        .map(|i| lut.get(i as usize).copied().ok_or_else(|| malformed("a lookup index is out of range")))
                        .collect::<Result<_, _>>()?
                }
                other => return Err(malformed(format!("unknown float array compression {other:#x}"))),
            };
            return Ok(if t == ty::DOUBLE { Value::Doubles(vals) } else { Value::Floats(vals.into_iter().map(|v| v as f32).collect()) });
        }
        if let Some((w, size, kind)) = numeric_layout(t) {
            let total = n.checked_mul(w).ok_or_else(|| malformed("an array size overflows"))?;
            self.charge(total)?;
            let raw = c.take(total.checked_mul(size).ok_or_else(|| malformed("an array size overflows"))?)?;
            return Ok(match kind {
                b'f' | b'h' => Value::Floats((0..total).map(|i| component(&raw[i * size..], size, kind) as f32).collect()),
                b'd' => Value::Doubles((0..total).map(|i| component(&raw[i * size..], size, kind)).collect()),
                _ => Value::Ints((0..total).map(|i| component(&raw[i * size..], size, kind) as i64).collect()),
            });
        }
        match t {
            ty::TOKEN => {
                self.charge(n)?;
                let raw = c.take(n.checked_mul(4).ok_or_else(|| malformed("an array size overflows"))?)?;
                let v = raw.chunks_exact(4).map(|s| self.token(u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64)).collect::<Result<_, _>>()?;
                Ok(Value::Tokens(v))
            }
            ty::STRING | ty::ASSET_PATH => {
                self.charge(n)?;
                let raw = c.take(n.checked_mul(4).ok_or_else(|| malformed("an array size overflows"))?)?;
                let v = raw.chunks_exact(4).map(|s| self.string(u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64)).collect::<Result<_, _>>()?;
                Ok(if t == ty::ASSET_PATH { Value::Assets(v) } else { Value::Strs(v) })
            }
            other => Err(unsupported(format!("an array of crate type {other}, which is not read"))),
        }
    }
}

fn empty_array(t: u8) -> Value {
    match t {
        ty::TOKEN => Value::Tokens(Vec::new()),
        ty::STRING => Value::Strs(Vec::new()),
        ty::ASSET_PATH => Value::Assets(Vec::new()),
        ty::INT | ty::UINT | ty::INT64 | ty::UINT64 | ty::BOOL | ty::UCHAR => Value::Ints(Vec::new()),
        ty::DOUBLE | ty::VEC2D | ty::VEC3D | ty::VEC4D | ty::MATRIX2D | ty::MATRIX3D | ty::MATRIX4D | ty::QUATD => {
            Value::Doubles(Vec::new())
        }
        _ => Value::Floats(Vec::new()),
    }
}

fn reference_value(asset: String, prim: String, offset: f64, scale: f64) -> Value {
    Value::Dict(vec![
        ("asset".into(), Value::Str(asset)),
        ("prim".into(), Value::Str(prim)),
        ("offset".into(), Value::Float(offset)),
        ("scale".into(), Value::Float(scale)),
    ])
}

// ─── writing: LZ4 and integer coding ────────────────────────────────────────

/// One LZ4 block that holds `src` as literals and nothing else: the token
/// (the literal count in its high nibble, 15 when more follow), the count's
/// extension bytes (255 each, then the remainder), and the bytes. A block may
/// end after its literals, so this is valid LZ4 that any decoder takes, with
/// no match finder behind it.
fn lz4_literals(src: &[u8], out: &mut Vec<u8>) {
    let n = src.len();
    if n < 15 {
        out.push((n as u8) << 4);
    } else {
        out.push(0xF0);
        let mut rest = n - 15;
        while rest >= 255 {
            out.push(255);
            rest -= 255;
        }
        out.push(rest as u8);
    }
    out.extend_from_slice(src);
}

/// `TfFastCompression` of `src`, in literal-only blocks: [`fast_decompress`]'s
/// inverse. A source that fits one block is a zero byte and the block; a
/// larger one is the chunk count, then each chunk's i32 size and block (what
/// `TfFastCompression::CompressToBuffer` writes, up to its 127 chunks).
fn fast_compress(src: &[u8]) -> Vec<u8> {
    fast_compress_chunked(src, LZ4_MAX_BLOCK)
}

/// [`fast_compress`] with the block size a parameter, so the several-chunk
/// framing is tested without a source of two gigabytes.
fn fast_compress_chunked(src: &[u8], max_block: usize) -> Vec<u8> {
    debug_assert!(max_block > 0 && src.len().div_ceil(max_block) <= 127, "more chunks than TfFastCompression frames");
    let mut out = Vec::with_capacity(src.len() + src.len() / 255 + 16);
    if src.len() <= max_block {
        out.push(0);
        lz4_literals(src, &mut out);
        return out;
    }
    out.push(src.len().div_ceil(max_block) as u8);
    for chunk in src.chunks(max_block) {
        let at = out.len();
        out.extend_from_slice(&[0; 4]);
        lz4_literals(chunk, &mut out);
        let size = (out.len() - at - 4) as i32;
        out[at..at + 4].copy_from_slice(&size.to_le_bytes());
    }
    out
}

/// `Sdf_IntegerCompression`'s coding, [`decode_ints`]'s inverse: each value as
/// its difference from the one before (in the width's own wrapping
/// arithmetic), then the most common difference, a 2-bit code for every value
/// (the common one, or an 8-, 16- or 32-bit integer: 16, 32 or 64 when
/// `wide`), then the odd-sized differences. The common difference is the
/// first to reach the highest count, a tie going to the larger, as OpenUSD's
/// does. Nothing codes to nothing.
fn encode_ints(values: &[i64], wide: bool) -> Vec<u8> {
    if values.is_empty() {
        return Vec::new();
    }
    let mut deltas = Vec::with_capacity(values.len());
    let mut prev = 0i64;
    for &v in values {
        deltas.push(if wide { v.wrapping_sub(prev) } else { (v as i32).wrapping_sub(prev as i32) as i64 });
        prev = v;
    }
    let mut counts: HashMap<i64, usize> = HashMap::new();
    let (mut common, mut best) = (0i64, 0usize);
    for &d in &deltas {
        let count = counts.entry(d).or_insert(0);
        *count += 1;
        if *count > best || (*count == best && d > common) {
            (common, best) = (d, *count);
        }
    }
    let mut codes = vec![0u8; (values.len() * 2).div_ceil(8)];
    let mut data = Vec::new();
    for (i, &d) in deltas.iter().enumerate() {
        let code = if d == common {
            0
        } else if wide {
            if let Ok(x) = i16::try_from(d) {
                data.extend_from_slice(&x.to_le_bytes());
                1
            } else if let Ok(x) = i32::try_from(d) {
                data.extend_from_slice(&x.to_le_bytes());
                2
            } else {
                data.extend_from_slice(&d.to_le_bytes());
                3
            }
        } else if let Ok(x) = i8::try_from(d) {
            data.push(x as u8);
            1
        } else if let Ok(x) = i16::try_from(d) {
            data.extend_from_slice(&x.to_le_bytes());
            2
        } else {
            data.extend_from_slice(&(d as i32).to_le_bytes());
            3
        };
        codes[i / 4] |= code << (2 * (i % 4));
    }
    let mut out = if wide { common.to_le_bytes().to_vec() } else { (common as i32).to_le_bytes().to_vec() };
    out.extend_from_slice(&codes);
    out.extend_from_slice(&data);
    out
}

// ─── writing: the file ──────────────────────────────────────────────────────
//
// The layer, as `usdcat` would encode it. Where the reader above is the
// inverse of a step, the step is named; the layout of each spec kind is what
// Pixar's writer made of `stress`, `comp`, `typed` and our own fixtures (the
// pairs in `testdata/usd/`), not what the format would also allow.

/// The crate version written. Nothing litegen writes needs a later one:
/// timecodes are 0.9.0, path expressions 0.10.0, splines 0.12.0.
const WRITE_VERSION: [u8; 3] = [0, 8, 0];
/// The first token of every crate `usdcat` writes. No other token is index 0,
/// so a negated index in the path table is never "negative zero".
const TOKEN_ZERO: &str = ";-)";
/// `ValueRep`'s bits: an array, and a value held in the payload itself. The
/// compressed bit stays clear: arrays here are uncompressed.
const REP_ARRAY: u64 = 1 << 63;
const REP_INLINED: u64 = 1 << 62;
/// Tokens' bytes the reader reads at most (`parse`'s limit on the token table).
const MAX_TOKEN_BYTES: usize = 1 << 28;
/// Fields the crate keeps for itself in an attribute's field set: metadata
/// under any of these names would be read back as the field.
const ATTR_FIELDS: [&str; 8] = ["custom", "typeName", "variability", "default", "connectionPaths", "interpolation", "elementSize", "timeSamples"];
/// Fields of the pseudo-root the reader takes for its own.
const ROOT_FIELDS: [&str; 3] = ["subLayers", "subLayerOffsets", "primChildren"];

/// A value held in its `ValueRep`'s payload.
fn inline(t: u8, v: u32) -> u64 {
    REP_INLINED | (u64::from(t) << 48) | u64::from(v)
}

/// A value at file offset `at`.
fn out_of_line(t: u8, at: usize) -> u64 {
    (u64::from(t) << 48) | at as u64
}

/// An array at file offset `at` (0 for an empty one).
fn array_at(t: u8, at: usize) -> u64 {
    REP_ARRAY | (u64::from(t) << 48) | at as u64
}

/// The variant of `v`, for a refusal's message: what the layer holds, in the
/// words the reader's `Value` uses. A tuple's width is part of it, since a
/// width is what tells a vector, a quaternion and a matrix apart.
fn kind(v: &Value) -> String {
    match v {
        Value::Bool(_) => "Bool".into(),
        Value::Int(_) => "Int".into(),
        Value::Float(_) => "Float".into(),
        Value::Str(_) => "Str".into(),
        Value::Token(_) => "Token".into(),
        Value::Asset(_) => "Asset".into(),
        Value::Path(_) => "Path".into(),
        Value::Tuple(t) => format!("Tuple({})", t.len()),
        Value::Ints(_) => "Ints".into(),
        Value::Floats(_) => "Floats".into(),
        Value::Doubles(_) => "Doubles".into(),
        Value::Strs(_) => "Strs".into(),
        Value::Tokens(_) => "Tokens".into(),
        Value::Assets(_) => "Assets".into(),
        Value::Dict(_) => "Dict".into(),
        Value::List(_) => "List".into(),
        Value::Block => "Block".into(),
    }
}

fn cannot(type_name: &str, v: &Value) -> MeshError {
    unsupported(format!("a {} value cannot be written as {type_name}", kind(v)))
}

/// The crate type an attribute's declared type is stored as (the role types,
/// `point3f` and the like, are the vector underneath), and whether it is an
/// array. Any other type is refused by name: a crate cannot hold it, or this
/// writer does not know how.
fn crate_type(type_name: &str) -> Result<(u8, bool), MeshError> {
    let (scalar, array) = match type_name.strip_suffix("[]") {
        Some(s) => (s, true),
        None => (type_name, false),
    };
    let t = match scalar {
        "bool" => ty::BOOL,
        "uchar" => ty::UCHAR,
        "int" => ty::INT,
        "uint" => ty::UINT,
        "int64" => ty::INT64,
        "uint64" => ty::UINT64,
        "half" => ty::HALF,
        "float" => ty::FLOAT,
        "double" => ty::DOUBLE,
        "string" => ty::STRING,
        "token" => ty::TOKEN,
        "asset" => ty::ASSET_PATH,
        "matrix2d" => ty::MATRIX2D,
        "matrix3d" => ty::MATRIX3D,
        "matrix4d" | "frame4d" => ty::MATRIX4D,
        "quatd" => ty::QUATD,
        "quatf" => ty::QUATF,
        "quath" => ty::QUATH,
        "int2" => ty::VEC2I,
        "int3" => ty::VEC3I,
        "int4" => ty::VEC4I,
        "half2" | "texCoord2h" => ty::VEC2H,
        "half3" | "point3h" | "vector3h" | "normal3h" | "color3h" | "texCoord3h" => ty::VEC3H,
        "half4" | "color4h" => ty::VEC4H,
        "float2" | "texCoord2f" => ty::VEC2F,
        "float3" | "point3f" | "vector3f" | "normal3f" | "color3f" | "texCoord3f" => ty::VEC3F,
        "float4" | "color4f" => ty::VEC4F,
        "double2" | "texCoord2d" => ty::VEC2D,
        "double3" | "point3d" | "vector3d" | "normal3d" | "color3d" | "texCoord3d" => ty::VEC3D,
        "double4" | "color4d" => ty::VEC4D,
        "timecode" => return Err(unsupported(format!("{type_name} values need crate version 0.9.0, and this writer writes 0.8.0"))),
        _ => return Err(unsupported(format!("the attribute type {type_name} has no crate type this writer knows"))),
    };
    Ok((t, array))
}

/// `x` as a half, rounded to nearest. A finite `x` that rounds past the
/// largest half (65,504, so from 65,520 on) is refused, as an integer that
/// does not fit its type is: the crate would hold an infinity the layer
/// never said. An infinity or a NaN the layer says is kept.
fn half_of(x: f64, type_name: &str) -> Result<u16, MeshError> {
    let h = f32_to_half(x as f32);
    if h & 0x7FFF == 0x7C00 && x.is_finite() {
        return Err(past_range(x, type_name));
    }
    Ok(h)
}

/// `x` as a float, rounded to nearest, refused as [`half_of`] refuses when
/// it is finite and rounds past f32::MAX.
fn float_of(x: f64, type_name: &str) -> Result<f32, MeshError> {
    let f = x as f32;
    if f.is_infinite() && x.is_finite() {
        return Err(past_range(x, type_name));
    }
    Ok(f)
}

fn past_range(x: f64, type_name: &str) -> MeshError {
    unsupported(format!("{x} is past the range of {type_name}, which would hold it as an infinity"))
}

/// `x` as an int8, when it is one exactly: what the crate inlines a vector's
/// or a matrix's components as.
fn int8_exact(x: f64) -> Option<u8> {
    (x.fract() == 0.0 && (-128.0..=127.0).contains(&x)).then_some(x as i8 as u8)
}

/// A component in the type's own encoding: `size` bytes of `kind` (the reader's
/// `numeric_layout`).
fn put_component(out: &mut Vec<u8>, size: usize, kind: u8, x: f64, type_name: &str) -> Result<(), MeshError> {
    match (size, kind) {
        (2, b'h') => out.extend_from_slice(&half_of(x, type_name)?.to_le_bytes()),
        (4, b'f') => out.extend_from_slice(&float_of(x, type_name)?.to_le_bytes()),
        (8, b'd') => out.extend_from_slice(&x.to_le_bytes()),
        (4, b'i') if x.fract() == 0.0 && (i32::MIN as f64..=i32::MAX as f64).contains(&x) => out.extend_from_slice(&(x as i32).to_le_bytes()),
        _ => return Err(unsupported(format!("{x} does not fit a component of {type_name}"))),
    }
    Ok(())
}

/// An integer in a `t`-typed array's encoding, or `None` when it does not fit.
fn put_int(out: &mut Vec<u8>, t: u8, size: usize, kind: u8, x: i64) -> Option<()> {
    match (t, size, kind) {
        (ty::BOOL, ..) => out.push(u8::try_from(x).ok().filter(|b| *b <= 1)?),
        (_, 1, _) => out.push(u8::try_from(x).ok()?),
        (_, 4, b'i') => out.extend_from_slice(&i32::try_from(x).ok()?.to_le_bytes()),
        (_, 4, _) => out.extend_from_slice(&u32::try_from(x).ok()?.to_le_bytes()),
        (_, 8, b'i') => out.extend_from_slice(&x.to_le_bytes()),
        (_, 8, _) => out.extend_from_slice(&u64::try_from(x).ok()?.to_le_bytes()),
        _ => return None,
    }
    Some(())
}

/// A node of the path table: one path element under its parent.
struct PathNode {
    /// The element's token: a prim's name, a variant selection's `{set=name}`, a property's name.
    token: u32,
    property: bool,
    /// Its children in the order they were added. The empty path has a slot and is nobody's child, so the tree walk never reaches it.
    children: Vec<u32>,
}

/// The elements of a path in the form this crate writes them in: `(end of
/// the element in the path, its token, whether it names a property)`. A
/// prim's name follows a `/`, or follows a variant selection's `}` with no
/// separator (`/A{v=x}B`); a property follows a `.`. Any other shape (a
/// relationship target `.rel[/B]`, a mapper) is not one this writer writes.
fn path_elements(path: &str) -> Result<Vec<(usize, &str, bool)>, MeshError> {
    let bad = || unsupported(format!("the path {} is not one this writer keeps", super::usd_stage::short(path)));
    let b = path.as_bytes();
    if b.first() != Some(&b'/') {
        return Err(bad());
    }
    let mut out = Vec::new();
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            b'{' => {
                let close = path[i..].find('}').ok_or_else(bad)? + i;
                out.push((close + 1, &path[i..=close], false));
                i = close + 1;
                if b.get(i) == Some(&b'/') {
                    return Err(bad());
                }
            }
            b'.' => {
                let name = &path[i + 1..];
                if name.is_empty() || name.contains(['/', '.', '{', '}', '[', ']']) {
                    return Err(bad());
                }
                out.push((path.len(), name, true));
                i = path.len();
            }
            b'}' | b'[' | b']' => return Err(bad()),
            _ => {
                let end = path[i..].find(['/', '.', '{', '}', '[', ']']).map_or(path.len(), |k| i + k);
                if end == i {
                    return Err(bad());
                }
                out.push((end, &path[i..end], false));
                i = end;
                if b.get(i) == Some(&b'/') {
                    i += 1;
                    if i == b.len() {
                        return Err(bad());
                    }
                }
            }
        }
    }
    Ok(out)
}

/// The crate being written: the file so far (the bootstrap's place, then each
/// value that lives out of line) and the tables its sections are made of.
/// Every table is a `Vec` in the order its entries were first needed, with a
/// map beside it that is only ever looked up, so no hash order reaches the
/// output.
struct Encoder {
    out: Vec<u8>,
    tokens: Vec<String>,
    token_ix: HashMap<String, u32>,
    /// STRINGS: the token each string is.
    strings: Vec<u32>,
    string_ix: HashMap<String, u32>,
    nodes: Vec<PathNode>,
    path_ix: HashMap<String, u32>,
    empty_path: Option<u32>,
    fields: Vec<(u32, u64)>,
    field_ix: HashMap<(u32, u64), u32>,
    /// FIELDSETS: each set is its field indices and a terminator.
    fieldsets: Vec<u32>,
    fieldset_ix: HashMap<Vec<u32>, u32>,
    /// SPECS: path, field set and spec type.
    specs: Vec<(u32, u32, i64)>,
    spec_paths: HashSet<u32>,
}

impl Encoder {
    fn new() -> Encoder {
        let mut e = Encoder {
            out: vec![0; BOOTSTRAP_LEN],
            tokens: vec![TOKEN_ZERO.to_string()],
            token_ix: HashMap::from([(TOKEN_ZERO.to_string(), 0)]),
            strings: Vec::new(),
            string_ix: HashMap::new(),
            nodes: Vec::new(),
            path_ix: HashMap::new(),
            empty_path: None,
            fields: Vec::new(),
            field_ix: HashMap::new(),
            fieldsets: Vec::new(),
            fieldset_ix: HashMap::new(),
            specs: Vec::new(),
            spec_paths: HashSet::new(),
        };
        // The absolute root is path 0, named by the empty token.
        let empty = e.tokens.len() as u32;
        e.tokens.push(String::new());
        e.token_ix.insert(String::new(), empty);
        e.nodes.push(PathNode { token: empty, property: false, children: Vec::new() });
        e.path_ix.insert("/".to_string(), 0);
        e
    }

    fn put_u32(&mut self, x: u32) {
        self.out.extend_from_slice(&x.to_le_bytes());
    }

    fn put_u64(&mut self, x: u64) {
        self.out.extend_from_slice(&x.to_le_bytes());
    }

    fn put_f64(&mut self, x: f64) {
        self.out.extend_from_slice(&x.to_le_bytes());
    }

    fn align8(&mut self) {
        while self.out.len() % 8 != 0 {
            self.out.push(0);
        }
    }

    fn check_table(&self, n: usize, what: &str) -> Result<(), MeshError> {
        if n >= MAX_TABLE {
            return Err(unsupported(format!("more {what} than the reader reads")));
        }
        Ok(())
    }

    fn token(&mut self, s: &str) -> Result<u32, MeshError> {
        if let Some(&i) = self.token_ix.get(s) {
            return Ok(i);
        }
        if s.contains('\0') {
            return Err(unsupported("a token or string holds a NUL byte, which a crate's NUL-separated token table cannot"));
        }
        self.check_table(self.tokens.len(), "tokens")?;
        let i = self.tokens.len() as u32;
        self.tokens.push(s.to_string());
        self.token_ix.insert(s.to_string(), i);
        Ok(i)
    }

    fn string(&mut self, s: &str) -> Result<u32, MeshError> {
        if let Some(&i) = self.string_ix.get(s) {
            return Ok(i);
        }
        let token = self.token(s)?;
        self.check_table(self.strings.len(), "strings")?;
        let i = self.strings.len() as u32;
        self.strings.push(token);
        self.string_ix.insert(s.to_string(), i);
        Ok(i)
    }

    /// The table index of `path`, adding it and every ancestor it needs
    /// (as `CrateFile::_AddPath` does). The empty path has a slot of its own and no
    /// place in the tree: a reference to a layer's default prim names it.
    fn path(&mut self, path: &str) -> Result<u32, MeshError> {
        if path.is_empty() {
            if let Some(i) = self.empty_path {
                return Ok(i);
            }
            let i = self.nodes.len() as u32;
            self.nodes.push(PathNode { token: 0, property: false, children: Vec::new() });
            self.empty_path = Some(i);
            return Ok(i);
        }
        if path.len() > MAX_PATH_LEN {
            return Err(unsupported(format!("the path {} is longer than the reader reads", super::usd_stage::short(path))));
        }
        if let Some(&i) = self.path_ix.get(path) {
            return Ok(i);
        }
        let mut cur = 0u32;
        for (end, name, property) in path_elements(path)? {
            let prefix = &path[..end];
            cur = match self.path_ix.get(prefix) {
                Some(&i) => i,
                None => {
                    let token = self.token(name)?;
                    if property && token == 0 {
                        return Err(unsupported(format!("a property named {TOKEN_ZERO}")));
                    }
                    self.check_table(self.nodes.len(), "paths")?;
                    let i = self.nodes.len() as u32;
                    self.nodes.push(PathNode { token, property, children: Vec::new() });
                    self.nodes[cur as usize].children.push(i);
                    self.path_ix.insert(prefix.to_string(), i);
                    i
                }
            };
        }
        Ok(cur)
    }

    fn field(&mut self, name: &str, rep: u64) -> Result<u32, MeshError> {
        let key = (self.token(name)?, rep);
        if let Some(&i) = self.field_ix.get(&key) {
            return Ok(i);
        }
        self.check_table(self.fields.len(), "fields")?;
        let i = self.fields.len() as u32;
        self.fields.push(key);
        self.field_ix.insert(key, i);
        Ok(i)
    }

    /// A spec: its path, its type and its fields, in order.
    fn spec(&mut self, path: &str, spec_type: i64, fields: Vec<(&str, u64)>) -> Result<(), MeshError> {
        let p = self.path(path)?;
        if !self.spec_paths.insert(p) {
            return Err(malformed(format!("two specs for {}", super::usd_stage::short(path))));
        }
        let ids = fields.into_iter().map(|(name, rep)| self.field(name, rep)).collect::<Result<Vec<u32>, MeshError>>()?;
        let set = match self.fieldset_ix.get(&ids) {
            Some(&s) => s,
            None => {
                self.check_table(self.fieldsets.len(), "field sets")?;
                let s = self.fieldsets.len() as u32;
                self.fieldsets.extend_from_slice(&ids);
                self.fieldsets.push(u32::MAX);
                self.fieldset_ix.insert(ids, s);
                s
            }
        };
        self.check_table(self.specs.len(), "specs")?;
        self.specs.push((p, set, spec_type));
        Ok(())
    }

    /// `write` after the layout of `_Writer::_RecursiveWrite`: room for the
    /// offset to where this value's own data ends, then whatever `f` writes
    /// (nested out-of-line data), then the offset filled in. The caller writes
    /// the value's `ValueRep` next, at that end.
    fn nested<R>(&mut self, f: impl FnOnce(&mut Encoder) -> Result<R, MeshError>) -> Result<R, MeshError> {
        let at = self.out.len();
        self.out.extend_from_slice(&[0; 8]);
        let r = f(self)?;
        let offset = (self.out.len() - at) as i64;
        self.out[at..at + 8].copy_from_slice(&offset.to_le_bytes());
        Ok(r)
    }

    // ── values ──────────────────────────────────────────────────────────────

    /// A value of a declared type: `t` and `array` are its `crate_type`.
    fn typed(&mut self, type_name: &str, t: u8, array: bool, v: &Value) -> Result<u64, MeshError> {
        if matches!(v, Value::Block) {
            return Ok(inline(ty::VALUE_BLOCK, 0));
        }
        match (t, array) {
            (ty::STRING | ty::TOKEN | ty::ASSET_PATH, false) => self.text_scalar(t, type_name, v),
            (ty::STRING | ty::TOKEN | ty::ASSET_PATH, true) => self.text_array(t, type_name, v),
            (_, false) => self.numeric_scalar(t, type_name, v),
            (_, true) => self.numeric_array(t, type_name, v),
        }
    }

    fn text_scalar(&mut self, t: u8, type_name: &str, v: &Value) -> Result<u64, MeshError> {
        Ok(match (t, v) {
            (ty::STRING, Value::Str(s)) => inline(t, self.string(s)?),
            (ty::TOKEN, Value::Token(s)) => inline(t, self.token(s)?),
            // A scalar asset path is its token; the array form keeps strings.
            (ty::ASSET_PATH, Value::Asset(s)) => inline(t, self.token(s)?),
            _ => return Err(cannot(type_name, v)),
        })
    }

    fn text_array(&mut self, t: u8, type_name: &str, v: &Value) -> Result<u64, MeshError> {
        let items = match (t, v) {
            (ty::STRING, Value::Strs(s)) | (ty::ASSET_PATH, Value::Assets(s)) => s,
            (ty::TOKEN, Value::Tokens(s)) => s,
            _ => return Err(cannot(type_name, v)),
        };
        if items.len() > MAX_VALUES {
            return Err(unsupported(format!("an array of {} elements", items.len())));
        }
        if items.is_empty() {
            return Ok(array_at(t, 0));
        }
        let mut indexes = Vec::with_capacity(items.len());
        for s in items {
            indexes.push(if t == ty::TOKEN { self.token(s)? } else { self.string(s)? });
        }
        self.align8();
        let at = self.out.len();
        self.put_u64(indexes.len() as u64);
        for i in indexes {
            self.put_u32(i);
        }
        Ok(array_at(t, at))
    }

    fn numeric_scalar(&mut self, t: u8, type_name: &str, v: &Value) -> Result<u64, MeshError> {
        let Some((w, size, kind)) = numeric_layout(t) else {
            return Err(unsupported(format!("{type_name} has no numeric layout")));
        };
        let bad = || cannot(type_name, v);
        if w == 1 {
            let int = |lo: i64, hi: i64| match v {
                Value::Int(i) if (lo..=hi).contains(i) => Ok(*i),
                _ => Err(bad()),
            };
            return Ok(match t {
                ty::BOOL => match v {
                    Value::Bool(b) => inline(t, u32::from(*b)),
                    _ => return Err(bad()),
                },
                ty::UCHAR => inline(t, int(0, 255)? as u32),
                ty::INT => inline(t, int(i64::from(i32::MIN), i64::from(i32::MAX))? as i32 as u32),
                ty::UINT => inline(t, int(0, i64::from(u32::MAX))? as u32),
                // Inlined when the integer fits 32 bits, as `_EncodeInline` does.
                ty::INT64 => {
                    let i = int(i64::MIN, i64::MAX)?;
                    match i32::try_from(i) {
                        Ok(small) => inline(t, small as u32),
                        Err(_) => self.eight(t, &i.to_le_bytes()),
                    }
                }
                ty::UINT64 => {
                    let i = int(0, i64::MAX)?;
                    match u32::try_from(i) {
                        Ok(small) => inline(t, small),
                        Err(_) => self.eight(t, &(i as u64).to_le_bytes()),
                    }
                }
                _ => {
                    let f = match v {
                        Value::Float(f) => *f,
                        Value::Int(i) => *i as f64,
                        _ => return Err(bad()),
                    };
                    match t {
                        // A half and a float are always inline; a double only when it is exactly a float.
                        ty::HALF => inline(t, u32::from(half_of(f, type_name)?)),
                        ty::FLOAT => inline(t, float_of(f, type_name)?.to_bits()),
                        ty::DOUBLE if f64::from(f as f32) == f => inline(t, (f as f32).to_bits()),
                        ty::DOUBLE => self.eight(t, &f.to_le_bytes()),
                        _ => return Err(unsupported(format!("{type_name} is no scalar this writer encodes"))),
                    }
                }
            });
        }
        let Value::Tuple(c) = v else { return Err(bad()) };
        if c.len() != w {
            return Err(bad());
        }
        // Two halves fill the four bytes: always inline.
        if t == ty::VEC2H {
            let half = |x: f64| half_of(x, type_name).map(u32::from);
            return Ok(inline(t, half(c[0])? | half(c[1])? << 16));
        }
        let matrix = matches!(t, ty::MATRIX2D | ty::MATRIX3D | ty::MATRIX4D);
        let quat = matches!(t, ty::QUATD | ty::QUATF | ty::QUATH);
        // A vector of int8s, or a matrix that is an int8 diagonal, is its bytes.
        let inlined: Option<Vec<u8>> = if quat {
            None
        } else if matrix {
            let n = if w == 4 { 2 } else if w == 9 { 3 } else { 4 };
            let off_diagonal_zero = c.iter().enumerate().all(|(k, x)| k / n == k % n || *x == 0.0);
            off_diagonal_zero.then(|| (0..n).map(|i| int8_exact(c[i * n + i])).collect::<Option<Vec<u8>>>()).flatten()
        } else {
            c.iter().map(|x| int8_exact(*x)).collect::<Option<Vec<u8>>>()
        };
        if let Some(mut bytes) = inlined {
            bytes.resize(4, 0);
            return Ok(inline(t, u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])));
        }
        let at = self.out.len();
        for x in c {
            put_component(&mut self.out, size, kind, *x, type_name)?;
        }
        Ok(out_of_line(t, at))
    }

    /// A scalar of eight bytes that did not fit its `ValueRep`.
    fn eight(&mut self, t: u8, bytes: &[u8; 8]) -> u64 {
        let at = self.out.len();
        self.out.extend_from_slice(bytes);
        out_of_line(t, at)
    }

    fn numeric_array(&mut self, t: u8, type_name: &str, v: &Value) -> Result<u64, MeshError> {
        let Some((w, size, kind)) = numeric_layout(t) else {
            return Err(unsupported(format!("{type_name} has no numeric layout")));
        };
        let total = match (kind, v) {
            (b'f' | b'h', Value::Floats(f)) => f.len(),
            (b'd', Value::Doubles(d)) => d.len(),
            (b'i' | b'u', Value::Ints(i)) => i.len(),
            _ => return Err(cannot(type_name, v)),
        };
        if total % w != 0 {
            return Err(unsupported(format!("an array of {total} numbers is not whole {type_name} elements")));
        }
        let n = total / w;
        if n > MAX_VALUES {
            return Err(unsupported(format!("an array of {n} elements")));
        }
        if n == 0 {
            return Ok(array_at(t, 0));
        }
        // Eight-aligned, so a reader may map the elements in place.
        self.align8();
        let at = self.out.len();
        self.put_u64(n as u64);
        self.out.reserve(total * size);
        match v {
            Value::Floats(f) => {
                for x in f {
                    put_component(&mut self.out, size, kind, f64::from(*x), type_name)?;
                }
            }
            Value::Doubles(d) => {
                for x in d {
                    put_component(&mut self.out, size, kind, *x, type_name)?;
                }
            }
            Value::Ints(ints) => {
                for x in ints {
                    put_int(&mut self.out, t, size, kind, *x).ok_or_else(|| unsupported(format!("{x} does not fit an element of {type_name}")))?;
                }
            }
            _ => return Err(cannot(type_name, v)),
        }
        Ok(array_at(t, at))
    }

    /// A value of no declared type (metadata, a dictionary's entry): its crate
    /// type is what its `Value` variant says. A variant that does not say one
    /// is refused by name, never guessed: a `Tuple(4)` may be a vector, a
    /// quaternion or a matrix, a `Path` has no crate type, a `List` is not typed.
    /// A float is a double and an integer an int, since the layer does not
    /// keep the difference; a flat array is of scalars.
    fn untyped(&mut self, v: &Value, depth: usize) -> Result<u64, MeshError> {
        if depth > MAX_VALUE_DEPTH {
            return Err(unsupported("values nest deeper than the reader reads"));
        }
        let label = kind(v);
        match v {
            Value::Bool(_) => self.typed(&label, ty::BOOL, false, v),
            Value::Int(i) if i32::try_from(*i).is_ok() => self.typed(&label, ty::INT, false, v),
            Value::Int(_) => self.typed(&label, ty::INT64, false, v),
            Value::Float(_) => self.typed(&label, ty::DOUBLE, false, v),
            Value::Str(_) => self.typed(&label, ty::STRING, false, v),
            Value::Token(_) => self.typed(&label, ty::TOKEN, false, v),
            Value::Asset(_) => self.typed(&label, ty::ASSET_PATH, false, v),
            Value::Tuple(t) => match t.len() {
                2 => self.typed(&label, ty::VEC2D, false, v),
                3 => self.typed(&label, ty::VEC3D, false, v),
                9 => self.typed(&label, ty::MATRIX3D, false, v),
                16 => self.typed(&label, ty::MATRIX4D, false, v),
                4 => Err(unsupported("a Tuple(4) with no declared type: it may be a vector, a quaternion or a 2x2 matrix, and the layer does not say which")),
                n => Err(unsupported(format!("a Tuple({n}) has no crate type"))),
            },
            Value::Ints(i) if i.iter().all(|x| i32::try_from(*x).is_ok()) => self.typed(&label, ty::INT, true, v),
            Value::Ints(_) => self.typed(&label, ty::INT64, true, v),
            Value::Floats(_) => self.typed(&label, ty::FLOAT, true, v),
            Value::Doubles(_) => self.typed(&label, ty::DOUBLE, true, v),
            Value::Strs(_) => self.typed(&label, ty::STRING, true, v),
            Value::Tokens(_) => self.typed(&label, ty::TOKEN, true, v),
            Value::Assets(_) => self.typed(&label, ty::ASSET_PATH, true, v),
            Value::Dict(d) => self.dictionary(d, depth),
            Value::Block => Ok(inline(ty::VALUE_BLOCK, 0)),
            Value::Path(_) => Err(unsupported("a Path value has no crate type outside a path field")),
            Value::List(_) => Err(unsupported("an untyped List value has no crate type")),
        }
    }

    /// A dictionary, entries in the layer's order: `[u64 count]`, then for each
    /// `[u32 key (a string)][offset][the value's data][u64 ValueRep]`
    /// (`WriteMap` of `VtValue`s). An empty one is inline.
    fn dictionary(&mut self, d: &[(String, Value)], depth: usize) -> Result<u64, MeshError> {
        if d.is_empty() {
            return Ok(inline(ty::DICTIONARY, 0));
        }
        let mut seen = HashSet::with_capacity(d.len());
        if let Some((k, _)) = d.iter().find(|(k, _)| !seen.insert(k.as_str())) {
            return Err(unsupported(format!("the dictionary key {k:?} appears twice, and a dictionary holds one entry for a key")));
        }
        let at = self.out.len();
        self.put_u64(d.len() as u64);
        for (key, v) in d {
            let k = self.string(key)?;
            self.put_u32(k);
            let rep = self.nested(|e| e.untyped(v, depth + 1))?;
            self.put_u64(rep);
        }
        Ok(out_of_line(ty::DICTIONARY, at))
    }

    /// An attribute's time samples: `[offset][the times, a double array][u64
    /// ValueRep of it][offset][the values' data][u64 count][a ValueRep each]`
    /// (`_Writer::Write(TimeSamples)`).
    fn time_samples(&mut self, attribute: &str, type_name: &str, t: u8, array: bool, samples: &[(f64, Value)]) -> Result<u64, MeshError> {
        // A NaN is neither before nor after another time, so the ordering
        // check below would let it through: finiteness is checked first.
        if samples.iter().any(|s| !s.0.is_finite()) {
            return Err(unsupported(format!("the time samples of {} have a time that is not finite", super::usd_stage::short(attribute))));
        }
        if samples.windows(2).any(|w| w[0].0 >= w[1].0) {
            return Err(unsupported(format!("the time samples of {} are not at strictly increasing times", super::usd_stage::short(attribute))));
        }
        let at = self.out.len();
        let times: Vec<f64> = samples.iter().map(|s| s.0).collect();
        let times_rep = self.nested(|e| e.numeric_array(ty::DOUBLE, "double[]", &Value::Doubles(times)))?;
        self.put_u64(times_rep);
        let reps = self.nested(|e| samples.iter().map(|(_, v)| e.typed(type_name, t, array, v)).collect::<Result<Vec<u64>, MeshError>>())?;
        self.put_u64(reps.len() as u64);
        for r in reps {
            self.put_u64(r);
        }
        Ok(out_of_line(ty::TIME_SAMPLES, at))
    }

    /// A token, string or path list op: a header byte (explicit, and which of the
    /// lists follow), then each list as `[u64 count][u32 index...]`, in the
    /// order explicit, add, prepend, append, delete. `None` when the op says nothing.
    fn list_op<T>(&mut self, t: u8, op: &ListOp<T>, mut item: impl FnMut(&mut Encoder, &T) -> Result<(), MeshError>) -> Result<Option<u64>, MeshError> {
        let explicit = op.explicit.as_deref().filter(|e| !e.is_empty()).unwrap_or(&[]);
        let lists: [&[T]; 5] = [explicit, &op.add, &op.prepend, &op.append, &op.delete];
        let header: u8 = [2u8, 4, 32, 64, 8].iter().zip(lists).filter(|(_, l)| !l.is_empty()).fold(0, |h, (bit, _)| h | bit) | u8::from(op.explicit.is_some());
        if header == 0 {
            return Ok(None);
        }
        let at = self.out.len();
        self.out.push(header);
        for list in lists.iter().filter(|l| !l.is_empty()) {
            if list.len() > MAX_LIST_OP_ITEMS {
                return Err(unsupported(format!("a list op of {} items", list.len())));
            }
            self.put_u64(list.len() as u64);
            for x in *list {
                item(self, x)?;
            }
        }
        Ok(Some(out_of_line(t, at)))
    }

    fn token_list_op(&mut self, t: u8, op: &ListOp<String>) -> Result<Option<u64>, MeshError> {
        self.list_op(t, op, |e, s| {
            let i = if t == ty::TOKEN_LIST_OP { e.token(s)? } else { e.string(s)? };
            e.put_u32(i);
            Ok(())
        })
    }

    fn path_list_op(&mut self, op: &ListOp<String>) -> Result<Option<u64>, MeshError> {
        self.list_op(ty::PATH_LIST_OP, op, |e, p| {
            let i = e.path(p)?;
            e.put_u32(i);
            Ok(())
        })
    }

    /// References (with their empty custom data) or payloads, each as `[u32
    /// asset (a string)][u32 prim path][f64 offset][f64 scale]`.
    fn reference_list_op(&mut self, t: u8, op: &ListOp<Reference>) -> Result<Option<u64>, MeshError> {
        self.list_op(t, op, |e, r| {
            let (asset, prim) = (e.string(&r.asset)?, e.path(&r.prim)?);
            e.put_u32(asset);
            e.put_u32(prim);
            e.put_f64(r.offset);
            e.put_f64(r.scale);
            if t == ty::REFERENCE_LIST_OP {
                e.put_u64(0);
            }
            Ok(())
        })
    }

    /// A vector of names (`primChildren`, `properties`, ...): `[u64 count][u32 token...]`.
    fn token_vector(&mut self, names: &[String]) -> Result<u64, MeshError> {
        let indexes = names.iter().map(|n| self.token(n)).collect::<Result<Vec<u32>, MeshError>>()?;
        let at = self.out.len();
        self.put_u64(indexes.len() as u64);
        for i in indexes {
            self.put_u32(i);
        }
        Ok(out_of_line(ty::TOKEN_VECTOR, at))
    }

    // ── specs ───────────────────────────────────────────────────────────────

    fn layer(&mut self, layer: &Layer) -> Result<(), MeshError> {
        let mut fields: Vec<(&str, u64)> = Vec::new();
        for (key, v) in &layer.meta {
            if ROOT_FIELDS.contains(&key.as_str()) {
                return Err(unsupported(format!("the layer metadata {key} is a field the crate keeps for itself")));
            }
            fields.push((key, self.untyped(v, 0)?));
        }
        if !layer.sublayers.is_empty() {
            let mut strings = Vec::with_capacity(layer.sublayers.len());
            for (s, ..) in &layer.sublayers {
                strings.push(self.string(s)?);
            }
            let at = self.out.len();
            self.put_u64(strings.len() as u64);
            for s in strings {
                self.put_u32(s);
            }
            fields.push(("subLayers", out_of_line(ty::STRING_VECTOR, at)));
            let at = self.out.len();
            self.put_u64(layer.sublayers.len() as u64);
            for (_, offset, scale) in &layer.sublayers {
                self.put_f64(*offset);
                self.put_f64(*scale);
            }
            fields.push(("subLayerOffsets", out_of_line(ty::LAYER_OFFSET_VECTOR, at)));
        }
        for name in &layer.root_children {
            self.check_prim_name(name)?;
        }
        if !layer.root_children.is_empty() {
            fields.push(("primChildren", self.token_vector(&layer.root_children)?));
        }
        self.spec("/", SPEC_PSEUDO_ROOT, fields)?;

        // The prims, each before its children: an explicit stack, so a deep
        // hierarchy is not a deep recursion. A path nothing lists is not written, and is an error.
        let mut visited: HashSet<String> = HashSet::new();
        let mut stack: Vec<(String, bool)> = layer.root_children.iter().rev().map(|n| (super::usd_stage::child_path("/", n), false)).collect();
        while let Some((path, variant)) = stack.pop() {
            let spec = layer.specs.get(&path).ok_or_else(|| malformed(format!("{} is listed but has no spec", super::usd_stage::short(&path))))?;
            let fields = self.prim_fields(spec, variant)?;
            self.spec(&path, if variant { SPEC_VARIANT } else { SPEC_PRIM }, fields)?;
            for (name, prop) in &spec.props {
                self.property(&path, name, prop)?;
            }
            let mut next: Vec<(String, bool)> = Vec::new();
            for (set, variants) in &spec.variant_sets {
                let fields = if variants.is_empty() { Vec::new() } else { vec![("variantChildren", self.token_vector(variants)?)] };
                self.spec(&format!("{path}{{{set}=}}"), SPEC_VARIANT_SET, fields)?;
                next.extend(variants.iter().map(|v| (format!("{path}{{{set}={v}}}"), true)));
            }
            for name in &spec.children {
                self.check_prim_name(name)?;
                next.push((super::usd_stage::child_path(&path, name), false));
            }
            visited.insert(path);
            stack.extend(next.into_iter().rev());
        }
        if let Some(orphan) = layer.specs.keys().filter(|p| !visited.contains(*p)).min() {
            return Err(malformed(format!("the spec {} is not reached from the layer's root prims", super::usd_stage::short(orphan))));
        }
        Ok(())
    }

    fn check_prim_name(&self, name: &str) -> Result<(), MeshError> {
        if super::usda::valid_prim_name(name) {
            return Ok(());
        }
        Err(unsupported(format!("{} is not a prim name a path can hold", super::usd_stage::short(name))))
    }

    /// A prim's (or a variant's) fields, in the order `usdcat` writes them:
    /// the specifier and type, its metadata, then its children and properties.
    /// A variant carries neither specifier nor type.
    fn prim_fields<'a>(&mut self, spec: &'a PrimSpec, variant: bool) -> Result<Vec<(&'a str, u64)>, MeshError> {
        let mut f: Vec<(&'a str, u64)> = Vec::new();
        if !variant {
            let specifier = match spec.specifier {
                Specifier::Def => 0,
                Specifier::Over => 1,
                Specifier::Class => 2,
            };
            f.push(("specifier", inline(ty::SPECIFIER, specifier)));
            if !spec.type_name.is_empty() {
                f.push(("typeName", inline(ty::TOKEN, self.token(&spec.type_name)?)));
            }
        }
        if let Some(active) = spec.active {
            f.push(("active", inline(ty::BOOL, u32::from(active))));
        }
        if let Some(kind) = &spec.kind {
            f.push(("kind", inline(ty::TOKEN, self.token(kind)?)));
        }
        if let Some(data) = &spec.custom_data {
            f.push(("customData", self.dictionary(data, 0)?));
        }
        if let Some(rep) = spec.api_schemas.as_ref().map(|op| self.token_list_op(ty::TOKEN_LIST_OP, op)).transpose()?.flatten() {
            f.push(("apiSchemas", rep));
        }
        if let Some(rep) = spec.references.as_ref().map(|op| self.reference_list_op(ty::REFERENCE_LIST_OP, op)).transpose()?.flatten() {
            f.push(("references", rep));
        }
        if let Some(rep) = spec.payloads.as_ref().map(|op| self.reference_list_op(ty::PAYLOAD_LIST_OP, op)).transpose()?.flatten() {
            f.push(("payload", rep));
        }
        if let Some(rep) = spec.inherits.as_ref().map(|op| self.path_list_op(op)).transpose()?.flatten() {
            f.push(("inheritPaths", rep));
        }
        if let Some(rep) = spec.specializes.as_ref().map(|op| self.path_list_op(op)).transpose()?.flatten() {
            f.push(("specializes", rep));
        }
        if !spec.variant_selection.is_empty() {
            let mut pairs = Vec::with_capacity(spec.variant_selection.len());
            for (set, name) in &spec.variant_selection {
                pairs.push((self.string(set)?, self.string(name)?));
            }
            let at = self.out.len();
            self.put_u64(pairs.len() as u64);
            for (set, name) in pairs {
                self.put_u32(set);
                self.put_u32(name);
            }
            f.push(("variantSelection", out_of_line(ty::VARIANT_SELECTION_MAP, at)));
        }
        if let Some(rep) = spec.variant_set_names.as_ref().map(|op| self.token_list_op(ty::STRING_LIST_OP, op)).transpose()?.flatten() {
            f.push(("variantSetNames", rep));
        }
        if !spec.variant_sets.is_empty() {
            let sets: Vec<String> = spec.variant_sets.iter().map(|(s, _)| s.clone()).collect();
            f.push(("variantSetChildren", self.token_vector(&sets)?));
        }
        if !spec.children.is_empty() {
            f.push(("primChildren", self.token_vector(&spec.children)?));
        }
        if !spec.props.is_empty() {
            let names: Vec<String> = spec.props.iter().map(|(n, _)| n.clone()).collect();
            f.push(("properties", self.token_vector(&names)?));
        }
        Ok(f)
    }

    /// A property's spec. An attribute is `custom`, its type, its variability, its default,
    /// its connections, its metadata and its time samples; a relationship is uniform and its
    /// targets. (`Variability::Config` is written as `uniform`: OpenUSD has no third kind,
    /// and `usdcat` makes the same of the text `config`.)
    fn property(&mut self, prim: &str, name: &str, prop: &super::usd_stage::Prop) -> Result<(), MeshError> {
        if name.is_empty() || name.contains(['/', '.', '{', '}', '[', ']']) {
            return Err(unsupported(format!("{} is not a property name a path can hold", super::usd_stage::short(name))));
        }
        let path = format!("{prim}.{name}");
        match prop {
            super::usd_stage::Prop::Attr(a) => {
                let (t, array) = crate_type(&a.type_name)?;
                let variability = match a.variability {
                    Variability::Varying => 0,
                    Variability::Uniform | Variability::Config => 1,
                };
                let mut f: Vec<(&str, u64)> =
                    vec![("custom", inline(ty::BOOL, 0)), ("typeName", inline(ty::TOKEN, self.token(&a.type_name)?)), ("variability", inline(ty::VARIABILITY, variability))];
                if let Some(v) = &a.default {
                    f.push(("default", self.typed(&a.type_name, t, array, v)?));
                }
                if let Some(rep) = a.connections.as_ref().map(|op| self.path_list_op(op)).transpose()?.flatten() {
                    f.push(("connectionPaths", rep));
                }
                if let Some(i) = &a.interpolation {
                    f.push(("interpolation", inline(ty::TOKEN, self.token(i)?)));
                }
                if let Some(n) = a.element_size {
                    f.push(("elementSize", self.untyped(&Value::Int(n), 0)?));
                }
                for (key, v) in &a.metadata {
                    if ATTR_FIELDS.contains(&key.as_str()) {
                        return Err(unsupported(format!("the metadata {key} of {} is a field the crate keeps for itself", super::usd_stage::short(&path))));
                    }
                    f.push((key, self.untyped(v, 0)?));
                }
                if let Some(samples) = &a.samples {
                    f.push(("timeSamples", self.time_samples(&path, &a.type_name, t, array, samples)?));
                }
                self.spec(&path, SPEC_ATTRIBUTE, f)
            }
            super::usd_stage::Prop::Rel(r) => {
                let mut f: Vec<(&str, u64)> = vec![("variability", inline(ty::VARIABILITY, 1))];
                if let Some(rep) = r.targets.as_ref().map(|op| self.path_list_op(op)).transpose()?.flatten() {
                    f.push(("targetPaths", rep));
                }
                self.spec(&path, SPEC_RELATIONSHIP, f)
            }
        }
    }

    // ── the sections ────────────────────────────────────────────────────────

    /// `[u64 compressed size][TfFastCompression of the integer coding]`.
    fn compressed_ints(out: &mut Vec<u8>, values: &[i64]) {
        let packed = fast_compress(&encode_ints(values, false));
        out.extend_from_slice(&(packed.len() as u64).to_le_bytes());
        out.extend_from_slice(&packed);
    }

    fn finish(mut self) -> Result<Vec<u8>, MeshError> {
        let mut toc: Vec<(&str, usize, usize)> = Vec::new();
        let mut section = |out: &mut Vec<u8>, name: &'static str, build: &dyn Fn(&mut Vec<u8>) -> Result<(), MeshError>| -> Result<(), MeshError> {
            let start = out.len();
            build(out)?;
            toc.push((name, start, out.len() - start));
            Ok(())
        };
        let signed = |v: &[u32]| -> Vec<i64> { v.iter().map(|x| i64::from(*x as i32)).collect() };
        let mut out = std::mem::take(&mut self.out);

        section(&mut out, "TOKENS", &|out| {
            let mut blob = Vec::new();
            for t in &self.tokens {
                blob.extend_from_slice(t.as_bytes());
                blob.push(0);
            }
            if blob.len() > MAX_TOKEN_BYTES {
                return Err(unsupported("a token table larger than the reader reads"));
            }
            let packed = fast_compress(&blob);
            out.extend_from_slice(&(self.tokens.len() as u64).to_le_bytes());
            out.extend_from_slice(&(blob.len() as u64).to_le_bytes());
            out.extend_from_slice(&(packed.len() as u64).to_le_bytes());
            out.extend_from_slice(&packed);
            Ok(())
        })?;
        section(&mut out, "STRINGS", &|out| {
            out.extend_from_slice(&(self.strings.len() as u64).to_le_bytes());
            for s in &self.strings {
                out.extend_from_slice(&s.to_le_bytes());
            }
            Ok(())
        })?;
        section(&mut out, "FIELDS", &|out| {
            out.extend_from_slice(&(self.fields.len() as u64).to_le_bytes());
            let tokens: Vec<i64> = self.fields.iter().map(|(t, _)| i64::from(*t)).collect();
            Encoder::compressed_ints(out, &tokens);
            let reps: Vec<u8> = self.fields.iter().flat_map(|(_, r)| r.to_le_bytes()).collect();
            let packed = fast_compress(&reps);
            out.extend_from_slice(&(packed.len() as u64).to_le_bytes());
            out.extend_from_slice(&packed);
            Ok(())
        })?;
        section(&mut out, "FIELDSETS", &|out| {
            out.extend_from_slice(&(self.fieldsets.len() as u64).to_le_bytes());
            Encoder::compressed_ints(out, &signed(&self.fieldsets));
            Ok(())
        })?;
        section(&mut out, "PATHS", &|out| {
            let (mut index, mut element, mut jump) = (Vec::new(), Vec::new(), Vec::new());
            // Each node's entries: itself and everything below it, children after parents.
            let mut size = vec![0u64; self.nodes.len()];
            for i in (0..self.nodes.len()).rev() {
                size[i] = 1 + self.nodes[i].children.iter().map(|c| size[*c as usize]).sum::<u64>();
            }
            let mut stack: Vec<(u32, bool)> = vec![(0, false)];
            while let Some((i, has_sibling)) = stack.pop() {
                let node = &self.nodes[i as usize];
                index.push(i64::from(i));
                element.push(if node.property { -i64::from(node.token) } else { i64::from(node.token) });
                jump.push(match (node.children.is_empty(), has_sibling) {
                    (false, true) => size[i as usize] as i64,
                    (true, true) => 0,
                    (false, false) => -1,
                    (true, false) => -2,
                });
                let last = node.children.len();
                stack.extend(node.children.iter().enumerate().rev().map(|(k, c)| (*c, k + 1 < last)));
            }
            out.extend_from_slice(&(self.nodes.len() as u64).to_le_bytes());
            out.extend_from_slice(&(index.len() as u64).to_le_bytes());
            for ints in [&index, &element, &jump] {
                Encoder::compressed_ints(out, ints);
            }
            Ok(())
        })?;
        section(&mut out, "SPECS", &|out| {
            out.extend_from_slice(&(self.specs.len() as u64).to_le_bytes());
            let columns: [Vec<i64>; 3] = [
                self.specs.iter().map(|s| i64::from(s.0)).collect(),
                self.specs.iter().map(|s| i64::from(s.1)).collect(),
                self.specs.iter().map(|s| s.2).collect(),
            ];
            for column in &columns {
                Encoder::compressed_ints(out, column);
            }
            Ok(())
        })?;

        // The table of contents ends the file; the bootstrap at its start points at it.
        let toc_at = out.len();
        out.extend_from_slice(&(toc.len() as u64).to_le_bytes());
        for (name, start, size) in toc {
            let mut padded = [0u8; 16];
            padded[..name.len()].copy_from_slice(name.as_bytes());
            out.extend_from_slice(&padded);
            out.extend_from_slice(&(start as i64).to_le_bytes());
            out.extend_from_slice(&(size as i64).to_le_bytes());
        }
        out[..MAGIC.len()].copy_from_slice(MAGIC);
        out[8..8 + WRITE_VERSION.len()].copy_from_slice(&WRITE_VERSION);
        out[16..24].copy_from_slice(&(toc_at as i64).to_le_bytes());
        Ok(out)
    }
}

/// Encode a layer as a crate (version 0.8.0), the bytes `usdcat` would write
/// for the same text, up to the order of its tables. The layer is what
/// [`super::usda::parse`] makes of litegen's own USDA, so a text layer and its
/// crate cannot say different things; [`parse`] reads the result back as an
/// equal layer.
///
/// What the layer's types do not say is never guessed: a `type_name` or a
/// `Value` this cannot write is [`MeshError::Unsupported`], naming it. Arrays
/// are uncompressed, the structural sections are not, and nothing here needs a
/// match finder.
pub fn write(layer: &Layer) -> Result<Vec<u8>, MeshError> {
    let mut e = Encoder::new();
    e.layer(layer)?;
    e.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lz4_decodes_literals_matches_and_overlapping_runs() {
        // "abcabcabcabc" = literals "abc" + match(offset 3, length 9).
        // A block always ends with a literals-only sequence (here, empty).
        let block = [0x35u8, b'a', b'b', b'c', 3, 0, 0x00];
        let mut out = Vec::new();
        // Token 0x35: 3 literals, match length 5 + 4 = 9.
        lz4_block(&block, &mut out, 64).unwrap();
        assert_eq!(out, b"abcabcabcabc");
        // A match before the block start, and output past the limit, fail.
        assert!(lz4_block(&[0x10, b'a', 5, 0], &mut Vec::new(), 64).is_err());
        assert!(lz4_block(&block, &mut Vec::new(), 8).is_err());
    }

    #[test]
    fn integer_coding_matches_the_reference_decoder() {
        // common = 1, codes: [0, 1, 2, 3] -> deltas 1, int8 -2, int16 300, int32 -70000.
        let mut data = 1i32.to_le_bytes().to_vec();
        data.push(0b11_10_01_00);
        data.push((-2i8) as u8);
        data.extend_from_slice(&300i16.to_le_bytes());
        data.extend_from_slice(&(-70000i32).to_le_bytes());
        assert_eq!(decode_ints(&data, 4, false).unwrap(), vec![1, -1, 299, -69701]);
        assert!(decode_ints(&data[..6], 4, false).is_err());
    }

    #[test]
    fn halves_convert_exactly() {
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0x3D00), 1.25);
        assert_eq!(half_to_f32(0xC000), -2.0);
        assert_eq!(half_to_f32(0x0001), 2f32.powi(-24));
        assert!(half_to_f32(0x7C00).is_infinite());
    }

    #[test]
    fn f32_to_half_is_the_inverse_partner_of_half_to_f32() {
        // Every half goes to an f32 and back to itself; a NaN stays a NaN.
        for h in 0..=u16::MAX {
            let x = half_to_f32(h);
            if x.is_nan() {
                assert!(half_to_f32(f32_to_half(x)).is_nan(), "{h:#06x}");
            } else {
                assert_eq!(f32_to_half(x), h, "{h:#06x} = {x}");
            }
        }
        // Halfway goes to the even neighbour, not up: 1 + 2^-11 lies between 1
        // (0x3C00) and the next half (0x3C01); 1 + 3 * 2^-11 between 0x3C01 and 0x3C02.
        assert_eq!(f32_to_half(1.0 + 2f32.powi(-11)), 0x3C00);
        assert_eq!(f32_to_half(1.0 + 3.0 * 2f32.powi(-11)), 0x3C02);
        assert_eq!(f32_to_half(1.0 + 2f32.powi(-11) + 2f32.powi(-20)), 0x3C01, "over halfway goes up");
        // The top: 65504 is the largest half, 65520 is halfway to 2^16 (even, so infinity).
        assert_eq!(f32_to_half(65504.0), 0x7BFF);
        assert_eq!(f32_to_half(65519.0), 0x7BFF);
        assert_eq!(f32_to_half(65520.0), 0x7C00);
        assert_eq!(f32_to_half(f32::MAX), 0x7C00);
        assert_eq!(f32_to_half(f32::NEG_INFINITY), 0xFC00);
        // The bottom: subnormals step by 2^-24, and 2^-25 is halfway to zero.
        assert_eq!(f32_to_half(2f32.powi(-14)), 0x0400);
        assert_eq!(f32_to_half(2f32.powi(-24)), 0x0001);
        assert_eq!(f32_to_half(1.5 * 2f32.powi(-24)), 0x0002);
        assert_eq!(f32_to_half(2f32.powi(-25)), 0x0000);
        assert_eq!(f32_to_half(2f32.powi(-25) * 1.001), 0x0001);
        assert_eq!(f32_to_half(2f32.powi(-30)), 0x0000);
        assert_eq!(f32_to_half(-0.0), 0x8000);
        assert_eq!(f32_to_half(-2f32.powi(-30)), 0x8000, "the sign survives underflow");
        assert_eq!(half_to_f32(f32_to_half(0.333)) as f64, 0.3330078125);
        // It agrees with the rounding `usd::to_half_precision` does by arithmetic,
        // over values spread across the half range.
        let mut seed = 0x2545_F491u32;
        for _ in 0..200_000 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let e = (seed >> 24) as i32 % 40 - 26;
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let m = 1.0 + (seed >> 9) as f32 / (1u32 << 23) as f32;
            let x = m * 2f32.powi(e) * if seed & 1 == 1 { -1.0 } else { 1.0 };
            if x.abs() < 65_000.0 {
                assert_eq!(half_to_f32(f32_to_half(x)), super::super::usd::to_half_precision(x), "{x}");
            }
        }
    }

    #[test]
    fn a_path_tree_that_loops_is_refused() {
        let tokens = vec!["".to_string(), "A".to_string()];
        // Entry 1 claims a sibling at 1 + (-1)... jumps of 0 mean "sibling
        // next"; make entry 1 jump back onto entry 0.
        let r = build_paths(&tokens, 2, &[0, 1], &[0, 1], &[-1, 0]);
        assert!(r.is_err(), "{r:?}");
    }

    /// The byte offset of crate section `name`'s data, from the table of
    /// contents (`toc offset` at byte 16; entries of name[16], start, size).
    fn section_start(bytes: &[u8], name: &str) -> usize {
        let toc = u64::from_le_bytes(bytes[16..24].try_into().unwrap()) as usize;
        let n = u64::from_le_bytes(bytes[toc..toc + 8].try_into().unwrap()) as usize;
        (0..n)
            .map(|i| toc + 8 + i * 32)
            .find(|&e| bytes[e..e + 16].split(|b| *b == 0).next() == Some(name.as_bytes()))
            .map(|e| u64::from_le_bytes(bytes[e + 16..e + 24].try_into().unwrap()) as usize)
            .unwrap()
    }

    /// 2026-10-12 review, finding 23.10: a value of a crate type this reader
    /// does not decode (57 and 59, from newer USD: path expressions, splines)
    /// skips its field and is named in a warning; the layer is read. It used
    /// to fail the whole layer. A broken value still refuses it.
    #[test]
    fn a_value_of_an_unread_type_skips_its_field_not_the_layer() {
        let mut cr = Crate { b: &[0u8; 64], minor: 11, tokens: Vec::new(), strings: Vec::new(), paths: PathTable::default(), values: 0 };
        let mut unread = std::collections::BTreeSet::new();
        for t in [57u64, 59] {
            assert!(cr.value((t << 48) | 8, 0).is_err());
            assert!(matches!(field_value(&mut cr, (t << 48) | 8, &mut unread), Ok(None)), "type {t}");
        }
        assert_eq!(unread.into_iter().collect::<Vec<_>>(), ["57", "59"]);
        // A broken value is still the layer's error: an int array past the end.
        assert!(field_value(&mut cr, (1u64 << 63) | (3u64 << 48) | 1000, &mut std::collections::BTreeSet::new()).is_err());
    }

    /// Sort what USD itself keeps unordered, so two layers compare by content:
    /// dictionary entries (a `VtDictionary` is a sorted map, so `usdcat`
    /// writes `customData` by key, nested dictionaries too) and the metadata
    /// fields of an attribute (the crate lists them by name). A layer's own
    /// metadata keeps the order it was written in, and so does everything
    /// else: `typed.usdc` agrees with `typed.usda` on it as it stands.
    fn sort_unordered(entries: &mut [(String, Value)]) {
        sort_nested(entries);
        entries.sort_by(|a, b| a.0.cmp(&b.0));
    }

    fn sort_nested(entries: &mut [(String, Value)]) {
        for (_, v) in entries.iter_mut() {
            if let Value::Dict(d) = v {
                sort_unordered(d);
            }
        }
    }

    fn by_content(mut l: Layer) -> Layer {
        sort_nested(&mut l.meta);
        for spec in l.specs.values_mut() {
            if let Some(d) = spec.custom_data.as_mut() {
                sort_unordered(d);
            }
            for (_, prop) in spec.props.iter_mut() {
                if let super::super::usd_stage::Prop::Attr(a) = prop {
                    sort_unordered(&mut a.metadata);
                }
            }
        }
        l
    }

    /// The crate reader fills what `usda_keeps_variability_metadata_and_token_types`
    /// shows the text reader fills. `typed.usdc` is `usdcat typed.usda -o
    /// typed.usdc` (Apple USD Tools 0.25.2): Pixar's writer, so the reader is
    /// tested on bytes our writer never produced.
    #[test]
    fn usdc_reader_fills_the_same_fields() {
        let text = super::super::usda::parse(include_bytes!("testdata/usd/typed.usda"), "typed.usda").unwrap();
        let krate = parse(include_bytes!("testdata/usd/typed.usdc"), "typed.usda").unwrap();

        // The crate holds these itself, so they are checked on the crate's own
        // layer: two readers that both dropped them would still agree.
        let root = &krate.specs["/Root"];
        assert_eq!(root.attr("purpose").unwrap().variability, Variability::Uniform);
        assert_eq!(root.attr("mode").unwrap().variability, Variability::Varying);
        assert_eq!(root.attr("purpose").unwrap().default, Some(Value::Token("render".into())));
        assert_eq!(root.attr("title").unwrap().default, Some(Value::Str("a title".into())));
        assert_eq!(root.attr("modes").unwrap().default, Some(Value::Tokens(vec!["fast".into(), "slow".into()])));
        assert_eq!(root.attr("titles").unwrap().default, Some(Value::Strs(vec!["one".into(), "two".into()])));
        assert_eq!(root.attr("animated").unwrap().samples, Some(vec![(0.0, Value::Token("start".into())), (1.0, Value::Token("end".into()))]));
        assert_eq!(krate.meta("upAxis"), Some(&Value::Token("Y".into())));
        let documented = root.attr("documented").unwrap();
        let field = |k: &str| documented.metadata.iter().find(|(n, _)| n == k).map(|(_, v)| v);
        assert_eq!(field("documentation"), Some(&Value::Str("what it is".into())));
        assert_eq!(field("hidden"), Some(&Value::Bool(true)));
        let data = field("customData").and_then(Value::dict).unwrap();
        assert!(data.contains(&("tag".to_string(), Value::Token("attr token".into()))), "{data:?}");
        assert!(data.contains(&("note".to_string(), Value::Str("attribute data".into()))), "{data:?}");

        assert_eq!(by_content(krate), by_content(text));
    }

    /// The text reader keeps a `half` as the decimal it reads, and a crate
    /// stores it as a half: `half h = 0.333` is 0.333 from `stress.usda` and
    /// 0.3330078125 from `stress.usdc` (the reader's choice for third-party
    /// text, which a read does not change). So the comparison rounds the text
    /// side's `half`-typed values to half precision first, as Pixar's writer
    /// did.
    fn halves_rounded(mut l: Layer) -> Layer {
        let round = |v: f64| half_to_f32(f32_to_half(v as f32)) as f64;
        let round_value = |v: &mut Value| match v {
            Value::Float(f) => *f = round(*f),
            Value::Floats(fs) => fs.iter_mut().for_each(|f| *f = half_to_f32(f32_to_half(*f))),
            Value::Tuple(t) => t.iter_mut().for_each(|f| *f = round(*f)),
            _ => {}
        };
        let is_half = |t: &str| {
            let t = t.trim_end_matches("[]");
            t.starts_with("half") || t.starts_with("quath") || (t.ends_with('h') && t.chars().any(|c| c.is_ascii_digit()))
        };
        for spec in l.specs.values_mut() {
            for (_, prop) in spec.props.iter_mut() {
                if let super::super::usd_stage::Prop::Attr(a) = prop {
                    if is_half(&a.type_name) {
                        a.default.iter_mut().for_each(round_value);
                        for (_, v) in a.samples.iter_mut().flatten() {
                            round_value(v);
                        }
                    }
                }
            }
        }
        l
    }

    /// On the files Pixar's writer made from `comp.usda` and `stress.usda`,
    /// the text reader and the crate reader fill every field of the layer
    /// alike: specs, prim fields (references, variants, inherits, ...),
    /// properties with their variability, defaults, samples, connections and
    /// metadata, and the layer's own metadata. `comp` compares exactly;
    /// `stress` after `halves_rounded` (the one value the two read
    /// differently, by design).
    #[test]
    fn the_text_and_crate_readers_agree_on_the_comp_and_stress_layers() {
        let text = |bytes: &[u8], id: &str| super::super::usda::parse(bytes, id).unwrap();
        let comp_text = text(include_bytes!("testdata/usd/comp.usda"), "comp.usda");
        let comp_crate = parse(include_bytes!("testdata/usd/comp.usdc"), "comp.usda").unwrap();
        assert_eq!(by_content(comp_crate), by_content(comp_text));

        let stress_text = text(include_bytes!("testdata/usd/stress.usda"), "stress.usda");
        let stress_crate = parse(include_bytes!("testdata/usd/stress.usdc"), "stress.usda").unwrap();
        assert_eq!(by_content(stress_crate), by_content(halves_rounded(stress_text)));
    }

    /// 2026-10-12 review, finding 8: the path table was sized by its declared
    /// count, bounded only by MAX_TABLE, so `cube.usdc` claiming 64Mi paths
    /// read at 1.6 GB of RSS. The table now holds what the encoded entries
    /// name — the count is kept for bounds checks only (OpenUSD's own
    /// `comp.usdc` declares one slot more than it encodes).
    #[test]
    fn the_path_table_holds_entries_not_the_declared_count() {
        let t = build_paths(&["/".to_string()], MAX_TABLE, &[0], &[0], &[-2]).unwrap();
        assert_eq!((t.len(), t.slots.len()), (MAX_TABLE, 1));
        assert_eq!(t.get(0), Some(Some(&"/".to_string())));
        assert_eq!(t.get(5), Some(None), "a slot no entry fills");
        assert_eq!(t.get(MAX_TABLE), None, "past the table");

        let cube = include_bytes!("testdata/usd/cube.usdc").to_vec();
        let honest = super::super::usd::read_usda(&cube, &super::super::package::NoSidecars).unwrap().scene;
        let at = section_start(&cube, "PATHS");
        let mut lying = cube.clone();
        lying[at..at + 8].copy_from_slice(&67_108_864u64.to_le_bytes());
        let read = super::super::usd::read_usda(&lying, &super::super::package::NoSidecars).unwrap().scene;
        assert_eq!(read, honest);
    }

    // ─── the crate writer ───────────────────────────────────────────────────

    fn usda_layer(text: &str) -> Layer {
        super::super::usda::parse(text.as_bytes(), "t.usda").unwrap_or_else(|e| panic!("{e}"))
    }

    fn clip(s: String) -> String {
        s.chars().take(300).collect()
    }

    /// The first place two layers differ, as a short message: a `Layer` holds
    /// whole meshes, and `assert_eq!` on two of them prints megabytes.
    fn first_difference(got: &Layer, want: &Layer) -> Option<String> {
        use super::super::usd_stage::Prop;
        if got.meta != want.meta {
            return Some(clip(format!("layer metadata {:?} is not {:?}", got.meta, want.meta)));
        }
        if got.sublayers != want.sublayers {
            return Some(clip(format!("sublayers {:?} are not {:?}", got.sublayers, want.sublayers)));
        }
        if got.root_children != want.root_children {
            return Some(clip(format!("root children {:?} are not {:?}", got.root_children, want.root_children)));
        }
        if got.warnings != want.warnings {
            return Some(clip(format!("warnings {:?} are not {:?}", got.warnings, want.warnings)));
        }
        let mut paths: Vec<&String> = want.specs.keys().chain(got.specs.keys()).collect();
        paths.sort();
        paths.dedup();
        for p in paths {
            let (Some(g), Some(w)) = (got.specs.get(p), want.specs.get(p)) else {
                return Some(format!("{p} is a spec of only one of the layers"));
            };
            let bare = |s: &PrimSpec| PrimSpec { props: Vec::new(), ..s.clone() };
            if bare(g) != bare(w) {
                return Some(clip(format!("{p}: prim fields {:?} are not {:?}", bare(g), bare(w))));
            }
            let (gn, wn): (Vec<&String>, Vec<&String>) = (g.props.iter().map(|(n, _)| n).collect(), w.props.iter().map(|(n, _)| n).collect());
            if gn != wn {
                return Some(clip(format!("{p}: properties {gn:?} are not {wn:?}")));
            }
            for ((name, gp), (_, wp)) in g.props.iter().zip(&w.props) {
                match (gp, wp) {
                    (Prop::Attr(a), Prop::Attr(b)) => {
                        let fields: [(&str, bool); 8] = [
                            ("type_name", a.type_name == b.type_name),
                            ("variability", a.variability == b.variability),
                            ("default", a.default == b.default),
                            ("samples", a.samples == b.samples),
                            ("connections", a.connections == b.connections),
                            ("interpolation", a.interpolation == b.interpolation),
                            ("element_size", a.element_size == b.element_size),
                            ("metadata", a.metadata == b.metadata),
                        ];
                        if let Some((field, _)) = fields.iter().find(|(_, same)| !same) {
                            let (x, y) = match *field {
                                "default" => (format!("{:?}", a.default), format!("{:?}", b.default)),
                                "samples" => (format!("{:?}", a.samples), format!("{:?}", b.samples)),
                                "metadata" => (format!("{:?}", a.metadata), format!("{:?}", b.metadata)),
                                _ => (format!("{a:?}"), format!("{b:?}")),
                            };
                            return Some(clip(format!("{p}.{name}: {field} {x} is not {y}")));
                        }
                    }
                    (a, b) if a != b => return Some(clip(format!("{p}.{name}: {a:?} is not {b:?}"))),
                    _ => {}
                }
            }
        }
        None
    }

    fn assert_same_layer(what: &str, got: &Layer, want: &Layer) {
        if got != want {
            panic!("{what}: {}", first_difference(got, want).unwrap_or_else(|| "the layers differ somewhere first_difference does not look".into()));
        }
    }

    /// Write `layer` as a crate and read it back.
    fn through_the_crate(what: &str, layer: &Layer) -> Layer {
        let bytes = write(layer).unwrap_or_else(|e| panic!("{what}: {e}"));
        parse(&bytes, &layer.id).unwrap_or_else(|e| panic!("{what}: the crate does not read back: {e}"))
    }

    /// Every layer litegen writes (Ruling in Task 1's notes): each fixture's
    /// USDA text, in both write modes (a single file, and a package with its
    /// texture files), parsed.
    fn layers_we_write() -> Vec<(String, Layer)> {
        use super::super::{fixtures, write_scene, MeshFormat, WriteOptions};
        let mut out = Vec::new();
        for (name, scene) in fixtures::all() {
            for self_contained in [true, false] {
                let mode = if self_contained { "self-contained" } else { "package" };
                let opts = WriteOptions { self_contained, ..Default::default() };
                let files = write_scene(&scene, MeshFormat::Usda, opts).unwrap_or_else(|e| panic!("{name} ({mode}): {e}")).package.files;
                let layer = super::super::usda::parse(&files[0].bytes, "model.usda").unwrap_or_else(|e| panic!("{name} ({mode}): {e}"));
                out.push((format!("{name} ({mode})"), layer));
            }
        }
        out
    }

    /// The layer at the precision its attributes declare (Ruling U16). Sdf
    /// stores a `float` as an f32, and our text reader keeps the decimal it
    /// reads, `0.6`, as an f64 by design, so a crate reads that back as
    /// 0.6000000238418579: one stored number, two layers. The comparison is
    /// therefore at the declared precision: the values of an attribute whose
    /// declared `type_name` is single-precision (`float`, `float3`,
    /// `color3f`, `quatf`... as the writer's own type table says) are rounded
    /// to f32 on the text side. The choice is by `type_name` alone, never by
    /// looking at a value. Everything else stays strict: arrays already hold
    /// f32s, a `double` and a dictionary's entry hold f64s, and a `half` is
    /// not rounded here (the writer rounds its `half3[]` scales itself, Ruling
    /// U13b, so they must round-trip as they are).
    fn as_a_crate_holds(mut l: Layer) -> Layer {
        let single = |t: &str| matches!(crate_type(t), Ok((ty::FLOAT | ty::VEC2F | ty::VEC3F | ty::VEC4F | ty::QUATF, _)));
        let round = |v: &mut Value| match v {
            Value::Float(f) => *f = f64::from(*f as f32),
            Value::Tuple(t) => t.iter_mut().for_each(|x| *x = f64::from(*x as f32)),
            _ => {}
        };
        for spec in l.specs.values_mut() {
            for (_, prop) in spec.props.iter_mut() {
                if let super::super::usd_stage::Prop::Attr(a) = prop {
                    if single(&a.type_name) {
                        a.default.iter_mut().for_each(round);
                        for (_, v) in a.samples.iter_mut().flatten() {
                            round(v);
                        }
                    }
                }
            }
        }
        l
    }

    #[test]
    fn integer_coding_round_trips() {
        // The worked example in OpenUSD's integerCoding.cpp: the common value
        // (1), the 2-bit codes (8-bit, common, common, 32-bit | 8-bit, common,
        // 8-bit), then the odd-sized deltas.
        let mut known = 1i32.to_le_bytes().to_vec();
        known.extend([0b11_00_00_01, 0b00_01_00_01, 123]);
        known.extend(100_000i32.to_le_bytes());
        known.extend([0, 0]);
        assert_eq!(encode_ints(&[123, 124, 125, 100_125, 100_125, 100_126, 100_126], false), known);

        let mut seed = 0x1234_5678u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let narrow: Vec<(&str, Vec<i64>)> = vec![
            ("empty", vec![]),
            ("one value", vec![42]),
            ("one negative value", vec![-1]),
            ("all equal", vec![7; 100]),
            ("all zero", vec![0; 9]),
            ("a run", (0..1000).collect()),
            ("a count that is not a multiple of four", vec![5, 6, 7, 8, 9, 10, 11]),
            ("two", vec![3, 1]),
            ("three", vec![3, 1, 4]),
            ("the extremes", vec![i32::MIN as i64, i32::MAX as i64, 0, -1, i32::MIN as i64, i32::MAX as i64]),
            // Deltas of 0 (common), 5 (8-bit), 300 (16-bit) and 70,000 (32-bit).
            ("each width code", vec![0, 0, 5, 305, 70_305, 70_305, 70_300, 70_000, -230_000, -230_000]),
            ("a field set's terminator", vec![0, 1, 2, -1, 3, 4, -1, 0, 1, 2, -1]),
            ("random", (0..777).map(|_| next() as i32 as i64).collect()),
            ("small random", (0..777).map(|_| (next() % 50) as i64).collect()),
        ];
        for (what, values) in &narrow {
            let packed = encode_ints(values, false);
            assert_eq!(decode_ints(&packed, values.len(), false).ok().as_ref(), Some(values), "32-bit: {what}");
            assert_eq!(packed.is_empty(), values.is_empty(), "32-bit: {what}: nothing codes to nothing");
        }
        let wide: Vec<(&str, Vec<i64>)> = vec![
            ("empty", vec![]),
            ("one value", vec![i64::MAX]),
            ("all equal", vec![-9; 33]),
            ("the extremes", vec![i64::MIN, i64::MAX, 0, -1, i64::MIN, i64::MAX, 1]),
            // Deltas of 0 (common), 500 (16-bit), 100,000 (32-bit) and 5e9 (64-bit).
            ("each width code", vec![0, 0, 500, 100_500, 5_000_100_500, 5_000_100_500, 0, -5_000_000_000]),
            ("beyond 32 bits", vec![0, 1, 2, 5_000_000_000, -5_000_000_000, 0, 1, 2, 3]),
            ("random", (0..777).map(|_| next() as i64).collect()),
            ("small random", (0..777).map(|_| (next() % 50) as i64).collect()),
        ];
        for (what, values) in &wide {
            let packed = encode_ints(values, true);
            assert_eq!(decode_ints(&packed, values.len(), true).ok().as_ref(), Some(values), "64-bit: {what}");
        }
        // The encoding is smaller than the plain integers where it should be: a
        // run of 1000 consecutive values takes the common value, two bits each.
        assert_eq!(encode_ints(&(1..=1000).collect::<Vec<i64>>(), false).len(), 4 + 250);
    }

    #[test]
    fn literal_only_lz4_round_trips_through_our_decoder() {
        let mut seed = 7u32;
        let mut bytes = |n: usize| -> Vec<u8> {
            (0..n)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 24) as u8
                })
                .collect()
        };
        // Lengths either side of each place the block's length field changes
        // shape: 15 (the token's nibble is full), 270 (one 255 extension byte
        // is full) and 525.
        for n in [0usize, 1, 14, 15, 16, 269, 270, 271, 524, 525, 526, 100_000] {
            let src = bytes(n);
            let packed = fast_compress(&src);
            assert_eq!(packed[0], 0, "{n} bytes are one chunk");
            // One sequence: the token, the length's extension bytes, the literals.
            let extension = if n >= 15 { (n - 15) / 255 + 1 } else { 0 };
            assert_eq!(packed.len(), 1 + 1 + extension + n, "{n} bytes are literals and nothing else");
            assert_eq!(fast_decompress(&packed, n).ok(), Some(src), "{n} bytes");
            if n > 0 {
                assert!(fast_decompress(&packed, n - 1).is_err(), "{n} bytes: the decoder's bound holds");
            }
        }
        // Several chunks, as TfFastCompression frames what one block cannot
        // hold: the chunk count, then each chunk's i32 size and block.
        let src = bytes(1000);
        let packed = fast_compress_chunked(&src, 300);
        assert_eq!(packed[0], 4, "ceil(1000 / 300) chunks");
        assert_eq!(fast_decompress(&packed, 1000).ok(), Some(src.clone()));
        assert_eq!(fast_compress_chunked(&src, 500)[0], 2);
        assert_eq!(fast_compress_chunked(&src, 1000)[0], 0, "a source that fits one block has no chunk count");
        assert_eq!(fast_compress(&src), fast_compress_chunked(&src, LZ4_MAX_BLOCK));
    }

    #[test]
    fn the_bootstrap_says_0_8_0() {
        let layer = Layer { id: "t.usdc".into(), root_children: vec!["P".into()], specs: [("/P".to_string(), PrimSpec::default())].into(), ..Default::default() };
        let bytes = write(&layer).unwrap();
        assert!(looks_like_crate(&bytes));
        assert_eq!(&bytes[..8], b"PXR-USDC");
        assert_eq!(&bytes[8..16], &[0, 8, 0, 0, 0, 0, 0, 0], "version 0.8.0, patch and the rest of its eight bytes zero");
        assert!(bytes[24..BOOTSTRAP_LEN].iter().all(|b| *b == 0), "the reserved bytes are zero");
        // The table of contents lists the six sections `usdcat` writes, in its order.
        let toc = u64::from_le_bytes(bytes[16..24].try_into().unwrap()) as usize;
        assert!(toc >= BOOTSTRAP_LEN && toc < bytes.len());
        let count = u64::from_le_bytes(bytes[toc..toc + 8].try_into().unwrap()) as usize;
        let names: Vec<String> = (0..count)
            .map(|i| {
                let at = toc + 8 + i * 32;
                String::from_utf8(bytes[at..at + 16].iter().copied().take_while(|b| *b != 0).collect()).unwrap()
            })
            .collect();
        assert_eq!(names, ["TOKENS", "STRINGS", "FIELDS", "FIELDSETS", "PATHS", "SPECS"]);
        assert_eq!(bytes.len(), toc + 8 + count * 32, "the table of contents ends the file");
        assert_eq!(parse(&bytes, "t.usdc").unwrap().root_children, ["P"]);
    }

    #[test]
    fn an_empty_layer_writes_and_reads_back() {
        // No fields at all: the section tables are empty and still decode.
        let empty = Layer { id: "t.usdc".into(), ..Default::default() };
        assert_same_layer("an empty layer", &through_the_crate("an empty layer", &empty), &empty);
        let meta = Layer { id: "t.usdc".into(), meta: vec![("upAxis".into(), Value::Token("Z".into()))], ..Default::default() };
        assert_same_layer("a layer of metadata", &through_the_crate("a layer of metadata", &meta), &meta);
    }

    #[test]
    fn usdc_round_trips_every_layer_we_write() {
        let layers = layers_we_write();
        assert_eq!(layers.len(), super::super::fixtures::all().len() * 2, "every fixture, in both write modes");
        for (name, layer) in &layers {
            let back = through_the_crate(name, layer);
            assert!(back.warnings.is_empty(), "{name}: {:?}", back.warnings);
            assert_same_layer(name, &back, &as_a_crate_holds(layer.clone()));
        }
    }

    #[test]
    fn usdc_round_trips_the_layers_pixars_writer_encoded() {
        // The text of `comp`, `typed` and `stress` has what no fixture of ours
        // does: sublayers with offsets, references and payloads (one with no
        // prim path), inherits, variant sets and their selections, `over` and
        // `class` prims, quaternions, matrices, assets, time samples and
        // dictionaries of every type.
        let comp = usda_layer(std::str::from_utf8(include_bytes!("testdata/usd/comp.usda")).unwrap());
        let typed = usda_layer(std::str::from_utf8(include_bytes!("testdata/usd/typed.usda")).unwrap());
        // The text reader keeps a `half` as the decimal it reads, a crate holds the half.
        let stress = halves_rounded(usda_layer(std::str::from_utf8(include_bytes!("testdata/usd/stress.usda")).unwrap()));
        for (name, layer) in [("comp", comp), ("typed", typed), ("stress", stress)] {
            assert_same_layer(name, &through_the_crate(name, &layer), &layer);
        }
    }

    #[test]
    fn usdc_output_is_deterministic() {
        // Two parses make two `Layer`s whose hash maps iterate in different
        // orders, so a writer that lets one decide an order differs here.
        let (first, second) = (layers_we_write(), layers_we_write());
        for ((name, a), (_, b)) in first.iter().zip(&second) {
            let (x, y) = (write(a).unwrap(), write(b).unwrap());
            assert!(looks_like_crate(&x), "{name}: what was written is not a crate");
            assert!(x == y, "{name}: two writes of the same layer differ ({} and {} bytes)", x.len(), y.len());
            assert!(x == write(a).unwrap(), "{name}: one layer written twice differs");
        }
    }

    #[test]
    fn nested_dictionaries_keep_their_types() {
        let text = r#"#usda 1.0
(
    customLayerData = {
        string s = "str"
        token t = "tok"
        dictionary one = {
            token t = "inner tok"
            string s = "inner str"
            int i = -7
            int64 big = 5000000000
            double d = 0.1
            bool flag = 1
            asset a = @x.png@
            dictionary two = {
                string[] ss = ["a", "b"]
                token[] ts = ["c", "d"]
                int[] ints = [1, 2, 3]
                double[] ds = [0.1, 0.25]
                dictionary three = { string deep = "bottom" }
                dictionary hollow = { }
            }
        }
        float3 v = (1, 2, 3)
        dictionary hollow = { }
    }
)

def Xform "P" (
    customData = {
        dictionary litegen = {
            token type = "spot"
            string name = "the spot"
            color3f color = (0.5, 0.25, 1)
            float intensity = 2.5
        }
    }
)
{
}
"#;
        let layer = usda_layer(text);
        let back = through_the_crate("nested dictionaries", &layer);
        assert_same_layer("nested dictionaries", &back, &layer);
        // The crate holds the types itself: a token and a string stay two
        // types, at every depth.
        let get = |v: &Value, path: &[&str]| -> Value {
            let mut at = v.clone();
            for key in path {
                at = at.dict().and_then(|d| d.iter().find(|(k, _)| k == key)).unwrap_or_else(|| panic!("no {key} in {at:?}")).1.clone();
            }
            at
        };
        let data = back.meta("customLayerData").unwrap();
        assert_eq!(get(data, &["s"]), Value::Str("str".into()));
        assert_eq!(get(data, &["t"]), Value::Token("tok".into()));
        assert_eq!(get(data, &["one", "t"]), Value::Token("inner tok".into()));
        assert_eq!(get(data, &["one", "s"]), Value::Str("inner str".into()));
        assert_eq!(get(data, &["one", "i"]), Value::Int(-7));
        assert_eq!(get(data, &["one", "big"]), Value::Int(5_000_000_000));
        assert_eq!(get(data, &["one", "d"]), Value::Float(0.1));
        assert_eq!(get(data, &["one", "flag"]), Value::Bool(true));
        assert_eq!(get(data, &["one", "a"]), Value::Asset("x.png".into()));
        assert_eq!(get(data, &["one", "two", "ss"]), Value::Strs(vec!["a".into(), "b".into()]));
        assert_eq!(get(data, &["one", "two", "ts"]), Value::Tokens(vec!["c".into(), "d".into()]));
        assert_eq!(get(data, &["one", "two", "ints"]), Value::Ints(vec![1, 2, 3]));
        assert_eq!(get(data, &["one", "two", "ds"]), Value::Doubles(vec![0.1, 0.25]));
        assert_eq!(get(data, &["one", "two", "three", "deep"]), Value::Str("bottom".into()));
        assert_eq!(get(data, &["one", "two", "hollow"]), Value::Dict(Vec::new()));
        assert_eq!(get(data, &["hollow"]), Value::Dict(Vec::new()));
        assert_eq!(get(data, &["v"]), Value::Tuple(vec![1.0, 2.0, 3.0]));
        let lit = back.specs["/P"].custom_data.clone().map(Value::Dict).unwrap();
        assert_eq!(get(&lit, &["litegen", "type"]), Value::Token("spot".into()));
        assert_eq!(get(&lit, &["litegen", "name"]), Value::Str("the spot".into()));
        assert_eq!(get(&lit, &["litegen", "color"]), Value::Tuple(vec![0.5, 0.25, 1.0]));
        assert_eq!(get(&lit, &["litegen", "intensity"]), Value::Float(2.5));
        // The entries keep the order the layer gave them.
        let keys: Vec<&str> = back.meta("customLayerData").and_then(Value::dict).unwrap().iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["s", "t", "one", "v", "hollow"]);
    }

    #[test]
    fn defaults_beside_samples_survive() {
        let text = r#"#usda 1.0
def Xform "P"
{
    float x = 2
    float x.timeSamples = { 0: 1, 1: 3 }
    uniform token t = "a"
    token t.timeSamples = { 0: "b", 4: "c" }
    float3[] v = [(1, 2, 3)]
    float3[] v.timeSamples = { 0: [(1, 2, 3)], 2: [(4, 5, 6), (7, 8, 9)] }
    double y.timeSamples = { 5: 7.5 }
    float blocked = None
    float blocked.timeSamples = { 1: None, 2: 4 }
    double z = 1.5 (
        interpolation = "constant"
        elementSize = 1
        doc = "kept"
    )
    double z.timeSamples = { 0: 0.5 }
}
"#;
        let layer = usda_layer(text);
        let back = through_the_crate("defaults beside samples", &layer);
        assert_same_layer("defaults beside samples", &back, &layer);
        let p = &back.specs["/P"];
        let at = |name: &str| p.attr(name).unwrap_or_else(|| panic!("no {name}"));
        let samples = |pairs: &[(f64, Value)]| Some(pairs.to_vec());
        assert_eq!(at("x").default, Some(Value::Float(2.0)));
        assert_eq!(at("x").samples, samples(&[(0.0, Value::Float(1.0)), (1.0, Value::Float(3.0))]));
        assert_eq!(at("t").default, Some(Value::Token("a".into())));
        assert_eq!(at("t").variability, Variability::Uniform, "the second statement does not undo `uniform`");
        assert_eq!(at("t").samples, samples(&[(0.0, Value::Token("b".into())), (4.0, Value::Token("c".into()))]));
        assert_eq!(at("v").default, Some(Value::Floats(vec![1.0, 2.0, 3.0])));
        assert_eq!(at("v").samples, samples(&[(0.0, Value::Floats(vec![1.0, 2.0, 3.0])), (2.0, Value::Floats(vec![4.0, 5.0, 6.0, 7.0, 8.0, 9.0]))]));
        assert_eq!(at("y").default, None, "samples alone are not a default");
        assert_eq!(at("y").samples, samples(&[(5.0, Value::Float(7.5))]));
        assert_eq!(at("blocked").default, Some(Value::Block), "a blocked default is not no default");
        assert_eq!(at("blocked").samples, samples(&[(1.0, Value::Block), (2.0, Value::Float(4.0))]));
        assert_eq!(at("z").default, Some(Value::Float(1.5)));
        assert_eq!(at("z").samples, samples(&[(0.0, Value::Float(0.5))]));
        assert_eq!((at("z").interpolation.as_deref(), at("z").element_size), (Some("constant"), Some(1)));
        assert_eq!(at("z").metadata, vec![("documentation".to_string(), Value::Str("kept".into()))]);
    }

    /// One attribute on one prim, the smallest layer that holds it.
    fn one_attribute(type_name: &str, variability: Variability, default: Option<Value>, samples: Option<Vec<(f64, Value)>>) -> Layer {
        let attr = super::super::usd_stage::AttrSpec { type_name: type_name.into(), variability, default, samples, ..Default::default() };
        let prim = PrimSpec { props: vec![("a".into(), super::super::usd_stage::Prop::Attr(attr))], ..Default::default() };
        Layer { id: "t.usdc".into(), root_children: vec!["P".into()], specs: [("/P".to_string(), prim)].into(), ..Default::default() }
    }

    /// What a value is, as far as the crate's types are concerned: the
    /// variant, and a tuple's width.
    fn kind_of(v: &Value) -> String {
        match v {
            Value::Tuple(t) => format!("Tuple({})", t.len()),
            other => format!("{other:?}").split(['(', '{']).next().unwrap_or_default().to_string(),
        }
    }

    fn kinds_in(v: &Value, out: &mut std::collections::BTreeMap<String, Value>) {
        out.entry(kind_of(v)).or_insert_with(|| v.clone());
        if let Value::Dict(d) = v {
            for (_, x) in d {
                kinds_in(x, out);
            }
        }
    }

    #[test]
    fn usdc_encodes_every_type_we_write() {
        use super::super::usd_stage::Prop;
        let layers = layers_we_write();
        // Every declared type, with the first attribute that has it; every
        // kind of value in metadata and dictionaries, with the first example.
        let mut types: std::collections::BTreeMap<String, AttrSpecExample> = std::collections::BTreeMap::new();
        let mut kinds: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
        type AttrSpecExample = (Variability, Option<Value>, Option<Vec<(f64, Value)>>);
        for (_, layer) in &layers {
            for (_, v) in &layer.meta {
                kinds_in(v, &mut kinds);
            }
            for spec in layer.specs.values() {
                for (_, v) in spec.custom_data.iter().flatten() {
                    kinds_in(v, &mut kinds);
                }
                for (_, prop) in &spec.props {
                    if let Prop::Attr(a) = prop {
                        types.entry(a.type_name.clone()).or_insert_with(|| (a.variability, a.default.clone(), a.samples.clone()));
                        for (_, v) in &a.metadata {
                            kinds_in(v, &mut kinds);
                        }
                    }
                }
            }
        }
        // The enumeration must not shrink unnoticed: these are what the
        // writer's output is made of today.
        for t in [
            "asset", "bool", "color3f", "color3f[]", "float", "float2", "float3", "float3[]", "float4", "float[]", "half3[]", "int", "int[]", "matrix4d", "matrix4d[]",
            "normal3f[]", "point3f[]", "quatf", "quatf[]", "string", "texCoord2f[]", "token", "token[]", "vector3f[]",
        ] {
            assert!(types.contains_key(t), "no fixture's layer holds a {t} (the types held: {:?})", types.keys().collect::<Vec<_>>());
        }
        for k in ["Dict", "Float", "Str", "Token", "Tuple(3)"] {
            assert!(kinds.contains_key(k), "no metadata or dictionary holds a {k} (the kinds held: {:?})", kinds.keys().collect::<Vec<_>>());
        }
        for (type_name, (variability, default, samples)) in types {
            let layer = one_attribute(&type_name, variability, default, samples);
            assert_same_layer(&format!("a {type_name} attribute"), &through_the_crate(&type_name, &layer), &as_a_crate_holds(layer.clone()));
        }
        for (kind, value) in kinds {
            let meta = vec![("customLayerData".to_string(), Value::Dict(vec![("k".to_string(), value)]))];
            let layer = Layer { id: "t.usdc".into(), meta, ..Default::default() };
            assert_same_layer(&format!("a {kind} in a dictionary"), &through_the_crate(&kind, &layer), &layer);
        }
    }

    /// Each crate type the writer knows, in the forms that take different paths
    /// through it (no fixture holds them all): a scalar and an array, an empty
    /// array, a value that fits its `ValueRep` and one that does not, and a
    /// blocked value; each as a default and as time samples.
    #[test]
    fn usdc_round_trips_each_type_in_each_form() {
        let t = |v: &[f64]| Value::Tuple(v.to_vec());
        let ints = |v: &[i64]| Value::Ints(v.to_vec());
        let f = |v: &[f32]| Value::Floats(v.to_vec());
        let d = |v: &[f64]| Value::Doubles(v.to_vec());
        let strs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        let tenth = f64::from(0.1f32);
        let eye3 = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let eye4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let moved = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 5.0, 6.0, 7.0, 1.0];
        let cases: Vec<(&str, Value)> = vec![
            ("bool", Value::Bool(true)),
            ("bool", Value::Bool(false)),
            ("uchar", Value::Int(255)),
            ("int", Value::Int(-7)),
            ("int", Value::Int(i64::from(i32::MIN))),
            ("uint", Value::Int(4_000_000_000)),
            ("int64", Value::Int(-5)),
            ("int64", Value::Int(1 << 40)),
            ("int64", Value::Int(i64::MIN)),
            ("uint64", Value::Int(7)),
            ("uint64", Value::Int(1 << 40)),
            ("uint64", Value::Int(i64::MAX)),
            ("half", Value::Float(0.5)),
            ("float", Value::Float(-3.0)),
            ("float", Value::Float(tenth)),
            ("double", Value::Float(0.5)),
            ("double", Value::Float(0.1)),
            ("double", Value::Float(1e300)),
            ("half2", t(&[0.5, -2.0])),
            ("half3", t(&[1.0, 2.0, 0.5])),
            ("half4", t(&[1.0, 2.0, 3.0, 4.0])),
            ("float2", t(&[1.0, -2.0])),
            ("float2", t(&[0.5, 2.0])),
            ("float3", t(&[1.0, 2.0, 3.0])),
            ("float3", t(&[1.0, 2.0, 3.5])),
            ("float3", t(&[200.0, 0.0, 0.0])),
            ("float4", t(&[-128.0, 127.0, 0.0, 1.0])),
            ("double3", t(&[1.0, 2.0, 0.1])),
            ("double4", t(&[0.0, 0.0, 0.0, 1.0])),
            ("int2", t(&[1000.0, 2.0])),
            ("int3", t(&[1.0, -2.0, 3.0])),
            ("int4", t(&[1.0, 2.0, 3.0, 4_000_000.0])),
            ("matrix2d", t(&[2.0, 0.0, 0.0, 3.0])),
            ("matrix2d", t(&[1.0, 2.0, 3.0, 4.0])),
            ("matrix3d", t(&eye3)),
            ("matrix3d", t(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.5])),
            ("matrix4d", t(&eye4)),
            ("matrix4d", t(&moved)),
            ("matrix4d", t(&[0.5, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 1.0])),
            ("quatd", t(&[0.0, 0.0, 0.0, 1.0])),
            ("quatf", t(&[0.0, 0.5, 0.0, 0.5])),
            ("quath", t(&[0.5, 0.5, 0.5, 0.5])),
            ("string", Value::Str(String::new())),
            ("string", Value::Str("héllo wörld".into())),
            ("token", Value::Token("x".into())),
            ("asset", Value::Asset("a.png".into())),
            ("asset", Value::Asset(String::new())),
            ("bool[]", ints(&[0, 1, 1])),
            ("uchar[]", ints(&[0, 255])),
            ("int[]", ints(&[-1, 2])),
            ("uint[]", ints(&[0, 4_000_000_000])),
            ("int64[]", ints(&[i64::MIN, 0, i64::MAX])),
            ("uint64[]", ints(&[0, i64::MAX])),
            ("half[]", f(&[0.5, 1.0])),
            ("float[]", f(&[0.1, 2.0])),
            ("double[]", d(&[0.1, 1e300])),
            ("half2[]", f(&[1.0, 2.0, 3.0, 4.0])),
            ("float3[]", f(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])),
            ("point3f[]", f(&[0.1, 0.2, 0.3])),
            ("double3[]", d(&[1.0, 2.0, 0.1])),
            ("int2[]", ints(&[1, 2, 3, 4])),
            ("int4[]", ints(&[1, 2, 3, 4])),
            ("matrix2d[]", d(&[1.0, 0.0, 0.0, 1.0])),
            ("matrix3d[]", d(&eye3)),
            ("matrix4d[]", d(&[eye4, moved].concat())),
            ("quatf[]", f(&[0.0, 0.0, 0.0, 1.0, 0.5, 0.5, 0.5, 0.5])),
            ("quatd[]", d(&[0.0, 0.0, 0.0, 1.0])),
            ("string[]", Value::Strs(strs(&["a", "b", "a"]))),
            ("token[]", Value::Tokens(strs(&["c", "d"]))),
            ("asset[]", Value::Assets(strs(&["x.png", "y.png"]))),
            ("int[]", ints(&[])),
            ("float3[]", f(&[])),
            ("double[]", d(&[])),
            ("token[]", Value::Tokens(Vec::new())),
            ("string[]", Value::Strs(Vec::new())),
            ("asset[]", Value::Assets(Vec::new())),
            ("float", Value::Block),
            ("float3[]", Value::Block),
            ("token", Value::Block),
        ];
        for (type_name, value) in cases {
            let what = format!("{type_name} {value:?}");
            let layer = one_attribute(type_name, Variability::Varying, Some(value.clone()), None);
            assert_same_layer(&format!("a default of {what}"), &through_the_crate(&what, &layer), &layer);
            let samples = vec![(0.0, value.clone()), (2.5, value)];
            let layer = one_attribute(type_name, Variability::Uniform, None, Some(samples));
            assert_same_layer(&format!("samples of {what}"), &through_the_crate(&what, &layer), &layer);
        }
    }

    /// The writer inlines in the `ValueRep` what OpenUSD's `_EncodeInline`
    /// does, and nothing it does not: a reader takes either, so this is the
    /// size of the file and its likeness to `usdcat`'s. Each case: the type,
    /// the value, whether it is inlined.
    #[test]
    fn values_are_inlined_where_openusd_inlines_them() {
        let t = |v: &[f64]| Value::Tuple(v.to_vec());
        let cases: Vec<(&str, Value, bool)> = vec![
            ("bool", Value::Bool(true), true),
            ("int", Value::Int(-7), true),
            ("float", Value::Float(0.1), true),
            ("half", Value::Float(0.1), true),
            ("token", Value::Token("t".into()), true),
            ("string", Value::Str("s".into()), true),
            ("asset", Value::Asset("a".into()), true),
            ("double", Value::Float(0.5), true),
            ("double", Value::Float(0.1), false),
            ("int64", Value::Int(i64::from(i32::MAX)), true),
            ("int64", Value::Int(i64::from(i32::MAX) + 1), false),
            ("uint64", Value::Int(i64::from(u32::MAX)), true),
            ("uint64", Value::Int(i64::from(u32::MAX) + 1), false),
            ("half2", t(&[0.1, 0.2]), true),
            ("float3", t(&[1.0, -128.0, 127.0]), true),
            ("float3", t(&[1.0, 128.0, 0.0]), false),
            ("float3", t(&[1.0, 0.5, 0.0]), false),
            ("double3", t(&[1.0, 2.0, 3.0]), true),
            ("int3", t(&[1.0, 2.0, 3.0]), true),
            ("matrix4d", t(&[2.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 1.0]), true),
            ("matrix4d", t(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0]), false),
            ("matrix3d", t(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]), true),
            ("quatf", t(&[0.0, 0.0, 0.0, 1.0]), false),
            ("float[]", Value::Floats(vec![1.0]), false),
            ("float", Value::Block, true),
        ];
        for (type_name, value, inlined) in cases {
            let (ty, array) = crate_type(type_name).unwrap();
            let mut e = Encoder::new();
            let rep = e.typed(type_name, ty, array, &value).unwrap();
            assert_eq!(rep & REP_INLINED != 0, inlined, "{type_name} {value:?}: {rep:#x}");
        }
        // An empty array is an array with no payload: nothing to point at.
        let mut e = Encoder::new();
        let rep = e.typed("float[]", ty::FLOAT, true, &Value::Floats(Vec::new())).unwrap();
        assert_eq!((rep & REP_ARRAY != 0, rep & REP_INLINED != 0, rep & 0xFFFF_FFFF_FFFF), (true, false, 0));
    }

    #[test]
    fn usdc_refuses_what_it_cannot_encode() {
        let dict = |v: Value| Layer { id: "t.usdc".into(), meta: vec![("customLayerData".into(), Value::Dict(vec![("k".into(), v)]))], ..Default::default() };
        let attr = |type_name: &str, v: Value| one_attribute(type_name, Variability::Varying, Some(v), None);
        let cases: Vec<(&str, Layer, &str)> = vec![
            ("a type no schema has", attr("frobnicator", Value::Int(1)), "frobnicator"),
            ("an array of that type", attr("frobnicator[]", Value::Ints(vec![1])), "frobnicator[]"),
            ("a type that needs a newer crate", attr("timecode", Value::Float(1.0)), "timecode"),
            ("a type with no value", attr("opaque", Value::Int(1)), "opaque"),
            ("a vector holding a scalar", attr("float3", Value::Float(1.0)), "float3"),
            ("a vector of the wrong width", attr("float3", Value::Tuple(vec![1.0, 2.0])), "float3"),
            ("an array that is not whole elements", attr("float3[]", Value::Floats(vec![1.0; 4])), "float3[]"),
            ("an int too large for its type", attr("int", Value::Int(1 << 40)), "int"),
            ("a negative uint", attr("uint", Value::Int(-1)), "uint"),
            ("a bool array holding a 2", attr("bool[]", Value::Ints(vec![1, 2])), "bool[]"),
            ("a uchar array holding 300", attr("uchar[]", Value::Ints(vec![300])), "uchar[]"),
            ("a uint array holding a negative", attr("uint[]", Value::Ints(vec![-1])), "uint[]"),
            ("a half array that is not a float array", attr("half[]", Value::Doubles(vec![1.0])), "half[]"),
            ("a string where a token goes", attr("token", Value::Str("x".into())), "token"),
            ("a path in a dictionary", dict(Value::Path("/A".into())), "Path"),
            ("an untyped list in a dictionary", dict(Value::List(vec![Value::Int(1), Value::Str("a".into())])), "List"),
            ("a 4-tuple in a dictionary, which is a vector, a quaternion or a matrix", dict(Value::Tuple(vec![1.0; 4])), "Tuple(4)"),
            ("a NUL in a string", dict(Value::Str("a\0b".into())), "NUL"),
            ("a dictionary key twice", dict(Value::Dict(vec![("a".into(), Value::Int(1)), ("a".into(), Value::Int(2))])), "twice"),
        ];
        for (what, layer, named) in cases {
            match write(&layer) {
                Err(MeshError::Unsupported(m)) => assert!(m.contains(named), "{what}: the error does not name {named}: {m}"),
                other => panic!("{what}: expected a refusal naming {named}, got {:?}", other.map(|b| b.len())),
            }
        }
        // A spec nothing reaches is not dropped silently.
        let orphan = Layer { id: "t.usdc".into(), specs: [("/Orphan".to_string(), PrimSpec::default())].into(), ..Default::default() };
        assert!(matches!(write(&orphan), Err(MeshError::Malformed(m)) if m.contains("/Orphan")));
    }

    /// A finite number past what its type holds is refused, as an integer
    /// that does not fit is, instead of going into the crate as an infinity
    /// the layer never said: a half from 65,520 on (what rounds past 65,504)
    /// as a scalar, a vector's component or an array's element, and a float
    /// past f32::MAX (the 2026-11-10 follow-up of Task 1's review). What the
    /// types hold is kept: their largest finite values, what rounds to them,
    /// and an infinity or a NaN the layer says itself.
    #[test]
    fn usdc_refuses_a_number_past_its_type() {
        let attr = |type_name: &str, v: Value| one_attribute(type_name, Variability::Varying, Some(v), None);
        let t = |c: &[f64]| Value::Tuple(c.to_vec());
        let cases: Vec<(&str, Layer, &str)> = vec![
            ("a half", attr("half", Value::Float(70_000.0)), "70000"),
            ("a half that rounds to infinity", attr("half", Value::Float(65_520.0)), "65520"),
            ("a negative half", attr("half", Value::Float(-1e6)), "-1000000"),
            ("a float", attr("float", Value::Float(1e39)), "float"),
            ("a half2 component", attr("half2", t(&[70_000.0, 0.0])), "half2"),
            ("a half3 component", attr("half3", t(&[1.0, 2.0, 70_000.0])), "half3"),
            ("a float3 component", attr("float3", t(&[1e39, 0.5, 0.0])), "float3"),
            ("a half[] element", attr("half[]", Value::Floats(vec![1.0, 70_000.0])), "half[]"),
            ("a half3[] element", attr("half3[]", Value::Floats(vec![1.0, 2.0, 3.0, 4.0, 5.0, 1e5])), "half3[]"),
        ];
        for (what, layer, named) in cases {
            match write(&layer) {
                Err(MeshError::Unsupported(m)) => assert!(m.contains(named) && m.contains("range"), "{what}: the error does not name {named}: {m}"),
                other => panic!("{what}: expected a refusal naming {named}, got {:?}", other.map(|b| b.len())),
            }
        }
        for (type_name, v) in [
            ("half", Value::Float(65_504.0)),
            ("half", Value::Float(-65_519.0)),
            ("half", Value::Float(f64::INFINITY)),
            ("half", Value::Float(f64::NAN)),
            ("float", Value::Float(f64::from(f32::MAX))),
            ("float", Value::Float(f64::NEG_INFINITY)),
            ("half3", t(&[65_504.0, 1.0, f64::INFINITY])),
            ("half[]", Value::Floats(vec![65_504.0, f32::INFINITY])),
        ] {
            write(&attr(type_name, v.clone())).unwrap_or_else(|e| panic!("{type_name} {v:?} is kept: {e}"));
        }
    }

    /// Layers built by hand that a crate cannot keep, each refused with an
    /// error of the right kind that names what is wrong: a layer the text
    /// reader cannot produce but a caller can build. For each case, the
    /// regression it would catch:
    /// - times going backwards, or the same time twice: a crate whose
    ///   samples are not in order, which OpenUSD's reader takes as undefined
    ///   (the ordering check dropped or loosened to `>`);
    /// - a NaN or an infinite time: the same, since `NaN >= x` is false and
    ///   the ordering check alone lets it through (the finiteness check dropped);
    /// - a listed child with no spec: a `primChildren` that names a prim with
    ///   nothing behind it (the lookup's error turned into a skip);
    /// - a prim listed twice, so two specs on one path: a crate with two specs
    ///   for one path (the duplicate check dropped);
    /// - a path over `MAX_PATH_LEN`: a crate the reader refuses to read back
    ///   (the length check dropped);
    /// - a relationship target such as `.rel[/B]`: a path in a form the
    ///   writer's path table does not hold (the canonical-form check dropped,
    ///   so the target would be written as some other path).
    #[test]
    fn usdc_refuses_layers_it_cannot_keep() {
        let sampled = |samples: Vec<(f64, Value)>| one_attribute("float", Variability::Varying, None, Some(samples));
        let at = |t: f64| (t, Value::Float(1.0));
        let prims = |children: &[&str], specs: &[&str]| Layer {
            id: "t.usdc".into(),
            root_children: children.iter().map(|c| c.to_string()).collect(),
            specs: specs.iter().map(|p| (p.to_string(), PrimSpec::default())).collect(),
            ..Default::default()
        };
        let long = "P".repeat(MAX_PATH_LEN);
        let mut too_long = prims(&[long.as_str()], &[]);
        too_long.specs.insert(format!("/{long}"), PrimSpec::default());
        let target = RelSpec { targets: Some(ListOp::explicit(vec!["/P.rel[/B]".to_string()])) };
        let mut related = prims(&["P"], &[]);
        related.specs.insert("/P".into(), PrimSpec { props: vec![("rel".into(), super::super::usd_stage::Prop::Rel(target))], ..Default::default() });

        // (what, layer, malformed or unsupported, a word the message has)
        let cases: Vec<(&str, Layer, bool, &str)> = vec![
            ("times going backwards", sampled(vec![at(1.0), at(0.5)]), false, "increasing"),
            ("the same time twice", sampled(vec![at(1.0), at(1.0)]), false, "increasing"),
            ("a NaN time", sampled(vec![at(f64::NAN)]), false, "/P.a"),
            ("a NaN time after a good one", sampled(vec![at(0.0), at(f64::NAN)]), false, "/P.a"),
            ("an infinite time", sampled(vec![at(0.0), at(f64::INFINITY)]), false, "/P.a"),
            ("a listed child with no spec", prims(&["Ghost"], &[]), true, "Ghost"),
            ("two specs on one path", prims(&["A", "A"], &["/A"]), true, "two specs"),
            ("a path over MAX_PATH_LEN", too_long, false, "longer"),
            ("a relationship target such as .rel[/B]", related, false, ".rel[/B]"),
        ];
        for (what, layer, malformed, word) in cases {
            match write(&layer) {
                Err(MeshError::Malformed(m)) if malformed => assert!(m.contains(word), "{what}: the error does not name {word:?}: {m}"),
                Err(MeshError::Unsupported(m)) if !malformed => assert!(m.contains(word), "{what}: the error does not name {word:?}: {m}"),
                other => panic!("{what}: expected {} naming {word:?}, got {:?}", if malformed { "Malformed" } else { "Unsupported" }, other.map(|b| b.len())),
            }
        }
    }
}
