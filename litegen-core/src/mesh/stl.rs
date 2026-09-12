//! STL: both variants in, binary out.
//!
//! # Telling the two variants apart
//!
//! The test everyone writes first — "does the file start with `solid`?" — is
//! wrong, and getting it wrong is the single most common STL bug. The binary
//! layout opens with 80 bytes of free-form header, and a long line of exporters
//! (SolidWorks among them) fill it with text that begins with the word `solid`.
//! Sniffing the prefix therefore hands a binary file to the ASCII parser, which
//! either errors on float data that is not text or — worse — scrapes a few
//! plausible-looking numbers out of it and returns a mesh that is nonsense.
//!
//! What is unambiguous is the arithmetic: a binary STL is EXACTLY
//! `84 + 50 * n` bytes, where `n` is the little-endian u32 at offset 80. So the
//! length is checked first and the keyword second.
//!
//! Within `super::MAX_INPUT_BYTES` that order is not merely better, it is
//! exact. For the equation to hold, `n = (len - 84) / 50`; at the 128 MiB cap
//! that bounds `n` below `0x0028_F5B0`, whose top byte — the file's byte 83 — is
//! therefore `0x00`. A text STL cannot contain a NUL, so no ASCII file under the
//! cap can satisfy the binary test, and the two detections cannot collide.
//!
//! # What STL cannot carry
//!
//! No indices, no uvs, no colour, and one normal per FACET rather than per
//! vertex — which is why the lossiness table in `super` lists STL as keeping
//! only positions and facet normals. See [`read`] for what that costs on the way
//! in and [`write`] for what it costs on the way out.
//!
//! # Why the writer emits binary only
//!
//! ASCII STL is roughly five times larger for the same geometry — a coordinate
//! that is 4 bytes binary becomes 12-15 characters of text, and every facet
//! carries seven keyword lines on top of its nine numbers — and it buys nothing.
//! There is no field binary cannot express, and every consumer reads both. The
//! reader accepts ASCII because vendors and CAD tools emit it; the writer has no
//! reason to produce it.
//!
//! # Error taxonomy
//!
//! STL has no magic number, so the header checks above ARE its identity:
//! `NotThisFormat` covers bytes that pass neither (including a binary file whose
//! declared count disagrees with its length, which is what truncation looks
//! like). `Malformed` is for bytes that announced themselves as STL and then
//! failed to parse — a bad number, a facet block out of order, a non-finite
//! coordinate. `Unsupported` is for the one thing STL files contain that this
//! reader declines to guess at: a facet loop with more than three vertices.

use super::ir::Mesh;
use super::MeshError;

const HEADER_LEN: usize = 80;
/// Header plus the u32 triangle count — everything before the first triangle.
const BINARY_PREFIX_LEN: usize = HEADER_LEN + 4;
/// 12 floats (facet normal, then three vertices) and a u16 attribute word.
const TRI_RECORD_LEN: usize = 50;

/// What we stamp into the 80-byte header we write.
///
/// It must not begin with `solid`: that is the mirror image of the read bug in
/// the module docs, and readers that sniff the prefix are everywhere, including
/// in tools we will never get to fix. The trailing space is part of the tag
/// because the mesh name follows it.
const HEADER_TAG: &str = "litegen stl ";

fn not_stl(detail: String) -> MeshError {
    MeshError::NotThisFormat { expected: super::MeshFormat::Stl, detail }
}

