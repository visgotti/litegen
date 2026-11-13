//! PLY (Stanford polygon format): reads ASCII and binary in both byte orders,
//! writes binary little-endian.
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
//! writing needs no swapping. Reading `binary_big_endian` too, which older
//! scanning pipelines and SGI-era tools wrote, does not change that: the byte
//! order we write is chosen by what readers take, not by what we can read.
//!
//! # Texture coordinates and aliases
//!
//! PLY has no one UV spelling. The reader takes the ones three.js (the engine
//! behind the dashboard's viewer) reads — `s|u|texture_u|tx` and
//! `t|v|texture_v|ty`, the first present winning (r170 PLYLoader.js:331-332)
//! — with V flipped to the scene's downward V, and writes `s`/`t`, Blender's
//! spelling. Colour likewise reads `red|diffuse_red|r|diffuse_r` (and so on,
//! PLYLoader.js:333-335), on vertices and on faces; a face's colour colours
//! its corners, each its own vertex. A face's own `texcoord` list is named in
//! a warning, not carried.
//!
//! # Points and lines
//!
//! PLY has one pool of vertices and, over it, the `face` and `edge` elements;
//! the scene has a pool for each primitive. So the reader makes a primitive of
//! each element, over the vertices it uses and no others (the faces' too, when
//! edges come beside them), and a file with faces alone keeps every vertex it
//! has, as before.
//!
//! - **Edges** (`vertex1`, `vertex2`, any integer type, and optionally
//!   `red`/`green`/`blue`) are lines. An edge's colour colours its two ends over
//!   the vertices' own, each end a vertex of its own, as a face's colour
//!   colours its corners. An `edge` element without those two properties is
//!   stepped over by its layout and named in a warning. Edges are charged to
//!   the index budget from the declared count, before one is read.
//! - **A file with no faces and no edges is a point cloud:** a point for each
//!   vertex, with whatever the vertices carry. A vertex no face uses, beside
//!   faces, is a vertex and no point. The same goes for a Gaussian-splat PLY
//!   (3D Gaussian Splatting's training output): its vertices are points with
//!   their positions and, where they have `red`/`green`/`blue`, their colours;
//!   what makes them splats (`f_dc_*`, `f_rest_*`, `opacity`, `scale_*`,
//!   `rot_*`) is named in one warning and not carried.
//! - **The writer** puts the triangles, then the lines, then the points in the
//!   pool, writes the `face` element for the triangles and an `edge` element
//!   for the lines, and no `face` element for points alone. Points beside
//!   triangles or lines would read back as nothing, so [`prepare`] keeps points
//!   only in a scene with neither, and [`losses`] names the ones it leaves out.
//!
//! # Colour
//!
//! Per-vertex colour is carried in both directions, as `uchar` RGBA. PLY has no
//! colour-space field, and every consumer that matters — three.js (the engine
//! behind the dashboard's viewer), MeshLab, Blender — reads those bytes as
//! **sRGB**, so that is how they are written and read: linear colour from the
//! scene is encoded to sRGB on the way out and decoded on the way in, and alpha,
//! which has no transfer curve, is stored as is. Writing linear values straight
//! into the bytes — what this codec once did — renders every colour visibly too
//! dark. A material's base colour has nowhere else to go in PLY, so it is baked
//! into the vertex colours of the primitives that use it (see
//! `pose::degrade`).

use super::ir::Mesh;
use super::math::{linear_to_srgb, srgb_to_linear};
use super::scene::{Loss, LossKind, Node, Primitive, Scene, SceneMesh, Topology, MAX_SCENE_VERTICES};
use super::MeshError;

/// A real PLY header is a few hundred bytes. This cap is what stops a file that
/// opens with `ply` and never closes its header from making us scan — and
/// UTF-8-check — a hundred megabytes of binary looking for a line that is not
/// there.
const MAX_HEADER_BYTES: usize = 1 << 20;

/// Cap on the `comment name ...` line we write. A name is a label; an unbounded
/// one in a header is just a bigger blast radius for no benefit.
const MAX_NAME_CHARS: usize = 256;

/// How much of an offending token an error message may quote.
///
/// Error text goes to logs, to the operator, and potentially into a response,
/// so an unbounded quote makes the input's size the error's size. The realistic
/// case is not an attack: it is a vendor returning a minified JSON or HTML
/// error page where a mesh was expected, which then comes back as a
/// `NotThisFormat` whose Display string is the entire page.
const MAX_QUOTED: usize = 80;

/// Quote a header token for an error message, truncated on a char boundary.
fn quoted(s: &str) -> String {
    if s.len() <= MAX_QUOTED {
        return format!("{s:?}");
    }
    let mut end = MAX_QUOTED;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{:?}… ({} bytes)", &s[..end], s.len())
}

fn not_ply(detail: impl Into<String>) -> MeshError {
    MeshError::NotThisFormat { expected: super::MeshFormat::Ply, detail: detail.into() }
}

fn bad(detail: impl Into<String>) -> MeshError {
    MeshError::Malformed(detail.into())
}

// ---------------------------------------------------------------------------
// Header model
// ---------------------------------------------------------------------------

