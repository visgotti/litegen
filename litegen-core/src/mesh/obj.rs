//! Wavefront OBJ — the plain-text container.
//!
//! OBJ is the format everyone can open and nobody implements the same way, so
//! this reader is deliberately narrow: the polygonal subset (`v`, `vt`, `vn`,
//! `f`) and nothing else. What it does accept it accepts exactly — negative
//! indices, every `v`/`v/vt`/`v//vn`/`v/vt/vn` spelling and n-gons all turn up
//! in real vendor output, and a reader that quietly mishandles any of them
//! produces a file that looks fine until someone textures it.
//!
//! # The one structural thing OBJ does differently
//!
//! OBJ keeps three *independent* index spaces — positions, texture coordinates,
//! normals — and a face corner names one entry from each. Our IR, like a GPU
//! vertex buffer and like glTF, is per-vertex: one index selects a position and
//! its uv and its normal together. Bridging the two is the whole job of
//! [`read`], and doing it by the position index alone is the classic wrong
//! answer — see the comment on the corner cache.

use std::collections::HashMap;
use std::fmt;

use super::ir::Mesh;
use super::MeshError;
use super::MeshFormat;

/// How far into the file we look for evidence that these bytes are not text at
/// all. A real OBJ's first kilobyte is a comment banner and some `v` lines; a
/// mislabelled GLB or binary PLY has structure there we can recognise.
const SNIFF_BYTES: usize = 1024;

fn not_obj(detail: impl Into<String>) -> MeshError {
    MeshError::NotThisFormat { expected: MeshFormat::Obj, detail: detail.into() }
}

/// Reject bytes that are plainly another container before we try to decode them
/// as text.
///
/// OBJ has no magic number, so "is this an OBJ?" can only ever be answered by
/// what the bytes are *not*. Without this, handing `read` a GLB reports a UTF-8
/// error deep in the binary buffer — technically true, useless to the caller,
/// and the wrong variant: the caller's mistake was the format tag, and
/// `NotThisFormat` is the only error that says so.
fn sniff(bytes: &[u8]) -> Result<(), MeshError> {
    if bytes.is_empty() {
        return Err(not_obj("empty input"));
    }
    if bytes.starts_with(b"glTF") {
        return Err(not_obj("starts with the GLB magic `glTF`"));
    }
    // `ply` alone is a plausible OBJ token prefix; the newline right after it is
    // what makes it a PLY header line.
    if bytes.starts_with(b"ply") && matches!(bytes.get(3), Some(b'\r' | b'\n')) {
        return Err(not_obj("starts with the PLY magic `ply`"));
    }
    let head = &bytes[..bytes.len().min(SNIFF_BYTES)];
    if let Some(off) = head.iter().position(|b| *b == 0) {
        return Err(not_obj(format!("NUL byte at offset {off}; OBJ is a text format")));
    }
    Ok(())
}

/// Is this token a keyword the OBJ spec defines?
///
/// Used only as evidence of the format, never to decide behaviour — the match in
/// [`read`] does that. It is deliberately the full spec list, including the
/// records this module refuses: a file full of NURBS patches is still an OBJ,
/// and `Unsupported` is the honest answer for it rather than `NotThisFormat`.
fn is_obj_keyword(kw: &str) -> bool {
    matches!(
        kw,
        // geometry
        "v" | "vn" | "vt" | "vp" | "f" | "l" | "p"
        // grouping, shading, materials, display state
        | "o" | "g" | "s" | "mg" | "mtllib" | "usemtl" | "maplib" | "usemap"
        | "bevel" | "c_interp" | "d_interp" | "lod" | "shadow_obj" | "trace_obj"
        | "ctech" | "stech"
        // free-form surfaces
        | "cstype" | "deg" | "bmat" | "step" | "curv" | "curv2" | "surf" | "parm"
        | "trim" | "hole" | "scrv" | "sp" | "con" | "end"
    )
}

fn malformed(lineno: usize, detail: impl fmt::Display) -> MeshError {
    MeshError::Malformed(format!("obj line {lineno}: {detail}"))
}

fn unsupported(lineno: usize, detail: impl fmt::Display) -> MeshError {
    MeshError::Unsupported(format!("obj line {lineno}: {detail}"))
}

/// Parse one numeric token, rejecting the non-finite spellings Rust's `f32`
/// parser accepts.
///
/// `"nan"`, `"inf"` and `"-Infinity"` all parse successfully into values that
/// no consumer of the IR can do anything with: they poison bounds, they make
/// facet normals NaN, and STL/PLY would happily write them back out as
/// geometry. Rejecting them here is the only place the whole pipeline gets to
/// see the offending line number.
fn number(tok: &str, lineno: usize, what: &str) -> Result<f32, MeshError> {
    let v: f32 = tok
        .parse()
        .map_err(|_| malformed(lineno, format_args!("{what} `{tok}` is not a number")))?;
    if !v.is_finite() {
        return Err(malformed(lineno, format_args!("{what} `{tok}` is not a finite number")));
    }
    Ok(v)
}

fn required<'a>(
    tokens: &mut impl Iterator<Item = &'a str>,
    lineno: usize,
    what: &str,
) -> Result<&'a str, MeshError> {
    tokens
        .next()
        .ok_or_else(|| malformed(lineno, format_args!("missing {what}")))
}

