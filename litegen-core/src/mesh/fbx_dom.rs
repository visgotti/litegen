//! FBX's document model: a tree of named records, each carrying a list of
//! typed properties — read from either of FBX's two encodings, and written in
//! the binary one.
//!
//! # One tree, two encodings
//!
//! A binary FBX and an ASCII FBX are the same document spelled differently:
//! `Model: 1234, "Model::Hips", "LimbNode" { … }` in text is a record named
//! `Model` with an int64, a string and a string property and some children in
//! binary. Everything above this file reads records and properties and never
//! learns which encoding they came from, so Maya's ASCII exports and the SDK's
//! binary ones go through exactly the same semantic reader.
//!
//! # Why the tree is lazy
//!
//! The record STRUCTURE (names, nesting, where each property list lives) is
//! parsed up front, with every offset checked; the PROPERTIES are decoded only
//! when the semantic reader asks for them, and arrays only when it asks for
//! their values. A 100 MB vendor file is mostly arrays the reader needs once
//! (vertices, indices) plus arrays it never needs at all (edges, smoothing
//! groups, binormals), and a hostile one can describe millions of tiny records:
//! decoding everything eagerly would cost memory on both, for nothing. Records
//! live in one flat arena linked by index, so neither building nor dropping the
//! tree recurses — the nesting depth of an untrusted file is a number its author
//! chose.
//!
//! # Untrusted input
//!
//! Every length and offset is checked against the enclosing record before it is
//! used; record nesting and record counts are capped; an array's declared
//! element count is bounded by [`MAX_DECODED_BYTES`] arithmetic before anything
//! is allocated, compressed arrays inflate with exactly that many bytes as the
//! limit (a zlib bomb stops there), and every decoded array is charged to one
//! budget for the whole document so that a thousand arrays cannot do together
//! what one is not allowed to.
//!
//! @see <https://code.blender.org/2013/08/fbx-binary-file-format-specification/>

use super::MeshError;
use std::borrow::Cow;
use std::cell::Cell;

/// The first 21 bytes of every binary FBX: the text, two spaces and a NUL.
/// The two bytes after it are `1A 00` in every writer we have seen; readers do
/// not check them and neither do we.
pub(super) const BINARY_MAGIC: &[u8] = b"Kaydara FBX Binary  \0";
/// Magic, the two `1A 00` bytes, then the u32 version.
pub(super) const BINARY_HEADER_LEN: usize = 27;

/// Record nesting deeper than any writer produces (real files nest six
/// levels: `Objects > Geometry > LayerElement > …`). The cap is what keeps a
/// hostile chain of ten million nested records from being a ten-million-frame
/// stack.
const MAX_DEPTH: usize = 64;
/// Records in one document. A 3 MB Mixamo character has ~20k; the largest
/// legitimate files are mostly arrays and have far fewer records than bytes /
/// 100. Four million is far past either and bounds the arena at ~150 MB.
pub(super) const MAX_RECORDS: usize = 4_000_000;
/// Bytes all decoded arrays may occupy together, the decoded-value budget of
/// `glb.rs` restated for a format whose arrays are doubles: twice the input
/// cap, which no honest file reaches — a 128 MB FBX of uncompressed doubles
/// decodes to exactly 128 MB.
pub(super) const MAX_DECODED_BYTES: usize = 2 * super::MAX_INPUT_BYTES;

/// Arrays at least this large are written zlib-compressed. The SDK's own
/// threshold sits between 939 and 1,072 bytes in every file measured (see the
/// research notes on array encoding); 1 KiB reproduces that choice.
const COMPRESS_THRESHOLD: usize = 1024;

fn malformed(detail: impl Into<String>) -> MeshError {
    MeshError::Malformed(format!("fbx: {}", detail.into()))
}

fn unsupported(detail: impl Into<String>) -> MeshError {
    MeshError::Unsupported(format!("fbx: {}", detail.into()))
}

/// A record in a [`Dom`], by index. Index 0 is a synthetic root whose
/// children are the document's top-level records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct RecId(u32);

pub(super) const ROOT: RecId = RecId(0);
const NONE: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Encoding {
    Binary,
    Ascii,
}

