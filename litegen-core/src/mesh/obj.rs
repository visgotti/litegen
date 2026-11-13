//! Wavefront OBJ (+ MTL) — the plain-text container.
//!
//! OBJ is the format everyone can open and nobody implements the same way, so
//! this reader is deliberately narrow: the polygonal subset (`v`, `vt`, `vn`,
//! `f`, `l`, `p`, `o`, `usemtl`, `mtllib`) and nothing else. What it does accept it
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
//! # Points and lines
//!
//! `p v1 v2 …` is a point for each vertex it names, and `l v1 v2 …` the
//! polyline through its vertices: a segment for each pair, in order. Each
//! joins the primitive of its own kind for the material in force, beside the
//! object's faces, and is charged to the same index budget as a face's
//! triangles before it is built (two indices a segment, one a point). An `l`
//! may spell its vertices `v/vt`, which is read as a face corner's is (a
//! position, and the texture coordinate it is drawn with; the same position
//! with another `vt` is another vertex); the other spellings are read too.
//!
//! The writer writes one `l a b` per segment, never a polyline: three.js's
//! `OBJLoader` pushes a polyline's vertices straight into `LineSegments`, so
//! `l 1 2 3 4` would draw 1–2 and 3–4 and lose 2–3. And it starts an object
//! for each run of one topology, because one `l` or `p` turns the whole
//! object it is in, faces included, into lines or points there. A file with
//! no MTL also gives each kind a `usemtl` of its own (`faces`, `lines`,
//! `points`): without an MTL the loader caches a material by its name, and
//! the first object's kind of material wins. That holds for one file, and
//! for a package whose scene has no materials, which `prepare` gives one
//! white material per kind present and so writes an MTL for, where it wrote none.
//! Neither statement has a place for a normal, and `p` none for a texture
//! coordinate either, so `prepare` leaves them out and [`losses`] names them.
//! A point with no coordinate has nothing to sample a map by (three.js
//! smears the map across each point sprite, and glTF refuses a map on a
//! primitive without `TEXCOORD`), so a package draws a points primitive with
//! an untextured copy of its material (`paint_1` beside `paint`, factors
//! kept, no map), or strips the material itself where only points use it,
//! and [`losses`] names those bindings as `textures`. Research Q6 of
//! `docs/superpowers/research/research-consumers.md` has the measurements.
//!
//! # Material layers: Blender's PBR extension to MTL
//!
//! A package's MTL holds four of glTF's material layers through the PBR
//! extension Blender reads and writes. Each key means what it means to
//! Blender, in Blender's units: its importer and exporter
//! (`source/blender/io/wavefront_obj/importer/obj_import_mtl.cc` and
//! `exporter/obj_export_mtl.cc`), and its Principled BSDF's sockets
//! (`source/blender/nodes/shader/nodes/node_shader_bsdf_principled.cc`), all
//! at v5.2.3 on github.com/blender/blender.
//!
//! - **`Pc`** is four times the clearcoat factor, Blender's Coat Weight. The
//!   exporter writes `coat *= 4.0f` ("Clearcoat used to include an implicit
//!   0.25 factor, so stay compatible to old versions", obj_export_mtl.cc:
//!   258-259), and the importer reads `0.25f * mtl_mat.cc_thickness`
//!   (obj_import_mtl.cc:353-356). Blender's glTF add-on maps
//!   clearcoatFactor to Coat Weight one to one
//!   (`scripts/addons_core/io_scene_gltf2/blender/imp/pbrMetallicRoughness.py:205-206`).
//! - **`Pcr`** is the clearcoat roughness, Coat Roughness (:357-359).
//! - **`Ps`** of one value is Sheen Weight (:350-352), under the Sheen Tint
//!   the importer leaves white (principled :286-287): a grey sheen colour.
//!   Three values are the colour itself, as assimp reads them
//!   (`code/AssetLib/Obj/ObjFileMtlImporter.cpp:227-230`, v6.0.5). The
//!   writer writes the brightest channel as a grey triple, `Ps w w w`.
//!   Blender reads its first number (`parse_float`,
//!   `source/blender/io/common/intern/string_utils.cc:120-141`), and assimp
//!   all three, where one value would read as (w, 0, 0), a red sheen
//!   (`getColorRGBA`, ObjFileMtlImporter.cpp:373-383). MTL has no sheen
//!   roughness.
//! - **`aniso`** is the anisotropy strength, Anisotropic, and **`anisor`**
//!   its rotation, Anisotropic Rotation (:360-365), a fraction of a turn:
//!   "Rotates the direction of anisotropy, with 1.0 going full circle"
//!   (principled :201-206). glTF's anisotropyRotation is 2π times it in
//!   radians, as Blender's own glTF add-on converts
//!   (`scripts/addons_core/io_scene_gltf2/blender/com/conversion.py:203-210`).
//!   It is written as an f64, so a rotation reads back to the bit.
//! - **`Ni`** is the ior, but in a PBR MTL only: one with any of `Pr`, `Pm`,
//!   `Ps`, `Pc`, `Pcr`, `aniso`, `anisor`, `map_Pr` or `map_Pm`. Classic
//!   exporters write `Ni 1` by default, and an ior of 1 takes a dielectric's
//!   reflection away, so a classic MTL's `Ni` stays unread. Blender reads it
//!   on every material, ignoring 0 and below ("Some files have `Ni 0`,
//!   ignore those values", obj_import_mtl.cc:338-341). Here 0 and below are
//!   left out with a warning, and the writer leaves an ior of 0 out and says
//!   so. Blender's own defaults are unset too, as Collada's exporter defaults
//!   are (see `dae`). Blender writes `Ni` on every material
//!   (exporter/obj_export_file_writer.cc:656): 1 for a material without a
//!   Principled BSDF (`float refraction_index = 1.0f`, obj_export_mtl.cc:
//!   220-223), and otherwise the Principled IOR. That defaults to 1.45 in
//!   2.8-3.6 (principled :83, v3.6.23), where it drew transmission only
//!   ("IOR: Index of refraction for transmission", Blender 3.6's manual,
//!   docs.blender.org/manual/en/3.6/render/shader_nodes/shader/principled.html),
//!   and to 1.5 from 4.0 (principled :61-63), glTF's own default. So 1, 1.45
//!   and 1.5 read as unset. The writer leaves an ior of 1 or 1.45 out and
//!   names it, and leaves 1.5 out unnamed, as it draws the same.
//! - **`Tf`** in a PBR MTL is Blender's Transmission Weight, `Tf t t t`
//!   (obj_export_mtl.cc:249, :256, :304; obj_export_file_writer.cc:684-688),
//!   read back as the three's mean (obj_import_mtl.cc:367-373). So is
//!   **`Kt`**, which Blender's reader takes as `Tf`
//!   (importer/obj_import_file_reader.cc:967). OBJ keeps no transmission, so
//!   a weight above 0 is named in a warning and not carried. A classic MTL's
//!   `Tf` or `Kt`, a transmission filter colour, stays unread, as before.
//!
//! A layer exists above its weight's off value: a clearcoat when `Pc` / 4 is
//! in (0, 1], a sheen when its brightest `Ps` is, an anisotropy when `aniso`
//! is. Blender writes these keys in its PBR mode, which is off by default
//! (`export_pbr_extensions`, `source/blender/editors/io/io_obj.cc:360-365`).
//! There it writes every key, zeros included
//! (exporter/obj_export_file_writer.cc:656-683), except `Pr`, `Pm` and `Ps`
//! where a map takes their place (:663, :666, :669). A coat weight of 0 is no
//! coat. A `Pcr` or `anisor` without its weight adds nothing. What the file
//! leaves unsaid is Blender's default, so the scene draws what Blender draws:
//! a coat roughness of 0.03 (principled :236-237), a sheen roughness of 0.5
//! (:276-277) and an anisotropic rotation of 0 (:201-202). A value glTF's
//! field can't hold (`Pc` outside 0 to 4, the rest outside 0 to 1), one that
//! is no number, or a `Ps` of neither one value nor three is dropped with a
//! warning, never clamped.
//!
//! litegen writes a layer's factors alone. The extension has no clearcoat or
//! anisotropy maps, and its sheen map, `map_Ps`, holds a weight, not glTF's
//! colour (obj_export_file_writer.cc:561-567, :717); the reader drops one
//! with a warning. [`prepare`] drops the layer maps, the clearcoat's normal
//! map among them, before choosing OBJ's one uv set. It also reduces a
//! coloured sheen to its brightest channel at Blender's sheen roughness, and
//! drops what the reader would refuse. [`losses`] names each. One file has no
//! MTL, and no layers.
//!
//! # Colour
//!
//! OBJ and MTL predate colour management and say nothing about colour space.
//! three.js — the engine behind the dashboard's viewer — reads `Kd`/`Ke` and
//! the `v x y z r g b` vertex-colour extension as sRGB, as do MeshLab and
//! Unity; Blender reads `Kd` as linear. This codec follows the majority, and the
//! product's own viewer: linear scene colour is encoded to sRGB on write and
//! decoded on read.
//!
//! A read lists `MAX_WARNINGS` distinct warnings and counts the rest in a
//! last line, and each warning clips a name or path it quotes to
//! `MAX_QUOTED` characters (Ruling A45).
//!
//! Ruling IDs ("Ruling A21b") are mesh-gaps item 4's rulings, listed with
//! their costs in the status block of
//! `docs/superpowers/plans/2026-11-04-mesh-material-layers.md`.

use std::collections::{HashMap, HashSet};
use std::fmt;

use super::ir::Mesh;
use super::math::{linear_to_srgb, srgb_to_linear};
use super::package::{self, NoSidecars, Package, PackageFile, Sidecars};
use super::scene::{
    AlphaMode, Anisotropy, Clearcoat, Image, ImageFormat, Loss, LossKind, Material, MaterialLayer, Node, Primitive,
    Sampler, Scene, SceneMesh, Sheen, Texture, TextureRef, TextureSlot, Topology, UvTransform, Wrap,
};
use super::{MeshError, MeshFormat, SceneRead};

/// How far into the file we look for evidence that these bytes are not text at
/// all. A real OBJ's first kilobyte is a comment banner and some `v` lines; a
/// mislabelled GLB or binary PLY has structure there we can recognise.
const SNIFF_BYTES: usize = 1024;

/// `.mtl` files one OBJ may name. Real files name one; the bound keeps a
/// hostile `mtllib` line from turning into thousands of sidecar lookups.
const MAX_MATERIAL_LIBRARIES: usize = 16;

/// Distinct warnings a read lists; past this they are counted, not listed,
/// as threemf.rs counts its own (Ruling A45). An MTL can warn on every line,
/// for as many materials as the scene holds.
const MAX_WARNINGS: usize = 64;

/// Characters a warning keeps of a name or path it quotes from the files,
/// then "…" (Ruling A45): well past any real one.
const MAX_QUOTED: usize = 256;

/// A read's warnings, budgeted: each is charged before it is listed, and
/// one past [`MAX_WARNINGS`] is counted in a last line instead.
#[derive(Default)]
struct Warnings {
    list: Vec<String>,
    dropped: usize,
}

impl Warnings {
    fn push(&mut self, w: String) {
        if self.list.contains(&w) {
            return;
        }
        if self.list.len() < MAX_WARNINGS {
            self.list.push(w);
        } else {
            self.dropped += 1;
        }
    }

    fn into_vec(mut self) -> Vec<String> {
        if self.dropped > 0 {
            self.list.push(format!("... and {} more warning(s)", self.dropped));
        }
        self.list
    }
}

/// A name or path from the files, as a warning quotes it: at most
/// [`MAX_QUOTED`] characters, then "…".
fn clip(s: &str) -> String {
    match s.char_indices().nth(MAX_QUOTED) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

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

/// One primitive under construction: the faces, or the segments, or the
/// points of one object that use one material.
struct PrimBuild {
    material: Option<String>,
    topology: Topology,
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
    /// Which of `prims` holds each material's faces, lines and points (in
    /// that order): a lookup per statement, which a scan made quadratic
    /// across 80k `usemtl` lines. Indexed by [`Topology`] so that a lookup
    /// allocates nothing.
    by_material: HashMap<Option<String>, [Option<usize>; 3]>,
}

/// Everything the OBJ text itself describes, before materials are resolved.
struct Parsed {
    objects: Vec<ObjBuild>,
    /// Each `mtllib` statement's rest of line, as written (see `read_scene`).
    libraries: Vec<String>,
    /// Whether the `v` lines carried colour (all of them, or none).
    has_colors: bool,
    first_name: Option<String>,
    /// Concave faces too large to ear-clip, fanned instead.
    fanned_concave: usize,
}

/// The place of a topology in [`ObjBuild::by_material`]'s slots.
fn slot(topology: Topology) -> usize {
    match topology {
        Topology::Triangles => 0,
        Topology::Lines => 1,
        Topology::Points => 2,
    }
}

/// The primitive of `obj` that takes `topology` elements in `material`,
/// started when there is none yet: a mesh is the faces, the lines and the
/// points of one object, per material, as glTF's primitives are.
fn primitive_for(
    obj: &mut ObjBuild,
    material: &Option<String>,
    topology: Topology,
    total_prims: &mut usize,
    lineno: usize,
) -> Result<usize, MeshError> {
    let at = slot(topology);
    if let Some(pi) = obj.by_material.get(material).and_then(|slots| slots[at]) {
        return Ok(pi);
    }
    *total_prims += 1;
    if *total_prims > super::scene::MAX_NODES {
        return Err(unsupported(
            lineno,
            format_args!("more than {} object and material parts", super::scene::MAX_NODES),
        ));
    }
    obj.by_material.entry(material.clone()).or_default()[at] = Some(obj.prims.len());
    obj.prims.push(PrimBuild {
        material: material.clone(),
        topology,
        positions: Vec::new(),
        uvs: Vec::new(),
        normals: Vec::new(),
        colors: Vec::new(),
        indices: Vec::new(),
        cache: HashMap::new(),
        layout: None,
    });
    Ok(obj.prims.len() - 1)
}

/// OBJ's source arrays as the file stands at one statement: its three index
/// spaces, and the colours of its `v` lines when they carry any.
struct Sources<'a> {
    v: &'a [[f32; 3]],
    vc: &'a [[f32; 4]],
    vt: &'a [[f32; 2]],
    vn: &'a [[f32; 3]],
    colored: bool,
}

