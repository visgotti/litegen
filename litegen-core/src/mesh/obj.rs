//! Wavefront OBJ (+ MTL) — the plain-text container.
//!
//! OBJ is the format everyone can open and nobody implements the same way, so
//! this reader is deliberately narrow: the polygonal subset (`v`, `vt`, `vn`,
//! `f`, `o`, `usemtl`, `mtllib`) and nothing else. What it does accept it
//! accepts exactly — negative indices, every `v`/`v/vt`/`v//vn`/`v/vt/vn`
//! spelling and n-gons all turn up in real vendor output, and a reader that
//! quietly mishandles any of them produces a file that looks fine until someone
//! textures it.
//!
//! # The one structural thing OBJ does differently
//!
//! OBJ keeps three *independent* index spaces — positions, texture coordinates,
//! normals — and a face corner names one entry from each. The scene, like a GPU
//! vertex buffer and like glTF, is per-vertex: one index selects a position and
//! its uv and its normal together. Bridging the two is the whole job of the
//! reader, and doing it by the position index alone is the classic wrong answer
//! — see the comment on the corner cache.
//!
//! # Objects, materials and the `.mtl` sidecar
//!
//! Each `o` starts a mesh; within it, each `usemtl` switches to the primitive
//! for that material, so a multi-material object becomes one mesh with one
//! primitive per material — glTF's own shape. `g` groups are ignored: they are
//! selection sets in most exporters, not objects. Materials live in `.mtl`
//! files named by `mtllib`, and textures in files those name; both are looked up
//! through the caller's [`Sidecars`], because conversion never touches a
//! filesystem. A library or texture that was not supplied is a warning, not a
//! failure: the geometry is still right.
//!
//! The writer has two shapes. In package mode it writes `model.obj`,
//! `model.mtl` and the textures beside them, which is what DCC tools call an
//! OBJ "with materials". Self-contained, it writes one file: no `mtllib` (a
//! reference to a file that is not there is reported by viewers as a broken
//! model), with each material's base colour baked into per-vertex colour so the
//! parts keep their colours.
//!
//! # Colour
//!
//! OBJ and MTL predate colour management and say nothing about colour space.
//! three.js — the engine behind the dashboard's viewer — reads `Kd`/`Ke` and
//! the `v x y z r g b` vertex-colour extension as sRGB, as do MeshLab and
//! Unity; Blender reads `Kd` as linear. This codec follows the majority, and the
//! product's own viewer: linear scene colour is encoded to sRGB on write and
//! decoded on read.

use std::collections::HashMap;
use std::fmt;

use super::ir::Mesh;
use super::math::{linear_to_srgb, srgb_to_linear};
use super::package::{self, NoSidecars, Package, PackageFile, Sidecars};
use super::scene::{
    AlphaMode, Image, ImageFormat, Material, Node, Primitive, Sampler, Scene, SceneMesh, Texture,
    TextureRef, UvTransform, Wrap,
};
use super::{MeshError, MeshFormat, SceneRead};

/// How far into the file we look for evidence that these bytes are not text at
/// all. A real OBJ's first kilobyte is a comment banner and some `v` lines; a
/// mislabelled GLB or binary PLY has structure there we can recognise.
const SNIFF_BYTES: usize = 1024;

/// `.mtl` files one OBJ may name. Real files name one; the bound keeps a
/// hostile `mtllib` line from turning into thousands of sidecar lookups.
const MAX_MATERIAL_LIBRARIES: usize = 16;

fn not_obj(detail: impl Into<String>) -> MeshError {
    MeshError::NotThisFormat { expected: MeshFormat::Obj, detail: detail.into() }
}

/// Reject bytes that are plainly another container before we try to decode them
/// as text.
///
/// OBJ has no magic number, so "is this an OBJ?" can only ever be answered by
/// what the bytes are *not*. Without this, handing the reader a GLB reports a
/// UTF-8 error deep in the binary buffer — technically true, useless to the
/// caller, and the wrong variant: the caller's mistake was the format tag, and
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
/// the reader does that. It is deliberately the full spec list, including the
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
/// no consumer of the scene can do anything with: they poison bounds, they make
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

/// One primitive under construction: the faces of one object that use one
/// material.
struct PrimBuild {
    material: Option<String>,
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
    /// The corner cache, and the reason this reader exists.
    ///
    /// Reading an OBJ by its `v` index alone — the naive shortcut — is wrong in
    /// exactly the cases that matter. At a UV seam the same position appears
    /// with two different `vt` indices, and keying on `v` lets one of them win,
    /// which smears the texture across the seam. At a hard edge the same
    /// position appears with two different `vn` indices, and keying on `v`
    /// welds them into one smooth normal, which rounds off the crease the author
    /// modelled. Both survive validation and both look like a rendering bug
    /// later. Keying on the full triple splits those corners into separate
    /// vertices, which is what a per-vertex representation means.
    cache: HashMap<Corner, u32>,
    /// Fixed by the primitive's first corner: a primitive carries uvs and
    /// normals for all of its vertices or for none, so its faces have to agree.
    layout: Option<(bool, bool)>,
}

struct ObjBuild {
    name: Option<String>,
    prims: Vec<PrimBuild>,
}

/// Everything the OBJ text itself describes, before materials are resolved.
struct Parsed {
    objects: Vec<ObjBuild>,
    libraries: Vec<String>,
    /// Whether the `v` lines carried colour (all of them, or none).
    has_colors: bool,
    first_name: Option<String>,
}

