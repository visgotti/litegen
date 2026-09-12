//! PLY (Stanford polygon format): reads ASCII and binary little-endian, writes
//! binary little-endian.
//!
//! # The trap this file exists to avoid
//!
//! Every other container in this module has a layout fixed by its spec. PLY
//! does not — its ASCII header *describes* the record layout, and the same mesh
//! is legally written with the properties in any order, with extra per-vertex
//! properties (`confidence`, `quality`, material ids, `s`/`t`) interleaved
//! between the ones we want. A reader that assumes `x y z` come first and are
//! four bytes each does not fail on such a file; it returns a mesh built out of
//! someone else's confidence values. That is the worst possible outcome for a
//! conversion layer, because nothing downstream can tell it happened.
//!
//! So the header is parsed into an explicit per-property layout, and every
//! property in every record is either decoded or skipped **by its exact
//! declared width** — including the properties of elements we do not care about
//! at all, which is why unknown elements are walked rather than estimated.
//!
//! # Why we write binary_little_endian
//!
//! ASCII PLY is roughly four times the size and loses float precision through
//! the decimal round-trip. Binary little-endian is what every PLY consumer
//! supports and is already the byte order of every machine we run on, so
//! writing needs no swapping.
//!
//! # What this codec drops, and why
//!
//! - **uvs**, per the lossiness table in [`super`]. PLY has no UV convention we
//!   can rely on: `s`/`t`, `u`/`v`, `texture_u`/`texture_v` and per-face
//!   `texcoord` lists all appear in the wild, with no way to tell which one an
//!   unfamiliar file means. Guessing wrong writes garbage into a channel a
//!   renderer will happily sample, so we carry none of them in either
//!   direction.
//! - **Per-vertex colour variation.** PLY stores colour per vertex; the IR has
//!   a single `base_color`. See [`read`] for the compromise and why averaging
//!   would be worse.

use super::ir::Mesh;
use super::MeshError;

/// A real PLY header is a few hundred bytes. This cap is what stops a file that
/// opens with `ply` and never closes its header from making us scan — and
/// UTF-8-check — a hundred megabytes of binary looking for a line that is not
/// there.
const MAX_HEADER_BYTES: usize = 1 << 20;

/// Cap on the `comment name ...` line we write. A name is a label; an unbounded
/// one in a header is just a bigger blast radius for no benefit.
const MAX_NAME_CHARS: usize = 256;

fn not_ply(detail: impl Into<String>) -> MeshError {
    MeshError::NotThisFormat { expected: super::MeshFormat::Ply, detail: detail.into() }
}

fn bad(detail: impl Into<String>) -> MeshError {
    MeshError::Malformed(detail.into())
}

// ---------------------------------------------------------------------------
// Header model
// ---------------------------------------------------------------------------

/// A PLY scalar type. The `intN`/`uintN`/`floatN` spellings are aliases the
/// later PLY tooling emits for exactly the same widths, so they parse to the
/// same variant rather than being rejected as exotic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(tok: &str) -> Option<Scalar> {
        Some(match tok {
            "char" | "int8" => Scalar::I8,
            "uchar" | "uint8" => Scalar::U8,
            "short" | "int16" => Scalar::I16,
            "ushort" | "uint16" => Scalar::U16,
            "int" | "int32" => Scalar::I32,
            "uint" | "uint32" => Scalar::U32,
            "float" | "float32" => Scalar::F32,
            "double" | "float64" => Scalar::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Scalar::I8 | Scalar::U8 => 1,
            Scalar::I16 | Scalar::U16 => 2,
            Scalar::I32 | Scalar::U32 | Scalar::F32 => 4,
            Scalar::F64 => 8,
        }
    }

    fn is_float(self) -> bool {
        matches!(self, Scalar::F32 | Scalar::F64)
    }

    fn name(self) -> &'static str {
        match self {
            Scalar::I8 => "char",
            Scalar::U8 => "uchar",
            Scalar::I16 => "short",
            Scalar::U16 => "ushort",
            Scalar::I32 => "int",
            Scalar::U32 => "uint",
            Scalar::F32 => "float",
            Scalar::F64 => "double",
        }
    }

    /// Inclusive value range, used to reject an ASCII token the binary form of
    /// the same file could not have held. Every one of these bounds is exact in
    /// `f64` (the widest is `u32::MAX`), which is why the whole reader can carry
    /// values as `f64` without losing an index.
    fn range(self) -> (f64, f64) {
        match self {
            Scalar::I8 => (i8::MIN as f64, i8::MAX as f64),
            Scalar::U8 => (0.0, u8::MAX as f64),
            Scalar::I16 => (i16::MIN as f64, i16::MAX as f64),
            Scalar::U16 => (0.0, u16::MAX as f64),
            Scalar::I32 => (i32::MIN as f64, i32::MAX as f64),
            Scalar::U32 => (0.0, u32::MAX as f64),
            Scalar::F32 | Scalar::F64 => (f64::NEG_INFINITY, f64::INFINITY),
        }
    }

    /// Decode one little-endian value. Returns `None` only when `b` is shorter
    /// than the type, which is how truncation is reported without panicking;
    /// past the `get(..n)` the slice is exactly `n` bytes, so the indexing below
    /// cannot be out of bounds.
    fn decode_le(self, b: &[u8]) -> Option<f64> {
        let s = b.get(..self.size())?;
        Some(match self {
            Scalar::I8 => s[0] as i8 as f64,
            Scalar::U8 => s[0] as f64,
            Scalar::I16 => i16::from_le_bytes([s[0], s[1]]) as f64,
            Scalar::U16 => u16::from_le_bytes([s[0], s[1]]) as f64,
            Scalar::I32 => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64,
            Scalar::U32 => u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64,
            Scalar::F32 => f32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64,
            Scalar::F64 => {
                f64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
            }
        })
    }

    /// Decode one ASCII token, held to the same range the binary form would
    /// impose. `255` is a legal `uchar`; `300` is not, in either encoding, and
    /// accepting it in one would make the two paths disagree about the same
    /// file.
    fn parse_ascii(self, tok: &str) -> Result<f64, MeshError> {
        if self.is_float() {
            return tok
                .parse::<f64>()
                .map_err(|_| bad(format!("'{tok}' is not a {} value", self.name())));
        }
        let v: i64 = tok
            .parse()
            .map_err(|_| bad(format!("'{tok}' is not a {} value", self.name())))?;
        let (lo, hi) = self.range();
        let v = v as f64;
        if v < lo || v > hi {
            return Err(bad(format!("{tok} is out of range for {}", self.name())));
        }
        Ok(v)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Scalar(Scalar),
    /// `property list <count> <item> <name>` — a per-record variable-length run.
    List { count: Scalar, item: Scalar },
}

#[derive(Debug, Clone)]
struct Property {
    name: String,
    kind: Kind,
}

#[derive(Debug, Clone)]
struct Element {
    name: String,
    count: usize,
    props: Vec<Property>,
}