/// The three components every `v` and `vn` line starts with. Takes only
/// `&'static str` labels: this runs once per vertex line of a file that may be
/// 128 MB, so it must not allocate a message it will not use.
fn xyz<'a>(
    tokens: &mut impl Iterator<Item = &'a str>,
    lineno: usize,
) -> Result<[f32; 3], MeshError> {
    let x = number(required(tokens, lineno, "the x component")?, lineno, "x")?;
    let y = number(required(tokens, lineno, "the y component")?, lineno, "y")?;
    let z = number(required(tokens, lineno, "the z component")?, lineno, "z")?;
    Ok([x, y, z])
}

/// One face corner, in OBJ's own terms: an index into each of the three spaces,
/// already resolved to 0-based and range-checked.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Corner {
    v: u32,
    vt: Option<u32>,
    vn: Option<u32>,
}

/// Turn one OBJ index field into a 0-based index into a list of `count` entries.
///
/// OBJ indices are 1-based, and negative ones count back from the end of the
/// list *as it stands at this line* — which is why everything here is resolved
/// during the single forward pass rather than after the file is read. Files
/// that interleave `v` blocks with `f` blocks (one block per object, very
/// common) depend on exactly that reading: their positive indices are absolute
/// from the top of the file while their negative ones are local to the block.
fn resolve(field: &str, count: usize, lineno: usize, what: &str) -> Result<u32, MeshError> {
    if field.is_empty() {
        return Err(malformed(lineno, format_args!("face corner has an empty {what} index")));
    }
    // i64, not usize: the value may legitimately be negative, and a hostile
    // 30-digit index has to fail as a parse error rather than wrap into a
    // plausible one.
    let raw: i64 = field
        .parse()
        .map_err(|_| malformed(lineno, format_args!("{what} index `{field}` is not an integer")))?;
    let count = i64::try_from(count)
        .map_err(|_| malformed(lineno, format_args!("{what} list is too long to index")))?;
    let zero_based = if raw > 0 {
        raw - 1
    } else if raw < 0 {
        count + raw
    } else {
        return Err(malformed(lineno, format_args!("{what} index 0; OBJ indices are 1-based")));
    };
    if zero_based < 0 || zero_based >= count {
        return Err(malformed(
            lineno,
            format_args!("{what} index {raw} is out of range for {count} entries"),
        ));
    }
    u32::try_from(zero_based)
        .map_err(|_| malformed(lineno, format_args!("{what} index {raw} exceeds u32")))
}

fn parse_corner(
    tok: &str,
    lineno: usize,
    nv: usize,
    nvt: usize,
    nvn: usize,
) -> Result<Corner, MeshError> {
    let mut fields = tok.split('/');
    // `unwrap_or("")` cannot trigger — `split` always yields once — but an empty
    // first field is a real malformed input (`/2/3`) and `resolve` names it.
    let v = resolve(fields.next().unwrap_or(""), nv, lineno, "vertex")?;
    // An omitted middle field (`1//3`) is the documented way to say "no texture
    // coordinate", so empty means absent, not malformed.
    let vt = match fields.next() {
        Some("") | None => None,
        Some(f) => Some(resolve(f, nvt, lineno, "texture")?),
    };
    let vn = match fields.next() {
        Some("") | None => None,
        Some(f) => Some(resolve(f, nvn, lineno, "normal")?),
    };
    if fields.next().is_some() {
        return Err(malformed(
            lineno,
            format_args!("face corner `{tok}` has more than three `/`-separated fields"),
        ));
    }
    Ok(Corner { v, vt, vn })
}

pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    sniff(bytes)?;
    let text = std::str::from_utf8(bytes).map_err(|e| {
        let valid = e.valid_up_to();
        // Report the line, like every other error here, so a caller staring at a
        // 200 MB file has somewhere to look; the byte offset alone is not that.
        let lineno = 1 + bytes[..valid].iter().filter(|b| **b == b'\n').count();
        malformed(lineno, format_args!("input is not UTF-8 (first bad byte at offset {valid})"))
    })?;

    // OBJ's three index spaces, in file order. These are the *source* arrays;
    // the IR arrays below are built from face corners, not from these.
    let mut src_v: Vec<[f32; 3]> = Vec::new();
    let mut src_vt: Vec<[f32; 2]> = Vec::new();
    let mut src_vn: Vec<[f32; 3]> = Vec::new();

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    // The corner cache, and the reason this reader exists.
    //
    // Reading an OBJ by its `v` index alone — the naive shortcut — is wrong in
    // exactly the cases that matter. At a UV seam the same position appears with
    // two different `vt` indices, and keying on `v` lets one of them win, which
    // smears the texture across the seam. At a hard edge the same position
    // appears with two different `vn` indices, and keying on `v` welds them into
    // one smooth normal, which rounds off the crease the author modelled. Both
    // survive validation and both look like a rendering bug later. Keying on the
    // full triple splits those corners into separate IR vertices, which is what
    // a per-vertex representation means.
    let mut cache: HashMap<Corner, u32> = HashMap::new();
    // Fixed by the first corner in the file: the IR carries uvs and normals for
    // all vertices or for none, so the file has to be consistent.
    let mut layout: Option<(bool, bool)> = None;
    let mut name: Option<String> = None;
    // Evidence that these bytes really are an OBJ: at least one line whose
    // keyword belongs to the format. Geometry is NOT the test — a file of
    // comments and an `o` line is a valid, empty OBJ, and the module contract
    // puts `Empty` in `mesh::read`'s hands, not here. What this catches is the
    // other direction: text that is some other format (an ASCII STL, a README)
    // handed over with the wrong tag, where every line is an unknown keyword.
    let mut saw_record = false;
    let mut line_count = 0usize;
    // Scratch, cleared per face rather than reallocated: `f` lines outnumber
    // every other line in the file.
    let mut corners: Vec<Corner> = Vec::new();
    let mut slots: Vec<u32> = Vec::new();

    for (i, raw) in text.lines().enumerate() {
        let lineno = i + 1;
        line_count = lineno;
        // `#` starts a comment anywhere on the line, not only in column 1.
        let body = match raw.find('#') {
            Some(c) => &raw[..c],
            None => raw,
        };
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        // A trailing backslash continues the statement onto the next line. It is
        // legal OBJ and we do not join lines, so say so instead of failing later
        // with a confusing "`\` is not a number" on a line that reads fine.
        if body.ends_with('\\') {
            return Err(unsupported(lineno, "line continuation (`\\`) is not supported"));
        }
        let mut tokens = body.split_whitespace();
        let Some(kw) = tokens.next() else { continue };
        if is_obj_keyword(kw) {
            saw_record = true;
        }

        match kw {
            "v" => {
                let p = xyz(&mut tokens, lineno)?;
                // Collected, not counted: `tokens.count()` consumes the
                // iterator without looking at it, so `v 0 0 0 xyzzy` was the
                // one place in this reader where a junk token returned Ok. The
                // `vt` arm below already parses its optional component for
                // exactly this reason.
                let rest: Vec<&str> = tokens.collect();
                match rest.len() {
                    // `w` is a rational-surface weight; for polygonal geometry it
                    // is always 1 and means nothing, so it is parsed (to reject
                    // garbage) and then dropped.
                    0 => {}
                    1 => {
                        rest[0].parse::<f32>().map_err(|_| {
                            malformed(lineno, format!("w `{}` is not a number", rest[0]))
                        })?;
                    }
                    // The MeshLab-style `v x y z r g b [a]` extension. The IR has
                    // no per-vertex colour field, so there is nowhere to put it
                    // and no target that could carry it — and silently dropping
                    // colour that is not in the module's lossiness table would be
                    // exactly the quiet wrongness this module exists to avoid.
                    3 | 4 => {
                        return Err(unsupported(
                            lineno,
                            "per-vertex colour on `v`; the IR carries only a flat base colour",
                        ))
                    }
                    n => {
                        return Err(malformed(
                            lineno,
                            format_args!("`v` takes 3 or 4 components, found {}", n + 3),
                        ))
                    }
                }
                src_v.push(p);
            }
            "vn" => {
                // Stored as written, never re-normalised: the lossiness table
                // promises obj keeps normals, and "keeps" does not mean "keeps a
                // rescaled version of". A consumer that needs unit normals can
                // say so; this codec cannot tell a sloppy exporter from a
                // deliberate one.
                let n = xyz(&mut tokens, lineno)?;
                if tokens.count() != 0 {
                    return Err(malformed(lineno, "`vn` takes exactly 3 components"));
                }
                src_vn.push(n);
            }
            "vt" => {
                let u = number(required(&mut tokens, lineno, "vt u")?, lineno, "u")?;
                // `vt u` alone is legal (1-D texture); its V is 0.
                let v = match tokens.next() {
                    Some(t) => number(t, lineno, "v")?,
                    None => 0.0,
                };
                // The third component is a depth coordinate for 3-D textures.
                // Parse it so a garbage token still fails, then drop it: the IR
                // is [u, v].
                if let Some(t) = tokens.next() {
                    number(t, lineno, "w")?;
                }
                if tokens.count() != 0 {
                    return Err(malformed(lineno, "`vt` takes at most 3 components"));
                }
                // OBJ puts the texture origin at the bottom-left with V running
                // up; glTF — and therefore this IR, since GLB is the hub every
                // other codec meets in — puts it at the top-left with V running
                // down. Flipping here rather than in each writer means exactly
                // one codec knows about the discrepancy. `write` flips back.
                //
                // Note `1.0 - v` is its own inverse only where the subtraction is
                // exact (Sterbenz: v in [0.5, 1]); elsewhere an obj -> obj
                // round-trip can move a V by one ulp. That is the price of
                // holding one convention in the IR, and it is far cheaper than
                // the alternative, where a GLB written from an OBJ is textured
                // upside down.
                src_vt.push([u, 1.0 - v]);
            }
            "f" => {
                corners.clear();
                for tok in tokens {
                    let c = parse_corner(tok, lineno, src_v.len(), src_vt.len(), src_vn.len())?;
                    let shape = (c.vt.is_some(), c.vn.is_some());
                    match layout {
                        None => layout = Some(shape),
                        // Real files do mix these (a textured group followed by an
                        // untextured one). We cannot: the IR's `uvs` is all-or-
                        // nothing, so the choice is inventing (0,0) for the
                        // corners that lack one — silently wrong texturing — or
                        // saying we will not do it.
                        Some(prev) if prev != shape => {
                            return Err(unsupported(
                                lineno,
                                format_args!(
                                    "face corner `{tok}` has (uv, normal) = ({}, {}) but earlier \
                                     corners have ({}, {}); the IR cannot carry an attribute for \
                                     only some vertices",
                                    shape.0, shape.1, prev.0, prev.1
                                ),
                            ))
                        }
                        Some(_) => {}
                    }
                    corners.push(c);
                }
                if corners.len() < 3 {
                    return Err(malformed(
                        lineno,
                        format_args!("face has {} corners; a polygon needs at least 3", corners.len()),
                    ));
                }
                let (want_vt, want_vn) = layout.unwrap_or((false, false));
                // Interning is per corner, not per triangle, so a fan shares its
                // hub vertex instead of emitting it n-2 times.
                slots.clear();
                for c in &corners {
                    let idx = match cache.get(c) {
                        Some(existing) => *existing,
                        None => {
                            let idx = u32::try_from(positions.len()).map_err(|_| {
                                unsupported(
                                    lineno,
                                    "more than 2^32 vertices; the IR indexes with u32",
                                )
                            })?;
                            // `resolve` already bounded every one of these, but
                            // the reads go through `get` so that reordering the
                            // resolution later cannot turn a logic slip into a
                            // panic on hostile input.
                            let p = *src_v.get(c.v as usize).ok_or_else(|| {
                                malformed(lineno, "vertex index out of range after resolution")
                            })?;
                            positions.push(p);
                            if want_vt {
                                let t = c
                                    .vt
                                    .and_then(|i| src_vt.get(i as usize))
                                    .ok_or_else(|| malformed(lineno, "texture index out of range"))?;
                                uvs.push(*t);
                            }
                            if want_vn {
                                let n = c
                                    .vn
                                    .and_then(|i| src_vn.get(i as usize))
                                    .ok_or_else(|| malformed(lineno, "normal index out of range"))?;
                                normals.push(*n);
                            }
                            cache.insert(*c, idx);
                            idx
                        }
                    };
                    slots.push(idx);
                }
                // Fan triangulation: (0, i, i+1). Correct for convex polygons and
                // for the quads that make up almost all n-gons in the wild;
                // concave n-gons can fan into overlapping triangles, but the
                // alternative (ear clipping, needing a face plane and a winding
                // test) is a geometry library, and the OBJ files that reach a
                // format converter come from triangulating exporters.
                for w in 1..slots.len() - 1 {
                    indices.extend_from_slice(&[slots[0], slots[w], slots[w + 1]]);
                }
            }
            // The first `o` names the mesh; later ones are additional objects we
            // merge into the single IR mesh, and renaming the result after the
            // last of them would be arbitrary.
            "o" => {
                if name.is_none() {
                    let rest = body[kw.len()..].trim();
                    if !rest.is_empty() {
                        name = Some(rest.to_string());
                    }
                }
            }
            // Geometry we cannot represent. Dropping it would hand back a mesh
            // that is missing parts of the file with nothing to say so — the one
            // outcome this module treats as worse than an error.
            "l" | "p" => {
                return Err(unsupported(
                    lineno,
                    format_args!("`{kw}` element; this module reads triangles only"),
                ))
            }
            "curv" | "curv2" | "surf" | "cstype" | "deg" | "bmat" | "step" | "parm" | "trim"
            | "hole" | "scrv" | "sp" => {
                return Err(unsupported(
                    lineno,
                    format_args!("free-form geometry (`{kw}`); only polygonal OBJ is supported"),
                ))
            }
            // Everything else — `g`, `s`, `usemtl`, `mtllib`, `usemap`, `bevel`,
            // and any vendor extension — is grouping, shading or material state.
            // The lossiness table already says obj conversion drops colour, and
            // grouping has nowhere to land in a single-mesh IR, so these are
            // ignored rather than refused. An unrecognised keyword is ignored
            // too, but it does not count as evidence that this is an OBJ.
            _ => {}
        }
    }

    if !saw_record {
        return Err(not_obj(format!(
            "no OBJ records in {line_count} lines of text"
        )));
    }

    let (has_vt, has_vn) = layout.unwrap_or((false, false));
    Ok(Mesh {
        positions,
        indices,
        normals: if has_vn { Some(normals) } else { None },
        uvs: if has_vt { Some(uvs) } else { None },
        // Dropped on purpose, in both directions: OBJ cannot express a colour
        // without an `.mtl` sidecar, and litegen rehosts every asset file under
        // its own key, so a `mtllib` reference emitted here would point at
        // nothing by the time a customer fetched it. The module docs list this
        // as the one thing obj loses.
        base_color: None,
        name,
    })
}