fn parse_obj(bytes: &[u8]) -> Result<Parsed, MeshError> {
    sniff(bytes)?;
    let text = std::str::from_utf8(bytes).map_err(|e| {
        let valid = e.valid_up_to();
        // Report the line, like every other error here, so a caller staring at a
        // 200 MB file has somewhere to look; the byte offset alone is not that.
        let lineno = 1 + bytes[..valid].iter().filter(|b| **b == b'\n').count();
        malformed(lineno, format_args!("input is not UTF-8 (first bad byte at offset {valid})"))
    })?;

    // OBJ's three index spaces, in file order. These are the *source* arrays;
    // the scene's arrays are built from face corners, not from these.
    let mut src_v: Vec<[f32; 3]> = Vec::new();
    let mut src_vc: Vec<[f32; 4]> = Vec::new();
    let mut src_vt: Vec<[f32; 2]> = Vec::new();
    let mut src_vn: Vec<[f32; 3]> = Vec::new();
    // Whether the first `v` line carried colour; every other one must agree.
    let mut colored: Option<bool> = None;

    let mut objects: Vec<ObjBuild> = vec![ObjBuild { name: None, prims: Vec::new() }];
    let mut material: Option<String> = None;
    let mut libraries: Vec<String> = Vec::new();
    let mut first_name: Option<String> = None;
    // Evidence that these bytes really are an OBJ: at least one line whose
    // keyword belongs to the format. Geometry is NOT the test — a file of
    // comments and an `o` line is a valid, empty OBJ, and the module contract
    // puts `Empty` in `mesh::read`'s hands, not here. What this catches is the
    // other direction: text that is some other format (an ASCII STL, a README,
    // a Collada or USDA document) handed over with the wrong tag, where every
    // line is an unknown keyword.
    let mut saw_record = false;
    let mut line_count = 0usize;
    let mut total_vertices = 0usize;
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
                // iterator without looking at it, so `v 0 0 0 xyzzy` was once
                // the one place in this reader where a junk token returned Ok.
                let rest: Vec<&str> = tokens.collect();
                let this_colored = match rest.len() {
                    // `w` is a rational-surface weight; for polygonal geometry it
                    // is always 1 and means nothing, so it is parsed (to reject
                    // garbage) and then dropped.
                    0 => false,
                    1 => {
                        rest[0].parse::<f32>().map_err(|_| {
                            malformed(lineno, format!("w `{}` is not a number", rest[0]))
                        })?;
                        false
                    }
                    // The `v x y z r g b` vertex-colour extension (MeshLab,
                    // Blender, ZBrush exports, three.js reads it): sRGB in 0..1.
                    3 => {
                        let mut c = [1.0f32; 4];
                        for k in 0..3 {
                            let x = number(rest[k], lineno, "colour")?;
                            c[k] = srgb_to_linear(x.clamp(0.0, 1.0));
                        }
                        src_vc.push(c);
                        true
                    }
                    // Seven components could be `x y z w r g b` or `x y z r g b
                    // a`, and writers disagree; guessing would mean wrong
                    // positions or wrong colours.
                    4 => {
                        return Err(unsupported(
                            lineno,
                            "`v` with 7 components is ambiguous (a weight plus colour, or colour plus alpha)",
                        ))
                    }
                    n => {
                        return Err(malformed(
                            lineno,
                            format_args!("`v` takes 3, 4 or 6 components, found {}", n + 3),
                        ))
                    }
                };
                match colored {
                    None => colored = Some(this_colored),
                    // The scene carries colour for all of a primitive's vertices
                    // or none; a file where only some carry it would need an
                    // invented colour for the rest.
                    Some(prev) if prev != this_colored => {
                        return Err(unsupported(
                            lineno,
                            "only some `v` lines carry vertex colour; the scene cannot carry an \
                             attribute for only some vertices",
                        ))
                    }
                    Some(_) => {}
                }
                src_v.push(p);
            }
            "vn" => {
                // Stored as written, never re-normalised: "keeps normals" does
                // not mean "keeps a rescaled version of". The glTF writer
                // normalises because glTF requires it; OBJ does not.
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
                // Parse it so a garbage token still fails, then drop it.
                if let Some(t) = tokens.next() {
                    number(t, lineno, "w")?;
                }
                if tokens.count() != 0 {
                    return Err(malformed(lineno, "`vt` takes at most 3 components"));
                }
                // OBJ puts the texture origin at the bottom-left with V running
                // up; the scene (glTF) puts it at the top-left with V running
                // down. Flipping here rather than in each writer means exactly
                // one codec knows about the discrepancy. `write` flips back.
                //
                // Note `1.0 - v` is its own inverse only where the subtraction is
                // exact (Sterbenz: v in [0.5, 1]); elsewhere an obj -> obj
                // round-trip can move a V by one ulp. That is the price of
                // holding one convention in the scene, and it is far cheaper than
                // the alternative, where a GLB written from an OBJ is textured
                // upside down.
                src_vt.push([u, 1.0 - v]);
            }
            "f" => {
                corners.clear();
                for tok in tokens {
                    corners.push(parse_corner(tok, lineno, src_v.len(), src_vt.len(), src_vn.len())?);
                }
                if corners.len() < 3 {
                    return Err(malformed(
                        lineno,
                        format_args!("face has {} corners; a polygon needs at least 3", corners.len()),
                    ));
                }
                let obj = objects.last_mut().ok_or_else(|| malformed(lineno, "no current object"))?;
                let pi = match obj.prims.iter().position(|p| p.material == material) {
                    Some(pi) => pi,
                    None => {
                        obj.prims.push(PrimBuild {
                            material: material.clone(),
                            positions: Vec::new(),
                            uvs: Vec::new(),
                            normals: Vec::new(),
                            colors: Vec::new(),
                            indices: Vec::new(),
                            cache: HashMap::new(),
                            layout: None,
                        });
                        obj.prims.len() - 1
                    }
                };
                let prim = &mut obj.prims[pi];
                for c in &corners {
                    let shape = (c.vt.is_some(), c.vn.is_some());
                    match prim.layout {
                        None => prim.layout = Some(shape),
                        // Real files do mix these (a textured group followed by
                        // an untextured one). Within one material of one object
                        // we cannot: the choice is inventing (0,0) for the corners
                        // that lack one — silently wrong texturing — or saying we
                        // will not do it.
                        Some(prev) if prev != shape => {
                            return Err(unsupported(
                                lineno,
                                format_args!(
                                    "face corner has (uv, normal) = ({}, {}) but earlier corners of \
                                     this object and material have ({}, {}); the scene cannot carry \
                                     an attribute for only some vertices",
                                    shape.0, shape.1, prev.0, prev.1
                                ),
                            ))
                        }
                        Some(_) => {}
                    }
                }
                let (want_vt, want_vn) = prim.layout.unwrap_or((false, false));
                // Interning is per corner, not per triangle, so a fan shares its
                // hub vertex instead of emitting it n-2 times.
                slots.clear();
                for c in &corners {
                    let idx = match prim.cache.get(c) {
                        Some(existing) => *existing,
                        None => {
                            let idx = u32::try_from(prim.positions.len()).map_err(|_| {
                                unsupported(lineno, "more than 2^32 vertices; the scene indexes with u32")
                            })?;
                            total_vertices += 1;
                            if total_vertices > super::scene::MAX_SCENE_VERTICES {
                                return Err(unsupported(
                                    lineno,
                                    format_args!(
                                        "more than {} vertices",
                                        super::scene::MAX_SCENE_VERTICES
                                    ),
                                ));
                            }
                            // `resolve` already bounded every one of these, but
                            // the reads go through `get` so that reordering the
                            // resolution later cannot turn a logic slip into a
                            // panic on hostile input.
                            let p = *src_v.get(c.v as usize).ok_or_else(|| {
                                malformed(lineno, "vertex index out of range after resolution")
                            })?;
                            prim.positions.push(p);
                            if colored == Some(true) {
                                let col = *src_vc
                                    .get(c.v as usize)
                                    .ok_or_else(|| malformed(lineno, "vertex colour out of range"))?;
                                prim.colors.push(col);
                            }
                            if want_vt {
                                let t = c
                                    .vt
                                    .and_then(|i| src_vt.get(i as usize))
                                    .ok_or_else(|| malformed(lineno, "texture index out of range"))?;
                                prim.uvs.push(*t);
                            }
                            if want_vn {
                                let n = c
                                    .vn
                                    .and_then(|i| src_vn.get(i as usize))
                                    .ok_or_else(|| malformed(lineno, "normal index out of range"))?;
                                prim.normals.push(*n);
                            }
                            prim.cache.insert(*c, idx);
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
                if prim.indices.len() + (slots.len() - 2) * 3 > super::scene::MAX_SCENE_INDICES {
                    return Err(unsupported(lineno, "too many triangles"));
                }
                for w in 1..slots.len() - 1 {
                    prim.indices.extend_from_slice(&[slots[0], slots[w], slots[w + 1]]);
                }
            }
            // Each `o` starts a new object. The first one before any geometry
            // names the object that is already open rather than leaving an empty
            // nameless one behind.
            "o" => {
                let rest = body[kw.len()..].trim();
                let name = (!rest.is_empty()).then(|| rest.to_string());
                if first_name.is_none() {
                    first_name = name.clone();
                }
                match objects.last_mut() {
                    Some(o) if o.prims.is_empty() => o.name = name,
                    _ => objects.push(ObjBuild { name, prims: Vec::new() }),
                }
            }
            "usemtl" => {
                let rest = body[kw.len()..].trim();
                material = (!rest.is_empty()).then(|| rest.to_string());
            }
            "mtllib" => {
                // Names are whitespace-separated; a name with spaces cannot be
                // told apart from two names, which is the spec's problem, not
                // one to guess around.
                for lib in body[kw.len()..].split_whitespace() {
                    if libraries.len() >= MAX_MATERIAL_LIBRARIES {
                        return Err(unsupported(lineno, "too many material libraries"));
                    }
                    libraries.push(lib.to_string());
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
            // Everything else — `g`, `s`, `usemap`, `bevel`, and any vendor
            // extension — is grouping or display state with nowhere to land in
            // the scene, so it is ignored rather than refused. An unrecognised
            // keyword is ignored too, but it does not count as evidence that
            // this is an OBJ.
            _ => {}
        }
    }

    if !saw_record {
        return Err(not_obj(format!("no OBJ records in {line_count} lines of text")));
    }
    Ok(Parsed { objects, libraries, has_colors: colored == Some(true), first_name })
}

// ─── MTL ────────────────────────────────────────────────────────────────────

/// One texture statement (`map_Kd [options] file`) after its options.
struct MapSpec {
    file: String,
    offset: [f32; 2],
    scale: [f32; 2],
    clamp: bool,
    /// `-bm`: bump multiplier, which for a normal map is its scale.
    bump: Option<f32>,
}

/// Split a map statement's options from its file name.
///
/// Options take a fixed number of arguments each; whatever is left is the file
/// name, which may legally contain spaces.
fn parse_map(args: &str) -> Option<MapSpec> {
    let toks: Vec<&str> = args.split_whitespace().collect();
    let mut spec = MapSpec { file: String::new(), offset: [0.0, 0.0], scale: [1.0, 1.0], clamp: false, bump: None };
    let mut i = 0;
    let num = |t: Option<&&str>| t.and_then(|s| s.parse::<f32>().ok()).filter(|v| v.is_finite());
    while i < toks.len() && toks[i].starts_with('-') {
        let opt = toks[i].to_ascii_lowercase();
        // Options with up to three numeric arguments (u [v [w]]) consume as
        // many numbers as follow them.
        let take_nums = |from: usize, max: usize| -> usize {
            let mut n = 0;
            while n < max && num(toks.get(from + n)).is_some() {
                n += 1;
            }
            n
        };
        match opt.as_str() {
            "-o" | "-s" | "-t" => {
                let n = take_nums(i + 1, 3);
                if n >= 1 {
                    let u = num(toks.get(i + 1)).unwrap_or(0.0);
                    let v = if n >= 2 { num(toks.get(i + 2)).unwrap_or(0.0) } else if opt == "-s" { 1.0 } else { 0.0 };
                    match opt.as_str() {
                        "-o" => spec.offset = [u, v],
                        "-s" => spec.scale = [u, if n >= 2 { v } else { u }],
                        _ => {}
                    }
                }
                i += 1 + n;
            }
            "-clamp" => {
                spec.clamp = toks.get(i + 1).is_some_and(|v| v.eq_ignore_ascii_case("on"));
                i += 2;
            }
            "-bm" => {
                spec.bump = num(toks.get(i + 1));
                i += 2;
            }
            "-mm" => i += 1 + take_nums(i + 1, 2),
            "-boost" | "-texres" => i += 2,
            "-blendu" | "-blendv" | "-cc" | "-imfchan" | "-type" => i += 2,
            // An option we do not know: its argument count is unknown, so the
            // file name cannot be found reliably.
            _ => return None,
        }
    }
    if i >= toks.len() {
        return None;
    }
    spec.file = toks[i..].join(" ");
    Some(spec)
}

#[derive(Default)]
struct MtlMaterial {
    kd: Option<[f32; 3]>,
    ke: Option<[f32; 3]>,
    alpha: Option<f32>,
    ns: Option<f32>,
    pr: Option<f32>,
    pm: Option<f32>,
    map_kd: Option<MapSpec>,
    map_ke: Option<MapSpec>,
    norm: Option<MapSpec>,
}

fn parse_mtl(text: &str, lib: &str, warnings: &mut Vec<String>) -> Vec<(String, MtlMaterial)> {
    let mut out: Vec<(String, MtlMaterial)> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let body = match raw.find('#') {
            Some(c) => &raw[..c],
            None => raw,
        }
        .trim();
        let mut toks = body.split_whitespace();
        let Some(kw) = toks.next() else { continue };
        let kw_lower = kw.to_ascii_lowercase();
        let rest = body[kw.len()..].trim();
        if kw_lower == "newmtl" {
            out.push((rest.to_string(), MtlMaterial::default()));
            continue;
        }
        let Some((_, m)) = out.last_mut() else { continue };
        let floats: Vec<f32> = rest.split_whitespace().filter_map(|t| t.parse::<f32>().ok()).filter(|v| v.is_finite()).collect();
        let rgb = |f: &[f32]| -> Option<[f32; 3]> {
            match f {
                [r, g, b, ..] => Some([*r, *g, *b]),
                [x] => Some([*x, *x, *x]),
                _ => None,
            }
        };
        match kw_lower.as_str() {
            "kd" => m.kd = rgb(&floats),
            "ke" => m.ke = rgb(&floats),
            "d" => m.alpha = floats.first().copied(),
            "tr" => m.alpha = floats.first().map(|t| 1.0 - t),
            "ns" => m.ns = floats.first().copied(),
            "pr" => m.pr = floats.first().copied(),
            "pm" => m.pm = floats.first().copied(),
            "map_kd" => m.map_kd = parse_map(rest),
            "map_ke" => m.map_ke = parse_map(rest),
            "norm" => m.norm = parse_map(rest),
            // Ambient, specular, illumination model and the like describe a
            // Phong surface the scene's metallic-roughness model does not have.
            "ka" | "ks" | "illum" | "ni" | "tf" | "map_ka" | "map_ks" | "map_ns" => {}
            "map_bump" | "bump" | "map_d" | "disp" | "decal" | "refl" | "map_pr" | "map_pm" | "map_ps"
            | "aniso" | "anisor" | "pc" | "pcr" | "ps" => {
                warnings.push(format!(
                    "{lib} line {}: `{kw}` (a {}) has no counterpart in the scene and was dropped",
                    i + 1,
                    match kw_lower.as_str() {
                        "map_bump" | "bump" => "height/bump map",
                        "map_d" => "separate alpha map",
                        "disp" => "displacement map",
                        _ => "material property",
                    }
                ));
            }
            _ => {}
        }
    }
    out
}

/// Turn an MTL `-o`/`-s` pair (OBJ's V-up UV space) into the scene's
/// `KHR_texture_transform` (V-down). In V-up space the map samples
/// `uv * s + o`; with `v_down = 1 - v_up` that becomes offset
/// `(o_u, 1 - s_v - o_v)` and scale `s`.
fn mtl_to_transform(m: &MapSpec) -> Option<UvTransform> {
    let t = UvTransform {
        offset: [m.offset[0], 1.0 - m.scale[1] - m.offset[1]],
        rotation: 0.0,
        scale: m.scale,
    };
    (!t.is_identity()).then_some(t)
}

fn transform_to_mtl(t: &UvTransform) -> ([f32; 2], [f32; 2]) {
    // Inverse of `mtl_to_transform`. Rotation has no MTL spelling; `prepare`
    // reports it as a loss.
    ([t.offset[0], 1.0 - t.scale[1] - t.offset[1]], t.scale)
}

/// Resolve a texture statement into the scene, through the sidecars.
fn load_map(
    scene: &mut Scene,
    images: &mut HashMap<String, Option<usize>>,
    spec: &MapSpec,
    lib_path: &str,
    sidecars: &dyn Sidecars,
    warnings: &mut Vec<String>,
) -> Option<TextureRef> {
    let Some(path) = package::resolve_relative(lib_path, &spec.file) else {
        warnings.push(format!("texture {:?} points outside the model's files; dropped", spec.file));
        return None;
    };
    let image = match images.get(&path) {
        Some(i) => *i,
        None => {
            let loaded = match sidecars.get(&path) {
                None => {
                    warnings.push(format!("texture {path:?} was not supplied; dropped"));
                    None
                }
                Some(bytes) => match ImageFormat::sniff(&bytes) {
                    Some(format) => {
                        let name = path.rsplit('/').next().and_then(|f| f.rsplit_once('.')).map(|(s, _)| s.to_string());
                        scene.images.push(Image { name, format, data: bytes });
                        Some(scene.images.len() - 1)
                    }
                    None => {
                        warnings.push(format!("texture {path:?} is not PNG, JPEG, WebP or KTX2; dropped"));
                        None
                    }
                },
            };
            images.insert(path.clone(), loaded);
            loaded
        }
    }?;
    let wrap = if spec.clamp { Wrap::ClampToEdge } else { Wrap::Repeat };
    scene.textures.push(Texture {
        name: scene.images[image].name.clone(),
        image,
        sampler: Sampler { wrap_s: wrap, wrap_t: wrap, ..Default::default() },
    });
    Some(TextureRef { texture: scene.textures.len() - 1, uv_set: 0, transform: mtl_to_transform(spec) })
}

/// Read an OBJ (and the `.mtl` files and textures it names, from `sidecars`).
pub fn read_scene(bytes: &[u8], sidecars: &dyn Sidecars) -> Result<SceneRead, MeshError> {
    let parsed = parse_obj(bytes)?;
    let mut warnings = Vec::new();
    let mut scene = Scene { name: parsed.first_name.clone(), ..Default::default() };

    // Materials, by name, from every library the file names.
    let mut library: Vec<(String, MtlMaterial, String)> = Vec::new();
    for lib in &parsed.libraries {
        let Some(path) = package::normalize_relative(lib) else {
            warnings.push(format!("material library {lib:?} points outside the model's files; ignored"));
            continue;
        };
        match sidecars.get(&path) {
            None => warnings.push(format!("material library {path:?} was not supplied; materials are untextured defaults")),
            Some(b) => match String::from_utf8(b) {
                Ok(text) => {
                    for (name, m) in parse_mtl(&text, &path, &mut warnings) {
                        library.push((name, m, path.clone()));
                    }
                }
                Err(_) => warnings.push(format!("material library {path:?} is not UTF-8; ignored")),
            },
        }
    }
    let mut material_index: HashMap<String, usize> = HashMap::new();
    let mut images: HashMap<String, Option<usize>> = HashMap::new();
    let mut material_for = |scene: &mut Scene, name: &str, warnings: &mut Vec<String>| -> usize {
        if let Some(i) = material_index.get(name) {
            return *i;
        }
        let mut mat = Material { name: Some(name.to_string()), metallic: 0.0, roughness: 1.0, ..Default::default() };
        match library.iter().find(|(n, _, _)| n == name) {
            None => {
                if !parsed.libraries.is_empty() {
                    warnings.push(format!("material {name:?} is not defined in any supplied library"));
                }
            }
            Some((_, m, lib)) => {
                if let Some(kd) = m.kd {
                    mat.base_color = [srgb_to_linear(kd[0]), srgb_to_linear(kd[1]), srgb_to_linear(kd[2]), 1.0];
                }
                if let Some(a) = m.alpha {
                    let a = a.clamp(0.0, 1.0);
                    mat.base_color[3] = a;
                    if a < 1.0 {
                        mat.alpha_mode = AlphaMode::Blend;
                    }
                }
                if let Some(ke) = m.ke {
                    mat.emissive = [srgb_to_linear(ke[0]), srgb_to_linear(ke[1]), srgb_to_linear(ke[2])];
                }
                // PBR extension first; otherwise Blender's specular-exponent
                // mapping (`Ns = (1 - roughness)^2 * 1000`).
                mat.roughness = match (m.pr, m.ns) {
                    (Some(pr), _) => pr.clamp(0.0, 1.0),
                    (None, Some(ns)) => (1.0 - (ns.clamp(0.0, 1000.0) / 1000.0).sqrt()).clamp(0.0, 1.0),
                    (None, None) => 1.0,
                };
                mat.metallic = m.pm.unwrap_or(0.0).clamp(0.0, 1.0);
                if let Some(spec) = &m.map_kd {
                    mat.base_color_texture = load_map(scene, &mut images, spec, lib, sidecars, warnings);
                }
                if let Some(spec) = &m.map_ke {
                    mat.emissive_texture = load_map(scene, &mut images, spec, lib, sidecars, warnings);
                    if mat.emissive == [0.0; 3] {
                        // A map with no factor means the map itself is the glow.
                        mat.emissive = [1.0; 3];
                    }
                }
                if let Some(spec) = &m.norm {
                    mat.normal_texture = load_map(scene, &mut images, spec, lib, sidecars, warnings);
                    mat.normal_scale = spec.bump.unwrap_or(1.0);
                }
            }
        }
        scene.materials.push(mat);
        let i = scene.materials.len() - 1;
        material_index.insert(name.to_string(), i);
        i
    };

    for obj in parsed.objects {
        let mut mesh = SceneMesh { name: obj.name.clone(), ..Default::default() };
        for p in obj.prims {
            if p.indices.is_empty() {
                continue;
            }
            let material = p.material.as_deref().map(|n| material_for(&mut scene, n, &mut warnings));
            let (has_vt, has_vn) = p.layout.unwrap_or((false, false));
            mesh.primitives.push(Primitive {
                positions: p.positions,
                normals: has_vn.then_some(p.normals),
                uvs: if has_vt { vec![p.uvs] } else { Vec::new() },
                colors: parsed.has_colors.then_some(p.colors),
                indices: p.indices,
                material,
                ..Default::default()
            });
        }
        if mesh.primitives.is_empty() {
            continue;
        }
        scene.meshes.push(mesh);
        let mi = scene.meshes.len() - 1;
        scene.add_root(Node { name: obj.name, mesh: Some(mi), ..Default::default() });
    }
    super::pose::prune_unused_textures(&mut scene);
    scene.validate()?;
    Ok(SceneRead { scene, warnings })
}

/// Read an OBJ into the flat mesh (no sidecars: materials are not resolved).
///
/// The flat mesh's one colour comes from the first vertex's colour when the
/// file carries vertex colour — OBJ has no material colour without its `.mtl`.
pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    let r = read_scene(bytes, &NoSidecars)?;
    let mut m = super::pose::flatten(&r.scene)?;
    if m.name.is_none() {
        // An `o` with no faces still names the (empty) model.
        m.name = r.scene.name.clone();
    }
    if m.base_color.is_none() {
        m.base_color = r.scene.meshes.iter().flat_map(|mm| &mm.primitives).find_map(|p| p.colors.as_ref()?.first().copied());
    }
    Ok(m)
}