/// The byte order of a binary body, as its header's `format` line declares
/// it. It matters only where bytes become a value, in `Scalar::decode`:
/// record layout, skipping, list bounds and budgets are counted in bytes, so
/// they are the same in both orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ByteOrder {
    Little,
    Big,
}

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

    /// Decode one value stored in `order`. Returns `None` only when `b` is
    /// shorter than the type, which is how truncation is reported without
    /// panicking; past the `get(..n)` the slice is exactly `n` bytes, so the
    /// indexing below cannot be out of bounds.
    ///
    /// `byte(i)` is the value's i-th byte counting from the least significant,
    /// whichever order the file stores them in, so one table decodes both
    /// orders and the two cannot drift apart. A one-byte value reads the same
    /// in both.
    fn decode(self, b: &[u8], order: ByteOrder) -> Option<f64> {
        let s = b.get(..self.size())?;
        let byte = |i: usize| match order {
            ByteOrder::Little => s[i],
            ByteOrder::Big => s[s.len() - 1 - i],
        };
        Some(match self {
            Scalar::I8 => byte(0) as i8 as f64,
            Scalar::U8 => byte(0) as f64,
            Scalar::I16 => i16::from_le_bytes([byte(0), byte(1)]) as f64,
            Scalar::U16 => u16::from_le_bytes([byte(0), byte(1)]) as f64,
            Scalar::I32 => i32::from_le_bytes([byte(0), byte(1), byte(2), byte(3)]) as f64,
            Scalar::U32 => u32::from_le_bytes([byte(0), byte(1), byte(2), byte(3)]) as f64,
            Scalar::F32 => f32::from_le_bytes([byte(0), byte(1), byte(2), byte(3)]) as f64,
            Scalar::F64 => f64::from_le_bytes([
                byte(0), byte(1), byte(2), byte(3), byte(4), byte(5), byte(6), byte(7),
            ]),
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
    BinaryBe,
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
    // Names seen, for the repeated-element check in one pass (scanning every
    // earlier element per element made a 1 MiB header take 11.5 s).
    let mut element_names: std::collections::HashSet<String> = std::collections::HashSet::new();
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
        // Search only within the header budget, not to the end of the file.
        // Bounding `pos` alone does not bound the WORK: a file that opens with
        // `ply` and then never emits another newline makes both the scan below
        // and the UTF-8 check that follows it run over the whole remainder
        // before `pos` has a chance to exceed the cap. Measured at 120 MB of
        // input: 0.57 s and 361 MB resident, on the poll path, from bytes a
        // vendor supplied.
        let window_end = bytes.len().min(pos.saturating_add(MAX_HEADER_BYTES));
        let (raw, next) = match bytes[pos..window_end].iter().position(|b| *b == b'\n') {
            Some(k) => (&bytes[pos..pos + k], pos + k + 1),
            None if window_end < bytes.len() => {
                return Err(bad(format!(
                    "header line {} exceeds the {MAX_HEADER_BYTES}-byte limit",
                    lineno + 1
                )));
            }
            // A final line with no newline can only be a truncated header: the
            // body, if any, starts after end_header's own newline.
            None => (&bytes[pos..window_end], bytes.len()),
        };
        pos = next;
        lineno += 1;

        // The magic is checked on the raw bytes, BEFORE decoding as text: a
        // GLB or a binary STL handed over as PLY has a first line that is not
        // UTF-8 at all, and that is the caller's wrong format tag
        // (`NotThisFormat`), not a broken PLY header (`Malformed`).
        if lineno == 1 {
            let first = raw.strip_suffix(b"\r").unwrap_or(raw);
            if first.trim_ascii() != b"ply" {
                let shown = String::from_utf8_lossy(&first[..first.len().min(MAX_QUOTED)]).into_owned();
                return Err(not_ply(format!("first line is {}, expected \"ply\"", quoted(&shown))));
            }
            continue;
        }

        let line = std::str::from_utf8(raw)
            .map_err(|_| bad(format!("header line {lineno} is not text")))?
            .trim_end_matches('\r')
            .trim();

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
                    "binary_big_endian" => Encoding::BinaryBe,
                    other => {
                        return Err(not_ply(format!("unknown PLY encoding {}", quoted(other))));
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
                if !element_names.insert(ename.to_string()) {
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
                    //
                    // The ITEM type has to be checked too, not just the count:
                    // without it `property list uchar float vertex_indices`
                    // parsed happily and `read_face` truncated 0.9 to vertex 0
                    // — the exact guess this rejection exists to prevent, made
                    // silently, on geometry.
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
                return Err(bad(format!("unknown header directive {} on line {lineno}", quoted(other))));
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
    /// A list's items, refused before any is read when it holds more than
    /// `max` of them.
    fn list(&mut self, count: Scalar, item: Scalar, max: usize) -> Result<Vec<f64>, MeshError>;
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

/// A binary body, in the byte order its header declares. Only `scalar` reads
/// the order, and a list's count goes through `scalar` too, so a count is in
/// the file's order before it is bounded.
struct Binary<'a> {
    buf: &'a [u8],
    pos: usize,
    order: ByteOrder,
}

impl<'a> Binary<'a> {
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

impl<'a> Body for Binary<'a> {
    fn start_record(&mut self) -> Result<(), MeshError> {
        Ok(())
    }

    fn end_record(&mut self) -> Result<(), MeshError> {
        Ok(())
    }

    fn scalar(&mut self, t: Scalar) -> Result<f64, MeshError> {
        let b = self.take(t.size())?;
        t.decode(b, self.order).ok_or_else(|| bad("truncated scalar"))
    }

    fn skip_scalar(&mut self, t: Scalar) -> Result<(), MeshError> {
        self.take(t.size())?;
        Ok(())
    }

    fn list(&mut self, count: Scalar, item: Scalar, max: usize) -> Result<Vec<f64>, MeshError> {
        let n = self.list_len(count, item)?;
        if n > max {
            return Err(too_long(n, max));
        }
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

impl<'a> Binary<'a> {
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

    fn list(&mut self, count: Scalar, item: Scalar, max: usize) -> Result<Vec<f64>, MeshError> {
        let n = self.list_len(count)?;
        if n > max {
            return Err(too_long(n, max));
        }
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
const SLOT_S: usize = 10;
const SLOTS: usize = 12;

/// Each slot's property names, the first one present winning: the spec's
/// spelling, then the ones three.js r170 PLYLoader.js:325-335 also reads.
/// A file naming both `red` and `diffuse_red` reads `red`; the other is
/// skipped by its declared width like any property we do not use.
const SLOT_NAMES: [&[&str]; SLOTS] = [
    &["x", "px", "posx"],
    &["y", "py", "posy"],
    &["z", "pz", "posz"],
    &["nx", "normalx"],
    &["ny", "normaly"],
    &["nz", "normalz"],
    &["red", "diffuse_red", "r", "diffuse_r"],
    &["green", "diffuse_green", "g", "diffuse_g"],
    &["blue", "diffuse_blue", "b", "diffuse_b"],
    &["alpha"],
    &["s", "u", "texture_u", "tx"],
    &["t", "v", "texture_v", "ty"],
];

/// The property each slot reads from, by [`SLOT_NAMES`]; slots in `wanted`
/// only.
fn slots_of(elem: &Element, wanted: std::ops::Range<usize>) -> Result<[Option<usize>; SLOTS], MeshError> {
    let mut at: [Option<usize>; SLOTS] = [None; SLOTS];
    for slot in wanted {
        for name in SLOT_NAMES[slot] {
            let mut found = elem.props.iter().enumerate().filter(|(_, p)| p.name == *name).map(|(i, _)| i);
            if let Some(i) = found.next() {
                if found.next().is_some() {
                    return Err(bad(format!("{} element declares '{name}' twice", elem.name)));
                }
                at[slot] = Some(i);
                break;
            }
        }
    }
    Ok(at)
}

/// Divisor per colour slot (r, g, b, a), from each component's declared type.
fn color_scales(elem: &Element, at: &[Option<usize>; SLOTS]) -> Result<[f64; 4], MeshError> {
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
    Ok(color_scale)
}

struct VertexPlan {
    /// Property index -> slot, `None` for properties read past and discarded.
    slot_of: Vec<Option<usize>>,
    has_normals: bool,
    has_color: bool,
    has_alpha: bool,
    has_uv: bool,
    /// Only one of a UV pair is present: named in a warning.
    half_uv: Option<String>,
    /// Divisor per colour slot (r, g, b, a), from each component's declared
    /// type: colour is conventionally `uchar`, but float PLY colour is already
    /// 0..1 and dividing it by 255 would black out the mesh.
    color_scale: [f64; 4],
}

fn plan_vertex(elem: &Element) -> Result<VertexPlan, MeshError> {
    let mut slot_of = vec![None; elem.props.len()];
    let at = slots_of(elem, 0..SLOTS)?;
    for (slot, i) in at.iter().enumerate() {
        let Some(i) = *i else { continue };
        let p = &elem.props[i];
        // A list named `x` is not a coordinate under any reading of the spec,
        // and treating it as one would consume the wrong bytes.
        if let Kind::List { .. } = p.kind {
            return Err(bad(format!("vertex property '{}' is a list; it must be a scalar", p.name)));
        }
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

    let color_scale = color_scales(elem, &at)?;
    let has_uv = at[SLOT_S].is_some() && at[SLOT_S + 1].is_some();
    let half_uv = match (at[SLOT_S], at[SLOT_S + 1]) {
        (Some(i), None) | (None, Some(i)) => Some(elem.props[i].name.clone()),
        _ => None,
    };
    if !has_uv {
        for s in [SLOT_S, SLOT_S + 1] {
            if let Some(i) = at[s] {
                slot_of[i] = None;
            }
        }
    }

    Ok(VertexPlan {
        slot_of,
        has_normals: normal_present == 3,
        has_color: color_present == 3,
        has_alpha,
        has_uv,
        half_uv,
        color_scale,
    })
}

/// Index of the face element's vertex-index list property.
///
/// `vertex_indices` is the spec spelling; `vertex_index` is what several widely
/// used exporters write, and both are common enough in files we will actually be
/// handed that rejecting the second would be pedantry with a support cost.
/// What a face record carries: its vertex-index list, and a colour.
struct FacePlan {
    indices: usize,
    /// The colour properties by slot (r, g, b, a), with their divisors.
    color: Option<([usize; 3], Option<usize>, [f64; 4])>,
    /// A per-face `texcoord` list, which is named and not carried.
    texcoord: bool,
}

fn plan_face(elem: &Element) -> Result<FacePlan, MeshError> {
    let indices = face_indices(elem)?;
    let at = slots_of(elem, SLOT_R..SLOT_A + 1)?;
    let scalar = |i: Option<usize>| i.filter(|i| matches!(elem.props[*i].kind, Kind::Scalar(_)));
    let color = match (scalar(at[SLOT_R]), scalar(at[SLOT_R + 1]), scalar(at[SLOT_R + 2])) {
        (Some(r), Some(g), Some(b)) => Some(([r, g, b], scalar(at[SLOT_A]), color_scales(elem, &at)?)),
        _ => None,
    };
    let texcoord = elem.props.iter().any(|p| p.name == "texcoord" && matches!(p.kind, Kind::List { .. }));
    Ok(FacePlan { indices, color, texcoord })
}

/// Index of a face (or strip) element's vertex-index list property.
fn face_indices(elem: &Element) -> Result<usize, MeshError> {
    for (i, p) in elem.props.iter().enumerate() {
        if matches!(p.name.as_str(), "vertex_indices" | "vertex_index" | "vertex_indexes") {
            return match p.kind {
                // The ITEM type matters as much as the count type: without this
                // check `property list uchar float vertex_indices` parsed
                // happily and the reader truncated 0.9 to vertex 0 — the exact
                // silent guess the count-type rejection exists to prevent, made
                // on geometry. Checked here rather than at property-parse time
                // so that a coordinate wrongly declared as a list still gets its
                // own, more specific diagnosis.
                Kind::List { item, .. } if item.is_float() => Err(bad(format!(
                    "face property '{}' has {} items; vertex indices must be an integer type",
                    p.name,
                    item.name()
                ))),
                Kind::List { .. } => Ok(i),
                Kind::Scalar(_) => Err(bad(format!(
                    "face property '{}' is a scalar; it must be a list",
                    p.name
                ))),
            };
        }
    }
    Err(bad(format!("{} element has no vertex_indices list property", elem.name)))
}

/// What an `edge` record carries: its two ends, and a colour.
struct EdgePlan {
    /// The `vertex1` and `vertex2` properties.
    ends: [usize; 2],
    /// The colour properties by slot (r, g, b, a), with their divisors.
    color: Option<([usize; 3], Option<usize>, [f64; 4])>,
}

/// The plan for an `edge` element, or `None` for one with neither `vertex1`
/// nor `vertex2`, which has no way to be walked to a segment (the PLY paper
/// names those two, "two pointers to the vertices that delineate the edge").
fn plan_edge(elem: &Element) -> Result<Option<EdgePlan>, MeshError> {
    let find = |name: &str| elem.props.iter().position(|p| p.name == name);
    let ends = match (find("vertex1"), find("vertex2")) {
        (None, None) => return Ok(None),
        (Some(a), Some(b)) => [a, b],
        (Some(_), None) => return Err(bad("edge element has vertex1 but not vertex2")),
        (None, Some(_)) => return Err(bad("edge element has vertex2 but not vertex1")),
    };
    for i in ends {
        let p = &elem.props[i];
        match p.kind {
            Kind::List { .. } => return Err(bad(format!("edge property '{}' is a list; it must be a scalar", p.name))),
            Kind::Scalar(t) if t.is_float() => {
                return Err(bad(format!("edge property '{}' has {} type; vertex indices must be an integer type", p.name, t.name())))
            }
            Kind::Scalar(_) => {}
        }
    }
    let at = slots_of(elem, SLOT_R..SLOT_A + 1)?;
    let scalar = |i: Option<usize>| i.filter(|i| matches!(elem.props[*i].kind, Kind::Scalar(_)));
    let color = match (scalar(at[SLOT_R]), scalar(at[SLOT_R + 1]), scalar(at[SLOT_R + 2])) {
        (Some(r), Some(g), Some(b)) => Some(([r, g, b], scalar(at[SLOT_A]), color_scales(elem, &at)?)),
        _ => None,
    };
    Ok(Some(EdgePlan { ends, color }))
}

/// Whether a property name is one of a Gaussian splat's: the 3D Gaussian
/// Splatting training output holds `f_dc_*` (spherical-harmonic colour),
/// `f_rest_*`, `opacity`, `scale_*` and `rot_*` on every vertex. Returns the
/// group's name as the warning spells it.
fn splat_group(name: &str) -> Option<&'static str> {
    let numbered = |prefix: &str| name.strip_prefix(prefix).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    if numbered("f_dc_") {
        Some("f_dc_*")
    } else if numbered("f_rest_") {
        Some("f_rest_*")
    } else if name == "opacity" {
        Some("opacity")
    } else if numbered("scale_") {
        Some("scale_*")
    } else if numbered("rot_") {
        Some("rot_*")
    } else {
        None
    }
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
        MeshError::Unsupported(d) => MeshError::Unsupported(format!("{elem} record {i}: {d}")),
        other => other,
    }
}

/// A face list longer than the corners whose fan the mesh's index ceiling
/// still has room for.
fn too_long(n: usize, max: usize) -> MeshError {
    MeshError::Unsupported(format!(
        "a face of {n} corners; only {max} fit before its triangles pass the {}-index ceiling",
        super::scene::MAX_SCENE_INDICES
    ))
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

/// One face: its triangles onto `out`, and its colour when the plan has one.
/// Returns whether it was fanned without knowing its shape: concave past the
/// ear-clipping cap, or (the second flag) read before its vertices.
fn read_face<B: Body>(
    b: &mut B,
    elem: &Element,
    plan: &FacePlan,
    positions: &[[f32; 3]],
    out: &mut Vec<u32>,
    reserved: usize,
    color: &mut [f64; 4],
) -> Result<(bool, bool), MeshError> {
    let idx_prop = plan.indices;
    let mut corners: Vec<f64> = Vec::new();
    let slot = |i: usize| -> Option<usize> {
        let (rgb, a, _) = plan.color.as_ref()?;
        rgb.iter().position(|&p| p == i).or_else(|| (*a == Some(i)).then_some(3))
    };
    b.start_record()?;
    for (i, p) in elem.props.iter().enumerate() {
        match p.kind {
            Kind::Scalar(t) => match slot(i) {
                Some(k) => color[k] = b.scalar(t)?,
                None => b.skip_scalar(t)?,
            },
            Kind::List { count, item } => {
                if i == idx_prop {
                    // Counted against the ceiling across every face (and the
                    // edges already read, `reserved`), before a corner is
                    // read: a fan of n corners is 3(n − 2) indices.
                    let room = super::scene::MAX_SCENE_INDICES.saturating_sub(out.len().saturating_add(reserved)) / 3 + 2;
                    corners = b.list(count, item, room)?;
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
    // Fanned when convex, ear-clipped when not (`polygon`), winding kept. A
    // face read before its vertices (legal, unusual) can only be fanned; the
    // range check after the body catches indices past the vertices.
    let pts: Option<Vec<[f64; 3]>> = idx.iter().map(|&i| positions.get(i as usize).map(|p| p.map(f64::from))).collect();
    let blind = pts.is_none() && idx.len() > 3;
    let (tris, fanned) = match pts {
        Some(pts) if idx.len() > 3 => super::polygon::triangles(&pts),
        _ => ((1..idx.len() - 1).map(|k| [0, k, k + 1]).collect(), false),
    };
    for t in tris {
        out.extend_from_slice(&[idx[t[0]], idx[t[1]], idx[t[2]]]);
    }
    Ok((fanned, blind))
}

/// One triangle strip (the PLY paper's `tristrips` element) onto `out`. `-1`
/// restarts the strip; within one, triangle k is (k, k+1, k+2) for even k and
/// (k+1, k, k+2) for odd k, so every triangle keeps the strip's winding. A
/// triangle with a repeated corner — how strips are stitched — draws nothing
/// and is dropped.
fn read_strip<B: Body>(b: &mut B, elem: &Element, idx_prop: usize, out: &mut Vec<u32>, reserved: usize) -> Result<(), MeshError> {
    let mut list: Vec<f64> = Vec::new();
    b.start_record()?;
    for (i, p) in elem.props.iter().enumerate() {
        match p.kind {
            Kind::Scalar(t) => b.skip_scalar(t)?,
            Kind::List { count, item } if i == idx_prop => {
                // A strip of n corners is at most 3(n − 2) indices.
                let room = super::scene::MAX_SCENE_INDICES.saturating_sub(out.len().saturating_add(reserved)) / 3 + 2;
                list = b.list(count, item, room)?;
            }
            Kind::List { count, item } => b.skip_list(count, item)?,
        }
    }
    b.end_record()?;
    let mut strip: Vec<u32> = Vec::new();
    for &v in &list {
        if v == -1.0 {
            strip.clear();
            continue;
        }
        if !(0.0..=(u32::MAX as f64)).contains(&v) {
            return Err(bad(format!("strip vertex index {v} is out of range")));
        }
        strip.push(v as u32);
        let k = strip.len();
        if k >= 3 {
            let (a, m, c) = (strip[k - 3], strip[k - 2], strip[k - 1]);
            if a != m && m != c && a != c {
                out.extend_from_slice(&if (k - 3) % 2 == 0 { [a, m, c] } else { [m, a, c] });
            }
        }
    }
    Ok(())
}

/// One edge: its two ends onto `out`, and its colour when the plan has one.
fn read_edge<B: Body>(b: &mut B, elem: &Element, plan: &EdgePlan, out: &mut Vec<u32>, color: &mut [f64; 4]) -> Result<(), MeshError> {
    let slot = |i: usize| -> Option<usize> {
        let (rgb, a, _) = plan.color.as_ref()?;
        rgb.iter().position(|&p| p == i).or_else(|| (*a == Some(i)).then_some(3))
    };
    let mut ends = [0.0f64; 2];
    b.start_record()?;
    for (i, p) in elem.props.iter().enumerate() {
        match p.kind {
            Kind::Scalar(t) => {
                if let Some(e) = plan.ends.iter().position(|&x| x == i) {
                    ends[e] = b.scalar(t)?;
                } else if let Some(k) = slot(i) {
                    color[k] = b.scalar(t)?;
                } else {
                    b.skip_scalar(t)?;
                }
            }
            Kind::List { count, item } => b.skip_list(count, item)?,
        }
    }
    b.end_record()?;
    for c in ends {
        if !(0.0..=(u32::MAX as f64)).contains(&c) {
            return Err(bad(format!("edge vertex index {c} is out of range")));
        }
        out.push(c as u32);
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

/// One primitive's worth of a file: its vertices, what they carry, and the
/// indices over them.
struct PlyPart {
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    uvs: Option<Vec<[f32; 2]>>,
    colors: Option<Vec<[f32; 4]>>,
    indices: Vec<u32>,
}

/// Everything a PLY file carries that the scene models.
struct PlyData {
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    /// Scene convention: V runs down the image.
    uvs: Option<Vec<[f32; 2]>>,
    /// Linear RGBA, decoded from the file's sRGB.
    colors: Option<Vec<[f32; 4]>>,
    indices: Vec<u32>,
    /// What `indices` draw: triangles, or, in a file with no faces, segments
    /// (its edges) or points (each vertex).
    topology: Topology,
    /// The edges of a file that also has faces: a primitive of their own, as
    /// the faces' are, each over the vertices it uses.
    lines: Option<PlyPart>,
    name: Option<String>,
    /// Concave faces too large to ear-clip, fanned instead.
    fanned_concave: usize,
    warnings: Vec<String>,
}

/// One stored colour component, as a 0..1 value in the scene's linear space.
///
/// `scale` is the component type's full-scale value (255 for `uchar`, 1 for
/// float). RGB is sRGB-encoded in the file and decoded here; alpha has no
/// transfer curve. Out-of-range and non-finite values clamp rather than wrap.
fn decode_color(raw: f64, scale: f64, alpha: bool) -> f32 {
    let v = raw / scale;
    let v = if v.is_finite() { v.clamp(0.0, 1.0) as f32 } else { 0.0 };
    if alpha {
        v
    } else {
        srgb_to_linear(v)
    }
}

/// `a`'s entry for each of `ends`, in order.
fn gather<T: Copy>(a: &[T], ends: &[u32]) -> Vec<T> {
    ends.iter().map(|&i| a[i as usize]).collect()
}

/// The vertices `ends` name and no others, in the order the file has them,
/// with `ends` renumbered over them. `ends` are indices into `positions`,
/// which the caller has checked.
fn compact(
    positions: &[[f32; 3]],
    normals: Option<&[[f32; 3]]>,
    uvs: Option<&[[f32; 2]]>,
    colors: Option<&[[f32; 4]]>,
    ends: &[u32],
) -> PlyPart {
    let mut used = vec![false; positions.len()];
    for &i in ends {
        used[i as usize] = true;
    }
    let mut number = vec![0u32; positions.len()];
    let mut kept: Vec<u32> = Vec::new();
    for (v, _) in used.iter().enumerate().filter(|(_, u)| **u) {
        number[v] = kept.len() as u32;
        kept.push(v as u32);
    }
    PlyPart {
        positions: gather(positions, &kept),
        normals: normals.map(|a| gather(a, &kept)),
        uvs: uvs.map(|a| gather(a, &kept)),
        colors: colors.map(|a| gather(a, &kept)),
        indices: ends.iter().map(|&i| number[i as usize]).collect(),
    }
}

fn read_body<B: Body>(b: &mut B, h: &Header) -> Result<PlyData, MeshError> {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    // (first triangle, triangles, colour) for each coloured face.
    let mut face_colors: Vec<(usize, usize, [f32; 4])> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    // The edges, as the pairs of vertices they join, and a colour for each
    // when the element carries one.
    let mut edge_ends: Vec<u32> = Vec::new();
    let mut edge_colors: Vec<[f32; 4]> = Vec::new();
    // What the faces and strips declared: a file that declares none is a
    // point cloud, one that declares some and draws none is not.
    let mut declared_faces = 0usize;
    // Polygons read before the vertex element, fanned not knowing their shape.
    let mut fanned_blind = 0usize;
    let mut saw_vertex = false;
    let mut fanned_concave = 0usize;

    for elem in &h.elements {
        b.check_element(elem)?;
        match elem.name.as_str() {
            "vertex" => {
                saw_vertex = true;
                if elem.count > super::scene::MAX_SCENE_VERTICES {
                    return Err(MeshError::Unsupported(format!(
                        "{} vertices, past the {}-vertex ceiling",
                        elem.count,
                        super::scene::MAX_SCENE_VERTICES
                    )));
                }
                let plan = plan_vertex(elem)?;
                if let Some(n) = &plan.half_uv {
                    warnings.push(format!("vertex property '{n}' has no partner coordinate; texture coordinates were not read"));
                }
                // A Gaussian splat (3D Gaussian Splatting's training output)
                // is read as the vertices it is placed at (points, where the
                // file has no faces or edges); what makes it a splat is not
                // carried. The warning is made here, before the faces are
                // known, so it does not say which.
                let groups: Vec<&'static str> = ["f_dc_*", "f_rest_*", "opacity", "scale_*", "rot_*"]
                    .into_iter()
                    .filter(|g| elem.props.iter().any(|p| splat_group(&p.name) == Some(*g)))
                    .collect();
                if groups.iter().any(|g| matches!(*g, "f_dc_*" | "scale_*" | "rot_*")) {
                    warnings.push(format!(
                        "the vertices carry a Gaussian splat's properties ({}), which are not carried; the vertices are read without them",
                        groups.join(", ")
                    ));
                }
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
                    if plan.has_color {
                        let c = |slot: usize, k: usize| decode_color(v[slot], plan.color_scale[k], k == 3);
                        let a = if plan.has_alpha { c(SLOT_A, 3) } else { 1.0 };
                        colors.push([c(SLOT_R, 0), c(SLOT_R + 1, 1), c(SLOT_R + 2, 2), a]);
                    }
                    if plan.has_uv {
                        // V up in the file (Blender writes it so, three.js
                        // reads it so); down in the scene.
                        uvs.push([f(SLOT_S, "s")?, 1.0 - f(SLOT_S + 1, "t")?]);
                    }
                }
            }
            "face" => {
                let plan = plan_face(elem)?;
                if plan.texcoord {
                    warnings.push("the faces carry their own texture coordinates (`texcoord`), which are not carried".into());
                }
                declared_faces = declared_faces.saturating_add(elem.count);
                for i in 0..elem.count {
                    let before = indices.len();
                    let mut c = [0.0f64; 4];
                    let (fanned, blind) =
                        read_face(b, elem, &plan, &positions, &mut indices, edge_ends.len(), &mut c).map_err(|e| at_record("face", i, e))?;
                    fanned_concave += usize::from(fanned);
                    fanned_blind += usize::from(blind);
                    if let Some((_, alpha, scale)) = &plan.color {
                        let k = |n: usize| decode_color(c[n], scale[n], n == 3);
                        let rgba = [k(0), k(1), k(2), if alpha.is_some() { k(3) } else { 1.0 }];
                        face_colors.push((before / 3, (indices.len() - before) / 3, rgba));
                    }
                }
            }
            "tristrips" => {
                let idx_prop = face_indices(elem)?;
                declared_faces = declared_faces.saturating_add(elem.count);
                for i in 0..elem.count {
                    read_strip(b, elem, idx_prop, &mut indices, edge_ends.len()).map_err(|e| at_record("tristrips", i, e))?;
                }
            }
            "edge" => match plan_edge(elem)? {
                // An edge element this reader cannot walk to segments is
                // stepped over by its layout, as any element it does not
                // model is, and named.
                None => {
                    if elem.count > 0 {
                        warnings.push(format!(
                            "the edge element has no vertex1 and vertex2 properties, so its {} edge(s) were not read",
                            elem.count
                        ));
                    }
                    skip_records(b, elem)?;
                }
                Some(plan) => {
                    // Charged to the index budget before one record is read,
                    // from the declared count: two indices an edge.
                    let charged = elem.count.saturating_mul(2).saturating_add(indices.len()).saturating_add(edge_ends.len());
                    if charged > super::scene::MAX_SCENE_INDICES {
                        return Err(MeshError::Unsupported(format!(
                            "{} edges, past the {}-index ceiling",
                            elem.count,
                            super::scene::MAX_SCENE_INDICES
                        )));
                    }
                    for i in 0..elem.count {
                        let mut c = [0.0f64; 4];
                        read_edge(b, elem, &plan, &mut edge_ends, &mut c).map_err(|e| at_record("edge", i, e))?;
                        if let Some((_, alpha, scale)) = &plan.color {
                            let k = |n: usize| decode_color(c[n], scale[n], n == 3);
                            edge_colors.push([k(0), k(1), k(2), if alpha.is_some() { k(3) } else { 1.0 }]);
                        }
                    }
                }
            },
            // Materials, range grids and the like: legal PLY elements we do
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
    if fanned_blind > 0 {
        warnings.push(format!(
            "{fanned_blind} face(s) of more than three corners came before the vertices and were fanned; a concave one may be drawn wrong"
        ));
    }
    if let Some(bad_idx) = edge_ends.iter().find(|i| **i as usize >= positions.len()) {
        return Err(bad(format!(
            "edge vertex index {bad_idx} is out of range for {} vertices",
            positions.len()
        )));
    }
    let mut normals = (!normals.is_empty()).then_some(normals);
    let mut colors = (!colors.is_empty()).then_some(colors);
    let mut uvs = (!uvs.is_empty()).then_some(uvs);
    // The edges are a primitive of their own, over the vertices they use: the
    // scene has a pool of vertices for each primitive and the file one for
    // all. Taken from the file's pool here, before the faces' colours give
    // their corners vertices of their own.
    let mut lines: Option<PlyPart> = None;
    if !edge_ends.is_empty() {
        lines = Some(if !edge_colors.is_empty() {
            // An edge's colour colours its two ends, over the vertices' own,
            // as a face's colours its corners: each end a vertex of its own.
            if edge_ends.len() > MAX_SCENE_VERTICES {
                return Err(MeshError::Unsupported(format!(
                    "edge colours give each of {} ends its own vertex, past the {MAX_SCENE_VERTICES}-vertex ceiling",
                    edge_ends.len()
                )));
            }
            PlyPart {
                positions: gather(&positions, &edge_ends),
                normals: normals.as_deref().map(|a| gather(a, &edge_ends)),
                uvs: uvs.as_deref().map(|a| gather(a, &edge_ends)),
                colors: Some((0..edge_ends.len()).map(|k| edge_colors[k / 2]).collect()),
                indices: (0..edge_ends.len() as u32).collect(),
            }
        } else {
            compact(&positions, normals.as_deref(), uvs.as_deref(), colors.as_deref(), &edge_ends)
        });
    }
    if !face_colors.is_empty() {
        // Each corner its own vertex, coloured by its face (over any vertex
        // colour, as three.js r170 PLYLoader.js:416-421 does).
        let corners = indices.len();
        if corners > MAX_SCENE_VERTICES {
            return Err(MeshError::Unsupported(format!(
                "face colours give each of {corners} corners its own vertex, past the {MAX_SCENE_VERTICES}-vertex ceiling"
            )));
        }
        let take = |a: &[[f32; 3]]| indices.iter().map(|&i| a[i as usize]).collect::<Vec<_>>();
        let mut run = 0usize;
        let mut corner_colors = Vec::with_capacity(corners);
        for k in 0..corners {
            let tri = k / 3;
            while run + 1 < face_colors.len() && face_colors[run + 1].0 <= tri {
                run += 1;
            }
            let (first, n, rgba) = face_colors[run];
            let own = colors.as_ref().map_or([1.0; 4], |c| c[indices[k] as usize]);
            corner_colors.push(if tri >= first && tri < first + n { rgba } else { own });
        }
        normals = normals.map(|n| take(&n));
        uvs = uvs.map(|u| indices.iter().map(|&i| u[i as usize]).collect());
        colors = Some(corner_colors);
        positions = take(&positions);
        indices = (0..corners as u32).collect();
    } else if lines.is_some() && !indices.is_empty() {
        // Faces and edges are two primitives, each over its own vertices;
        // a file with faces alone keeps every vertex it has.
        let part = compact(&positions, normals.as_deref(), uvs.as_deref(), colors.as_deref(), &indices);
        (positions, normals, uvs, colors, indices) = (part.positions, part.normals, part.uvs, part.colors, part.indices);
    }

    // What the file draws. Faces make triangles; a file with no faces and
    // edges makes lines; one with neither a face nor an edge is a point cloud,
    // a point for every vertex. Vertices no face uses, beside faces, are
    // vertices and no points.
    let mut topology = Topology::Triangles;
    if indices.is_empty() {
        if let Some(part) = lines.take() {
            topology = Topology::Lines;
            (positions, normals, uvs, colors, indices) = (part.positions, part.normals, part.uvs, part.colors, part.indices);
        } else if declared_faces == 0 {
            topology = Topology::Points;
            indices = (0..positions.len() as u32).collect();
        }
    }

    Ok(PlyData {
        positions,
        normals,
        uvs,
        colors,
        indices,
        topology,
        lines,
        name: h.name.clone(),
        fanned_concave,
        warnings,
    })
}

/// Read a PLY file (ASCII, or binary in either byte order) into the flat mesh.
///
/// The flat mesh has one colour for the whole model, so it takes the FIRST
/// vertex's: averaging would invent a colour that appears nowhere in the
/// source, whereas the first vertex's is at least one the file contains. The
/// per-vertex colours survive only through [`read_scene`].
pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    let d = read_data(bytes)?;
    // The flat mesh draws triangles only: a file of points or of edges has
    // none (and `mesh::read` calls it empty).
    let indices = if d.topology == Topology::Triangles { d.indices } else { Vec::new() };
    Ok(Mesh {
        positions: d.positions,
        indices,
        normals: d.normals,
        uvs: d.uvs,
        base_color: d.colors.as_ref().and_then(|c| c.first().copied()),
        name: d.name,
    })
}

/// Read a PLY file as a one-mesh scene with its per-vertex colours.
pub fn read_scene(bytes: &[u8]) -> Result<Scene, MeshError> {
    read_scene_warned(bytes).map(|(scene, _)| scene)
}

/// [`read_scene`], with what was read approximately.
pub fn read_scene_warned(bytes: &[u8]) -> Result<(Scene, Vec<String>), MeshError> {
    let d = read_data(bytes)?;
    let mut warnings = d.warnings;
    if d.fanned_concave > 0 {
        warnings.push(format!(
            "{} concave face(s) of more than {} corners were fan-triangulated",
            d.fanned_concave,
            super::polygon::MAX_EAR_CLIP
        ));
    }
    let mut scene = Scene::default();
    let mut primitives = vec![Primitive {
        positions: d.positions,
        normals: d.normals,
        uvs: d.uvs.map(|u| vec![u]).unwrap_or_default(),
        colors: d.colors,
        indices: d.indices,
        topology: d.topology,
        ..Default::default()
    }];
    if let Some(l) = d.lines {
        primitives.push(Primitive {
            positions: l.positions,
            normals: l.normals,
            uvs: l.uvs.map(|u| vec![u]).unwrap_or_default(),
            colors: l.colors,
            indices: l.indices,
            topology: Topology::Lines,
            ..Default::default()
        });
    }
    scene.meshes.push(SceneMesh { name: d.name.clone(), primitives, ..Default::default() });
    scene.add_root(Node { name: d.name, mesh: Some(0), ..Default::default() });
    Ok((scene, warnings))
}

fn read_data(bytes: &[u8]) -> Result<PlyData, MeshError> {
    let h = parse_header(bytes)?;
    let body = bytes.get(h.body_offset..).unwrap_or(&[]);
    // The one place an encoding becomes a byte order.
    let order = match h.encoding {
        Encoding::BinaryLe => ByteOrder::Little,
        Encoding::BinaryBe => ByteOrder::Big,
        Encoding::Ascii => {
            let text = std::str::from_utf8(body)
                .map_err(|_| bad("ascii PLY body is not valid UTF-8"))?;
            let mut b = Ascii { lines: text.lines(), cur: None };
            return read_body(&mut b, &h);
        }
    };
    let mut b = Binary { buf: body, pos: 0, order };
    read_body(&mut b, &h)
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

/// One linear colour component as the sRGB byte PLY readers expect (alpha:
/// linear, no curve). Rounded rather than truncated, so a colour that came in
/// as byte `n` goes back out as exactly `n` — `ply -> ply` is lossless.
fn quantise(c: f32, alpha: bool) -> u8 {
    if !c.is_finite() {
        return 0;
    }
    let c = c.clamp(0.0, 1.0);
    let encoded = if alpha { c } else { linear_to_srgb(c) };
    (encoded * 255.0).round() as u8
}

fn quantise_rgba(c: [f32; 4]) -> [u8; 4] {
    [quantise(c[0], false), quantise(c[1], false), quantise(c[2], false), quantise(c[3], true)]
}

/// Write the flat mesh as binary little-endian PLY. Its one base colour is
/// written on every vertex: PLY has no material block, so per-vertex colour is
/// the only place a flat colour can live.
pub fn write(mesh: &Mesh) -> Result<Vec<u8>, MeshError> {
    // write() is public, so it can be called with a mesh that never went through
    // the module-level entry point. Better to fail here than to emit a file
    // whose face list points past its own vertex list.
    mesh.validate()?;
    let colors = mesh.base_color.map(|c| vec![c; mesh.positions.len()]);
    write_data(&mesh.positions, mesh.normals.as_deref(), mesh.uvs.as_deref(), colors.as_deref(), &mesh.indices, mesh.name.as_deref())
}

/// The scene reduced to exactly what [`write_scene`] encodes, which is what
/// [`read_scene`] gives back for it: one mesh of at most one primitive of each
/// topology (`pose::degrade` merges them), over one pool of vertices.
///
/// - **Points are kept only alone.** A PLY with faces or edges is a mesh, and
///   one vertex pool cannot say which of its vertices are points: the reader
///   makes a point cloud of a file with no faces and no edges only (a vertex
///   no face uses, beside faces, is just a vertex). So points beside
///   triangles or lines are left out, and [`losses`] names them.
/// - **The columns are the file's, not a primitive's.** A normal, a texture
///   coordinate or a colour that only some of the primitives have is given
///   to the others as a merge gives it: a facet normal for triangles and +Z
///   for lines and points, `(0, 0)`, and white.
/// - **Each primitive keeps the vertices it draws.** Edges are read over the
///   vertices they use, and so are the faces beside them; a point cloud is a
///   point for each vertex, in order. A primitive that names its vertices
///   otherwise is renumbered as the reader numbers them.
pub fn prepare(scene: &Scene, opts: super::WriteOptions) -> Result<Scene, MeshError> {
    let mut caps = super::MeshFormat::Ply.capabilities(opts);
    if scene.meshes.iter().flat_map(|m| &m.primitives).any(|p| p.topology != Topology::Points) {
        caps.points = false;
    }
    let mut s = super::pose::degrade(scene, &caps)?;
    let has_lines = s.meshes.iter().flat_map(|m| &m.primitives).any(|p| p.topology == Topology::Lines);
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
        let uvs = p.uvs.first().map(Vec::as_slice);
        // The points are the vertices the indices name, so a primitive is left
        // as it is only when they name every vertex, each once and in order: an
        // identity prefix of a longer list would keep vertices no point draws,
        // which the file would hold and the reader return.
        let names_each_vertex = p.indices.len() == p.positions.len() && p.indices.iter().enumerate().all(|(k, &i)| i as usize == k);
        let part = match p.topology {
            // A point for each vertex: the points, in the order they draw.
            Topology::Points if !names_each_vertex => PlyPart {
                positions: gather(&p.positions, &p.indices),
                normals: p.normals.as_deref().map(|a| gather(a, &p.indices)),
                uvs: uvs.map(|a| gather(a, &p.indices)),
                colors: p.colors.as_deref().map(|a| gather(a, &p.indices)),
                indices: (0..p.indices.len() as u32).collect(),
            },
            Topology::Lines => compact(&p.positions, p.normals.as_deref(), uvs, p.colors.as_deref(), &p.indices),
            Topology::Triangles if has_lines => compact(&p.positions, p.normals.as_deref(), uvs, p.colors.as_deref(), &p.indices),
            _ => continue,
        };
        p.positions = part.positions;
        p.normals = part.normals;
        p.uvs = part.uvs.into_iter().collect();
        p.colors = part.colors;
        p.indices = part.indices;
    }
    // One pool has one set of columns.
    let mut prims: Vec<&mut Primitive> = s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()).collect();
    let (any_normals, any_uvs, any_colors) =
        (prims.iter().any(|p| p.normals.is_some()), prims.iter().any(|p| !p.uvs.is_empty()), prims.iter().any(|p| p.colors.is_some()));
    for p in prims.iter_mut() {
        let n = p.positions.len();
        if any_normals && p.normals.is_none() {
            p.normals = Some(match p.topology {
                Topology::Triangles => super::pose::derived_normals(&p.positions, &p.indices),
                _ => vec![[0.0, 0.0, 1.0]; n],
            });
        }
        if any_uvs && p.uvs.is_empty() {
            p.uvs = vec![vec![[0.0; 2]; n]];
        }
        if any_colors && p.colors.is_none() {
            p.colors = Some(vec![[1.0; 4]; n]);
        }
    }
    Ok(s)
}

/// What [`prepare`] leaves out beyond the capability table: the points of a
/// scene that also has triangles or lines, counted by primitive as the points
/// and lines a container lacks are.
pub fn losses(scene: &Scene) -> Vec<Loss> {
    let prims = scene.meshes.iter().flat_map(|m| &m.primitives);
    let points = prims.clone().filter(|p| p.topology == Topology::Points).count();
    if points > 0 && prims.clone().any(|p| p.topology != Topology::Points) {
        return vec![Loss::new(
            LossKind::Points,
            format!("{points} point primitive(s)"),
            "ply keeps points only in a file with no faces or edges: one vertex pool could not say which of its vertices are points".to_string(),
        )];
    }
    Vec::new()
}

/// Write a scene already reduced to PLY's capabilities (`MeshFormat::prepare`):
/// at most one primitive of each topology, optionally with normals, one UV set
/// and vertex colours. They share one pool of vertices, the triangles' first:
/// the triangles are the `face` element, the lines the `edge` element, and
/// points, which only a scene with neither may hold, are the vertices alone
/// (a file with no `face` element is a point cloud).
pub fn write_scene(scene: &Scene) -> Result<Vec<u8>, MeshError> {
    let prims: Vec<&Primitive> = scene.meshes.iter().flat_map(|m| &m.primitives).collect();
    let of = |t: Topology| -> Vec<&Primitive> { prims.iter().copied().filter(|p| p.topology == t).collect() };
    let (tris, lines, points) = (of(Topology::Triangles), of(Topology::Lines), of(Topology::Points));
    if prims.is_empty() || tris.len() > 1 || lines.len() > 1 || points.len() > 1 {
        return Err(MeshError::Malformed(
            "ply write: the scene must be merged into one primitive of each topology first".into(),
        ));
    }
    if !points.is_empty() && !(tris.is_empty() && lines.is_empty()) {
        return Err(MeshError::Malformed(
            "ply write: points cannot share a file with faces or edges; the scene must be prepared first".into(),
        ));
    }
    let parts: Vec<&Primitive> = tris.iter().chain(&lines).chain(&points).copied().collect();
    // One pool has one set of columns: all of the primitives have each, or none.
    let agree = |has: &dyn Fn(&Primitive) -> bool| parts.iter().all(|p| has(p)) || !parts.iter().any(|p| has(p));
    if !(agree(&|p| p.normals.is_some()) && agree(&|p| !p.uvs.is_empty()) && agree(&|p| p.colors.is_some())) {
        return Err(MeshError::Malformed(
            "ply write: the primitives disagree about their normals, texture coordinates or colours; the scene must be prepared first".into(),
        ));
    }
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let (mut normals, mut uvs, mut colors) = (Vec::new(), Vec::new(), Vec::new());
    for p in &parts {
        positions.extend_from_slice(&p.positions);
        normals.extend(p.normals.iter().flatten().copied());
        uvs.extend(p.uvs.first().into_iter().flatten().copied());
        colors.extend(p.colors.iter().flatten().copied());
    }
    let first = parts[0];
    let edge_base = tris.first().map_or(0, |p| p.positions.len() as u32);
    let edges: Vec<u32> = lines.first().map(|p| p.indices.iter().map(|i| i + edge_base).collect()).unwrap_or_default();
    let name = scene.meshes.iter().find_map(|m| m.name.clone()).or_else(|| scene.name.clone());
    write_file(
        &positions,
        first.normals.is_some().then_some(normals.as_slice()),
        (!first.uvs.is_empty()).then_some(uvs.as_slice()),
        first.colors.is_some().then_some(colors.as_slice()),
        tris.first().map(|p| p.indices.as_slice()),
        &edges,
        name.as_deref(),
    )
}

fn write_data(
    positions: &[[f32; 3]],
    normals: Option<&[[f32; 3]]>,
    uvs: Option<&[[f32; 2]]>,
    colors: Option<&[[f32; 4]]>,
    indices: &[u32],
    name: Option<&str>,
) -> Result<Vec<u8>, MeshError> {
    write_file(positions, normals, uvs, colors, Some(indices), &[], name)
}

/// One file: the pool of vertices, and over it the faces (`None` leaves the
/// `face` element out, as a point cloud's file has none) and the edges (none
/// leaves the `edge` element out).
fn write_file(
    positions: &[[f32; 3]],
    normals: Option<&[[f32; 3]]>,
    uvs: Option<&[[f32; 2]]>,
    colors: Option<&[[f32; 4]]>,
    faces: Option<&[u32]>,
    edges: &[u32],
    name: Option<&str>,
) -> Result<Vec<u8>, MeshError> {
    if normals.is_some_and(|n| n.len() != positions.len())
        || uvs.is_some_and(|u| u.len() != positions.len())
        || colors.is_some_and(|c| c.len() != positions.len())
    {
        return Err(MeshError::Malformed("ply write: attribute lengths disagree".into()));
    }
    let indices = faces.unwrap_or(&[]);
    if let Some(bad) = indices.iter().chain(edges).find(|i| **i as usize >= positions.len()) {
        return Err(MeshError::Malformed(format!("ply write: index {bad} out of range")));
    }
    let has_normals = normals.is_some();
    let triangle_count = indices.len() / 3;
    let edge_count = edges.len() / 2;

    let mut header = String::with_capacity(512);
    header.push_str("ply\nformat binary_little_endian 1.0\n");
    header.push_str("comment written by litegen mesh converter\n");
    if let Some(n) = name.and_then(header_name) {
        header.push_str("comment name ");
        header.push_str(&n);
        header.push('\n');
    }
    header.push_str(&format!("element vertex {}\n", positions.len()));
    header.push_str("property float x\nproperty float y\nproperty float z\n");
    if has_normals {
        header.push_str("property float nx\nproperty float ny\nproperty float nz\n");
    }
    if uvs.is_some() {
        // Blender's spelling, read by three.js; V up, as both have it.
        header.push_str("property float s\nproperty float t\n");
    }
    if colors.is_some() {
        header.push_str(
            "property uchar red\nproperty uchar green\nproperty uchar blue\nproperty uchar alpha\n",
        );
    }
    if faces.is_some() {
        header.push_str(&format!("element face {triangle_count}\n"));
        // uchar/uint is the pairing every reader handles: a count of 3 always fits a
        // uchar, and uint indices cover any mesh that fits under MAX_INPUT_BYTES.
        header.push_str("property list uchar uint vertex_indices\n");
    }
    if !edges.is_empty() {
        // The PLY paper's `edge` element: "two pointers to the vertices that
        // delineate the edge".
        header.push_str(&format!("element edge {edge_count}\nproperty int vertex1\nproperty int vertex2\n"));
    }
    header.push_str("end_header\n");

    let per_vertex = 12 + if has_normals { 12 } else { 0 } + if uvs.is_some() { 8 } else { 0 } + if colors.is_some() { 4 } else { 0 };
    let mut out = Vec::with_capacity(header.len() + positions.len() * per_vertex + triangle_count * 13 + edge_count * 8);
    out.extend_from_slice(header.as_bytes());

    for (i, p) in positions.iter().enumerate() {
        for v in p {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if let Some(ns) = normals {
            for v in &ns[i] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        if let Some(us) = uvs {
            out.extend_from_slice(&us[i][0].to_le_bytes());
            out.extend_from_slice(&(1.0 - us[i][1]).to_le_bytes());
        }
        if let Some(cs) = colors {
            out.extend_from_slice(&quantise_rgba(cs[i]));
        }
    }

    for tri in indices.chunks_exact(3) {
        out.push(3u8);
        for i in tri {
            out.extend_from_slice(&i.to_le_bytes());
        }
    }
    // Checked against the vertices above, so they fit an `int`.
    for end in edges.chunks_exact(2).flatten() {
        out.extend_from_slice(&(*end as i32).to_le_bytes());
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::{fixtures, MeshFormat, WriteOptions};
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
        fn u16(mut self, v: u16) -> Self {
            self.0.extend_from_slice(&v.to_le_bytes());
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
        // Values chosen on the sRGB u8 grid (alpha on the linear one): those
        // are exactly the ones a lossless colour round-trip must preserve bit
        // for bit.
        m.base_color = Some([1.0, srgb_to_linear(128.0 / 255.0), 0.0, 64.0 / 255.0]);
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
            base_color: Some([0.0, 1.0, srgb_to_linear(51.0 / 255.0), 1.0]),
            name: Some("widget".into()),
        };
        let back = read(&write(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn uvs_round_trip_as_s_and_t() {
        let mut m = tri_mesh();
        m.uvs = Some(vec![[0.0, 0.25], [1.0, 0.0], [0.0, 1.0]]);
        m.normals = Some(vec![[0.0, 0.0, 1.0]; 3]);
        m.name = Some("uv test".into());
        let bytes = write(&m).unwrap();
        let head = String::from_utf8_lossy(&bytes[..bytes.windows(10).position(|w| w == b"end_header").unwrap()]).into_owned();
        assert!(head.contains("property float s\nproperty float t\n"), "{head}");
        let back = read(&bytes).unwrap();
        assert_eq!(back.uvs, m.uvs);
        assert_eq!(back.normals, m.normals);
        assert_eq!(back.positions, m.positions);
        assert_eq!(back.name.as_deref(), Some("uv test"));
    }

    fn scene_of(src: &str) -> (Scene, Vec<String>) {
        read_scene_warned(src.as_bytes()).unwrap_or_else(|e| panic!("{e}"))
    }

    /// 2026-10-12 review, finding 24.10: texture coordinates spelled as three.js
    /// r170 PLYLoader.js:331-332 reads them (`s|u|texture_u|tx`,
    /// `t|v|texture_v|ty`, first present wins) are read, V flipped to the
    /// scene's downward V (Blender writes `s`/`t` with V up).
    #[test]
    fn texture_coordinates_are_read_in_every_common_spelling() {
        for (s, t) in [("s", "t"), ("u", "v"), ("texture_u", "texture_v"), ("tx", "ty")] {
            let src = format!("ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nproperty float {s}\nproperty float {t}\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0 0 0\n1 0 0 1 0\n0 1 0 0 1\n3 0 1 2\n");
            let (scene, _) = scene_of(&src);
            assert_eq!(scene.meshes[0].primitives[0].uvs, vec![vec![[0.0, 1.0], [1.0, 1.0], [0.0, 0.0]]], "{s}/{t}");
        }
    }

    /// 2026-10-12 review, finding 24.9: colour spelled `diffuse_red` (or `r`,
    /// `diffuse_r`; three.js r170 PLYLoader.js:333-335) is colour.
    #[test]
    fn colour_aliases_are_read() {
        let src = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nproperty uchar diffuse_red\nproperty uchar diffuse_green\nproperty uchar diffuse_blue\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0 255 0 0\n1 0 0 255 0 0\n0 1 0 255 0 0\n3 0 1 2\n";
        let (scene, _) = scene_of(src);
        let c = scene.meshes[0].primitives[0].colors.clone().expect("colours");
        assert!(c.iter().all(|c| *c == [1.0, 0.0, 0.0, 1.0]), "{c:?}");
    }

    /// 2026-10-12 review, finding 24.8: a face's colour colours its corners,
    /// each corner its own vertex, as three.js r170 PLYLoader.js:510-520 reads
    /// it; a face's own texcoord list is named, not silently dropped.
    #[test]
    fn face_colours_colour_their_corners() {
        let src = "ply\nformat ascii 1.0\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\nelement face 2\nproperty list uchar int vertex_indices\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nend_header\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n3 0 1 2 255 0 0\n3 1 3 2 0 0 255\n";
        let (scene, _) = scene_of(src);
        let p = &scene.meshes[0].primitives[0];
        assert_eq!((p.positions.len(), p.triangle_count()), (6, 2));
        let c = p.colors.clone().expect("colours");
        assert!(c[..3].iter().all(|c| *c == [1.0, 0.0, 0.0, 1.0]) && c[3..].iter().all(|c| *c == [0.0, 0.0, 1.0, 1.0]), "{c:?}");
        let src = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nproperty list uchar float texcoord\nend_header\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2 6 0 0 1 0 0 1\n";
        let (_, warnings) = scene_of(src);
        assert!(warnings.iter().any(|w| w.contains("texcoord")), "{warnings:?}");
    }

    /// 2026-10-12 review, note N3: a face element written before the vertex
    /// element (legal, unusual) cannot be ear-clipped — its corners are not
    /// known yet — so a polygon of more than three corners is fanned, and now
    /// says so.
    #[test]
    fn faces_before_their_vertices_are_fanned_with_a_warning() {
        let src = "ply\nformat ascii 1.0\nelement face 1\nproperty list uchar int vertex_indices\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\nend_header\n4 0 1 2 3\n0 0 0\n1 0 0\n1 1 0\n0 1 0\n";
        let (scene, warnings) = scene_of(src);
        assert_eq!(scene.meshes[0].primitives[0].triangle_count(), 2);
        assert!(warnings.iter().any(|w| w.contains("before")), "{warnings:?}");
    }

    /// 2026-10-12 review, finding 24.11: triangle strips (the PLY paper's
    /// `tristrips`, -1 restarting a strip) are read as triangles, winding
    /// alternating. A file of only edges was once refused, naming them; its
    /// edges are lines now (mesh-gaps item 10).
    #[test]
    fn triangle_strips_are_read_and_edges_alone_are_lines() {
        let src = "ply\nformat ascii 1.0\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\nelement tristrips 1\nproperty list int int vertex_indices\nend_header\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n4 0 1 2 3\n";
        let (scene, _) = scene_of(src);
        assert_eq!(scene.meshes[0].primitives[0].indices, [0, 1, 2, 2, 1, 3]);
        let src = "ply\nformat ascii 1.0\nelement vertex 2\nproperty float x\nproperty float y\nproperty float z\nelement edge 1\nproperty int vertex1\nproperty int vertex2\nend_header\n0 0 0\n1 0 0\n0 1\n";
        let (scene, _) = scene_of(src);
        let p = &scene.meshes[0].primitives[0];
        assert_eq!((p.topology, &p.indices, p.positions.len()), (Topology::Lines, &vec![0, 1], 2));
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
        // The bytes are sRGB; the scene's colour is linear.
        assert_eq!(m.base_color, Some([1.0, srgb_to_linear(128.0 / 255.0), 0.0, 1.0]));
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
        // First vertex wins in the flat view; the scene keeps all three.
        // RGB is sRGB-decoded, alpha is not.
        assert_eq!(
            m.base_color,
            Some([
                srgb_to_linear(10.0 / 255.0),
                srgb_to_linear(20.0 / 255.0),
                srgb_to_linear(30.0 / 255.0),
                40.0 / 255.0
            ])
        );
    }

    #[test]
    fn ascii_float_colour_is_not_divided_by_255() {
        let src = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
                   property float z\nproperty float red\nproperty float green\nproperty float blue\n\
                   element face 1\nproperty list uchar int vertex_indices\nend_header\n\
                   0 0 0 1 0.5 0\n1 0 0 0 0 0\n0 1 0 0 0 0\n3 0 1 2\n";
        let m = read(src.as_bytes()).unwrap();
        // Float colour is 0..1 already — not divided by 255 — and sRGB-encoded
        // like every other PLY colour.
        assert_eq!(m.base_color, Some([1.0, srgb_to_linear(0.5), 0.0, 1.0]));
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
        // Refused from the header, before a record is read: the count is past
        // the vertex ceiling whatever the body holds.
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("4000000000 vertices")), "{e}");
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
    //
    // Each test reads a file one `bin_*` function builds, and
    // `big_endian_twins_read_identically` reads every one of them again in
    // both byte orders, so no body is written out twice.

    const BIN_HEAD: &str = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                            property float x\nproperty float y\nproperty float z\n\
                            element face 1\nproperty list uchar int vertex_indices\nend_header\n";

    /// The three vertices `BIN_HEAD` declares.
    fn bin_vertices() -> Buf {
        Buf::default()
            .f32(0.0).f32(0.0).f32(0.0)
            .f32(1.0).f32(0.0).f32(0.0)
            .f32(0.0).f32(1.0).f32(0.0)
    }

    fn bin_body() -> Buf {
        bin_vertices().u8(3).i32(0).i32(1).i32(2)
    }

    fn bin_minimal() -> Vec<u8> {
        with_header(BIN_HEAD, bin_body())
    }

    #[test]
    fn binary_minimal() {
        let m = read(&bin_minimal()).unwrap();
        assert_eq!(m.positions, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        assert_eq!(m.indices, vec![0, 1, 2]);
    }

    fn bin_any_order_with_extras() -> Vec<u8> {
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 2\n\
                    property uchar red\nproperty float y\nproperty double z\n\
                    property int junk\nproperty float x\nproperty uchar green\n\
                    property uchar blue\nelement face 1\n\
                    property list uchar uint vertex_indices\nend_header\n";
        let body = Buf::default()
            .u8(255).f32(2.0).f64(3.0).i32(-7).f32(1.0).u8(128).u8(0)
            .u8(17).f32(5.0).f64(6.0).i32(-7).f32(4.0).u8(0).u8(0)
            .u8(3).u32(0).u32(1).u32(0);
        with_header(head, body)
    }

    #[test]
    fn binary_properties_in_any_order_with_extras() {
        // 23 bytes per vertex: uchar, float, double, int, float, uchar, uchar.
        // Every width has to be right or the whole record stream shifts.
        let m = read(&bin_any_order_with_extras()).unwrap();
        assert_eq!(m.positions, vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
        assert_eq!(m.base_color, Some([1.0, srgb_to_linear(128.0 / 255.0), 0.0, 1.0]));
    }

    /// Every vertex coloured `ushort` 0xFF00, 0x8000, 0x00FF, alpha 0x4000.
    fn bin_ushort_colour() -> Vec<u8> {
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                    property float x\nproperty float y\nproperty float z\n\
                    property ushort red\nproperty ushort green\nproperty ushort blue\n\
                    property ushort alpha\nelement face 1\n\
                    property list uchar int vertex_indices\nend_header\n";
        let body = Buf::default()
            .f32(0.0).f32(0.0).f32(0.0).u16(0xFF00).u16(0x8000).u16(0x00FF).u16(0x4000)
            .f32(1.0).f32(0.0).f32(0.0).u16(0xFF00).u16(0x8000).u16(0x00FF).u16(0x4000)
            .f32(0.0).f32(1.0).f32(0.0).u16(0xFF00).u16(0x8000).u16(0x00FF).u16(0x4000)
            .u8(3).i32(0).i32(1).i32(2);
        with_header(head, body)
    }

    #[test]
    fn binary_ushort_colour_is_divided_by_65535() {
        // Full scale is 65535: divided by 255, every component would read as
        // 1. Each one's two bytes differ, so the big-endian twin, read the
        // wrong way round, would be another colour. RGB is sRGB-decoded,
        // alpha is not.
        let m = read(&bin_ushort_colour()).unwrap();
        assert_eq!(
            m.base_color,
            Some([
                srgb_to_linear(65280.0 / 65535.0),
                srgb_to_linear(32768.0 / 65535.0),
                srgb_to_linear(255.0 / 65535.0),
                16384.0 / 65535.0
            ])
        );
    }

    fn bin_unknown_element_with_a_list() -> Vec<u8> {
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                    property float x\nproperty float y\nproperty float z\n\
                    element edge 2\nproperty list uchar int ends\nproperty ushort flags\n\
                    element face 1\nproperty list uchar int vertex_indices\nend_header\n";
        let body = bin_vertices()
            // edge 0: two ends + flags; edge 1: three ends + flags
            .u8(2).i32(0).i32(1).u8(9).u8(9)
            .u8(3).i32(0).i32(1).i32(2).u8(9).u8(9)
            .u8(3).i32(0).i32(1).i32(2);
        with_header(head, body)
    }

    #[test]
    fn binary_unknown_element_with_a_list_is_walked() {
        let m = read(&bin_unknown_element_with_a_list()).unwrap();
        assert_eq!(m.positions.len(), 3);
        assert_eq!(m.indices, vec![0, 1, 2]);
    }

    fn bin_unknown_fixed_size_element() -> Vec<u8> {
        let head = "ply\nformat binary_little_endian 1.0\nelement junk 2\n\
                    property double a\nproperty uchar b\nelement vertex 3\n\
                    property float x\nproperty float y\nproperty float z\n\
                    element face 1\nproperty list uchar int vertex_indices\nend_header\n";
        let mut body = Buf::default().f64(1.0).u8(1).f64(2.0).u8(2);
        body.0.extend_from_slice(&bin_body().0);
        with_header(head, body)
    }

    #[test]
    fn binary_unknown_fixed_size_element_is_skipped_wholesale() {
        let m = read(&bin_unknown_fixed_size_element()).unwrap();
        assert_eq!(m.positions.len(), 3);
    }

    fn bin_double_positions() -> Vec<u8> {
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                    property double x\nproperty double y\nproperty double z\n\
                    element face 1\nproperty list uchar int vertex_indices\nend_header\n";
        let body = Buf::default()
            .f64(0.5).f64(0.25).f64(0.125)
            .f64(1.0).f64(0.0).f64(0.0)
            .f64(0.0).f64(1.0).f64(0.0)
            .u8(3).i32(0).i32(1).i32(2);
        with_header(head, body)
    }

    #[test]
    fn binary_double_positions() {
        let m = read(&bin_double_positions()).unwrap();
        assert_eq!(m.positions[0], [0.5, 0.25, 0.125]);
    }

    /// `bin_minimal` 20 bytes short: the last vertex is cut.
    fn bin_cut_mid_vertex() -> Vec<u8> {
        let full = bin_minimal();
        full[..full.len() - 20].to_vec()
    }

    #[test]
    fn binary_truncated_mid_vertex() {
        // The fixed-size check catches this before a single record is read,
        // which is the whole point of having it.
        let e = read(&bin_cut_mid_vertex()).unwrap_err();
        let d = malformed(&e);
        assert!(d.contains("element 'vertex' declares 3 records"), "{d}");
        assert!(d.contains("bytes remain"), "{d}");
    }

    /// `bin_minimal` 13 bytes short: every vertex, and none of the face.
    fn bin_cut_before_the_face() -> Vec<u8> {
        let full = bin_minimal();
        full[..full.len() - 13].to_vec()
    }

    #[test]
    fn binary_truncated_where_no_up_front_check_can_see_it() {
        // The vertex element's bytes are all present — the up-front check passes
        // because it counts every remaining byte — and the body then runs out at
        // the face element's list count. Only the cursor catches this one.
        let e = read(&bin_cut_before_the_face()).unwrap_err();
        assert!(malformed(&e).contains("truncated body"), "{e}");
    }

    fn bin_cut_in_a_face_list() -> Vec<u8> {
        with_header(BIN_HEAD, bin_vertices().u8(3).i32(0).i32(1)) // one index short
    }

    #[test]
    fn binary_truncated_at_a_face_list() {
        let e = read(&bin_cut_in_a_face_list()).unwrap_err();
        assert!(malformed(&e).contains("face record 0"), "{e}");
    }

    fn bin_hostile_vertex_count() -> Vec<u8> {
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 4000000000\n\
                    property float x\nproperty float y\nproperty float z\n\
                    element face 0\nproperty list uchar int vertex_indices\nend_header\n";
        with_header(head, Buf::default().f32(0.0).f32(0.0).f32(0.0))
    }

    #[test]
    fn binary_hostile_vertex_count_is_rejected_up_front() {
        let e = read(&bin_hostile_vertex_count()).unwrap_err();
        let d = malformed(&e);
        assert!(d.contains("declares 4000000000 records"), "{d}");
        assert!(d.contains("bytes remain"), "{d}");
    }

    fn bin_hostile_list_length() -> Vec<u8> {
        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 3\n\
                    property float x\nproperty float y\nproperty float z\n\
                    element face 1\nproperty list uint uint vertex_indices\nend_header\n";
        with_header(head, bin_vertices().u32(u32::MAX).u32(0).u32(1).u32(2))
    }

    #[test]
    fn binary_hostile_list_length_is_rejected_before_allocating() {
        let e = read(&bin_hostile_list_length()).unwrap_err();
        assert!(malformed(&e).contains("declares 4294967295 items"), "{e}");
    }

    fn bin_trailing_padding() -> Vec<u8> {
        let mut full = bin_minimal();
        full.extend_from_slice(&[0u8; 16]);
        full
    }

    #[test]
    fn binary_trailing_padding_is_tolerated() {
        assert_eq!(read(&bin_trailing_padding()).unwrap().positions.len(), 3);
    }

    fn bin_face_index_out_of_range() -> Vec<u8> {
        with_header(BIN_HEAD, bin_vertices().u8(3).i32(0).i32(1).i32(7))
    }

    #[test]
    fn binary_face_index_out_of_range() {
        let e = read(&bin_face_index_out_of_range()).unwrap_err();
        assert!(malformed(&e).contains("index 7 out of range for 3 vertices"), "{e}");
    }

    fn bin_negative_face_index() -> Vec<u8> {
        with_header(BIN_HEAD, bin_vertices().u8(3).i32(0).i32(-1).i32(2))
    }

    #[test]
    fn binary_negative_face_index() {
        let e = read(&bin_negative_face_index()).unwrap_err();
        assert!(malformed(&e).contains("out of range"), "{e}");
    }

    fn bin_face_with_two_vertices() -> Vec<u8> {
        with_header(BIN_HEAD, bin_vertices().u8(2).i32(0).i32(1))
    }

    #[test]
    fn binary_face_with_two_vertices() {
        let e = read(&bin_face_with_two_vertices()).unwrap_err();
        assert!(malformed(&e).contains("face has 2 vertices"), "{e}");
    }

    fn bin_face_before_vertex() -> Vec<u8> {
        let head = "ply\nformat binary_little_endian 1.0\nelement face 1\n\
                    property list uchar int vertex_indices\nelement vertex 3\n\
                    property float x\nproperty float y\nproperty float z\nend_header\n";
        let body = Buf::default()
            .u8(3).i32(0).i32(1).i32(2)
            .f32(0.0).f32(0.0).f32(0.0)
            .f32(1.0).f32(0.0).f32(0.0)
            .f32(0.0).f32(1.0).f32(0.0);
        with_header(head, body)
    }

    #[test]
    fn face_element_may_precede_vertex_element() {
        let m = read(&bin_face_before_vertex()).unwrap();
        assert_eq!(m.indices, vec![0, 1, 2]);
        assert_eq!(m.positions.len(), 3);
    }

    fn bin_singular_vertex_index() -> Vec<u8> {
        with_header(&BIN_HEAD.replace("vertex_indices", "vertex_index"), bin_body())
    }

    #[test]
    fn vertex_index_singular_spelling_is_accepted() {
        let m = read(&bin_singular_vertex_index()).unwrap();
        assert_eq!(m.indices, vec![0, 1, 2]);
    }

    // -- big-endian ---------------------------------------------------------

    /// One value of each scalar type: the type's two spellings, the value's
    /// big-endian bytes written out by hand (never through a swapper), and the
    /// value, which is exact in f32.
    const EVERY_SCALAR: [(&str, &str, &[u8], f64); 8] = [
        ("char", "int8", &[0xFE], -2.0),
        ("uchar", "uint8", &[0xFA], 250.0),
        ("short", "int16", &[0xFE, 0xD4], -300.0),
        ("ushort", "uint16", &[0x01, 0x02], 258.0), // 0x0102
        ("int", "int32", &[0xFF, 0xFE, 0xEE, 0x90], -70_000.0),
        ("uint", "uint32", &[0x01, 0x02, 0x03, 0x04], 16_909_060.0), // 0x01020304
        ("float", "float32", &[0x3F, 0xC0, 0x00, 0x00], 1.5),
        ("double", "float64", &[0xC0, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], -2.25),
    ];

    /// One file in each byte order, big-endian first: `header` sits between
    /// the `format` line and `end_header`, and the body is given scalar by
    /// scalar as big-endian bytes. The little-endian twin reverses each
    /// scalar's bytes.
    fn twins(header: &str, scalars: &[&[u8]]) -> (Vec<u8>, Vec<u8>) {
        let file = |format: &str, body: Vec<u8>| {
            let mut v = format!("ply\nformat {format} 1.0\n{header}end_header\n").into_bytes();
            v.extend(body);
            v
        };
        let le = scalars.iter().flat_map(|s| s.iter().rev().copied()).collect();
        (file("binary_big_endian", scalars.concat()), file("binary_little_endian", le))
    }

    /// Every scalar type, under both spellings, from a big-endian body. Each
    /// value lands in a slot that keeps it exactly: x, y and z; the normal,
    /// which is checked to be finite and never renormalised; and the UV, read
    /// as `[s, 1 - t]`. The little-endian twin reads the same.
    #[test]
    fn every_scalar_type_reads_big_endian() {
        let body: Vec<&[u8]> = EVERY_SCALAR.iter().map(|s| s.2).collect();
        for types in [EVERY_SCALAR.map(|s| s.0), EVERY_SCALAR.map(|s| s.1)] {
            let mut header = String::from("element vertex 1\n");
            for (ty, slot) in types.iter().zip(["x", "y", "z", "nx", "ny", "nz", "s", "t"]) {
                header.push_str(&format!("property {ty} {slot}\n"));
            }
            let (be, le) = twins(&header, &body);
            let read = read_scene_warned(&be).unwrap_or_else(|e| panic!("{types:?}: {e:?}"));
            let p = &read.0.meshes[0].primitives[0];
            assert_eq!(p.positions, vec![[-2.0, 250.0, -300.0]], "{types:?}");
            assert_eq!(p.normals, Some(vec![[258.0, -70_000.0, 16_909_060.0]]), "{types:?}");
            // t is -2.25, so 1 - t is 3.25.
            assert_eq!(p.uvs, vec![vec![[1.5, 3.25]]], "{types:?}");
            assert_eq!(read_scene_warned(&le).unwrap(), read, "{types:?}: the little-endian twin");
        }
    }

    /// Face lists under the uchar, ushort and uint count types, with int and
    /// uint indices, from a big-endian body. The face is a concave quad, so it
    /// is ear-clipped, not fanned, into the same triangles as its
    /// little-endian twin's.
    #[test]
    fn big_endian_lists_read_with_every_count_type() {
        const F0: &[u8] = &[0x00, 0x00, 0x00, 0x00]; // float 0
        const F1: &[u8] = &[0x3F, 0x80, 0x00, 0x00]; // float 1
        const F2: &[u8] = &[0x40, 0x00, 0x00, 0x00]; // float 2
        // A dart, (0,0) (2,1) (0,2) (1,1): corner 3 is its notch.
        let vertices: [&[u8]; 12] = [F0, F0, F0, F2, F1, F0, F0, F2, F0, F1, F1, F0];
        // Corners 0 1 2 3, the same bytes as int or uint.
        let corners: [&[u8]; 4] = [&[0, 0, 0, 0], &[0, 0, 0, 1], &[0, 0, 0, 2], &[0, 0, 0, 3]];
        let counts: [(&str, &[u8]); 3] = [("uchar", &[4]), ("ushort", &[0, 4]), ("uint", &[0, 0, 0, 4])];
        for (count_type, count) in counts {
            for index_type in ["int", "uint"] {
                let header = format!(
                    "element vertex 4\nproperty float x\nproperty float y\nproperty float z\n\
                     element face 1\nproperty list {count_type} {index_type} vertex_indices\n"
                );
                let mut body = vertices.to_vec();
                body.push(count);
                body.extend(corners);
                let (be, le) = twins(&header, &body);
                let read = read_scene_warned(&be).unwrap_or_else(|e| panic!("{count_type} {index_type}: {e:?}"));
                let p = &read.0.meshes[0].primitives[0];
                assert_eq!(p.positions, vec![[0.0, 0.0, 0.0], [2.0, 1.0, 0.0], [0.0, 2.0, 0.0], [1.0, 1.0, 0.0]], "{count_type} {index_type}");
                // Clipped at the notch; a fan from corner 0 would be 0 1 2, 0 2 3.
                assert_eq!(p.indices, vec![3, 0, 1, 1, 2, 3], "{count_type} {index_type}");
                assert_eq!(read_scene_warned(&le).unwrap(), read, "{count_type} {index_type}: the little-endian twin");
            }
        }
    }

    /// `Scalar::decode` itself: each type from its big-endian bytes and from
    /// their reversal. The reader always hands it exactly the type's width,
    /// so two things only a direct call shows: it reads just that width from
    /// a longer slice, and a short slice is `None`, never a panic, in either
    /// byte order.
    #[test]
    fn decode_reads_each_type_in_either_byte_order() {
        for (name, _, be, want) in EVERY_SCALAR {
            let t = Scalar::parse(name).unwrap();
            let le: Vec<u8> = be.iter().rev().copied().collect();
            for (bytes, order) in [(be, ByteOrder::Big), (&le[..], ByteOrder::Little)] {
                let mut longer = bytes.to_vec();
                longer.push(0xA5);
                assert_eq!(t.decode(bytes, order), Some(want), "{name} {order:?}");
                assert_eq!(t.decode(&longer, order), Some(want), "{name} {order:?}, a byte longer");
                assert_eq!(t.decode(&bytes[..bytes.len() - 1], order), None, "{name} {order:?}, a byte short");
            }
        }
    }

    /// The big-endian twin of a binary little-endian PLY file: the header
    /// with its format line rewritten to `binary_big_endian` (the version,
    /// or the lack of one, kept), and every scalar of the body, list counts
    /// included, with its bytes reversed. It reads the header itself, apart
    /// from `parse_header`, so the reader and this test cannot share a
    /// mistake there.
    ///
    /// The body is walked by its declared layout, but only as far as its
    /// bytes go: a hostile count ends the walk where the data ends, not 4e9
    /// records later. What the walk does not reach (a scalar cut short,
    /// padding, whatever follows a negative list count) is copied as it is,
    /// so a truncated file has a twin truncated at the same byte.
    fn to_big_endian(le: &[u8]) -> Vec<u8> {
        #[derive(Clone, Copy)]
        enum Prop {
            Scalar(usize),
            /// The widths of the count and of each item, and whether the
            /// count is signed.
            List { count: usize, signed: bool, item: usize },
        }
        // Widths by type name, written out apart from `Scalar::size`.
        let width = |ty: &str| match ty {
            "char" | "uchar" | "int8" | "uint8" => 1,
            "short" | "ushort" | "int16" | "uint16" => 2,
            "int" | "uint" | "int32" | "uint32" | "float" | "float32" => 4,
            "double" | "float64" => 8,
            other => panic!("to_big_endian: no PLY type {other:?}"),
        };
        let mut out = Vec::with_capacity(le.len());
        let mut elements: Vec<(u64, Vec<Prop>)> = Vec::new();
        let mut at = 0;
        let mut rewrote_format = false;
        // The header, a line (LF or CRLF) at a time.
        loop {
            let end = at + 1 + le[at..].iter().position(|&b| b == b'\n').expect("to_big_endian: no end_header line");
            let line = std::str::from_utf8(&le[at..end]).expect("to_big_endian: a header line is not text");
            at = end;
            let words: Vec<&str> = line.split_whitespace().collect();
            // Only the leading words count, as in `parse_header`.
            match words[..] {
                ["format", "binary_little_endian", ..] => {
                    rewrote_format = true;
                    out.extend_from_slice(line.replacen("binary_little_endian", "binary_big_endian", 1).as_bytes());
                    continue;
                }
                ["format", ..] => panic!("to_big_endian: {line:?} is not binary little-endian"),
                ["element", _, records, ..] => elements.push((records.parse().unwrap_or(u64::MAX), Vec::new())),
                ["property", "list", count, item, ..] => {
                    let signed = matches!(count, "char" | "int8" | "short" | "int16" | "int" | "int32");
                    let list = Prop::List { count: width(count), signed, item: width(item) };
                    elements.last_mut().expect("to_big_endian: a property before any element").1.push(list);
                }
                ["property", ty, ..] => {
                    let scalar = Prop::Scalar(width(ty));
                    elements.last_mut().expect("to_big_endian: a property before any element").1.push(scalar);
                }
                _ => {}
            }
            out.extend_from_slice(line.as_bytes());
            if words == ["end_header"] {
                break;
            }
        }
        assert!(rewrote_format, "to_big_endian: no format line");
        // The body. `swap` reverses one scalar of `n` bytes, if the body still
        // holds it.
        let swap = |out: &mut Vec<u8>, at: &mut usize, n: usize| match le.get(*at..*at + n) {
            Some(s) => {
                out.extend(s.iter().rev());
                *at += n;
                true
            }
            None => false,
        };
        'walk: for (records, props) in &elements {
            // An element without properties holds no bytes, whatever its count.
            if props.is_empty() {
                continue;
            }
            for _ in 0..*records {
                for &prop in props {
                    match prop {
                        Prop::Scalar(n) => {
                            if !swap(&mut out, &mut at, n) {
                                break 'walk;
                            }
                        }
                        Prop::List { count, signed, item } => {
                            let Some(raw) = le.get(at..at + count) else { break 'walk };
                            // The count's value, read little-endian.
                            let mut len = raw.iter().rev().fold(0i64, |v, &b| (v << 8) | i64::from(b));
                            if signed && (raw[count - 1] & 0x80) != 0 {
                                len -= 1 << (8 * count);
                            }
                            swap(&mut out, &mut at, count);
                            // A negative count ends the reading there.
                            let Ok(len) = usize::try_from(len) else { break 'walk };
                            let present = len.min((le.len() - at) / item);
                            let run = &le[at..at + present * item];
                            if item == 1 {
                                out.extend_from_slice(run);
                            } else {
                                for s in run.chunks_exact(item) {
                                    out.extend(s.iter().rev());
                                }
                            }
                            at += present * item;
                            if present < len {
                                break 'walk;
                            }
                        }
                    }
                }
            }
        }
        out.extend_from_slice(&le[at..]);
        out
    }

    /// `write_scene` of a fixture, first reduced to the one primitive PLY
    /// holds, as `mesh::write_scene` reduces it. A fixture that can't be
    /// written so is named, as `name`, in the panic.
    fn fixture_ply(name: &str, fixture: &Scene) -> Vec<u8> {
        MeshFormat::Ply
            .prepare(fixture, WriteOptions::default())
            .and_then(|prepared| write_scene(&prepared))
            .unwrap_or_else(|e| panic!("the {name} fixture can't be written as PLY: {e:?}"))
    }

    /// `file` with the first `from` in it replaced by `to`: an edit to the
    /// header, which comes before any body byte.
    fn replace_first(file: &[u8], from: &str, to: &str) -> Vec<u8> {
        let at = file.windows(from.len()).position(|w| w == from.as_bytes()).unwrap_or_else(|| panic!("no {from:?}"));
        [&file[..at], to.as_bytes(), &file[at + from.len()..]].concat()
    }

    /// `file` with every header line ended by CRLF, and its body unchanged.
    fn with_crlf_header(file: &[u8]) -> Vec<u8> {
        let end = file.windows(11).position(|w| w == b"end_header\n").expect("end_header") + 11;
        let head = std::str::from_utf8(&file[..end]).unwrap().replace('\n', "\r\n");
        [head.as_bytes(), &file[end..]].concat()
    }

    /// Binary little-endian versions, with the same content, of the cases
    /// four ASCII tests build: UVs under each spelling
    /// (`texture_coordinates_are_read_in_every_common_spelling`), face colours
    /// and a face's texcoord list (`face_colours_colour_their_corners`), a
    /// triangle strip and edges alone
    /// (`triangle_strips_are_read_and_edges_alone_are_lines`), and float
    /// colour (`ascii_float_colour_is_not_divided_by_255`). Each is checked
    /// for what its ASCII test pins, so a twin of it is a real one.
    fn bin_versions_of_the_ascii_cases() -> Vec<(String, Vec<u8>)> {
        let mut files = Vec::new();
        for (s, t) in [("s", "t"), ("u", "v"), ("texture_u", "texture_v"), ("tx", "ty")] {
            let head = format!(
                "ply\nformat binary_little_endian 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
                 property float z\nproperty float {s}\nproperty float {t}\nelement face 1\n\
                 property list uchar int vertex_indices\nend_header\n"
            );
            let body = Buf::default()
                .f32(0.0).f32(0.0).f32(0.0).f32(0.0).f32(0.0)
                .f32(1.0).f32(0.0).f32(0.0).f32(1.0).f32(0.0)
                .f32(0.0).f32(1.0).f32(0.0).f32(0.0).f32(1.0)
                .u8(3).i32(0).i32(1).i32(2);
            let file = with_header(&head, body);
            let (scene, _) = read_scene_warned(&file).unwrap();
            assert_eq!(scene.meshes[0].primitives[0].uvs, vec![vec![[0.0, 1.0], [1.0, 1.0], [0.0, 0.0]]], "{s}/{t}");
            files.push((format!("UVs as {s}/{t}"), file));
        }

        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 4\nproperty float x\nproperty float y\n\
                    property float z\nelement face 2\nproperty list uchar int vertex_indices\n\
                    property uchar red\nproperty uchar green\nproperty uchar blue\nend_header\n";
        let body = bin_vertices().f32(1.0).f32(1.0).f32(0.0)
            .u8(3).i32(0).i32(1).i32(2).u8(255).u8(0).u8(0)
            .u8(3).i32(1).i32(3).i32(2).u8(0).u8(0).u8(255);
        let file = with_header(head, body);
        let (scene, _) = read_scene_warned(&file).unwrap();
        let p = &scene.meshes[0].primitives[0];
        assert_eq!((p.positions.len(), p.triangle_count()), (6, 2), "face colours");
        let (red, blue) = ([1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(p.colors, Some(vec![red, red, red, blue, blue, blue]), "face colours");
        files.push(("face colours".to_string(), file));

        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
                    property float z\nelement face 1\nproperty list uchar int vertex_indices\n\
                    property list uchar float texcoord\nend_header\n";
        let body = bin_vertices().u8(3).i32(0).i32(1).i32(2)
            .u8(6).f32(0.0).f32(0.0).f32(1.0).f32(0.0).f32(0.0).f32(1.0);
        let file = with_header(head, body);
        let (_, warnings) = read_scene_warned(&file).unwrap();
        assert!(warnings.iter().any(|w| w.contains("texcoord")), "{warnings:?}");
        files.push(("a face's texcoord list".to_string(), file));

        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 4\nproperty float x\nproperty float y\n\
                    property float z\nelement tristrips 1\nproperty list int int vertex_indices\nend_header\n";
        let file = with_header(head, bin_vertices().f32(1.0).f32(1.0).f32(0.0).i32(4).i32(0).i32(1).i32(2).i32(3));
        let (scene, _) = read_scene_warned(&file).unwrap();
        assert_eq!(scene.meshes[0].primitives[0].indices, [0, 1, 2, 2, 1, 3], "a triangle strip");
        files.push(("a triangle strip".to_string(), file));

        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 2\nproperty float x\nproperty float y\n\
                    property float z\nelement edge 1\nproperty int vertex1\nproperty int vertex2\nend_header\n";
        let file = with_header(head, Buf::default().f32(0.0).f32(0.0).f32(0.0).f32(1.0).f32(0.0).f32(0.0).i32(0).i32(1));
        let (scene, _) = read_scene_warned(&file).unwrap();
        let p = &scene.meshes[0].primitives[0];
        assert_eq!((p.topology, &p.indices), (Topology::Lines, &vec![0, 1]), "edges alone");
        files.push(("edges alone".to_string(), file));

        let head = "ply\nformat binary_little_endian 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
                    property float z\nproperty float red\nproperty float green\nproperty float blue\n\
                    element face 1\nproperty list uchar int vertex_indices\nend_header\n";
        let body = Buf::default()
            .f32(0.0).f32(0.0).f32(0.0).f32(1.0).f32(0.5).f32(0.0)
            .f32(1.0).f32(0.0).f32(0.0).f32(0.0).f32(0.0).f32(0.0)
            .f32(0.0).f32(1.0).f32(0.0).f32(0.0).f32(0.0).f32(0.0)
            .u8(3).i32(0).i32(1).i32(2);
        let file = with_header(head, body);
        assert_eq!(read(&file).unwrap().base_color, Some([1.0, srgb_to_linear(0.5), 0.0, 1.0]), "float colour");
        files.push(("float colour".to_string(), file));
        files
    }

    /// Each `bin_*` builder's file, the binary versions of four ASCII tests'
    /// cases (`bin_versions_of_the_ascii_cases`), and `write_scene` of every
    /// fixture read the same from their big-endian twins, scenes, warnings
    /// and errors alike. Every fixture must read, so that its twin cannot
    /// pass as two equal errors.
    #[test]
    fn big_endian_twins_read_identically() {
        let mut files: Vec<(String, Vec<u8>)> = [
            ("minimal", bin_minimal()),
            ("properties in any order", bin_any_order_with_extras()),
            ("ushort colour", bin_ushort_colour()),
            ("an unknown element with a list", bin_unknown_element_with_a_list()),
            ("an unknown fixed-size element", bin_unknown_fixed_size_element()),
            ("double positions", bin_double_positions()),
            ("cut mid-vertex", bin_cut_mid_vertex()),
            ("cut before the face", bin_cut_before_the_face()),
            ("cut in a face list", bin_cut_in_a_face_list()),
            ("4,000,000,000 vertices", bin_hostile_vertex_count()),
            ("a list longer than the bytes left", bin_hostile_list_length()),
            ("trailing padding", bin_trailing_padding()),
            ("a face index out of range", bin_face_index_out_of_range()),
            ("a negative face index", bin_negative_face_index()),
            ("a face of two vertices", bin_face_with_two_vertices()),
            ("a face before its vertices", bin_face_before_vertex()),
            ("vertex_index, singular", bin_singular_vertex_index()),
            ("a face past the index ceiling", bin_face_past_the_index_ceiling()),
        ]
        .into_iter()
        .map(|(name, file)| (name.to_string(), file))
        .collect();
        files.extend(bin_versions_of_the_ascii_cases());
        for (name, le) in &files {
            assert_eq!(read_scene_warned(&to_big_endian(le)), read_scene_warned(le), "{name}");
        }
        for (name, fixture) in fixtures::all() {
            let le = fixture_ply(name, &fixture);
            let want = read_scene_warned(&le).unwrap_or_else(|e| panic!("the {name} fixture's PLY doesn't read: {e:?}"));
            assert_eq!(read_scene_warned(&to_big_endian(&le)), Ok(want), "the {name} fixture");
        }
    }

    /// The hostile files, as big-endian twins, are refused as their
    /// little-endian originals are, with the same error: 4,000,000,000
    /// vertices before a record is read, a list longer than the bytes left,
    /// a face past the index ceiling before its corners are read, and three
    /// bodies that end early. Trailing padding is read past in both orders.
    #[test]
    fn big_endian_bounds_hold() {
        let is_malformed: fn(&MeshError) -> bool = |e| matches!(e, MeshError::Malformed(_));
        let is_unsupported: fn(&MeshError) -> bool = |e| matches!(e, MeshError::Unsupported(_));
        for (name, le, kind) in [
            ("4,000,000,000 vertices", bin_hostile_vertex_count(), is_malformed),
            ("a list longer than the bytes left", bin_hostile_list_length(), is_malformed),
            ("a face past the index ceiling", bin_face_past_the_index_ceiling(), is_unsupported),
            ("cut mid-vertex", bin_cut_mid_vertex(), is_malformed),
            ("cut before the face", bin_cut_before_the_face(), is_malformed),
            ("cut in a face list", bin_cut_in_a_face_list(), is_malformed),
        ] {
            let refused = read_scene_warned(&le);
            assert!(refused.as_ref().is_err_and(kind), "{name}: {refused:?}");
            assert_eq!(read_scene_warned(&to_big_endian(&le)), refused, "{name}");
        }
        let le = bin_trailing_padding();
        let read = read_scene_warned(&le).unwrap();
        assert_eq!(read.0.meshes[0].primitives[0].positions.len(), 3);
        assert_eq!(read_scene_warned(&to_big_endian(&le)), Ok(read), "trailing padding");
    }

    /// Two big-endian files, each cut at every length, with each of its bits
    /// flipped in turn, and with 2–8 bits flipped at random in 10,000 seeded
    /// cases: every reading is `Ok` or `Err`, never a panic. The cube
    /// fixture's twin has the writer's layout, whose list counts are `uchar`.
    /// Review Focus 2's file has `ushort` and `uint` counts, so the flips
    /// also reach a count of two or four bytes, read big-endian before it is
    /// bounded. Each file reads, as its little-endian twin does, before it is
    /// broken, and each is under 4 KiB, so the single-bit sweep is the whole
    /// file.
    #[test]
    fn big_endian_truncation_and_bit_flips_never_panic() {
        const FLIP_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
        let cube_le = fixture_ply("cube", &fixtures::cube());
        let cube = to_big_endian(&cube_le);
        let (counts, counts_le) = unknown_elements_twins();
        let survives = |bytes: &[u8]| std::panic::catch_unwind(move || read_scene_warned(bytes)).is_ok();
        for (name, be, le) in [("the cube's twin", cube, cube_le), ("Review Focus 2's file", counts, counts_le)] {
            assert!(be.len() < 4096, "{name} is {} bytes, past the 4 KiB the single-bit sweep is sized for", be.len());
            let want = read_scene_warned(&le).unwrap_or_else(|e| panic!("{name}, little-endian: {e:?}"));
            assert_eq!(read_scene_warned(&be), Ok(want), "{name} itself");
            for cut in 0..be.len() {
                assert!(survives(&be[..cut]), "{name}: cut to {cut} bytes");
            }
            let mut flipped = be.clone();
            for bit in 0..be.len() * 8 {
                flipped[bit / 8] ^= 1 << (bit % 8);
                assert!(survives(&flipped), "{name}: bit {bit} flipped");
                flipped[bit / 8] ^= 1 << (bit % 8);
            }
            // xorshift64 (Marsaglia, 2003): std only, and the same cases every
            // run, seeded again for each file.
            let mut state = FLIP_SEED;
            let mut next = move || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            };
            let bits = be.len() as u64 * 8;
            for case in 0..10_000 {
                let flips = 2 + next() % 7;
                let mut chosen: Vec<u64> = Vec::new();
                while (chosen.len() as u64) < flips {
                    let bit = next() % bits;
                    if !chosen.contains(&bit) {
                        chosen.push(bit);
                    }
                }
                let mut flipped = be.clone();
                for &bit in &chosen {
                    flipped[(bit / 8) as usize] ^= 1 << (bit % 8);
                }
                assert!(survives(&flipped), "{name}, case {case}: bits {chosen:?} flipped");
            }
        }
    }

    /// Review Focus 1: a list count of two or four bytes is read big-endian
    /// before it is held to the bytes left. Its two readings fall on either
    /// side of that bound, so a count read the wrong way round cannot pass:
    /// `03 00` is 768 corners, more than the 12 bytes left hold, where
    /// little-endian it would be the 3 that follow; and `00 03` is those 3,
    /// where little-endian it would be 768. The index ceiling's bound on a
    /// four-byte count is `big_endian_bounds_hold`'s face past the ceiling.
    #[test]
    fn big_endian_multibyte_list_counts_are_bounded() {
        const F0: &[u8] = &[0, 0, 0, 0]; // float 0
        const F1: &[u8] = &[0x3F, 0x80, 0, 0]; // float 1
        let vertices: [&[u8]; 9] = [F0, F0, F0, F1, F0, F0, F0, F1, F0];
        let corners: [&[u8]; 3] = [&[0, 0, 0, 0], &[0, 0, 0, 1], &[0, 0, 0, 2]];
        // Each count type, with a count past the bound and that count's
        // big-endian value, then a count of 3.
        let counts: [(&str, &[u8], usize, &[u8]); 4] = [
            ("short", &[0x03, 0x00], 768, &[0x00, 0x03]),
            ("ushort", &[0x03, 0x00], 768, &[0x00, 0x03]),
            ("int", &[0x03, 0, 0, 0], 50_331_648, &[0, 0, 0, 0x03]),
            ("uint", &[0x03, 0, 0, 0], 50_331_648, &[0, 0, 0, 0x03]),
        ];
        for (count_type, over, declared, three) in counts {
            let header = format!(
                "element vertex 3\nproperty float x\nproperty float y\nproperty float z\n\
                 element face 1\nproperty list {count_type} int vertex_indices\n"
            );
            let file = |count: &[u8]| {
                let mut body = vertices.to_vec();
                body.push(count);
                body.extend(corners);
                twins(&header, &body).0
            };
            match read_scene_warned(&file(over)) {
                Err(MeshError::Malformed(d)) => {
                    assert!(d.contains(&format!("list declares {declared} items")), "{count_type}: {d}")
                }
                other => panic!("{count_type}: {other:?}"),
            }
            let (scene, _) = read_scene_warned(&file(three)).unwrap_or_else(|e| panic!("{count_type}: {e:?}"));
            assert_eq!(scene.meshes[0].primitives[0].indices, vec![0, 1, 2], "{count_type}");
        }
    }

    /// Review Focus 2's file, big-endian first, then its little-endian twin:
    /// see `big_endian_unknown_elements_are_skipped_by_size`.
    fn unknown_elements_twins() -> (Vec<u8>, Vec<u8>) {
        const F0: &[u8] = &[0, 0, 0, 0]; // float 0
        const F1: &[u8] = &[0x3F, 0x80, 0, 0]; // float 1
        let header = "element junk 2\nproperty double a\nproperty ushort b\nproperty char c\n\
                      element vertex 3\nproperty float x\nproperty float y\nproperty float z\n\
                      element range_grid 3\nproperty list ushort int ends\nproperty list uint uchar tags\n\
                      property ushort flags\nelement face 1\nproperty list uchar int vertex_indices\n";
        let body: &[&[u8]] = &[
            // junk: a double, a ushort and a char, twice.
            &[0x3F, 0xF8, 0, 0, 0, 0, 0, 0], &[0x01, 0x02], &[0xFE],
            &[0xC0, 0x02, 0, 0, 0, 0, 0, 0], &[0xAB, 0xCD], &[0x7F],
            // vertex: (0, 0, 0), (1, 0, 0), (0, 1, 0).
            F0, F0, F0, F1, F0, F0, F0, F1, F0,
            // range_grid: two ends and a tag; neither; three ends and two tags.
            &[0x00, 0x02], &[0, 0, 0, 7], &[0, 0, 0, 8], &[0, 0, 0, 1], &[5], &[0x12, 0x34],
            &[0x00, 0x00], &[0, 0, 0, 0], &[0x00, 0x01],
            &[0x00, 0x03], &[0, 0, 0, 9], &[0, 0, 0, 10], &[0, 0, 0, 11], &[0, 0, 0, 2], &[1], &[2], &[0xFF, 0xFE],
            // face: 0 1 2.
            &[3], &[0, 0, 0, 0], &[0, 0, 0, 1], &[0, 0, 0, 2],
        ];
        twins(header, body)
    }

    /// Review Focus 2: elements the reader does not model are stepped over
    /// in a big-endian file by declared widths and list counts only. `junk`
    /// has a fixed size and is skipped whole. `range_grid` carries lists, so
    /// it is walked record by record, and each skipped list's count, two
    /// bytes and then four, is read big-endian to step the right distance:
    /// `00 02` read little-endian would be 512 items, past the end of the
    /// file. Each element reads right only if the one before it was stepped
    /// over exactly.
    #[test]
    fn big_endian_unknown_elements_are_skipped_by_size() {
        let (be, le) = unknown_elements_twins();
        let read = read_scene_warned(&be).unwrap_or_else(|e| panic!("{e:?}"));
        let p = &read.0.meshes[0].primitives[0];
        assert_eq!(p.positions, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        assert_eq!(p.indices, vec![0, 1, 2]);
        assert!(read.1.is_empty(), "{:?}", read.1);
        assert_eq!(read_scene_warned(&le), Ok(read), "the little-endian twin");
    }

    /// Review Focus 3: `format binary_big_endian` with no version reads as
    /// `binary_big_endian 1.0` does, and as little-endian with none does. A
    /// version other than 1.0 is refused in big-endian too.
    #[test]
    fn big_endian_without_a_version_reads() {
        for (name, le) in [("minimal", bin_minimal()), ("cube", fixture_ply("cube", &fixtures::cube()))] {
            let want = read_scene_warned(&le).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            let le_bare = replace_first(&le, "binary_little_endian 1.0\n", "binary_little_endian\n");
            let be_bare = to_big_endian(&le_bare);
            assert!(be_bare.starts_with(b"ply\nformat binary_big_endian\n"), "{name}");
            assert_eq!(read_scene_warned(&be_bare), Ok(want.clone()), "{name}: big-endian, no version");
            assert_eq!(read_scene_warned(&le_bare), Ok(want.clone()), "{name}: little-endian, no version");
            let be = to_big_endian(&le);
            assert_eq!(read_scene_warned(&be), Ok(want), "{name}: big-endian 1.0");
            let later = replace_first(&be, "binary_big_endian 1.0", "binary_big_endian 2.0");
            assert!(matches!(read_scene_warned(&later), Err(MeshError::Unsupported(d)) if d.contains("2.0")), "{name}");
        }
    }

    /// Review Focus 4: a big-endian file whose header lines end in CRLF. Its
    /// body starts just past `end_header\r\n`, as a little-endian file's
    /// does; a byte early or late would shift every value after it.
    #[test]
    fn big_endian_with_crlf_header_reads() {
        for (name, le) in [("minimal", bin_minimal()), ("cube", fixture_ply("cube", &fixtures::cube()))] {
            let want = read_scene_warned(&le).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            // `to_big_endian` reads CRLF headers too: either way round gives the same file.
            assert_eq!(to_big_endian(&with_crlf_header(&le)), with_crlf_header(&to_big_endian(&le)), "{name}");
            for (order, file) in [("big-endian", to_big_endian(&le)), ("little-endian", le)] {
                let crlf = with_crlf_header(&file);
                assert!(crlf.starts_with(b"ply\r\nformat binary_") && crlf.windows(12).any(|w| w == b"end_header\r\n"), "{name} {order}");
                assert_eq!(read_scene_warned(&crlf), Ok(want.clone()), "{name} {order}");
            }
        }
    }

    /// Review Focus 5: a file that says big-endian over a little-endian
    /// body. Every value of two bytes or more reads byte-reversed. For these
    /// files, the reading has to stop with an error at the first value that
    /// cannot be, never return a mesh made of those values, and never panic:
    /// an index past the vertices, a coordinate that is not a number, a list
    /// count past the bytes left, and the cube as our writer writes it. Not
    /// every mislabelled file can be caught: a point cloud, or a mesh with
    /// `uchar` indices, can read `Ok` as finite garbage.
    #[test]
    fn a_mislabelled_little_endian_body_fails_cleanly() {
        let mislabelled = |le: &[u8]| read_scene_warned(&replace_first(le, "binary_little_endian", "binary_big_endian"));
        // Corner 1, stored 01 00 00 00, reads as 16,777,216.
        let e = mislabelled(&bin_minimal()).unwrap_err();
        assert!(malformed(&e).contains("face index 16777216 out of range for 3 vertices"), "{e:?}");
        // x = 1.0078124 (bits 3F80FFFF), stored FF FF 80 3F, reads as a NaN.
        let body = Buf::default()
            .f32(f32::from_bits(0x3F80_FFFF)).f32(0.0).f32(0.0)
            .f32(1.0).f32(0.0).f32(0.0)
            .f32(0.0).f32(1.0).f32(0.0)
            .u8(3).i32(0).i32(1).i32(2);
        let e = mislabelled(&with_header(BIN_HEAD, body)).unwrap_err();
        assert!(malformed(&e).contains("x is NaN, which is not a finite number"), "{e:?}");
        // A ushort count of 3, stored 03 00, reads as 768.
        let head = BIN_HEAD.replace("list uchar int", "list ushort int");
        let e = mislabelled(&with_header(&head, bin_vertices().u16(3).i32(0).i32(1).i32(2))).unwrap_err();
        assert!(malformed(&e).contains("list declares 768 items"), "{e:?}");
        // The cube as our writer writes it: its corners misread too.
        let e = mislabelled(&fixture_ply("cube", &fixtures::cube())).unwrap_err();
        assert!(matches!(e, MeshError::Malformed(_)), "the cube: {e:?}");
    }

    /// A big-endian file this crate did not write: the cube fixture's PLY,
    /// rewritten by `testdata/ply/make_be.py` with Python's `struct` and a
    /// header parser of its own (see that folder's README). `to_big_endian`
    /// of the fixture's PLY is the same file byte for byte, so the two
    /// converters, written apart, would have to make one mistake the same way
    /// twice; and the file reads as the fixture's PLY does, scene and
    /// warnings alike. The bytes are checked first, because the file pins
    /// the writer's output for the cube: a change to the fixture, `prepare`,
    /// the sRGB encoding or the writer fails here as a stale file until the
    /// file is made again.
    #[test]
    fn an_independently_written_big_endian_file_reads() {
        const FILE: &[u8] = include_bytes!("testdata/ply/cube_be_struct.ply");
        assert!(FILE.starts_with(b"ply\nformat binary_big_endian 1.0\n"), "the file is not big-endian");
        let le = fixture_ply("cube", &fixtures::cube());
        assert!(
            to_big_endian(&le) == FILE,
            "cube_be_struct.ply is stale: the cube's PLY has changed since the file was made. \
             Remake it per src/mesh/testdata/ply/README.md"
        );
        let want = read_scene_warned(&le).unwrap_or_else(|e| panic!("the fixture's PLY: {e:?}"));
        assert_eq!(read_scene_warned(FILE), Ok(want));
    }

    /// Writes the cube fixture's PLY, `make_be.py`'s input, to the path in
    /// `PLY_CUBE_LE_OUT`, to make `testdata/ply/cube_be_struct.ply` again
    /// (see that folder's README): `PLY_CUBE_LE_OUT=/tmp/cube_le.ply cargo
    /// test --lib writes_the_cube_ply_to_the_file_named_by_the_environment --
    /// --ignored`.
    #[test]
    #[ignore = "set PLY_CUBE_LE_OUT to a path; writes the input of testdata/ply/make_be.py"]
    fn writes_the_cube_ply_to_the_file_named_by_the_environment() {
        let path = std::env::var("PLY_CUBE_LE_OUT").expect("PLY_CUBE_LE_OUT");
        std::fs::write(&path, fixture_ply("cube", &fixtures::cube())).unwrap_or_else(|e| panic!("{path}: {e}"));
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

    #[test]
    fn colour_is_encoded_as_srgb_bytes_like_every_ply_reader_expects() {
        // Linear 0.5 is sRGB byte 188. Writing 128 — the linear value scaled —
        // renders it as linear 0.216 in three.js, MeshLab and Blender: the
        // too-dark colours this codec used to produce.
        let mut m = tri_mesh();
        m.base_color = Some([0.5, 0.5, 0.5, 0.5]);
        let bytes = write(&m).unwrap();
        let head_end = bytes.windows(11).position(|w| w == b"end_header\n").unwrap() + 11;
        assert_eq!(&bytes[head_end + 12..head_end + 16], &[188, 188, 188, 128]);
    }

    #[test]
    fn the_scene_keeps_every_vertex_colour() {
        let src = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
                   property float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\n\
                   element face 1\nproperty list uchar int vertex_indices\nend_header\n\
                   0 0 0 255 0 0\n1 0 0 0 255 0\n0 1 0 0 0 255\n3 0 1 2\n";
        let scene = read_scene(src.as_bytes()).unwrap();
        let p = &scene.meshes[0].primitives[0];
        assert_eq!(
            p.colors.as_deref(),
            Some(&[[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]][..])
        );
        // And they survive a write.
        let back = read_scene(&write_scene(&scene).unwrap()).unwrap();
        assert_eq!(back.meshes[0].primitives[0].colors, p.colors);
    }

    #[test]
    fn binary_bytes_tagged_as_ply_are_not_this_format() {
        // A GLB handed over as PLY: the first "line" is binary, which says the
        // caller named the wrong container, not that a PLY header is broken.
        let glb = super::super::glb::write(&tri_mesh()).unwrap();
        match read(&glb).unwrap_err() {
            MeshError::NotThisFormat { expected: super::super::MeshFormat::Ply, .. } => {}
            e => panic!("{e:?}"),
        }
        // The magic is still the whole first line, not a prefix.
        assert!(matches!(read(b"plyfile\nformat ascii 1.0\n"), Err(MeshError::NotThisFormat { .. })));
    }

    /// One face of 4,000,003 corners, with an `int` count and `uchar` items.
    fn bin_face_past_the_index_ceiling() -> Vec<u8> {
        let n: usize = 4_000_003; // 3(n − 2) = 12,000,003 indices
        let mut bytes = b"ply\nformat binary_little_endian 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list int uchar vertex_indices\nend_header\n".to_vec();
        for v in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend_from_slice(&(n as i32).to_le_bytes());
        bytes.extend((0..n).map(|i| (i % 3) as u8));
        bytes
    }

    /// 2026-10-12 review, finding 10: a face's corners became a Vec<f64>, then
    /// a Vec<u32>, then 3(n−2) triangle indices, and the ceiling was checked
    /// only on the finished scene: one 133M-corner face (int count, uchar
    /// items, 127 MiB) reached 3.29 GB. Its corners are now counted against
    /// the index ceiling before they are read.
    #[test]
    fn a_face_past_the_index_ceiling_is_refused_before_its_corners_are_read() {
        let e = read_scene(&bin_face_past_the_index_ceiling()).unwrap_err();
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("face record 0") && m.contains("corners")), "{e:?}");
    }

    /// 2026-10-12 review, finding 17: each element was checked against every
    /// earlier one for a repeated name, so a 1 MiB header took 11.5 s.
    #[test]
    fn a_header_of_many_elements_is_checked_in_one_pass() {
        let mut h = String::from("ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\n");
        for i in 0..40_000 {
            h.push_str(&format!("element extra{i} 0\n"));
        }
        h.push_str("end_header\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n");
        let started = std::time::Instant::now();
        assert_eq!(read_scene(h.as_bytes()).unwrap().triangle_count(), 1);
        assert!(started.elapsed().as_millis() < 1500, "{:?}", started.elapsed());
        let dup = "ply\nformat ascii 1.0\nelement vertex 0\nproperty float x\nelement vertex 0\nproperty float x\nend_header\n";
        assert!(read_scene(dup.as_bytes()).is_err(), "a repeated element is still refused");
    }

    // ---- points and lines (mesh-gaps item 10, Task 3) -------------------------

    mod points_and_lines {
        use super::*;
        use super::super::super as mesh;
        use mesh::package::NoSidecars;
        use mesh::scene::{LossKind, Topology};

        const XYZ: &str = "property float x\nproperty float y\nproperty float z\n";

        /// An ASCII PLY: `elements` is the header after the format line.
        fn ascii(elements: &str, body: &str) -> String {
            format!("ply\nformat ascii 1.0\n{elements}end_header\n{body}")
        }

        /// A binary little-endian PLY with `elements` for a header.
        fn binary(elements: &str, body: Buf) -> Vec<u8> {
            with_header(&format!("ply\nformat binary_little_endian 1.0\n{elements}end_header\n"), body)
        }

        fn read_ok(file: &[u8]) -> (Scene, Vec<String>) {
            let r = mesh::read_scene(file, MeshFormat::Ply, &NoSidecars);
            assert!(r.is_ok(), "{:?}", r.as_ref().err());
            let r = r.unwrap();
            (r.scene, r.warnings)
        }

        fn elements(s: &Scene, topology: Topology) -> usize {
            s.meshes
                .iter()
                .flat_map(|m| &m.primitives)
                .filter(|p| p.topology == topology)
                .map(|p| p.indices.len() / topology.indices_per_element())
                .sum()
        }

        /// The topologies of a scene's primitives, in scene order.
        fn topologies(s: &Scene) -> Vec<Topology> {
            s.meshes.iter().flat_map(|m| &m.primitives).map(|p| p.topology).collect()
        }

        /// The first primitive of `topology`.
        fn first(s: &Scene, topology: Topology) -> &Primitive {
            s.meshes.iter().flat_map(|m| &m.primitives).find(|p| p.topology == topology).unwrap_or_else(|| panic!("no {topology:?} primitive in {:?}", topologies(s)))
        }

        const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
        const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
        const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
        const YELLOW: [f32; 4] = [1.0, 1.0, 0.0, 1.0];

        // ── reading ─────────────────────────────────────────────────────────

        /// A file with no `face` element is a point cloud: one points
        /// primitive of every vertex, with the colours the vertices carry, in
        /// ASCII and in binary, and with a `face` element of no records. The
        /// converter's own `read_scene` takes it as a scene: a gate that
        /// counts triangles refuses it as `Empty`.
        #[test]
        fn ply_without_faces_is_a_point_cloud() {
            let rgb = "property uchar red\nproperty uchar green\nproperty uchar blue\n";
            let head = format!("element vertex 4\n{XYZ}{rgb}");
            let ascii_file = ascii(&head, "0 0 0 255 0 0\n1 0 0 0 255 0\n1 1 0 0 0 255\n0 1 0 255 255 0\n").into_bytes();
            let mut body = Buf::default();
            for (p, c) in [([0.0, 0.0, 0.0], [255, 0, 0]), ([1.0, 0.0, 0.0], [0, 255, 0]), ([1.0, 1.0, 0.0], [0, 0, 255]), ([0.0, 1.0, 0.0], [255, 255, 0])] {
                body = body.f32(p[0]).f32(p[1]).f32(p[2]).u8(c[0]).u8(c[1]).u8(c[2]);
            }
            let binary_file = binary(&head, body);
            let no_faces = ascii(&format!("{head}element face 0\nproperty list uchar int vertex_indices\n"), "0 0 0 255 0 0\n1 0 0 0 255 0\n1 1 0 0 0 255\n0 1 0 255 255 0\n").into_bytes();
            for (label, file) in [("ascii", ascii_file), ("binary", binary_file), ("a face element of no faces", no_faces)] {
                let (s, warnings) = read_ok(&file);
                assert!(warnings.is_empty(), "{label}: {warnings:?}");
                assert_eq!((s.meshes.len(), s.meshes[0].primitives.len()), (1, 1), "{label}");
                let p = &s.meshes[0].primitives[0];
                assert_eq!((p.topology, &p.indices), (Topology::Points, &vec![0, 1, 2, 3]), "{label}");
                assert_eq!(p.positions, [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], "{label}");
                assert_eq!(p.colors, Some(vec![RED, GREEN, BLUE, YELLOW]), "{label}");
                assert_eq!((s.triangle_count(), s.drawable_count()), (0, 4), "{label}");
                s.validate().expect("valid");
                // The flat view draws triangles only: it has none to give.
                assert!(matches!(mesh::read(&file, MeshFormat::Ply), Err(MeshError::Empty)), "{label}: the flat read");
            }
        }

        /// An `edge` element (`vertex1`, `vertex2`) is a line list: one
        /// segment per record, over the file's vertices. The indices may be of
        /// any integer type.
        #[test]
        fn ply_edges_are_lines() {
            let ring = "0 0 0\n1 0 0\n1 1 0\n0 1 0\n";
            let ascii_file = ascii(&format!("element vertex 4\n{XYZ}element edge 4\nproperty int vertex1\nproperty int vertex2\n"), &format!("{ring}0 1\n1 2\n2 3\n3 0\n")).into_bytes();
            let mut body = Buf::default();
            for p in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]] {
                body = body.f32(p[0]).f32(p[1]).f32(p[2]);
            }
            for (a, b) in [(0, 1), (1, 2), (2, 3), (3, 0)] {
                body = body.u32(a).u32(b);
            }
            let binary_file = binary(&format!("element vertex 4\n{XYZ}element edge 4\nproperty uint vertex1\nproperty uint vertex2\n"), body);
            for (label, file) in [("ascii, int", ascii_file), ("binary, uint", binary_file)] {
                let (s, warnings) = read_ok(&file);
                assert!(warnings.is_empty(), "{label}: {warnings:?}");
                assert_eq!(topologies(&s), [Topology::Lines], "{label}");
                let p = &s.meshes[0].primitives[0];
                assert_eq!(p.indices, [0, 1, 1, 2, 2, 3, 3, 0], "{label}");
                assert_eq!(p.positions, [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], "{label}");
                assert_eq!((s.triangle_count(), elements(&s, Topology::Lines), s.drawable_count()), (0, 4, 4), "{label}");
                s.validate().expect("valid");
            }
        }

        /// An edge's own colour colours its two ends, over the vertices' own,
        /// as a face's colours its corners: each segment's ends are vertices
        /// of their own, so two edges that meet at a vertex can differ there.
        #[test]
        fn an_edges_colour_colours_its_ends() {
            let src = ascii(
                &format!("element vertex 3\n{XYZ}element edge 2\nproperty int vertex1\nproperty int vertex2\nproperty uchar red\nproperty uchar green\nproperty uchar blue\n"),
                "0 0 0\n1 0 0\n1 1 0\n0 1 255 0 0\n1 2 0 0 255\n",
            );
            let (s, _) = read_ok(src.as_bytes());
            let p = &s.meshes[0].primitives[0];
            assert_eq!((p.topology, &p.indices), (Topology::Lines, &vec![0, 1, 2, 3]));
            assert_eq!(p.positions, [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]], "the shared vertex is one at each edge");
            assert_eq!(p.colors, Some(vec![RED, RED, BLUE, BLUE]));
        }

        /// A file with faces and edges is two primitives, each over the
        /// vertices it uses and no others: PLY has one vertex pool and the
        /// scene one per primitive. A vertex neither uses is not drawn.
        #[test]
        fn faces_and_edges_are_two_primitives_over_the_vertices_each_uses() {
            let src = ascii(
                &format!("element vertex 6\n{XYZ}element face 1\nproperty list uchar int vertex_indices\nelement edge 1\nproperty int vertex1\nproperty int vertex2\n"),
                "0 0 0\n1 0 0\n0 1 0\n5 5 5\n6 5 5\n9 9 9\n3 0 1 2\n3 4\n",
            );
            let (s, _) = read_ok(src.as_bytes());
            assert_eq!(topologies(&s), [Topology::Triangles, Topology::Lines]);
            let (tri, line) = (first(&s, Topology::Triangles), first(&s, Topology::Lines));
            assert_eq!((&tri.positions, &tri.indices), (&vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], &vec![0, 1, 2]));
            assert_eq!((&line.positions, &line.indices), (&vec![[5.0, 5.0, 5.0], [6.0, 5.0, 5.0]], &vec![0, 1]));
            assert_eq!(s.drawable_count(), 2);
        }

        /// Review Focus 5: only a file with no faces is a point cloud. A
        /// file with faces and vertices no face uses keeps them as the
        /// vertices they are, in the triangles' primitive, and makes no
        /// points of them, with edges beside the faces or without.
        #[test]
        fn unreferenced_vertices_beside_faces_are_not_points() {
            let verts = "0 0 0\n1 0 0\n0 1 0\n7 7 7\n8 8 8\n";
            let faces = ascii(&format!("element vertex 5\n{XYZ}element face 1\nproperty list uchar int vertex_indices\n"), &format!("{verts}3 0 1 2\n"));
            let (s, _) = read_ok(faces.as_bytes());
            assert_eq!(topologies(&s), [Topology::Triangles], "no points primitive");
            let p = &s.meshes[0].primitives[0];
            assert_eq!((p.positions.len(), &p.indices), (5, &vec![0, 1, 2]), "the vertices stay where they were");
            assert_eq!((elements(&s, Topology::Points), s.drawable_count()), (0, 1));
            let both = ascii(
                &format!("element vertex 5\n{XYZ}element face 1\nproperty list uchar int vertex_indices\nelement edge 1\nproperty int vertex1\nproperty int vertex2\n"),
                &format!("{verts}3 0 1 2\n0 2\n"),
            );
            let (s, _) = read_ok(both.as_bytes());
            assert_eq!(topologies(&s), [Topology::Triangles, Topology::Lines], "still no points primitive");
            assert_eq!(s.drawable_count(), 2);
        }

        /// Faces, edges and a colour on each face: the faces take the corner
        /// vertices their colours need, and the edges, read from the same
        /// pool, are not disturbed by it.
        #[test]
        fn face_colours_and_edges_read_from_the_same_pool() {
            let src = ascii(
                &format!("element vertex 5\n{XYZ}element face 1\nproperty list uchar int vertex_indices\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nelement edge 1\nproperty int vertex1\nproperty int vertex2\n"),
                "0 0 0\n1 0 0\n0 1 0\n5 5 5\n6 5 5\n3 0 1 2 255 0 0\n3 4\n",
            );
            let (s, _) = read_ok(src.as_bytes());
            assert_eq!(topologies(&s), [Topology::Triangles, Topology::Lines]);
            assert_eq!(first(&s, Topology::Triangles).colors, Some(vec![RED; 3]));
            assert_eq!(first(&s, Topology::Lines).positions, [[5.0, 5.0, 5.0], [6.0, 5.0, 5.0]]);
        }

        /// An edge element that does not hold `vertex1` and `vertex2` is not
        /// one this reader can walk to a segment: it is stepped over by its
        /// layout, as before, and the warning names what was not read.
        #[test]
        fn an_edge_element_without_vertex1_and_vertex2_is_skipped_and_named() {
            let src = ascii(
                &format!("element vertex 3\n{XYZ}element edge 2\nproperty list uchar int ends\nproperty uchar flags\nelement face 1\nproperty list uchar int vertex_indices\n"),
                "0 0 0\n1 0 0\n0 1 0\n2 0 1 9\n3 0 1 2 9\n3 0 1 2\n",
            );
            let (s, warnings) = read_ok(src.as_bytes());
            assert_eq!(topologies(&s), [Topology::Triangles]);
            assert!(warnings.iter().any(|w| w.contains("edge") && w.contains("vertex1") && w.contains("vertex2") && w.contains('2')), "{warnings:?}");
        }

        /// What an edge may not be: an index past the vertices, a negative
        /// one, a real number, or one end without the other.
        #[test]
        fn a_broken_edge_is_refused() {
            let edge = |props: &str, record: &str| -> MeshError {
                let src = ascii(&format!("element vertex 2\n{XYZ}element edge 1\n{props}"), &format!("0 0 0\n1 0 0\n{record}\n"));
                read_scene_warned(src.as_bytes()).err().unwrap_or_else(|| panic!("read, expected a refusal: {props:?} {record}"))
            };
            let ints = "property int vertex1\nproperty int vertex2\n";
            for (props, record, needle) in [
                (ints, "0 2", "edge vertex index 2 is out of range"),
                (ints, "-1 0", "edge vertex index -1 is out of range"),
                ("property float vertex1\nproperty int vertex2\n", "0 1", "vertex indices must be an integer type"),
                ("property int vertex1\nproperty uchar flags\n", "0 1", "vertex1 but not vertex2"),
                ("property list uchar int vertex1\nproperty int vertex2\n", "1 0 1", "is a list; it must be a scalar"),
            ] {
                match edge(props, record) {
                    MeshError::Malformed(d) => assert!(d.contains(needle), "{props:?} {record}: {d}"),
                    e => panic!("{props:?} {record}: {e:?}"),
                }
            }
        }

        /// A Gaussian-splat PLY (the 3D Gaussian Splatting training output)
        /// reads as points with their positions; its splat properties
        /// (`f_dc_*` spherical-harmonic colour, `opacity`, `scale_*`, `rot_*`)
        /// are named in one warning. Where it also has `red`/`green`/`blue`,
        /// the colours come too.
        #[test]
        fn a_gaussian_splat_ply_is_points_and_names_what_it_does_not_carry() {
            let splat = "property float f_dc_0\nproperty float f_dc_1\nproperty float f_dc_2\nproperty float opacity\n\
                         property float scale_0\nproperty float scale_1\nproperty float scale_2\n\
                         property float rot_0\nproperty float rot_1\nproperty float rot_2\nproperty float rot_3\n";
            let row = |p: [f32; 3], rgb: Option<[u8; 3]>| {
                let mut b = Buf::default().f32(p[0]).f32(p[1]).f32(p[2]);
                for k in 0..11 {
                    b = b.f32(0.25 * k as f32);
                }
                match rgb {
                    Some(c) => b.u8(c[0]).u8(c[1]).u8(c[2]),
                    None => b,
                }
            };
            let join = |a: Buf, b: Buf| Buf([a.0, b.0].concat());
            let plain = binary(&format!("element vertex 2\n{XYZ}{splat}"), join(row([1.0, 2.0, 3.0], None), row([4.0, 5.0, 6.0], None)));
            let (s, warnings) = read_ok(&plain);
            assert_eq!(topologies(&s), [Topology::Points]);
            assert_eq!(s.meshes[0].primitives[0].positions, [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
            assert_eq!(s.meshes[0].primitives[0].colors, None);
            assert_eq!(warnings.len(), 1, "one warning: {warnings:?}");
            for name in ["f_dc_*", "opacity", "scale_*", "rot_*"] {
                assert!(warnings[0].contains(name), "{name} is named: {warnings:?}");
            }
            let tinted = binary(
                &format!("element vertex 2\n{XYZ}{splat}property uchar red\nproperty uchar green\nproperty uchar blue\n"),
                join(row([1.0, 2.0, 3.0], Some([255, 0, 0])), row([4.0, 5.0, 6.0], Some([0, 0, 255]))),
            );
            let (s, warnings) = read_ok(&tinted);
            assert_eq!(s.meshes[0].primitives[0].colors, Some(vec![RED, BLUE]));
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            // The warning is made at the vertices, before the file's faces are
            // known, so it says nothing of points: it is true where the same
            // file has faces too, and there the vertices are not points.
            assert!(!warnings[0].contains("as points"), "{warnings:?}");
            let mut body = Buf::default();
            for p in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
                body = body.f32(p[0]).f32(p[1]).f32(p[2]).f32(0.5).f32(0.5);
            }
            let with_face = binary(
                &format!("element vertex 3\n{XYZ}property float opacity\nproperty float scale_0\nelement face 1\nproperty list uchar int vertex_indices\n"),
                body.u8(3).i32(0).i32(1).i32(2),
            );
            let (s, warnings) = read_ok(&with_face);
            assert_eq!(topologies(&s), [Topology::Triangles]);
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(warnings[0].contains("Gaussian splat") && !warnings[0].contains("as points"), "{warnings:?}");
        }

        // ── the content rule: prepare and losses ────────────────────────────

        /// `wire`'s meshes, `names` of them, as a scene.
        fn pieces(names: &[&str]) -> Scene {
            let wire = fixtures::wire();
            let mut s = Scene { materials: wire.materials.clone(), ..Default::default() };
            for name in names {
                s.meshes.push(wire.meshes.iter().find(|m| m.name.as_deref() == Some(name)).unwrap_or_else(|| panic!("no mesh {name}")).clone());
                let mesh = s.meshes.len() - 1;
                s.add_root(Node { name: Some((*name).into()), mesh: Some(mesh), ..Default::default() });
            }
            s
        }

        /// PLY has one vertex pool, so points beside faces or edges would
        /// read back as nothing. `prepare` keeps points only in a scene with
        /// no triangles and no lines, and `losses` names what it left out, by
        /// primitive, the way 3MF's content rules are named; the lines and
        /// triangles are never left out.
        #[test]
        fn ply_keeps_points_only_alone() {
            let opts = WriteOptions::default();
            let cases: [(&[&str], &[Topology], usize); 7] = [
                (&["wire_points"], &[Topology::Points], 0),
                (&["wire_points", "wire_loop"], &[Topology::Lines], 1),
                (&["wire_points", "wire_cube"], &[Topology::Triangles], 1),
                (&["wire_points", "wire_loop", "wire_cube"], &[Topology::Triangles, Topology::Lines], 1),
                (&["wire_points", "wire_points", "wire_cube"], &[Topology::Triangles], 2),
                (&["wire_loop", "wire_cube"], &[Topology::Triangles, Topology::Lines], 0),
                (&["wire_loop"], &[Topology::Lines], 0),
            ];
            for (names, kept, dropped) in cases {
                let s = pieces(names);
                let ctx = format!("{names:?}");
                let prepared = MeshFormat::Ply.prepare(&s, opts).unwrap_or_else(|e| panic!("{ctx}: {e}"));
                prepared.validate().expect("valid");
                assert_eq!(topologies(&prepared), kept, "{ctx}");
                let said: Vec<(LossKind, String)> = losses(&s).into_iter().map(|l| (l.kind, l.what)).collect();
                let want: Vec<(LossKind, String)> = if dropped == 0 { Vec::new() } else { vec![(LossKind::Points, format!("{dropped} point primitive(s)"))] };
                assert_eq!(said, want, "{ctx}");
            }
            // The points that stay are all of them.
            let alone = MeshFormat::Ply.prepare(&pieces(&["wire_points"]), opts).unwrap();
            assert_eq!((elements(&alone, Topology::Points), alone.meshes.len()), (9, 1));
            // And the loss is the converter's too: the write names it once.
            let w = mesh::write_scene(&pieces(&["wire_points", "wire_cube"]), MeshFormat::Ply, opts).expect("write");
            let named: Vec<&str> = w.losses.iter().filter(|l| l.kind == LossKind::Points).map(|l| l.what.as_str()).collect();
            assert_eq!(named, ["1 point primitive(s)"]);
        }

        /// A scene of points alone is written as a clean point cloud (no
        /// `face` element, no `edge` element), one of lines alone as vertices
        /// and edges, and each reads back as itself: the positions bit for
        /// bit and the indices as written, the colours as bytes hold them.
        /// Through the converter's `write_scene` and `read_scene`, so a gate
        /// that counts triangles fails it.
        #[test]
        fn a_scene_of_points_or_lines_alone_writes_and_reads_back() {
            let wire = fixtures::wire();
            for (name, topology, has_edges) in [("wire_points", Topology::Points, false), ("wire_loop", Topology::Lines, true)] {
                let alone = pieces(&[name]);
                let w = mesh::write_scene(&alone, MeshFormat::Ply, WriteOptions::default());
                assert!(w.is_ok(), "{name}: {:?}", w.as_ref().err());
                let file = w.unwrap().package.files.remove(0).bytes;
                let header = String::from_utf8_lossy(&file[..file.windows(11).position(|w| w == b"end_header\n").unwrap()]).into_owned();
                assert!(!header.contains("element face"), "{name}: {header}");
                assert_eq!(header.contains("element edge"), has_edges, "{name}: {header}");
                let (back, _) = read_ok(&file);
                let (want, got) = (first(&wire, topology), first(&back, topology));
                let bits = |p: &Primitive| -> Vec<u32> { p.positions.iter().flatten().map(|v| v.to_bits()).collect() };
                assert_eq!((&got.indices, bits(got)), (&want.indices, bits(want)), "{name}");
                let bytes = |p: &Primitive| -> Vec<[u8; 4]> { p.colors.as_ref().unwrap().iter().map(|c| quantise_rgba(*c)).collect() };
                assert_eq!(bytes(got), bytes(want), "{name}: the colours, as bytes");
                assert_eq!(topologies(&back), [topology], "{name}");
            }
        }

        /// `fixtures::wire()` through the writer and the reader: the cube's
        /// triangles and the loop's segments come back, the loop's positions
        /// bit for bit, its indices as written, its colours as bytes hold
        /// them, and the grid's points, which PLY cannot keep beside faces,
        /// are named once. The shared vertex pool gives the loop the normals
        /// the cube has (`+Z`, which is what a merge gives a primitive that
        /// draws no triangle), and `prepare` says so.
        #[test]
        fn wire_round_trips_through_ply() {
            let wire = fixtures::wire();
            let opts = WriteOptions::default();
            let w = mesh::write_scene(&wire, MeshFormat::Ply, opts).expect("write");
            let points: Vec<&str> = w.losses.iter().filter(|l| l.kind == LossKind::Points).map(|l| l.what.as_str()).collect();
            assert_eq!(points, ["1 point primitive(s)"]);
            assert!(!w.losses.iter().any(|l| l.kind == LossKind::Lines), "{:?}", w.losses);
            let file = &w.package.files[0].bytes;
            let header = String::from_utf8_lossy(&file[..file.windows(11).position(|w| w == b"end_header\n").unwrap()]).into_owned();
            assert!(header.contains("element face 12\n") && header.contains("element edge 4\n"), "{header}");
            let (back, warnings) = read_ok(file);
            assert!(warnings.is_empty(), "{warnings:?}");
            assert_eq!(topologies(&back), [Topology::Triangles, Topology::Lines]);
            assert_eq!((elements(&back, Topology::Triangles), elements(&back, Topology::Lines), elements(&back, Topology::Points)), (12, 4, 0));
            let (want, got) = (first(&wire, Topology::Lines), first(&back, Topology::Lines));
            let bits = |p: &Primitive| -> Vec<u32> { p.positions.iter().flatten().map(|v| v.to_bits()).collect() };
            assert_eq!((&got.indices, bits(got), &got.colors), (&want.indices, bits(want), &want.colors));
            assert_eq!(got.normals, Some(vec![[0.0, 0.0, 1.0]; 4]));
            // `prepare` is what comes back.
            let prepared = MeshFormat::Ply.prepare(&wire, opts).expect("prepare");
            assert_eq!(topologies(&prepared), topologies(&back));
            let (p, g) = (first(&prepared, Topology::Lines), got);
            assert_eq!((&p.indices, bits(p), &p.normals, &p.colors), (&g.indices, bits(g), &g.normals, &g.colors));
        }

        /// One vertex pool has one set of columns: a normal, a texture
        /// coordinate or a colour that only some of the primitives have is
        /// given to the others as a merge gives it (`+Z`, `(0, 0)`, white),
        /// and `prepare` says so, so that the file and the scene agree.
        #[test]
        fn the_primitives_of_one_file_share_its_columns() {
            let mut s = pieces(&["wire_cube", "wire_loop"]);
            s.meshes[0].primitives[0].colors = None;
            s.materials.clear();
            s.meshes[0].primitives[0].material = None;
            s.meshes[0].primitives[0].uvs = vec![vec![[0.5, 0.25]; s.meshes[0].primitives[0].positions.len()]];
            let prepared = MeshFormat::Ply.prepare(&s, WriteOptions::default()).expect("prepare");
            for p in prepared.meshes.iter().flat_map(|m| &m.primitives) {
                assert!(p.normals.is_some() && p.colors.is_some() && p.uvs.len() == 1, "{:?}: every primitive has every column", p.topology);
            }
            let line = first(&prepared, Topology::Lines);
            assert_eq!((line.normals.as_ref().unwrap()[0], line.uvs[0][0]), ([0.0, 0.0, 1.0], [0.0, 0.0]));
            let w = mesh::write_scene(&s, MeshFormat::Ply, WriteOptions::default()).expect("write");
            let (back, _) = read_ok(&w.package.files[0].bytes);
            assert_eq!(first(&back, Topology::Lines).uvs, line.uvs);
            assert_eq!(first(&back, Topology::Lines).normals, line.normals);
        }

        /// `prepare` is what the reader gives back, vertices included: edges
        /// are read over the vertices they use, and so are the faces beside
        /// them; a point cloud is a point for each vertex, in order. A scene
        /// that names its vertices otherwise is renumbered the same way.
        #[test]
        fn prepare_numbers_the_vertices_as_the_reader_does() {
            let quad = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
            let prim = |topology: Topology, indices: &[u32]| Primitive { positions: quad.clone(), indices: indices.to_vec(), topology, ..Default::default() };
            let scene = |prims: Vec<Primitive>| {
                let mut s = Scene::default();
                for (k, p) in prims.into_iter().enumerate() {
                    s.meshes.push(SceneMesh { name: Some(format!("m{k}")), primitives: vec![p], ..Default::default() });
                    s.add_root(Node { name: Some(format!("m{k}")), mesh: Some(k), ..Default::default() });
                }
                s
            };
            // Five vertices of which the points use the first three, in order: an
            // identity prefix, such as a glTF POINTS primitive over a subset.
            let subset = Primitive {
                positions: quad.iter().copied().chain([[0.5, 0.5, 1.0]]).collect(),
                indices: vec![0, 1, 2],
                topology: Topology::Points,
                ..Default::default()
            };
            // A triangle on 3 of 4 vertices beside a segment on 2 of them; and points, 3 of 4 and one twice.
            let cases = [
                ("faces and edges", scene(vec![prim(Topology::Triangles, &[0, 1, 3]), prim(Topology::Lines, &[3, 0])])),
                ("points", scene(vec![prim(Topology::Points, &[2, 0, 2])])),
                ("points over a subset", scene(vec![subset])),
                ("edges alone", scene(vec![prim(Topology::Lines, &[2, 3, 3, 1])])),
            ];
            for (label, s) in cases {
                let opts = WriteOptions::default();
                let prepared = MeshFormat::Ply.prepare(&s, opts).unwrap_or_else(|e| panic!("{label}: {e}"));
                let w = mesh::write_scene(&s, MeshFormat::Ply, opts).unwrap_or_else(|e| panic!("{label}: {e}"));
                let (back, _) = read_ok(&w.package.files[0].bytes);
                let shape = |s: &Scene| -> Vec<(Topology, Vec<[f32; 3]>, Vec<u32>)> {
                    s.meshes.iter().flat_map(|m| &m.primitives).map(|p| (p.topology, p.positions.clone(), p.indices.clone())).collect()
                };
                assert_eq!(shape(&prepared), shape(&back), "{label}");
                // And the numbers themselves, for the cases that renumber.
                let (kind, want) = match label {
                    "points" => (Topology::Points, (vec![0, 1, 2], vec![quad[2], quad[0], quad[2]])),
                    "points over a subset" => (Topology::Points, (vec![0, 1, 2], vec![quad[0], quad[1], quad[2]])),
                    "edges alone" => (Topology::Lines, (vec![1, 2, 2, 0], vec![quad[1], quad[2], quad[3]])),
                    _ => (Topology::Lines, (vec![1, 0], vec![quad[0], quad[3]])),
                };
                let p = first(&prepared, kind);
                assert_eq!((p.indices.clone(), p.positions.clone()), want, "{label}");
            }
        }

        /// The writer takes what `prepare` gives it, and refuses what no
        /// file could say: points beside faces would read back as nothing,
        /// and two primitives of one topology are two pools.
        #[test]
        fn the_writer_refuses_what_prepare_would_have_changed() {
            let both = pieces(&["wire_cube", "wire_points"]);
            match write_scene(&both) {
                Err(MeshError::Malformed(d)) => assert!(d.contains("points") && d.contains("faces"), "{d}"),
                other => panic!("{other:?}"),
            }
            let twice = pieces(&["wire_cube", "wire_cube"]);
            match write_scene(&twice) {
                Err(MeshError::Malformed(d)) => assert!(d.contains("merged"), "{d}"),
                other => panic!("{other:?}"),
            }
        }

        // ── hostile input ───────────────────────────────────────────────────

        /// A PLY header of `n` vertices (x, y, z floats) and `extra` elements
        /// after it, with a body of `body` bytes of zeros.
        fn zeros(n: usize, extra: &str, body: usize) -> Vec<u8> {
            let mut file = format!("ply\nformat binary_little_endian 1.0\nelement vertex {n}\n{XYZ}{extra}end_header\n").into_bytes();
            file.resize(file.len() + body, 0);
            file
        }

        /// A 4M-point cloud is the ceiling: within the scene's caps (4M
        /// vertices, 12M indices), it reads as 4M points, and one point more
        /// is refused at its vertex element, naming the ceiling.
        #[test]
        fn a_four_million_point_cloud_is_within_the_scene_caps() {
            let started = std::time::Instant::now();
            let at = mesh::scene::MAX_SCENE_VERTICES;
            assert_eq!(at, 4_000_000, "the counts below are written for this ceiling");
            let (s, _) = read_ok(&zeros(at, "", at * 12));
            assert_eq!((topologies(&s), s.drawable_count()), (vec![Topology::Points], at));
            let e = mesh::read_scene(&zeros(at + 1, "", (at + 1) * 12), MeshFormat::Ply, &NoSidecars).err();
            assert!(matches!(&e, Some(MeshError::Unsupported(m)) if m.contains("4000000-vertex ceiling")), "{e:?}");
            eprintln!("a 4M-point PLY, read and refused: {:?}", started.elapsed());
        }

        /// Edges are charged to the index budget (12M) as faces are, from the
        /// element's declared count and before one record is read: 6,000,001
        /// edges (12,000,002 indices) with all their bytes present are
        /// refused, naming the ceiling, and so is a declared count of billions
        /// in a file with none.
        #[test]
        fn edges_are_charged_to_the_index_budget_before_they_are_read() {
            let edges = "element edge 6000001\nproperty int vertex1\nproperty int vertex2\n";
            let e = mesh::read_scene(&zeros(3, edges, 3 * 12 + 6_000_001 * 8), MeshFormat::Ply, &NoSidecars).err();
            assert!(matches!(&e, Some(MeshError::Unsupported(m)) if m.contains("12000000-index ceiling")), "{e:?}");
            let ascii_edges = ascii(&format!("element vertex 2\n{XYZ}element edge 4000000000\nproperty int vertex1\nproperty int vertex2\n"), "0 0 0\n1 0 0\n0 1\n");
            let e = read_scene_warned(ascii_edges.as_bytes()).err();
            assert!(matches!(&e, Some(MeshError::Unsupported(m)) if m.contains("12000000-index ceiling")), "{e:?}");
        }

        /// Faces, strips and edges are one index budget, whichever comes first.
        /// Edges after faces are charged with the faces already read: two
        /// triangles (6 indices) and 5,999,999 edges (11,999,998) pass the
        /// ceiling by 4, and the refusal comes from the declared count, with no
        /// edge record in the file. 5,999,997 edges reach the ceiling exactly,
        /// so the charge lets them through (the file then ends for want of
        /// records). A face or a strip after edges is bounded by what the
        /// edges leave (`reserved`): after 100 edges (200 indices) it may have
        /// 3,999,935 corners, so one of 4,000,000, which a budget that forgot
        /// the edges would still read, is refused from its count alone.
        #[test]
        fn edges_faces_and_strips_share_the_index_budget() {
            let head = |after: &str| format!("element vertex 3\n{XYZ}{after}");
            let edges = |n: usize| format!("element edge {n}\nproperty int vertex1\nproperty int vertex2\n");
            let two_triangles = "element face 2\nproperty list uchar int vertex_indices\n";
            let body = "0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n3 0 1 2\n";

            let over = ascii(&head(&format!("{two_triangles}{}", edges(5_999_999))), body);
            let e = read_scene_warned(over.as_bytes()).err();
            assert!(
                matches!(&e, Some(MeshError::Unsupported(m)) if m.contains("5999999 edges") && m.contains("12000000-index ceiling")),
                "one over, from the declared count: {e:?}"
            );
            let at = ascii(&head(&format!("{two_triangles}{}", edges(5_999_997))), body);
            let e = read_scene_warned(at.as_bytes()).err();
            assert!(e.as_ref().is_some_and(|x| !x.to_string().contains("ceiling")), "at the ceiling the charge passes: {e:?}");

            for (element, what) in [("face", "a face"), ("tristrips", "a strip")] {
                let list = format!("element {element} 1\nproperty list int int vertex_indices\n");
                let body = format!("0 0 0\n1 0 0\n0 1 0\n{}4000000\n", "0 1\n".repeat(100));
                let file = ascii(&head(&format!("{}{list}", edges(100))), &body);
                let e = read_scene_warned(file.as_bytes()).err();
                assert!(
                    matches!(&e, Some(MeshError::Unsupported(m)) if m.contains("a face of 4000000 corners; only 3999935 fit")),
                    "{what} after 100 edges: {e:?}"
                );
            }
        }

        /// The overview's hostile-input bar on the new paths. Three lean
        /// files, each read whole (carrying what they should), then swept
        /// ([`layer_sweep`]) with every bit of them among the single flips,
        /// and read through the converter's `read_scene`: every reading is
        /// `Ok` or `Err`, never a panic.
        #[test]
        fn ply_points_and_lines_survive_truncation_and_bit_flips() {
            use mesh::fixtures::sweeps::{layer_sweep, span};
            let ring = "0 0 0\n1 0 0\n1 1 0\n0 1 0\n";
            // Edges with colours, beside a face.
            let edges = binary(
                &format!("element vertex 4\n{XYZ}element face 1\nproperty list uchar int vertex_indices\nelement edge 4\nproperty int vertex1\nproperty int vertex2\nproperty uchar red\nproperty uchar green\nproperty uchar blue\n"),
                {
                    let mut b = Buf::default();
                    for p in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]] {
                        b = b.f32(p[0]).f32(p[1]).f32(p[2]);
                    }
                    b = b.u8(3).i32(0).i32(1).i32(2);
                    for (k, (a, c)) in [(0, 1), (1, 2), (2, 3), (3, 0)].into_iter().enumerate() {
                        b = b.i32(a).i32(c).u8(k as u8 * 60).u8(255 - k as u8 * 60).u8(7);
                    }
                    b
                },
            );
            // A coloured ASCII point cloud.
            let cloud = ascii(&format!("element vertex 4\n{XYZ}property uchar red\nproperty uchar green\nproperty uchar blue\n"), &ring.lines().map(|l| format!("{l} 10 20 30\n")).collect::<String>()).into_bytes();
            // A splat: the properties it names and skips, with a colour.
            let splat = binary(
                &format!("element vertex 2\n{XYZ}property float f_dc_0\nproperty float opacity\nproperty float scale_0\nproperty float rot_0\nproperty uchar red\nproperty uchar green\nproperty uchar blue\n"),
                Buf::default().f32(1.0).f32(2.0).f32(3.0).f32(0.5).f32(0.5).f32(0.5).f32(0.5).u8(1).u8(2).u8(3).f32(4.0).f32(5.0).f32(6.0).f32(0.5).f32(0.5).f32(0.5).f32(0.5).u8(4).u8(5).u8(6),
            );
            for (name, file, want, marker) in [
                ("a lean PLY of a face and coloured edges", edges, (1, 4, 0), "element face"),
                ("a lean ASCII point cloud", cloud, (0, 0, 4), "element vertex"),
                ("a lean splat", splat, (0, 0, 2), "element vertex"),
            ] {
                let whole = read_ok(&file).0;
                let got = (elements(&whole, Topology::Triangles), elements(&whole, Topology::Lines), elements(&whole, Topology::Points));
                assert_eq!(got, want, "{name} itself carries them");
                layer_sweep(name, &file, span(name, &file, marker, "end_header"), |bytes| {
                    let _ = mesh::read_scene(bytes, MeshFormat::Ply, &NoSidecars);
                });
            }
        }

        /// The flags of both rows of the table say what the codecs now do.
        #[test]
        fn the_capabilities_hold_points_and_lines() {
            for opts in [WriteOptions::default(), WriteOptions { self_contained: false, ..Default::default() }] {
                let caps = MeshFormat::Ply.capabilities(opts);
                assert!(caps.points && caps.lines, "{caps:?}");
            }
        }
    }
}