impl Element {
    /// Bytes per record, or `None` when the record contains a list and so has no
    /// fixed size. Drives the whole-element skip fast path and the up-front
    /// sanity check on a declared count.
    fn fixed_size(&self) -> Option<usize> {
        let mut total = 0usize;
        for p in &self.props {
            match p.kind {
                Kind::Scalar(t) => total = total.checked_add(t.size())?,
                Kind::List { .. } => return None,
            }
        }
        Some(total)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Ascii,
    BinaryLe,
}

struct Header {
    encoding: Encoding,
    elements: Vec<Element>,
    /// Recovered from the `comment name ...` line our own writer emits.
    name: Option<String>,
    /// Offset of the first body byte, i.e. just past `end_header\n`.
    body_offset: usize,
}

/// Parse the ASCII header.
///
/// The header is read byte-wise rather than by `str::lines` on the whole input
/// because everything after `end_header` may be binary, and a binary body is
/// very unlikely to be valid UTF-8.
fn parse_header(bytes: &[u8]) -> Result<Header, MeshError> {
    if bytes.is_empty() {
        return Err(not_ply("empty input"));
    }

    let mut pos = 0usize;
    let mut lineno = 0usize;
    let mut encoding: Option<Encoding> = None;
    let mut elements: Vec<Element> = Vec::new();
    let mut name: Option<String> = None;

    loop {
        if pos >= bytes.len() {
            return Err(bad("header ended without an end_header line"));
        }
        if pos > MAX_HEADER_BYTES {
            return Err(bad(format!(
                "no end_header within the first {MAX_HEADER_BYTES} bytes"
            )));
        }
        let (raw, next) = match bytes[pos..].iter().position(|b| *b == b'\n') {
            Some(k) => (&bytes[pos..pos + k], pos + k + 1),
            // A final line with no newline can only be a truncated header: the
            // body, if any, starts after end_header's own newline.
            None => (&bytes[pos..], bytes.len()),
        };
        pos = next;
        lineno += 1;

        let line = std::str::from_utf8(raw)
            .map_err(|_| bad(format!("header line {lineno} is not text")))?
            .trim_end_matches('\r')
            .trim();

        if lineno == 1 {
            // The magic is the whole first line, not a prefix: `plyfile` is not
            // a PLY file and must not be mistaken for a truncated one.
            if line != "ply" {
                return Err(not_ply(format!("first line is {line:?}, expected \"ply\"")));
            }
            continue;
        }
        if line.is_empty() {
            continue;
        }

        let mut tok = line.split_whitespace();
        let keyword = match tok.next() {
            Some(k) => k,
            None => continue,
        };

        match keyword {
            "comment" => {
                // Our writer stores `Mesh::name` here because PLY has nowhere
                // else to put it, and the lossiness table does not license
                // dropping it. Only the first such comment counts, and only the
                // exact `name` keyword, so an unrelated exporter's comment
                // cannot rename someone's mesh.
                if name.is_none() {
                    let rest: Vec<&str> = tok.collect();
                    if rest.first() == Some(&"name") && rest.len() > 1 {
                        name = Some(rest[1..].join(" "));
                    }
                }
            }
            // `obj_info` is metadata some scanners emit; it carries nothing we
            // model, but it is legal and must not be an error.
            "obj_info" => {}
            "format" => {
                if encoding.is_some() {
                    return Err(bad("header has more than one format line"));
                }
                if !elements.is_empty() {
                    return Err(bad("format line appears after the first element"));
                }
                let kind = tok.next().ok_or_else(|| not_ply("format line has no encoding"))?;
                // PLY has had exactly one version for its entire life, so an
                // omitted one is unambiguous rather than suspicious.
                let version = tok.next().unwrap_or("1.0");
                if version != "1.0" {
                    return Err(MeshError::Unsupported(format!(
                        "PLY format version {version}; this module reads 1.0"
                    )));
                }
                encoding = Some(match kind {
                    "ascii" => Encoding::Ascii,
                    "binary_little_endian" => Encoding::BinaryLe,
                    // Readable in principle, but byte-swapping every scalar is
                    // untested code on a path no modern exporter takes. Naming
                    // it is the honest failure; reading it little-endian would
                    // silently return a mesh of denormals.
                    "binary_big_endian" => {
                        return Err(MeshError::Unsupported(
                            "binary_big_endian PLY: this module reads ascii and \
                             binary_little_endian only"
                                .to_string(),
                        ))
                    }
                    other => {
                        return Err(not_ply(format!("unknown PLY encoding {other:?}")));
                    }
                });
            }
            "element" => {
                if encoding.is_none() {
                    return Err(not_ply("element line before the format line"));
                }
                let ename = tok.next().ok_or_else(|| bad("element line has no name"))?;
                let count_tok = tok.next().ok_or_else(|| {
                    bad(format!("element '{ename}' has no count"))
                })?;
                let count: u64 = count_tok
                    .parse()
                    .map_err(|_| bad(format!("element '{ename}' count {count_tok:?} is not a number")))?;
                let count = usize::try_from(count)
                    .map_err(|_| bad(format!("element '{ename}' count {count} does not fit in memory")))?;
                if elements.iter().any(|e| e.name == ename) {
                    // Two elements of the same name make "which one is the
                    // vertex list?" unanswerable; the spec does not allow it.
                    return Err(bad(format!("duplicate element '{ename}'")));
                }
                elements.push(Element { name: ename.to_string(), count, props: Vec::new() });
            }
            "property" => {
                let elem = elements
                    .last_mut()
                    .ok_or_else(|| bad("property line before any element line"))?;
                let ty = tok.next().ok_or_else(|| bad("property line has no type"))?;
                let (kind, pname) = if ty == "list" {
                    let c = tok.next().ok_or_else(|| bad("list property has no count type"))?;
                    let it = tok.next().ok_or_else(|| bad("list property has no item type"))?;
                    let c = Scalar::parse(c)
                        .ok_or_else(|| bad(format!("unknown list count type {c:?}")))?;
                    let it = Scalar::parse(it)
                        .ok_or_else(|| bad(format!("unknown list item type {it:?}")))?;
                    // A float count or a float index is not a thing any reader
                    // can act on, and silently truncating one would be a guess.
                    if c.is_float() {
                        return Err(bad(format!(
                            "list count type is {}; it must be an integer type",
                            c.name()
                        )));
                    }
                    (Kind::List { count: c, item: it }, tok.next())
                } else {
                    let t = Scalar::parse(ty)
                        .ok_or_else(|| bad(format!("unknown property type {ty:?}")))?;
                    (Kind::Scalar(t), tok.next())
                };
                let pname = pname.ok_or_else(|| bad("property line has no name"))?;
                elem.props.push(Property { name: pname.to_string(), kind });
            }
            "end_header" => {
                let encoding = encoding.ok_or_else(|| not_ply("header has no format line"))?;
                return Ok(Header { encoding, elements, name, body_offset: pos });
            }
            // PLY defines no other header keywords. Ignoring one would mean
            // ignoring whatever layout information it carried, and the layout is
            // the one thing we cannot afford to be wrong about.
            other => {
                return Err(bad(format!("unknown header directive {other:?} on line {lineno}")));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Body readers
// ---------------------------------------------------------------------------

/// One source of element records. Implemented once for ASCII and once for
/// binary so the vertex/face/skip logic — the part where a mistake is silent —
/// exists in exactly one place.
trait Body {
    /// ASCII: advance to the next record's line. Binary: nothing; records are
    /// simply consecutive.
    fn start_record(&mut self) -> Result<(), MeshError>;
    /// ASCII: assert the line held no more than the header promised. Binary:
    /// nothing.
    fn end_record(&mut self) -> Result<(), MeshError>;
    fn scalar(&mut self, t: Scalar) -> Result<f64, MeshError>;
    fn skip_scalar(&mut self, t: Scalar) -> Result<(), MeshError>;
    fn list(&mut self, count: Scalar, item: Scalar) -> Result<Vec<f64>, MeshError>;
    fn skip_list(&mut self, count: Scalar, item: Scalar) -> Result<(), MeshError>;

    /// Reject a declared record count that the remaining bytes cannot possibly
    /// satisfy, before anything is allocated or looped over.
    fn check_element(&self, _elem: &Element) -> Result<(), MeshError> {
        Ok(())
    }

    /// Skip an entire element in one step if the encoding allows it. Returns
    /// `false` when the caller must walk the records instead.
    fn skip_element(&mut self, _elem: &Element) -> Result<bool, MeshError> {
        Ok(false)
    }
}

struct BinaryLe<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> BinaryLe<'a> {
    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], MeshError> {
        let end = self.pos.checked_add(n).ok_or_else(|| bad("record length overflows"))?;
        let s = self.buf.get(self.pos..end).ok_or_else(|| {
            bad(format!(
                "truncated body: wanted {n} bytes at body offset {}, {} remain",
                self.pos,
                self.remaining()
            ))
        })?;
        self.pos = end;
        Ok(s)
    }
}

impl<'a> Body for BinaryLe<'a> {
    fn start_record(&mut self) -> Result<(), MeshError> {
        Ok(())
    }

    fn end_record(&mut self) -> Result<(), MeshError> {
        Ok(())
    }

    fn scalar(&mut self, t: Scalar) -> Result<f64, MeshError> {
        let b = self.take(t.size())?;
        t.decode_le(b).ok_or_else(|| bad("truncated scalar"))
    }

    fn skip_scalar(&mut self, t: Scalar) -> Result<(), MeshError> {
        self.take(t.size())?;
        Ok(())
    }

    fn list(&mut self, count: Scalar, item: Scalar) -> Result<Vec<f64>, MeshError> {
        let n = self.list_len(count, item)?;
        // `n` is only allocated after list_len has proved the bytes for it
        // exist, so a hostile count field costs a bounds check, not 32 GB.
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(self.scalar(item)?);
        }
        Ok(out)
    }

    fn skip_list(&mut self, count: Scalar, item: Scalar) -> Result<(), MeshError> {
        let n = self.list_len(count, item)?;
        let bytes = n.checked_mul(item.size()).ok_or_else(|| bad("list length overflows"))?;
        self.take(bytes)?;
        Ok(())
    }

    fn check_element(&self, elem: &Element) -> Result<(), MeshError> {
        // Only possible for fixed-size records; an element containing a list
        // has no size until its counts are read, so those are caught by
        // truncation during the walk instead.
        if let Some(size) = elem.fixed_size() {
            let needed = elem
                .count
                .checked_mul(size)
                .ok_or_else(|| bad(format!("element '{}' size overflows", elem.name)))?;
            if needed > self.remaining() {
                return Err(bad(format!(
                    "element '{}' declares {} records ({needed} bytes) but {} bytes remain",
                    elem.name,
                    elem.count,
                    self.remaining()
                )));
            }
        }
        Ok(())
    }

    fn skip_element(&mut self, elem: &Element) -> Result<bool, MeshError> {
        match elem.fixed_size() {
            Some(size) => {
                let bytes = elem
                    .count
                    .checked_mul(size)
                    .ok_or_else(|| bad(format!("element '{}' size overflows", elem.name)))?;
                self.take(bytes)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

impl<'a> BinaryLe<'a> {
    /// Read a list's count and prove its payload is actually present.
    fn list_len(&mut self, count: Scalar, item: Scalar) -> Result<usize, MeshError> {
        let raw = self.scalar(count)?;
        if raw < 0.0 {
            return Err(bad(format!("list length {raw} is negative")));
        }
        let n = raw as usize;
        let bytes = n.checked_mul(item.size()).ok_or_else(|| bad("list length overflows"))?;
        if bytes > self.remaining() {
            return Err(bad(format!(
                "list declares {n} items ({bytes} bytes) but {} bytes remain",
                self.remaining()
            )));
        }
        Ok(n)
    }
}

struct Ascii<'a> {
    lines: std::str::Lines<'a>,
    cur: Option<std::str::SplitWhitespace<'a>>,
}

impl<'a> Ascii<'a> {
    fn next_token(&mut self) -> Result<&'a str, MeshError> {
        let it = self.cur.as_mut().ok_or_else(|| bad("record read before its line was started"))?;
        it.next().ok_or_else(|| bad("record has fewer values than the header declares"))
    }
}

impl<'a> Body for Ascii<'a> {
    /// One record per line: that is how every PLY writer emits ASCII, and it is
    /// the only rule under which a short record is detectable at all. If records
    /// were allowed to flow across lines, a file missing one value would
    /// silently borrow the next record's first value and shift the whole mesh.
    fn start_record(&mut self) -> Result<(), MeshError> {
        loop {
            match self.lines.next() {
                Some(l) if l.trim().is_empty() => continue,
                Some(l) => {
                    self.cur = Some(l.split_whitespace());
                    return Ok(());
                }
                None => return Err(bad("body ends before all declared records were read")),
            }
        }
    }

    fn end_record(&mut self) -> Result<(), MeshError> {
        if let Some(it) = self.cur.as_mut() {
            if let Some(extra) = it.next() {
                return Err(bad(format!(
                    "record has more values than the header declares (trailing {extra:?})"
                )));
            }
        }
        self.cur = None;
        Ok(())
    }

    fn scalar(&mut self, t: Scalar) -> Result<f64, MeshError> {
        let tok = self.next_token()?;
        t.parse_ascii(tok)
    }

    fn skip_scalar(&mut self, _t: Scalar) -> Result<(), MeshError> {
        // Deliberately not parsed: a property we do not model is allowed to hold
        // anything, and rejecting a file over a value we never look at would be
        // a false negative, not strictness.
        self.next_token()?;
        Ok(())
    }

    fn list(&mut self, count: Scalar, item: Scalar) -> Result<Vec<f64>, MeshError> {
        let n = self.list_len(count)?;
        // Capacity is capped because the count is untrusted here too; the tokens
        // run out long before a real list gets this long.
        let mut out = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            out.push(self.scalar(item)?);
        }
        Ok(out)
    }

    fn skip_list(&mut self, count: Scalar, _item: Scalar) -> Result<(), MeshError> {
        let n = self.list_len(count)?;
        for _ in 0..n {
            self.next_token()?;
        }
        Ok(())
    }
}

impl<'a> Ascii<'a> {
    fn list_len(&mut self, count: Scalar) -> Result<usize, MeshError> {
        let tok = self.next_token()?;
        let raw = count.parse_ascii(tok)?;
        if raw < 0.0 {
            return Err(bad(format!("list length {tok} is negative")));
        }
        Ok(raw as usize)
    }
}

// ---------------------------------------------------------------------------
// Vertex / face plans
// ---------------------------------------------------------------------------

const SLOT_X: usize = 0;
const SLOT_NX: usize = 3;
const SLOT_R: usize = 6;
const SLOT_A: usize = 9;
const SLOTS: usize = 10;

struct VertexPlan {
    /// Property index -> slot, `None` for properties read past and discarded.
    slot_of: Vec<Option<usize>>,
    has_normals: bool,
    has_color: bool,
    has_alpha: bool,
    /// Divisor per colour slot (r, g, b, a), from each component's declared
    /// type: colour is conventionally `uchar`, but float PLY colour is already
    /// 0..1 and dividing it by 255 would black out the mesh.
    color_scale: [f64; 4],
}

fn slot_for(name: &str) -> Option<usize> {
    Some(match name {
        "x" => SLOT_X,
        "y" => SLOT_X + 1,
        "z" => SLOT_X + 2,
        "nx" => SLOT_NX,
        "ny" => SLOT_NX + 1,
        "nz" => SLOT_NX + 2,
        // Only the spec spellings. `diffuse_red`, `r`, and friends exist, but
        // each is one exporter's private convention and mapping them by guess is
        // how a mesh ends up tinted by a scanner's confidence channel.
        "red" => SLOT_R,
        "green" => SLOT_R + 1,
        "blue" => SLOT_R + 2,
        "alpha" => SLOT_A,
        _ => return None,
    })
}

fn plan_vertex(elem: &Element) -> Result<VertexPlan, MeshError> {
    let mut slot_of = vec![None; elem.props.len()];
    let mut at: [Option<usize>; SLOTS] = [None; SLOTS];

    for (i, p) in elem.props.iter().enumerate() {
        let slot = match slot_for(&p.name) {
            Some(s) => s,
            None => continue,
        };
        match p.kind {
            Kind::Scalar(_) => {}
            // A list named `x` is not a coordinate under any reading of the
            // spec, and treating it as one would consume the wrong bytes.
            Kind::List { .. } => {
                return Err(bad(format!(
                    "vertex property '{}' is a list; it must be a scalar",
                    p.name
                )))
            }
        }
        if at[slot].is_some() {
            return Err(bad(format!("vertex element declares '{}' twice", p.name)));
        }
        at[slot] = Some(i);
        slot_of[i] = Some(slot);
    }

    for (slot, axis) in [(SLOT_X, "x"), (SLOT_X + 1, "y"), (SLOT_X + 2, "z")] {
        if at[slot].is_none() {
            return Err(bad(format!("vertex element has no '{axis}' property")));
        }
    }

    // Partial normals or partial colour are rejected rather than ignored: a file
    // carrying `nx` but not `ny` disagrees with itself, and silently discarding
    // the half that is there hides the disagreement from whoever produced it.
    let normal_present = (SLOT_NX..SLOT_NX + 3).filter(|s| at[*s].is_some()).count();
    if normal_present != 0 && normal_present != 3 {
        return Err(bad("vertex element has some of nx/ny/nz but not all three"));
    }
    let color_present = (SLOT_R..SLOT_R + 3).filter(|s| at[*s].is_some()).count();
    if color_present != 0 && color_present != 3 {
        return Err(bad("vertex element has some of red/green/blue but not all three"));
    }
    let has_alpha = at[SLOT_A].is_some();
    if has_alpha && color_present == 0 {
        return Err(bad("vertex element has 'alpha' but no red/green/blue"));
    }

    let mut color_scale = [1.0f64; 4];
    for (k, slot) in (SLOT_R..=SLOT_A).enumerate() {
        let Some(i) = at[slot] else { continue };
        let Kind::Scalar(t) = elem.props[i].kind else { continue };
        color_scale[k] = match t {
            Scalar::U8 => 255.0,
            Scalar::U16 => 65535.0,
            // Float colour is already normalised; anything else has no agreed
            // full-scale value, and inventing one would be a silent tint.
            Scalar::F32 | Scalar::F64 => 1.0,
            other => {
                return Err(bad(format!(
                    "colour property '{}' has type {}; PLY colour is uchar, ushort or float",
                    elem.props[i].name,
                    other.name()
                )))
            }
        };
    }

    Ok(VertexPlan {
        slot_of,
        has_normals: normal_present == 3,
        has_color: color_present == 3,
        has_alpha,
        color_scale,
    })
}

/// Index of the face element's vertex-index list property.
///
/// `vertex_indices` is the spec spelling; `vertex_index` is what several widely
/// used exporters write, and both are common enough in files we will actually be
/// handed that rejecting the second would be pedantry with a support cost.
fn plan_face(elem: &Element) -> Result<usize, MeshError> {
    for (i, p) in elem.props.iter().enumerate() {
        if matches!(p.name.as_str(), "vertex_indices" | "vertex_index" | "vertex_indexes") {
            return match p.kind {
                Kind::List { .. } => Ok(i),
                Kind::Scalar(_) => Err(bad(format!(
                    "face property '{}' is a scalar; it must be a list",
                    p.name
                ))),
            };
        }
    }
    Err(bad("face element has no vertex_indices list property"))
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Prefix a record-level error with the element and record it came from. PLY
/// bodies have no self-describing structure, so "truncated at body offset 91100"
/// on its own tells nobody which record went wrong.
fn at_record(elem: &str, i: usize, e: MeshError) -> MeshError {
    match e {
        MeshError::Malformed(d) => MeshError::Malformed(format!("{elem} record {i}: {d}")),
        other => other,
    }
}

fn read_vertex<B: Body>(
    b: &mut B,
    elem: &Element,
    plan: &VertexPlan,
) -> Result<[f64; SLOTS], MeshError> {
    let mut v = [0.0f64; SLOTS];
    b.start_record()?;
    for (i, p) in elem.props.iter().enumerate() {
        match (p.kind, plan.slot_of[i]) {
            (Kind::Scalar(t), Some(slot)) => v[slot] = b.scalar(t)?,
            (Kind::Scalar(t), None) => b.skip_scalar(t)?,
            (Kind::List { count, item }, _) => b.skip_list(count, item)?,
        }
    }
    b.end_record()?;
    Ok(v)
}

fn read_face<B: Body>(
    b: &mut B,
    elem: &Element,
    idx_prop: usize,
    out: &mut Vec<u32>,
) -> Result<(), MeshError> {
    let mut corners: Vec<f64> = Vec::new();
    b.start_record()?;
    for (i, p) in elem.props.iter().enumerate() {
        match p.kind {
            Kind::Scalar(t) => b.skip_scalar(t)?,
            Kind::List { count, item } => {
                if i == idx_prop {
                    corners = b.list(count, item)?;
                } else {
                    b.skip_list(count, item)?;
                }
            }
        }
    }
    b.end_record()?;

    if corners.len() < 3 {
        // A one- or two-vertex "face" is a point or an edge. Dropping it would
        // be the silent partial mesh this module refuses to produce.
        return Err(bad(format!("face has {} vertices, need at least 3", corners.len())));
    }
    let mut idx = Vec::with_capacity(corners.len());
    for c in &corners {
        if !(0.0..=(u32::MAX as f64)).contains(c) {
            return Err(bad(format!("vertex index {c} is out of range")));
        }
        idx.push(*c as u32);
    }
    // Fan triangulation. Correct for convex polygons, which is what exporters
    // emit; a concave n-gon would need ear clipping, and no vendor we convert
    // for produces one. Winding is preserved, so normals stay consistent.
    for k in 1..idx.len() - 1 {
        out.push(idx[0]);
        out.push(idx[k]);
        out.push(idx[k + 1]);
    }
    Ok(())
}

fn skip_records<B: Body>(b: &mut B, elem: &Element) -> Result<(), MeshError> {
    // A property-less element occupies no bytes at all in binary; treating it as
    // a blank line per record in ASCII would be a guess, and the two encodings
    // have to agree about the same file.
    if elem.props.is_empty() {
        return Ok(());
    }
    if b.skip_element(elem)? {
        return Ok(());
    }
    for i in 0..elem.count {
        let r = (|| {
            b.start_record()?;
            for p in &elem.props {
                match p.kind {
                    Kind::Scalar(t) => b.skip_scalar(t)?,
                    Kind::List { count, item } => b.skip_list(count, item)?,
                }
            }
            b.end_record()
        })();
        r.map_err(|e| at_record(&elem.name, i, e))?;
    }
    Ok(())
}

fn finite(v: f64, what: &str) -> Result<f32, MeshError> {
    if !v.is_finite() {
        // NaN coordinates survive validate() and every arithmetic step, then
        // turn up as an invisible mesh or a collapsed bounding box much later.
        return Err(bad(format!("{what} is {v}, which is not a finite number")));
    }
    Ok(v as f32)
}

fn read_body<B: Body>(b: &mut B, h: &Header) -> Result<Mesh, MeshError> {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut base_color: Option<[f32; 4]> = None;
    let mut saw_vertex = false;

    for elem in &h.elements {
        b.check_element(elem)?;
        match elem.name.as_str() {
            "vertex" => {
                saw_vertex = true;
                let plan = plan_vertex(elem)?;
                for i in 0..elem.count {
                    let v = read_vertex(b, elem, &plan).map_err(|e| at_record("vertex", i, e))?;
                    let f = |slot: usize, what: &str| {
                        finite(v[slot], what).map_err(|e| at_record("vertex", i, e))
                    };
                    positions.push([f(SLOT_X, "x")?, f(SLOT_X + 1, "y")?, f(SLOT_X + 2, "z")?]);
                    if plan.has_normals {
                        normals.push([
                            f(SLOT_NX, "nx")?,
                            f(SLOT_NX + 1, "ny")?,
                            f(SLOT_NX + 2, "nz")?,
                        ]);
                    }
                    // PLY colour is per vertex; the IR has one flat base colour.
                    // The first vertex wins. Averaging would invent a colour
                    // that appears nowhere in the source — plausible, wrong, and
                    // undetectable — whereas the first vertex's colour is at
                    // least a colour the file actually contains. Per-vertex
                    // colour variation is genuinely lost here; see the module
                    // docs on `super` for why the IR stays this small.
                    if plan.has_color && i == 0 {
                        let c = |slot: usize, k: usize| -> f32 {
                            let raw = v[slot] / plan.color_scale[k];
                            if raw.is_finite() { raw.clamp(0.0, 1.0) as f32 } else { 0.0 }
                        };
                        let a = if plan.has_alpha { c(SLOT_A, 3) } else { 1.0 };
                        base_color = Some([c(SLOT_R, 0), c(SLOT_R + 1, 1), c(SLOT_R + 2, 2), a]);
                    }
                }
            }
            "face" => {
                let idx_prop = plan_face(elem)?;
                for i in 0..elem.count {
                    read_face(b, elem, idx_prop, &mut indices)
                        .map_err(|e| at_record("face", i, e))?;
                }
            }
            // Edges, materials, range grids, tristrips: legal PLY elements we do
            // not model. Skipping them requires their property layout, which is
            // exactly why the header parser keeps every element it sees.
            _ => skip_records(b, elem)?,
        }
    }

    if !saw_vertex {
        return Err(bad("no 'vertex' element in header"));
    }

    // Checked here rather than relying on Mesh::validate so the message can name
    // the element, and because `face` may legally precede `vertex` in the
    // header, in which case the count is not known until now.
    if let Some(bad_idx) = indices.iter().find(|i| **i as usize >= positions.len()) {
        return Err(bad(format!(
            "face index {bad_idx} out of range for {} vertices",
            positions.len()
        )));
    }

    Ok(Mesh {
        positions,
        indices,
        normals: if normals.is_empty() { None } else { Some(normals) },
        // uvs are never carried: see the module docs.
        uvs: None,
        base_color,
        name: h.name.clone(),
    })
}

/// Read a PLY file (ASCII or binary little-endian) into the neutral mesh.
pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    let h = parse_header(bytes)?;
    let body = bytes.get(h.body_offset..).unwrap_or(&[]);
    match h.encoding {
        Encoding::BinaryLe => {
            let mut b = BinaryLe { buf: body, pos: 0 };
            read_body(&mut b, &h)
        }
        Encoding::Ascii => {
            let text = std::str::from_utf8(body)
                .map_err(|_| bad("ascii PLY body is not valid UTF-8"))?;
            let mut b = Ascii { lines: text.lines(), cur: None };
            read_body(&mut b, &h)
        }
    }
    // Trailing bytes past the last element are deliberately not an error: some
    // exporters pad, and the header fully determines where the data ends.
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Make a name safe to carry in a header comment.
///
/// Header lines are newline-delimited ASCII, so an unfiltered name could inject
/// header lines outright. Whitespace runs collapse to single spaces because the
/// reader recovers the name by re-joining tokens; collapsing on write is what
/// makes that recovery exact rather than approximate.
fn header_name(raw: &str) -> Option<String> {
    let cleaned = raw
        .split_whitespace()
        .map(|w| w.chars().filter(|c| c.is_ascii_graphic()).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned: String = cleaned.chars().take(MAX_NAME_CHARS).collect();
    let cleaned = cleaned.trim_end().to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn quantise(c: f32) -> u8 {
    if !c.is_finite() {
        return 0;
    }
    // Round rather than truncate so a value that came in as `n/255` goes back
    // out as exactly `n`, which is what makes ply -> ply colour lossless.
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Write the mesh as binary little-endian PLY.
pub fn write(mesh: &Mesh) -> Result<Vec<u8>, MeshError> {
    // write() is public, so it can be called with a mesh that never went through
    // the module-level entry point. Better to fail here than to emit a file
    // whose face list points past its own vertex list.
    mesh.validate()?;

    let has_normals = mesh.normals.is_some();
    let color = mesh.base_color.map(|c| {
        [quantise(c[0]), quantise(c[1]), quantise(c[2]), quantise(c[3])]
    });

    let mut header = String::with_capacity(512);
    header.push_str("ply\nformat binary_little_endian 1.0\n");
    header.push_str("comment written by litegen mesh converter\n");
    if let Some(n) = mesh.name.as_deref().and_then(header_name) {
        header.push_str("comment name ");
        header.push_str(&n);
        header.push('\n');
    }
    header.push_str(&format!("element vertex {}\n", mesh.positions.len()));
    header.push_str("property float x\nproperty float y\nproperty float z\n");
    if has_normals {
        header.push_str("property float nx\nproperty float ny\nproperty float nz\n");
    }
    if color.is_some() {
        header.push_str(
            "property uchar red\nproperty uchar green\nproperty uchar blue\nproperty uchar alpha\n",
        );
    }
    header.push_str(&format!("element face {}\n", mesh.triangle_count()));
    // uchar/uint is the pairing every reader handles: a count of 3 always fits a
    // uchar, and uint indices cover any mesh that fits under MAX_INPUT_BYTES.
    header.push_str("property list uchar uint vertex_indices\n");
    header.push_str("end_header\n");

    let per_vertex = 12 + if has_normals { 12 } else { 0 } + if color.is_some() { 4 } else { 0 };
    let mut out = Vec::with_capacity(
        header.len() + mesh.positions.len() * per_vertex + mesh.triangle_count() * 13,
    );
    out.extend_from_slice(header.as_bytes());

    let empty: Vec<[f32; 3]> = Vec::new();
    let normals = mesh.normals.as_ref().unwrap_or(&empty);
    for (i, p) in mesh.positions.iter().enumerate() {
        for v in p {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if has_normals {
            // validate() has already established normals.len() == positions.len().
            let n = normals.get(i).copied().unwrap_or([0.0, 0.0, 0.0]);
            for v in &n {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        if let Some(c) = color {
            // The one base colour is written on every vertex: PLY has no
            // material block, so per-vertex colour is the only place a flat
            // colour can live, and a reader that sees it on one vertex only
            // would show an unshaded mesh.
            out.extend_from_slice(&c);
        }
    }

    for tri in mesh.indices.chunks_exact(3) {
        out.push(3u8);
        for i in tri {
            out.extend_from_slice(&i.to_le_bytes());
        }
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tri_mesh() -> Mesh {
        Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            normals: None,
            uvs: None,
            base_color: None,
            name: None,
        }
    }

    fn malformed(e: &MeshError) -> &str {
        match e {
            MeshError::Malformed(d) => d,
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    /// Assemble a binary body from little-endian pieces.
    #[derive(Default)]
    struct Buf(Vec<u8>);
    impl Buf {
        fn u8(mut self, v: u8) -> Self {
            self.0.push(v);
            self
        }
        fn i32(mut self, v: i32) -> Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn u32(mut self, v: u32) -> Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn f32(mut self, v: f32) -> Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn f64(mut self, v: f64) -> Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
    }

    fn with_header(header: &str, body: Buf) -> Vec<u8> {
        let mut v = header.as_bytes().to_vec();
        v.extend_from_slice(&body.0);
        v
    }

    // -- round trips --------------------------------------------------------

    #[test]
    fn round_trip_positions_and_indices() {
        let m = tri_mesh();
        let back = read(&write(&m).unwrap()).unwrap();
        assert_eq!(back.positions, m.positions);
        assert_eq!(back.indices, m.indices);
        assert_eq!(back.normals, None);
        assert_eq!(back.base_color, None);
        assert_eq!(back.name, None);
    }

    #[test]
    fn round_trip_with_normals() {
        let mut m = tri_mesh();
        m.normals = Some(vec![[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]]);
        let back = read(&write(&m).unwrap()).unwrap();
        assert_eq!(back.normals, m.normals);
        assert_eq!(back.positions, m.positions);
    }

    #[test]
    fn round_trip_with_base_color() {
        let mut m = tri_mesh();
        // Values chosen on the u8 grid: those are exactly the ones a lossless
        // colour round-trip must preserve bit for bit.
        m.base_color = Some([1.0, 128.0 / 255.0, 0.0, 64.0 / 255.0]);
        let back = read(&write(&m).unwrap()).unwrap();
        assert_eq!(back.base_color, m.base_color);
    }

    #[test]
    fn round_trip_everything_at_once() {
        let m = Mesh {
            positions: vec![
                [-1.5, 2.25, 0.125],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            normals: Some(vec![
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [-1.0, 0.0, 0.0],
            ]),
            uvs: None,
            base_color: Some([0.0, 1.0, 51.0 / 255.0, 1.0]),
            name: Some("widget".into()),
        };
        let back = read(&write(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn uvs_are_dropped_but_nothing_else_is() {
        let mut m = tri_mesh();
        m.uvs = Some(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
        m.normals = Some(vec![[0.0, 0.0, 1.0]; 3]);
        m.name = Some("uv test".into());
        let back = read(&write(&m).unwrap()).unwrap();
        assert_eq!(back.uvs, None, "the lossiness table says ply drops uvs");
        assert_eq!(back.normals, m.normals);
        assert_eq!(back.positions, m.positions);
        assert_eq!(back.name.as_deref(), Some("uv test"));
    }

    #[test]
    fn name_is_sanitised_and_recovered() {
        let mut m = tri_mesh();
        // A newline here would inject header lines if it were written raw.
        m.name = Some("  evil\nply\nformat ascii 1.0  ".into());
        let bytes = write(&m).unwrap();
        assert_eq!(
            bytes.windows(4).filter(|w| *w == b"ply\n").count(),
            1,
            "only the magic line may say ply"
        );
        assert_eq!(read(&bytes).unwrap().name.as_deref(), Some("evil ply format ascii 1.0"));
    }

    #[test]
    fn name_that_sanitises_to_nothing_is_omitted() {
        let mut m = tri_mesh();
        m.name = Some("\u{2603}\u{2603}".into());
        let bytes = write(&m).unwrap();
        assert!(!String::from_utf8_lossy(&bytes[..80]).contains("comment name"));
        assert_eq!(read(&bytes).unwrap().name, None);
    }

    #[test]
    fn written_header_is_the_advertised_one() {
        let m = tri_mesh();
        let bytes = write(&m).unwrap();
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(300)]).to_string();
        assert!(head.starts_with("ply\nformat binary_little_endian 1.0\n"));
        assert!(head.contains("comment written by litegen mesh converter\n"));
        assert!(head.contains("element vertex 3\n"));
        assert!(head.contains("element face 1\n"));
        assert!(head.contains("property list uchar uint vertex_indices\nend_header\n"));
    }

    #[test]
    fn colour_is_written_on_every_vertex() {
        let mut m = tri_mesh();
        m.base_color = Some([1.0, 0.0, 0.0, 1.0]);
        let bytes = write(&m).unwrap();
        let head_end = bytes.windows(11).position(|w| w == b"end_header\n").unwrap() + 11;
        let body = &bytes[head_end..];
        // 3 vertices * (12 position bytes + 4 colour bytes), then the face.
        assert_eq!(body.len(), 3 * 16 + 13);
        for v in 0..3 {
            assert_eq!(&body[v * 16 + 12..v * 16 + 16], &[255, 0, 0, 255]);
        }
    }

    #[test]
    fn a_written_face_is_a_uchar_count_then_three_uints() {
        let bytes = write(&tri_mesh()).unwrap();
        let head_end = bytes.windows(11).position(|w| w == b"end_header\n").unwrap() + 11;
        let face = &bytes[head_end + 36..];
        assert_eq!(face, &[3, 0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]);
    }

    // -- ascii --------------------------------------------------------------

    #[test]
    fn ascii_minimal() {
        let src = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
                   property float z\nelement face 1\nproperty list uchar int vertex_indices\n\
                   end_header\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n";
        let m = read(src.as_bytes()).unwrap();
        assert_eq!(m.positions, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        assert_eq!(m.indices, vec![0, 1, 2]);
    }

    #[test]
    fn ascii_properties_in_any_order_with_extras() {
        // y before x, a double z, and two properties we do not model. Reading
        // this positionally would produce a mesh made of `confidence`.
        let src = "ply\nformat ascii 1.0\ncomment made by something else\n\
                   element vertex 3\nproperty float confidence\nproperty float y\n\
                   property double z\nproperty int material\nproperty float x\n\
                   property uchar red\nproperty uchar green\nproperty uchar blue\n\
                   element face 1\nproperty list uchar int vertex_indices\nend_header\n\
                   0.9 2 3 7 1 255 128 0\n0.8 5 6 7 4 0 0 0\n0.7 8 9 7 7 0 0 0\n3 0 1 2\n";
        let m = read(src.as_bytes()).unwrap();
        assert_eq!(m.positions, vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]]);
        assert_eq!(m.base_color, Some([1.0, 128.0 / 255.0, 0.0, 1.0]));
        assert_eq!(m.normals, None);
    }

    #[test]
    fn ascii_normals_and_alpha() {
        let src = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
                   property float z\nproperty float nx\nproperty float ny\nproperty float nz\n\
                   property uchar red\nproperty uchar green\nproperty uchar blue\n\
                   property uchar alpha\nelement face 1\n\
                   property list uchar int vertex_indices\nend_header\n\
                   0 0 0 0 0 1 10 20 30 40\n1 0 0 0 1 0 1 2 3 4\n0 1 0 1 0 0 5 6 7 8\n3 0 1 2\n";
        let m = read(src.as_bytes()).unwrap();
        assert_eq!(
            m.normals,
            Some(vec![[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]])
        );
        // First vertex wins; the other two are genuinely lost.
        assert_eq!(
            m.base_color,
            Some([10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0, 40.0 / 255.0])
        );
    }

    #[test]
    fn ascii_float_colour_is_not_divided_by_255() {
        let src = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
                   property float z\nproperty float red\nproperty float green\nproperty float blue\n\
                   element face 1\nproperty list uchar int vertex_indices\nend_header\n\
                   0 0 0 1 0.5 0\n1 0 0 0 0 0\n0 1 0 0 0 0\n3 0 1 2\n";
        let m = read(src.as_bytes()).unwrap();
        assert_eq!(m.base_color, Some([1.0, 0.5, 0.0, 1.0]));
    }

    #[test]
    fn ascii_crlf_and_blank_lines() {
        let src = "ply\r\nformat ascii 1.0\r\nelement vertex 3\r\nproperty float x\r\n\
                   property float y\r\nproperty float z\r\nelement face 1\r\n\
                   property list uchar int vertex_indices\r\nend_header\r\n\
                   0 0 0\r\n\r\n1 0 0\r\n0 1 0\r\n3 0 1 2\r\n";
        let m = read(src.as_bytes()).unwrap();
        assert_eq!(m.positions.len(), 3);
        assert_eq!(m.indices, vec![0, 1, 2]);
    }

    #[test]
    fn ascii_quad_is_fanned() {
        let src = "ply\nformat ascii 1.0\nelement vertex 4\nproperty float x\nproperty float y\n\
                   property float z\nelement face 1\nproperty list uchar int vertex_indices\n\
                   end_header\n0 0 0\n1 0 0\n1 1 0\n0 1 0\n4 0 1 2 3\n";
        let m = read(src.as_bytes()).unwrap();
        assert_eq!(m.indices, vec![0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn ascii_unknown_element_is_skipped_by_layout() {
        // An `edge` element sits between vertex and face and carries a list, so
        // it can only be skipped by actually walking its records.
        let src = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
                   property float z\nelement edge 2\nproperty list uchar int ends\n\
                   property uchar flags\nelement face 1\n\
                   property list uchar int vertex_indices\nend_header\n\
                   0 0 0\n1 0 0\n0 1 0\n2 0 1 9\n3 0 1 2 9\n3 0 1 2\n";
        let m = read(src.as_bytes()).unwrap();
        assert_eq!(m.positions.len(), 3);
        assert_eq!(m.indices, vec![0, 1, 2]);
    }

    #[test]
    fn ascii_record_with_too_few_values() {
        let src = "ply\nformat ascii 1.0\nelement vertex 2\nproperty float x\nproperty float y\n\
                   property float z\nelement face 1\nproperty list uchar int vertex_indices\n\
                   end_header\n0 0 0\n1 0\n3 0 1 2\n";
        let e = read(src.as_bytes()).unwrap_err();
        assert!(malformed(&e).contains("fewer values"), "{e}");
    }

    #[test]
    fn ascii_record_with_too_many_values() {
        let src = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
                   property float z\nelement face 1\nproperty list uchar int vertex_indices\n\
                   end_header\n0 0 0 99\n1 0 0\n0 1 0\n3 0 1 2\n";
        let e = read(src.as_bytes()).unwrap_err();
        assert!(malformed(&e).contains("more values"), "{e}");
    }

    #[test]
    fn ascii_body_ends_early() {
        let src = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
                   property float z\nelement face 1\nproperty list uchar int vertex_indices\n\
                   end_header\n0 0 0\n1 0 0\n";
        let e = read(src.as_bytes()).unwrap_err();
        assert!(malformed(&e).contains("body ends before"), "{e}");
    }

    #[test]
    fn ascii_value_out_of_range_for_its_type() {
        let src = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
                   property float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\n\
                   element face 1\nproperty list uchar int vertex_indices\nend_header\n\
                   0 0 0 300 0 0\n1 0 0 0 0 0\n0 1 0 0 0 0\n3 0 1 2\n";
        let e = read(src.as_bytes()).unwrap_err();
        assert!(malformed(&e).contains("out of range for uchar"), "{e}");
    }

    #[test]
    fn ascii_non_numeric_value() {
        let src = "ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\n\
                   property float z\nelement face 0\nproperty list uchar int vertex_indices\n\
                   end_header\nzero 0 0\n";
        let e = read(src.as_bytes()).unwrap_err();
        assert!(malformed(&e).contains("is not a float value"), "{e}");
    }

    #[test]
    fn ascii_non_finite_position_is_rejected() {
        let src = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
                   property float z\nelement face 1\nproperty list uchar int vertex_indices\n\
                   end_header\nnan 0 0\n1 0 0\n0 1 0\n3 0 1 2\n";
        let e = read(src.as_bytes()).unwrap_err();
        assert!(malformed(&e).contains("not a finite number"), "{e}");
    }

    #[test]
    fn ascii_hostile_vertex_count_fails_without_allocating() {
        let src = "ply\nformat ascii 1.0\nelement vertex 4000000000\nproperty float x\n\
                   property float y\nproperty float z\nelement face 0\n\
                   property list uchar int vertex_indices\nend_header\n0 0 0\n";
        let e = read(src.as_bytes()).unwrap_err();
        assert!(malformed(&e).contains("body ends before"), "{e}");
    }

    #[test]
    fn ascii_body_that_is_not_utf8() {
        let mut v =
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\n\
              property float z\nelement face 0\nproperty list uchar int vertex_indices\n\
              end_header\n"
                .to_vec();
        v.extend_from_slice(&[0xff, 0xfe, 0x0a]);
        let e = read(&v).unwrap_err();
        assert!(malformed(&e).contains("not valid UTF-8"), "{e}");
    }

    // -- binary -------------------------------------------------------------

    const BIN_HEAD: &str = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                            property float x\nproperty float y\nproperty float z\n\
                            element face 1\nproperty list uchar int vertex_indices\nend_header\n";

    fn bin_body() -> Buf {
        Buf::default()
            .f32(0.0).f32(0.0).f32(0.0)
            .f32(1.0).f32(0.0).f32(0.0)
            .f32(0.0).f32(1.0).f32(0.0)
            .u8(3).i32(0).i32(1).i32(2)
    }

    #[test]
    fn binary_minimal() {
        let m = read(&with_header(BIN_HEAD, bin_body())).unwrap();
        assert_eq!(m.positions, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        assert_eq!(m.indices, vec![0, 1, 2]);
    }

    #[test]
    fn binary_properties_in_any_order_with_extras() {
        // 23 bytes per vertex: uchar, float, double, int, float, uchar, uchar.
        // Every width has to be right or the whole record stream shifts.
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 2\n\
                    property uchar red\nproperty float y\nproperty double z\n\
                    property int junk\nproperty float x\nproperty uchar green\n\
                    property uchar blue\nelement face 1\n\
                    property list uchar uint vertex_indices\nend_header\n";
        let body = Buf::default()
            .u8(255).f32(2.0).f64(3.0).i32(-7).f32(1.0).u8(128).u8(0)
            .u8(17).f32(5.0).f64(6.0).i32(-7).f32(4.0).u8(0).u8(0)
            .u8(3).u32(0).u32(1).u32(0);
        let m = read(&with_header(head, body)).unwrap();
        assert_eq!(m.positions, vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
        assert_eq!(m.base_color, Some([1.0, 128.0 / 255.0, 0.0, 1.0]));
    }

    #[test]
    fn binary_unknown_element_with_a_list_is_walked() {
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                    property float x\nproperty float y\nproperty float z\n\
                    element edge 2\nproperty list uchar int ends\nproperty ushort flags\n\
                    element face 1\nproperty list uchar int vertex_indices\nend_header\n";
        let body = Buf::default()
            .f32(0.0).f32(0.0).f32(0.0)
            .f32(1.0).f32(0.0).f32(0.0)
            .f32(0.0).f32(1.0).f32(0.0)
            // edge 0: two ends + flags; edge 1: three ends + flags
            .u8(2).i32(0).i32(1).u8(9).u8(9)
            .u8(3).i32(0).i32(1).i32(2).u8(9).u8(9)
            .u8(3).i32(0).i32(1).i32(2);
        let m = read(&with_header(head, body)).unwrap();
        assert_eq!(m.positions.len(), 3);
        assert_eq!(m.indices, vec![0, 1, 2]);
    }

    #[test]
    fn binary_unknown_fixed_size_element_is_skipped_wholesale() {
        let head = "ply\nformat binary_little_endian 1.0\nelement junk 2\n\
                    property double a\nproperty uchar b\nelement vertex 3\n\
                    property float x\nproperty float y\nproperty float z\n\
                    element face 1\nproperty list uchar int vertex_indices\nend_header\n";
        let mut body = Buf::default().f64(1.0).u8(1).f64(2.0).u8(2);
        body.0.extend_from_slice(&bin_body().0);
        let m = read(&with_header(head, body)).unwrap();
        assert_eq!(m.positions.len(), 3);
    }

    #[test]
    fn binary_double_positions() {
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                    property double x\nproperty double y\nproperty double z\n\
                    element face 1\nproperty list uchar int vertex_indices\nend_header\n";
        let body = Buf::default()
            .f64(0.5).f64(0.25).f64(0.125)
            .f64(1.0).f64(0.0).f64(0.0)
            .f64(0.0).f64(1.0).f64(0.0)
            .u8(3).i32(0).i32(1).i32(2);
        let m = read(&with_header(head, body)).unwrap();
        assert_eq!(m.positions[0], [0.5, 0.25, 0.125]);
    }

    #[test]
    fn binary_truncated_mid_vertex() {
        // The fixed-size check catches this before a single record is read,
        // which is the whole point of having it.
        let full = with_header(BIN_HEAD, bin_body());
        let cut = &full[..full.len() - 20];
        let e = read(cut).unwrap_err();
        let d = malformed(&e);
        assert!(d.contains("element 'vertex' declares 3 records"), "{d}");
        assert!(d.contains("bytes remain"), "{d}");
    }

    #[test]
    fn binary_truncated_where_no_up_front_check_can_see_it() {
        // The vertex element's bytes are all present — the up-front check passes
        // because it counts every remaining byte — and the body then runs out at
        // the face element's list count. Only the cursor catches this one.
        let full = with_header(BIN_HEAD, bin_body());
        let cut = &full[..full.len() - 13];
        let e = read(cut).unwrap_err();
        assert!(malformed(&e).contains("truncated body"), "{e}");
    }

    #[test]
    fn binary_truncated_at_a_face_list() {
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                    property float x\nproperty float y\nproperty float z\n\
                    element face 1\nproperty list uchar int vertex_indices\nend_header\n";
        let body = Buf::default()
            .f32(0.0).f32(0.0).f32(0.0)
            .f32(1.0).f32(0.0).f32(0.0)
            .f32(0.0).f32(1.0).f32(0.0)
            .u8(3).i32(0).i32(1); // one index short
        let e = read(&with_header(head, body)).unwrap_err();
        assert!(malformed(&e).contains("face record 0"), "{e}");
    }

    #[test]
    fn binary_hostile_vertex_count_is_rejected_up_front() {
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 4000000000\n\
                    property float x\nproperty float y\nproperty float z\n\
                    element face 0\nproperty list uchar int vertex_indices\nend_header\n";
        let body = Buf::default().f32(0.0).f32(0.0).f32(0.0);
        let e = read(&with_header(head, body)).unwrap_err();
        let d = malformed(&e);
        assert!(d.contains("declares 4000000000 records"), "{d}");
        assert!(d.contains("bytes remain"), "{d}");
    }

    #[test]
    fn binary_hostile_list_length_is_rejected_before_allocating() {
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                    property float x\nproperty float y\nproperty float z\n\
                    element face 1\nproperty list uint uint vertex_indices\nend_header\n";
        let body = Buf::default()
            .f32(0.0).f32(0.0).f32(0.0)
            .f32(1.0).f32(0.0).f32(0.0)
            .f32(0.0).f32(1.0).f32(0.0)
            .u32(u32::MAX)
            .u32(0).u32(1).u32(2);
        let e = read(&with_header(head, body)).unwrap_err();
        assert!(malformed(&e).contains("declares 4294967295 items"), "{e}");
    }

    #[test]
    fn binary_trailing_padding_is_tolerated() {
        let mut full = with_header(BIN_HEAD, bin_body());
        full.extend_from_slice(&[0u8; 16]);
        assert_eq!(read(&full).unwrap().positions.len(), 3);
    }

    #[test]
    fn binary_face_index_out_of_range() {
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                    property float x\nproperty float y\nproperty float z\n\
                    element face 1\nproperty list uchar int vertex_indices\nend_header\n";
        let body = Buf::default()
            .f32(0.0).f32(0.0).f32(0.0)
            .f32(1.0).f32(0.0).f32(0.0)
            .f32(0.0).f32(1.0).f32(0.0)
            .u8(3).i32(0).i32(1).i32(7);
        let e = read(&with_header(head, body)).unwrap_err();
        assert!(malformed(&e).contains("index 7 out of range for 3 vertices"), "{e}");
    }

    #[test]
    fn binary_negative_face_index() {
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                    property float x\nproperty float y\nproperty float z\n\
                    element face 1\nproperty list uchar int vertex_indices\nend_header\n";
        let body = Buf::default()
            .f32(0.0).f32(0.0).f32(0.0)
            .f32(1.0).f32(0.0).f32(0.0)
            .f32(0.0).f32(1.0).f32(0.0)
            .u8(3).i32(0).i32(-1).i32(2);
        let e = read(&with_header(head, body)).unwrap_err();
        assert!(malformed(&e).contains("out of range"), "{e}");
    }

    #[test]
    fn binary_face_with_two_vertices() {
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                    property float x\nproperty float y\nproperty float z\n\
                    element face 1\nproperty list uchar int vertex_indices\nend_header\n";
        let body = Buf::default()
            .f32(0.0).f32(0.0).f32(0.0)
            .f32(1.0).f32(0.0).f32(0.0)
            .f32(0.0).f32(1.0).f32(0.0)
            .u8(2).i32(0).i32(1);
        let e = read(&with_header(head, body)).unwrap_err();
        assert!(malformed(&e).contains("face has 2 vertices"), "{e}");
    }

    #[test]
    fn face_element_may_precede_vertex_element() {
        let head = "ply\nformat binary_little_endian 1.0\nelement face 1\n\
                    property list uchar int vertex_indices\nelement vertex 3\n\
                    property float x\nproperty float y\nproperty float z\nend_header\n";
        let body = Buf::default()
            .u8(3).i32(0).i32(1).i32(2)
            .f32(0.0).f32(0.0).f32(0.0)
            .f32(1.0).f32(0.0).f32(0.0)
            .f32(0.0).f32(1.0).f32(0.0);
        let m = read(&with_header(head, body)).unwrap();
        assert_eq!(m.indices, vec![0, 1, 2]);
        assert_eq!(m.positions.len(), 3);
    }

    #[test]
    fn vertex_index_singular_spelling_is_accepted() {
        let head = BIN_HEAD.replace("vertex_indices", "vertex_index");
        let m = read(&with_header(&head, bin_body())).unwrap();
        assert_eq!(m.indices, vec![0, 1, 2]);
    }

    // -- header errors ------------------------------------------------------

    #[test]
    fn empty_input_is_not_this_format() {
        assert!(matches!(read(&[]), Err(MeshError::NotThisFormat { .. })));
    }

    #[test]
    fn wrong_magic_is_not_this_format() {
        let e = read(b"glTF\0\0\0\0rest of a glb").unwrap_err();
        match e {
            MeshError::NotThisFormat { expected, detail } => {
                assert_eq!(expected, super::super::MeshFormat::Ply);
                assert!(detail.contains("expected"), "{detail}");
            }
            other => panic!("expected NotThisFormat, got {other:?}"),
        }
    }

    #[test]
    fn a_prefix_of_ply_is_not_a_ply() {
        assert!(matches!(
            read(b"plyfile\nformat ascii 1.0\nend_header\n"),
            Err(MeshError::NotThisFormat { .. })
        ));
    }

    #[test]
    fn header_without_end_header() {
        let e = read(b"ply\nformat ascii 1.0\nelement vertex 1\n").unwrap_err();
        assert!(malformed(&e).contains("without an end_header"), "{e}");
    }

    #[test]
    fn header_without_a_format_line() {
        let e = read(b"ply\nelement vertex 0\nend_header\n").unwrap_err();
        // The element line comes first and is rejected there; either way the
        // caller learns this is not a PLY we can read.
        assert!(matches!(e, MeshError::NotThisFormat { .. }), "{e:?}");
    }

    #[test]
    fn big_endian_is_named_not_misread() {
        let e = read(b"ply\nformat binary_big_endian 1.0\nend_header\n").unwrap_err();
        match e {
            MeshError::Unsupported(d) => assert!(d.contains("binary_big_endian"), "{d}"),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn unknown_encoding_is_not_this_format() {
        let e = read(b"ply\nformat base64 1.0\nend_header\n").unwrap_err();
        assert!(matches!(e, MeshError::NotThisFormat { .. }), "{e:?}");
    }

    #[test]
    fn future_format_version_is_unsupported() {
        let e = read(b"ply\nformat ascii 2.0\nend_header\n").unwrap_err();
        match e {
            MeshError::Unsupported(d) => assert!(d.contains("2.0"), "{d}"),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn unknown_property_type() {
        let e = read(
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty quad x\nend_header\n0\n",
        )
        .unwrap_err();
        assert!(malformed(&e).contains("unknown property type"), "{e}");
    }

    #[test]
    fn unknown_header_directive() {
        let e = read(b"ply\nformat ascii 1.0\nsurprise 3\nend_header\n").unwrap_err();
        assert!(malformed(&e).contains("unknown header directive"), "{e}");
    }

    #[test]
    fn property_before_any_element() {
        let e = read(b"ply\nformat ascii 1.0\nproperty float x\nend_header\n").unwrap_err();
        assert!(malformed(&e).contains("before any element"), "{e}");
    }

    #[test]
    fn duplicate_element_name() {
        let e = read(
            b"ply\nformat ascii 1.0\nelement vertex 0\nproperty float x\n\
              element vertex 0\nproperty float x\nend_header\n",
        )
        .unwrap_err();
        assert!(malformed(&e).contains("duplicate element"), "{e}");
    }

    #[test]
    fn non_numeric_element_count() {
        let e = read(b"ply\nformat ascii 1.0\nelement vertex lots\nend_header\n").unwrap_err();
        assert!(malformed(&e).contains("is not a number"), "{e}");
    }

    #[test]
    fn float_list_count_type_is_rejected() {
        let e = read(
            b"ply\nformat ascii 1.0\nelement face 1\nproperty list float int vertex_indices\n\
              end_header\n3 0 1 2\n",
        )
        .unwrap_err();
        assert!(malformed(&e).contains("must be an integer type"), "{e}");
    }

    #[test]
    fn header_line_that_is_not_text() {
        let e = read(&[b'p', b'l', b'y', b'\n', 0xff, 0xfe, b'\n']).unwrap_err();
        assert!(malformed(&e).contains("is not text"), "{e}");
    }

    #[test]
    fn a_header_that_never_ends_is_capped() {
        // `ply` magic then a megabyte of newline-separated comments: the cap is
        // what stops this from being a full scan of a hostile file.
        let mut v = b"ply\nformat ascii 1.0\n".to_vec();
        while v.len() < MAX_HEADER_BYTES * 2 {
            v.extend_from_slice(b"comment padding padding padding\n");
        }
        let e = read(&v).unwrap_err();
        assert!(malformed(&e).contains("no end_header within"), "{e}");
    }

    // -- vertex layout errors ----------------------------------------------

    #[test]
    fn vertex_element_missing_a_coordinate() {
        let e = read(
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\n\
              end_header\n0 0\n",
        )
        .unwrap_err();
        assert!(malformed(&e).contains("no 'z' property"), "{e}");
    }

    #[test]
    fn no_vertex_element_at_all() {
        let e = read(
            b"ply\nformat ascii 1.0\nelement face 1\nproperty list uchar int vertex_indices\n\
              end_header\n3 0 1 2\n",
        )
        .unwrap_err();
        assert!(malformed(&e).contains("no 'vertex' element"), "{e}");
    }

    #[test]
    fn partial_normals_are_rejected() {
        let e = read(
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\n\
              property float z\nproperty float nx\nend_header\n0 0 0 1\n",
        )
        .unwrap_err();
        assert!(malformed(&e).contains("some of nx/ny/nz"), "{e}");
    }

    #[test]
    fn partial_colour_is_rejected() {
        let e = read(
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\n\
              property float z\nproperty uchar red\nproperty uchar green\nend_header\n0 0 0 1 2\n",
        )
        .unwrap_err();
        assert!(malformed(&e).contains("some of red/green/blue"), "{e}");
    }

    #[test]
    fn alpha_without_rgb_is_rejected() {
        let e = read(
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\n\
              property float z\nproperty uchar alpha\nend_header\n0 0 0 1\n",
        )
        .unwrap_err();
        assert!(malformed(&e).contains("'alpha' but no red/green/blue"), "{e}");
    }

    #[test]
    fn duplicate_vertex_property() {
        let e = read(
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float x\n\
              property float y\nproperty float z\nend_header\n0 0 0 0\n",
        )
        .unwrap_err();
        assert!(malformed(&e).contains("declares 'x' twice"), "{e}");
    }

    #[test]
    fn coordinate_declared_as_a_list() {
        let e = read(
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty list uchar float x\n\
              property float y\nproperty float z\nend_header\n1 0 0 0\n",
        )
        .unwrap_err();
        assert!(malformed(&e).contains("is a list"), "{e}");
    }

    #[test]
    fn colour_in_an_unscalable_type_is_rejected() {
        let e = read(
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\n\
              property float z\nproperty int red\nproperty int green\nproperty int blue\n\
              end_header\n0 0 0 1 2 3\n",
        )
        .unwrap_err();
        assert!(malformed(&e).contains("PLY colour is uchar"), "{e}");
    }

    #[test]
    fn face_indices_declared_as_a_scalar() {
        let e = read(
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\n\
              property float z\nelement face 1\nproperty int vertex_indices\nend_header\n\
              0 0 0\n0\n",
        )
        .unwrap_err();
        assert!(malformed(&e).contains("must be a list"), "{e}");
    }

    #[test]
    fn face_element_without_an_index_property() {
        let e = read(
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\n\
              property float z\nelement face 1\nproperty uchar flags\nend_header\n0 0 0\n7\n",
        )
        .unwrap_err();
        assert!(malformed(&e).contains("no vertex_indices list property"), "{e}");
    }

    // -- write-side guards --------------------------------------------------

    #[test]
    fn write_rejects_an_invalid_mesh_rather_than_emitting_one() {
        let m = Mesh { positions: vec![[0.0; 3]], indices: vec![0, 1, 2], ..Default::default() };
        assert!(matches!(write(&m), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn a_point_cloud_round_trips_to_zero_faces() {
        // Not this codec's call to reject: `super::read` owns the Empty policy,
        // and rejecting here would make ply the only format that disagrees.
        let m = Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
            indices: vec![],
            ..Default::default()
        };
        let back = read(&write(&m).unwrap()).unwrap();
        assert_eq!(back.positions, m.positions);
        assert!(back.indices.is_empty());
    }

    #[test]
    fn out_of_gamut_colour_is_clamped_not_wrapped() {
        let mut m = tri_mesh();
        m.base_color = Some([2.0, -1.0, f32::NAN, 1.0]);
        let back = read(&write(&m).unwrap()).unwrap();
        assert_eq!(back.base_color, Some([1.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn large_mesh_round_trips_exactly() {
        // Enough vertices that any per-record width mistake compounds visibly.
        let n = 2000u32;
        let positions: Vec<[f32; 3]> =
            (0..n).map(|i| [i as f32 * 0.5, -(i as f32), i as f32 / 3.0]).collect();
        let indices: Vec<u32> = (0..n - 2).flat_map(|i| [i, i + 1, i + 2]).collect();
        let m = Mesh { positions, indices, ..Default::default() };
        let back = read(&write(&m).unwrap()).unwrap();
        assert_eq!(back.positions, m.positions);
        assert_eq!(back.indices, m.indices);
    }
}