// ─── writer ─────────────────────────────────────────────────────────────────

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
/// the scene held real geometry, not a guess at what the caller meant.
fn finite(v: [f32; 3], what: &str, i: usize) -> Result<(), MeshError> {
    if v.iter().any(|c| !c.is_finite()) {
        return Err(MeshError::Malformed(format!(
            "obj write: {what} {i} is not finite ({v:?})"
        )));
    }
    Ok(())
}

/// Make a name safe for an `o`/`usemtl`/`newmtl` line.
///
/// Those records run to the end of the line, so spaces would be fine for `o`,
/// but a newline would terminate the record and turn the rest of the name into
/// a bogus keyword line, and `#` would comment out everything after it.
/// Backslash is mangled because OBJ treats a trailing `\` as a line continuation
/// — a Windows path used as a glTF node name is exactly this shape, and it once
/// produced a file our own reader then rejected. Material names also lose their
/// whitespace: `usemtl`/`newmtl` readers disagree about names with spaces.
/// Trim BEFORE sanitising, or trimming a trailing space re-exposes a backslash
/// the sanitiser just neutralised.
fn safe_name(raw: &str, allow_spaces: bool) -> String {
    raw.trim()
        .chars()
        .map(|c| {
            if c == '#' || c == '\\' || c.is_control() || (!allow_spaces && c.is_whitespace()) {
                '_'
            } else {
                c
            }
        })
        .collect()
}

