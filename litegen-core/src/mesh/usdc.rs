//! USDC — USD's binary "crate" encoding — parsed into a [`Layer`].
//!
//! # Why read crate at all
//!
//! Apple's tools write it: `usdzip --arkitAsset`, `usdcrush`, Reality
//! Converter and every iOS export produce a usdz whose root layer is a crate
//! file, and so do Blender's and most DCCs' USD exporters. A USD reader that
//! only understands text reads none of what vendors and users actually ship.
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

use super::usd_stage::{Layer, ListOp, PrimSpec, Reference, RelSpec, Specifier, Value, MAX_LIST_OP_ITEMS, MAX_PATH_LEN};
use super::usda::MAX_VALUES;
use super::{MeshError, MeshFormat};

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
    paths: Vec<Option<String>>,
    values: usize,
}

/// A decoded field: its name and value.
type Field = (String, Value);

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

    let mut cr = Crate { b: bytes, minor, tokens: Vec::new(), strings: Vec::new(), paths: Vec::new(), values: 0 };

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
    // Properties arrive in any order; they attach to their prim after, and are
    // then put in the prim's authored `properties` order.
    let mut props: Vec<(String, String, i64, Vec<Field>)> = Vec::new();
    let mut orders: Vec<(String, Vec<String>)> = Vec::new();
    let mut variant_sets: Vec<(String, String, Vec<String>)> = Vec::new();
    for (path_index, fieldset, spec_type) in specs {
        let path = cr.paths[path_index].clone().ok_or_else(|| malformed("a spec's path was never decoded"))?;
        let mut fields_here: Vec<Field> = Vec::new();
        let mut i = fieldset;
        loop {
            let f = *fieldsets.get(i).ok_or_else(|| malformed("a field set runs past its table"))?;
            if f < 0 || f == 0xFFFF_FFFF {
                break;
            }
            let (tok, rep) = *fields.get(f as usize).ok_or_else(|| malformed("a field set names a field that does not exist"))?;
            let name = cr.tokens[tok as usize].clone();
            let v = cr.value(rep, 0)?;
            fields_here.push((name, v));
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
    Ok(layer)
}

fn strs(v: &Value) -> Vec<String> {
    match v {
        Value::Strs(s) | Value::Assets(s) => s.clone(),
        Value::Str(s) => vec![s.clone()],
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
            "interpolation" => a.interpolation = v.as_str().map(str::to_string),
            "elementSize" => a.element_size = v.as_i64(),
            _ => {}
        }
    }
    // Quaternion attributes are stored x, y, z, w — already the stage's order.
    Ok(a)
}

/// Decode the PATHS tree: a pre-order walk where each entry carries its
/// element token and a jump saying whether a child, a sibling or both follow.
fn build_paths(tokens: &[String], total: usize, idx: &[i64], elem: &[i64], jumps: &[i64]) -> Result<Vec<Option<String>>, MeshError> {
    let mut paths: Vec<Option<String>> = vec![None; total];
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
            paths[slot] = Some(path.clone());
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
        self.paths.get(i as usize).cloned().flatten().ok_or_else(|| malformed("a value names a path that does not exist"))
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
            ty::TOKEN => Value::Str(self.token(v as u64)?),
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
            ty::TOKEN => Value::Str(self.token(c.u32()? as u64)?),
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
                Value::Strs(v)
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
            Some(Some(p)) if p != "/" => Ok(p.clone()),
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
                Ok(Value::Strs(v))
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
        ty::TOKEN | ty::STRING => Value::Strs(Vec::new()),
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
    fn a_path_tree_that_loops_is_refused() {
        let tokens = vec!["".to_string(), "A".to_string()];
        // Entry 1 claims a sibling at 1 + (-1)... jumps of 0 mean "sibling
        // next"; make entry 1 jump back onto entry 0.
        let r = build_paths(&tokens, 2, &[0, 1], &[0, 1], &[-1, 0]);
        assert!(r.is_err(), "{r:?}");
    }
}