#[derive(Debug, Clone, Copy)]
enum PropSrc<'a> {
    None,
    /// `count` properties packed back to back in `data`.
    Binary { data: &'a [u8], count: usize },
    /// The raw text between the record's `:` and its `{` or line end.
    Text(&'a str),
    /// An ASCII `*N { a: … }` array, folded into one array property.
    TextArray { count: usize, text: &'a str },
}

#[derive(Debug, Clone)]
struct Rec<'a> {
    name: &'a str,
    props: PropSrc<'a>,
    first_child: u32,
    last_child: u32,
    next_sibling: u32,
}

/// One decoded property.
#[derive(Debug, Clone)]
pub(super) enum Prop<'a> {
    Bool(bool),
    /// `Y`, `I`, `L` and ASCII integers.
    Int(i64),
    /// `F`, `D` and ASCII decimals.
    Float(f64),
    /// `S`. Bytes, not text: FBX strings are not NUL-terminated, carry the
    /// `\x00\x01` name/class separator and are UTF-8 only by convention.
    Str(Cow<'a, [u8]>),
    /// `R`: raw bytes (embedded file contents, the file id).
    Raw(&'a [u8]),
    Array(ArrayRef<'a>),
}

impl<'a> Prop<'a> {
    pub(super) fn as_i64(&self) -> Option<i64> {
        match self {
            Prop::Int(v) => Some(*v),
            Prop::Bool(b) => Some(*b as i64),
            // ASCII writers spell some integers with a decimal point.
            Prop::Float(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Some(*f as i64),
            _ => None,
        }
    }

    pub(super) fn as_f64(&self) -> Option<f64> {
        match self {
            Prop::Int(v) => Some(*v as f64),
            Prop::Float(f) => Some(*f),
            Prop::Bool(b) => Some(*b as i64 as f64),
            _ => None,
        }
    }

    pub(super) fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Prop::Str(s) => Some(s),
            Prop::Raw(r) => Some(r),
            _ => None,
        }
    }

    /// The string, decoded leniently: invalid UTF-8 becomes U+FFFD rather than
    /// an error, because a name is never worth refusing a model over.
    pub(super) fn as_str(&self) -> Option<Cow<'_, str>> {
        match self {
            Prop::Str(s) => Some(String::from_utf8_lossy(s)),
            _ => None,
        }
    }

}

/// An array property, not yet decoded.
#[derive(Debug, Clone, Copy)]
pub(super) enum ArrayRef<'a> {
    Binary { kind: u8, count: usize, encoding: u32, payload: &'a [u8] },
    Text { count: usize, text: &'a str },
}

/// A parsed document.
pub(super) struct Dom<'a> {
    pub(super) encoding: Encoding,
    pub(super) version: u32,
    recs: Vec<Rec<'a>>,
    decoded: Cell<usize>,
}

fn elem_size(kind: u8) -> Option<usize> {
    match kind {
        b'b' | b'c' => Some(1),
        b'i' | b'f' => Some(4),
        b'l' | b'd' => Some(8),
        _ => None,
    }
}

fn le_u32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn le_u64(b: &[u8], at: usize) -> Option<u64> {
    let s = b.get(at..at.checked_add(8)?)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Some(u64::from_le_bytes(a))
}

/// Line number of a byte offset, for ASCII error messages.
fn line_of(b: &[u8], pos: usize) -> usize {
    1 + b[..pos.min(b.len())].iter().filter(|c| **c == b'\n').count()
}

impl<'a> Dom<'a> {
    fn new(encoding: Encoding, version: u32) -> Dom<'a> {
        Dom {
            encoding,
            version,
            recs: vec![Rec { name: "", props: PropSrc::None, first_child: NONE, last_child: NONE, next_sibling: NONE }],
            decoded: Cell::new(0),
        }
    }

    fn push(&mut self, parent: u32, name: &'a str, props: PropSrc<'a>) -> Result<u32, MeshError> {
        if self.recs.len() >= MAX_RECORDS {
            return Err(unsupported(format!("the document has more than {MAX_RECORDS} records")));
        }
        let id = self.recs.len() as u32;
        self.recs.push(Rec { name, props, first_child: NONE, last_child: NONE, next_sibling: NONE });
        let p = &mut self.recs[parent as usize];
        if p.last_child == NONE {
            p.first_child = id;
        } else {
            let last = p.last_child as usize;
            p.last_child = id;
            self.recs[last].next_sibling = id;
            return Ok(id);
        }
        p.last_child = id;
        Ok(id)
    }

    pub(super) fn name(&self, r: RecId) -> &'a str {
        self.recs.get(r.0 as usize).map_or("", |x| x.name)
    }

    /// Children in file order.
    pub(super) fn children(&self, r: RecId) -> Children<'_, 'a> {
        let first = self.recs.get(r.0 as usize).map_or(NONE, |x| x.first_child);
        Children { dom: self, next: first }
    }

    /// The first child called `name`.
    pub(super) fn child(&self, r: RecId, name: &str) -> Option<RecId> {
        self.children(r).find(|c| self.name(*c) == name)
    }

    /// Every child called `name`.
    pub(super) fn children_named<'s>(&'s self, r: RecId, name: &'s str) -> impl Iterator<Item = RecId> + 's {
        self.children(r).filter(move |c| self.name(*c) == name)
    }

    /// The record's properties, decoded.
    pub(super) fn props(&self, r: RecId) -> Result<Vec<Prop<'a>>, MeshError> {
        let Some(rec) = self.recs.get(r.0 as usize) else { return Ok(Vec::new()) };
        match rec.props {
            PropSrc::None => Ok(Vec::new()),
            PropSrc::Binary { data, count } => decode_binary_props(data, count, rec.name),
            PropSrc::Text(t) => parse_text_props(t, rec.name),
            PropSrc::TextArray { count, text } => Ok(vec![Prop::Array(ArrayRef::Text { count, text })]),
        }
    }

    /// The first property, when the record is a `Name: value` leaf.
    pub(super) fn first_prop(&self, r: RecId) -> Result<Option<Prop<'a>>, MeshError> {
        Ok(self.props(r)?.into_iter().next())
    }

    /// The value of a `Name: value` child record, if present.
    pub(super) fn child_prop(&self, r: RecId, name: &str) -> Result<Option<Prop<'a>>, MeshError> {
        match self.child(r, name) {
            Some(c) => self.first_prop(c),
            None => Ok(None),
        }
    }

    /// The array held by a `Name: [array]` child record, if present.
    pub(super) fn child_array(&self, r: RecId, name: &str) -> Result<Option<ArrayRef<'a>>, MeshError> {
        match self.child_prop(r, name)? {
            None => Ok(None),
            Some(Prop::Array(a)) => Ok(Some(a)),
            Some(_) => Err(malformed(format!("{} {name} is not an array", self.name(r)))),
        }
    }

    fn charge(&self, bytes: usize, what: &str) -> Result<(), MeshError> {
        let total = self.decoded.get().saturating_add(bytes);
        if total > MAX_DECODED_BYTES {
            return Err(unsupported(format!(
                "decoding {what} would take the document past {MAX_DECODED_BYTES} bytes of array data; \
                 it describes more geometry than this converter will hold"
            )));
        }
        self.decoded.set(total);
        Ok(())
    }

    /// A binary array's element bytes, inflated if compressed.
    fn binary_payload(
        &self,
        kind: u8,
        count: usize,
        encoding: u32,
        payload: &'a [u8],
        what: &str,
    ) -> Result<Cow<'a, [u8]>, MeshError> {
        let size = elem_size(kind).ok_or_else(|| malformed(format!("{what} has array type {:?}", kind as char)))?;
        let bytes = count
            .checked_mul(size)
            .filter(|b| *b <= MAX_DECODED_BYTES)
            .ok_or_else(|| unsupported(format!("{what} declares {count} elements, past the decode limit")))?;
        match encoding {
            0 => {
                if payload.len() != bytes {
                    return Err(malformed(format!(
                        "{what} declares {count} elements ({bytes} bytes) but stores {} bytes",
                        payload.len()
                    )));
                }
                Ok(Cow::Borrowed(payload))
            }
            1 => {
                // Inflating stops at `bytes` (one zlib bomb ends there), and
                // every caller has already charged what the array decodes
                // to, never less than `bytes`, so a thousand bombs exhaust the
                // budget. The inflated buffer itself is transient: charging it
                // as well counted every compressed array twice, and refused
                // our own writer's 500k-triangle files.
                Ok(Cow::Owned(super::fbx::inflate_zlib(payload, bytes, what)?))
            }
            other => Err(malformed(format!("{what} uses array encoding {other}; only 0 (raw) and 1 (zlib) exist"))),
        }
    }

    /// Decode an array as doubles. Float and integer arrays both convert;
    /// FBX writers disagree about which element type some fields use.
    pub(super) fn f64s(&self, a: &ArrayRef<'a>, what: &str) -> Result<Vec<f64>, MeshError> {
        match *a {
            ArrayRef::Binary { kind, count, encoding, payload } => {
                self.charge(count.saturating_mul(8), what)?;
                let raw = self.binary_payload(kind, count, encoding, payload, what)?;
                Ok(match kind {
                    b'd' => raw.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap_or([0; 8]))).collect(),
                    b'f' => raw.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap_or([0; 4])) as f64).collect(),
                    b'i' => raw.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap_or([0; 4])) as f64).collect(),
                    b'l' => raw.chunks_exact(8).map(|c| i64::from_le_bytes(c.try_into().unwrap_or([0; 8])) as f64).collect(),
                    _ => raw.iter().map(|b| *b as f64).collect(),
                })
            }
            ArrayRef::Text { count, text } => {
                self.charge(count.saturating_mul(8), what)?;
                parse_text_numbers(text, count, what, |tok| tok.parse::<f64>().ok().filter(|f| f.is_finite()))
            }
        }
    }

    /// Decode an array as floats, keeping `f` arrays bit for bit.
    pub(super) fn f32s(&self, a: &ArrayRef<'a>, what: &str) -> Result<Vec<f32>, MeshError> {
        match *a {
            ArrayRef::Binary { kind: b'f', count, encoding, payload } => {
                self.charge(count.saturating_mul(4), what)?;
                let raw = self.binary_payload(b'f', count, encoding, payload, what)?;
                Ok(raw.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap_or([0; 4]))).collect())
            }
            _ => Ok(self.f64s(a, what)?.into_iter().map(|v| v as f32).collect()),
        }
    }

    /// Decode `KeyAttrDataFloat`, whose third value per key is not a number
    /// at all but two 16-bit tangent weights packed into a float's bits.
    /// From 7.2 the SDK's ASCII writer therefore prints every value of this
    /// array as the integer holding the float's bits; ufbx reads them back
    /// the same way. A token with a fraction or an exponent (a writer that
    /// does not follow the convention) is read as the float it spells.
    pub(super) fn key_attr_floats(&self, a: &ArrayRef<'a>, what: &str) -> Result<Vec<f32>, MeshError> {
        match *a {
            ArrayRef::Text { count, text } if self.version >= 7200 => {
                self.charge(count.saturating_mul(4), what)?;
                parse_text_numbers(text, count, what, |tok| match tok.parse::<i64>() {
                    Ok(i) => i32::try_from(i)
                        .map(|i| i as u32)
                        .or_else(|_| u32::try_from(i))
                        .ok()
                        .map(f32::from_bits),
                    Err(_) => tok.parse::<f64>().ok().filter(|f| f.is_finite()).map(|f| f as f32),
                })
            }
            _ => self.f32s(a, what),
        }
    }

    /// Decode an integer array. A float array here is an error, not a cast:
    /// indices that arrive as doubles mean the file is not what we think.
    pub(super) fn i64s(&self, a: &ArrayRef<'a>, what: &str) -> Result<Vec<i64>, MeshError> {
        match *a {
            ArrayRef::Binary { kind, count, encoding, payload } => {
                if !matches!(kind, b'i' | b'l' | b'b' | b'c') {
                    return Err(malformed(format!("{what} is a {:?} array where integers belong", kind as char)));
                }
                self.charge(count.saturating_mul(8), what)?;
                let raw = self.binary_payload(kind, count, encoding, payload, what)?;
                Ok(match kind {
                    b'i' => raw.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap_or([0; 4])) as i64).collect(),
                    b'l' => raw.chunks_exact(8).map(|c| i64::from_le_bytes(c.try_into().unwrap_or([0; 8]))).collect(),
                    _ => raw.iter().map(|b| *b as i64).collect(),
                })
            }
            ArrayRef::Text { count, text } => {
                self.charge(count.saturating_mul(8), what)?;
                parse_text_numbers(text, count, what, |tok| tok.parse::<i64>().ok())
            }
        }
    }

    /// Decode an integer array whose values must fit 32 bits.
    pub(super) fn i32s(&self, a: &ArrayRef<'a>, what: &str) -> Result<Vec<i32>, MeshError> {
        self.i64s(a, what)?
            .into_iter()
            .map(|v| i32::try_from(v).map_err(|_| malformed(format!("{what} holds {v}, outside 32 bits"))))
            .collect()
    }
}