fn unique(name: String, used: &mut Vec<String>) -> String {
    let mut candidate = name.clone();
    let mut n = 1;
    while used.contains(&candidate) {
        candidate = format!("{name}_{n}");
        n += 1;
    }
    used.push(candidate.clone());
    candidate
}

/// Specular exponent for a roughness, Blender's mapping (the inverse of the
/// reader's).
fn roughness_to_ns(r: f32) -> f32 {
    let s = 1.0 - r.clamp(0.0, 1.0);
    s * s * 1000.0
}

fn srgb3(c: [f32; 3]) -> [f32; 3] {
    [linear_to_srgb(c[0].clamp(0.0, 1.0)), linear_to_srgb(c[1].clamp(0.0, 1.0)), linear_to_srgb(c[2].clamp(0.0, 1.0))]
}

/// Write a scene already reduced to OBJ's capabilities (`MeshFormat::prepare`):
/// every primitive in world space, at identity roots.
///
/// `self_contained` writes `model.obj` alone; otherwise `model.mtl` and the
/// textures come with it, named so that the references resolve when the files
/// sit side by side.
pub fn write_scene(scene: &Scene, self_contained: bool) -> Result<Package, MeshError> {
    scene.validate()?;
    let prims: Vec<(&SceneMesh, &Primitive)> =
        scene.meshes.iter().flat_map(|m| m.primitives.iter().map(move |p| (m, p))).collect();
    let any_colors = prims.iter().any(|(_, p)| p.colors.is_some());
    let mut out = String::with_capacity(
        prims.iter().map(|(_, p)| p.positions.len() * 3 + p.triangle_count()).sum::<usize>() * 24 + 128,
    );
    put(&mut out, format_args!("# exported by litegen mesh conversion\n"));

    let write_mtl = !self_contained && !scene.materials.is_empty();
    let mut files: Vec<PackageFile> = Vec::new();
    let mut material_names: Vec<String> = Vec::new();
    if write_mtl {
        put(&mut out, format_args!("mtllib model.mtl\n"));
        let mut used: Vec<String> = Vec::new();
        for (i, m) in scene.materials.iter().enumerate() {
            let base = safe_name(m.name.as_deref().unwrap_or(""), false);
            let base = if base.is_empty() { format!("material_{i}") } else { base };
            material_names.push(unique(base, &mut used));
        }
        // OBJ's material state is sticky: a group with no `usemtl` of its own
        // inherits the previous one (three.js's OBJLoader does exactly that),
        // so a part with no material after a red part would render red. Every
        // primitive therefore names a material, and the ones without get a
        // plain white one.
        if prims.iter().any(|(_, p)| p.material.is_none()) {
            let name = unique("default".to_string(), &mut used);
            material_names.push(name);
        }
        let (mtl, textures) = write_mtl_text(scene, &material_names)?;
        files.push(PackageFile { path: "model.mtl".into(), bytes: mtl.into_bytes() });
        files.extend(textures);
    }

    // OBJ indices are global across the file, so each primitive's indices are
    // offset by the vertices written before it.
    let (mut base_v, mut base_vt, mut base_vn) = (0usize, 0usize, 0usize);
    let mut used_objects: Vec<String> = Vec::new();
    let mut last_mesh: Option<*const SceneMesh> = None;
    for (mi, (mesh, p)) in prims.iter().enumerate() {
        // One `o` per mesh: a multi-material mesh keeps its primitives under one
        // object, which is how the reader rebuilds it.
        if last_mesh != Some(*mesh as *const SceneMesh) {
            last_mesh = Some(*mesh as *const SceneMesh);
            let raw = mesh.name.as_deref().unwrap_or("");
            let safe = safe_name(raw, true);
            if !safe.is_empty() || prims.len() > 1 {
                let safe = if safe.is_empty() { format!("object_{mi}") } else { safe };
                let name = unique(safe, &mut used_objects);
                put(&mut out, format_args!("o {name}\n"));
            }
        }
        // Float formatting: Rust's `Display` for `f32` emits the shortest
        // decimal that parses back to the identical bit pattern — `0.1` stays
        // `0.1`, and a value that genuinely needs 9 digits gets 9.
        for (i, v) in p.positions.iter().enumerate() {
            finite(*v, "position", i)?;
            if any_colors {
                // Every `v` line carries colour or none does (the reader
                // refuses a mix), so a primitive without colour writes white —
                // the colour that changes nothing.
                let c = p.colors.as_ref().map_or([1.0; 3], |cs| [cs[i][0], cs[i][1], cs[i][2]]);
                let c = srgb3(c);
                put(&mut out, format_args!("v {} {} {} {} {} {}\n", v[0], v[1], v[2], c[0], c[1], c[2]));
            } else {
                put(&mut out, format_args!("v {} {} {}\n", v[0], v[1], v[2]));
            }
        }
        let uvs = p.uvs.first();
        if let Some(uvs) = uvs {
            for (i, t) in uvs.iter().enumerate() {
                finite([t[0], t[1], 0.0], "uv", i)?;
                // Undo the V flip taken on read; see the `vt` arm there.
                put(&mut out, format_args!("vt {} {}\n", t[0], 1.0 - t[1]));
            }
        }
        if let Some(normals) = &p.normals {
            for (i, n) in normals.iter().enumerate() {
                finite(*n, "normal", i)?;
                put(&mut out, format_args!("vn {} {} {}\n", n[0], n[1], n[2]));
            }
        }
        if write_mtl {
            // The default material is the extra name after the scene's own.
            let m = p.material.unwrap_or(scene.materials.len());
            put(&mut out, format_args!("usemtl {}\n", material_names[m]));
        }
        // The scene is per-vertex, so one index is the position, texture and
        // normal index at once — which is also why an OBJ this writer produced
        // reads back with the vertex count it was written from.
        for tri in p.indices.chunks_exact(3) {
            out.push('f');
            for idx in tri {
                let i = *idx as usize;
                let (v, t, n) = (base_v + i + 1, base_vt + i + 1, base_vn + i + 1);
                match (uvs.is_some(), p.normals.is_some()) {
                    (true, true) => put(&mut out, format_args!(" {v}/{t}/{n}")),
                    (true, false) => put(&mut out, format_args!(" {v}/{t}")),
                    (false, true) => put(&mut out, format_args!(" {v}//{n}")),
                    (false, false) => put(&mut out, format_args!(" {v}")),
                }
            }
            out.push('\n');
        }
        base_v += p.positions.len();
        if uvs.is_some() {
            base_vt += p.positions.len();
        }
        if p.normals.is_some() {
            base_vn += p.positions.len();
        }
    }

    let mut pkg = Package::single("model.obj", out.into_bytes());
    pkg.files.extend(files);
    Ok(pkg)
}