/// Give `prim` the vertices of one statement's `corners`, one per distinct
/// corner, and put their numbers in `slots`, in the order of the corners.
/// A primitive carries uvs and normals for all of its vertices or for none,
/// so the corners have to agree with the primitive's first.
fn intern_corners(
    prim: &mut PrimBuild,
    corners: &[Corner],
    src: &Sources,
    total_vertices: &mut usize,
    slots: &mut Vec<u32>,
    lineno: usize,
) -> Result<(), MeshError> {
    let noun = match prim.topology {
        Topology::Triangles => "face corner",
        Topology::Lines => "line vertex",
        Topology::Points => "point",
    };
    for c in corners {
        let shape = (c.vt.is_some(), c.vn.is_some());
        match prim.layout {
            None => prim.layout = Some(shape),
            // Real files do mix these (a textured group followed by an
            // untextured one). Within one material of one object we cannot:
            // the choice is inventing (0,0) for the corners that lack one —
            // silently wrong texturing — or saying we will not do it.
            Some(prev) if prev != shape => {
                return Err(unsupported(
                    lineno,
                    format_args!(
                        "{noun} has (uv, normal) = ({}, {}) but earlier corners of \
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
    // Interning is per corner, not per element, so a fan shares its hub vertex
    // instead of emitting it n-2 times.
    slots.clear();
    for c in corners {
        let idx = match prim.cache.get(c) {
            Some(existing) => *existing,
            None => {
                let idx = u32::try_from(prim.positions.len())
                    .map_err(|_| unsupported(lineno, "more than 2^32 vertices; the scene indexes with u32"))?;
                *total_vertices += 1;
                if *total_vertices > super::scene::MAX_SCENE_VERTICES {
                    return Err(unsupported(
                        lineno,
                        format_args!("more than {} vertices", super::scene::MAX_SCENE_VERTICES),
                    ));
                }
                // `resolve` already bounded every one of these, but the reads
                // go through `get` so that reordering the resolution later
                // cannot turn a logic slip into a panic on hostile input.
                let p = *src
                    .v
                    .get(c.v as usize)
                    .ok_or_else(|| malformed(lineno, "vertex index out of range after resolution"))?;
                prim.positions.push(p);
                if src.colored {
                    let col = *src
                        .vc
                        .get(c.v as usize)
                        .ok_or_else(|| malformed(lineno, "vertex colour out of range"))?;
                    prim.colors.push(col);
                }
                if want_vt {
                    let t = c
                        .vt
                        .and_then(|i| src.vt.get(i as usize))
                        .ok_or_else(|| malformed(lineno, "texture index out of range"))?;
                    prim.uvs.push(*t);
                }
                if want_vn {
                    let n = c
                        .vn
                        .and_then(|i| src.vn.get(i as usize))
                        .ok_or_else(|| malformed(lineno, "normal index out of range"))?;
                    prim.normals.push(*n);
                }
                prim.cache.insert(*c, idx);
                idx
            }
        };
        slots.push(idx);
    }
    Ok(())
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
    // A UTF-8 byte-order mark is an encoding signature, not text (Unicode
    // §23.8): left in, it glues itself to the first keyword and that whole
    // statement is skipped.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);

    // OBJ's three index spaces, in file order. These are the *source* arrays;
    // the scene's arrays are built from face corners, not from these.
    let mut src_v: Vec<[f32; 3]> = Vec::new();
    let mut src_vc: Vec<[f32; 4]> = Vec::new();
    let mut src_vt: Vec<[f32; 2]> = Vec::new();
    let mut src_vn: Vec<[f32; 3]> = Vec::new();
    // Whether the first `v` line carried colour; every other one must agree.
    let mut colored: Option<bool> = None;

    let mut objects: Vec<ObjBuild> = vec![ObjBuild { name: None, prims: Vec::new(), by_material: HashMap::new() }];
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
    // Counted across the whole file as it is read, so a refusal names the line
    // that crosses a ceiling and costs nothing past it.
    let (mut total_indices, mut total_prims) = (0usize, 0usize);
    let mut fanned_concave = 0usize;
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
                    // A fan of n corners is 3(n − 2) indices: refused as the
                    // corners arrive, not after a 60M-corner line is held.
                    if total_indices.saturating_add(corners.len().saturating_sub(1) * 3) > super::scene::MAX_SCENE_INDICES {
                        return Err(unsupported(
                            lineno,
                            format_args!("the faces pass {} indices", super::scene::MAX_SCENE_INDICES),
                        ));
                    }
                    corners.push(parse_corner(tok, lineno, src_v.len(), src_vt.len(), src_vn.len())?);
                }
                if corners.len() < 3 {
                    return Err(malformed(
                        lineno,
                        format_args!("face has {} corners; a polygon needs at least 3", corners.len()),
                    ));
                }
                let obj = objects.last_mut().ok_or_else(|| malformed(lineno, "no current object"))?;
                let pi = primitive_for(obj, &material, Topology::Triangles, &mut total_prims, lineno)?;
                let prim = &mut obj.prims[pi];
                let src = Sources { v: &src_v, vc: &src_vc, vt: &src_vt, vn: &src_vn, colored: colored == Some(true) };
                intern_corners(prim, &corners, &src, &mut total_vertices, &mut slots, lineno)?;
                // A polygon of n corners is n − 2 triangles, fanned when convex
                // and ear-clipped when not (`polygon`): a fan of a concave face
                // covers its notch and turns a triangle backwards. three.js's
                // OBJLoader fans; Blender keeps the n-gon, as authored.
                total_indices += (slots.len() - 2) * 3;
                if total_indices > super::scene::MAX_SCENE_INDICES {
                    return Err(unsupported(
                        lineno,
                        format_args!("the faces pass {} indices", super::scene::MAX_SCENE_INDICES),
                    ));
                }
                if slots.len() == 3 {
                    prim.indices.extend_from_slice(&slots);
                } else {
                    let pts: Vec<[f64; 3]> = corners.iter().map(|c| src_v[c.v as usize].map(f64::from)).collect();
                    let (tris, fanned) = super::polygon::triangles(&pts);
                    fanned_concave += usize::from(fanned);
                    for t in tris {
                        prim.indices.extend_from_slice(&[slots[t[0]], slots[t[1]], slots[t[2]]]);
                    }
                }
            }
            // Each `o` starts a new object. The first one before any geometry
            // names the object that is already open rather than leaving an empty
            // nameless one behind.
            // Each `o` or `g` starts a part, as three.js reads them (r170
            // OBJLoader.js:20, `/^[og]\s*(.+)?/`).
            "o" | "g" => {
                let rest = body[kw.len()..].trim();
                let name = (!rest.is_empty()).then(|| rest.to_string());
                if first_name.is_none() {
                    first_name = name.clone();
                }
                match objects.last_mut() {
                    Some(o) if o.prims.is_empty() => o.name = name,
                    _ => {
                        if objects.len() >= super::scene::MAX_NODES {
                            return Err(unsupported(
                                lineno,
                                format_args!("more than {} objects", super::scene::MAX_NODES),
                            ));
                        }
                        objects.push(ObjBuild { name, prims: Vec::new(), by_material: HashMap::new() })
                    }
                }
            }
            "usemtl" => {
                let rest = body[kw.len()..].trim();
                material = (!rest.is_empty()).then(|| rest.to_string());
            }
            "mtllib" => {
                // Kept whole: `read_scene` decides between one name with spaces
                // and the spec's list. Statements past the cap are counted
                // there from the names they hold.
                let rest = body[kw.len()..].trim();
                if !rest.is_empty() && libraries.len() <= MAX_MATERIAL_LIBRARIES {
                    libraries.push(rest.to_string());
                }
            }
            // `l v1 v2 … vn` is the polyline through its vertices, n − 1
            // segments of two indices each, and `p v1 … vn` is n points. Their
            // corners are spelt as a face's are (the grammar allows `v/vt` for a
            // line and bare `v` for a point; the rest is read and kept, not
            // refused), and each statement joins the primitive of its own kind
            // for the material in force, beside the object's faces.
            "l" | "p" => {
                let (topology, noun, least) = if kw == "l" {
                    (Topology::Lines, "lines", 2)
                } else {
                    (Topology::Points, "points", 1)
                };
                corners.clear();
                for tok in tokens {
                    // Refused as the corners arrive, like a face's: a segment
                    // for each corner after the first, a point for each.
                    let would_be = match topology {
                        Topology::Points => corners.len() + 1,
                        _ => corners.len() * 2,
                    };
                    if total_indices.saturating_add(would_be) > super::scene::MAX_SCENE_INDICES {
                        return Err(unsupported(
                            lineno,
                            format_args!("the {noun} pass {} indices", super::scene::MAX_SCENE_INDICES),
                        ));
                    }
                    corners.push(parse_corner(tok, lineno, src_v.len(), src_vt.len(), src_vn.len())?);
                }
                if corners.len() < least {
                    return Err(malformed(
                        lineno,
                        if topology == Topology::Lines {
                            format!("line has {} vertices; a line needs at least 2", corners.len())
                        } else {
                            "point has no vertices; a point statement needs at least 1".to_string()
                        },
                    ));
                }
                let obj = objects.last_mut().ok_or_else(|| malformed(lineno, "no current object"))?;
                let pi = primitive_for(obj, &material, topology, &mut total_prims, lineno)?;
                let prim = &mut obj.prims[pi];
                let src = Sources { v: &src_v, vc: &src_vc, vt: &src_vt, vn: &src_vn, colored: colored == Some(true) };
                intern_corners(prim, &corners, &src, &mut total_vertices, &mut slots, lineno)?;
                if topology == Topology::Lines {
                    for pair in slots.windows(2) {
                        prim.indices.extend_from_slice(pair);
                    }
                    total_indices += (slots.len() - 1) * 2;
                } else {
                    prim.indices.extend_from_slice(&slots);
                    total_indices += slots.len();
                }
                if total_indices > super::scene::MAX_SCENE_INDICES {
                    return Err(unsupported(
                        lineno,
                        format_args!("the {noun} pass {} indices", super::scene::MAX_SCENE_INDICES),
                    ));
                }
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
    Ok(Parsed { objects, libraries, has_colors: colored == Some(true), first_name, fanned_concave })
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
    /// Options this reader does not know, stepped over with their numbers.
    unknown: Vec<String>,
}

/// Split a map statement's options from its file name.
///
/// Options take a fixed number of arguments each; whatever is left is the file
/// name, which may legally contain spaces.
fn parse_map(args: &str) -> Option<MapSpec> {
    let toks: Vec<&str> = args.split_whitespace().collect();
    let mut spec = MapSpec { file: String::new(), offset: [0.0, 0.0], scale: [1.0, 1.0], clamp: false, bump: None, unknown: Vec::new() };
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
                        // "-s u v w … v is optional … The default is 1, 1, 1"
                        // (the MTL spec): one number scales u alone.
                        "-s" => spec.scale = [u, v],
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
            // An option we do not know: its arguments are taken to be the
            // numbers and on/off words after it (every MTL option's are), and
            // the caller names it.
            _ => {
                spec.unknown.push(toks[i].to_string());
                i += 1;
                while i + 1 < toks.len()
                    && (num(toks.get(i)).is_some() || toks[i].eq_ignore_ascii_case("on") || toks[i].eq_ignore_ascii_case("off"))
                {
                    i += 1;
                }
            }
        }
    }
    if i >= toks.len() {
        return None;
    }
    spec.file = toks[i..].join(" ");
    Some(spec)
}

/// [`parse_map`], with a warning for what it stepped over or could not find.
fn map_noted(rest: &str, lib: &str, line: usize, kw: &str, warnings: &mut Warnings) -> Option<MapSpec> {
    let spec = parse_map(rest);
    match &spec {
        None => warnings.push(format!("{} line {line}: `{kw}` names no file; its texture was dropped", clip(lib))),
        Some(s) if !s.unknown.is_empty() => warnings.push(format!(
            "{} line {line}: `{kw}` option(s) {} are not known; each was skipped with the numbers after it",
            clip(lib),
            clip(&s.unknown.join(", "))
        )),
        Some(_) => {}
    }
    spec
}

/// A statement [`read_layers`] reads once the whole material is known, as
/// written: its line, its keyword's spelling and its values.
struct Statement {
    line: usize,
    keyword: String,
    /// The first four: enough to tell one from three from more.
    values: Vec<String>,
}

impl Statement {
    fn new(line: usize, keyword: &str, rest: &str) -> Statement {
        Statement { line, keyword: keyword.to_string(), values: rest.split_whitespace().take(4).map(str::to_string).collect() }
    }

    fn first(&self) -> &str {
        self.values.first().map_or("", String::as_str)
    }

    /// The statement as a warning quotes it, `` `Pc 5` ``, bounded.
    fn quoted(&self) -> String {
        let text = std::iter::once(self.keyword.as_str()).chain(self.values.iter().map(String::as_str)).collect::<Vec<_>>().join(" ");
        let short: String = text.chars().take(48).collect();
        let more = if short.len() < text.len() { "..." } else { "" };
        format!("`{short}{more}`")
    }
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
    /// Any of `Pr`, `Pm`, `Ps`, `Pc`, `Pcr`, `aniso`, `anisor`, `map_Pr` or
    /// `map_Pm`: a PBR MTL, whose `Ni` is an ior (module docs).
    pbr: bool,
    ni: Option<Statement>,
    pc: Option<Statement>,
    pcr: Option<Statement>,
    ps: Option<Statement>,
    aniso: Option<Statement>,
    anisor: Option<Statement>,
    /// `Tf` or `Kt`: in a PBR MTL, Blender's Transmission Weight (module
    /// docs).
    tf: Option<Statement>,
}

fn parse_mtl(text: &str, lib: &str, warnings: &mut Warnings) -> Vec<(String, MtlMaterial)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
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
        if matches!(kw_lower.as_str(), "pr" | "pm" | "ps" | "pc" | "pcr" | "aniso" | "anisor" | "map_pr" | "map_pm") {
            m.pbr = true;
        }
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
            "map_kd" => m.map_kd = map_noted(rest, lib, i + 1, kw, warnings),
            "map_ke" => m.map_ke = map_noted(rest, lib, i + 1, kw, warnings),
            "norm" => m.norm = map_noted(rest, lib, i + 1, kw, warnings),
            // `Ni` and the layers of Blender's PBR extension, read once the
            // whole material is known (`read_layers`).
            "ni" | "pc" | "pcr" | "ps" | "aniso" | "anisor" | "tf" | "kt" => {
                let slot = match kw_lower.as_str() {
                    "ni" => &mut m.ni,
                    "pc" => &mut m.pc,
                    "pcr" => &mut m.pcr,
                    "ps" => &mut m.ps,
                    "aniso" => &mut m.aniso,
                    "anisor" => &mut m.anisor,
                    // One field to Blender, as to its reader: the last wins.
                    _ => &mut m.tf,
                };
                *slot = Some(Statement::new(i + 1, kw, rest));
            }
            // Ambient, specular, illumination model and the like describe a
            // Phong surface the scene's metallic-roughness model does not have.
            "ka" | "ks" | "illum" | "map_ka" | "map_ks" | "map_ns" => {}
            "map_bump" | "bump" | "map_d" | "disp" | "decal" | "refl" | "map_pr" | "map_pm" | "map_ps" => {
                warnings.push(format!(
                    "{} line {}: `{kw}` (a {}) has no counterpart in the scene and was dropped",
                    clip(lib),
                    i + 1,
                    match kw_lower.as_str() {
                        "map_bump" | "bump" => "height/bump map",
                        "map_d" => "separate alpha map",
                        "disp" => "displacement map",
                        // Blender's, a weight; glTF's sheen map is a colour.
                        "map_ps" => "sheen weight map",
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

// What Blender's Principled BSDF draws where its PBR extension to MTL says
// nothing (node_shader_bsdf_principled.cc, v5.2.3), so what the reader gives
// (module docs): a coat's roughness without `Pcr` (:236-237), a sheen's
// roughness, which MTL has no key for (:276-277), and an anisotropy's rotation
// without `anisor` (:201-202).
const BLENDER_COAT_ROUGHNESS: f32 = 0.03;
const BLENDER_SHEEN_ROUGHNESS: f32 = 0.5;
const BLENDER_ANISOTROPIC_ROTATION: f32 = 0.0;

/// The `Ni` values Blender writes by default, which a PBR MTL's reader takes
/// as unset (module docs): 1 for a material without a Principled BSDF, 1.45
/// as 2.8-3.6's Principled default, and 1.5 as 4.x's, glTF's own default.
const DEFAULT_NI: [f32; 3] = [1.0, 1.45, 1.5];

/// Whether `v` is within glTF's 0 to 1 for a factor.
fn unit(v: f32) -> bool {
    (0.0..=1.0).contains(&v)
}

/// glTF's anisotropyRotation, in radians, as `anisor`: Blender's Anisotropic
/// Rotation, a fraction of a full turn (module docs), in [0, 1). An f64, so
/// that [`rotation_from_turn`] gives back the same f32 for any rotation in
/// [0, 2π); any other comes back as the same direction in that range.
fn turn_from_rotation(r: f32) -> f64 {
    let t = (f64::from(r) / std::f64::consts::TAU).rem_euclid(1.0);
    // `rem_euclid` can round up to 1 itself, just below a whole turn.
    if t < 1.0 { t } else { 0.0 }
}

fn rotation_from_turn(t: f64) -> f32 {
    (t * std::f64::consts::TAU) as f32
}

/// `s`'s number, if it is finite and within `range`; otherwise a warning
/// naming the statement and saying what is done `instead`.
fn checked<T>(s: &Statement, range: std::ops::RangeInclusive<T>, instead: &str, lib: &str, warnings: &mut Warnings) -> Option<T>
where
    T: std::str::FromStr + PartialOrd + Copy + Into<f64> + fmt::Display,
{
    let why = match s.first().parse::<T>() {
        Ok(v) if v.into().is_finite() && range.contains(&v) => return Some(v),
        Ok(v) if v.into().is_finite() => format!("is outside {} to {}", range.start(), range.end()),
        _ => "is not a number".to_string(),
    };
    warnings.push(format!("{} line {}: {} {why}; {instead}", clip(lib), s.line, s.quoted()));
    None
}

/// `Ps` as a sheen colour (module docs): one value is Blender's Sheen Weight
/// under a white tint, a grey, and three are the colour, as assimp reads
/// them. Every channel is within 0 to 1, or the sheen is dropped with a
/// warning.
fn sheen_colour(s: &Statement, lib: &str, warnings: &mut Warnings) -> Option<[f32; 3]> {
    let numbers: Option<Vec<f32>> = s.values.iter().map(|t| t.parse::<f32>().ok().filter(|v| v.is_finite())).collect();
    let why = match numbers.as_deref() {
        Some(&[w]) if unit(w) => return Some([w; 3]),
        Some(&[r, g, b]) if unit(r) && unit(g) && unit(b) => return Some([r, g, b]),
        None | Some([]) => "is not a number",
        Some([_]) => "is outside 0 to 1",
        Some([_, _, _]) => "has a channel outside 0 to 1",
        Some(_) => "is neither one weight nor three colour channels",
    };
    warnings.push(format!("{} line {}: {} {why}; the sheen was dropped", clip(lib), s.line, s.quoted()));
    None
}

/// The layers and ior a material's PBR statements give, read as Blender
/// reads them (module docs). A layer exists above its weight's off value;
/// a `Pcr` or `anisor` without its weight adds nothing and says nothing; a
/// value its glTF field can't hold, or that is no number, is dropped with a
/// warning, never clamped.
fn read_layers(mat: &mut Material, m: &MtlMaterial, lib: &str, warnings: &mut Warnings) {
    if let Some(pc) = m.pc.as_ref().and_then(|s| checked(s, 0.0..=4.0, "the clearcoat was dropped", lib, warnings)) {
        if pc > 0.0 {
            let instead = format!("Blender's default coat roughness of {BLENDER_COAT_ROUGHNESS} is used");
            let roughness = m.pcr.as_ref().and_then(|s| checked(s, 0.0..=1.0, &instead, lib, warnings)).unwrap_or(BLENDER_COAT_ROUGHNESS);
            mat.clearcoat = Some(Clearcoat { factor: pc / 4.0, roughness, ..Default::default() });
        }
    }
    if let Some(color) = m.ps.as_ref().and_then(|s| sheen_colour(s, lib, warnings)) {
        if color.iter().any(|&c| c > 0.0) {
            mat.sheen = Some(Sheen { color, roughness: BLENDER_SHEEN_ROUGHNESS, ..Default::default() });
        }
    }
    if let Some(strength) = m.aniso.as_ref().and_then(|s| checked(s, 0.0..=1.0, "the anisotropy was dropped", lib, warnings)) {
        if strength > 0.0 {
            let instead = format!("Blender's default rotation of {BLENDER_ANISOTROPIC_ROTATION} is used");
            let rotation = m
                .anisor
                .as_ref()
                .and_then(|s| checked(s, 0.0f64..=1.0, &instead, lib, warnings))
                .map_or(BLENDER_ANISOTROPIC_ROTATION, rotation_from_turn);
            mat.anisotropy = Some(Anisotropy { strength, rotation, texture: None });
        }
    }
    // A classic MTL's `Ni` stays unread, and unremarked.
    if let (true, Some(s)) = (m.pbr, &m.ni) {
        match s.first().parse::<f32>() {
            // An exporter's default: unset. Then KHR_materials_ior's schema,
            // less its 0, which Blender reads as no ior.
            Ok(v) if DEFAULT_NI.contains(&v) => {}
            Ok(v) if v > 0.0 && super::scene::ior_in_schema(v) => mat.ior = Some(v),
            parsed => {
                let why = match parsed {
                    Ok(v) if v.is_finite() && v <= 0.0 => "is 0 or below, which Blender reads as no ior",
                    Ok(v) if v.is_finite() => "is between 0 and 1, which KHR_materials_ior does not allow",
                    _ => "is not a number",
                };
                warnings.push(format!("{} line {}: {} {why}; it was left out", clip(lib), s.line, s.quoted()));
            }
        }
    }
    // Blender's Transmission Weight, the mean of the three, a missing one as
    // 0: OBJ keeps no transmission, so it is named. A classic MTL's `Tf` or
    // `Kt` stays unremarked.
    if let (true, Some(s)) = (m.pbr, &m.tf) {
        let channel = |i: usize| s.values.get(i).and_then(|t| t.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(0.0);
        let weight = (channel(0) + channel(1) + channel(2)) / 3.0;
        if weight > 0.0 {
            let name = clip(mat.name.as_deref().unwrap_or(""));
            warnings.push(format!("{} line {}: material {name:?}: {} (transmission) {weight} is not carried", clip(lib), s.line, s.keyword));
        }
    }
}

/// Resolve a texture statement into the scene, through the sidecars.
fn load_map(
    scene: &mut Scene,
    images: &mut HashMap<String, Option<usize>>,
    spec: &MapSpec,
    lib_path: &str,
    sidecars: &dyn Sidecars,
    warnings: &mut Warnings,
) -> Option<TextureRef> {
    let Some(path) = package::resolve_relative(lib_path, &spec.file) else {
        warnings.push(format!("texture {:?} points outside the model's files; dropped", clip(&spec.file)));
        return None;
    };
    let image = match images.get(&path) {
        Some(i) => *i,
        None => {
            let loaded = match sidecars.get(&path) {
                None => {
                    warnings.push(format!("texture {:?} was not supplied; dropped", clip(&path)));
                    None
                }
                Some(bytes) => match ImageFormat::sniff(&bytes) {
                    Some(format) => {
                        let name = path.rsplit('/').next().and_then(|f| f.rsplit_once('.')).map(|(s, _)| s.to_string());
                        scene.images.push(Image { name, format, data: bytes });
                        Some(scene.images.len() - 1)
                    }
                    None => {
                        warnings.push(format!("texture {:?} is not PNG, JPEG, WebP or KTX2; dropped", clip(&path)));
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
    let mut warnings = Warnings::default();
    if parsed.fanned_concave > 0 {
        warnings.push(format!(
            "{} concave face(s) of more than {} corners were fan-triangulated",
            parsed.fanned_concave,
            super::polygon::MAX_EAR_CLIP
        ));
    }
    let mut scene = Scene { name: parsed.first_name.clone(), ..Default::default() };

    // Materials, by name, from every library the file names.
    let mut library: Vec<(String, MtlMaterial, String)> = Vec::new();
    // A statement's rest of line is one file when a file has that name, spaces
    // and all (three.js r170 OBJLoader.js:661 reads it so); otherwise the
    // spec's whitespace-separated list.
    let mut names: Vec<String> = Vec::new();
    for line in &parsed.libraries {
        let whole = line.contains(char::is_whitespace)
            && package::normalize_relative(line).is_some_and(|p| sidecars.get(&p).is_some());
        if whole || !line.contains(char::is_whitespace) {
            names.push(line.clone());
        } else {
            names.extend(line.split_whitespace().map(str::to_string));
        }
    }
    if names.len() > MAX_MATERIAL_LIBRARIES {
        warnings.push(format!(
            "the file names more than {MAX_MATERIAL_LIBRARIES} material libraries; the first {MAX_MATERIAL_LIBRARIES} were read"
        ));
        names.truncate(MAX_MATERIAL_LIBRARIES);
    }
    for lib in &names {
        let Some(path) = package::normalize_relative(lib) else {
            warnings.push(format!("material library {:?} points outside the model's files; ignored", clip(lib)));
            continue;
        };
        match sidecars.get(&path) {
            None => warnings.push(format!("material library {:?} was not supplied; materials are untextured defaults", clip(&path))),
            Some(b) => match String::from_utf8(b) {
                Ok(text) => {
                    for (name, m) in parse_mtl(&text, &path, &mut warnings) {
                        library.push((name, m, path.clone()));
                    }
                }
                Err(_) => warnings.push(format!("material library {:?} is not UTF-8; ignored", clip(&path))),
            },
        }
    }
    let mut material_index: HashMap<String, usize> = HashMap::new();
    let mut images: HashMap<String, Option<usize>> = HashMap::new();
    let mut material_for = |scene: &mut Scene, name: &str, warnings: &mut Warnings| -> usize {
        if let Some(i) = material_index.get(name) {
            return *i;
        }
        let mut mat = Material { name: Some(name.to_string()), metallic: 0.0, roughness: 1.0, ..Default::default() };
        match library.iter().find(|(n, _, _)| n == name) {
            None => {
                if !parsed.libraries.is_empty() {
                    warnings.push(format!("material {:?} is not defined in any supplied library", clip(name)));
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
                    mat.emissive = ke.map(linear_extended);
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
                read_layers(&mut mat, m, lib, warnings);
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
                topology: p.topology,
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
    Ok(SceneRead { scene, warnings: warnings.into_vec() })
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
        // The flat mesh is the triangles alone, so its colour is theirs: a
        // line's or a point's colour, first in the file, is not the mesh's.
        m.base_color = r
            .scene
            .meshes
            .iter()
            .flat_map(|mm| &mm.primitives)
            .filter(|p| p.topology == Topology::Triangles)
            .find_map(|p| p.colors.as_ref()?.first().copied());
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

/// Names handed out so far, and where each base's numbering resumes: a scan
/// of a list from `_1` for every repeat was cubic (8,000 same-named parts
/// took 179 s to write).
#[derive(Default)]
struct Taken {
    used: HashSet<String>,
    next: HashMap<String, usize>,
}

fn unique(name: String, taken: &mut Taken) -> String {
    let mut candidate = name.clone();
    let mut n = taken.next.get(&name).copied().unwrap_or(1);
    while taken.used.contains(&candidate) {
        candidate = format!("{name}_{n}");
        n += 1;
    }
    taken.next.insert(name, n);
    taken.used.insert(candidate.clone());
    candidate
}

/// A sidecar path `{base}.{ext}` no earlier one takes, else `{base}_1.{ext}`,
/// `{base}_2.{ext}`, …: the number goes before the extension, so the file
/// still opens as what it is. Compared folded, since paths that differ only in
/// case are one file on macOS and Windows.
fn unique_file(base: &str, ext: &str, taken: &mut Taken) -> String {
    let key = base.to_ascii_lowercase();
    let mut n = taken.next.get(&key).copied().unwrap_or(0);
    let mut candidate = if n == 0 { format!("{base}.{ext}") } else { format!("{base}_{n}.{ext}") };
    while taken.used.contains(&candidate.to_ascii_lowercase()) {
        n += 1;
        candidate = format!("{base}_{n}.{ext}");
    }
    taken.next.insert(key, n);
    taken.used.insert(candidate.to_ascii_lowercase());
    candidate
}

/// Specular exponent for a roughness, Blender's mapping (the inverse of the
/// reader's).
fn roughness_to_ns(r: f32) -> f32 {
    let s = 1.0 - r.clamp(0.0, 1.0);
    s * s * 1000.0
}

/// `Ke` as sRGB up to 1, as `Kd`, and straight on above it: emission brighter
/// than 1 (glTF's emissive strength) keeps its value where clamping lost it.
fn srgb_extended(c: f32) -> f32 {
    if c > 1.0 { c } else { linear_to_srgb(c.max(0.0)) }
}

/// [`srgb_extended`]'s inverse.
fn linear_extended(c: f32) -> f32 {
    if c > 1.0 { c } else { srgb_to_linear(c.max(0.0)) }
}

fn srgb3(c: [f32; 3]) -> [f32; 3] {
    [linear_to_srgb(c[0].clamp(0.0, 1.0)), linear_to_srgb(c[1].clamp(0.0, 1.0)), linear_to_srgb(c[2].clamp(0.0, 1.0))]
}

/// The scene reduced to exactly what [`write_scene`] encodes, which is also
/// what [`read_scene`] gives back for it: in a package, the material layers
/// as an MTL holds them (`mtl_layers`) and its sidedness and alpha
/// (`mtl_alpha_and_sides`); then the generic reduction to OBJ's capabilities
/// (`pose::degrade`). The layers go first, so the one uv set `degrade` keeps
/// is never chosen for a map an MTL drops; only the materials are copied for
/// it (`pose::degrade_with_materials`).
///
/// Then what the statements cannot say: no normal on a line or point, no
/// texture coordinate on a point, and, in a package, no map on the material
/// of a points primitive (`untexture_points`: an untextured copy named as the
/// writer names it, or the material itself where only points use it). Then
/// what the reader gives back that the scene does not name: one colour on
/// every `v` line once any has one, a white `default` material for a
/// primitive of a package with none, and, in a file with no MTL (one file, or
/// a package of no materials), a white material for each kind that is
/// written, `faces`, `lines` and `points`, in the order they are met: one
/// file names them with `usemtl` alone, and a package of no materials
/// writes the MTL that holds them.
pub fn prepare(scene: &Scene, opts: super::WriteOptions) -> Result<Scene, MeshError> {
    let caps = MeshFormat::Obj.capabilities(opts);
    // One file has no MTL, and `degrade` drops every layer.
    let mut s = if opts.self_contained {
        super::pose::degrade(scene, &caps)?
    } else {
        let slots = layer_slots();
        let materials: Vec<Material> = scene
            .materials
            .iter()
            .map(|m| {
                let mut m = m.clone();
                mtl_layers(&mut m, &slots);
                mtl_alpha_and_sides(&mut m);
                m
            })
            .collect();
        super::pose::degrade_with_materials(scene, materials, &caps)?
    };
    // An `l` takes `v/vt` and no normal, and a `p` neither (OBJ's grammar):
    // what the statement cannot say is not in the scene the writer encodes,
    // which is the scene the reader gives back. `losses` names it.
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
        if p.topology != Topology::Triangles {
            p.normals = None;
            p.tangents = None;
        }
        if p.topology == Topology::Points {
            p.uvs.clear();
        }
    }
    // With no coordinate to sample by, a point's material holds no map either.
    if !opts.self_contained {
        untexture_points(&mut s);
    }
    // Every `v` line carries a colour or none does (the reader refuses a mix),
    // so once one primitive has colours the writer gives the others white, the
    // colour that changes nothing; and the reader gives that back.
    if s.meshes.iter().flat_map(|m| &m.primitives).any(|p| p.colors.is_some()) {
        for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()).filter(|p| p.colors.is_none()) {
            p.colors = Some(vec![[1.0; 4]; p.positions.len()]);
        }
    }
    // A package names a material for every primitive (`usemtl` is sticky), so
    // those with none get a white `default` one in the MTL, and the reader
    // gives it back as a material of its own: the reader's own start for a
    // material the MTL leaves bare, which `Kd 1 1 1`, `Pr 1` and `Pm 0` say.
    if !opts.self_contained
        && !s.materials.is_empty()
        && s.meshes.iter().flat_map(|m| &m.primitives).any(|p| p.material.is_none())
    {
        let name = package_material_names(&s, true).pop();
        s.materials.push(Material { name, metallic: 0.0, roughness: 1.0, ..Default::default() });
        let default = s.materials.len() - 1;
        for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()).filter(|p| p.material.is_none()) {
            p.material = Some(default);
        }
    }
    // A file with no MTL (one file, or a package of no materials) names no
    // material but through `usemtl`, and with lines or points in it each kind
    // names one of its own (`write_scene`'s `kind_names`: `faces`, `lines`,
    // `points`). The reader gives each name back as a white material of its
    // own, in the order the kinds are written, so the scene holds those
    // materials too. With the materials in the scene, a package of this kind
    // writes the MTL that names them.
    if (opts.self_contained || s.materials.is_empty())
        && s.meshes.iter().flat_map(|m| &m.primitives).any(|p| p.topology != Topology::Triangles)
    {
        s.materials.clear();
        let mut made: [Option<usize>; 3] = [None; 3];
        for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
            let (at, name) = match p.topology {
                Topology::Triangles => (0, "faces"),
                Topology::Lines => (1, "lines"),
                Topology::Points => (2, "points"),
            };
            if made[at].is_none() {
                s.materials.push(Material { name: Some(name.to_string()), metallic: 0.0, roughness: 1.0, ..Default::default() });
                made[at] = Some(s.materials.len() - 1);
            }
            p.material = made[at];
        }
    }
    Ok(s)
}

/// Whether `m` binds a map a package's MTL writes: its base-colour, emissive
/// or normal texture, on the one uv set OBJ holds. [`prepare`] has run
/// `degrade` by the time it asks, so those are the maps left; [`losses`] asks
/// of a scene `degrade` has not seen, and goes through [`mtl_mapped_materials`]
/// for the same answer.
fn has_mtl_map(m: &Material) -> bool {
    let sets = MeshFormat::Obj.capabilities(super::WriteOptions::default()).uv_sets as u32;
    [&m.base_color_texture, &m.emissive_texture, &m.normal_texture].into_iter().flatten().any(|r| r.uv_set < sets)
}

/// For each material of `scene` (not yet through `degrade`), whether a
/// package's MTL still writes a map for it: [`has_mtl_map`] of the material as
/// `prepare` has it after `degrade`, asked of the scene before it. A map on a later uv set is kept, moved to
/// the first (`UvSetPlan`), so the set it names in `scene` decides nothing;
/// the plan is made as `degrade` makes it, from the materials less their
/// layers (`mtl_layers`), over how many uv sets each primitive has and which
/// material it draws, which is all the plan reads of a mesh.
fn mtl_mapped_materials(scene: &Scene) -> Vec<bool> {
    let caps = MeshFormat::Obj.capabilities(super::WriteOptions { self_contained: false, ..Default::default() });
    let slots = layer_slots();
    let materials: Vec<Material> = scene
        .materials
        .iter()
        .map(|m| {
            let mut m = m.clone();
            mtl_layers(&mut m, &slots);
            m
        })
        .collect();
    let meshes = scene
        .meshes
        .iter()
        .map(|m| SceneMesh {
            primitives: m.primitives.iter().map(|p| Primitive { material: p.material, uvs: vec![Vec::new(); p.uvs.len()], ..Default::default() }).collect(),
            ..Default::default()
        })
        .collect();
    let planned = Scene { materials, meshes, ..Default::default() };
    let plan = super::pose::UvSetPlan::new(&planned, caps.uv_sets, &super::pose::kept_texture_slots(&caps));
    planned
        .materials
        .iter()
        .enumerate()
        .map(|(i, m)| {
            [&m.base_color_texture, &m.emissive_texture, &m.normal_texture]
                .into_iter()
                .flatten()
                .any(|r| (plan.set_index(i, r.uv_set) as usize) < caps.uv_sets)
        })
        .collect()
}

/// `p` names a vertex and nothing else, so a point has no texture coordinate
/// to sample a map by, and its material must hold none (a package; one file
/// has no MTL). Left on, a map is smeared across each point sprite by
/// three.js's `OBJLoader` (its `PointsMaterial` takes the material's map),
/// and read back through glTF it is a map on a primitive with no `TEXCOORD`,
/// which the Khronos validator refuses (`MESH_PRIMITIVE_TOO_FEW_TEXCOORDS`).
///
/// So a points primitive whose material has a map is bound to a copy with
/// none: every texture slot empty, the factors as they were and the normal
/// scale at its default (the reader gives a material with no normal map no
/// other). The copy is named as the writer will name it, the original's name
/// numbered (`paint_1`), so that `prepare` equals what the reader gives back.
/// A material only points use has nothing else to keep its maps for, and is
/// stripped where it stands: an original left unused would be written, with
/// its image, and not read back.
fn untexture_points(s: &mut Scene) {
    let others: HashSet<usize> =
        s.meshes.iter().flat_map(|m| &m.primitives).filter(|p| p.topology != Topology::Points).filter_map(|p| p.material).collect();
    let without_maps = |m: &Material| -> Material {
        let mut m = m.clone();
        for (_, r) in m.texture_slots_mut() {
            *r = None;
        }
        m.normal_scale = Material::default().normal_scale;
        m
    };
    let mut copies: HashMap<usize, usize> = HashMap::new();
    let mut stripped = false;
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()).filter(|p| p.topology == Topology::Points) {
        let Some(at) = p.material.filter(|&at| s.materials.get(at).is_some_and(has_mtl_map)) else { continue };
        if others.contains(&at) {
            let copy = *copies.entry(at).or_insert_with(|| {
                s.materials.push(without_maps(&s.materials[at]));
                s.materials.len() - 1
            });
            p.material = Some(copy);
        } else {
            s.materials[at] = without_maps(&s.materials[at]);
            stripped = true;
        }
    }
    if !copies.is_empty() {
        let names = package_material_names(s, false);
        for &copy in copies.values() {
            s.materials[copy].name = Some(names[copy].clone());
        }
    }
    if stripped {
        super::pose::prune_unused_textures(s);
    }
}

/// What an MTL says of a material's sidedness and alpha (Ruling A36): it has
/// no double-sided flag, and its one alpha is `d`, written from the colour's
/// alpha (`Material::drawn_alpha`) and read as blending only below 1. So a
/// material reads back single-sided, and a BLEND whose drawn alpha is 1 (its
/// alpha in its base-colour texture alone, or nowhere) reads back OPAQUE. It
/// has no alpha test either: a MASK is drawn wholly (its colour's alpha at or
/// above the cutoff, so no `d`, and its texture's alpha is not read: OPAQUE)
/// or not at all (below it: `d 0`, a BLEND of alpha 0), whatever its texture's
/// cut-out texels say (item 3, Task 5, ruling K21).
/// `losses` names the first (double-sidedness), the alpha cutoff and the
/// textured BLEND.
fn mtl_alpha_and_sides(m: &mut Material) {
    m.double_sided = false;
    if m.alpha_mode == AlphaMode::Mask {
        m.base_color[3] = m.drawn_alpha();
        m.alpha_mode = AlphaMode::Blend;
    }
    if m.alpha_mode == AlphaMode::Blend && m.drawn_alpha() >= 1.0 {
        m.alpha_mode = AlphaMode::Opaque;
    }
}

/// The texture slots of the layers a package keeps, every one of which goes:
/// litegen writes a layer's factors alone (module docs).
fn layer_slots() -> Vec<TextureSlot> {
    let kept = MeshFormat::Obj.capabilities(super::WriteOptions { self_contained: false, ..Default::default() }).layers;
    MaterialLayer::ALL.into_iter().filter(|&l| kept.keeps(l)).flat_map(|l| l.texture_slots().iter().copied()).collect()
}

/// What [`mtl_layers`] drops of one material's layers, for [`losses`].
#[derive(Default)]
struct MtlDrops {
    /// The bound slots of the layers kept.
    maps: Vec<TextureSlot>,
    /// A coloured sheen, written as one weight.
    sheen_colour: bool,
    /// A sheen roughness other than Blender's, which MTL has no key for.
    sheen_roughness: bool,
    // Values outside glTF's own range, which the reader would refuse.
    clearcoat_factor: bool,
    clearcoat_roughness: bool,
    sheen_range: bool,
    anisotropy_strength: bool,
    ior_zero: bool,
    /// An ior the reader takes as an exporter's default ([`DEFAULT_NI`]).
    ior_default: Option<f32>,
}

/// `m`'s layers as Blender's PBR extension to MTL holds them, which is what
/// [`read_scene`] gives back for what [`write_mtl_text`] writes (module docs),
/// and what that drops:
/// - a layer at its off value goes, unnamed, as it draws nothing;
/// - what the reader would refuse goes, which glTF's reader passes through:
///   a clearcoat factor, sheen colour or anisotropy strength outside 0 to 1
///   drops its layer, a clearcoat roughness outside 0 to 1 becomes Blender's,
///   and an ior of 0 goes, as Blender reads `Ni 0` as none;
/// - every map of a layer kept goes, by slot (`layer_slots`), the
///   clearcoat's normal map among them: litegen writes a layer's factors
///   alone;
/// - a sheen's colour becomes its brightest channel in grey, and its
///   roughness Blender's;
/// - an anisotropy's rotation becomes the turn it is written as;
/// - an ior equal to one of Blender's defaults goes, as the reader takes it
///   as unset ([`DEFAULT_NI`]).
fn mtl_layers(m: &mut Material, layer_slots: &[TextureSlot]) -> MtlDrops {
    let mut d = MtlDrops::default();
    // The layers MTL can't hold at all go first, maps and all.
    match m.clearcoat.as_ref().map(|c| c.factor) {
        Some(f) if f == 0.0 => m.clearcoat = None,
        Some(f) if !(f > 0.0 && f <= 1.0) => {
            m.clearcoat = None;
            d.clearcoat_factor = true;
        }
        _ => {}
    }
    match m.sheen.as_ref().map(|s| s.color) {
        Some(c) if !c.iter().all(|&v| unit(v)) => {
            m.sheen = None;
            d.sheen_range = true;
        }
        Some(c) if c.iter().all(|&v| v == 0.0) => m.sheen = None,
        _ => {}
    }
    match m.anisotropy.as_ref().map(|a| a.strength) {
        Some(s) if s == 0.0 => m.anisotropy = None,
        Some(s) if !(s > 0.0 && s <= 1.0) => {
            m.anisotropy = None;
            d.anisotropy_strength = true;
        }
        _ => {}
    }
    // Then every map of the layers kept, whichever field holds it.
    for (slot, r) in m.texture_slots_mut() {
        if layer_slots.contains(&slot) && r.take().is_some() {
            d.maps.push(slot);
        }
    }
    // And the factors, as written and read back.
    if let Some(c) = m.clearcoat.take() {
        d.clearcoat_roughness = !unit(c.roughness);
        let roughness = if d.clearcoat_roughness { BLENDER_COAT_ROUGHNESS } else { c.roughness };
        m.clearcoat = Some(Clearcoat { factor: c.factor, roughness, ..Default::default() });
    }
    if let Some(s) = m.sheen.take() {
        let weight = s.color[0].max(s.color[1]).max(s.color[2]);
        d.sheen_colour = s.color != [weight; 3];
        d.sheen_roughness = s.roughness != BLENDER_SHEEN_ROUGHNESS;
        m.sheen = Some(Sheen { color: [weight; 3], roughness: BLENDER_SHEEN_ROUGHNESS, ..Default::default() });
    }
    if let Some(a) = m.anisotropy.take() {
        m.anisotropy = Some(Anisotropy { strength: a.strength, rotation: rotation_from_turn(turn_from_rotation(a.rotation)), ..Default::default() });
    }
    match m.ior {
        Some(v) if v == 0.0 => d.ior_zero = true,
        Some(v) if DEFAULT_NI.contains(&v) => d.ior_default = Some(v),
        _ => {}
    }
    if d.ior_zero || d.ior_default.is_some() {
        m.ior = None;
    }
    d
}

/// Write a scene already reduced to OBJ's capabilities (`MeshFormat::prepare`):
/// every primitive in world space, at identity roots.
///
/// What OBJ and its MTL cannot say of alpha, sidedness and shading, beyond
/// the capability table: vertex colours are RGB; MTL has no alpha test, no
/// double-sidedness and no unlit flag its readers honour, and its alpha is the
/// colour's alone (`d`), so a texture's alpha cannot make a material blend;
/// and a single-file
/// OBJ, which bakes each material's colour into RGB vertex colours, keeps no
/// translucency at all. A package's MTL holds the layers OBJ keeps less
/// some parts, each named here and never as the layer's own loss
/// (`scene::losses`' "clearcoat on ..."): `mtl_layers` lists them.
pub fn losses(scene: &Scene, self_contained: bool) -> Vec<Loss> {
    let mut out = Vec::new();
    let mut push = |kind: LossKind, what: String, why: &str| out.push(Loss::new(kind, what, format!("obj {why}")));
    if scene.meshes.iter().flat_map(|m| &m.primitives).any(|p| p.colors.as_ref().is_some_and(|c| c.iter().any(|c| c[3] < 1.0))) {
        push(LossKind::Alpha, "vertex colour alpha".into(), "vertex colours are RGB");
    }
    // `l` takes no normal and `p` neither a normal nor a texture coordinate
    // (OBJ's grammar), so `prepare` leaves them out; counted by primitive, as
    // the points and lines a container lacks are.
    let primitives = |f: &dyn Fn(&Primitive) -> bool| scene.meshes.iter().flat_map(|m| &m.primitives).filter(|p| f(p)).count();
    let n = primitives(&|p| p.topology != Topology::Triangles && p.normals.is_some());
    if n > 0 {
        push(LossKind::Normals, format!("normals of {n} line or point primitive(s)"), "has no place for a normal on an `l` or `p` statement");
    }
    let n = primitives(&|p| p.topology == Topology::Points && !p.uvs.is_empty());
    if n > 0 {
        push(LossKind::Uvs, format!("texture coordinates of {n} point primitive(s)"), "has no place for a texture coordinate on a `p` statement");
    }
    // With none to sample by, the material a point is drawn with loses its
    // maps too (`untexture_points`); one file has no MTL, and has no maps.
    // Which materials still have a map to lose is `prepare`'s answer, after
    // `degrade` has moved a sampled uv set to the first.
    let mapped = if self_contained || primitives(&|p| p.topology == Topology::Points) == 0 { Vec::new() } else { mtl_mapped_materials(scene) };
    let n = primitives(&|p| p.topology == Topology::Points && p.material.is_some_and(|m| mapped.get(m).copied().unwrap_or(false)));
    if n > 0 {
        push(
            LossKind::Textures,
            format!("texture bindings of {n} point primitive(s)"),
            "has no place for a texture coordinate on a `p` statement, so points are drawn with their material's maps left out",
        );
    }
    let count = |f: &dyn Fn(&Material) -> bool| scene.materials.iter().filter(|m| f(m)).count();
    let n = count(&|m| m.alpha_mode == AlphaMode::Mask);
    if n > 0 {
        push(LossKind::Alpha, format!("alpha cutoff of {n} material(s)"), "has no alpha test; each draws as wholly opaque or not at all");
    }
    let n = count(&|m| m.double_sided);
    if n > 0 {
        push(LossKind::Materials, format!("double-sidedness of {n} material(s)"), "has no double-sided flag");
    }
    let n = count(&|m| m.unlit);
    if n > 0 {
        push(LossKind::Materials, format!("unlit shading of {n} material(s)"), "has no unlit flag its readers honour");
    }
    if self_contained {
        let n = count(&|m| m.drawn_alpha() < 1.0);
        if n > 0 {
            push(LossKind::Alpha, format!("translucency of {n} material(s)"), "in one file bakes each material's colour into RGB vertex colours");
        }
    } else {
        // `d` is the colour's alpha alone (`mtl_alpha_and_sides`): a BLEND
        // whose alpha lives in its base-colour texture writes none, and reads
        // back opaque.
        let n = count(&|m| m.alpha_mode == AlphaMode::Blend && m.drawn_alpha() >= 1.0 && m.base_color_texture.is_some());
        if n > 0 {
            push(
                LossKind::Alpha,
                format!("alpha blending of {n} material(s)"),
                "writes a material's alpha as `d`, its colour's alpha alone, so a BLEND material whose alpha lives in its \
                 base-colour texture is written opaque",
            );
        }
        let slots = layer_slots();
        let drops: Vec<MtlDrops> = scene.materials.iter().map(|m| mtl_layers(&mut m.clone(), &slots)).collect();
        // Each map by its slot, glTF's name for it, so none can go unnamed.
        for slot in slots {
            let n = drops.iter().filter(|d| d.maps.contains(&slot)).count();
            if n > 0 {
                push(
                    LossKind::Materials,
                    format!("{} texture on {n} material(s)", slot.as_str()),
                    "writes a layer's factors alone; Blender's PBR extension has no clearcoat or anisotropy maps, and its \
                     sheen map holds a weight, not glTF's colour",
                );
            }
        }
        let parts: [(fn(&MtlDrops) -> bool, &str, String); 7] = [
            (
                |d| d.sheen_colour,
                "sheen colour reduced to one channel",
                "holds sheen as one weight (`Ps`, Blender's Sheen Weight under a white tint), so the colour's brightest channel is kept, in grey".into(),
            ),
            (|d| d.sheen_roughness, "sheen roughness", format!("has no sheen roughness; readers draw Blender's default of {BLENDER_SHEEN_ROUGHNESS}")),
            (
                |d| d.clearcoat_factor,
                "an out-of-range clearcoat factor",
                "holds a clearcoat factor from 0 to 1 only, glTF's range (`Pc` is four times it), so these clearcoats are dropped".into(),
            ),
            (
                |d| d.clearcoat_roughness,
                "an out-of-range clearcoat roughness",
                format!("holds a clearcoat roughness from 0 to 1 only, glTF's range, so these take Blender's default of {BLENDER_COAT_ROUGHNESS}"),
            ),
            (|d| d.sheen_range, "an out-of-range sheen colour", "holds a sheen colour from 0 to 1 only, glTF's range, so these sheens are dropped".into()),
            (
                |d| d.anisotropy_strength,
                "an out-of-range anisotropy strength",
                "holds an anisotropy strength from 0 to 1 only, glTF's range, so these anisotropies are dropped".into(),
            ),
            (|d| d.ior_zero, "an ior of 0", "leaves an ior of 0 out, as Blender reads `Ni 0` as no ior".into()),
        ];
        for (dropped, what, why) in parts {
            let n = drops.iter().filter(|d| dropped(d)).count();
            if n > 0 {
                push(LossKind::Materials, format!("{what} on {n} material(s)"), &why);
            }
        }
        // 1.5, glTF's own default, draws the same unset, so it goes unnamed.
        for v in DEFAULT_NI.into_iter().filter(|&v| v != super::scene::DEFAULT_IOR) {
            let n = drops.iter().filter(|d| d.ior_default == Some(v)).count();
            if n > 0 {
                push(
                    LossKind::Materials,
                    format!("an ior of {v} on {n} material(s)"),
                    &format!(
                        "reads `Ni {v}` as unset, since Blender writes it by default (1 for a material without a \
                         Principled BSDF, 1.45 as 2.8-3.6's Principled default); the ior is dropped"
                    ),
                );
            }
        }
    }
    out
}

/// `self_contained` writes `model.obj` alone; otherwise `model.mtl` and the
/// textures come with it, named so that the references resolve when the files
/// sit side by side.
pub fn write_scene(scene: &Scene, self_contained: bool) -> Result<Package, MeshError> {
    scene.validate()?;
    let prims: Vec<(&SceneMesh, &Primitive)> =
        scene.meshes.iter().flat_map(|m| m.primitives.iter().map(move |p| (m, p))).collect();
    let any_colors = prims.iter().any(|(_, p)| p.colors.is_some());
    let mut out = String::with_capacity(
        prims.iter().map(|(_, p)| p.positions.len() * 3 + p.indices.len() / p.topology.indices_per_element()).sum::<usize>() * 24 + 128,
    );
    put(&mut out, format_args!("# exported by litegen mesh conversion\n"));

    let write_mtl = !self_contained && !scene.materials.is_empty();
    let mut files: Vec<PackageFile> = Vec::new();
    let mut material_names: Vec<String> = Vec::new();
    if write_mtl {
        put(&mut out, format_args!("mtllib model.mtl\n"));
        // OBJ's material state is sticky: a group with no `usemtl` of its own
        // inherits the previous one (three.js's OBJLoader does exactly that),
        // so a part with no material after a red part would render red. Every
        // primitive therefore names a material, and the ones without get a
        // plain white one.
        material_names = package_material_names(scene, prims.iter().any(|(_, p)| p.material.is_none()));
        let (mtl, textures) = write_mtl_text(scene, &material_names)?;
        files.push(PackageFile { path: "model.mtl".into(), bytes: mtl.into_bytes() });
        files.extend(textures);
    }

    // OBJ indices are global across the file, so each primitive's indices are
    // offset by the vertices written before it.
    let (mut base_v, mut base_vt, mut base_vn) = (0usize, 0usize, 0usize);
    let mut used_objects = Taken::default();
    let mut last_mesh: Option<*const SceneMesh> = None;
    let mut last_topology: Option<Topology> = None;
    // A file with no MTL names no material but through `usemtl`, and
    // three.js's OBJLoader (without an MTL) caches a material by its name
    // alone, so the first object's kind of material wins: a triangle after a
    // `p` object got a `PointsMaterial` (research Q6). When lines or points
    // share such a file, each kind names a material of its own.
    let kind_names = !write_mtl && prims.iter().any(|(_, p)| p.topology != Topology::Triangles);
    for (mi, (mesh, p)) in prims.iter().enumerate() {
        // One `o` per mesh: a multi-material mesh keeps its primitives under one
        // object, which is how the reader rebuilds it. And one per run of a
        // topology within it: three.js turns the whole object, faces included,
        // into lines or points at the first `l` or `p` in it.
        if last_mesh != Some(*mesh as *const SceneMesh) || last_topology != Some(p.topology) {
            last_mesh = Some(*mesh as *const SceneMesh);
            last_topology = Some(p.topology);
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
        // What a statement can say of a vertex: `f` takes a texture
        // coordinate and a normal, `l` a texture coordinate (`v/vt`) and `p`
        // neither (the grammar of the OBJ spec). `prepare` has left out the
        // rest; the writer does not trust that for the file it writes.
        let uvs = if p.topology == Topology::Points { None } else { p.uvs.first() };
        let normals = if p.topology == Topology::Triangles { p.normals.as_ref() } else { None };
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
        if write_mtl {
            // The default material is the extra name after the scene's own.
            let m = p.material.unwrap_or(scene.materials.len());
            put(&mut out, format_args!("usemtl {}\n", material_names[m]));
        } else if kind_names {
            let name = match p.topology {
                Topology::Triangles => "faces",
                Topology::Lines => "lines",
                Topology::Points => "points",
            };
            put(&mut out, format_args!("usemtl {name}\n"));
        }
        // The scene is per-vertex, so one index is the position, texture and
        // normal index at once — which is also why an OBJ this writer produced
        // reads back with the vertex count it was written from.
        match p.topology {
            Topology::Triangles => {
                for tri in p.indices.chunks_exact(3) {
                    out.push('f');
                    for idx in tri {
                        let i = *idx as usize;
                        let (v, t, n) = (base_v + i + 1, base_vt + i + 1, base_vn + i + 1);
                        match (uvs.is_some(), normals.is_some()) {
                            (true, true) => put(&mut out, format_args!(" {v}/{t}/{n}")),
                            (true, false) => put(&mut out, format_args!(" {v}/{t}")),
                            (false, true) => put(&mut out, format_args!(" {v}//{n}")),
                            (false, false) => put(&mut out, format_args!(" {v}")),
                        }
                    }
                    out.push('\n');
                }
            }
            // One `l a b` per segment, never `l a b c d`: three.js's OBJLoader
            // pushes a polyline's vertices straight into `LineSegments`, so a
            // polyline would draw 1–2 and 3–4 and lose 2–3 (research Q6).
            Topology::Lines => {
                for seg in p.indices.chunks_exact(2) {
                    out.push('l');
                    for idx in seg {
                        let i = *idx as usize;
                        let (v, t) = (base_v + i + 1, base_vt + i + 1);
                        if uvs.is_some() {
                            put(&mut out, format_args!(" {v}/{t}"));
                        } else {
                            put(&mut out, format_args!(" {v}"));
                        }
                    }
                    out.push('\n');
                }
            }
            Topology::Points => {
                for idx in &p.indices {
                    put(&mut out, format_args!("p {}\n", base_v + *idx as usize + 1));
                }
            }
        }
        base_v += p.positions.len();
        if uvs.is_some() {
            base_vt += p.positions.len();
        }
        if normals.is_some() {
            base_vn += p.positions.len();
        }
    }

    let mut pkg = Package::single("model.obj", out.into_bytes());
    pkg.files.extend(files);
    Ok(pkg)
}

/// The names a package's MTL gives its materials: the scene's own, each made a
/// safe and unique name, then one more, `default`, when `with_default` is set
/// for the primitives that have no material.
fn package_material_names(scene: &Scene, with_default: bool) -> Vec<String> {
    let mut used = Taken::default();
    let mut names: Vec<String> = Vec::with_capacity(scene.materials.len() + 1);
    for (i, m) in scene.materials.iter().enumerate() {
        let base = safe_name(m.name.as_deref().unwrap_or(""), false);
        let base = if base.is_empty() { format!("material_{i}") } else { base };
        names.push(unique(base, &mut used));
    }
    if with_default {
        names.push(unique("default".to_string(), &mut used));
    }
    names
}

fn write_mtl_text(scene: &Scene, names: &[String]) -> Result<(String, Vec<PackageFile>), MeshError> {
    let mut mtl = String::new();
    put(&mut mtl, format_args!("# exported by litegen mesh conversion\n"));
    // Textures as files beside the .mtl, one per image, named after it.
    let mut image_files: Vec<Option<String>> = vec![None; scene.images.len()];
    let mut files = Vec::new();
    let mut used = Taken::default();
    for (i, im) in scene.images.iter().enumerate() {
        // Only what `losses` does not call dropped (PNG, JPEG) is shipped.
        if !im.format.is_universal() {
            continue;
        }
        let stem = package::safe_file_stem(im.name.as_deref().unwrap_or(""), &format!("texture_{i}"));
        let name = unique_file(&format!("textures/{stem}"), im.format.extension(), &mut used);
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
        // The highlight as a Phong reader draws it (three.js's MTLLoader reads
        // Ks as sRGB), so a rough surface does not wash white; Ns carries the
        // roughness for importers that read it as Blender writes it.
        let ks = srgb3(m.phong_specular());
        put(&mut mtl, format_args!("Ka 0 0 0\nKs {} {} {}\n", ks[0], ks[1], ks[2]));
        put(&mut mtl, format_args!("Ns {}\n", roughness_to_ns(m.roughness)));
        // The PBR extension, for the importers that read it (Blender, Unity).
        put(&mut mtl, format_args!("Pr {}\nPm {}\n", m.roughness, m.metallic));
        // Its layers, in Blender's units (module docs): factors alone, as
        // `prepare` left them.
        if let Some(c) = &m.clearcoat {
            put(&mut mtl, format_args!("Pc {}\nPcr {}\n", c.factor * 4.0, c.roughness));
        }
        if let Some(s) = &m.sheen {
            // As a grey triple: assimp reads one value as (w, 0, 0).
            let w = s.color[0].max(s.color[1]).max(s.color[2]);
            put(&mut mtl, format_args!("Ps {w} {w} {w}\n"));
        }
        if let Some(a) = &m.anisotropy {
            put(&mut mtl, format_args!("aniso {}\nanisor {}\n", a.strength, turn_from_rotation(a.rotation)));
        }
        if let Some(ior) = m.ior {
            put(&mut mtl, format_args!("Ni {ior}\n"));
        }
        // The alpha the material is drawn with: none when OPAQUE.
        let alpha = m.drawn_alpha();
        if alpha < 1.0 {
            put(&mut mtl, format_args!("d {alpha}\n"));
        }
        if m.emissive != [0.0; 3] {
            let ke = m.emissive.map(srgb_extended);
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
    let opts = super::WriteOptions { self_contained: true, ..Default::default() };
    let scene = MeshFormat::Obj.prepare(&super::pose::from_flat(mesh), opts)?;
    Ok(write_scene(&scene, true)?.files.remove(0).bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::package::MemorySidecars;
    use super::super::fixtures::sweeps::{layer_sweep, lean_layered};

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
        assert_eq!(m.name.as_deref(), Some("group1"), "`g` names a part as `o` does");
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

    /// An OBJ's warnings quote what its files name, and a file can name
    /// anything at length. So a warning clips each name or path it quotes to
    /// 256 characters, marked "…" past that, the MTL warnings' library
    /// prefix included, and a read lists 64 distinct warnings and counts the
    /// rest in one line, as threemf.rs does (Ruling A45). A name of ordinary
    /// length is quoted whole, as the missing library's `missing.mtl`.
    #[test]
    fn warnings_are_clipped_and_counted() {
        let long = "n".repeat(100_000);
        let triangle = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
        let r = read_scene(format!("mtllib missing.mtl\nusemtl {long}\n{triangle}").as_bytes(), &NoSidecars).expect("read");
        assert!(r.warnings.iter().any(|w| w.contains("material library \"missing.mtl\" was not supplied")), "{:?}", r.warnings);
        let undefined = r.warnings.iter().find(|w| w.contains("is not defined")).expect("the undefined material's warning");
        let n = undefined.chars().count();
        assert!(n < 400 && undefined.contains("nnn…"), "{n} characters: {}", undefined.chars().take(160).collect::<String>());
        // Item 4's MTL warnings, from a library with a long path and a
        // material with a long name: the transmission it can't carry and a
        // clearcoat out of range.
        let lib = format!("{}.mtl", "l".repeat(300));
        let files = vec![PackageFile { path: lib.clone(), bytes: format!("newmtl {long}\nPr 0.5\nTf 0.8 0.8 0.8\nPc 9\n").into_bytes() }];
        let r = read_scene(format!("mtllib {lib}\nusemtl {long}\n{triangle}").as_bytes(), &MemorySidecars { files: &files }).expect("read");
        assert_eq!(r.warnings.len(), 2, "{} warning(s)", r.warnings.len());
        let prefix = format!("{}… line ", "l".repeat(256));
        for w in &r.warnings {
            let n = w.chars().count();
            assert!(n < 700 && w.starts_with(&prefix), "{n} characters: {}", w.chars().take(300).collect::<String>());
        }
        assert!(r.warnings.iter().any(|w| w.contains("nnn…") && w.contains("(transmission) 0.8 is not carried")), "the material's name, clipped");
        // A hundred distinct warnings: 64 listed, the rest counted.
        let files = vec![PackageFile { path: "m.mtl".into(), bytes: format!("newmtl m\n{}", "map_Kd\n".repeat(100)).into_bytes() }];
        let r = read_scene(format!("mtllib m.mtl\nusemtl m\n{triangle}").as_bytes(), &MemorySidecars { files: &files }).expect("read");
        assert_eq!(r.warnings.len(), 65, "{:?}", &r.warnings[r.warnings.len().saturating_sub(2)..]);
        assert!(r.warnings[..64].iter().all(|w| w.contains("`map_Kd` names no file")), "{:?}", &r.warnings[..2]);
        assert_eq!(r.warnings[64], "... and 36 more warning(s)");
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
        let opts = super::super::WriteOptions { self_contained: false, ..Default::default() };
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
        let opts = super::super::WriteOptions { self_contained: true, ..Default::default() };
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

    /// 2026-10-12 review, finding 24.4: `g` starts a part as `o` does, as
    /// three.js reads it (r170 OBJLoader.js:20, `/^[og]\s*(.+)?/`).
    #[test]
    fn groups_become_parts_as_objects_do() {
        let r = read_scene(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\ng a\nf 1 2 3\ng b\nf 1 2 4\n", &NoSidecars).unwrap();
        let names: Vec<_> = r.scene.nodes.iter().filter(|n| n.mesh.is_some()).map(|n| n.name.clone()).collect();
        assert_eq!(names, [Some("a".to_string()), Some("b".to_string())]);
    }

    /// 2026-10-12 review, finding 24.5: `mtllib`'s rest of line is first one
    /// file name, spaces and all (three.js r170 OBJLoader.js:661 reads it so),
    /// and only then the spec's list of names; past the cap the rest are
    /// ignored with a warning, not the file refused.
    #[test]
    fn a_material_library_name_may_hold_spaces() {
        let files = vec![PackageFile { path: "my mats.mtl".into(), bytes: b"newmtl red\nKd 1 0 0\n".to_vec() }];
        let src = b"mtllib my mats.mtl\nusemtl red\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
        let r = read_scene(src, &MemorySidecars { files: &files }).unwrap();
        assert_eq!(r.scene.materials[0].base_color, [1.0, 0.0, 0.0, 1.0], "{:?}", r.warnings);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let src = format!("mtllib {}\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n", "a.mtl ".repeat(17));
        let r = read_scene(src.as_bytes(), &NoSidecars).unwrap();
        assert!(r.warnings.iter().any(|w| w.contains("material libraries")), "{:?}", r.warnings);
    }

    fn textured_with(mtl: &str) -> SceneRead {
        let files = vec![
            PackageFile { path: "m.mtl".into(), bytes: mtl.as_bytes().to_vec() },
            PackageFile { path: "t.png".into(), bytes: super::super::fixtures::quadrant_png() },
        ];
        let src = b"mtllib m.mtl\nusemtl t\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n";
        read_scene(src, &MemorySidecars { files: &files }).unwrap()
    }

    /// 2026-10-12 review, finding 24.6: `-s u` scales u alone; v keeps the
    /// default of 1 (the MTL spec: "-s u v w … v is optional … The default is
    /// 1, 1, 1").
    #[test]
    fn a_single_scale_number_scales_u_alone() {
        let r = textured_with("newmtl t\nmap_Kd -s 2 t.png\n");
        let t = r.scene.materials[0].base_color_texture.as_ref().unwrap().transform.unwrap();
        assert_eq!((t.scale, t.offset), ([2.0, 1.0], [0.0, 0.0]));
    }

    /// 2026-10-12 review, finding 24.7: an option this reader does not know no
    /// longer drops the texture silently: the option and the numbers after it
    /// are stepped over, and a warning names it.
    #[test]
    fn an_unknown_map_option_is_named_and_the_texture_kept() {
        let r = textured_with("newmtl t\nmap_Kd -foo 1 t.png\n");
        assert!(r.scene.materials[0].base_color_texture.is_some(), "{:?}", r.warnings);
        assert!(r.warnings.iter().any(|w| w.contains("-foo")), "{:?}", r.warnings);
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

    /// 2026-10-12 review, finding 24: a UTF-8 byte-order mark (what Windows
    /// editors put first) glued itself to the first keyword, so the first `v`
    /// was skipped and every positive index pointed one vertex on — or, with
    /// `mtllib` first, the materials went to defaults. three.js's OBJLoader and
    /// MTLLoader read such files.
    #[test]
    fn a_byte_order_mark_is_not_part_of_the_first_statement() {
        let plain = parse(QUAD).unwrap();
        let bom = parse(&format!("\u{feff}{QUAD}")).unwrap();
        assert_eq!(bom, plain);
        let with_v_first = parse("\u{feff}v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
        assert_eq!(with_v_first.positions, [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);

        let obj = "\u{feff}mtllib m.mtl\nusemtl red\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
        let files = vec![PackageFile { path: "m.mtl".into(), bytes: "\u{feff}newmtl red\nKd 1 0 0\n".as_bytes().to_vec() }];
        let r = read_scene(obj.as_bytes(), &MemorySidecars { files: &files }).unwrap();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.scene.materials[0].name.as_deref(), Some("red"));
        assert_eq!(r.scene.materials[0].base_color[..3], [1.0, 0.0, 0.0]);
    }

    /// 2026-10-12 review, finding 10: the index ceiling was checked per
    /// primitive and the node count only on the finished scene, so a file of
    /// `o a / f 1 1 1` ×4M (48 MB) built ~5 GB before anything refused. Both
    /// now count across the file and refuse at the line that crosses them.
    #[test]
    fn objects_and_indices_are_counted_across_the_file_and_refused_where_they_cross() {
        let face = |corners: usize| format!("f {}\n", (0..corners).map(|i| (i % 3 + 1).to_string()).collect::<Vec<_>>().join(" "));
        // 9M indices in one object, then 6M in a second: each under the
        // ceiling, together over it.
        let two = format!("v 0 0 0\nv 1 0 0\nv 0 1 0\no a\n{}o b\n{}", face(3_000_002), face(2_000_002));
        let e = read_scene(two.as_bytes(), &NoSidecars).map(|_| ()).unwrap_err();
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("obj line 7")), "{e:?}");
        let many: String = std::iter::once("v 0 0 0\nv 1 0 0\nv 0 1 0\n".to_string())
            .chain((0..=super::super::scene::MAX_NODES).map(|_| "o a\nf 1 2 3\n".to_string()))
            .collect();
        let e = read_scene(many.as_bytes(), &NoSidecars).map(|_| ()).unwrap_err();
        let at = 4 + 2 * super::super::scene::MAX_NODES;
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains(&format!("obj line {at}"))), "{e:?}");
    }

    /// 2026-10-12 review #22: every material was written `Ks 0.5` and a fully
    /// rough one `Ns 0`, and three.js's MTLLoader draws shininess 0 as a
    /// uniform specular wash. The Phong specular is glTF's F0 (0.04 for a
    /// dielectric, the base colour for a metal), faded by roughness, so a
    /// fully rough surface has no highlight. `Pr`/`Pm` keep the exact values.
    #[test]
    fn the_phong_specular_follows_the_material_so_rough_surfaces_do_not_wash() {
        let mut s = Scene::default();
        for (name, metallic, roughness, color) in [("rough", 0.0, 1.0, [0.8, 0.2, 0.2]), ("glossy", 0.0, 0.0, [0.8, 0.2, 0.2]), ("gold", 1.0, 0.0, [1.0, 0.766, 0.336])] {
            s.materials.push(Material {
                name: Some(name.into()),
                base_color: [color[0], color[1], color[2], 1.0],
                metallic,
                roughness,
                ..Default::default()
            });
        }
        for m in 0..3 {
            s.meshes.push(SceneMesh {
                primitives: vec![Primitive {
                    positions: vec![[0.0, 0.0, m as f32], [1.0, 0.0, m as f32], [0.0, 1.0, m as f32]],
                    indices: vec![0, 1, 2],
                    material: Some(m),
                    ..Default::default()
                }],
                ..Default::default()
            });
            s.add_root(Node { mesh: Some(m), ..Default::default() });
        }
        let pkg = write_scene(&s, false).unwrap();
        let mtl = String::from_utf8(pkg.files.iter().find(|f| f.path == "model.mtl").unwrap().bytes.clone()).unwrap();
        let ks = |name: &str| -> [f32; 3] {
            let block = mtl.split("newmtl ").find(|b| b.starts_with(name)).unwrap_or_else(|| panic!("{name}\n{mtl}"));
            let line = block.lines().find(|l| l.starts_with("Ks ")).unwrap();
            let v: Vec<f32> = line[3..].split_whitespace().map(|x| x.parse().unwrap()).collect();
            [v[0], v[1], v[2]]
        };
        assert_eq!(ks("rough"), [0.0; 3], "no highlight on a fully rough surface\n{mtl}");
        let glossy = ks("glossy").map(super::super::math::srgb_to_linear);
        assert!(glossy.iter().all(|c| (c - 0.04).abs() < 1e-3), "a dielectric's F0: {glossy:?}");
        let gold = ks("gold").map(super::super::math::srgb_to_linear);
        assert!((gold[0] - 1.0).abs() < 1e-3 && (gold[1] - 0.766).abs() < 1e-3 && (gold[2] - 0.336).abs() < 1e-3, "a metal's F0 is its colour: {gold:?}");
    }

    // ---- material layers: Blender's PBR extension to MTL --------------------

    /// The layers an MTL can hold, as one value to compare.
    type Layers = (Option<Clearcoat>, Option<Sheen>, Option<Anisotropy>, Option<f32>);

    fn layers(m: &Material) -> Layers {
        (m.clearcoat.clone(), m.sheen.clone(), m.anisotropy.clone(), m.ior)
    }

    /// One quad per material, each with `sets` uv sets (set k at u = k, so
    /// the sets differ), beside textures 0 (the fixtures' quadrants) and 1
    /// (their bands).
    fn layer_quads(materials: Vec<Material>, sets: usize) -> Scene {
        let mut s = Scene::default();
        for (i, (name, data)) in [("quadrants", super::super::fixtures::quadrant_png()), ("bands", super::super::fixtures::bands_png())].into_iter().enumerate() {
            s.images.push(Image { name: Some(name.into()), format: ImageFormat::Png, data });
            s.textures.push(Texture { name: Some(name.into()), image: i, sampler: Sampler::default() });
        }
        for (i, m) in materials.into_iter().enumerate() {
            let name = m.name.clone();
            s.materials.push(m);
            let z = i as f32;
            let set = |k: usize| vec![[k as f32, 0.0], [k as f32 + 0.5, 0.0], [k as f32 + 0.5, 0.5], [k as f32, 0.5]];
            s.meshes.push(SceneMesh {
                name: name.clone(),
                primitives: vec![Primitive {
                    positions: vec![[0.0, 0.0, z], [1.0, 0.0, z], [1.0, 1.0, z], [0.0, 1.0, z]],
                    uvs: (0..sets).map(set).collect(),
                    indices: vec![0, 1, 2, 0, 2, 3],
                    material: Some(i),
                    ..Default::default()
                }],
                ..Default::default()
            });
            s.add_root(Node { name, mesh: Some(i), ..Default::default() });
        }
        s
    }

    /// `scene` written as an OBJ package (`mesh::write_scene`, so with its
    /// losses) and read back, beside the scene it was prepared as and its MTL.
    struct Through {
        prepared: Scene,
        back: SceneRead,
        losses: Vec<Loss>,
        mtl: String,
    }

    fn through_package(scene: &Scene) -> Through {
        let opts = super::super::WriteOptions { self_contained: false, ..Default::default() };
        let prepared = MeshFormat::Obj.prepare(scene, opts).expect("prepare");
        let w = super::super::write_scene(scene, MeshFormat::Obj, opts).expect("write");
        let mtl = w.package.files.iter().find(|f| f.path == "model.mtl").map(|f| String::from_utf8(f.bytes.clone()).unwrap()).unwrap_or_default();
        let back = read_scene(&w.package.files[0].bytes, &w.package.resolver()).expect("read");
        Through { prepared, back, losses: w.losses, mtl }
    }

    /// The materials losses' `what`s, sorted.
    fn material_losses(losses: &[Loss]) -> Vec<&str> {
        let mut out: Vec<&str> = losses.iter().filter(|l| l.kind == LossKind::Materials).map(|l| l.what.as_str()).collect();
        out.sort_unstable();
        out
    }

    /// The lines of `mtl`'s `newmtl name` block.
    fn mtl_lines<'a>(mtl: &'a str, name: &str) -> Vec<&'a str> {
        let block = mtl.split("\nnewmtl ").find(|b| b.lines().next() == Some(name)).unwrap_or_else(|| panic!("no {name} in\n{mtl}"));
        block.lines().collect()
    }

    fn named<'a>(s: &'a Scene, name: &str) -> &'a Material {
        s.materials.iter().find(|m| m.name.as_deref() == Some(name)).unwrap_or_else(|| panic!("no material {name}"))
    }

    /// The material an MTL defines as `m`, read through a one-triangle OBJ,
    /// and the read's warnings.
    fn mtl_material(mtl: &str) -> (Material, Vec<String>) {
        let files = vec![PackageFile { path: "m.mtl".into(), bytes: mtl.as_bytes().to_vec() }];
        let r = read_scene(b"mtllib m.mtl\nusemtl m\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n", &MemorySidecars { files: &files }).expect("read");
        (r.scene.materials[0].clone(), r.warnings)
    }

    /// Blender's PBR extension to MTL, written and read back in package mode
    /// (Ruling A21): `Pc` and `Pcr` hold a clearcoat, `Ps` a grey sheen,
    /// `aniso` and `anisor` an anisotropy, and `Ni` an ior, each exactly and
    /// in Blender's units (module docs): `Pc` is four times glTF's
    /// clearcoatFactor and `anisor` a fraction of a full turn.
    #[test]
    fn mtl_pbr_extension_round_trips() {
        let coat = |factor, roughness| Some(Clearcoat { factor, roughness, ..Default::default() });
        let plain = |name: &str| Material { name: Some(name.into()), metallic: 0.0, ..Default::default() };
        let materials = vec![
            Material { clearcoat: coat(0.6, 0.2), ior: Some(1.33), ..plain("lacquer") },
            Material { clearcoat: coat(1.0, 0.0), ior: Some(2.42), ..plain("full_coat") },
            Material {
                sheen: Some(Sheen { color: [0.7; 3], roughness: 0.5, ..Default::default() }),
                anisotropy: Some(Anisotropy { strength: 0.8, rotation: 0.6, texture: None }),
                ..plain("satin")
            },
            Material { anisotropy: Some(Anisotropy { strength: 0.25, rotation: 5.0, texture: None }), ior: Some(1.2), ..plain("turned") },
        ];
        let t = through_package(&layer_quads(materials.clone(), 1));
        for (name, want) in [
            ("lacquer", &["Pc 2.4", "Pcr 0.2", "Ni 1.33"][..]),
            ("full_coat", &["Pc 4", "Pcr 0", "Ni 2.42"][..]),
            ("satin", &["Ps 0.7 0.7 0.7", "aniso 0.8"][..]),
            ("turned", &["aniso 0.25", "Ni 1.2"][..]),
        ] {
            for line in want {
                assert!(mtl_lines(&t.mtl, name).contains(line), "{name}: no {line:?} in\n{}", t.mtl);
            }
        }
        // 0.6 rad and 5 rad as fractions of a turn: 0.6/2π and 5/2π.
        let anisor = |name: &str| -> f64 { mtl_lines(&t.mtl, name).iter().find_map(|l| l.strip_prefix("anisor ")).expect("anisor").parse().unwrap() };
        assert!((anisor("satin") - 0.095_492_966).abs() < 1e-8, "{}", anisor("satin"));
        assert!((anisor("turned") - 0.795_774_715).abs() < 1e-8, "{}", anisor("turned"));
        assert!(t.back.warnings.is_empty(), "{:?}", t.back.warnings);
        for m in &materials {
            let name = m.name.as_deref().unwrap();
            assert_eq!(layers(named(&t.prepared, name)), layers(m), "{name}: prepared");
            assert_eq!(layers(named(&t.back.scene, name)), layers(m), "{name}: read back");
        }
        assert!(material_losses(&t.losses).is_empty(), "{:?}", t.losses);
    }

    /// MTL holds sheen as one weight, Blender's Sheen Weight (`Ps`) under a
    /// white tint, with no key for its roughness (Rulings A21, A21d), and
    /// litegen writes no layer maps. A coloured sheen is written as its
    /// brightest channel, in grey: the triple `Ps w w w`, which Blender reads
    /// as w and assimp as (w, w, w), where one value would be its red
    /// (w, 0, 0) (A21h). Its roughness becomes Blender's default of 0.5, and
    /// its maps go; the write names each. A grey sheen at 0.5 loses nothing.
    #[test]
    fn a_sheen_colour_is_reduced_and_reported() {
        let map = |texture| Some(TextureRef { texture, uv_set: 0, transform: None });
        let velvet = Material {
            name: Some("velvet".into()),
            metallic: 0.0,
            sheen: Some(Sheen { color: [0.9, 0.6, 0.8], color_texture: map(0), roughness: 0.4, roughness_texture: map(1) }),
            ..Default::default()
        };
        let felt = Material { name: Some("felt".into()), metallic: 0.0, sheen: Some(Sheen { color: [0.5; 3], roughness: 0.5, ..Default::default() }), ..Default::default() };
        let t = through_package(&layer_quads(vec![velvet, felt], 1));
        let grey = |w: f32| Some(Sheen { color: [w; 3], color_texture: None, roughness: 0.5, roughness_texture: None });
        assert_eq!(named(&t.prepared, "velvet").sheen, grey(0.9), "prepared");
        assert_eq!(named(&t.back.scene, "velvet").sheen, grey(0.9), "read back");
        assert_eq!(named(&t.back.scene, "felt").sheen, grey(0.5));
        assert!(mtl_lines(&t.mtl, "velvet").contains(&"Ps 0.9 0.9 0.9"), "{}", t.mtl);
        assert_eq!(
            material_losses(&t.losses),
            [
                "sheen colour reduced to one channel on 1 material(s)",
                "sheen roughness on 1 material(s)",
                "sheenColor texture on 1 material(s)",
                "sheenRoughness texture on 1 material(s)",
            ]
        );
        // Only the sheen maps used the images, and they went.
        assert!(t.back.scene.images.is_empty());
    }

    /// A `Ps` of three numbers is the sheen colour, as assimp reads it
    /// (code/AssetLib/Obj/ObjFileMtlImporter.cpp:227-230, v6.0.5; Ruling
    /// A21g), and one number stays Blender's grey weight. The sheen exists
    /// when its brightest channel is above 0. A channel outside 0 to 1, or a
    /// count other than one or three, drops it with a warning. Written, a
    /// coloured sheen reduces to its brightest channel and is named, like any
    /// coloured sheen.
    #[test]
    fn a_three_value_ps_is_a_sheen_colour() {
        let sheen = |color| Some(Sheen { color, roughness: 0.5, ..Default::default() });
        for (ps, want) in [("0.9 0.6 0.8", sheen([0.9, 0.6, 0.8])), ("0.3", sheen([0.3; 3])), ("0 0.5 0", sheen([0.0, 0.5, 0.0])), ("0 0 0", None)] {
            let (m, warnings) = mtl_material(&format!("newmtl m\nPs {ps}\n"));
            assert_eq!(m.sheen, want, "Ps {ps}");
            assert!(warnings.is_empty(), "Ps {ps}: {warnings:?}");
        }
        for ps in ["1.2 0.5 0.5", "0.5 -0.1 0.2", "0.5 0.5", "0.5 abc 0.5", "0.1 0.2 0.3 0.4"] {
            let (m, warnings) = mtl_material(&format!("newmtl m\nPs {ps}\n"));
            assert_eq!(m.sheen, None, "Ps {ps}");
            assert!(
                warnings.len() == 1 && warnings[0].starts_with("m.mtl line 2: ") && warnings[0].contains(&format!("`Ps {ps}`")),
                "Ps {ps}: {warnings:?}"
            );
        }
        let (m, _) = mtl_material("newmtl m\nPs 0.9 0.6 0.8\n");
        let t = through_package(&layer_quads(vec![m], 1));
        assert_eq!(t.back.scene.materials[0].sheen, sheen([0.9; 3]));
        assert_eq!(material_losses(&t.losses), ["sheen colour reduced to one channel on 1 material(s)"]);
    }

    /// `Ni` is an ior only in a PBR MTL, one with any of `Pr`, `Pm`, `Ps`,
    /// `Pc`, `Pcr`, `aniso`, `anisor`, `map_Pr` or `map_Pm` (Ruling A21):
    /// classic exporters write `Ni 1` by default, and an ior of 1 takes a
    /// dielectric's reflection away. A classic MTL's `Ni` stays unread and
    /// unremarked. In a PBR MTL an `Ni` glTF's ior can't hold is left out
    /// with a warning naming it: 0 or below, which Blender ignores
    /// (obj_import_mtl.cc:338-341, v5.2.3; A21c), between 0 and 1, which
    /// KHR_materials_ior's schema refuses (A16), or no number (A16i).
    #[test]
    fn ni_is_an_ior_only_in_a_pbr_mtl() {
        for (body, want) in [
            ("Kd 1 1 1\nNi 1.000000\nillum 2", None),
            ("Ni 0", None),
            ("Ni abc", None),
            ("Pr 0.5\nNi 1.33", Some(1.33)),
            ("Ni 1.33\nPm 0", Some(1.33)),
            ("Ni 2.42\nPs 0", Some(2.42)),
            ("Ni 1.2\nPcr 0.2", Some(1.2)),
            ("Pc 0\nNi 1.6", Some(1.6)),
            ("aniso 0\nNi 1.6", Some(1.6)),
            ("anisor 0\nNi 1.6", Some(1.6)),
        ] {
            let (m, warnings) = mtl_material(&format!("newmtl m\n{body}\n"));
            assert_eq!(m.ior, want, "{body}");
            assert!(warnings.is_empty(), "{body}: {warnings:?}");
        }
        // A PBR map says so too; the map itself is dropped with its own warning.
        for body in ["map_Pr r.png\nNi 1.7", "map_Pm m.png\nNi 1.7"] {
            let (m, warnings) = mtl_material(&format!("newmtl m\n{body}\n"));
            assert_eq!(m.ior, Some(1.7), "{body}");
            assert!(!warnings.iter().any(|w| w.contains("`Ni")), "{body}: {warnings:?}");
        }
        for value in ["0", "-1", "0.5", "nan", "abc"] {
            let (m, warnings) = mtl_material(&format!("newmtl m\nPr 0.5\nNi {value}\n"));
            assert_eq!(m.ior, None, "Ni {value}");
            assert!(
                warnings.len() == 1 && warnings[0].starts_with("m.mtl line 3: ") && warnings[0].contains(&format!("`Ni {value}`")),
                "Ni {value}: {warnings:?}"
            );
        }
    }

    /// The `Ni` values Blender writes by default are unset in a PBR MTL, as
    /// Collada's exporter defaults are (Ruling A21f; the module docs cite
    /// them): 1 for a material with no Principled BSDF, 1.45 for 2.8-3.6's
    /// Principled default, which there drew transmission only, and 1.5,
    /// glTF's own default and 4.x's. Written, an ior of 1 or 1.45 is left
    /// out and named, as this reader would drop it; 1.5 is left out unnamed,
    /// since it draws the same.
    #[test]
    fn default_ni_values_read_as_unset() {
        for (value, want) in [("1", None), ("1.000000", None), ("1.45", None), ("1.450000", None), ("1.5", None), ("1.500000", None), ("1.6", Some(1.6))] {
            let (m, warnings) = mtl_material(&format!("newmtl m\nPr 0.5\nNi {value}\n"));
            assert_eq!(m.ior, want, "Ni {value}");
            assert!(warnings.is_empty(), "Ni {value}: {warnings:?}");
        }
        let plain = |name: &str, ior: f32| Material { name: Some(name.into()), metallic: 0.0, ior: Some(ior), ..Default::default() };
        let t = through_package(&layer_quads(vec![plain("nodeless", 1.0), plain("blender3", 1.45), plain("blender4", 1.5), plain("other", 1.6)], 1));
        for (name, want) in [("nodeless", None), ("blender3", None), ("blender4", None), ("other", Some(1.6))] {
            assert_eq!(named(&t.prepared, name).ior, want, "{name}: prepared");
            assert_eq!(named(&t.back.scene, name).ior, want, "{name}: read back");
            assert_eq!(mtl_lines(&t.mtl, name).iter().any(|l| l.starts_with("Ni ")), want.is_some(), "{name}: Ni written\n{}", t.mtl);
        }
        assert_eq!(material_losses(&t.losses), ["an ior of 1 on 1 material(s)", "an ior of 1.45 on 1 material(s)"]);
    }

    /// An MTL as Blender 5.2 writes it in PBR mode: every key at six
    /// decimals, zeros included (obj_export_file_writer.cc:656-683,
    /// obj_export_io.hh:172-175, v5.2.3). It reads as Blender draws it
    /// (Rulings A21a, A21b, A21d), by Blender's importer
    /// (obj_import_mtl.cc:350-365) and Principled BSDF
    /// (node_shader_bsdf_principled.cc), v5.2.3:
    /// - `Pc` is four times the coat weight, which is glTF's clearcoatFactor
    ///   ("Clearcoat used to include an implicit 0.25 factor");
    /// - `Ps` is the sheen weight under a white tint, so a grey sheen colour;
    /// - `anisor` is a fraction of a turn ("with 1.0 going full circle"),
    ///   2π times it glTF's radians, as Blender's glTF add-on converts;
    /// - a weight of 0 adds no layer, and a `Pcr` or `anisor` without its
    ///   weight adds nothing;
    /// - what the MTL leaves unsaid is Blender's default: a coat roughness
    ///   of 0.03, a sheen roughness of 0.5 and an anisotropic rotation of 0;
    /// - its `Ni 1.500000`, Blender 4.x's default IOR, is unset (A21f).
    #[test]
    fn a_blender_pbr_mtl_reads_as_blender_draws_it() {
        let blender = |ps: f32, pc: f32, pcr: f32, aniso: f32, anisor: f32| {
            format!(
                "newmtl m\nKd 0.800000 0.800000 0.800000\nKs 0.500000 0.500000 0.500000\nKe 0.000000 0.000000 0.000000\nNi 1.500000\n\
                 d 1.000000\nillum 2\nPr 0.500000\nPm 0.000000\nPs {ps:.6}\nPc {pc:.6}\nPcr {pcr:.6}\naniso {aniso:.6}\nanisor {anisor:.6}\n"
            )
        };
        let coat = |factor, roughness| Some(Clearcoat { factor, roughness, ..Default::default() });
        let sheen = |w: f32| Some(Sheen { color: [w; 3], roughness: 0.5, ..Default::default() });
        let aniso = |strength, rotation| Some(Anisotropy { strength, rotation, texture: None });
        for (mtl, want) in [
            // A full coat, at the coat roughness Blender writes by default.
            (blender(0.0, 4.0, 0.03, 0.0, 0.0), (coat(1.0, 0.03), None, None, None)),
            // Sheen and a quarter-turn anisotropy; a coat roughness with no coat.
            (blender(0.3, 0.0, 0.5, 0.5, 0.25), (None, sheen(0.3), aniso(0.5, std::f32::consts::FRAC_PI_2), None)),
            // What the file leaves unsaid.
            ("newmtl m\nPc 1\naniso 0.5\nPs 1\n".to_string(), (coat(0.25, 0.03), sheen(1.0), aniso(0.5, 0.0), None)),
        ] {
            let (m, warnings) = mtl_material(&mtl);
            assert_eq!(layers(&m), want, "{mtl}");
            assert!(warnings.is_empty(), "{mtl}: {warnings:?}");
        }
    }

    /// A layer value glTF's field can't hold, or that is no number, is
    /// dropped with a warning naming its statement and line, never clamped
    /// (Rulings A16, A16i, A21a): `Pc` outside 0 to 4 (a coat weight outside
    /// 0 to 1), and `Ps`, `aniso`, `Pcr` or `anisor` outside 0 to 1. A
    /// dropped weight drops its layer; a dropped `Pcr` or `anisor` leaves
    /// Blender's default. Without its weight, a `Pcr` or `anisor` adds
    /// nothing and says nothing.
    #[test]
    fn a_layer_value_out_of_range_is_dropped_with_a_warning() {
        let coat = |factor, roughness| Some(Clearcoat { factor, roughness, ..Default::default() });
        let aniso = |strength, rotation| Some(Anisotropy { strength, rotation, texture: None });
        let cases: [(&str, Option<&str>, Layers); 14] = [
            ("Pc 5", Some("`Pc 5`"), (None, None, None, None)),
            ("Pc -1", Some("`Pc -1`"), (None, None, None, None)),
            ("Pc abc", Some("`Pc abc`"), (None, None, None, None)),
            ("Pc inf", Some("`Pc inf`"), (None, None, None, None)),
            ("Pc 2\nPcr 1.5", Some("`Pcr 1.5`"), (coat(0.5, 0.03), None, None, None)),
            ("Pc 2\nPcr nan", Some("`Pcr nan`"), (coat(0.5, 0.03), None, None, None)),
            ("Ps 1.2", Some("`Ps 1.2`"), (None, None, None, None)),
            ("Ps -0.5", Some("`Ps -0.5`"), (None, None, None, None)),
            ("aniso -0.1", Some("`aniso -0.1`"), (None, None, None, None)),
            ("aniso nan", Some("`aniso nan`"), (None, None, None, None)),
            ("aniso 0.5\nanisor 2", Some("`anisor 2`"), (None, None, aniso(0.5, 0.0), None)),
            ("Pcr 3", None, (None, None, None, None)),
            ("anisor 7", None, (None, None, None, None)),
            ("Pcr junk\nanisor junk", None, (None, None, None, None)),
        ];
        for (body, statement, want) in cases {
            let (m, warnings) = mtl_material(&format!("newmtl m\n{body}\n"));
            assert_eq!(layers(&m), want, "{body}");
            match statement {
                Some(s) => assert!(
                    warnings.len() == 1 && warnings[0].starts_with(&format!("m.mtl line {}: ", 1 + body.lines().count())) && warnings[0].contains(s),
                    "{body}: {warnings:?}"
                ),
                None => assert!(warnings.is_empty(), "{body}: {warnings:?}"),
            }
        }
    }

    /// The maps an MTL can't hold go before `degrade` chooses OBJ's one uv
    /// set (Ruling A8): it keeps the set the kept maps sample, and a
    /// clearcoat map is not one of them, so a clearcoat map on set 1 doesn't
    /// take it from set 0. The coat stays, and its map is named.
    #[test]
    fn a_layer_map_never_takes_objs_uv_set() {
        let coat = Material {
            name: Some("coat".into()),
            metallic: 0.0,
            clearcoat: Some(Clearcoat { factor: 0.5, texture: Some(TextureRef { texture: 0, uv_set: 1, transform: None }), ..Default::default() }),
            ..Default::default()
        };
        let scene = layer_quads(vec![coat], 2);
        let set0 = scene.meshes[0].primitives[0].uvs[0].clone();
        let t = through_package(&scene);
        for (label, s) in [("prepared", &t.prepared), ("read back", &t.back.scene)] {
            let uvs = &s.meshes[0].primitives[0].uvs;
            assert_eq!((uvs.len(), &uvs[0]), (1, &set0), "{label}: OBJ keeps set 0");
            assert_eq!(s.materials[0].clearcoat, Some(Clearcoat { factor: 0.5, ..Default::default() }), "{label}");
        }
        assert_eq!(material_losses(&t.losses), ["clearcoat texture on 1 material(s)"]);
    }

    /// What MTL can't hold of the layers OBJ keeps goes in `prepare`, and the
    /// write names each part by material count, never as the layer's own
    /// loss ("clearcoat on ..."):
    /// - a kept layer's maps, the clearcoat's own normal map among them (A8);
    /// - an ior of 0, which Blender reads as unset (A21c);
    /// - a schema-invalid value glTF passes through, which the reader would
    ///   refuse (A16): a clearcoat factor, sheen colour or anisotropy
    ///   strength outside 0 to 1 drops its layer, and a clearcoat roughness
    ///   outside 0 to 1 becomes Blender's 0.03.
    ///
    /// A layer at its off value goes unnamed, as it draws nothing (A21b); an
    /// anisotropy rotation is written as a turn in [0, 1). The package reads
    /// back as prepared. One file holds no layers, and names them whole.
    #[test]
    fn what_an_mtl_cannot_hold_is_dropped_and_named() {
        let map = |texture| Some(TextureRef { texture, uv_set: 0, transform: None });
        let plain = |name: &str| Material { name: Some(name.into()), metallic: 0.0, ..Default::default() };
        let materials = vec![
            Material {
                clearcoat: Some(Clearcoat { factor: 1.0, texture: map(0), roughness: 0.1, roughness_texture: map(1), normal_texture: map(0), normal_scale: 0.5 }),
                ..plain("car_paint")
            },
            Material { anisotropy: Some(Anisotropy { strength: 0.7, rotation: -1.0, texture: map(1) }), ..plain("steel") },
            Material {
                clearcoat: Some(Clearcoat { factor: 0.0, texture: map(0), roughness: 0.3, ..Default::default() }),
                sheen: Some(Sheen { color: [0.0; 3], roughness: 0.2, ..Default::default() }),
                anisotropy: Some(Anisotropy { strength: 0.0, rotation: 1.0, texture: None }),
                ..plain("off")
            },
            Material {
                clearcoat: Some(Clearcoat { factor: 1.5, roughness: 0.2, ..Default::default() }),
                sheen: Some(Sheen { color: [1.2, 0.2, 0.2], roughness: 0.5, ..Default::default() }),
                anisotropy: Some(Anisotropy { strength: 1.3, rotation: 0.0, texture: None }),
                ior: Some(0.0),
                ..plain("invalid")
            },
            Material { clearcoat: Some(Clearcoat { factor: 0.5, roughness: 1.5, ..Default::default() }), ..plain("rough_coat") },
        ];
        let scene = layer_quads(materials, 1);
        let t = through_package(&scene);
        let coat = |factor, roughness| Some(Clearcoat { factor, roughness, ..Default::default() });
        for (name, want) in [
            ("car_paint", (coat(1.0, 0.1), None, None, None)),
            ("off", (None, None, None, None)),
            ("invalid", (None, None, None, None)),
            ("rough_coat", (coat(0.5, 0.03), None, None, None)),
        ] {
            assert_eq!(layers(named(&t.prepared, name)), want, "{name}: prepared");
        }
        // −1 rad is 2π − 1 as a turn in [0, 1).
        let steel = named(&t.prepared, "steel").anisotropy.clone().expect("steel's anisotropy");
        assert_eq!((steel.strength, &steel.texture), (0.7, &None));
        assert!((steel.rotation - (2.0 * std::f32::consts::PI - 1.0)).abs() < 1e-6, "{}", steel.rotation);
        for m in &t.prepared.materials {
            let name = m.name.as_deref().unwrap();
            assert_eq!(layers(named(&t.back.scene, name)), layers(m), "{name}: read back as prepared");
        }
        assert!(t.back.warnings.is_empty(), "{:?}", t.back.warnings);
        assert_eq!(
            material_losses(&t.losses),
            [
                "an ior of 0 on 1 material(s)",
                "an out-of-range anisotropy strength on 1 material(s)",
                "an out-of-range clearcoat factor on 1 material(s)",
                "an out-of-range clearcoat roughness on 1 material(s)",
                "an out-of-range sheen colour on 1 material(s)",
                "anisotropy texture on 1 material(s)",
                "clearcoat texture on 1 material(s)",
                "clearcoatNormal texture on 1 material(s)",
                "clearcoatRoughness texture on 1 material(s)",
            ]
        );
        let single = super::super::write_scene(&scene, MeshFormat::Obj, super::super::WriteOptions::default()).expect("write one file");
        let whole = material_losses(&single.losses);
        for layer in ["clearcoat on 4 material(s)", "sheen on 2 material(s)", "anisotropy on 3 material(s)", "ior on 1 material(s)"] {
            assert!(whole.contains(&layer), "{layer}: {whole:?}");
        }
        assert!(!whole.iter().any(|w| w.contains(" texture on ") || w.starts_with("an ")), "{whole:?}");
    }

    /// In a PBR MTL, `Tf t t t` is Blender's Transmission Weight
    /// (obj_export_mtl.cc:249, :256, :304 and obj_export_file_writer.cc:
    /// 684-688 write it; obj_import_mtl.cc:367-373 reads the three's mean,
    /// a missing one as 0, v5.2.3), and so is `Kt`, which Blender's reader
    /// takes as `Tf` (obj_import_file_reader.cc:967). OBJ keeps no
    /// transmission, so a weight above 0 is named and not carried (Rulings
    /// A21i, A21j). A classic MTL's `Tf` or `Kt`, a transmission filter
    /// colour such as Maya's `Tf 1 1 1`, stays silent.
    #[test]
    fn transmission_in_a_pbr_mtl_is_named_not_carried() {
        for (body, named) in [
            ("Pr 0.5\nTf 0.8 0.8 0.8", Some("Tf (transmission) 0.8 is not carried")),
            ("Pr 0.5\nTf 0.6", Some("Tf (transmission) 0.2 is not carried")),
            ("Pr 0.5\nTf 0 0 0", None),
            ("Kd 1 1 1\nTf 1 1 1", None),
            ("Pr 0.5\nKt 0.8 0.8 0.8", Some("Kt (transmission) 0.8 is not carried")),
            ("Pr 0.5\nKt 0 0 0", None),
            ("Kd 1 1 1\nKt 1 1 1", None),
        ] {
            let (m, warnings) = mtl_material(&format!("newmtl m\n{body}\n"));
            assert_eq!(m.transmission, None, "{body}");
            match named {
                Some(text) => assert!(warnings.len() == 1 && warnings[0].starts_with("m.mtl line 3: ") && warnings[0].contains(text), "{body}: {warnings:?}"),
                None => assert!(warnings.is_empty(), "{body}: {warnings:?}"),
            }
        }
    }

    /// Blender's sheen map, `map_Ps`, holds a Sheen Weight
    /// (obj_export_file_writer.cc:561-567, :717; obj_import_file_reader.cc:
    /// 753-754, v5.2.3), not glTF's sheen colour: the reader drops it and
    /// names it as what it is.
    #[test]
    fn a_sheen_weight_map_is_named_and_dropped() {
        let (m, warnings) = mtl_material("newmtl m\nmap_Ps sheen.png\n");
        assert_eq!(m.sheen, None);
        assert!(warnings.len() == 1 && warnings[0].contains("`map_Ps` (a sheen weight map)"), "{warnings:?}");
    }

    /// A package of one MTL, at `.0`, and no other file.
    struct OnlyMtl<'a>(&'a str, &'a [u8]);

    impl Sidecars for OnlyMtl<'_> {
        fn get(&self, path: &str) -> Option<Vec<u8>> {
            (path == self.0).then(|| self.1.to_vec())
        }
    }

    /// `scene`'s OBJ package, read whole, where it must carry two
    /// clearcoats, a sheen, an anisotropy and two iors, then its OBJ swept
    /// by [`layer_sweep`] with its MTL beside it, and its MTL through its
    /// OBJ. Every reading is `Ok` or `Err`. The MTL is under 4 KiB, so its
    /// single flips take every bit.
    fn obj_layer_sweeps(label: &str, scene: &Scene) {
        let read = |obj: &[u8], mtl: &OnlyMtl| super::super::read_scene(obj, MeshFormat::Obj, mtl);
        let opts = super::super::WriteOptions { self_contained: false, ..Default::default() };
        let pkg = super::super::write_scene(scene, MeshFormat::Obj, opts).unwrap().package;
        let paths: Vec<&str> = pkg.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["model.obj", "model.mtl"], "{label}: the package is the OBJ and its MTL");
        let (obj, mtl) = (&pkg.files[0].bytes, &pkg.files[1].bytes);
        let whole = read(obj, &OnlyMtl("model.mtl", mtl)).unwrap_or_else(|e| panic!("{label}'s package: {e}"));
        let has = |layer| whole.scene.materials.iter().filter(|m| m.has_layer(layer)).count();
        assert_eq!(
            [MaterialLayer::Clearcoat, MaterialLayer::Sheen, MaterialLayer::Anisotropy, MaterialLayer::Ior].map(has),
            [2, 1, 1, 2],
            "{label}'s package itself carries two clearcoats, a sheen, an anisotropy and two iors"
        );
        layer_sweep(&format!("{label}'s OBJ"), obj, 0..0, |bytes| {
            let _ = read(bytes, &OnlyMtl("model.mtl", mtl));
        });
        layer_sweep(&format!("{label}'s MTL"), mtl, 0..mtl.len(), |bytes| {
            let _ = read(obj, &OnlyMtl("model.mtl", bytes));
        });
    }

    /// The MTL statements item 4 reads, Blender's PBR extension (`Pc`, `Pcr`,
    /// `Ps` as one value and as three, `Ni`, `aniso`, `anisor`, and the `Tf`,
    /// `Kt` and `map_Ps` it names), through the reader: lean layered's
    /// package ([`obj_layer_sweeps`] over [`lean_layered`]), with the six of
    /// layered's materials the MTL carries layers for (all but the soap
    /// film, whose iridescence OBJ drops); and, for what the writer never
    /// writes (a one-value `Ps`, `Tf`, `Kt` and `map_Ps`), an MTL swept by
    /// [`layer_sweep`] through a two-face OBJ. Every reading is `Ok` or
    /// `Err`.
    #[test]
    fn obj_layer_inputs_survive_truncation_and_bit_flips() {
        obj_layer_sweeps("lean layered", &lean_layered(&["car_paint", "glass", "velvet", "plastic", "brushed_steel", "lacquer"]));
        let read = |obj: &[u8], mtl: &OnlyMtl| super::super::read_scene(obj, MeshFormat::Obj, mtl);
        let unwritten = b"newmtl m\nPr 0.5\nPs 0.4\nTf 0.8 0.8 0.8\nmap_Ps sheen.png\nnewmtl k\nPr 0.5\nKt 0.8 0.8 0.8\n";
        let two_faces = b"mtllib m.mtl\nusemtl m\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\nusemtl k\nf 1 2 3\n";
        let whole = read(two_faces, &OnlyMtl("m.mtl", unwritten)).unwrap_or_else(|e| panic!("the unwritten statements: {e}"));
        assert_eq!(named(&whole.scene, "m").sheen.as_ref().map(|s| s.color), Some([0.4; 3]), "the one-value Ps");
        for said in ["Tf (transmission) 0.8", "Kt (transmission) 0.8", "`map_Ps` (a sheen weight map)"] {
            assert!(whole.warnings.iter().any(|w| w.contains(said)), "no warning has {said:?}: {:?}", whole.warnings);
        }
        layer_sweep("an MTL of the statements the writer never writes", unwritten, 0..unwritten.len(), |bytes| {
            let _ = read(two_faces, &OnlyMtl("m.mtl", bytes));
        });
    }

    /// [`obj_layer_sweeps`] over the full-size `fixtures::layered()`: a
    /// minute in a debug build, seconds released. Run it with the other
    /// full-size sweeps:
    /// `cargo test --release --lib -- --ignored layer_inputs_survive`.
    #[test]
    #[ignore = "full-size layered: minutes in a debug build; run it released (see the doc comment)"]
    fn obj_layer_inputs_survive_truncation_and_bit_flips_full() {
        obj_layer_sweeps("layered", &super::super::fixtures::layered());
    }

    // ---- what an MTL cannot say of sidedness and alpha (Ruling A36) ---------

    /// MTL has no double-sided flag, so a package reads back single-sided:
    /// `prepare` gives the scene as it reads back, and the loss still names
    /// it. (Task 5b's Khronos import: SheenCloth, ToyCar,
    /// AnisotropyRotationTest and TransmissionRoughnessTest drew back faces in
    /// the expected picture that their obj -> glb output did not.)
    #[test]
    fn prepare_leaves_the_double_sidedness_an_mtl_cannot_hold() {
        let panel = Material { name: Some("panel".into()), metallic: 0.0, double_sided: true, ..Default::default() };
        let t = through_package(&layer_quads(vec![panel], 1));
        assert!(!named(&t.prepared, "panel").double_sided, "prepared");
        assert!(!named(&t.back.scene, "panel").double_sided, "read back");
        assert!(material_losses(&t.losses).contains(&"double-sidedness of 1 material(s)"), "{:?}", t.losses);
    }

    /// An MTL says a material's alpha with `d`, written from the colour's
    /// alpha (`drawn_alpha`), and the reader blends only below 1. So a BLEND
    /// material whose alpha lives in its base-colour texture alone reads back
    /// OPAQUE: `prepare` makes it OPAQUE and the loss names it. A BLEND with a
    /// colour alpha below 1 keeps its `d` and its blending (its texture's
    /// alpha multiplies in as before); one with neither a texture nor a
    /// colour alpha loses nothing and goes OPAQUE unnamed. (Task 5b: Khronos
    /// ClearCoatTest's Partial_Coating.)
    #[test]
    fn a_blend_whose_alpha_lives_in_its_texture_is_prepared_opaque_and_named() {
        let map = Some(TextureRef { texture: 0, uv_set: 0, transform: None });
        let blend = |name: &str, alpha: f32, base_color_texture: Option<TextureRef>| Material {
            name: Some(name.into()),
            alpha_mode: AlphaMode::Blend,
            base_color: [1.0, 1.0, 1.0, alpha],
            base_color_texture,
            metallic: 0.0,
            ..Default::default()
        };
        let t = through_package(&layer_quads(vec![blend("decal", 1.0, map.clone()), blend("veil", 0.5, map), blend("plain", 1.0, None)], 1));
        for (name, want) in [("decal", AlphaMode::Opaque), ("veil", AlphaMode::Blend), ("plain", AlphaMode::Opaque)] {
            assert_eq!(named(&t.prepared, name).alpha_mode, want, "{name}, prepared");
            assert_eq!(named(&t.back.scene, name).alpha_mode, want, "{name}, read back");
        }
        let alpha: Vec<&str> = t.losses.iter().filter(|l| l.kind == LossKind::Alpha).map(|l| l.what.as_str()).collect();
        assert_eq!(alpha, ["alpha blending of 1 material(s)"]);
    }

    /// MTL has no alpha test either: a MASK material is drawn wholly opaque
    /// (its colour's alpha, `drawn_alpha`, at or above the cutoff: no `d`, and
    /// its texture's alpha is not read) or not at all (below it: `d 0`, which
    /// reads back as a BLEND of alpha 0). `prepare` gives the scene as it reads
    /// back, and the loss names the cutoff. (Item 3, Task 5: Khronos
    /// StainedGlassLamp's grill and glass are MASK over KTX2 textures with
    /// cut-out texels, and their `obj -> glb` `lit` picture differed from the
    /// expected one because `prepare` kept the MASK.)
    #[test]
    fn a_mask_is_prepared_as_the_wholly_opaque_or_absent_material_an_mtl_reads_back() {
        let map = Some(TextureRef { texture: 0, uv_set: 0, transform: None });
        let mask = |name: &str, alpha: f32, base_color_texture: Option<TextureRef>| Material {
            name: Some(name.into()),
            alpha_mode: AlphaMode::Mask,
            alpha_cutoff: 0.5,
            base_color: [1.0, 1.0, 1.0, alpha],
            base_color_texture,
            metallic: 0.0,
            ..Default::default()
        };
        let t = through_package(&layer_quads(vec![mask("cutout", 1.0, map.clone()), mask("flat", 0.8, None), mask("gone", 0.2, None)], 1));
        // (name, alpha mode, the colour's alpha) as prepared and as read back.
        for (name, mode, alpha) in [("cutout", AlphaMode::Opaque, 1.0), ("flat", AlphaMode::Opaque, 1.0), ("gone", AlphaMode::Blend, 0.0)] {
            for (which, s) in [("prepared", &t.prepared), ("read back", &t.back.scene)] {
                assert_eq!(named(s, name).alpha_mode, mode, "{name}, {which}");
                assert_eq!(named(s, name).base_color[3], alpha, "{name}, {which}");
            }
        }
        // What is written: no `d` for the two drawn opaque, `d 0` for the absent one.
        let has_d = |name: &str| mtl_lines(&t.mtl, name).iter().any(|l| l.starts_with("d "));
        assert!(!has_d("cutout") && !has_d("flat"), "{}", t.mtl);
        assert!(mtl_lines(&t.mtl, "gone").contains(&"d 0"), "{}", t.mtl);
        let alpha: Vec<&str> = t.losses.iter().filter(|l| l.kind == LossKind::Alpha).map(|l| l.what.as_str()).collect();
        assert_eq!(alpha, ["alpha cutoff of 3 material(s)"]);
    }

    // ---- points and lines (mesh-gaps item 10, Task 3) -------------------------

    mod points_and_lines {
        use super::*;
        use super::super::super as mesh;
        use mesh::scene::Topology;

        /// The elements `topology` draws in `s`: triangles, segments or points.
        fn elements(s: &Scene, topology: Topology) -> usize {
            s.meshes
                .iter()
                .flat_map(|m| &m.primitives)
                .filter(|p| p.topology == topology)
                .map(|p| p.indices.len() / topology.indices_per_element())
                .sum()
        }

        /// The first primitive of the mesh called `name`.
        fn prim_of<'a>(s: &'a Scene, name: &str) -> &'a Primitive {
            let mesh = s.meshes.iter().find(|m| m.name.as_deref() == Some(name));
            &mesh.unwrap_or_else(|| panic!("no mesh called {name}")).primitives[0]
        }

        /// A linear colour from the sRGB bytes-as-fractions a `v` line holds.
        fn lin(r: f32, g: f32, b: f32) -> [f32; 4] {
            [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), 1.0]
        }

        fn read_ok(src: &str) -> SceneRead {
            let r = read_scene(src.as_bytes(), &NoSidecars);
            assert!(r.is_ok(), "{:?}\n{src}", r.as_ref().err());
            r.unwrap()
        }

        fn refused(src: &str) -> MeshError {
            read_scene(src.as_bytes(), &NoSidecars).err().unwrap_or_else(|| panic!("read, expected a refusal:\n{src}"))
        }

        /// The statements of each `o` section of an OBJ's text, by the
        /// object's name.
        fn sections(text: &str) -> Vec<(String, Vec<&str>)> {
            let mut out: Vec<(String, Vec<&str>)> = Vec::new();
            for line in text.lines() {
                if let Some(name) = line.strip_prefix("o ") {
                    out.push((name.to_string(), Vec::new()));
                } else if let Some(last) = out.last_mut() {
                    last.1.push(line);
                }
            }
            out
        }

        fn count(section: &[&str], keyword: &str) -> usize {
            section.iter().filter(|l| l.starts_with(keyword)).count()
        }

        /// `scene` as an OBJ in one file or a package: its text, its package
        /// and the losses the write reported.
        fn written(scene: &Scene, self_contained: bool) -> (String, Package, Vec<Loss>) {
            let opts = mesh::WriteOptions { self_contained, ..Default::default() };
            let w = mesh::write_scene(scene, MeshFormat::Obj, opts);
            assert!(w.is_ok(), "write: {:?}", w.as_ref().err());
            let w = w.unwrap();
            (String::from_utf8(w.package.files[0].bytes.clone()).unwrap(), w.package, w.losses)
        }

        // ── reading ─────────────────────────────────────────────────────────

        /// `p v…` gives points and `l v…` a polyline's segments: a polyline of
        /// four vertices gives three, in the order it walks them. The colours
        /// of the `v` lines come with them, and nothing is skipped or warned.
        /// A negative index counts back from the last vertex, as for faces.
        #[test]
        fn obj_reads_p_and_l() {
            let r = read_ok("o loop\nv 0 0 0 1 0 0\nv 1 0 0 0 1 0\nv 1 1 0 0 0 1\nv 0 1 0 1 1 0\nl 1 2 3 4\no dots\np 1 3\np -1\n");
            assert!(r.warnings.is_empty(), "{:?}", r.warnings);
            let (s, ring, dots) = (&r.scene, prim_of(&r.scene, "loop"), prim_of(&r.scene, "dots"));
            assert_eq!((elements(s, Topology::Lines), elements(s, Topology::Points), s.triangle_count(), s.drawable_count()), (3, 3, 0, 6));
            assert_eq!((ring.topology, &ring.indices), (Topology::Lines, &vec![0, 1, 1, 2, 2, 3]));
            assert_eq!(ring.positions, [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]]);
            assert_eq!(ring.colors, Some(vec![lin(1.0, 0.0, 0.0), lin(0.0, 1.0, 0.0), lin(0.0, 0.0, 1.0), lin(1.0, 1.0, 0.0)]));
            // `p 1 3` and `p -1` (the fourth vertex): three points, each
            // vertex once (the corner cache), numbered as met.
            assert_eq!((dots.topology, &dots.indices), (Topology::Points, &vec![0, 1, 2]));
            assert_eq!(dots.positions, [[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]]);
            assert_eq!(dots.colors, Some(vec![lin(1.0, 0.0, 0.0), lin(0.0, 0.0, 1.0), lin(1.0, 1.0, 0.0)]));
            // And the scene is valid, as every reader's must be.
            s.validate().expect("valid");
        }

        /// Review Focus 4: `l` may carry `v/vt` pairs. The texture
        /// coordinates are read as texture coordinates (V flipped, as for
        /// faces), and the position is the one the first number names. The
        /// file has four vertices and two `vt`, and the line starts at the
        /// third vertex with the first `vt`: a reader that took the `vt`
        /// index for a position would draw from the first vertex. The same
        /// vertex with another `vt` is another vertex of the scene, as for
        /// a face corner.
        #[test]
        fn obj_l_with_texture_coordinates() {
            let head = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0.25 0.25\nvt 0.75 0.5\n";
            let r = read_ok(&format!("{head}l 3/1 4/2\n"));
            let p = &r.scene.meshes[0].primitives[0];
            assert_eq!((p.topology, &p.indices), (Topology::Lines, &vec![0, 1]));
            assert_eq!(p.positions, [[1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], "the positions are the first numbers'");
            assert_eq!(p.uvs, [[[0.25, 0.75], [0.75, 0.5]]], "the texture coordinates are the second numbers'");
            let r = read_ok(&format!("{head}l 1/1 2/2 1/2\n"));
            let p = &r.scene.meshes[0].primitives[0];
            assert_eq!(p.indices, [0, 1, 1, 2], "three corners, one polyline: two segments");
            assert_eq!(p.positions, [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 0.0]], "vertex 1 with another vt is another vertex");
            // A `vt` past the list is out of range, named as one.
            match refused(&format!("{head}l 1/9 2/1\n")) {
                MeshError::Malformed(d) => assert!(d.contains("texture index 9") && d.contains("out of range"), "{d}"),
                e => panic!("{e:?}"),
            }
            // A line that has them for some corners only cannot carry them
            // for the rest, as a face cannot.
            assert!(matches!(refused(&format!("{head}l 1/1 2\n")), MeshError::Unsupported(_)));
        }

        /// A line or a point takes the material in force, as a face does, and
        /// is a primitive of its own beside the object's faces (a mesh is the
        /// faces, the lines and the points of one `o`, per material).
        #[test]
        fn a_line_or_point_takes_the_material_in_force() {
            let r = read_ok("v 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl red\nf 1 2 3\nl 1 2\nusemtl blue\np 3\n");
            let s = &r.scene;
            assert_eq!(s.meshes.len(), 1);
            let seen: Vec<(Topology, Option<&str>)> = s.meshes[0]
                .primitives
                .iter()
                .map(|p| (p.topology, p.material.and_then(|m| s.materials[m].name.as_deref())))
                .collect();
            assert_eq!(seen, [(Topology::Triangles, Some("red")), (Topology::Lines, Some("red")), (Topology::Points, Some("blue"))]);
        }

        /// The old test of the refusal and the warning, now that the scene
        /// draws lines and points: beside faces they are read (no warning),
        /// and a file of nothing else is a scene.
        #[test]
        fn lines_and_points_are_read_beside_faces_and_alone() {
            let r = read_ok("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\nl 1 2\np 1\nl 2 3\n");
            assert!(r.warnings.is_empty(), "{:?}", r.warnings);
            assert_eq!((r.scene.triangle_count(), elements(&r.scene, Topology::Lines), elements(&r.scene, Topology::Points)), (1, 2, 1));
            let r = read_ok("v 0 0 0\nv 1 0 0\nl 1 2\n");
            assert_eq!((r.scene.triangle_count(), elements(&r.scene, Topology::Lines)), (0, 1));
        }

        /// The `Empty` gate is about what is drawn, not triangles: through
        /// the converter's `read_scene`, a file of lines alone and a file of
        /// points alone are scenes. A gate that counts triangles refuses both
        /// as `Empty`.
        #[test]
        fn a_file_of_lines_alone_or_points_alone_is_a_scene() {
            for (src, topology, want) in [("v 0 0 0\nv 1 0 0\nl 1 2\n", Topology::Lines, 1), ("v 0 0 0\nv 1 0 0\np 1 2\n", Topology::Points, 2)] {
                let r = mesh::read_scene(src.as_bytes(), MeshFormat::Obj, &NoSidecars);
                assert!(r.is_ok(), "{topology:?}: {:?}", r.as_ref().err());
                let scene = r.unwrap().scene;
                assert_eq!((scene.triangle_count(), scene.drawable_count(), elements(&scene, topology)), (0, want, want), "{topology:?}");
                // The flat view draws triangles only: it has none to give.
                assert!(matches!(mesh::read(src.as_bytes(), MeshFormat::Obj), Err(MeshError::Empty)), "{topology:?}: the flat read");
            }
            // And vertices alone still draw nothing.
            assert!(matches!(mesh::read_scene(b"v 0 0 0\nv 1 0 0\n", MeshFormat::Obj, &NoSidecars), Err(MeshError::Empty)));
        }

        /// A line needs two vertices and a point one; the refusal names which.
        #[test]
        fn a_line_of_one_vertex_and_a_point_of_none_are_refused() {
            for (src, needle) in [("v 0 0 0\nl 1\n", "line has 1 vertices"), ("v 0 0 0\np\n", "point has no vertices")] {
                match refused(src) {
                    MeshError::Malformed(d) => assert!(d.contains("line 2") && d.contains(needle), "{d}"),
                    e => panic!("{e:?}"),
                }
            }
        }

        // ── writing ─────────────────────────────────────────────────────────

        /// Research Q6: three.js's `OBJLoader` makes a polyline's vertices
        /// straight into `LineSegments`, so a loop is written as one `l a b`
        /// per segment, and an object with an `l` or `p` in it is wholly
        /// lines or points, so each goes in an object of its own, with a
        /// `usemtl` of its own: without an MTL the loader caches materials
        /// by name and the first object's kind wins. `wire` in one file and
        /// in a package.
        #[test]
        fn lines_and_points_are_written_in_objects_of_their_own() {
            let wire = mesh::fixtures::wire();
            for self_contained in [true, false] {
                let ctx = if self_contained { "one file" } else { "a package" };
                let (text, package, _) = written(&wire, self_contained);
                let secs = sections(&text);
                let names: Vec<&str> = secs.iter().map(|(n, _)| n.as_str()).collect();
                assert_eq!(names, ["wire_cube", "wire_loop", "wire_points"], "{ctx}");
                let (cube, ring, dots) = (&secs[0].1, &secs[1].1, &secs[2].1);
                for (what, sec, f, l, p) in [("the cube", cube, 12, 0, 0), ("the loop", ring, 0, 4, 0), ("the grid", dots, 0, 0, 9)] {
                    assert_eq!((count(sec, "f "), count(sec, "l "), count(sec, "p ")), (f, l, p), "{ctx}: {what}");
                    assert_eq!(count(sec, "usemtl "), 1, "{ctx}: {what} names its material");
                }
                // One `l a b` per segment, each segment's two numbers alone.
                let base = count(cube, "v ");
                let numbers = |sec: &[&str], key: &str| -> Vec<Vec<usize>> {
                    sec.iter().filter(|l| l.starts_with(key)).map(|l| l[key.len()..].split(' ').map(|n| n.parse().unwrap()).collect()).collect()
                };
                assert_eq!(
                    numbers(ring, "l "),
                    [vec![base + 1, base + 2], vec![base + 2, base + 3], vec![base + 3, base + 4], vec![base + 4, base + 1]],
                    "{ctx}: the loop's four segments, one `l` each"
                );
                let first_dot = base + count(ring, "v ");
                let dot = |n: usize| vec![first_dot + n];
                assert_eq!(numbers(dots, "p "), (1..=9).map(dot).collect::<Vec<_>>(), "{ctx}: one point each");
                // The three objects name three materials when they are not
                // told apart by the MTL: a file has none, so its names differ.
                let used: Vec<&str> = secs.iter().map(|(_, s)| *s.iter().find(|l| l.starts_with("usemtl ")).unwrap()).collect();
                if self_contained {
                    assert!(used[0] != used[1] && used[1] != used[2] && used[0] != used[2], "{ctx}: {used:?}");
                } else {
                    assert_eq!(used[1..], ["usemtl default", "usemtl default"], "{ctx}: the MTL's own default");
                    let mtl = String::from_utf8(package.files.iter().find(|f| f.path == "model.mtl").unwrap().bytes.clone()).unwrap();
                    assert!(mtl.contains("newmtl default\n") && mtl.contains("newmtl wire_cube_mat\n"), "{mtl}");
                }
            }
        }

        /// A mesh of mixed primitives is several objects, so that none mixes
        /// faces with lines or points. Through the converter, `prepare` bakes
        /// every primitive into a mesh of its own, so each is an `o`; and the
        /// writer itself, given such a mesh as it stands, shares an `o`
        /// between a run of one topology and starts another at each change of
        /// it. The reader gives them back as the meshes they became.
        #[test]
        fn a_mesh_that_mixes_topologies_is_split_so_that_no_object_mixes_kinds() {
            let quad = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
            let prim = |topology: Topology, indices: &[u32]| Primitive { positions: quad.clone(), indices: indices.to_vec(), topology, ..Default::default() };
            let mut s = Scene::default();
            s.meshes.push(SceneMesh {
                name: Some("m".into()),
                primitives: vec![
                    prim(Topology::Triangles, &[0, 1, 2]),
                    prim(Topology::Triangles, &[0, 2, 3]),
                    prim(Topology::Lines, &[0, 1, 2, 3]),
                    prim(Topology::Triangles, &[1, 2, 3]),
                    prim(Topology::Points, &[0, 3]),
                ],
                ..Default::default()
            });
            s.add_root(Node { name: Some("m".into()), mesh: Some(0), ..Default::default() });
            let kinds = |text: &str| -> Vec<(String, usize, usize, usize)> {
                sections(text).iter().map(|(n, sec)| (n.clone(), count(sec, "f "), count(sec, "l "), count(sec, "p "))).collect()
            };
            let named = |rows: &[(&str, usize, usize, usize)]| -> Vec<(String, usize, usize, usize)> {
                rows.iter().map(|(n, f, l, p)| (n.to_string(), *f, *l, *p)).collect()
            };
            for self_contained in [true, false] {
                let ctx = format!("self-contained {self_contained}");
                // The writer, as given the mesh.
                let package = write_scene(&s, self_contained).expect("write");
                let text = String::from_utf8(package.files[0].bytes.clone()).unwrap();
                assert_eq!(kinds(&text), named(&[("m", 2, 0, 0), ("m_1", 0, 2, 0), ("m_2", 1, 0, 0), ("m_3", 0, 0, 2)]), "{ctx}");
                let back = read_scene(text.as_bytes(), &package.resolver()).expect("read");
                let order: Vec<Topology> = back.scene.meshes.iter().flat_map(|m| &m.primitives).map(|p| p.topology).collect();
                assert_eq!(order, [Topology::Triangles, Topology::Lines, Topology::Triangles, Topology::Points], "{ctx}");
                assert_eq!(back.scene.meshes.len(), 4, "{ctx}: one mesh per object");
                // The converter: one object for each primitive.
                let (text, package, _) = written(&s, self_contained);
                assert_eq!(
                    kinds(&text),
                    named(&[("m", 1, 0, 0), ("m_1", 1, 0, 0), ("m_2", 0, 2, 0), ("m_3", 1, 0, 0), ("m_4", 0, 0, 2)]),
                    "{ctx}: through the converter"
                );
                let back = read_scene(text.as_bytes(), &package.resolver()).expect("read");
                assert_eq!(back.scene.meshes.len(), 5, "{ctx}: through the converter, one mesh per object");
            }
        }

        /// `fixtures::wire()` through the writer and the reader, in one file
        /// and in a package: the same elements of each kind, the loop's and
        /// the grid's positions bit for bit and their indices as written, the
        /// same colours (within the sRGB round trip's last bit), and no line
        /// or point lost.
        #[test]
        fn wire_round_trips_through_obj() {
            let wire = mesh::fixtures::wire();
            for self_contained in [true, false] {
                let ctx = if self_contained { "one file" } else { "a package" };
                let (text, package, losses) = written(&wire, self_contained);
                assert!(!losses.iter().any(|l| matches!(l.kind, LossKind::Points | LossKind::Lines)), "{ctx}: {losses:?}");
                let back = read_scene(text.as_bytes(), &package.resolver()).expect("read").scene;
                for topology in [Topology::Triangles, Topology::Lines, Topology::Points] {
                    assert_eq!(elements(&back, topology), elements(&wire, topology), "{ctx}: {topology:?}");
                }
                assert_eq!(back.drawable_count(), 12 + 4 + 9, "{ctx}");
                let bits = |p: &Primitive| -> Vec<u32> { p.positions.iter().flatten().map(|v| v.to_bits()).collect() };
                for name in ["wire_loop", "wire_points"] {
                    let (want, got) = (prim_of(&wire, name), prim_of(&back, name));
                    assert_eq!((got.topology, &got.indices), (want.topology, &want.indices), "{ctx}: {name}");
                    assert_eq!(bits(got), bits(want), "{ctx}: {name}'s positions");
                    let (want, got) = (want.colors.as_ref().unwrap(), got.colors.as_ref().expect("colours"));
                    assert!(want.iter().zip(got).all(|(a, b)| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)), "{ctx}: {name}'s colours {got:?} vs {want:?}");
                }
            }
        }

        /// Every `v` line carries a colour or none does (the reader refuses a
        /// mix), so once the loop and the grid have colours the cube is
        /// written white, and the reader gives the cube white vertex colours.
        /// `prepare` is what comes back, so it gives the cube white as well.
        #[test]
        fn a_primitive_without_colours_beside_coloured_ones_is_prepared_white() {
            let wire = mesh::fixtures::wire();
            // The package: the cube keeps its material and has no colours of its own.
            let opts = mesh::WriteOptions { self_contained: false, ..Default::default() };
            let prepared = MeshFormat::Obj.prepare(&wire, opts).expect("prepare");
            let cube = prim_of(&prepared, "wire_cube");
            assert_eq!(cube.colors, Some(vec![[1.0; 4]; cube.positions.len()]));
            let (text, package, _) = written(&wire, false);
            let back = read_scene(text.as_bytes(), &package.resolver()).expect("read").scene;
            let got = prim_of(&back, "wire_cube").colors.clone().expect("the reader gives the cube colours");
            assert!(got.iter().all(|c| c.iter().all(|v| (v - 1.0).abs() < 1e-6)), "{got:?}");
        }

        /// A package names a material for every primitive (OBJ's `usemtl` is
        /// sticky), so the loop and the grid, which have none, get a white
        /// `default` one, and the reader gives it back as a material of its
        /// own. `prepare` is what comes back, so it holds that material too,
        /// named as the MTL names it. One file has no MTL, and names a material
        /// for each kind instead: see `a_file_with_no_mtl_names_a_material_for_each_kind`.
        #[test]
        fn a_primitive_without_a_material_is_prepared_with_the_default_one_a_package_reads_back() {
            let wire = mesh::fixtures::wire();
            let opts = mesh::WriteOptions { self_contained: false, ..Default::default() };
            let prepared = MeshFormat::Obj.prepare(&wire, opts).expect("prepare");
            let (text, package, _) = written(&wire, false);
            let back = read_scene(text.as_bytes(), &package.resolver()).expect("read").scene;
            assert_eq!(wire.materials.len(), 1, "the cube's material alone");
            assert_eq!(back.materials.len(), 2, "the cube's material and the default one");
            assert_eq!(prepared.materials.len(), back.materials.len(), "prepare holds the default material");
            let names = |s: &Scene| -> Vec<Option<String>> { s.materials.iter().map(|m| m.name.clone()).collect() };
            assert_eq!(names(&prepared), names(&back));
            for name in ["wire_cube", "wire_loop", "wire_points"] {
                let (want, got) = (prim_of(&prepared, name).material, prim_of(&back, name).material);
                assert_eq!(want, got, "{name}'s material");
                assert!(got.is_some(), "{name} names a material");
            }
            let default = &prepared.materials[prim_of(&prepared, "wire_loop").material.unwrap()];
            assert_eq!((default.base_color, default.roughness, default.metallic), ([1.0; 4], 1.0, 0.0));
        }

        /// A file with no MTL (one file, or a package of no materials) names no
        /// material but through `usemtl`, and with lines or points in it each
        /// kind names one of its own (`faces`, `lines`, `points`). The reader
        /// gives each name back as a white material, in the order the kinds are
        /// written, so `prepare` holds those materials and gives each primitive
        /// the one of its kind. A scene of triangles alone names none.
        #[test]
        fn a_file_with_no_mtl_names_a_material_for_each_kind() {
            let wire = mesh::fixtures::wire();
            let mut bare = wire.clone();
            bare.materials.clear();
            bare.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()).for_each(|p| p.material = None);
            let names = |s: &Scene| -> Vec<Option<String>> { s.materials.iter().map(|m| m.name.clone()).collect() };
            for (what, scene, self_contained) in [("one file", &wire, true), ("a package of no materials", &bare, false)] {
                let opts = mesh::WriteOptions { self_contained, ..Default::default() };
                let prepared = MeshFormat::Obj.prepare(scene, opts).expect("prepare");
                let (text, package, _) = written(scene, self_contained);
                let back = read_scene(text.as_bytes(), &package.resolver()).expect("read").scene;
                let kinds = ["faces", "lines", "points"].map(|n| Some(n.to_string()));
                assert_eq!(names(&back), kinds, "{what}: the reader makes a material of each name");
                assert_eq!(names(&prepared), names(&back), "{what}: prepare holds the same materials");
                for name in ["wire_cube", "wire_loop", "wire_points"] {
                    let (want, got) = (prim_of(&prepared, name).material, prim_of(&back, name).material);
                    assert_eq!(want, got, "{what}: {name}'s material");
                    assert!(got.is_some(), "{what}: {name} names a material");
                }
                for m in &prepared.materials {
                    assert_eq!((m.base_color, m.roughness, m.metallic), ([1.0; 4], 1.0, 0.0), "{what}: {m:?} is white");
                }
            }
            // Triangles alone name no material, and prepare adds none.
            let cube = mesh::fixtures::cube();
            let opts = mesh::WriteOptions { self_contained: true, ..Default::default() };
            let prepared = MeshFormat::Obj.prepare(&cube, opts).expect("prepare");
            assert!(prepared.materials.is_empty() && prepared.meshes[0].primitives[0].material.is_none());
            let (text, _, _) = written(&cube, true);
            assert!(!text.contains("usemtl"), "{text}");
        }

        /// A corner of an `l` or a `p` is spelt as a face's is, and its `vt`
        /// and `vn` are indices into their own lists: read for what they are
        /// and kept beside the position, never taken for a `v`. (The writer
        /// says `v/vt` for a line and `v` for a point; other exporters spell
        /// more.) The indices differ on purpose: `1//2` has the first
        /// position and the second normal, and `3/2` the third position and
        /// the second coordinate.
        #[test]
        fn a_line_or_point_reads_its_vt_and_vn_apart_from_its_v() {
            let text = "v 0 0 0\nv 1 0 0\nv 5 5 5\nvn 0 0 1\nvn 0 1 0\nvt 0.25 0.25\nvt 0.75 0.5\no ln\nl 1//2 2//1\no pt\np 3/2\n";
            let r = read_ok(text);
            assert!(r.warnings.is_empty(), "{:?}", r.warnings);
            let line = prim_of(&r.scene, "ln");
            assert_eq!((line.topology, &line.indices), (Topology::Lines, &vec![0, 1]));
            assert_eq!(line.positions, [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], "the v of each corner");
            assert_eq!(line.normals, Some(vec![[0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]), "the vn of each corner");
            assert!(line.uvs.is_empty());
            let point = prim_of(&r.scene, "pt");
            assert_eq!((point.topology, &point.indices), (Topology::Points, &vec![0]));
            assert_eq!(point.positions, [[5.0, 5.0, 5.0]], "the v of the corner");
            assert_eq!(point.uvs, vec![vec![[0.75, 0.5]]], "the vt of the corner");
            assert_eq!(point.normals, None);
        }

        /// The flat read (`obj::read`) is of the triangles alone, and its one
        /// base colour is the first vertex colour of those triangles: a loop
        /// of red vertices that comes first in the file is not the mesh's
        /// colour.
        #[test]
        fn the_flat_read_takes_its_colour_from_the_triangles() {
            let text = "o loop\nv 0 0 0 1 0 0\nv 1 0 0 1 0 0\nl 1 2\no tri\nv 0 0 1 0 0 1\nv 1 0 1 0 0 1\nv 0 1 1 0 0 1\nf 3 4 5\n";
            let m = read(text.as_bytes()).expect("flat read");
            assert_eq!(m.indices.len(), 3, "the triangle alone");
            assert_eq!(m.base_color, Some([0.0, 0.0, 1.0, 1.0]), "the triangle's blue, not the loop's red");
            // And the first triangle's colour when the loop comes second.
            let text = "o tri\nv 0 0 1 0 0 1\nv 1 0 1 0 0 1\nv 0 1 1 0 0 1\nf 1 2 3\no loop\nv 0 0 0 1 0 0\nv 1 0 0 1 0 0\nl 4 5\n";
            assert_eq!(read(text.as_bytes()).expect("flat read").base_color, Some([0.0, 0.0, 1.0, 1.0]));
        }

        /// A scene of points alone, and one of lines alone, writes as an OBJ
        /// (one file and a package) and reads back as itself. A gate in the
        /// converter's `write_scene` that counts triangles refuses them as
        /// `Empty`.
        #[test]
        fn a_scene_of_points_or_lines_alone_writes_and_reads_back() {
            let wire = mesh::fixtures::wire();
            for (name, topology) in [("wire_points", Topology::Points), ("wire_loop", Topology::Lines)] {
                let alone = Scene {
                    meshes: vec![wire.meshes.iter().find(|m| m.name.as_deref() == Some(name)).unwrap().clone()],
                    nodes: vec![Node { name: Some(name.into()), mesh: Some(0), ..Default::default() }],
                    roots: vec![0],
                    ..Default::default()
                };
                assert_eq!(alone.triangle_count(), 0, "{name}");
                for self_contained in [true, false] {
                    let ctx = format!("{name}, {}", if self_contained { "one file" } else { "a package" });
                    let (text, package, _) = written(&alone, self_contained);
                    let back = mesh::read_scene(text.as_bytes(), MeshFormat::Obj, &package.resolver());
                    assert!(back.is_ok(), "{ctx}: {:?}", back.as_ref().err());
                    let (want, got) = (&alone.meshes[0].primitives[0], &back.unwrap().scene.meshes[0].primitives[0]);
                    assert_eq!((got.topology, &got.indices, &got.positions), (topology, &want.indices, &want.positions), "{ctx}");
                }
            }
        }

        /// What an `l` or a `p` statement cannot say. `l` takes `v/vt` and
        /// no normal, `p` takes neither (the OBJ spec's grammar), so a line
        /// keeps its texture coordinates and loses its normals, and a point
        /// loses both. `prepare` leaves them out, so it is the scene the
        /// writer encodes and the reader gives back, and the write names each
        /// loss, with the lines and points counted by primitive.
        #[test]
        fn what_an_l_or_p_statement_cannot_say_is_dropped_and_named() {
            let along_z = |n: usize| Some(vec![[0.0, 0.0, 1.0]; n]);
            let uv = |n: usize| vec![(0..n).map(|i| [i as f32 * 0.25, 0.5]).collect::<Vec<[f32; 2]>>()];
            let pos = |n: usize| (0..n).map(|i| [i as f32, 0.0, 0.0]).collect::<Vec<[f32; 3]>>();
            let ring = Primitive { positions: pos(3), normals: along_z(3), uvs: uv(3), indices: vec![0, 1, 1, 2], topology: Topology::Lines, ..Default::default() };
            let dots = Primitive { positions: pos(2), normals: along_z(2), uvs: uv(2), indices: vec![0, 1], topology: Topology::Points, ..Default::default() };
            let mut s = Scene::default();
            for (name, p) in [("ring", ring), ("dots", dots)] {
                s.meshes.push(SceneMesh { name: Some(name.into()), primitives: vec![p], ..Default::default() });
                let mesh = s.meshes.len() - 1;
                s.add_root(Node { name: Some(name.into()), mesh: Some(mesh), ..Default::default() });
            }
            for self_contained in [true, false] {
                let ctx = if self_contained { "one file" } else { "a package" };
                let opts = mesh::WriteOptions { self_contained, ..Default::default() };
                let (text, package, losses) = written(&s, self_contained);
                assert_eq!(count(&text.lines().collect::<Vec<_>>(), "vn "), 0, "{ctx}: no statement can name a normal");
                assert_eq!(count(&text.lines().collect::<Vec<_>>(), "vt "), 3, "{ctx}: the line's coordinates, not the points'");
                let secs = sections(&text);
                let l: Vec<&str> = secs[0].1.iter().copied().filter(|l| l.starts_with("l ")).collect();
                let p: Vec<&str> = secs[1].1.iter().copied().filter(|l| l.starts_with("p ")).collect();
                assert_eq!((l, p), (vec!["l 1/1 2/2", "l 2/2 3/3"], vec!["p 4", "p 5"]), "{ctx}");
                let named = |kind: LossKind| -> Vec<&str> { losses.iter().filter(|l| l.kind == kind).map(|l| l.what.as_str()).collect() };
                assert_eq!(named(LossKind::Normals), ["normals of 2 line or point primitive(s)"], "{ctx}");
                assert_eq!(named(LossKind::Uvs), ["texture coordinates of 1 point primitive(s)"], "{ctx}");
                // The prepared scene is what comes back.
                let prepared = MeshFormat::Obj.prepare(&s, opts).expect("prepare");
                let back = read_scene(text.as_bytes(), &package.resolver()).expect("read").scene;
                for name in ["ring", "dots"] {
                    let (want, got) = (prim_of(&prepared, name), prim_of(&back, name));
                    assert_eq!((&got.normals, &got.uvs), (&want.normals, &want.uvs), "{ctx}: {name}");
                    assert_eq!(got.normals, None, "{ctx}: {name} has no normals");
                }
                assert_eq!(prim_of(&back, "ring").uvs, uv(3), "{ctx}: the line keeps its coordinates");
                assert!(prim_of(&back, "dots").uvs.is_empty(), "{ctx}: the points have none");
            }
        }

        /// A triangle and a row of three points, each with texture
        /// coordinates, drawn with one material that holds all three maps an
        /// MTL can (base colour, emissive, normal), beside their factors.
        /// `with_triangle` false leaves the points as the material's only
        /// users.
        fn paint_scene(with_triangle: bool) -> Scene {
            let mut s = Scene::default();
            s.images.push(Image { name: Some("quads".into()), format: ImageFormat::Png, data: mesh::fixtures::quadrant_png() });
            s.textures.push(Texture { name: None, image: 0, sampler: Default::default() });
            let map = || Some(TextureRef { texture: 0, uv_set: 0, transform: None });
            s.materials.push(Material {
                name: Some("paint".into()),
                base_color: [0.5, 0.25, 1.0, 1.0],
                base_color_texture: map(),
                emissive: [0.25, 0.0, 0.0],
                emissive_texture: map(),
                normal_texture: map(),
                normal_scale: 2.0,
                metallic: 0.0,
                roughness: 0.5,
                ..Default::default()
            });
            let uvs = vec![vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]];
            let corners = |z: f32| vec![[0.0, 0.0, z], [1.0, 0.0, z], [0.0, 1.0, z]];
            let tri = Primitive { positions: corners(0.0), uvs: uvs.clone(), indices: vec![0, 1, 2], material: Some(0), ..Default::default() };
            let dots = Primitive { positions: corners(1.0), uvs, indices: vec![0, 1, 2], topology: Topology::Points, material: Some(0), ..Default::default() };
            for (name, p) in [("tri", tri), ("dots", dots)].into_iter().filter(|(n, _)| with_triangle || *n == "dots") {
                s.meshes.push(SceneMesh { name: Some(name.into()), primitives: vec![p], ..Default::default() });
                let mesh = s.meshes.len() - 1;
                s.add_root(Node { name: Some(name.into()), mesh: Some(mesh), ..Default::default() });
            }
            s
        }

        /// OBJ's `p` names a vertex and nothing else, so a point has no
        /// texture coordinate to sample a map with. A map left on the
        /// material it is drawn with is smeared across each point sprite by
        /// three.js's OBJLoader (PointsMaterial takes the material's map),
        /// and read back through glTF it is a map on a primitive with no
        /// TEXCOORD, which the Khronos validator refuses
        /// (MESH_PRIMITIVE_TOO_FEW_TEXCOORDS). So a package draws points with
        /// an untextured copy of their material: named as the writer names
        /// a second `paint`, factors as they were, no map. The triangles keep
        /// the original. `prepare` is what the reader gives back, and the
        /// write names both what went: the coordinates and the bindings.
        #[test]
        fn points_are_drawn_with_an_untextured_copy_of_a_textured_material() {
            let opts = mesh::WriteOptions { self_contained: false, ..Default::default() };
            let s = paint_scene(true);
            let prepared = MeshFormat::Obj.prepare(&s, opts).expect("prepare");
            let (tri, dots) = (prim_of(&prepared, "tri"), prim_of(&prepared, "dots"));
            assert_eq!(prepared.materials.len(), 2, "the original, and an untextured copy for the points");
            assert_eq!((tri.material, dots.material), (Some(0), Some(1)), "the points use the copy");
            let (paint, copy) = (&prepared.materials[0], &prepared.materials[1]);
            assert_eq!(copy.name.as_deref(), Some("paint_1"), "named as the writer names a second `paint`");
            assert!(copy.texture_refs().next().is_none(), "no map on the copy: {copy:?}");
            assert_eq!(
                (copy.base_color, copy.emissive, copy.roughness, copy.metallic),
                (paint.base_color, paint.emissive, paint.roughness, paint.metallic),
                "the factors stay"
            );
            assert_eq!(copy.normal_scale, 1.0, "no normal map for the scale to scale");
            assert_eq!(paint.texture_refs().count(), 3, "the triangles keep theirs");
            assert!(dots.uvs.is_empty(), "and the points no coordinates");

            // What is written: the points' object names the copy, whose MTL
            // block has no map, and the original's block still has its three.
            let (text, package, losses) = written(&s, false);
            let mtl = String::from_utf8(package.files.iter().find(|f| f.path == "model.mtl").expect("an MTL").bytes.clone()).expect("utf8");
            let (original, plain) = (mtl_lines(&mtl, "paint"), mtl_lines(&mtl, "paint_1"));
            for key in ["map_Kd ", "map_Ke ", "norm "] {
                assert!(original.iter().any(|l| l.starts_with(key)), "paint keeps {key}: {original:?}");
                assert!(!plain.iter().any(|l| l.starts_with(key)), "paint_1 has no {key}: {plain:?}");
            }
            let secs = sections(&text);
            let uses = |object: &str| secs.iter().find(|(n, _)| n == object).map(|(_, body)| body.iter().copied().filter(|l| l.starts_with("usemtl")).collect::<Vec<_>>());
            assert_eq!((uses("tri"), uses("dots")), (Some(vec!["usemtl paint"]), Some(vec!["usemtl paint_1"])));

            // What the reader gives back is what `prepare` holds.
            let back = read_scene(text.as_bytes(), &package.resolver()).expect("read").scene;
            let (btri, bdots) = (prim_of(&back, "tri"), prim_of(&back, "dots"));
            let material = |p: &Primitive| &back.materials[p.material.expect("a material")];
            assert_eq!(back.materials.len(), prepared.materials.len());
            assert!(bdots.uvs.is_empty(), "the points read back with no coordinates");
            let (want, got) = (copy, material(bdots));
            assert_eq!(got.name, want.name);
            assert!(got.texture_refs().next().is_none(), "no map on the points' material: {got:?}");
            assert!(got.base_color.iter().zip(want.base_color).all(|(a, b)| (a - b).abs() < 1e-6), "{:?} vs {:?}", got.base_color, want.base_color);
            assert!(got.emissive.iter().zip(want.emissive).all(|(a, b)| (a - b).abs() < 1e-6), "{:?} vs {:?}", got.emissive, want.emissive);
            assert_eq!((got.roughness, got.metallic, got.normal_scale), (want.roughness, want.metallic, want.normal_scale));
            let kept = material(btri);
            assert!(kept.base_color_texture.is_some() && kept.emissive_texture.is_some() && kept.normal_texture.is_some(), "the triangles' material keeps its maps: {kept:?}");

            // Both are named, once, by primitive.
            let named = |kind: LossKind| -> Vec<&str> { losses.iter().filter(|l| l.kind == kind).map(|l| l.what.as_str()).collect() };
            assert_eq!(named(LossKind::Uvs), ["texture coordinates of 1 point primitive(s)"]);
            assert_eq!(named(LossKind::Textures), ["texture bindings of 1 point primitive(s)"]);

            // A material with no map costs the points nothing: no copy, and
            // nothing named.
            let mut bare = s.clone();
            bare.materials[0] = Material { name: Some("paint".into()), ..Default::default() };
            bare.textures.clear();
            bare.images.clear();
            let prepared = MeshFormat::Obj.prepare(&bare, opts).expect("prepare");
            assert_eq!(prepared.materials.len(), 1, "no copy of an untextured material");
            let (_, _, losses) = written(&bare, false);
            assert!(!losses.iter().any(|l| l.kind == LossKind::Textures), "{losses:?}");
        }

        /// The points are the material's only users: it is stripped where it
        /// stands. A textured original left unused would be written (with its
        /// image, as dead weight in the package) and not read back, and the
        /// reader's materials would be fewer than `prepare`'s.
        #[test]
        fn a_textured_material_only_points_use_is_stripped_in_place() {
            let opts = mesh::WriteOptions { self_contained: false, ..Default::default() };
            let s = paint_scene(false);
            let prepared = MeshFormat::Obj.prepare(&s, opts).expect("prepare");
            assert_eq!(prepared.materials.len(), 1, "no copy: nothing else uses the original");
            let paint = &prepared.materials[0];
            assert_eq!(paint.name.as_deref(), Some("paint"));
            assert!(paint.texture_refs().next().is_none(), "{paint:?}");
            assert_eq!((paint.base_color, paint.emissive, paint.normal_scale), ([0.5, 0.25, 1.0, 1.0], [0.25, 0.0, 0.0], 1.0));
            assert!(prepared.textures.is_empty() && prepared.images.is_empty(), "the map's texture and image go with it");

            let (text, package, losses) = written(&s, false);
            let paths: Vec<&str> = package.files.iter().map(|f| f.path.as_str()).collect();
            assert!(!paths.iter().any(|p| p.starts_with("textures/")), "no image shipped for a map nothing uses: {paths:?}");
            let mtl = String::from_utf8(package.files.iter().find(|f| f.path == "model.mtl").expect("an MTL").bytes.clone()).expect("utf8");
            assert!(!mtl.lines().any(|l| l.starts_with("map_") || l.starts_with("norm ")), "{mtl}");
            let back = read_scene(text.as_bytes(), &package.resolver()).expect("read").scene;
            assert_eq!(back.materials.len(), prepared.materials.len());
            let got = &back.materials[0];
            assert_eq!(got.name, paint.name);
            assert!(got.texture_refs().next().is_none() && prim_of(&back, "dots").uvs.is_empty());
            let named = |kind: LossKind| -> Vec<&str> { losses.iter().filter(|l| l.kind == kind).map(|l| l.what.as_str()).collect() };
            assert_eq!(named(LossKind::Textures), ["texture bindings of 1 point primitive(s)"]);
            assert_eq!(named(LossKind::Uvs), ["texture coordinates of 1 point primitive(s)"]);
        }

        /// A material whose maps sample the second uv set keeps them in a
        /// package: `degrade` moves the sampled set to the first, the one OBJ
        /// holds, so a points primitive drawn with that material is untextured
        /// all the same, and the write names it (`losses` decides it as
        /// `prepare` does, not by the set the map names before the move).
        /// `prepare` is still what the reader gives back, with the triangles
        /// that share the material (a copy for the points) or without them
        /// (the material stripped where it stands).
        #[test]
        fn a_map_on_a_later_uv_set_is_left_off_the_points_and_named() {
            let opts = mesh::WriteOptions { self_contained: false, ..Default::default() };
            for with_triangle in [true, false] {
                let ctx = if with_triangle { "shared with a triangle" } else { "points alone" };
                let mut s = paint_scene(with_triangle);
                for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
                    let second = p.uvs[0].iter().map(|[u, v]| [*v, *u]).collect();
                    p.uvs.push(second);
                }
                for (_, r) in s.materials[0].texture_refs_mut() {
                    r.uv_set = 1;
                }
                let prepared = MeshFormat::Obj.prepare(&s, opts).expect("prepare");
                let at = |scene: &Scene, node: &str| scene.materials[prim_of(scene, node).material.expect("a material")].texture_refs().count();
                assert_eq!(at(&prepared, "dots"), 0, "{ctx}: prepare leaves no map on the points' material");
                if with_triangle {
                    assert_eq!(at(&prepared, "tri"), 3, "{ctx}: the triangles keep theirs, moved to the first set");
                }

                let (text, package, losses) = written(&s, false);
                let named = |kind: LossKind| -> Vec<&str> { losses.iter().filter(|l| l.kind == kind).map(|l| l.what.as_str()).collect() };
                assert_eq!(named(LossKind::Textures), ["texture bindings of 1 point primitive(s)"], "{ctx}: {losses:?}");

                let back = read_scene(text.as_bytes(), &package.resolver()).expect("read").scene;
                assert_eq!(back.materials.len(), prepared.materials.len(), "{ctx}");
                assert_eq!(at(&back, "dots"), at(&prepared, "dots"), "{ctx}: the reader gives back what `prepare` holds");
                if with_triangle {
                    assert_eq!(at(&back, "tri"), at(&prepared, "tri"), "{ctx}");
                }
            }

            // A triangle without that second set keeps the material in scene
            // order (`UvSetPlan`), so `degrade` drops the maps with the set
            // they name, and the points have none left to lose: `prepare`
            // untextures nothing, and the write names no bindings of theirs.
            let mut lacking = paint_scene(true);
            let first = lacking.meshes[1].primitives[0].uvs[0].clone();
            lacking.meshes[1].primitives[0].uvs.push(first);
            for (_, r) in lacking.materials[0].texture_refs_mut() {
                r.uv_set = 1;
            }
            let prepared = MeshFormat::Obj.prepare(&lacking, opts).expect("prepare");
            let material = |node: &str| &prepared.materials[prim_of(&prepared, node).material.expect("a material")];
            assert_eq!(material("dots").texture_refs().count(), 0, "degrade dropped the maps");
            let (_, _, losses) = written(&lacking, false);
            assert!(!losses.iter().any(|l| l.what.starts_with("texture bindings of")), "{losses:?}");
        }

        // ── hostile input ───────────────────────────────────────────────────

        /// Point and line indices are charged to the same index budget as
        /// triangles (12M), before the elements are built: a face's three
        /// indices to the triangle, a segment's two and a point's one, all
        /// from one count, and the statement that crosses the ceiling is
        /// refused at the token that crosses it, naming its line. One reading
        /// holds it all: a face of 3,000,002 corners (3 × 3,000,000 = 9M
        /// indices), a polyline of 1,500,001 vertices (3M indices) that meets
        /// the ceiling to the index, and one point past it. The refusal names
        /// the point's line (6), so the polyline on line 5 was let through at
        /// the ceiling (a `>=` would have refused it there), the face's 9M
        /// were counted when the polyline's came (a budget of lines alone
        /// would let the point through), and the one over is refused.
        ///
        /// One reading of 4.5M tokens in place of two of 6M. The success
        /// reading the old test made on its own is the prefix of this one, up
        /// to line 5; no other test pins the ceiling met to the index for
        /// faces, so it is kept here and not dropped.
        #[test]
        fn lines_and_points_are_charged_to_the_index_budget() {
            let ceiling = mesh::scene::MAX_SCENE_INDICES;
            assert_eq!(ceiling, 12_000_000, "the counts below are written for this ceiling");
            let started = std::time::Instant::now();
            let face = (0..3_000_002).map(|i| (i % 3 + 1).to_string()).collect::<Vec<_>>().join(" ");
            let text = format!("v 0 0 0\nv 1 0 0\nv 0 1 0\nf {face}\nl{}\np 1\n", " 1".repeat(1_500_001));
            match refused(&text) {
                MeshError::Unsupported(d) => {
                    assert!(d.contains("obj line 6") && d.contains("the points pass 12000000 indices"), "{d}");
                }
                e => panic!("{e:?}"),
            }
            eprintln!("the index budget's one reading of about 4.5M tokens: {:?}", started.elapsed());
        }

        /// The overview's hostile-input bar on the new statements: a lean
        /// OBJ (a closed loop of coloured vertices, a textured line with
        /// `v/vt` pairs, and points by absolute and by relative index) read
        /// whole, then swept ([`layer_sweep`]) with every bit of its `l` and
        /// `p` statements among the single flips. Every reading is `Ok` or
        /// `Err`, never a panic.
        #[test]
        fn obj_points_and_lines_survive_truncation_and_bit_flips() {
            let name = "a lean OBJ of a loop, a textured line and points";
            let text = "\
o loop
v 0 0 0 1 0 0
v 1 0 0 0 1 0
v 1 1 0 0 0 1
v 0 1 0 1 1 0
l 1 2 3 4 1
o textured
vt 0 0
vt 1 0
vt 0 1
l 1/1 2/2 3/3
o dots
v 0 0 1 0.5 0.5 0.5
v 0 1 1 0.5 0.5 0.5
p 5 6 -1
";
            let file = text.as_bytes();
            let whole = read_ok(text);
            assert_eq!((elements(&whole.scene, Topology::Lines), elements(&whole.scene, Topology::Points)), (6, 3), "{name} itself carries both");
            layer_sweep(name, file, mesh::fixtures::sweeps::span(name, file, "l 1 2", "p 5"), |bytes| {
                let _ = mesh::read_scene(bytes, MeshFormat::Obj, &NoSidecars);
            });
        }
    }
}