/// Bounds-checked little-endian reads. Every multi-byte field in this file goes
/// through these, so a truncated record returns `None` instead of panicking on
/// untrusted vendor bytes.
fn le_u32(buf: &[u8], at: usize) -> Option<u32> {
    let b = buf.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn le_f32(buf: &[u8], at: usize) -> Option<f32> {
    let b = buf.get(at..at.checked_add(4)?)?;
    Some(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

enum Variant {
    /// Binary, carrying the declared triangle count — already proven against the
    /// file length, so it is safe to size an allocation from.
    Binary(usize),
    Ascii,
}

fn detect(bytes: &[u8]) -> Result<Variant, MeshError> {
    let declared = le_u32(bytes, HEADER_LEN);
    // u64 throughout: a hostile header can claim 4 294 967 295 triangles, and
    // `84 + 50 * n` in usize arithmetic wraps on a 32-bit target into a number
    // that would match some perfectly short file.
    let needed = declared.map(|n| BINARY_PREFIX_LEN as u64 + u64::from(n) * TRI_RECORD_LEN as u64);
    if let (Some(n), Some(needed)) = (declared, needed) {
        if needed == bytes.len() as u64 {
            return Ok(Variant::Binary(n as usize));
        }
    }

    let has_solid = starts_with_solid_keyword(bytes);
    // A NUL byte never appears in a text STL. Its presence means the `solid` we
    // are looking at is the first word of a binary header rather than the ASCII
    // keyword, which is the case the length check above would have caught had
    // the file not also been truncated or padded.
    let is_text = !bytes.contains(&0);
    if has_solid && is_text {
        return Ok(Variant::Ascii);
    }

    let mut detail = match needed {
        Some(needed) => format!(
            "length {} is not 84 + 50 * count: the count field says {} triangle(s), which needs \
             {needed} bytes",
            bytes.len(),
            declared.unwrap_or_default(),
        ),
        None => format!("length {} is short of the 84-byte binary header", bytes.len()),
    };
    if has_solid {
        detail.push_str(
            ", and while it opens with `solid` it also contains NUL bytes, so that is binary \
             header text and not the ASCII keyword — most likely a truncated binary STL",
        );
    } else {
        detail.push_str(", and it does not open with the ASCII keyword `solid`");
    }
    Err(not_stl(detail))
}

fn starts_with_solid_keyword(bytes: &[u8]) -> bool {
    let start = match bytes.iter().position(|b| !b.is_ascii_whitespace()) {
        Some(i) => i,
        None => return false,
    };
    let rest = &bytes[start..];
    match rest.get(..5) {
        // The keyword has to end at a boundary, or an OBJ named `solidify` and
        // anything else starting with those five letters would match.
        Some(w) if w.eq_ignore_ascii_case(b"solid") => {
            rest.get(5).is_none_or(|b| b.is_ascii_whitespace())
        }
        _ => false,
    }
}

/// Reject NaN and infinity at the door.
///
/// Both are representable in a binary STL's floats, and neither has a sane
/// downstream behaviour: one NaN coordinate poisons `Mesh::bounds`, every facet
/// normal that touches it, and every slicer that opens the converted file. The
/// ASCII path checks as it parses instead, so it can name the offending line.
fn check_finite(tri: &[[f32; 3]; 3], which: usize) -> Result<(), MeshError> {
    for (v, p) in tri.iter().enumerate() {
        for (c, x) in p.iter().enumerate() {
            if !x.is_finite() {
                return Err(MeshError::Malformed(format!(
                    "triangle {which} vertex {v} has a non-finite {} coordinate ({x})",
                    ["x", "y", "z"][c],
                )));
            }
        }
    }
    Ok(())
}

/// Read either STL variant.
///
/// STL is a triangle SOUP: every shared corner is repeated once per facet and
/// there are no indices at all. `Mesh::from_soup` welds bit-identical corners
/// back into an indexed mesh here, so the IR invariant holds and no downstream
/// writer has to reinvent it.
///
/// `normals` is left `None` even though every facet in the file carries one.
/// STL stores one normal per FACET; the IR stores one per VERTEX, and a welded
/// vertex belongs to several facets, so there is no non-arbitrary value to put
/// there. Copying a facet's normal onto its three corners would produce normals
/// that contradict each other at every shared vertex and that no longer describe
/// the surface — worse than the honest `None`, which tells the writers that need
/// normals (STL itself, GLB if it wanted them) to recompute from the geometry.
pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    match detect(bytes)? {
        Variant::Binary(count) => read_binary(bytes, count),
        Variant::Ascii => read_ascii(bytes),
    }
}

fn read_binary(bytes: &[u8], count: usize) -> Result<Mesh, MeshError> {
    if count == 0 {
        return Err(MeshError::Empty);
    }
    // `detect` proved the file is exactly `84 + 50 * count` bytes, so the
    // declared count is bounded by bytes we are already holding. This is the
    // allocation a "4 billion triangles" header is aiming at, and it cannot be
    // reached without 200 GB of file behind it.
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::with_capacity(count);

    for i in 0..count {
        // Belt and braces: the length check already guarantees this slice
        // exists. Guarding anyway is what keeps the two from drifting into a
        // panic on the poll path if either ever changes.
        let rec = i
            .checked_mul(TRI_RECORD_LEN)
            .and_then(|o| o.checked_add(BINARY_PREFIX_LEN))
            .and_then(|o| bytes.get(o..))
            .and_then(|s| s.get(..TRI_RECORD_LEN))
            .ok_or_else(|| MeshError::Malformed(format!("triangle {i} record is truncated")))?;

        // Bytes 0..12 hold the stored facet normal and are skipped: see `read`
        // for why it cannot be carried into the IR, and `write` for why we
        // recompute rather than copy it on the way back out.
        let mut tri = [[0.0f32; 3]; 3];
        for (v, vertex) in tri.iter_mut().enumerate() {
            for (c, slot) in vertex.iter_mut().enumerate() {
                let at = 12 + v * 12 + c * 4;
                *slot = le_f32(rec, at).ok_or_else(|| {
                    MeshError::Malformed(format!("triangle {i} vertex {v} is truncated"))
                })?;
            }
        }
        // Bytes 48..50 are the "attribute byte count", ignored on purpose. Its
        // only real use is Materialise's per-facet colour extension, which the
        // IR has nowhere to put (its colour is one flat value for the whole
        // mesh) and which the lossiness table already declares dropped. It is
        // junk in most files besides, so refusing files with a non-zero value
        // would reject working meshes for no gain.
        check_finite(&tri, i)?;
        tris.push(tri);
    }

    let mut mesh = Mesh::from_soup(&tris);
    mesh.name = header_name(bytes);
    Ok(mesh)
}

/// The mesh name we stamped into the header, if this file is one of ours.
///
/// Only our own tag is honoured. The header is free-form, so treating arbitrary
/// header text as a name would import a slicer's advertising banner as the mesh
/// name for half the files in the wild.
fn header_name(bytes: &[u8]) -> Option<String> {
    let header = bytes.get(..HEADER_LEN)?;
    let rest = header.strip_prefix(HEADER_TAG.as_bytes())?;
    let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
    let text = std::str::from_utf8(rest.get(..end)?).ok()?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Where the ASCII parser is inside the `solid`/`facet`/`outer loop` nesting.
///
/// Tracked explicitly rather than inferred from the last keyword so that an
/// out-of-order file fails on the line that broke the structure, naming what was
/// expected — the alternative is a parser that quietly skips what it does not
/// recognise and returns a mesh missing whichever facets were malformed.
#[derive(Clone, Copy, PartialEq)]
enum St {
    Outside,
    InSolid,
    InFacet,
    InLoop,
    AfterLoop,
}

impl St {
    fn describe(self) -> &'static str {
        match self {
            St::Outside => "outside any solid",
            St::InSolid => "inside a solid",
            St::InFacet => "inside a facet, before `outer loop`",
            St::InLoop => "inside a vertex loop",
            St::AfterLoop => "after `endloop`, before `endfacet`",
        }
    }
}

fn read_ascii(bytes: &[u8]) -> Result<Mesh, MeshError> {
    // Strict UTF-8, not a lossy decode: the only place a non-ASCII byte can
    // legitimately appear is a solid's name, and silently substituting
    // replacement characters there is how a mangled name ends up written back
    // out as though the author had chosen it.
    let text = std::str::from_utf8(bytes).map_err(|e| {
        MeshError::Malformed(format!("ASCII STL is not valid UTF-8 (byte {})", e.valid_up_to()))
    })?;

    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    let mut corners: Vec<[f32; 3]> = Vec::with_capacity(3);
    let mut name: Option<String> = None;
    let mut state = St::Outside;

    for (n, raw) in text.lines().enumerate() {
        let line = n + 1;
        // `split_whitespace` gives the tolerance the format needs for free:
        // leading indentation of any depth, tabs, runs of spaces, and the `\r`
        // of a CRLF file (which `lines` has already removed anyway).
        let mut words = raw.split_whitespace();
        let word = match words.next() {
            Some(w) => w,
            None => continue,
        };
        // Keywords are matched case-insensitively. STL's specification is a 1988
        // appendix, and writers disagree: `FACET NORMAL`, `endSolid`, `Vertex`
        // all occur in files that load everywhere else.
        let kw = word.to_ascii_lowercase();

        match (kw.as_str(), state) {
            ("solid", St::Outside) => {
                state = St::InSolid;
                // Some exporters concatenate several solids into one file. That
                // is one mesh to us — the IR holds a single geometry — so they
                // accumulate, and the first name we see wins because there is
                // nowhere to put a second.
                if name.is_none() {
                    let rest = raw.trim().get(word.len()..).unwrap_or("").trim();
                    if !rest.is_empty() {
                        name = Some(rest.to_string());
                    }
                }
            }
            ("facet", St::InSolid) => {
                // The `normal i j k` on this line is read past without being
                // parsed. We discard facet normals regardless (see `read`), and
                // not parsing them buys tolerance of the `1.#QNAN` and `-1.#IND`
                // normals that older exporters emit beside perfectly good
                // vertices — a file we would otherwise reject for a field whose
                // value we were about to throw away.
                state = St::InFacet;
            }
            ("outer", St::InFacet) => {
                match words.next() {
                    Some(w) if w.eq_ignore_ascii_case("loop") => {}
                    other => {
                        return Err(MeshError::Malformed(format!(
                            "line {line}: expected `outer loop`, found `outer {}`",
                            other.unwrap_or("")
                        )))
                    }
                }
                corners.clear();
                state = St::InLoop;
            }
            ("vertex", St::InLoop) => {
                let coords: Vec<&str> = words.collect();
                if coords.len() != 3 {
                    return Err(MeshError::Malformed(format!(
                        "line {line}: `vertex` takes 3 coordinates, found {}",
                        coords.len()
                    )));
                }
                let mut p = [0.0f32; 3];
                for (i, c) in coords.iter().enumerate() {
                    let v: f32 = c.parse().map_err(|_| {
                        MeshError::Malformed(format!("line {line}: `{c}` is not a number"))
                    })?;
                    // Rust's float parser accepts `nan`, `inf` and `infinity`,
                    // so a text STL can carry them as readily as a binary one.
                    if !v.is_finite() {
                        return Err(MeshError::Malformed(format!(
                            "line {line}: `{c}` is not a finite number"
                        )));
                    }
                    p[i] = v;
                }
                corners.push(p);
            }
            ("endloop", St::InLoop) => {
                match corners.len() {
                    3 => {}
                    more if more > 3 => {
                        return Err(MeshError::Unsupported(format!(
                            "line {line}: facet loop has {more} vertices; STL is triangles only \
                             and this module does not triangulate — fan-triangulating a polygon \
                             we cannot prove is planar and convex invents geometry"
                        )))
                    }
                    few => {
                        return Err(MeshError::Malformed(format!(
                            "line {line}: facet loop has {few} vertices, not 3"
                        )))
                    }
                }
                let tri = [corners[0], corners[1], corners[2]];
                tris.push(tri);
                state = St::AfterLoop;
            }
            ("endfacet", St::AfterLoop) => state = St::InSolid,
            ("endsolid", St::InSolid) => state = St::Outside,
            (other, st) => {
                return Err(MeshError::Malformed(format!(
                    "line {line}: unexpected `{other}` while {}",
                    st.describe()
                )))
            }
        }
    }

    if state != St::Outside {
        // A missing `endsolid` is common enough that tolerating it is tempting,
        // and it is exactly the wrong call: a file truncated at a facet boundary
        // is indistinguishable from one that merely forgot its last line, so
        // accepting it means returning a partial mesh and calling it whole.
        return Err(MeshError::Malformed(format!(
            "file ends while {} — truncated, or missing its `endsolid`",
            state.describe()
        )));
    }
    if tris.is_empty() {
        return Err(MeshError::Empty);
    }

    let mut mesh = Mesh::from_soup(&tris);
    mesh.name = name;
    Ok(mesh)
}

/// Write a binary STL. See the module docs for why never ASCII.
pub fn write(mesh: &Mesh) -> Result<Vec<u8>, MeshError> {
    // `to_soup` indexes `positions` unchecked, and this function is public: a
    // caller reaching it directly rather than through `super::write` (which
    // validates first) must get an error, not a panic.
    mesh.validate()?;

    let tris = mesh.to_soup();
    if tris.is_empty() {
        return Err(MeshError::Empty);
    }
    let count = u32::try_from(tris.len()).map_err(|_| {
        MeshError::Unsupported(format!(
            "{} triangles overflows the u32 count field a binary STL has room for",
            tris.len()
        ))
    })?;

    let mut out = Vec::with_capacity(BINARY_PREFIX_LEN + tris.len() * TRI_RECORD_LEN);
    out.extend_from_slice(&header(mesh.name.as_deref()));
    out.extend_from_slice(&count.to_le_bytes());

    for (i, tri) in tris.iter().enumerate() {
        check_finite(tri, i)?;
        // Computed from the winding rather than written as zero. Zero is legal
        // and common, and consumers split on it: those that derive the normal
        // from the winding do not care, but the ones that trust the stored value
        // (several slicers and CAD importers) shade the model inside-out.
        // Writing the real normal satisfies both readings. A degenerate
        // zero-area triangle still gets zero — there is no direction to invent,
        // and its winding is equally undefined.
        let normal = Mesh::facet_normal(tri);
        for c in normal {
            out.extend_from_slice(&c.to_le_bytes());
        }
        for v in tri {
            for c in v {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        // Attribute byte count: always zero. The field's only real use is
        // Materialise's per-facet colour, which the IR cannot express and which
        // the lossiness table already declares dropped — and a non-zero value
        // here is read as colour by the tools that understand it, so writing
        // anything else would be inventing one.
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    Ok(out)
}

fn header(name: Option<&str>) -> [u8; HEADER_LEN] {
    let mut header = [0u8; HEADER_LEN];
    let mut text = String::from(HEADER_TAG);
    // The name rides in the free-form header so `stl -> stl` keeps it, but only
    // when it survives verbatim: printable ASCII, short enough to fit. A
    // truncated or transliterated name is worse than none, because it reads as
    // deliberate to whoever opens the file next.
    if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
        let fits = n.len() <= HEADER_LEN - HEADER_TAG.len();
        let printable = n.bytes().all(|b| b == b' ' || b.is_ascii_graphic());
        if fits && printable {
            text.push_str(n);
        }
    }
    let b = text.as_bytes();
    let take = b.len().min(HEADER_LEN);
    header[..take].copy_from_slice(&b[..take]);
    // The rest stays NUL: readers that print the header should stop at the end
    // of the text, and the NULs double as the "this is not text" evidence
    // `detect` leans on.
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two triangles sharing an edge: enough for welding to be observable (6
    /// soup corners, 4 distinct positions) without being a wall of numbers.
    fn quad() -> Mesh {
        Mesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 3.0, 0.0],
                [0.0, 3.0, 0.0],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..Default::default()
        }
    }

    /// Build a binary STL by hand, so tests can lie about the header, the count,
    /// the stored normal and the attribute word in ways our writer never would.
    fn binary(
        header_text: &[u8],
        count: u32,
        tris: &[[[f32; 3]; 3]],
        stored_normal: [f32; 3],
        attr: u16,
    ) -> Vec<u8> {
        let mut out = vec![0u8; HEADER_LEN];
        let take = header_text.len().min(HEADER_LEN);
        out[..take].copy_from_slice(&header_text[..take]);
        out.extend_from_slice(&count.to_le_bytes());
        for t in tris {
            for c in stored_normal {
                out.extend_from_slice(&c.to_le_bytes());
            }
            for v in t {
                for c in v {
                    out.extend_from_slice(&c.to_le_bytes());
                }
            }
            out.extend_from_slice(&attr.to_le_bytes());
        }
        out
    }

    fn ascii(name: &str, tris: &[[[f32; 3]; 3]]) -> String {
        let mut s = format!("solid {name}\n");
        for t in tris {
            s.push_str("  facet normal 0.0 0.0 1.0\n    outer loop\n");
            for v in t {
                s.push_str(&format!("      vertex {} {} {}\n", v[0], v[1], v[2]));
            }
            s.push_str("    endloop\n  endfacet\n");
        }
        s.push_str(&format!("endsolid {name}\n"));
        s
    }

    fn soup() -> Vec<[[f32; 3]; 3]> {
        quad().to_soup()
    }

    // ---- round trips ----

    #[test]
    fn binary_round_trip_preserves_geometry_and_welds_the_soup() {
        let src = quad();
        let bytes = write(&src).unwrap();
        assert_eq!(bytes.len(), BINARY_PREFIX_LEN + 2 * TRI_RECORD_LEN);

        let back = read(&bytes).unwrap();
        assert_eq!(back.triangle_count(), 2);
        assert_eq!(back.positions.len(), 4, "shared corners must weld: {:?}", back.positions);
        assert_eq!(back.bounds(), src.bounds());
        assert_eq!(back.to_soup(), src.to_soup());
    }

    #[test]
    fn ascii_and_binary_of_the_same_mesh_read_identically() {
        let src = quad();
        let from_binary = read(&write(&src).unwrap()).unwrap();
        let from_ascii = read(ascii("q", &soup()).as_bytes()).unwrap();
        assert_eq!(from_ascii.to_soup(), from_binary.to_soup());
        assert_eq!(from_ascii.positions, from_binary.positions);
    }

    #[test]
    fn a_declared_count_of_zero_is_empty_not_an_empty_mesh() {
        let bytes = binary(b"litegen stl ", 0, &[], [0.0; 3], 0);
        assert_eq!(bytes.len(), BINARY_PREFIX_LEN);
        assert_eq!(read(&bytes), Err(MeshError::Empty));
    }

    // ---- variant detection: the bug this module exists to avoid ----

    #[test]
    fn a_binary_file_whose_header_starts_with_solid_is_still_read_as_binary() {
        // The whole point of the length check. A prefix sniff sends this file to
        // the ASCII parser and it comes back either as an error or as garbage.
        let tris = soup();
        let bytes = binary(b"solid ACME Exporter v1.2 -- binary output", 2, &tris, [0.0, 0.0, 1.0], 0);
        assert!(starts_with_solid_keyword(&bytes), "fixture must trip the naive test");

        let mesh = read(&bytes).unwrap();
        assert_eq!(mesh.triangle_count(), 2);
        assert_eq!(mesh.to_soup(), tris);
        assert_eq!(mesh.name, None, "a foreign header is not a name");
    }

    #[test]
    fn an_ascii_file_can_never_satisfy_the_binary_length_equation() {
        // Pad an ASCII file until its length is exactly 84 + 50n. It still reads
        // as ASCII, because byte 83 of any text file is not NUL and the count
        // field would have to be small enough to make it one.
        let mut text = ascii("padded", &soup());
        let over = text.len().saturating_sub(BINARY_PREFIX_LEN);
        text.push_str(&" ".repeat((TRI_RECORD_LEN - over % TRI_RECORD_LEN) % TRI_RECORD_LEN));
        let n = (text.len() - BINARY_PREFIX_LEN) / TRI_RECORD_LEN;
        assert_eq!(BINARY_PREFIX_LEN + n * TRI_RECORD_LEN, text.len(), "fixture must sit on 84 + 50n");
        let mesh = read(text.as_bytes()).unwrap();
        assert_eq!(mesh.triangle_count(), 2);
        assert_eq!(mesh.name.as_deref(), Some("padded"));
    }

    #[test]
    fn the_solid_keyword_must_end_at_a_boundary() {
        assert!(starts_with_solid_keyword(b"solid cube\n"));
        assert!(starts_with_solid_keyword(b"\n\t  SOLID\n"));
        assert!(starts_with_solid_keyword(b"solid"));
        assert!(!starts_with_solid_keyword(b"solidify me\n"));
        assert!(!starts_with_solid_keyword(b"# an obj\nv 0 0 0\n"));
        assert!(!starts_with_solid_keyword(b""));
    }

    // ---- ASCII tolerance ----

    #[test]
    fn ascii_tolerates_case_indentation_tabs_and_crlf() {
        let text = "SOLID Weird\r\n\
             \tFACET NORMAL 0 0 1\r\n\
             \t\touter   LOOP\r\n\
             \t\t\tVertex 0 0 0\r\n\
             \t\t\tvertex   2   0 0\r\n\
             \t\t\tvertex 2 3 0\r\n\
             \t\tENDLOOP\r\n\
             \tEndFacet\r\n\
             EndSolid Weird\r\n";
        let mesh = read(text.as_bytes()).unwrap();
        assert_eq!(mesh.triangle_count(), 1);
        assert_eq!(mesh.name.as_deref(), Some("Weird"));
        assert_eq!(mesh.to_soup(), vec![[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 3.0, 0.0]]]);
    }

    #[test]
    fn ascii_accepts_exponent_and_negative_zero_coordinates() {
        let text = "solid e\n\
             facet normal 0 0 0\n\
             outer loop\n\
             vertex -0.0 1e2 0\n\
             vertex 1.5E-1 0 0\n\
             vertex 0 0 +3\n\
             endloop\n\
             endfacet\n\
             endsolid\n";
        let mesh = read(text.as_bytes()).unwrap();
        assert_eq!(mesh.triangle_count(), 1);
        let (min, max) = mesh.bounds().unwrap();
        assert_eq!(min, [-0.0, 0.0, 0.0]);
        assert_eq!(max, [0.15, 100.0, 3.0]);
    }

    #[test]
    fn ascii_facet_normals_are_not_parsed_so_a_broken_one_does_not_reject_the_file() {
        // Old exporters emit `1.#QNAN` normals beside good vertices. Rejecting
        // the file over a field we are about to discard helps nobody.
        let text = "solid q\n\
             facet normal 1.#QNAN 1.#QNAN 1.#QNAN\n\
             outer loop\n\
             vertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\n\
             endloop\nendfacet\nendsolid q\n";
        assert_eq!(read(text.as_bytes()).unwrap().triangle_count(), 1);
    }

    #[test]
    fn ascii_accepts_a_bare_facet_keyword() {
        let text = "solid q\nfacet\nouter loop\n\
             vertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\n\
             endloop\nendfacet\nendsolid\n";
        assert_eq!(read(text.as_bytes()).unwrap().triangle_count(), 1);
    }

    #[test]
    fn ascii_without_a_name_leaves_the_name_unset() {
        let text = ascii("", &soup());
        assert_eq!(read(text.as_bytes()).unwrap().name, None);
    }

    #[test]
    fn ascii_concatenated_solids_accumulate_into_one_mesh() {
        let tris = soup();
        let text = format!("{}{}", ascii("first", &tris[..1]), ascii("second", &tris[1..]));
        let mesh = read(text.as_bytes()).unwrap();
        assert_eq!(mesh.triangle_count(), 2);
        assert_eq!(mesh.name.as_deref(), Some("first"), "the first name wins");
        assert_eq!(mesh.to_soup(), tris);
    }

    // ---- attributes: what is dropped and what is kept ----

    #[test]
    fn read_leaves_every_attribute_stl_cannot_carry_unset() {
        let mesh = read(&write(&quad()).unwrap()).unwrap();
        assert!(mesh.normals.is_none(), "facet normals must not become vertex normals");
        assert!(mesh.uvs.is_none());
        assert!(mesh.base_color.is_none());
    }

    #[test]
    fn read_ignores_the_stored_facet_normal_entirely() {
        // A file whose stored normals point the wrong way (or nowhere) still
        // yields the same geometry — the winding is the only source of truth.
        let tris = soup();
        let lying = binary(b"x", 2, &tris, [0.0, 0.0, -1.0], 0);
        let zeroed = binary(b"x", 2, &tris, [0.0, 0.0, 0.0], 0);
        assert_eq!(read(&lying).unwrap().to_soup(), tris);
        assert_eq!(read(&zeroed).unwrap().to_soup(), tris);
    }

    #[test]
    fn read_ignores_a_non_zero_attribute_word() {
        // Magics-style per-facet colour. We drop the colour (the table says so)
        // but the file is not rejected over it.
        let tris = soup();
        let coloured = binary(b"x", 2, &tris, [0.0, 0.0, 1.0], 0x8421);
        let mesh = read(&coloured).unwrap();
        assert_eq!(mesh.triangle_count(), 2);
        assert!(mesh.base_color.is_none());
    }

    #[test]
    fn write_stores_the_winding_normal_rather_than_zero() {
        let mesh = Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        let bytes = write(&mesh).unwrap();
        let n = [
            le_f32(&bytes, BINARY_PREFIX_LEN).unwrap(),
            le_f32(&bytes, BINARY_PREFIX_LEN + 4).unwrap(),
            le_f32(&bytes, BINARY_PREFIX_LEN + 8).unwrap(),
        ];
        assert!((n[2] - 1.0).abs() < 1e-6, "{n:?}");

        // Reversing the winding must reverse the stored normal, or the viewers
        // that trust it render the model inside-out.
        let flipped = Mesh { indices: vec![0, 2, 1], ..mesh };
        let bytes = write(&flipped).unwrap();
        assert!((le_f32(&bytes, BINARY_PREFIX_LEN + 8).unwrap() + 1.0).abs() < 1e-6);
    }

    #[test]
    fn write_sets_every_attribute_word_to_zero() {
        let bytes = write(&quad()).unwrap();
        for i in 0..2 {
            let at = BINARY_PREFIX_LEN + i * TRI_RECORD_LEN + 48;
            assert_eq!(&bytes[at..at + 2], &[0, 0], "triangle {i}");
        }
    }

    // ---- the header we write ----

    #[test]
    fn the_header_is_tagged_and_never_starts_with_solid() {
        let bytes = write(&quad()).unwrap();
        assert!(bytes.starts_with(HEADER_TAG.as_bytes()));
        assert!(
            !starts_with_solid_keyword(&bytes),
            "our own header must not trip a naive reader's prefix sniff",
        );
        assert_eq!(&bytes[HEADER_TAG.len()..HEADER_LEN], &[0u8; HEADER_LEN - HEADER_TAG.len()]);
    }

    #[test]
    fn an_ascii_name_round_trips_through_the_header() {
        let named = Mesh { name: Some("litegen_matrix_cube".into()), ..quad() };
        let back = read(&write(&named).unwrap()).unwrap();
        assert_eq!(back.name.as_deref(), Some("litegen_matrix_cube"));

        // And through ASCII in, binary out — the path a vendor file takes.
        let from_ascii = read(ascii("vendor mesh", &soup()).as_bytes()).unwrap();
        assert_eq!(from_ascii.name.as_deref(), Some("vendor mesh"));
        assert_eq!(read(&write(&from_ascii).unwrap()).unwrap().name.as_deref(), Some("vendor mesh"));
    }

    #[test]
    fn a_name_that_cannot_survive_the_header_is_dropped_not_mangled() {
        for bad in ["café au lait", &"x".repeat(HEADER_LEN), "line\nbreak"] {
            let m = Mesh { name: Some(bad.to_string()), ..quad() };
            let bytes = write(&m).unwrap();
            assert!(bytes.starts_with(HEADER_TAG.as_bytes()));
            assert_eq!(read(&bytes).unwrap().name, None, "{bad:?} must be dropped whole");
        }
    }

    #[test]
    fn a_foreign_header_is_never_mistaken_for_a_name() {
        let bytes = binary(b"Exported by SomeCAD 9000 (trial version)", 2, &soup(), [0.0; 3], 0);
        assert_eq!(read(&bytes).unwrap().name, None);
    }

    // ---- error arms ----

    fn expect_not_this_format(bytes: &[u8], ctx: &str) {
        match read(bytes) {
            Err(MeshError::NotThisFormat { expected, detail }) => {
                assert_eq!(expected, super::super::MeshFormat::Stl);
                assert!(!detail.is_empty(), "{ctx}: the detail must say what failed");
            }
            other => panic!("{ctx}: expected NotThisFormat, got {other:?}"),
        }
    }

    #[test]
    fn bytes_that_are_not_stl_at_all_are_not_this_format() {
        expect_not_this_format(b"", "empty");
        expect_not_this_format(&[0u8; 3], "three NULs");
        expect_not_this_format(b"# an obj file\nv 0 0 0\nf 1 1 1\n", "obj");
        expect_not_this_format(b"ply\nformat ascii 1.0\nend_header\n", "ply");
        expect_not_this_format(&[0xffu8; 256], "noise");
        expect_not_this_format(b"solidify: not a keyword", "near-miss keyword");
    }

    #[test]
    fn a_hostile_triangle_count_is_refused_before_anything_is_allocated() {
        // 84 bytes claiming 4 294 967 295 triangles — 200 GB of file that is not
        // there. The length check catches it without touching a Vec.
        let bytes = binary(b"hostile", u32::MAX, &[], [0.0; 3], 0);
        assert_eq!(bytes.len(), BINARY_PREFIX_LEN);
        match read(&bytes) {
            Err(MeshError::NotThisFormat { detail, .. }) => {
                assert!(detail.contains("4294967295"), "{detail}");
            }
            other => panic!("expected NotThisFormat, got {other:?}"),
        }
    }

    #[test]
    fn a_truncated_or_padded_binary_file_is_refused() {
        let full = write(&quad()).unwrap();
        expect_not_this_format(&full[..full.len() - 1], "one byte short");
        expect_not_this_format(&full[..BINARY_PREFIX_LEN + 10], "cut mid-record");

        let mut padded = full.clone();
        padded.push(0);
        expect_not_this_format(&padded, "trailing junk");
    }

    #[test]
    fn a_truncated_binary_file_whose_header_says_solid_explains_itself() {
        let full = binary(b"solid ACME binary", 2, &soup(), [0.0; 3], 0);
        match read(&full[..full.len() - 5]) {
            Err(MeshError::NotThisFormat { detail, .. }) => {
                assert!(detail.contains("NUL"), "{detail}");
            }
            other => panic!("expected NotThisFormat, got {other:?}"),
        }
    }

    #[test]
    fn a_non_finite_binary_coordinate_is_malformed() {
        for bad in [f32::NAN, f32::INFINITY] {
            let tris = vec![[[0.0, 0.0, 0.0], [1.0, bad, 0.0], [0.0, 1.0, 0.0]]];
            let bytes = binary(b"x", 1, &tris, [0.0; 3], 0);
            assert!(
                matches!(read(&bytes), Err(MeshError::Malformed(_))),
                "{bad} must be refused",
            );
        }
    }

    #[test]
    fn ascii_numbers_that_are_not_finite_numbers_are_malformed() {
        for bad in ["nan", "inf", "-infinity", "1,5", "seven", "1.0.0"] {
            let text = format!(
                "solid q\nfacet normal 0 0 1\nouter loop\n\
                 vertex 0 0 0\nvertex {bad} 0 0\nvertex 0 1 0\n\
                 endloop\nendfacet\nendsolid q\n"
            );
            match read(text.as_bytes()) {
                Err(MeshError::Malformed(d)) => assert!(d.contains("line 5"), "{bad}: {d}"),
                other => panic!("{bad}: expected Malformed, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_ascii_vertex_with_the_wrong_coordinate_count_is_malformed() {
        for line in ["vertex 0 0", "vertex 0 0 0 0", "vertex"] {
            let text = format!(
                "solid q\nfacet normal 0 0 1\nouter loop\n\
                 {line}\nvertex 1 0 0\nvertex 0 1 0\n\
                 endloop\nendfacet\nendsolid q\n"
            );
            assert!(
                matches!(read(text.as_bytes()), Err(MeshError::Malformed(_))),
                "{line:?} must be refused",
            );
        }
    }

    #[test]
    fn an_ascii_loop_with_more_than_three_vertices_is_unsupported() {
        let text = "solid q\nfacet normal 0 0 1\nouter loop\n\
             vertex 0 0 0\nvertex 1 0 0\nvertex 1 1 0\nvertex 0 1 0\n\
             endloop\nendfacet\nendsolid q\n";
        match read(text.as_bytes()) {
            Err(MeshError::Unsupported(d)) => assert!(d.contains('4'), "{d}"),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn an_ascii_loop_with_fewer_than_three_vertices_is_malformed() {
        let text = "solid q\nfacet normal 0 0 1\nouter loop\n\
             vertex 0 0 0\nvertex 1 0 0\n\
             endloop\nendfacet\nendsolid q\n";
        assert!(matches!(read(text.as_bytes()), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn ascii_keywords_out_of_order_are_malformed() {
        // Each of these is a structurally impossible file: a parser that skipped
        // what it did not expect would return a mesh missing facets instead.
        let cases: [&str; 4] = [
            "solid q\nvertex 0 0 0\nendsolid q\n",
            "solid q\nfacet normal 0 0 1\nvertex 0 0 0\nendsolid q\n",
            "solid q\nfacet normal 0 0 1\nouter\nendsolid q\n",
            "solid q\nsolid nested\nendsolid nested\nendsolid q\n",
        ];
        for text in cases {
            assert!(
                matches!(read(text.as_bytes()), Err(MeshError::Malformed(_))),
                "{text:?} must be refused",
            );
        }
    }

    #[test]
    fn an_unknown_ascii_keyword_is_malformed() {
        let text = "solid q\ncolor 1 0 0\nendsolid q\n";
        match read(text.as_bytes()) {
            Err(MeshError::Malformed(d)) => assert!(d.contains("color"), "{d}"),
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn an_ascii_file_truncated_mid_facet_or_missing_endsolid_is_malformed() {
        let whole = ascii("q", &soup());
        // Cut after a complete `endfacet`: the geometry is self-consistent, only
        // `endsolid` is gone. Accepting it would mean accepting a half-written
        // file as a whole mesh.
        let at = whole.find("endsolid").unwrap();
        assert!(matches!(read(&whole.as_bytes()[..at]), Err(MeshError::Malformed(_))));

        let mid = whole.find("endloop").unwrap();
        assert!(matches!(read(&whole.as_bytes()[..mid]), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn an_ascii_solid_with_no_facets_is_empty() {
        assert_eq!(read(b"solid nothing\nendsolid nothing\n"), Err(MeshError::Empty));
        assert_eq!(read(b"  solid\nendsolid\n"), Err(MeshError::Empty));
    }

    #[test]
    fn an_ascii_file_that_is_not_utf8_is_malformed() {
        // Passes the sniff (starts with `solid`, no NUL) and then fails to
        // decode: a Latin-1 name, most likely.
        let bytes = b"solid caf\xe9\nendsolid\n";
        match read(bytes) {
            Err(MeshError::Malformed(d)) => assert!(d.contains("UTF-8"), "{d}"),
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn write_refuses_an_empty_mesh() {
        assert_eq!(write(&Mesh::default()), Err(MeshError::Empty));
    }

    #[test]
    fn write_refuses_an_invalid_mesh_rather_than_panicking() {
        // `to_soup` would index out of bounds on both of these.
        let bad_index = Mesh { indices: vec![0, 1, 99], ..quad() };
        assert!(matches!(write(&bad_index), Err(MeshError::Malformed(_))));

        let partial = Mesh { indices: vec![0, 1], ..quad() };
        assert!(matches!(write(&partial), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn write_refuses_a_non_finite_position() {
        let m = Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, f32::NAN, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        assert!(matches!(write(&m), Err(MeshError::Malformed(_))));
    }

    // ---- hostile input must return, never panic ----

    #[test]
    fn no_prefix_or_corruption_of_a_valid_file_panics() {
        let binary_file = write(&quad()).unwrap();
        let ascii_file = ascii("q", &soup()).into_bytes();

        for full in [binary_file, ascii_file] {
            for cut in 0..=full.len() {
                let _ = read(&full[..cut]);
            }
            for i in 0..full.len() {
                let mut corrupt = full.clone();
                corrupt[i] = corrupt[i].wrapping_add(0x7f);
                let _ = read(&corrupt);
                let mut zeroed = full.clone();
                zeroed[i] = 0;
                let _ = read(&zeroed);
            }
        }
    }
}