pub(super) struct Children<'d, 'a> {
    dom: &'d Dom<'a>,
    next: u32,
}

impl Iterator for Children<'_, '_> {
    type Item = RecId;
    fn next(&mut self) -> Option<RecId> {
        if self.next == NONE {
            return None;
        }
        let id = self.next;
        self.next = self.dom.recs.get(id as usize).map_or(NONE, |r| r.next_sibling);
        Some(RecId(id))
    }
}

// ─── binary ─────────────────────────────────────────────────────────────────

/// Decode a binary property list. `count` is from the file; the loop is
/// bounded by the bytes, not by it.
fn decode_binary_props<'a>(data: &'a [u8], count: usize, rec: &str) -> Result<Vec<Prop<'a>>, MeshError> {
    let bad = |what: &str| malformed(format!("record {rec:?}: property list {what}"));
    let mut out = Vec::with_capacity(count.min(16));
    let mut p = 0usize;
    for _ in 0..count {
        let code = *data.get(p).ok_or_else(|| bad("is shorter than its declared property count"))?;
        p += 1;
        let take = |p: &mut usize, n: usize| -> Result<&'a [u8], MeshError> {
            let s = data.get(*p..p.checked_add(n).ok_or_else(|| bad("overflows"))?).ok_or_else(|| bad("is truncated"))?;
            *p += n;
            Ok(s)
        };
        let prop = match code {
            // 'C' is FBX's bool; 'B' and 'Z' are the one-byte scalars SDK 2020
            // writes for "UByte" values (accepted, never written).
            b'C' | b'B' | b'Z' => Prop::Bool(take(&mut p, 1)?[0] != 0),
            b'Y' => {
                let s = take(&mut p, 2)?;
                Prop::Int(i16::from_le_bytes([s[0], s[1]]) as i64)
            }
            b'I' => Prop::Int(i32::from_le_bytes(take(&mut p, 4)?.try_into().unwrap_or([0; 4])) as i64),
            b'L' => Prop::Int(i64::from_le_bytes(take(&mut p, 8)?.try_into().unwrap_or([0; 8]))),
            b'F' => Prop::Float(f32::from_le_bytes(take(&mut p, 4)?.try_into().unwrap_or([0; 4])) as f64),
            b'D' => Prop::Float(f64::from_le_bytes(take(&mut p, 8)?.try_into().unwrap_or([0; 8]))),
            b'S' | b'R' => {
                let n = le_u32(data, p).ok_or_else(|| bad("is truncated"))? as usize;
                p += 4;
                let s = take(&mut p, n)?;
                if code == b'S' {
                    Prop::Str(Cow::Borrowed(s))
                } else {
                    Prop::Raw(s)
                }
            }
            b'f' | b'd' | b'l' | b'i' | b'b' | b'c' => {
                let count = le_u32(data, p).ok_or_else(|| bad("is truncated"))? as usize;
                let encoding = le_u32(data, p + 4).ok_or_else(|| bad("is truncated"))?;
                let clen = le_u32(data, p + 8).ok_or_else(|| bad("is truncated"))? as usize;
                p += 12;
                let payload = take(&mut p, clen)?;
                Prop::Array(ArrayRef::Binary { kind: code, count, encoding, payload })
            }
            other => return Err(bad(&format!("has unknown type code 0x{other:02x}"))),
        };
        out.push(prop);
    }
    if p != data.len() {
        return Err(bad(&format!("has {} bytes after its last property", data.len() - p)));
    }
    Ok(out)
}