/// Append formatted text to a `String`.
///
/// `fmt::Write for String` returns a `Result` only because the trait must; it
/// cannot actually fail. Swallowing it here keeps ~10 call sites free of an
/// error path that does not exist, without an `unwrap` anywhere near input data.
fn put(out: &mut String, args: fmt::Arguments<'_>) {
    use fmt::Write as _;
    let _ = out.write_fmt(args);
}

/// Reject the values that would serialise to tokens no OBJ reader accepts.
///
/// `f32::Display` renders these as `NaN` and `inf`, which are not numbers in
/// OBJ's grammar — the file would be written happily and fail to re-read. A
/// mesh can only get here from another codec, so this is a loud assertion that
/// the IR held real geometry, not a guess at what the caller meant.
fn finite(v: [f32; 3], what: &str, i: usize) -> Result<(), MeshError> {
    if v.iter().any(|c| !c.is_finite()) {
        return Err(MeshError::Malformed(format!(
            "obj write: {what} {i} is not finite ({v:?})"
        )));
    }
    Ok(())
}

pub fn write(mesh: &Mesh) -> Result<Vec<u8>, MeshError> {
    // `mesh::write` validates too, but `obj::write` is also called directly (by
    // tests, and by anything that later reaches past the module front door), and
    // an out-of-range index here would emit a file that silently references a
    // vertex that does not exist.
    mesh.validate()?;

    let uvs = mesh.uvs.as_deref();
    let normals = mesh.normals.as_deref();

    // Rough but useful: ~24 bytes a line keeps the big meshes to one or two
    // reallocations instead of a dozen.
    let mut out = String::with_capacity(
        (mesh.positions.len() * 2 + mesh.triangle_count()) * 24 + 64,
    );
    // No `mtllib`/`usemtl`, ever. A colour in OBJ lives in a separate `.mtl`
    // file, and litegen's asset rehosting keys every file independently, so a
    // sidecar reference written here would not resolve for the customer who
    // downloads the mesh. An unresolvable material reference is worse than no
    // material: viewers report it as a broken file. `base_color` is dropped, as
    // the module's lossiness table says.
    put(&mut out, format_args!("# exported by litegen mesh conversion\n"));

    if let Some(raw) = &mesh.name {
        // `o` runs to the end of the line, so spaces in a name are fine but a
        // newline would terminate the record and turn the rest of the name into
        // a bogus keyword line; `#` would comment out everything after it.
        // Mangling those two characters beats refusing to convert geometry over
        // metadata.
        // Backslash is mangled for the same reason: OBJ treats a trailing `\`
        // as a line continuation, so a name ending in one — a Windows path used
        // as a glTF node name is exactly this shape — produced a file our own
        // reader then rejected. Trim BEFORE sanitising, or trimming a trailing
        // space re-exposes the backslash the sanitiser just neutralised.
        let safe: String = raw
            .trim()
            .chars()
            .map(|c| if c == '#' || c == '\\' || c.is_control() { '_' } else { c })
            .collect();
        let safe = safe.as_str();
        if !safe.is_empty() {
            put(&mut out, format_args!("o {safe}\n"));
        }
    }

    // Float formatting: Rust's `Display` for `f32` emits the shortest decimal
    // that parses back to the identical bit pattern. That is both halves of the
    // brief at once — `0.1` stays `0.1` rather than becoming `0.100000001`, and
    // a value that genuinely needs 9 digits gets 9 — so a fixed `{:.6}` would be
    // strictly worse: it is longer for round numbers and lossy for the rest.
    for (i, p) in mesh.positions.iter().enumerate() {
        finite(*p, "position", i)?;
        put(&mut out, format_args!("v {} {} {}\n", p[0], p[1], p[2]));
    }
    if let Some(uvs) = uvs {
        for (i, t) in uvs.iter().enumerate() {
            finite([t[0], t[1], 0.0], "uv", i)?;
            // Undo the V flip taken on read; see the `vt` arm there.
            put(&mut out, format_args!("vt {} {}\n", t[0], 1.0 - t[1]));
        }
    }
    if let Some(normals) = normals {
        for (i, n) in normals.iter().enumerate() {
            finite(*n, "normal", i)?;
            put(&mut out, format_args!("vn {} {} {}\n", n[0], n[1], n[2]));
        }
    }

    // The IR is per-vertex, so one IR index is the position, texture and normal
    // index at once — which is also why an OBJ this writer produced reads back
    // with the same vertex count it was written from.
    for tri in mesh.indices.chunks_exact(3) {
        out.push('f');
        for idx in tri {
            let one_based = *idx as usize + 1;
            match (uvs.is_some(), normals.is_some()) {
                (true, true) => put(&mut out, format_args!(" {one_based}/{one_based}/{one_based}")),
                (true, false) => put(&mut out, format_args!(" {one_based}/{one_based}")),
                (false, true) => put(&mut out, format_args!(" {one_based}//{one_based}")),
                (false, false) => put(&mut out, format_args!(" {one_based}")),
            }
        }
        out.push('\n');
    }

    Ok(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit quad: two triangles, four corners, every attribute present.
    /// UVs are dyadic so the `1.0 - v` flip is exact in both directions and the
    /// round-trip can be asserted bit-for-bit.
    const QUAD: &str = "\
# a quad
o panel
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
vt 0 0
vt 1 0
vt 1 1
vt 0 1
vn 0 0 1
f 1/1/1 2/2/1 3/3/1
f 1/1/1 3/3/1 4/4/1
";

    fn parse(s: &str) -> Result<Mesh, MeshError> {
        read(s.as_bytes())
    }

    fn text(mesh: &Mesh) -> String {
        String::from_utf8(write(mesh).expect("write")).expect("utf8")
    }

    // ---- happy paths -----------------------------------------------------

    #[test]
    fn reads_a_quad_with_every_attribute() {
        let m = parse(QUAD).expect("read");
        assert_eq!(m.name.as_deref(), Some("panel"));
        assert_eq!(m.triangle_count(), 2);
        // Each of the four corners has a unique (v, vt, vn) triple, so four IR
        // vertices — the shared normal does not merge anything.
        assert_eq!(m.positions.len(), 4);
        assert_eq!(m.indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(m.normals.as_ref().map(|n| n.len()), Some(4));
        assert_eq!(m.uvs.as_ref().map(|u| u.len()), Some(4));
        assert_eq!(m.base_color, None);
        m.validate().expect("valid");
    }

    #[test]
    fn round_trip_is_exact() {
        let a = parse(QUAD).expect("read");
        let b = parse(&text(&a)).expect("re-read");
        assert_eq!(a, b);
    }

    #[test]
    fn round_trip_preserves_the_name() {
        let a = parse(QUAD).expect("read");
        assert!(text(&a).contains("\no panel\n"));
        assert_eq!(parse(&text(&a)).expect("re-read").name.as_deref(), Some("panel"));
    }

    #[test]
    fn positions_only_round_trip() {
        let m = parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").expect("read");
        assert_eq!(m.uvs, None);
        assert_eq!(m.normals, None);
        let s = text(&m);
        assert!(s.contains("f 1 2 3"), "{s}");
        assert!(!s.contains("vt "), "{s}");
        assert!(!s.contains("vn "), "{s}");
        assert_eq!(parse(&s).expect("re-read"), m);
    }

    #[test]
    fn normals_without_uvs_round_trip() {
        let src = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvn 0 0 1\nf 1//1 2//1 3//1\n";
        let m = parse(src).expect("read");
        assert_eq!(m.uvs, None);
        assert_eq!(m.normals, Some(vec![[0.0, 0.0, 1.0]; 3]));
        let s = text(&m);
        assert!(s.contains("f 1//1 2//2 3//3"), "{s}");
        assert_eq!(parse(&s).expect("re-read"), m);
    }

    #[test]
    fn uvs_without_normals_round_trip() {
        let src = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n";
        let m = parse(src).expect("read");
        assert_eq!(m.normals, None);
        assert_eq!(m.uvs, Some(vec![[0.0, 1.0], [1.0, 1.0], [0.0, 0.0]]));
        let s = text(&m);
        assert!(s.contains("f 1/1 2/2 3/3"), "{s}");
        assert_eq!(parse(&s).expect("re-read"), m);
    }

    #[test]
    fn writes_no_material_reference() {
        let mut m = parse(QUAD).expect("read");
        m.base_color = Some([1.0, 0.0, 0.0, 1.0]);
        let s = text(&m);
        assert!(!s.contains("mtllib"), "{s}");
        assert!(!s.contains("usemtl"), "{s}");
        // And the colour is gone on the way back, as the lossiness table says.
        assert_eq!(parse(&s).expect("re-read").base_color, None);
    }

    // ---- OBJ's specific awkwardnesses ------------------------------------

    #[test]
    fn deduplicates_on_the_whole_triple_not_the_vertex_index() {
        // Position 1 is used with two different texture coordinates — a UV seam.
        // Keying on the `v` index alone would give 3 vertices and lose one uv.
        let src = "\
v 0 0 0
v 1 0 0
v 0 1 0
vt 0 0
vt 1 0
vt 0 1
vt 0.5 0.5
f 1/1 2/2 3/3
f 1/4 2/2 3/3
";
        let m = parse(src).expect("read");
        assert_eq!(m.positions.len(), 4);
        assert_eq!(m.positions[0], m.positions[3]);
        let uvs = m.uvs.expect("uvs");
        assert_eq!(uvs[0], [0.0, 1.0]);
        assert_eq!(uvs[3], [0.5, 0.5]);
        // The two faces share the corners they really do share.
        assert_eq!(m.indices, vec![0, 1, 2, 3, 1, 2]);
    }

    #[test]
    fn negative_indices_count_back_from_the_end_so_far() {
        let src = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf -3 -2 -1\n";
        let m = parse(src).expect("read");
        assert_eq!(m.indices, vec![0, 1, 2]);
        assert_eq!(m.positions[2], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn negative_indices_are_relative_to_their_own_block() {
        // Two objects, each addressing its own vertices with -1..-3. A reader
        // that resolved against the final list would mangle the first face.
        let src = "\
o a
v 0 0 0
v 1 0 0
v 0 1 0
f -3 -2 -1
o b
v 5 0 0
v 6 0 0
v 5 1 0
f -3 -2 -1
";
        let m = parse(src).expect("read");
        assert_eq!(m.indices, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(m.positions[3], [5.0, 0.0, 0.0]);
        assert_eq!(m.name.as_deref(), Some("a"), "the first `o` names the mesh");
    }

    #[test]
    fn mixed_positive_and_negative_indices_in_one_face() {
        let m = parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 -2 -1\n").expect("read");
        assert_eq!(m.indices, vec![0, 1, 2]);
    }

    #[test]
    fn ngons_triangulate_as_a_fan() {
        let src = "\
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
v -1 0.5 0
f 1 2 3 4 5
";
        let m = parse(src).expect("read");
        assert_eq!(m.triangle_count(), 3);
        assert_eq!(m.indices, vec![0, 1, 2, 0, 2, 3, 0, 3, 4]);
        // The hub vertex is interned once, not once per triangle.
        assert_eq!(m.positions.len(), 5);
    }

    #[test]
    fn uv_v_axis_is_flipped_on_read_and_restored_on_write() {
        let m = parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0.25\nvt 0 0.5\nvt 0 1\nf 1/1 2/2 3/3\n")
            .expect("read");
        // OBJ 0.25 (a quarter up from the bottom) is glTF 0.75 (three quarters
        // down from the top).
        assert_eq!(m.uvs, Some(vec![[0.0, 0.75], [0.0, 0.5], [0.0, 0.0]]));
        let s = text(&m);
        assert!(s.contains("vt 0 0.25"), "{s}");
        assert!(s.contains("vt 0 1"), "{s}");
    }

    #[test]
    fn vt_may_have_one_or_three_components() {
        let m = parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0.5\nvt 0.5 0.5 9\nvt 1 1\nf 1/1 2/2 3/3\n")
            .expect("read");
        let uvs = m.uvs.expect("uvs");
        assert_eq!(uvs[0], [0.5, 1.0], "a missing V is 0 in OBJ, so 1 after the flip");
        assert_eq!(uvs[1], [0.5, 0.5], "the third component is dropped");
    }

    #[test]
    fn v_accepts_a_fourth_weight_component() {
        let m = parse("v 0 0 0 1\nv 1 0 0 1\nv 0 1 0 0.5\nf 1 2 3\n").expect("read");
        assert_eq!(m.positions[2], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn ignores_grouping_shading_and_material_records() {
        let src = "\
# comment

mtllib scene.mtl
usemtl gold
g group1
s 1
v 0 0 0
v 1 0 0
v 0 1 0
f 1 2 3
";
        let m = parse(src).expect("read");
        assert_eq!(m.triangle_count(), 1);
        assert_eq!(m.name, None, "`g` is not `o`");
    }

    #[test]
    fn trailing_comments_and_crlf_and_no_final_newline() {
        let src = "v 0 0 0 # origin\r\nv 1 0 0\r\nv 0 1 0\r\nf 1 2 3";
        let m = parse(src).expect("read");
        assert_eq!(m.triangle_count(), 1);
        assert_eq!(m.positions[0], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn scientific_and_signed_notation_parses() {
        let m = parse("v 1e-3 -.5 +2.\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").expect("read");
        assert_eq!(m.positions[0], [0.001, -0.5, 2.0]);
    }

    #[test]
    fn a_file_with_vertices_but_no_faces_reads_empty() {
        // mod.rs::read turns this into MeshError::Empty; obj.rs must not.
        let m = parse("v 0 0 0\nv 1 0 0\n").expect("read");
        assert_eq!(m.triangle_count(), 0);
        assert_eq!(m.positions.len(), 0);
    }

    #[test]
    fn a_file_with_only_metadata_reads_empty_not_not_this_format() {
        // `mesh::tests::read_refuses_an_empty_mesh_rather_than_returning_one`
        // feeds exactly these bytes and expects MeshError::Empty, which only
        // mod.rs can raise — so this must come back Ok.
        let m = parse("# nothing here\no lonely\n").expect("read");
        assert_eq!(m.triangle_count(), 0);
        assert_eq!(m.name.as_deref(), Some("lonely"));
    }

    #[test]
    fn shortest_round_trip_float_formatting() {
        let m = parse("v 0.1 1 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").expect("read");
        let s = text(&m);
        assert!(s.contains("v 0.1 1 0\n"), "compact, and not 0.100000001: {s}");
        assert_eq!(parse(&s).expect("re-read").positions[0], [0.1, 1.0, 0.0]);
    }

    #[test]
    fn a_name_with_a_newline_cannot_break_the_file() {
        let mut m = parse(QUAD).expect("read");
        m.name = Some("evil\nf 1 1 1".to_string());
        let s = text(&m);
        assert!(s.contains("o evil_f 1 1 1\n"), "{s}");
        assert_eq!(parse(&s).expect("re-read").triangle_count(), 2);
    }

    // ---- NotThisFormat ---------------------------------------------------

    #[test]
    fn rejects_a_glb() {
        let err = read(b"glTF\x02\x00\x00\x00").unwrap_err();
        assert!(matches!(err, MeshError::NotThisFormat { expected: MeshFormat::Obj, .. }), "{err:?}");
    }

    #[test]
    fn rejects_a_ply() {
        let err = read(b"ply\nformat ascii 1.0\n").unwrap_err();
        assert!(matches!(err, MeshError::NotThisFormat { .. }), "{err:?}");
    }

    #[test]
    fn rejects_binary_input() {
        let err = read(&[b'v', b' ', 0, 1, 2, 3]).unwrap_err();
        assert!(matches!(err, MeshError::NotThisFormat { .. }), "{err:?}");
    }

    #[test]
    fn rejects_empty_input() {
        assert!(matches!(read(b"").unwrap_err(), MeshError::NotThisFormat { .. }));
    }

    #[test]
    fn rejects_text_with_no_obj_records() {
        // A text STL, say, or a stray README.
        let err = parse("solid thing\nfacet normal 0 0 1\nvertex 0 0 0\n").unwrap_err();
        assert!(matches!(err, MeshError::NotThisFormat { .. }), "{err:?}");
    }

    // ---- Malformed -------------------------------------------------------

    #[test]
    fn rejects_non_utf8_naming_the_line() {
        let mut bytes = b"v 0 0 0\nv 1 0 0\n".to_vec();
        bytes.extend_from_slice(&[b'v', b' ', 0xff, b'\n']);
        match read(&bytes).unwrap_err() {
            MeshError::Malformed(d) => assert!(d.contains("line 3") && d.contains("UTF-8"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_a_non_numeric_token_naming_the_line() {
        match parse("v 0 0 0\nv 1 0 0\nv 0 x 0\nf 1 2 3\n").unwrap_err() {
            MeshError::Malformed(d) => assert!(d.contains("line 3") && d.contains('x'), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_non_finite_numbers() {
        for bad in ["v nan 0 0", "v inf 0 0", "v -Infinity 0 0"] {
            let src = format!("{bad}\nv 1 0 0\nv 0 1 0\nf 1 2 3\n");
            match parse(&src).unwrap_err() {
                MeshError::Malformed(d) => assert!(d.contains("finite"), "{d}"),
                e => panic!("{bad}: {e:?}"),
            }
        }
    }

    #[test]
    fn rejects_a_truncated_vertex() {
        match parse("v 0 0\nf 1 1 1\n").unwrap_err() {
            MeshError::Malformed(d) => assert!(d.contains("line 1") && d.contains("missing"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_a_five_component_vertex() {
        match parse("v 0 0 0 1 2\nf 1 1 1\n").unwrap_err() {
            MeshError::Malformed(d) => assert!(d.contains("3 or 4"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_a_four_component_normal() {
        assert!(matches!(
            parse("v 0 0 0\nvn 0 0 1 1\nf 1//1 1//1 1//1\n").unwrap_err(),
            MeshError::Malformed(_)
        ));
    }

    #[test]
    fn rejects_a_four_component_uv() {
        assert!(matches!(
            parse("v 0 0 0\nvt 0 0 0 0\nf 1/1 1/1 1/1\n").unwrap_err(),
            MeshError::Malformed(_)
        ));
    }

    #[test]
    fn rejects_a_face_with_two_corners() {
        match parse("v 0 0 0\nv 1 0 0\nf 1 2\n").unwrap_err() {
            MeshError::Malformed(d) => assert!(d.contains("2 corners"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_index_zero() {
        match parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 1 2\n").unwrap_err() {
            MeshError::Malformed(d) => assert!(d.contains("1-based"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_an_index_past_the_end() {
        match parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 4\n").unwrap_err() {
            MeshError::Malformed(d) => assert!(d.contains("line 4") && d.contains("out of range"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_a_forward_reference() {
        // Three vertices exist by the end of the file, but not yet at line 2.
        match parse("v 0 0 0\nf 1 2 3\nv 1 0 0\nv 0 1 0\n").unwrap_err() {
            MeshError::Malformed(d) => assert!(d.contains("line 2"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_a_negative_index_past_the_start() {
        match parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nf -4 -2 -1\n").unwrap_err() {
            MeshError::Malformed(d) => assert!(d.contains("out of range"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_a_hostile_index() {
        // Wide enough to overflow i64, which is the point: it must fail as a
        // parse error rather than wrap into something in range.
        match parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 99999999999999999999999\n").unwrap_err() {
            MeshError::Malformed(d) => assert!(d.contains("not an integer"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_a_uv_index_when_no_vt_lines_exist() {
        match parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1/1 2/1 3/1\n").unwrap_err() {
            MeshError::Malformed(d) => assert!(d.contains("texture") && d.contains("out of range"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_an_empty_vertex_field() {
        match parse("v 0 0 0\nvt 0 0\nf /1 /1 /1\n").unwrap_err() {
            MeshError::Malformed(d) => assert!(d.contains("empty"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_a_corner_with_four_fields() {
        // Every field in the corner resolves; the fourth one is the problem.
        match parse("v 0 0 0\nvt 0 0\nvn 0 0 1\nf 1/1/1/1 1/1/1 1/1/1\n").unwrap_err() {
            MeshError::Malformed(d) => assert!(d.contains("more than three"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn write_rejects_non_finite_geometry() {
        let mut m = parse(QUAD).expect("read");
        m.positions[1][0] = f32::NAN;
        match write(&m).unwrap_err() {
            MeshError::Malformed(d) => assert!(d.contains("not finite"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn write_rejects_an_invalid_mesh() {
        let m = Mesh { positions: vec![[0.0; 3]], indices: vec![0, 1, 2], ..Default::default() };
        assert!(matches!(write(&m).unwrap_err(), MeshError::Malformed(_)));
    }

    // ---- Unsupported -----------------------------------------------------

    #[test]
    fn rejects_per_vertex_colour() {
        match parse("v 0 0 0 1 0 0\nv 1 0 0 1 0 0\nv 0 1 0 1 0 0\nf 1 2 3\n").unwrap_err() {
            MeshError::Unsupported(d) => assert!(d.contains("colour"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_line_and_point_elements() {
        for kw in ["l 1 2", "p 1"] {
            let src = format!("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n{kw}\n");
            match parse(&src).unwrap_err() {
                MeshError::Unsupported(d) => assert!(d.contains("triangles only"), "{d}"),
                e => panic!("{kw}: {e:?}"),
            }
        }
    }

    #[test]
    fn rejects_free_form_geometry() {
        let src = "v 0 0 0\nv 1 0 0\ncstype bezier\ncurv 0 1 1 2\n";
        match parse(src).unwrap_err() {
            MeshError::Unsupported(d) => assert!(d.contains("free-form"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_faces_that_disagree_about_uvs() {
        let src = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nf 1/1 2/1 3/1\nf 1 2 3\n";
        match parse(src).unwrap_err() {
            MeshError::Unsupported(d) => assert!(d.contains("line 6") && d.contains("only some"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_faces_that_disagree_about_normals() {
        let src = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvn 0 0 1\nf 1//1 2//1 3//1\nf 1 2 3\n";
        assert!(matches!(parse(src).unwrap_err(), MeshError::Unsupported(_)));
    }

    #[test]
    fn rejects_line_continuation() {
        let src = "v 0 0 \\\n0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
        match parse(src).unwrap_err() {
            MeshError::Unsupported(d) => assert!(d.contains("continuation"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn truncated_input_never_panics() {
        // Every prefix of a real file must come back Ok or Err, never a panic.
        let bytes = QUAD.as_bytes();
        for n in 0..bytes.len() {
            let _ = read(&bytes[..n]);
        }
    }
}