fn write_mtl_text(scene: &Scene, names: &[String]) -> Result<(String, Vec<PackageFile>), MeshError> {
    let mut mtl = String::new();
    put(&mut mtl, format_args!("# exported by litegen mesh conversion\n"));
    // Textures as files beside the .mtl, one per image, named after it.
    let mut image_files: Vec<Option<String>> = vec![None; scene.images.len()];
    let mut files = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for (i, im) in scene.images.iter().enumerate() {
        let stem = package::safe_file_stem(im.name.as_deref().unwrap_or(""), &format!("texture_{i}"));
        let name = unique(format!("textures/{stem}.{}", im.format.extension()), &mut used);
        files.push(PackageFile { path: name.clone(), bytes: im.data.clone() });
        image_files[i] = Some(name);
    }
    let map_line = |kw: &str, r: &TextureRef, extra: &str| -> Option<String> {
        let tex = scene.textures.get(r.texture)?;
        let file = image_files.get(tex.image)?.as_ref()?;
        let mut opts = String::new();
        if let Some(t) = &r.transform {
            let (o, s) = transform_to_mtl(t);
            // Three values each: three.js's MTLLoader consumes exactly three.
            opts.push_str(&format!("-o {} {} 0 -s {} {} 1 ", o[0], o[1], s[0], s[1]));
        }
        // No `-clamp`: three.js's MTLLoader does not know the option and reads
        // `-clamp on file.png` as the file name, losing the texture outright.
        // OBJ's capabilities say wrap modes do not survive; repeat is the
        // default everywhere.
        Some(format!("{kw} {extra}{opts}{file}\n"))
    };
    if names.len() > scene.materials.len() {
        // The default material for primitives that have none: white,
        // fully rough, as an absent material reads in every viewer.
        put(&mut mtl, format_args!("\nnewmtl {}\nKd 1 1 1\nKa 0 0 0\nKs 0 0 0\nNs 0\nPr 1\nPm 0\nillum 1\n", names[scene.materials.len()]));
    }
    for (m, name) in scene.materials.iter().zip(names) {
        put(&mut mtl, format_args!("\nnewmtl {name}\n"));
        let kd = srgb3([m.base_color[0], m.base_color[1], m.base_color[2]]);
        put(&mut mtl, format_args!("Kd {} {} {}\n", kd[0], kd[1], kd[2]));
        put(&mut mtl, format_args!("Ka 0 0 0\nKs 0.5 0.5 0.5\n"));
        put(&mut mtl, format_args!("Ns {}\n", roughness_to_ns(m.roughness)));
        // The PBR extension, for the importers that read it (Blender, Unity).
        put(&mut mtl, format_args!("Pr {}\nPm {}\n", m.roughness, m.metallic));
        if m.base_color[3] < 1.0 {
            put(&mut mtl, format_args!("d {}\n", m.base_color[3]));
        }
        if m.emissive != [0.0; 3] {
            let ke = srgb3(m.emissive);
            put(&mut mtl, format_args!("Ke {} {} {}\n", ke[0], ke[1], ke[2]));
        }
        put(&mut mtl, format_args!("illum 2\n"));
        if let Some(line) = m.base_color_texture.as_ref().and_then(|r| map_line("map_Kd", r, "")) {
            mtl.push_str(&line);
        }
        if let Some(line) = m.emissive_texture.as_ref().and_then(|r| map_line("map_Ke", r, "")) {
            mtl.push_str(&line);
        }
        if let Some(r) = &m.normal_texture {
            let bm = if m.normal_scale != 1.0 { format!("-bm {} ", m.normal_scale) } else { String::new() };
            if let Some(line) = map_line("norm", r, &bm) {
                mtl.push_str(&line);
            }
        }
    }
    Ok((mtl, files))
}