/// Parse a binary FBX's record structure.
pub(super) fn parse_binary(bytes: &[u8]) -> Result<Dom<'_>, MeshError> {
    if bytes.len() < BINARY_HEADER_LEN || !bytes.starts_with(BINARY_MAGIC) {
        return Err(malformed("binary header is truncated"));
    }
    let version = le_u32(bytes, 23).ok_or_else(|| malformed("binary header is truncated"))?;
    // 7.5 widened the three record-header fields to 64 bits (files over 4 GiB).
    let wide = version >= 7500;
    let hdr = if wide { 25 } else { 13 };
    let mut dom = Dom::new(Encoding::Binary, version);
    // (record, end offset of its children)
    let mut stack: Vec<(u32, usize)> = vec![(0, bytes.len())];
    let mut pos = BINARY_HEADER_LEN;
    while let Some(&(parent, end)) = stack.last() {
        if stack.len() > 1 && pos == end {
            // A nested list may end at its parent's end offset without a NULL
            // record; ufbx, Assimp and OpenFBX all accept that, and so do we.
            stack.pop();
            continue;
        }
        if pos.checked_add(hdr).is_none_or(|e| e > end) {
            return Err(malformed(format!(
                "record header at offset {pos} runs past the end of its {}",
                if stack.len() > 1 { "parent record" } else { "file (no top-level NULL record: truncated?)" }
            )));
        }
        let (end_off, nprops, plen) = if wide {
            (
                le_u64(bytes, pos).unwrap_or(0),
                le_u64(bytes, pos + 8).unwrap_or(0),
                le_u64(bytes, pos + 16).unwrap_or(0),
            )
        } else {
            (
                le_u32(bytes, pos).unwrap_or(0) as u64,
                le_u32(bytes, pos + 4).unwrap_or(0) as u64,
                le_u32(bytes, pos + 8).unwrap_or(0) as u64,
            )
        };
        let name_len = bytes[pos + hdr - 1] as usize;
        if end_off == 0 {
            // The NULL record: all-zero header, ends one list.
            if nprops != 0 || plen != 0 || name_len != 0 {
                return Err(malformed(format!("record at offset {pos} has end offset 0 but is not a NULL record")));
            }
            pos += hdr;
            if stack.len() == 1 {
                break; // end of the top-level list; the footer follows
            }
            if pos != end {
                return Err(malformed(format!("NULL record at offset {} is not the last child of its record", pos - hdr)));
            }
            stack.pop();
            continue;
        }
        let end_off = usize::try_from(end_off).map_err(|_| malformed(format!("record at offset {pos} ends past 2^64")))?;
        let name_start = pos + hdr;
        let name_end = name_start + name_len;
        let props_end = usize::try_from(plen)
            .ok()
            .and_then(|l| name_end.checked_add(l))
            .ok_or_else(|| malformed(format!("record at offset {pos} has an absurd property length")))?;
        if end_off > end || end_off <= pos || name_end > end_off || props_end > end_off {
            return Err(malformed(format!(
                "record at offset {pos} declares an end offset of {end_off}, outside its parent (which ends at {end}) \
                 or before its own properties"
            )));
        }
        let name = std::str::from_utf8(&bytes[name_start..name_end])
            .map_err(|_| malformed(format!("record name at offset {name_start} is not text")))?;
        let count = usize::try_from(nprops).map_err(|_| malformed(format!("record {name:?} has an absurd property count")))?;
        // A property is at least two bytes; a count beyond that is a lie the
        // decoder would discover later, but cheaper to reject now.
        if count > (props_end - name_end) {
            return Err(malformed(format!("record {name:?} claims {count} properties in {} bytes", props_end - name_end)));
        }
        let id = dom.push(parent, name, PropSrc::Binary { data: &bytes[name_end..props_end], count })?;
        if props_end < end_off {
            if stack.len() >= MAX_DEPTH {
                return Err(unsupported(format!("records nest deeper than {MAX_DEPTH} levels")));
            }
            stack.push((id, end_off));
            pos = props_end;
        } else {
            pos = end_off;
        }
    }
    Ok(dom)
}

// ─── ASCII ──────────────────────────────────────────────────────────────────

/// Whether these bytes look like an ASCII FBX document: a leading `; FBX`
/// comment (every writer emits it), or a first record named
/// `FBXHeaderExtension`.
pub(super) fn looks_like_ascii(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(64 * 1024)];
    let mut b = head.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(head);
    loop {
        while let Some((c, rest)) = b.split_first() {
            if c.is_ascii_whitespace() {
                b = rest;
            } else {
                break;
            }
        }
        if b.starts_with(b"; FBX") || b.starts_with(b";FBX") {
            return true;
        }
        if b.first() == Some(&b';') {
            match b.iter().position(|c| *c == b'\n') {
                Some(nl) => b = &b[nl + 1..],
                None => return false,
            }
            continue;
        }
        return b.starts_with(b"FBXHeaderExtension:");
    }
}

fn is_name_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'|' | b'-' | b'.')
}

/// Skip whitespace and `;` comments. With `newlines` false, stop at a line
/// end: that is where an ASCII statement ends.
fn skip_space(b: &[u8], pos: &mut usize, newlines: bool) {
    while let Some(&c) = b.get(*pos) {
        match c {
            b' ' | b'\t' => *pos += 1,
            b'\r' | b'\n' if newlines => *pos += 1,
            b';' if newlines => {
                while b.get(*pos).is_some_and(|c| *c != b'\n') {
                    *pos += 1;
                }
            }
            _ => break,
        }
    }
}

/// Parse an ASCII FBX's record structure.
///
/// The grammar, as every writer emits it: `Name: v, v, v` statements, a
/// trailing `{` opening a child list closed by `}`, values continuing onto the
/// next line after a trailing comma (long arrays), `;` comments, and arrays
/// spelled `Name: *N { a: v, v, … }`, which are folded into one array property
/// so the semantic reader sees what the binary encoding would have given it.
pub(super) fn parse_ascii(bytes: &[u8]) -> Result<Dom<'_>, MeshError> {
    let text = std::str::from_utf8(bytes).map_err(|e| {
        malformed(format!("ASCII document is not UTF-8 (line {})", line_of(bytes, e.valid_up_to())))
    })?;
    let b = text.as_bytes();
    let mut dom = Dom::new(Encoding::Ascii, 0);
    let mut stack: Vec<u32> = vec![0];
    let mut pos = if b.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    loop {
        skip_space(b, &mut pos, true);
        let Some(&c) = b.get(pos) else { break };
        if c == b'}' {
            if stack.len() == 1 {
                return Err(malformed(format!("unbalanced '}}' at line {}", line_of(b, pos))));
            }
            pos += 1;
            if let Some(closed) = stack.pop() {
                fold_text_array(&mut dom, closed)?;
            }
            continue;
        }
        let start = pos;
        while b.get(pos).is_some_and(|c| is_name_byte(*c)) {
            pos += 1;
        }
        if pos == start || b.get(pos) != Some(&b':') {
            return Err(malformed(format!("expected `Name:` at line {}", line_of(b, start))));
        }
        let name = &text[start..pos];
        pos += 1;
        let props_start = pos;
        let mut opens = false;
        let props_end;
        loop {
            skip_space(b, &mut pos, false);
            match b.get(pos) {
                None | Some(b'\r') | Some(b'\n') | Some(b'}') | Some(b';') => {
                    props_end = pos;
                    break;
                }
                Some(b'{') => {
                    props_end = pos;
                    opens = true;
                    pos += 1;
                    break;
                }
                Some(b',') => {
                    // A value list continues past a trailing comma, onto the
                    // next line if need be (long arrays, `Content: ,`).
                    pos += 1;
                    skip_space(b, &mut pos, true);
                }
                Some(b'"') => {
                    let close = b[pos + 1..]
                        .iter()
                        .position(|c| *c == b'"')
                        .ok_or_else(|| malformed(format!("unterminated string at line {}", line_of(b, pos))))?;
                    pos += close + 2;
                }
                Some(_) => {
                    while b.get(pos).is_some_and(|c| !matches!(c, b',' | b'{' | b'}' | b' ' | b'\t' | b'\r' | b'\n' | b';' | b'"')) {
                        pos += 1;
                    }
                }
            }
        }
        let parent = *stack.last().unwrap_or(&0);
        let src = if props_start < props_end { PropSrc::Text(&text[props_start..props_end]) } else { PropSrc::None };
        let id = dom.push(parent, name, src)?;
        if opens {
            if stack.len() >= MAX_DEPTH {
                return Err(unsupported(format!("records nest deeper than {MAX_DEPTH} levels")));
            }
            stack.push(id);
        }
    }
    if stack.len() != 1 {
        return Err(malformed("the document ends inside a '{' block"));
    }
    // The version, from the header record (the leading comment is advisory).
    let version = dom
        .child(ROOT, "FBXHeaderExtension")
        .and_then(|h| dom.child(h, "FBXVersion"))
        .and_then(|v| dom.first_prop(v).ok().flatten())
        .and_then(|p| p.as_i64())
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| malformed("ASCII document has no FBXHeaderExtension.FBXVersion"))?;
    dom.version = version;
    Ok(dom)
}