/// Write the flat mesh as one self-contained OBJ. Its colour, if any, is baked
/// into the vertices (see the module docs).
pub fn write(mesh: &Mesh) -> Result<Vec<u8>, MeshError> {
    // `mesh::write` validates too, but `obj::write` is also called directly, and
    // an out-of-range index here would emit a file that silently references a
    // vertex that does not exist.
    mesh.validate()?;
    let opts = super::WriteOptions { self_contained: true };
    let scene = MeshFormat::Obj.prepare(&super::pose::from_flat(mesh), opts)?;
    Ok(write_scene(&scene, true)?.files.remove(0).bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::package::MemorySidecars;

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
    fn a_single_file_carries_colour_as_vertex_colour_not_a_material_reference() {
        let mut m = parse(QUAD).expect("read");
        m.base_color = Some([1.0, 0.0, 0.0, 1.0]);
        let s = text(&m);
        // No sidecar reference: one file must be the whole model, and a
        // reference to a `.mtl` that is not there reads as a broken file.
        assert!(!s.contains("mtllib"), "{s}");
        assert!(!s.contains("usemtl"), "{s}");
        // The colour rides on the vertices instead, and comes back.
        assert!(s.contains("v 0 0 0 1 0 0\n"), "{s}");
        assert_eq!(parse(&s).expect("re-read").base_color, Some([1.0, 0.0, 0.0, 1.0]));
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
            MeshError::Malformed(d) => assert!(d.contains("3, 4 or 6"), "{d}"),
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
    fn reads_per_vertex_colour_as_srgb() {
        let src = "v 0 0 0 1 0 0\nv 1 0 0 0 0.5 0\nv 0 1 0 0 0 1\nf 1 2 3\n";
        let r = read_scene(src.as_bytes(), &NoSidecars).expect("read");
        let c = r.scene.meshes[0].primitives[0].colors.clone().expect("colours");
        assert_eq!(c[0], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(c[1], [0.0, srgb_to_linear(0.5), 0.0, 1.0], "sRGB 0.5 is linear 0.214");
        assert_eq!(c[2], [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn rejects_colour_on_only_some_vertices() {
        match parse("v 0 0 0 1 0 0\nv 1 0 0\nv 0 1 0 1 0 0\nf 1 2 3\n").unwrap_err() {
            MeshError::Unsupported(d) => assert!(d.contains("only some"), "{d}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn rejects_an_ambiguous_seven_component_vertex() {
        assert!(matches!(
            parse("v 0 0 0 1 1 0 0\nv 1 0 0 1 1 0 0\nv 0 1 0 1 1 0 0\nf 1 2 3\n").unwrap_err(),
            MeshError::Unsupported(_)
        ));
    }

    // ---- objects, materials and the .mtl sidecar ---------------------------

    fn sidecars(files: &[(&str, &[u8])]) -> Vec<PackageFile> {
        files.iter().map(|(p, b)| PackageFile { path: p.to_string(), bytes: b.to_vec() }).collect()
    }

    const TWO_OBJECTS: &str = "\
mtllib lib/materials.mtl
o left
v 0 0 0
v 1 0 0
v 0 1 0
v 1 1 0
vt 0 0
vt 1 0
vt 0 1
vt 1 1
usemtl red
f 1/1 2/2 3/3
usemtl textured
f 2/2 4/4 3/3
o right
v 5 0 0
v 6 0 0
v 5 1 0
usemtl red
f 5 6 7
";

    #[test]
    fn objects_become_meshes_and_materials_become_primitives() {
        let mtl = "newmtl red\nKd 1 0.5 0\nd 0.5\nPr 0.25\nPm 1\n\
                   newmtl textured\nKd 1 1 1\nmap_Kd -o 0.25 0 0 -s 0.5 0.5 1 ../tex/a b.png\n";
        let png = super::super::fixtures::quadrant_png();
        let files = sidecars(&[("lib/materials.mtl", mtl.as_bytes()), ("tex/a b.png", &png)]);
        let r = read_scene(TWO_OBJECTS.as_bytes(), &MemorySidecars { files: &files }).expect("read");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let s = &r.scene;
        assert_eq!(s.meshes.len(), 2);
        assert_eq!(s.meshes[0].name.as_deref(), Some("left"));
        assert_eq!(s.meshes[0].primitives.len(), 2, "one primitive per material");
        assert_eq!(s.meshes[1].primitives.len(), 1);
        let red = &s.materials[s.meshes[0].primitives[0].material.unwrap()];
        assert_eq!(red.name.as_deref(), Some("red"));
        // Kd is sRGB: 0.5 decodes to linear 0.214.
        assert_eq!(red.base_color, [1.0, srgb_to_linear(0.5), 0.0, 0.5]);
        assert_eq!(red.alpha_mode, AlphaMode::Blend);
        assert_eq!((red.roughness, red.metallic), (0.25, 1.0));
        // The second object reuses the same material rather than duplicating it.
        assert_eq!(s.meshes[1].primitives[0].material, s.meshes[0].primitives[0].material);
        let tex = &s.materials[s.meshes[0].primitives[1].material.unwrap()];
        let tr = tex.base_color_texture.as_ref().expect("texture resolved relative to the .mtl");
        assert_eq!(s.images[s.textures[tr.texture].image].data, png);
        // -o/-s in OBJ's V-up space become KHR_texture_transform in V-down.
        let t = tr.transform.expect("transform");
        assert_eq!(t.scale, [0.5, 0.5]);
        assert_eq!(t.offset, [0.25, 0.5]);
    }

    #[test]
    fn a_missing_library_or_texture_is_a_warning_not_a_failure() {
        let r = read_scene(TWO_OBJECTS.as_bytes(), &NoSidecars).expect("read");
        assert_eq!(r.scene.triangle_count(), 3);
        assert!(r.warnings.iter().any(|w| w.contains("materials.mtl")), "{:?}", r.warnings);
    }

    #[test]
    fn texture_paths_cannot_escape_the_package() {
        let mtl = "newmtl m\nmap_Kd ../../../etc/passwd\n";
        let files = sidecars(&[("lib/materials.mtl", mtl.as_bytes())]);
        let src = "mtllib lib/materials.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl m\nf 1 2 3\n";
        let r = read_scene(src.as_bytes(), &MemorySidecars { files: &files }).expect("read");
        assert!(r.scene.images.is_empty());
        assert!(r.warnings.iter().any(|w| w.contains("outside")), "{:?}", r.warnings);
    }

    #[test]
    fn a_package_round_trips_materials_and_textures() {
        let scene = super::super::fixtures::textured();
        let opts = super::super::WriteOptions { self_contained: false };
        let prepared = MeshFormat::Obj.prepare(&scene, opts).expect("prepare");
        let pkg = write_scene(&prepared, false).expect("write");
        let names: Vec<&str> = pkg.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names[0], "model.obj");
        assert!(names.contains(&"model.mtl"), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("textures/") && n.ends_with(".png")), "{names:?}");
        let back = read_scene(&pkg.files[0].bytes, &pkg.resolver()).expect("re-read");
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        assert_eq!(back.scene.triangle_count(), prepared.triangle_count());
        assert_eq!(back.scene.images.len(), 1);
        assert_eq!(back.scene.images[0].data, prepared.images[0].data);
        // Materials survive by value (through an sRGB round trip, so within a
        // quantisation step).
        for (a, b) in back.scene.materials.iter().zip(prepared.materials.iter()) {
            for k in 0..4 {
                assert!((a.base_color[k] - b.base_color[k]).abs() < 1e-4, "{a:?} vs {b:?}");
            }
            assert_eq!(a.base_color_texture.is_some(), b.base_color_texture.is_some());
        }
    }

    #[test]
    fn a_part_without_a_material_does_not_inherit_the_previous_one() {
        // OBJ's `usemtl` is sticky; without an explicit default, the second
        // part renders in the first part's red.
        let mut scene = super::super::fixtures::cube();
        scene.materials[0].base_color = [1.0, 0.0, 0.0, 1.0];
        let mut second = scene.meshes[0].clone();
        second.primitives[0].material = None;
        second.name = Some("plain".into());
        scene.meshes.push(second);
        scene.add_root(super::super::scene::Node { mesh: Some(1), ..Default::default() });
        let pkg = write_scene(&scene, false).expect("write");
        let obj = String::from_utf8(pkg.files[0].bytes.clone()).unwrap();
        assert_eq!(obj.matches("usemtl").count(), 2, "{obj}");
        let back = read_scene(&pkg.files[0].bytes, &pkg.resolver()).expect("read");
        let colors: Vec<[f32; 4]> = back
            .scene
            .meshes
            .iter()
            .flat_map(|m| &m.primitives)
            .map(|p| back.scene.materials[p.material.unwrap()].base_color)
            .collect();
        assert_eq!(colors, vec![[1.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]]);
    }

    #[test]
    fn a_self_contained_file_bakes_material_colour_into_vertices() {
        let scene = super::super::fixtures::textured();
        let opts = super::super::WriteOptions { self_contained: true };
        let prepared = MeshFormat::Obj.prepare(&scene, opts).expect("prepare");
        let pkg = write_scene(&prepared, true).expect("write");
        assert_eq!(pkg.files.len(), 1, "one file, nothing beside it");
        let s = String::from_utf8(pkg.files[0].bytes.clone()).unwrap();
        assert!(!s.contains("mtllib"));
        let back = read_scene(s.as_bytes(), &NoSidecars).expect("re-read");
        // The wedge is orange: some vertex must carry its colour.
        let orange = srgb_to_linear(linear_to_srgb(0.3));
        assert!(back.scene.meshes.iter().flat_map(|m| &m.primitives).any(|p| p
            .colors
            .as_ref()
            .is_some_and(|c| c.iter().any(|c| (c[1] - orange).abs() < 1e-3 && c[0] > 0.8))));
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