/// Fold `Name: *N { a: … }` into an array property of `Name`.
fn fold_text_array(dom: &mut Dom<'_>, rec: u32) -> Result<(), MeshError> {
    let r = &dom.recs[rec as usize];
    let PropSrc::Text(t) = r.props else { return Ok(()) };
    let t = t.trim();
    let Some(n) = t.strip_prefix('*') else { return Ok(()) };
    let child = r.first_child;
    let only = child != NONE && dom.recs[child as usize].next_sibling == NONE;
    if !only || dom.recs[child as usize].name != "a" || dom.recs[child as usize].first_child != NONE {
        return Err(malformed(format!("array record {:?} does not hold exactly one `a:` list", r.name)));
    }
    let count: usize = n
        .trim()
        .parse()
        .map_err(|_| malformed(format!("array record {:?} has count {:?}", r.name, n)))?;
    let text = match dom.recs[child as usize].props {
        PropSrc::Text(s) => s,
        PropSrc::None => "",
        _ => return Err(malformed(format!("array record {:?} is malformed", r.name))),
    };
    let r = &mut dom.recs[rec as usize];
    r.props = PropSrc::TextArray { count, text };
    r.first_child = NONE;
    r.last_child = NONE;
    Ok(())
}

/// Split ASCII property text into tokens and type them.
fn parse_text_props<'a>(t: &'a str, rec: &str) -> Result<Vec<Prop<'a>>, MeshError> {
    let b = t.as_bytes();
    let mut out = Vec::new();
    let mut pos = 0;
    loop {
        // Separators: commas and any whitespace, newlines included (the
        // structure pass already decided where the statement ends).
        while b.get(pos).is_some_and(|c| matches!(c, b',' | b' ' | b'\t' | b'\r' | b'\n')) {
            pos += 1;
        }
        let Some(&c) = b.get(pos) else { break };
        if c == b'"' {
            let close = b[pos + 1..]
                .iter()
                .position(|c| *c == b'"')
                .ok_or_else(|| malformed(format!("record {rec:?}: unterminated string")))?;
            let s = &t[pos + 1..pos + 1 + close];
            // The SDK escapes a double quote inside a string as `&quot;`.
            let v = if s.contains("&quot;") {
                Cow::Owned(s.replace("&quot;", "\"").into_bytes())
            } else {
                Cow::Borrowed(s.as_bytes())
            };
            out.push(Prop::Str(v));
            pos += close + 2;
            continue;
        }
        let start = pos;
        while b.get(pos).is_some_and(|c| !matches!(c, b',' | b' ' | b'\t' | b'\r' | b'\n' | b'"')) {
            pos += 1;
        }
        let tok = &t[start..pos];
        let numeric = tok.bytes().next().is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.'));
        let prop = if numeric {
            if let Ok(v) = tok.parse::<i64>() {
                Prop::Int(v)
            } else {
                match tok.parse::<f64>() {
                    Ok(f) if f.is_finite() => Prop::Float(f),
                    _ => return Err(malformed(format!("record {rec:?}: {:?} is not a number", truncate(tok, 40)))),
                }
            }
        } else {
            // Bare words: `T`/`Y` booleans, `*N` array markers left unfolded.
            Prop::Str(Cow::Borrowed(tok.as_bytes()))
        };
        out.push(prop);
    }
    Ok(out)
}

/// Parse the values of an ASCII array, which must number exactly `count`.
fn parse_text_numbers<T>(text: &str, count: usize, what: &str, parse: impl Fn(&str) -> Option<T>) -> Result<Vec<T>, MeshError> {
    let mut out = Vec::with_capacity(count.min(text.len() / 2 + 1));
    for tok in text.split(|c: char| c == ',' || c.is_ascii_whitespace()) {
        if tok.is_empty() {
            continue;
        }
        if out.len() == count {
            return Err(malformed(format!("{what} holds more than its declared {count} values")));
        }
        out.push(parse(tok).ok_or_else(|| malformed(format!("{what}: {:?} is not a valid element", truncate(tok, 40))))?);
    }
    if out.len() != count {
        return Err(malformed(format!("{what} declares {count} values but holds {}", out.len())));
    }
    Ok(out)
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Split an object's name property into (name, class).
///
/// Binary FBX writes `"Name\x00\x01Class"`; ASCII writes `"Class::Name"`. Only
/// the second property of an object record follows this convention.
pub(super) fn split_object_name(raw: &[u8], encoding: Encoding) -> (String, String) {
    match encoding {
        Encoding::Binary => match raw.windows(2).position(|w| w == [0, 1]) {
            Some(i) => (
                String::from_utf8_lossy(&raw[..i]).into_owned(),
                String::from_utf8_lossy(&raw[i + 2..]).into_owned(),
            ),
            None => (String::from_utf8_lossy(raw).into_owned(), String::new()),
        },
        Encoding::Ascii => {
            let s = String::from_utf8_lossy(raw);
            match s.find("::") {
                Some(i) => (s[i + 2..].to_string(), s[..i].to_string()),
                None => (s.into_owned(), String::new()),
            }
        }
    }
}

// ─── writer ─────────────────────────────────────────────────────────────────

/// A property to write.
#[derive(Debug, Clone)]
pub(super) enum WProp {
    /// `C`, the one-byte bool. Its byte is written as given: the SDK writes
    /// `'T'`/`'Y'`, Blender and Assimp `1`.
    Byte(u8),
    I32(i32),
    I64(i64),
    F64(f64),
    Str(Vec<u8>),
    Raw(Vec<u8>),
    ArrF32(Vec<f32>),
    ArrF64(Vec<f64>),
    ArrI32(Vec<i32>),
    ArrI64(Vec<i64>),
}

/// A record to write.
#[derive(Debug, Clone)]
pub(super) struct WRec {
    pub(super) name: &'static str,
    pub(super) props: Vec<WProp>,
    pub(super) children: Vec<WRec>,
    /// Terminate with a NULL record even without children (see
    /// [`WRec::needs_null`]).
    pub(super) force_null: bool,
}

impl WRec {
    pub(super) fn new(name: &'static str) -> WRec {
        WRec { name, props: Vec::new(), children: Vec::new(), force_null: false }
    }

    pub(super) fn with(name: &'static str, props: Vec<WProp>) -> WRec {
        WRec { name, props, children: Vec::new(), force_null: false }
    }

    pub(super) fn child(mut self, c: WRec) -> WRec {
        self.children.push(c);
        self
    }

    /// The SDK's rule for NULL records, measured over four of its files: after
    /// the children of every record that has children, and also after the
    /// childless `AnimationLayer`/`AnimationStack` and the empty `References`
    /// and `Properties70`. Assimp drops every animation of a file whose
    /// `AnimationLayer` lacks it.
    fn needs_null(&self) -> bool {
        !self.children.is_empty() || self.force_null
    }
}

fn write_array<T: Copy>(out: &mut Vec<u8>, code: u8, items: &[T], to_le: impl Fn(T, &mut Vec<u8>)) -> Result<(), MeshError> {
    let mut raw = Vec::with_capacity(items.len() * 8);
    for &v in items {
        to_le(v, &mut raw);
    }
    let count = u32::try_from(items.len()).map_err(|_| unsupported("an array has more than 2^32 elements"))?;
    out.push(code);
    out.extend_from_slice(&count.to_le_bytes());
    if raw.len() >= COMPRESS_THRESHOLD {
        let z = super::fbx::deflate_zlib(&raw);
        let clen = u32::try_from(z.len()).map_err(|_| unsupported("an array compresses to more than 4 GiB"))?;
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&clen.to_le_bytes());
        out.extend_from_slice(&z);
    } else {
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        out.extend_from_slice(&raw);
    }
    Ok(())
}

fn write_prop(p: &WProp, out: &mut Vec<u8>) -> Result<(), MeshError> {
    match p {
        WProp::Byte(b) => {
            out.push(b'C');
            out.push(*b);
        }
        WProp::I32(v) => {
            out.push(b'I');
            out.extend_from_slice(&v.to_le_bytes());
        }
        WProp::I64(v) => {
            out.push(b'L');
            out.extend_from_slice(&v.to_le_bytes());
        }
        WProp::F64(v) => {
            out.push(b'D');
            out.extend_from_slice(&v.to_le_bytes());
        }
        WProp::Str(s) | WProp::Raw(s) => {
            out.push(if matches!(p, WProp::Str(_)) { b'S' } else { b'R' });
            let n = u32::try_from(s.len()).map_err(|_| unsupported("a string property exceeds 4 GiB"))?;
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(s);
        }
        WProp::ArrF32(v) => write_array(out, b'f', v, |x, o| o.extend_from_slice(&x.to_le_bytes()))?,
        WProp::ArrF64(v) => write_array(out, b'd', v, |x, o| o.extend_from_slice(&x.to_le_bytes()))?,
        WProp::ArrI32(v) => write_array(out, b'i', v, |x, o| o.extend_from_slice(&x.to_le_bytes()))?,
        WProp::ArrI64(v) => write_array(out, b'l', v, |x, o| o.extend_from_slice(&x.to_le_bytes()))?,
    }
    Ok(())
}

/// Serialise one record (and its subtree) in the 7.4 layout: 32-bit header
/// fields, 13-byte NULL records. Our own trees are a handful of levels deep,
/// so this recursion is bounded by the writer, not by any input.
fn write_rec(r: &WRec, out: &mut Vec<u8>) -> Result<(), MeshError> {
    let start = out.len();
    out.extend_from_slice(&[0u8; 12]);
    let name = r.name.as_bytes();
    out.push(u8::try_from(name.len()).map_err(|_| unsupported("record name longer than 255 bytes"))?);
    out.extend_from_slice(name);
    let props_start = out.len();
    for p in &r.props {
        write_prop(p, out)?;
    }
    let plen = out.len() - props_start;
    for c in &r.children {
        write_rec(c, out)?;
    }
    if r.needs_null() {
        out.extend_from_slice(&[0u8; 13]);
    }
    let end = u32::try_from(out.len()).map_err(|_| unsupported("the file would exceed the 4 GiB a 7.4 FBX can address"))?;
    let nprops = r.props.len() as u32;
    out[start..start + 4].copy_from_slice(&end.to_le_bytes());
    out[start + 4..start + 8].copy_from_slice(&nprops.to_le_bytes());
    out[start + 8..start + 12].copy_from_slice(&(plen as u32).to_le_bytes());
    Ok(())
}

/// Serialise top-level records, then the top-level NULL record.
pub(super) fn write_records(top: &[WRec], out: &mut Vec<u8>) -> Result<(), MeshError> {
    for r in top {
        write_rec(r, out)?;
    }
    out.extend_from_slice(&[0u8; 13]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_binary() -> Vec<u8> {
        let mut out = BINARY_MAGIC.to_vec();
        out.extend_from_slice(&[0x1A, 0x00]);
        out.extend_from_slice(&7400u32.to_le_bytes());
        let top = vec![
            WRec::with("A", vec![WProp::I32(7), WProp::Str(b"hi".to_vec())])
                .child(WRec::with("B", vec![WProp::ArrF64((0..300).map(|i| i as f64).collect())])),
            WRec { force_null: true, ..WRec::new("Empty") },
        ];
        write_records(&top, &mut out).unwrap();
        out
    }

    #[test]
    fn binary_records_round_trip_with_compressed_arrays() {
        let bytes = tiny_binary();
        let dom = parse_binary(&bytes).unwrap();
        let a = dom.child(ROOT, "A").unwrap();
        let p = dom.props(a).unwrap();
        assert_eq!(p[0].as_i64(), Some(7));
        assert_eq!(p[1].as_str().as_deref(), Some("hi"));
        let arr = dom.child_array(a, "B").unwrap().unwrap();
        // 300 doubles is 2400 bytes: past the threshold, so zlib with 78 01.
        if let ArrayRef::Binary { encoding, payload, .. } = arr {
            assert_eq!(encoding, 1);
            assert_eq!(&payload[..2], &[0x78, 0x01]);
        }
        let v = dom.f64s(&arr, "B").unwrap();
        assert_eq!(v.len(), 300);
        assert_eq!(v[299], 299.0);
        assert!(dom.child(ROOT, "Empty").is_some());
    }

    #[test]
    fn a_record_that_claims_to_end_past_its_parent_is_refused() {
        let mut bytes = tiny_binary();
        // The first record's end offset is at 27; point it past the file.
        bytes[27..31].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(parse_binary(&bytes), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn a_zlib_array_cannot_inflate_past_its_declared_count() {
        // 4 declared doubles, payload that inflates to 2400 bytes.
        let big: Vec<u8> = (0..2400u32).map(|i| (i % 7) as u8).collect();
        let z = super::super::fbx::deflate_zlib(&big);
        let dom = Dom::new(Encoding::Binary, 7400);
        let arr = ArrayRef::Binary { kind: b'd', count: 4, encoding: 1, payload: &z };
        assert!(dom.f64s(&arr, "bomb").is_err());
    }

    #[test]
    fn a_compressed_array_is_charged_once_for_what_it_decodes_to() {
        let doubles: Vec<u8> = (0..300u32).flat_map(|i| (i as f64).to_le_bytes()).collect();
        let z = super::super::fbx::deflate_zlib(&doubles);
        let dom = Dom::new(Encoding::Binary, 7400);
        let arr = ArrayRef::Binary { kind: b'd', count: 300, encoding: 1, payload: &z };
        assert_eq!(dom.f64s(&arr, "B").unwrap().len(), 300);
        assert_eq!(dom.decoded.get(), 300 * 8, "the decoded doubles, not the transient inflated bytes as well");
    }

    #[test]
    fn a_huge_declared_count_is_refused_before_allocating() {
        let dom = Dom::new(Encoding::Binary, 7400);
        let arr = ArrayRef::Binary { kind: b'd', count: usize::MAX / 4, encoding: 1, payload: &[0x78, 0x01] };
        let started = std::time::Instant::now();
        assert!(matches!(dom.f64s(&arr, "huge"), Err(MeshError::Unsupported(_))));
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
    }

    #[test]
    fn ascii_records_props_and_arrays_parse() {
        let src = "; FBX 7.4.0 project file\nFBXHeaderExtension:  {\n\tFBXVersion: 7400\n}\n\
                   Objects:  {\n\tModel: 12, \"Model::Hips\", \"LimbNode\" {\n\t\tP: \"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",1,2.5,-3\n\t}\n\
                   \tGeometry: 5, \"Geometry::\", \"Mesh\" {\n\t\tVertices: *6 {\n\t\t\ta: 0,0,0,\n1,1,1\n\t\t}\n\t}\n}\n";
        assert!(looks_like_ascii(src.as_bytes()));
        let dom = parse_ascii(src.as_bytes()).unwrap();
        assert_eq!(dom.version, 7400);
        let objects = dom.child(ROOT, "Objects").unwrap();
        let model = dom.child(objects, "Model").unwrap();
        let p = dom.props(model).unwrap();
        assert_eq!(p[0].as_i64(), Some(12));
        let (name, class) = split_object_name(p[1].as_bytes().unwrap(), Encoding::Ascii);
        assert_eq!((name.as_str(), class.as_str()), ("Hips", "Model"));
        let pp = dom.props(dom.child(model, "P").unwrap()).unwrap();
        assert_eq!(pp.len(), 7);
        assert_eq!(pp[5].as_f64(), Some(2.5));
        let geo = dom.child(objects, "Geometry").unwrap();
        let arr = dom.child_array(geo, "Vertices").unwrap().unwrap();
        assert_eq!(dom.f64s(&arr, "v").unwrap(), vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn ascii_array_counts_must_match() {
        let src = "FBXHeaderExtension:  {\n\tFBXVersion: 7400\n}\nX: *3 {\n\ta: 1,2\n}\n";
        let dom = parse_ascii(src.as_bytes()).unwrap();
        let arr = dom.child_array(ROOT, "X").unwrap().unwrap();
        assert!(dom.f64s(&arr, "X").is_err());
    }

    #[test]
    fn ascii_detection_does_not_claim_other_text() {
        assert!(!looks_like_ascii(b"v 0 0 0\nf 1 2 3\n"));
        assert!(!looks_like_ascii(b"ply\nformat ascii 1.0\n"));
        assert!(!looks_like_ascii(b"{\"asset\":{}}"));
        assert!(!looks_like_ascii(b"solid x\n"));
        assert!(looks_like_ascii(b"\xEF\xBB\xBF; FBX 7.5.0 project file\n"));
    }
}
