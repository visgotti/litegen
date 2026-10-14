//! 3MF — the 3D Manufacturing Format (Core 1.4, Materials & Properties 1.2) —
//! read and write.
//!
//! # What 3MF is, and what that decides
//!
//! 3MF is what slicers read: PrusaSlicer, Bambu Studio, Cura and the printers
//! behind them. It is an OPC package (a ZIP of XML parts) whose model part
//! describes solid objects on a build platform, with optional colour: flat
//! base materials, per-vertex colour groups and 2D textures. It has no normals,
//! no skeleton, no animation and no surface model beyond a display colour. A
//! conversion into it is therefore the posed model, as a viewer shows it with
//! no clip playing — which [`prepare`] computes, and the writer encodes exactly.
//!
//! # Coordinates and units
//!
//! 3MF is Z-up: "the z-axis increasing to the top of the output field" (Core
//! §3.1). glTF is Y-up. The conversion is a +90° turn about X, `(x, y, z) →
//! (x, −z, y)`, and it lives in the one build item's transform rather than in
//! the vertices, so every vertex number is written as the scene holds it and
//! the reader undoes exactly that turn at the root. Readers that ignore the
//! convention are told nothing false: three.js's `ThreeMFLoader` returns the
//! Z-up model and documents `rotation.set(-PI / 2, 0, 0)` as the caller's job.
//!
//! The file says `unit="meter"` and the numbers are the scene's own metres.
//! That is the faithful choice: the scene is in metres (glTF's unit), so the
//! one statement that is true about these numbers is that they are metres, and
//! writing them unscaled keeps them bit-exact. The alternative — multiplying
//! by 1000 into the default millimetres — changes every coordinate's rounding
//! and buys nothing, because every slicer that reads 3MF honours the unit:
//! PrusaSlicer 2.9.4 (`get_unit_factor`: "meter" → 1000), Bambu Studio
//! (`bbs_get_unit_factor`, the same table) and Cura (`_getScaleFromUnit`,
//! `"meter": 1000`) all scale to millimetres on import, so a 1.8 m figure
//! arrives 1800 mm tall — its true size — and is scaled down there by a user
//! who wants a figurine. three.js reads the unit and does not scale at all,
//! which is also why metres render at the same size as the source glTF.
//!
//! # Objects, instancing, mirrors
//!
//! Each scene mesh becomes one `<object type="model">` whose `<mesh>` holds all
//! of its primitives: 3MF gives every TRIANGLE its own property group, so
//! primitives that differ only in material still share one vertex list, which
//! is deduplicated (3MF's manifold rules count edges by vertex index). Every
//! node that draws a mesh becomes a placement of that object: one build item
//! when there is one placement, otherwise one assembly object whose
//! `<component>`s place the parts. Components rather than one item per part,
//! because consumers "MAY rearrange the items ... to better pack the build
//! volume" but "MUST NOT transform [components] relative to each other" (Core
//! §3.4.3.1, §4.2) — a model's parts must stay where they are. A mesh drawn by
//! two nodes is one object placed twice, so instancing survives.
//!
//! Mirrored placements are written as they are. The spec allows them and puts
//! the work on the consumer — "When applying a transformation with a negative
//! determinant, the resultant shape MUST NOT change the sign of its volume"
//! (Core §3.3) — and three.js flips the front face for a negative determinant
//! (`WebGLRenderer`: `frontFaceCW = matrixWorld.determinant() < 0`). A placement
//! with SHEAR is baked into its own object instead: three.js applies item and
//! component transforms with `Object3D.applyMatrix4`, which decomposes them
//! into position, rotation and scale and silently drops the shear.
//!
//! Every object is `type="model"`, even an open surface the spec's manifold
//! rules do not hold for: PrusaSlicer 2.9.4 loads `model` objects only
//! (`VALID_OBJECT_TYPES = { "model" }`) and Bambu Studio `model` and `other`, so
//! the honest-looking `surface` would make both drop the part. Slicers repair
//! open meshes; they do not resurrect dropped ones.
//!
//! # Colour: one property group per triangle
//!
//! A triangle names ONE property group (Core §4.1.4.1), so a texture, vertex
//! colours and a flat colour cannot be combined on it; a textured material's
//! tint and a textured primitive's vertex colours are dropped and reported
//! ([`losses`]). Per primitive, in that order: a base-colour texture becomes a
//! `texture2d` + `texture2dgroup` (coordinates V-flipped: 3MF's origin is the
//! lower-left of the image); else vertex colours become a `colorgroup` (the
//! material's base colour multiplied in, alpha dropped — colour groups are
//! "assumed to be fully opaque"); else the material becomes a `<base>` in
//! `basematerials`. Colours are 8-bit sRGB (`#RRGGBBAA`), encoded from the
//! scene's linear values. Texture coordinates exist only inside a texture
//! group, so an untextured primitive's UVs cannot be written at all.
//!
//! `requiredextensions="m"` is written whenever the materials namespace is: the
//! spec reserves requiring an extension for a model that "would lose key
//! meaning without the extension data" (Core §3.4), and a converted model's
//! colours and texture are what makes it look like its source. A consumer
//! without the extension then refuses the file instead of printing a silently
//! colourless part; the slicers and viewers checked (PrusaSlicer 2.9.4, Bambu
//! Studio, Cura, three.js) do not enforce the attribute at all.
//!
//! # Reading
//!
//! Namespaces are resolved by URI, never by prefix (a file that binds the
//! materials extension to `mat:` reads the same as one using `m:`). The model
//! part is streamed, not built as a DOM: a million `<vertex>` elements would
//! cost hundreds of megabytes as one. Read: units (micron to foot) into
//! metres, components to any depth with cycles refused, build items and their
//! transforms, the production extension's `p:path` references to other model
//! parts, base materials, colour groups, textures, and per-triangle property
//! indices with the object's `pid`/`pindex` as defaults. A triangle whose
//! corners carry different colours or coordinates is split so each scene vertex
//! has one. Composite materials are read as their spec-defined display mix and
//! multi-property stacks as one layer, each with a warning; support objects,
//! display properties, slice stacks and beam lattices are skipped with one.
//! Slicer project data (`Metadata/*.config`, vendor attributes) is ignored.
//!
//! @see <https://github.com/3MFConsortium/spec_core/blob/master/3MF%20Core%20Specification.md>
//! @see <https://github.com/3MFConsortium/spec_materials/blob/master/3MF%20Materials%20Extension.md>

use std::collections::HashMap;
use std::fmt::Write as _;

use super::ir::Mesh;
use super::math::{self, linear_to_srgb, srgb_to_linear, Mat4};
use super::package::{self, NoSidecars, Package, Sidecars};
use super::pose;
use super::scene::{
    Filter, Image, ImageFormat, Loss, LossKind, Material, Node, Primitive, Sampler, Scene, SceneMesh, Texture,
    TextureRef, Transform, Wrap, MAX_NODES, MAX_SCENE_INDICES, MAX_SCENE_VERTICES,
};
use super::xml::{self, Element, XmlEvent};
use super::zip::{self, Archive, Store};
use super::{MeshError, MeshFormat, SceneRead};

// ─── package constants ──────────────────────────────────────────────────────

const NS_CORE: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
const NS_MATERIAL: &str = "http://schemas.microsoft.com/3dmanufacturing/material/2015/02";
const NS_PRODUCTION: &str = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";
const NS_TRIANGLE_SETS: &str = "http://schemas.microsoft.com/3dmanufacturing/trianglesets/2021/07";
const NS_SLICE: &str = "http://schemas.microsoft.com/3dmanufacturing/slice/2015/07";
const NS_BEAM_LATTICE: &str = "http://schemas.microsoft.com/3dmanufacturing/beamlattice/2017/02";
const NS_RELATIONSHIPS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const NS_CONTENT_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";

/// The package relationship that names the root model part (Core App. C.2).
const REL_START_PART: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
/// The model part's relationship to each texture it uses (Materials App. E.2).
/// three.js resolves textures only through it.
const REL_TEXTURE: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dtexture";

const CT_RELATIONSHIPS: &str = "application/vnd.openxmlformats-package.relationships+xml";
const CT_MODEL: &str = "application/vnd.ms-package.3dmanufacturing-3dmodel+xml";

const CONTENT_TYPES_PART: &str = "[Content_Types].xml";
const PACKAGE_RELS_PART: &str = "_rels/.rels";
/// three.js finds the root model by `^3D/[^/]*\.model$` and textures under
/// `3D/Texture(s)/`, whatever the relationships say, so the conventional names
/// are the ones that load everywhere.
const MODEL_PART: &str = "3D/3dmodel.model";
const MODEL_RELS_PART: &str = "3D/_rels/3dmodel.model.rels";
const TEXTURE_FOLDER: &str = "3D/Textures";
const PRIMARY_FILE: &str = "model.3mf";
const APPLICATION: &str = "litegen mesh converter";

/// glTF's +Y up to 3MF's +Z up: `(x, y, z) → (x, −z, y)`, a +90° turn about X.
/// Column-major, like every matrix in this module.
const Y_UP_TO_Z_UP: Mat4 = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, -1.0, 0.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];
/// Its inverse, applied at every root on read: `(x, y, z) → (x, z, −y)`.
const Z_UP_TO_Y_UP: Mat4 = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 0.0, -1.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

// ─── limits ─────────────────────────────────────────────────────────────────

/// Model parts one package may make us parse: the root plus the parts the
/// production extension's `p:path` names. Bambu Studio writes one per object,
/// so a full build plate has dozens; thousands is a scan, not a model.
const MAX_MODEL_PARTS: usize = 1024;
/// Resources (objects and property groups) one model part may define.
const MAX_RESOURCES: usize = MAX_NODES;
/// Entries across every property group in the package. Each one exists to be
/// named by a triangle corner, so more than the scene's index ceiling is
/// nothing a real file needs — and each costs an allocation before any
/// triangle has been read.
const MAX_PROPERTIES: usize = MAX_SCENE_INDICES;
/// How deeply components may nest. Real assemblies are a few levels deep
/// (PrusaSlicer refuses past 10); the bound keeps a chain of a million
/// single-component objects from becoming a million-deep walk.
const MAX_COMPONENT_DEPTH: usize = 64;
/// Image bytes one package may hand the scene, in total.
const MAX_IMAGE_BYTES: usize = super::MAX_INPUT_BYTES;
/// Distinct warnings kept; past this they are counted, not listed.
const MAX_WARNINGS: usize = 64;
/// Characters kept of a title or a name, which end up in other files' headers.
const MAX_TEXT: usize = 1024;

// ─── errors and small helpers ───────────────────────────────────────────────

fn malformed(detail: impl Into<String>) -> MeshError {
    MeshError::Malformed(detail.into())
}

fn unsupported(detail: impl Into<String>) -> MeshError {
    MeshError::Unsupported(detail.into())
}

fn not_3mf(detail: impl Into<String>) -> MeshError {
    MeshError::NotThisFormat { expected: MeshFormat::ThreeMf, detail: detail.into() }
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// A value from the file, quoted for an error message and bounded.
fn quoted(s: &str) -> String {
    format!("{:?}", truncate(s, 40))
}

/// Strip what XML 1.0 cannot carry, so a name reads back as it was written.
fn clean_text(s: &str) -> String {
    truncate(s, MAX_TEXT).chars().filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r')).collect()
}

/// Escape for an attribute value. Tabs and line breaks become character
/// references: written raw, every conforming parser normalises them to spaces.
fn escape_attr(s: &str) -> String {
    xml::escape(s).replace('\t', "&#9;").replace('\n', "&#10;").replace('\r', "&#13;")
}

/// Bits of a position for welding, with −0 folded into +0 (`-0.0 + 0.0` is
/// `+0.0`): they are the same point, and a weld that kept them apart would
/// leave an open edge.
fn weld_key(p: [f32; 3]) -> [u32; 3] {
    [(p[0] + 0.0).to_bits(), (p[1] + 0.0).to_bits(), (p[2] + 0.0).to_bits()]
}

fn flip_winding(indices: &mut [u32]) {
    for t in indices.chunks_exact_mut(3) {
        t.swap(1, 2);
    }
}

// ─── colour ─────────────────────────────────────────────────────────────────

/// One linear channel as 3MF's 8-bit sRGB.
fn srgb8(c: f32) -> u8 {
    (linear_to_srgb(c) * 255.0).round().clamp(0.0, 255.0) as u8
}

fn alpha8(a: f32) -> u8 {
    (a.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn decode_srgb8(v: u8) -> f32 {
    srgb_to_linear(v as f32 / 255.0)
}

/// A linear RGBA colour as the file stores it and reads it back.
fn quantized(c: [f32; 4]) -> [f32; 4] {
    [decode_srgb8(srgb8(c[0])), decode_srgb8(srgb8(c[1])), decode_srgb8(srgb8(c[2])), alpha8(c[3]) as f32 / 255.0]
}

/// `#RRGGBB` or `#RRGGBBAA` (Core §5.1.1); alpha defaults to opaque.
fn parse_color(s: &str) -> Option<[u8; 4]> {
    let h = s.trim().strip_prefix('#')?;
    if !(h.len() == 6 || h.len() == 8) || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?, if h.len() == 8 { byte(6)? } else { 255 }])
}

/// Always eight digits: assimp 6 reads a six-digit `displaycolor` as alpha 0.
fn hex_color(c: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3])
}

// ─── numbers ────────────────────────────────────────────────────────────────

/// The shortest decimal that reads back as exactly this value, in scientific
/// notation at the extremes where plain notation would run to dozens of zeros.
/// Rust's float formatting is locale-independent, as Core §2.3.2 requires.
fn num32(out: &mut String, v: f32) {
    let a = v.abs();
    let _ = if a != 0.0 && !(1e-5..1e9).contains(&a) { write!(out, "{v:e}") } else { write!(out, "{v}") };
}

fn num64(out: &mut String, v: f64) {
    let a = v.abs();
    let _ = if a != 0.0 && !(1e-5..1e15).contains(&a) { write!(out, "{v:e}") } else { write!(out, "{v}") };
}

fn parse_f32(s: &str) -> Option<f32> {
    s.trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

fn parse_f64(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// `ST_ResourceID` / `ST_ResourceIndex`: a non-negative integer below 2^31.
fn parse_index(s: &str) -> Option<u32> {
    s.trim().parse::<u32>().ok().filter(|v| *v < (1 << 31))
}

/// A 3MF matrix, `m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32`: row-major,
/// points as row vectors, translation in the last row (Core §3.3). That is the
/// transpose of a column-vector matrix, which makes its twelve numbers exactly
/// glTF's column-major elements 0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14.
const MATRIX_SLOTS: [usize; 12] = [0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14];

fn parse_matrix(s: &str) -> Option<Mat4> {
    let mut m = math::IDENTITY;
    let mut n = 0usize;
    for tok in s.split_ascii_whitespace() {
        let slot = *MATRIX_SLOTS.get(n)?;
        m[slot] = parse_f64(tok)?;
        n += 1;
    }
    (n == 12).then_some(m)
}

fn matrix_attr(m: &Mat4) -> String {
    let mut out = String::new();
    for (i, slot) in MATRIX_SLOTS.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        num64(&mut out, m[*slot]);
    }
    out
}

// ─── texture sampling ───────────────────────────────────────────────────────

/// `texture2d@filter`. three.js maps `auto` to linear with trilinear mipmaps,
/// `linear` and `nearest` to their filter without mipmaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TexFilter {
    Auto,
    Linear,
    Nearest,
}

impl TexFilter {
    fn as_str(self) -> &'static str {
        match self {
            TexFilter::Auto => "auto",
            TexFilter::Linear => "linear",
            TexFilter::Nearest => "nearest",
        }
    }
}

/// The 3MF filter closest to a scene sampler. Magnification decides first —
/// a NEAREST magnifier is a pixel-art texture, and blurring it is the visible
/// mistake — then whether minification uses mipmaps at all.
fn tex_filter(s: &Sampler) -> TexFilter {
    match (s.mag_filter, s.min_filter, s.mipmaps) {
        (Some(Filter::Nearest), _, _) => TexFilter::Nearest,
        (_, Some(_), false) => TexFilter::Linear,
        _ => TexFilter::Auto,
    }
}

/// The scene sampler a 3MF filter reads back as. `auto` is "unspecified", the
/// reader default, which is what three.js gives both.
fn filter_sampler(f: TexFilter, wrap_s: Wrap, wrap_t: Wrap) -> Sampler {
    let (mag, min, mipmaps) = match f {
        TexFilter::Auto => (None, None, true),
        TexFilter::Linear => (Some(Filter::Linear), Some(Filter::Linear), false),
        TexFilter::Nearest => (Some(Filter::Nearest), Some(Filter::Nearest), false),
    };
    Sampler { wrap_s, wrap_t, mag_filter: mag, min_filter: min, mipmaps, ..Default::default() }
}

/// Filters as a renderer applies them, defaults resolved: whether two samplers
/// look the same.
fn effective_filters(s: &Sampler) -> (Filter, Filter, bool, Filter) {
    let mag = s.mag_filter.unwrap_or(Filter::Linear);
    match s.min_filter {
        None => (mag, Filter::Linear, true, Filter::Linear),
        Some(min) => (mag, min, s.mipmaps, if s.mipmaps { s.mipmap_filter } else { Filter::Linear }),
    }
}

fn representable_sampler(s: &Sampler) -> Sampler {
    filter_sampler(tex_filter(s), s.wrap_s, s.wrap_t)
}

fn tile_style(w: Wrap) -> &'static str {
    match w {
        Wrap::Repeat => "wrap",
        Wrap::MirroredRepeat => "mirror",
        Wrap::ClampToEdge => "clamp",
    }
}

// ─── the property rule ──────────────────────────────────────────────────────

/// How one primitive's appearance is carried, given that a triangle names one
/// property group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    /// Its material's base-colour texture, through a texture2dgroup.
    Texture(usize),
    /// Its vertex colours (times its material's base colour), through a
    /// colorgroup.
    Colors,
    /// Its material's base colour, through basematerials.
    Base(usize),
    /// No material and no colours.
    Plain,
}

/// Texture first (it carries the most), then vertex colours, then the flat
/// colour. A texture needs UV set 0 — the only set 3MF holds — and a PNG or
/// JPEG, the only images it allows (Materials §6).
fn look(scene: &Scene, p: &Primitive) -> Look {
    let material = p.material.and_then(|m| scene.materials.get(m));
    if let Some(r) = material.and_then(|m| m.base_color_texture.as_ref()) {
        let image = scene.textures.get(r.texture).and_then(|t| scene.images.get(t.image));
        if r.uv_set == 0 && !p.uvs.is_empty() && image.is_some_and(|im| im.format.is_universal()) {
            return Look::Texture(r.texture);
        }
    }
    if p.colors.is_some() {
        return Look::Colors;
    }
    match p.material {
        Some(m) if m < scene.materials.len() => Look::Base(m),
        _ => Look::Plain,
    }
}

// ─── prepare: the lossiness specification ───────────────────────────────────

/// The scene reduced to exactly what [`write_scene`] encodes — and therefore
/// what [`read_scene`] gives back, up to node structure and the text rounding
/// of texture coordinates.
///
/// - animation dropped; skinned meshes posed into world space at new roots
///   (glTF's rule: a skinned mesh ignores its node's transform), morph targets
///   applied at each node's weights;
/// - placements whose world transform has no TRS form (shear) baked into their
///   own geometry, for the reason in the module docs;
/// - per primitive, the property rule ([`look`]): textured materials become a
///   white material with the texture (its `KHR_texture_transform` baked into
///   the coordinates), vertex colours absorb the base colour and lose alpha
///   (their material becomes a matte white, as a colour group reads back),
///   flat materials keep their base colour; every colour quantised to the 8-bit
///   sRGB the file holds; normals, tangents, extra UV sets and the UVs of
///   untextured primitives dropped; samplers reduced to 3MF's three filters;
/// - triangles with two corners at one point dropped (3MF requires distinct
///   vertex indices, and the writer welds by position);
/// - meshes, materials, textures and images nothing uses any more pruned, and
///   images named after the part each is written to.
///
/// Idempotent: preparing a prepared scene changes nothing.
pub fn prepare(scene: &Scene) -> Result<Scene, MeshError> {
    scene.validate()?;
    let mut s = pose_statically(scene)?;
    bake_unrepresentable_placements(&mut s);
    reduce_appearance(&mut s);
    drop_degenerate_triangles(&mut s);
    prune(&mut s);
    s.validate()?;
    Ok(s)
}

/// The scene with no clip playing, and nothing left that moves: skins and
/// morph targets evaluated exactly as `pose::world_primitives` does, but only
/// for the nodes that need it, so a static mesh drawn a thousand times stays
/// one mesh drawn a thousand times.
fn pose_statically(scene: &Scene) -> Result<Scene, MeshError> {
    let mut s = scene.clone();
    s.animations.clear();
    let has_skins = scene.nodes.iter().any(|n| n.skin.is_some());
    let has_morphs = scene.meshes.iter().any(|m| m.target_count() > 0);
    if has_skins || has_morphs {
        let pose = pose::default_pose(scene);
        let world = scene.world_matrices();
        // Instances that share a mesh and its weights share the baked copy.
        let mut morphed: HashMap<(usize, Vec<u32>), usize> = HashMap::new();
        for (n, node) in scene.nodes.iter().enumerate() {
            let Some(mi) = node.mesh else { continue };
            let mesh = &scene.meshes[mi];
            let weights = pose.weights.get(n).cloned().unwrap_or_default();
            let skin = node.skin.and_then(|k| scene.skins.get(k));
            let skinned = skin.is_some() && mesh.primitives.iter().any(|p| p.skin.is_some());
            if let (true, Some(skin)) = (skinned, skin) {
                let joint_mats: Vec<Mat4> = skin
                    .joints
                    .iter()
                    .zip(&skin.inverse_bind_matrices)
                    .map(|(&j, ibm)| math::mat_mul(&world[j], ibm))
                    .collect();
                let mut prims = Vec::with_capacity(mesh.primitives.len());
                for p in &mesh.primitives {
                    let (mut positions, _) = pose::morphed(p, &weights);
                    let mut indices = p.indices.clone();
                    match &p.skin {
                        Some(sw) => {
                            pose::skin_in_place(&mut positions, None, sw, &joint_mats);
                            if joint_mats.first().is_some_and(math::is_mirroring) {
                                flip_winding(&mut indices);
                            }
                        }
                        None => {
                            for q in positions.iter_mut() {
                                *q = math::transform_point(&world[n], *q);
                            }
                            if math::is_mirroring(&world[n]) {
                                flip_winding(&mut indices);
                            }
                        }
                    }
                    prims.push(static_primitive(p, positions, indices));
                }
                s.meshes.push(SceneMesh { name: mesh.name.clone(), primitives: prims, ..Default::default() });
                let baked = s.meshes.len() - 1;
                s.nodes[n].mesh = None;
                s.add_root(Node { name: node.name.clone(), mesh: Some(baked), ..Default::default() });
            } else if mesh.target_count() > 0 {
                let key = (mi, weights.iter().map(|w| w.to_bits()).collect::<Vec<u32>>());
                let baked = match morphed.get(&key) {
                    Some(&b) => b,
                    None => {
                        let prims = mesh
                            .primitives
                            .iter()
                            .map(|p| static_primitive(p, pose::morphed(p, &weights).0, p.indices.clone()))
                            .collect();
                        s.meshes.push(SceneMesh { name: mesh.name.clone(), primitives: prims, ..Default::default() });
                        let b = s.meshes.len() - 1;
                        morphed.insert(key, b);
                        b
                    }
                };
                s.nodes[n].mesh = Some(baked);
            }
        }
    }
    s.skins.clear();
    for n in s.nodes.iter_mut() {
        n.skin = None;
        n.weights = None;
    }
    for m in s.meshes.iter_mut() {
        m.weights.clear();
        m.target_names.clear();
        for p in m.primitives.iter_mut() {
            p.targets.clear();
            p.skin = None;
        }
    }
    Ok(s)
}

fn static_primitive(p: &Primitive, positions: Vec<[f32; 3]>, indices: Vec<u32>) -> Primitive {
    Primitive {
        positions,
        normals: None,
        tangents: None,
        uvs: p.uvs.clone(),
        colors: p.colors.clone(),
        skin: None,
        indices,
        material: p.material,
        targets: Vec::new(),
    }
}

/// Mesh placements whose world transform does not decompose into TRS, baked
/// into world-space copies at new identity roots.
fn bake_unrepresentable_placements(s: &mut Scene) {
    let world = s.world_matrices();
    for n in s.topological_order() {
        let Some(mi) = s.nodes[n].mesh else { continue };
        let w = world[n];
        // Kept as a placement only when it is an honest one: TRS (no shear)
        // and not collapsing the part, which a singular transform would.
        if math::decompose(&w).is_some() && math::det3(&math::upper3(&w)).abs() > 1e-12 {
            continue;
        }
        let mut mesh = s.meshes[mi].clone();
        for p in mesh.primitives.iter_mut() {
            for q in p.positions.iter_mut() {
                *q = math::transform_point(&w, *q);
            }
            p.normals = None;
            p.tangents = None;
            if math::is_mirroring(&w) {
                flip_winding(&mut p.indices);
            }
        }
        s.meshes.push(mesh);
        let baked = s.meshes.len() - 1;
        s.nodes[n].mesh = None;
        let name = s.nodes[n].name.clone();
        s.add_root(Node { name, mesh: Some(baked), ..Default::default() });
    }
}

/// The material of vertex-coloured triangles: white, so the colours are the
/// colour, and matte. Left without one, a glTF primitive takes the spec's
/// default material, which is fully METALLIC — a lit viewer would show a
/// printed colour as tinted chrome.
fn color_material() -> Material {
    Material { metallic: 0.0, roughness: 1.0, ..Default::default() }
}

fn textured_material(texture: usize) -> Material {
    Material {
        base_color_texture: Some(TextureRef { texture, uv_set: 0, transform: None }),
        metallic: 0.0,
        roughness: 1.0,
        ..Default::default()
    }
}

/// `<base name>` is required; a material with none is named by its position.
fn base_material(m: &Material, index: usize) -> Material {
    let name = m.name.as_deref().map(clean_text).filter(|n| !n.trim().is_empty());
    Material {
        name: Some(name.unwrap_or_else(|| format!("material_{index}"))),
        base_color: quantized(m.base_color),
        metallic: 0.0,
        roughness: 1.0,
        ..Default::default()
    }
}

fn reduce_appearance(s: &mut Scene) {
    let looks: Vec<Vec<Look>> =
        s.meshes.iter().map(|m| m.primitives.iter().map(|p| look(s, p)).collect()).collect();
    let old = std::mem::take(&mut s.materials);
    let mut materials: Vec<Material> = Vec::new();
    let mut textured: HashMap<usize, usize> = HashMap::new();
    let mut based: HashMap<usize, usize> = HashMap::new();
    let mut colored: Option<usize> = None;
    for (mi, mesh) in s.meshes.iter_mut().enumerate() {
        for (pi, p) in mesh.primitives.iter_mut().enumerate() {
            p.normals = None;
            p.tangents = None;
            p.uvs.truncate(1);
            let material = p.material.and_then(|m| old.get(m));
            match looks[mi][pi] {
                Look::Texture(t) => {
                    let transform = material.and_then(|m| m.base_color_texture.as_ref()).and_then(|r| r.transform);
                    if let (Some(tf), Some(set)) = (transform, p.uvs.first_mut()) {
                        for uv in set.iter_mut() {
                            *uv = tf.apply(*uv);
                        }
                    }
                    p.colors = None;
                    let k = *textured.entry(t).or_insert_with(|| {
                        materials.push(textured_material(t));
                        materials.len() - 1
                    });
                    p.material = Some(k);
                }
                Look::Colors => {
                    let base = material.map_or([1.0; 4], |m| m.base_color);
                    if let Some(cs) = p.colors.as_mut() {
                        for c in cs.iter_mut() {
                            *c = quantized([c[0] * base[0], c[1] * base[1], c[2] * base[2], 1.0]);
                        }
                    }
                    p.uvs.clear();
                    let k = *colored.get_or_insert_with(|| {
                        materials.push(color_material());
                        materials.len() - 1
                    });
                    p.material = Some(k);
                }
                Look::Base(m) => {
                    p.uvs.clear();
                    p.colors = None;
                    let k = *based.entry(m).or_insert_with(|| {
                        let index = materials.len();
                        materials.push(base_material(&old[m], index));
                        index
                    });
                    p.material = Some(k);
                }
                Look::Plain => {
                    p.uvs.clear();
                    p.colors = None;
                    p.material = None;
                }
            }
        }
    }
    s.materials = materials;
    for t in s.textures.iter_mut() {
        t.name = None;
        t.sampler = representable_sampler(&t.sampler);
    }
}

fn drop_degenerate_triangles(s: &mut Scene) {
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
        let positions = &p.positions;
        let mut kept = Vec::with_capacity(p.indices.len());
        for t in p.indices.chunks_exact(3) {
            let k = |i: u32| positions.get(i as usize).map(|v| weld_key(*v));
            let (a, b, c) = (k(t[0]), k(t[1]), k(t[2]));
            if a != b && b != c && a != c {
                kept.extend_from_slice(t);
            }
        }
        p.indices = kept;
    }
}

/// Drop what nothing draws any more, remapping indices, and name each image
/// after the part it is written to.
fn prune(s: &mut Scene) {
    for m in s.meshes.iter_mut() {
        m.primitives.retain(|p| !p.indices.is_empty());
    }
    let mut used = vec![false; s.meshes.len()];
    for n in s.nodes.iter_mut() {
        if let Some(m) = n.mesh {
            if s.meshes[m].primitives.is_empty() {
                n.mesh = None;
            } else {
                used[m] = true;
            }
        }
    }
    let mut remap = vec![None; s.meshes.len()];
    let mut meshes = Vec::new();
    for (i, m) in std::mem::take(&mut s.meshes).into_iter().enumerate() {
        if used[i] {
            remap[i] = Some(meshes.len());
            meshes.push(m);
        }
    }
    s.meshes = meshes;
    for n in s.nodes.iter_mut() {
        n.mesh = n.mesh.and_then(|m| remap[m]);
    }

    let mut used = vec![false; s.materials.len()];
    for p in s.meshes.iter().flat_map(|m| &m.primitives) {
        if let Some(m) = p.material {
            used[m] = true;
        }
    }
    let mut remap = vec![None; s.materials.len()];
    let mut materials = Vec::new();
    for (i, m) in std::mem::take(&mut s.materials).into_iter().enumerate() {
        if used[i] {
            remap[i] = Some(materials.len());
            materials.push(m);
        }
    }
    s.materials = materials;
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
        p.material = p.material.and_then(|m| remap[m]);
    }
    pose::prune_unused_textures(s);
    let names = texture_parts(&s.images);
    for (im, (stem, _)) in s.images.iter_mut().zip(names) {
        im.name = Some(stem);
    }
}

/// The part each image is written to, `3D/Textures/<stem>.<ext>`, and its stem:
/// readable, unique under OPC's case-insensitive part names, and safe on any
/// filesystem the package is unzipped onto. The stem is cut short enough that
/// a de-duplicating suffix keeps it within `safe_file_stem`'s 64 characters,
/// so naming the images a second time changes nothing.
fn texture_parts(images: &[Image]) -> Vec<(String, String)> {
    let mut used: Vec<String> = Vec::new();
    images
        .iter()
        .enumerate()
        .map(|(i, im)| {
            let base: String =
                package::safe_file_stem(im.name.as_deref().unwrap_or(""), &format!("texture_{i}")).chars().take(48).collect();
            let mut stem = base.clone();
            let mut n = 1;
            while used.iter().any(|u| u.eq_ignore_ascii_case(&stem)) {
                stem = format!("{base}_{n}");
                n += 1;
            }
            used.push(stem.clone());
            let part = format!("{TEXTURE_FOLDER}/{stem}.{}", im.format.extension());
            (stem, part)
        })
        .collect()
}

// ─── losses ─────────────────────────────────────────────────────────────────

/// What a 3MF cannot hold beyond what [`super::scene::losses`] names from the
/// capability table: the consequences of one property group per triangle, of
/// texture coordinates that exist only inside a texture, and of 3MF's place
/// in the world (parts, not a node tree).
pub fn losses(scene: &Scene) -> Vec<Loss> {
    let mut out = Vec::new();
    let mut push = |kind: LossKind, what: String, why: &str| out.push(Loss::new(kind, what, format!("3mf {why}")));
    let (mut tinted, mut tex_colors, mut stray_uvs, mut alpha, mut unbound) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut filters: Vec<usize> = Vec::new();
    let mut degenerate = 0usize;
    for p in scene.meshes.iter().flat_map(|m| &m.primitives) {
        let material = p.material.and_then(|m| scene.materials.get(m));
        let l = look(scene, p);
        match l {
            Look::Texture(t) => {
                if material.is_some_and(|m| m.base_color != [1.0; 4]) {
                    tinted += 1;
                }
                if p.colors.is_some() {
                    tex_colors += 1;
                }
                let survives = scene
                    .textures
                    .get(t)
                    .is_none_or(|tx| effective_filters(&tx.sampler) == effective_filters(&representable_sampler(&tx.sampler)));
                if !survives && !filters.contains(&t) {
                    filters.push(t);
                }
            }
            Look::Colors => {
                let base_alpha = material.map_or(1.0, |m| m.base_color[3]);
                if base_alpha < 1.0 || p.colors.as_ref().is_some_and(|cs| cs.iter().any(|c| c[3] < 1.0)) {
                    alpha += 1;
                }
                if !p.uvs.is_empty() {
                    stray_uvs += 1;
                }
            }
            Look::Base(_) | Look::Plain => {
                if !p.uvs.is_empty() {
                    stray_uvs += 1;
                }
            }
        }
        // A base-colour texture that could not be used for a reason other than
        // its image format (which `scene::losses` names): no UVs, or a UV set
        // past the first.
        let tex = material.and_then(|m| m.base_color_texture.as_ref());
        let universal = tex
            .and_then(|r| scene.textures.get(r.texture))
            .and_then(|t| scene.images.get(t.image))
            .is_some_and(|im| im.format.is_universal());
        if tex.is_some() && universal && !matches!(l, Look::Texture(_)) {
            unbound += 1;
        }
        for t in p.indices.chunks_exact(3) {
            let k = |i: u32| p.positions.get(i as usize).map(|v| weld_key(*v));
            if k(t[0]) == k(t[1]) || k(t[1]) == k(t[2]) || k(t[0]) == k(t[2]) {
                degenerate += 1;
            }
        }
    }
    if tinted > 0 {
        push(
            LossKind::Materials,
            format!("base-colour factor on {tinted} textured primitive(s)"),
            "gives each triangle one property group, so a texture cannot be tinted; the factor is dropped",
        );
    }
    if tex_colors > 0 {
        push(
            LossKind::VertexColors,
            format!("vertex colours on {tex_colors} textured primitive(s)"),
            "gives each triangle one property group; the texture is kept and the vertex colours dropped",
        );
    }
    if unbound > 0 {
        push(
            LossKind::Textures,
            format!("base-colour texture on {unbound} primitive(s) without UV set 0"),
            "maps a texture through its own coordinate group, which needs the first UV set; the base colour is used instead",
        );
    }
    if stray_uvs > 0 {
        push(
            LossKind::Uvs,
            format!("texture coordinates on {stray_uvs} untextured primitive(s)"),
            "stores texture coordinates only inside a texture's coordinate group",
        );
    }
    if alpha > 0 {
        push(LossKind::Alpha, format!("vertex colour alpha on {alpha} primitive(s)"), "colour groups are opaque");
    }
    if !filters.is_empty() {
        push(
            LossKind::TextureSampling,
            format!("sampling filter of {} texture(s)", filters.len()),
            "filters textures as auto, linear or nearest; other mipmap modes take the closest of those",
        );
    }
    if degenerate > 0 {
        push(
            LossKind::Geometry,
            format!("{degenerate} degenerate triangle(s)"),
            "requires a triangle's three vertices to be distinct; triangles with two corners at one point are dropped",
        );
    }
    let world = scene.world_matrices();
    let sheared = scene
        .nodes
        .iter()
        .enumerate()
        .filter(|(n, node)| node.mesh.is_some() && node.skin.is_none() && math::decompose(&world[*n]).is_none())
        .count();
    if sheared > 0 {
        push(
            LossKind::Geometry,
            format!("instancing of {sheared} sheared placement(s)"),
            "consumers decompose placements into translation, rotation and scale; sheared ones are baked into their own geometry",
        );
    }
    let empty = scene.nodes.iter().filter(|n| n.mesh.is_none()).count();
    if empty > 0 {
        push(
            LossKind::Hierarchy,
            format!("{empty} node(s) without geometry"),
            "places each part directly in one model; every part keeps its placement, the grouping nodes and their names do not survive",
        );
    }
    out
}

// ─── writer ─────────────────────────────────────────────────────────────────

/// Write a scene as a 3MF package: one file, textures inside it.
pub fn write_scene(scene: &Scene) -> Result<Package, MeshError> {
    if scene.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    // Always through `prepare`, which is idempotent: the encoder below relies
    // on every invariant it establishes, and a caller that already prepared
    // pays one clone for the guarantee.
    let s = prepare(scene)?;
    Ok(Package::single(PRIMARY_FILE, write_package(&s)?))
}

/// Write the flat mesh as a single-object 3MF.
pub fn write(mesh: &Mesh) -> Result<Vec<u8>, MeshError> {
    mesh.validate()?;
    if mesh.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    let mut p = write_scene(&pose::from_flat(mesh))?;
    Ok(p.files.remove(0).bytes)
}

/// A property group a triangle can name. Ids are assigned once every group's
/// contents are known, because groups must be defined before the objects that
/// reference them (Core §3.4: "Producers MUST define each element prior to
/// referencing it").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Group {
    Base,
    Colors,
    /// The texture2dgroup of one scene texture.
    Texture(usize),
}

struct TriOut {
    v: [u32; 3],
    prop: Option<(Group, [u32; 3])>,
}

struct ObjectOut {
    name: Option<String>,
    vertices: Vec<[f32; 3]>,
    triangles: Vec<TriOut>,
}

/// One texture's coordinate group, deduplicated as it fills.
struct CoordGroup {
    texture: usize,
    coords: Vec<[f32; 2]>,
    index: HashMap<[u32; 2], u32>,
}

#[derive(Default)]
struct Groups {
    bases: Vec<(String, [u8; 4])>,
    base_of: HashMap<usize, u32>,
    white: Option<u32>,
    colors: Vec<[u8; 3]>,
    color_of: HashMap<[u8; 3], u32>,
    /// One coordinate group per texture, in first-use order.
    textures: Vec<CoordGroup>,
    texture_slot: HashMap<usize, usize>,
}

impl Groups {
    fn base(&mut self, scene: &Scene, m: usize) -> u32 {
        if let Some(&i) = self.base_of.get(&m) {
            return i;
        }
        let mat = &scene.materials[m];
        let name = mat.name.clone().unwrap_or_else(|| format!("material_{m}"));
        let c = mat.base_color;
        self.bases.push((name, [srgb8(c[0]), srgb8(c[1]), srgb8(c[2]), alpha8(c[3])]));
        let i = (self.bases.len() - 1) as u32;
        self.base_of.insert(m, i);
        i
    }

    /// The opaque white a property-less primitive takes when it shares an
    /// object with ones that have properties: left without, it would inherit
    /// the object's default instead of glTF's default white.
    fn white(&mut self) -> u32 {
        if let Some(i) = self.white {
            return i;
        }
        self.bases.push(("default".to_string(), [255, 255, 255, 255]));
        let i = (self.bases.len() - 1) as u32;
        self.white = Some(i);
        i
    }

    fn color(&mut self, c: [f32; 4]) -> u32 {
        let key = [srgb8(c[0]), srgb8(c[1]), srgb8(c[2])];
        if let Some(&i) = self.color_of.get(&key) {
            return i;
        }
        self.colors.push(key);
        let i = (self.colors.len() - 1) as u32;
        self.color_of.insert(key, i);
        i
    }

    /// A texture coordinate, V flipped into 3MF's lower-left origin.
    fn coord(&mut self, texture: usize, uv: [f32; 2]) -> u32 {
        let slot = *self.texture_slot.entry(texture).or_insert_with(|| {
            self.textures.push(CoordGroup { texture, coords: Vec::new(), index: HashMap::new() });
            self.textures.len() - 1
        });
        let g = &mut self.textures[slot];
        let c = [uv[0], 1.0 - uv[1]];
        let key = [(c[0] + 0.0).to_bits(), (c[1] + 0.0).to_bits()];
        if let Some(&i) = g.index.get(&key) {
            return i;
        }
        g.coords.push(c);
        let i = (g.coords.len() - 1) as u32;
        g.index.insert(key, i);
        i
    }
}

fn object_out(scene: &Scene, mesh: &SceneMesh, groups: &mut Groups) -> ObjectOut {
    let looks: Vec<Look> = mesh.primitives.iter().map(|p| look(scene, p)).collect();
    let mixed = looks.iter().any(|l| *l != Look::Plain);
    let mut index: HashMap<[u32; 3], u32> = HashMap::new();
    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut triangles: Vec<TriOut> = Vec::with_capacity(mesh.primitives.iter().map(|p| p.triangle_count()).sum());
    for (p, l) in mesh.primitives.iter().zip(&looks) {
        for t in p.indices.chunks_exact(3) {
            let mut v = [0u32; 3];
            for k in 0..3 {
                let pos = p.positions[t[k] as usize];
                v[k] = *index.entry(weld_key(pos)).or_insert_with(|| {
                    vertices.push(pos);
                    (vertices.len() - 1) as u32
                });
            }
            if v[0] == v[1] || v[1] == v[2] || v[0] == v[2] {
                continue;
            }
            let prop = match *l {
                Look::Texture(tx) => {
                    let set = &p.uvs[0];
                    let c = [0, 1, 2].map(|k| groups.coord(tx, set[t[k] as usize]));
                    Some((Group::Texture(tx), c))
                }
                Look::Colors => {
                    let cs = p.colors.as_deref().unwrap_or(&[]);
                    let c = [0, 1, 2].map(|k| groups.color(cs.get(t[k] as usize).copied().unwrap_or([1.0; 4])));
                    Some((Group::Colors, c))
                }
                Look::Base(m) => {
                    let i = groups.base(scene, m);
                    Some((Group::Base, [i; 3]))
                }
                Look::Plain if mixed => {
                    let i = groups.white();
                    Some((Group::Base, [i; 3]))
                }
                Look::Plain => None,
            };
            triangles.push(TriOut { v, prop });
        }
    }
    ObjectOut { name: mesh.name.clone(), vertices, triangles }
}

/// The model part, and the images it references with their part names.
fn model_part(s: &Scene) -> Result<(String, Vec<(usize, String)>), MeshError> {
    let world = s.world_matrices();
    let mut placements: Vec<(usize, Mat4)> = Vec::new();
    for n in s.topological_order() {
        if let Some(m) = s.nodes[n].mesh {
            if s.meshes[m].primitives.iter().any(|p| p.triangle_count() > 0) {
                placements.push((m, world[n]));
            }
        }
    }
    if placements.is_empty() {
        return Err(MeshError::Empty);
    }

    // One object per mesh, in first-placement order.
    let mut groups = Groups::default();
    let mut object_of: Vec<Option<usize>> = vec![None; s.meshes.len()];
    let mut objects: Vec<ObjectOut> = Vec::new();
    for (m, _) in &placements {
        if object_of[*m].is_none() {
            object_of[*m] = Some(objects.len());
            objects.push(object_out(s, &s.meshes[*m], &mut groups));
        }
    }

    // Ids, in definition order: property groups, then objects, then the
    // assembly.
    let mut next = 1u32;
    let mut take = || {
        let id = next;
        next += 1;
        id
    };
    let base_id = (!groups.bases.is_empty()).then(&mut take);
    let color_id = (!groups.colors.is_empty()).then(&mut take);
    let texture_ids: Vec<(u32, u32)> = groups.textures.iter().map(|_| (take(), take())).collect();
    let object_ids: Vec<u32> = objects.iter().map(|_| take()).collect();
    let assembly_id = (placements.len() > 1).then(&mut take);
    let group_id = |g: Group| -> u32 {
        match g {
            Group::Base => base_id.unwrap_or(0),
            Group::Colors => color_id.unwrap_or(0),
            Group::Texture(t) => groups.texture_slot.get(&t).map_or(0, |&slot| texture_ids[slot].1),
        }
    };
    let uses_materials_extension = color_id.is_some() || !texture_ids.is_empty();

    let image_parts = texture_parts(&s.images);
    let mut images_used: Vec<(usize, String)> = Vec::new();

    let mut x = String::with_capacity(
        64 * objects.iter().map(|o| o.vertices.len() + o.triangles.len()).sum::<usize>() + 4096,
    );
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    // xml:lang "und": the titles and names are whatever the source said, in a
    // language nobody recorded, and Core §2.3.5 prescribes "und" for that.
    let _ = write!(x, "<model unit=\"meter\" xml:lang=\"und\" xmlns=\"{NS_CORE}\"");
    if uses_materials_extension {
        let _ = write!(x, " xmlns:m=\"{NS_MATERIAL}\" requiredextensions=\"m\"");
    }
    x.push_str(">\n");
    if let Some(title) = s.name.as_deref().map(clean_text).filter(|t| !t.trim().is_empty()) {
        let _ = writeln!(x, " <metadata name=\"Title\">{}</metadata>", xml::escape(&title));
    }
    let _ = writeln!(x, " <metadata name=\"Application\">{APPLICATION}</metadata>");
    x.push_str(" <resources>\n");

    if let Some(id) = base_id {
        let _ = writeln!(x, "  <basematerials id=\"{id}\">");
        for (name, c) in &groups.bases {
            let _ = writeln!(x, "   <base name=\"{}\" displaycolor=\"{}\"/>", escape_attr(&clean_text(name)), hex_color(*c));
        }
        x.push_str("  </basematerials>\n");
    }
    if let Some(id) = color_id {
        let _ = writeln!(x, "  <m:colorgroup id=\"{id}\">");
        for c in &groups.colors {
            let _ = writeln!(x, "<m:color color=\"{}\"/>", hex_color([c[0], c[1], c[2], 255]));
        }
        x.push_str("  </m:colorgroup>\n");
    }
    for (slot, g) in groups.textures.iter().enumerate() {
        let texture = g.texture;
        let tx = s.textures.get(texture).ok_or_else(|| malformed(format!("texture {texture} is out of range")))?;
        let im = s.images.get(tx.image).ok_or_else(|| malformed(format!("texture {texture} names a missing image")))?;
        let part = &image_parts[tx.image].1;
        if !images_used.iter().any(|(i, _)| *i == tx.image) {
            images_used.push((tx.image, part.clone()));
        }
        let _ = writeln!(
            x,
            "  <m:texture2d id=\"{}\" path=\"/{}\" contenttype=\"{}\" tilestyleu=\"{}\" tilestylev=\"{}\" filter=\"{}\"/>",
            texture_ids[slot].0,
            escape_attr(part),
            im.format.mime(),
            tile_style(tx.sampler.wrap_s),
            tile_style(tx.sampler.wrap_t),
            tex_filter(&tx.sampler).as_str()
        );
    }
    for (slot, g) in groups.textures.iter().enumerate() {
        let (tex_id, group) = texture_ids[slot];
        let _ = writeln!(x, "  <m:texture2dgroup id=\"{group}\" texid=\"{tex_id}\">");
        for c in &g.coords {
            x.push_str("<m:tex2coord u=\"");
            num32(&mut x, c[0]);
            x.push_str("\" v=\"");
            num32(&mut x, c[1]);
            x.push_str("\"/>\n");
        }
        x.push_str("  </m:texture2dgroup>\n");
    }

    for (o, id) in objects.iter().zip(&object_ids) {
        let _ = write!(x, "  <object id=\"{id}\" type=\"model\"");
        if let Some(name) = o.name.as_deref().map(clean_text).filter(|n| !n.is_empty()) {
            let _ = write!(x, " name=\"{}\"", escape_attr(&name));
        }
        // "If this object contains any triangles with assigned properties, the
        // object MUST specify pid and pindex" (Core §4).
        if let Some((g, p)) = o.triangles.iter().find_map(|t| t.prop) {
            let _ = write!(x, " pid=\"{}\" pindex=\"{}\"", group_id(g), p[0]);
        }
        x.push_str(">\n   <mesh>\n    <vertices>\n");
        for v in &o.vertices {
            x.push_str("<vertex x=\"");
            num32(&mut x, v[0]);
            x.push_str("\" y=\"");
            num32(&mut x, v[1]);
            x.push_str("\" z=\"");
            num32(&mut x, v[2]);
            x.push_str("\"/>\n");
        }
        x.push_str("    </vertices>\n    <triangles>\n");
        for t in &o.triangles {
            let _ = write!(x, "<triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"", t.v[0], t.v[1], t.v[2]);
            match t.prop {
                // Base materials cannot form gradients: p1 alone (Core §4.1.4.1).
                Some((Group::Base, p)) => {
                    let _ = write!(x, " pid=\"{}\" p1=\"{}\"", group_id(Group::Base), p[0]);
                }
                // The group on every triangle, not only on the object: assimp
                // reads per-corner colour only from triangles that carry it.
                Some((g, p)) => {
                    let _ = write!(x, " pid=\"{}\" p1=\"{}\" p2=\"{}\" p3=\"{}\"", group_id(g), p[0], p[1], p[2]);
                }
                None => {}
            }
            x.push_str("/>\n");
        }
        x.push_str("    </triangles>\n   </mesh>\n  </object>\n");
    }

    let (item_object, item_transform) = match assembly_id {
        Some(id) => {
            let _ = write!(x, "  <object id=\"{id}\" type=\"model\"");
            if let Some(name) = s.name.as_deref().map(clean_text).filter(|n| !n.is_empty()) {
                let _ = write!(x, " name=\"{}\"", escape_attr(&name));
            }
            x.push_str(">\n   <components>\n");
            for (m, w) in &placements {
                let oid = object_of[*m].map_or(0, |o| object_ids[o]);
                let _ = write!(x, "    <component objectid=\"{oid}\"");
                if *w != math::IDENTITY {
                    let _ = write!(x, " transform=\"{}\"", matrix_attr(w));
                }
                x.push_str("/>\n");
            }
            x.push_str("   </components>\n  </object>\n");
            (id, Y_UP_TO_Z_UP)
        }
        None => {
            let (m, w) = placements[0];
            (object_of[m].map_or(0, |o| object_ids[o]), math::mat_mul(&Y_UP_TO_Z_UP, &w))
        }
    };
    x.push_str(" </resources>\n <build>\n");
    let _ = writeln!(x, "  <item objectid=\"{item_object}\" transform=\"{}\"/>", matrix_attr(&item_transform));
    x.push_str(" </build>\n</model>\n");
    Ok((x, images_used))
}

fn write_package(s: &Scene) -> Result<Vec<u8>, MeshError> {
    let (model, images) = model_part(s)?;

    let mut types = String::new();
    types.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(types, "<Types xmlns=\"{NS_CONTENT_TYPES}\">");
    let _ = writeln!(types, " <Default Extension=\"rels\" ContentType=\"{CT_RELATIONSHIPS}\"/>");
    let _ = writeln!(types, " <Default Extension=\"model\" ContentType=\"{CT_MODEL}\"/>");
    let mut extensions: Vec<(&str, &str)> = Vec::new();
    for (i, _) in &images {
        let f = s.images[*i].format;
        if !extensions.iter().any(|(e, _)| *e == f.extension()) {
            extensions.push((f.extension(), f.mime()));
        }
    }
    for (ext, mime) in &extensions {
        let _ = writeln!(types, " <Default Extension=\"{ext}\" ContentType=\"{mime}\"/>");
    }
    types.push_str("</Types>\n");

    let mut rels = String::new();
    rels.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(rels, "<Relationships xmlns=\"{NS_RELATIONSHIPS}\">");
    let _ = writeln!(rels, " <Relationship Id=\"rel0\" Target=\"/{MODEL_PART}\" Type=\"{REL_START_PART}\"/>");
    rels.push_str("</Relationships>\n");

    let mut zip = zip::Writer::new();
    zip.add(CONTENT_TYPES_PART, types.as_bytes(), Store::Deflated)?;
    zip.add(PACKAGE_RELS_PART, rels.as_bytes(), Store::Deflated)?;
    zip.add(MODEL_PART, model.as_bytes(), Store::Deflated)?;
    if !images.is_empty() {
        let mut mrels = String::new();
        mrels.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        let _ = writeln!(mrels, "<Relationships xmlns=\"{NS_RELATIONSHIPS}\">");
        for (k, (_, part)) in images.iter().enumerate() {
            let _ = writeln!(mrels, " <Relationship Id=\"tex{k}\" Target=\"/{}\" Type=\"{REL_TEXTURE}\"/>", escape_attr(part));
        }
        mrels.push_str("</Relationships>\n");
        zip.add(MODEL_RELS_PART, mrels.as_bytes(), Store::Deflated)?;
        // PNG and JPEG are compressed already; deflating them again costs time
        // and saves nothing.
        for (i, part) in &images {
            zip.add(part, &s.images[*i].data, Store::Stored { align: 1 })?;
        }
    }
    Ok(zip.finish())
}

// ─── reader: package ────────────────────────────────────────────────────────

/// The ZIP with OPC's case-insensitive part names over it.
struct Pkg<'a> {
    archive: Archive<'a>,
    by_name: HashMap<String, usize>,
}

impl<'a> Pkg<'a> {
    fn new(archive: Archive<'a>) -> Result<Pkg<'a>, MeshError> {
        let mut by_name = HashMap::with_capacity(archive.entries.len());
        for (i, e) in archive.entries.iter().enumerate() {
            if let Some(prev) = by_name.insert(e.name.to_ascii_lowercase(), i) {
                // OPC compares part names as case-insensitive ASCII: two
                // entries that differ only in case are one part named twice,
                // and two readers may each pick a different one.
                return Err(malformed(format!(
                    "parts {} and {} differ only in case",
                    quoted(&archive.entries[prev].name),
                    quoted(&e.name)
                )));
            }
        }
        Ok(Pkg { archive, by_name })
    }

    fn read(&self, part: &str) -> Result<Option<Vec<u8>>, MeshError> {
        match self.by_name.get(&part.to_ascii_lowercase()) {
            Some(&i) => self.archive.read(&self.archive.entries[i]).map(Some),
            None => Ok(None),
        }
    }
}

/// A part name from a reference inside the package: absolute (`/3D/x.png`,
/// what 3MF requires) or relative to the referring part. `None` for anything
/// that points outside the package.
fn part_name(reference: &str, base: &str) -> Option<String> {
    let r = reference.trim();
    // A scheme (`http:`, `file:`, a drive letter) names something outside the
    // package; checked here because resolving against the referring part's
    // directory would bury it mid-path, where it reads as a folder name.
    if r.find(':').is_some_and(|colon| !r[..colon].contains('/')) {
        return None;
    }
    match r.strip_prefix('/') {
        Some(abs) => package::normalize_relative(abs),
        None => package::resolve_relative(base, r),
    }
}

/// The root model part, from the package relationships' StartPart.
fn start_part(pkg: &Pkg<'_>) -> Result<String, MeshError> {
    let Some(rels) = pkg.read(PACKAGE_RELS_PART)? else {
        return Err(not_3mf("the ZIP has no _rels/.rels, so it is not an OPC package"));
    };
    let doc = xml::parse(&rels)?;
    if !doc.is(NS_RELATIONSHIPS, "Relationships") {
        return Err(malformed("_rels/.rels is not an OPC relationships part"));
    }
    let starts: Vec<&Element> = doc
        .children
        .iter()
        .filter(|r| r.is(NS_RELATIONSHIPS, "Relationship") && r.attr("Type").map(str::trim) == Some(REL_START_PART))
        .collect();
    let start = match starts.as_slice() {
        // Another OPC document (a .docx) has no model part either; a package
        // that has one and does not name it is a broken 3MF.
        [] if pkg.by_name.keys().any(|n| n.starts_with("3d/") && n.ends_with(".model")) => {
            return Err(malformed("the package has a 3D model part but no StartPart relationship naming it"))
        }
        [] => return Err(not_3mf("the OPC package has no 3D model StartPart relationship")),
        [one] => *one,
        _ => return Err(malformed("the package names more than one 3D model StartPart")),
    };
    if start.attr("TargetMode").is_some_and(|m| m.trim().eq_ignore_ascii_case("External")) {
        return Err(malformed("the 3D model StartPart points outside the package"));
    }
    let target = start.attr("Target").ok_or_else(|| malformed("the StartPart relationship has no Target"))?;
    part_name(target, "").ok_or_else(|| malformed(format!("StartPart target {} is not a part of this package", quoted(target))))
}

// ─── reader: model parts ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjKind {
    Model,
    SolidSupport,
    Support,
    Surface,
    Other,
}

impl ObjKind {
    fn parse(s: &str) -> Option<ObjKind> {
        Some(match s.trim() {
            "model" => ObjKind::Model,
            "solidsupport" => ObjKind::SolidSupport,
            "support" => ObjKind::Support,
            "surface" => ObjKind::Surface,
            "other" => ObjKind::Other,
            _ => return None,
        })
    }
}

/// A triangle as the file states it: property fields absent stay absent, so
/// the object's defaults can be applied exactly as the spec says.
struct Tri {
    v: [u32; 3],
    pid: Option<u32>,
    p: [Option<u32>; 3],
}

struct Comp {
    objectid: u32,
    path: Option<String>,
    transform: Mat4,
}

enum Shape {
    Empty,
    Mesh { vertices: Vec<[f32; 3]>, triangles: Vec<Tri> },
    Components(Vec<Comp>),
}

struct Obj {
    id: u32,
    kind: ObjKind,
    name: Option<String>,
    pid: Option<u32>,
    pindex: Option<u32>,
    shape: Shape,
}

struct Tex2d {
    path: String,
    wrap_u: Wrap,
    wrap_v: Wrap,
    filter: TexFilter,
}

struct Composite {
    matid: u32,
    matindices: Vec<u32>,
    mixes: Vec<Vec<f32>>,
}

struct Multi {
    pids: Vec<u32>,
    entries: Vec<Vec<u32>>,
}

/// What a resource id names within one model part. IDs are unique across all
/// resources of a part (Core §3.4.2), so one map serves every kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Res {
    Object(usize),
    Bases(usize),
    Colors(usize),
    Texture(usize),
    TexCoords(usize),
    Composite(usize),
    Multi(usize),
    /// Display properties and other resources that are recognised and not
    /// read; a triangle naming one is an error.
    Ignored,
}

struct Item {
    objectid: u32,
    path: Option<String>,
    transform: Mat4,
}

/// One model part, parsed but not yet interpreted.
struct Part {
    name: String,
    /// Metres per file unit.
    unit: f64,
    title: Option<String>,
    ids: HashMap<u32, Res>,
    objects: Vec<Obj>,
    bases: Vec<Vec<(String, [u8; 4])>>,
    colors: Vec<Vec<[u8; 4]>>,
    textures: Vec<Tex2d>,
    texcoords: Vec<(u32, Vec<[f32; 2]>)>,
    composites: Vec<Composite>,
    multis: Vec<Multi>,
    items: Vec<Item>,
    saw_resources: bool,
    saw_build: bool,
    /// Lower-cased targets of this part's 3D Texture relationships, read the
    /// first time a texture is.
    texture_rels: Option<Vec<String>>,
}

/// Running totals across every part, charged before anything is allocated.
#[derive(Default)]
struct Budget {
    vertices: usize,
    triangles: usize,
    properties: usize,
}

impl Budget {
    fn vertex(&mut self) -> Result<(), MeshError> {
        self.vertices += 1;
        if self.vertices > MAX_SCENE_VERTICES {
            return Err(unsupported(format!(
                "the package has more than {MAX_SCENE_VERTICES} vertices, past what this converter will hold"
            )));
        }
        Ok(())
    }

    fn triangle(&mut self) -> Result<(), MeshError> {
        self.triangles += 1;
        if self.triangles.saturating_mul(3) > MAX_SCENE_INDICES {
            return Err(unsupported(format!(
                "the package has more than {} triangles, past what this converter will hold",
                MAX_SCENE_INDICES / 3
            )));
        }
        Ok(())
    }

    fn property(&mut self) -> Result<(), MeshError> {
        self.properties += 1;
        if self.properties > MAX_PROPERTIES {
            return Err(unsupported(format!("the package has more than {MAX_PROPERTIES} property entries")));
        }
        Ok(())
    }
}

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

/// Where the streaming parser is in the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Document,
    Model,
    Title,
    Resources,
    Bases(usize),
    Colors(usize),
    TexCoords(usize),
    Composite(usize),
    Multi(usize),
    Object(usize),
    Mesh(usize),
    Vertices(usize),
    Triangles(usize),
    Components(usize),
    Build,
    /// An element whose content is not read: a leaf, metadata, or anything in
    /// a namespace this reader does not interpret (extensions MUST be
    /// ignorable, Core §2.3.3.1).
    Skip,
}

/// Extensions a model may require that this reader does not implement, by
/// what they would change. Anything else unknown is refused too; these get a
/// name in the message.
const UNREAD_EXTENSIONS: &[(&str, &str)] = &[(NS_SLICE, "slice"), (NS_BEAM_LATTICE, "beam lattice")];

fn unit_scale(unit: &str) -> Option<f64> {
    Some(match unit.trim() {
        "micron" => 1e-6,
        "millimeter" => 1e-3,
        "centimeter" => 1e-2,
        "inch" => 0.0254,
        "foot" => 0.3048,
        "meter" => 1.0,
        _ => return None,
    })
}

fn wrap_style(s: Option<&str>) -> Option<Wrap> {
    Some(match s.map(str::trim) {
        None | Some("wrap") => Wrap::Repeat,
        Some("mirror") => Wrap::MirroredRepeat,
        // "none" outside a multi-property layer "behaves as clamp" (§6).
        Some("clamp") | Some("none") => Wrap::ClampToEdge,
        Some(_) => return None,
    })
}

struct PartParser<'w> {
    part: Part,
    stack: Vec<Ctx>,
    seen_root: bool,
    title: Option<String>,
    /// Where the current `<mesh>`'s vertices start in its object's list:
    /// non-zero only for a second `<mesh>` in one object.
    vertex_base: u32,
    budget: &'w mut Budget,
    warnings: &'w mut Warnings,
}

impl PartParser<'_> {
    fn register(&mut self, id: u32, res: Res) -> Result<(), MeshError> {
        if self.part.ids.len() >= MAX_RESOURCES {
            return Err(unsupported(format!("{} defines more than {MAX_RESOURCES} resources", self.part.name)));
        }
        if self.part.ids.insert(id, res).is_some() {
            return Err(malformed(format!("resource id {id} is defined twice in {}", self.part.name)));
        }
        Ok(())
    }

    fn id_attr(&self, el: &Element, attr: &str) -> Result<u32, MeshError> {
        let v = el.attr(attr).ok_or_else(|| malformed(format!("<{}> has no {attr}", el.name)))?;
        parse_index(v).ok_or_else(|| malformed(format!("<{}> {attr} {} is not a resource id", el.name, quoted(v))))
    }

    fn opt_index(el: &Element, attr: &str) -> Result<Option<u32>, MeshError> {
        match el.attr(attr) {
            None => Ok(None),
            Some(v) => parse_index(v)
                .map(Some)
                .ok_or_else(|| malformed(format!("<{}> {attr} {} is not an index", el.name, quoted(v)))),
        }
    }

    fn transform_attr(el: &Element) -> Result<Mat4, MeshError> {
        match el.attr("transform") {
            None => Ok(math::IDENTITY),
            Some(t) => parse_matrix(t)
                .ok_or_else(|| malformed(format!("<{}> transform {} is not twelve numbers", el.name, quoted(t)))),
        }
    }

    fn path_attr(el: &Element) -> Option<String> {
        el.attr_ns(NS_PRODUCTION, "path").map(str::to_string)
    }

    fn unexpected(el: &Element, parent: Ctx) -> MeshError {
        malformed(format!("<{}> cannot appear where it does ({parent:?})", el.name))
    }

    fn model(&mut self, el: &Element) -> Result<(), MeshError> {
        if let Some(u) = el.attr("unit") {
            self.part.unit = unit_scale(u).ok_or_else(|| malformed(format!("unit {} is not a 3MF unit", quoted(u))))?;
        }
        let ns_of = |prefix: &str| el.ns_decls.iter().find(|(p, _)| p == prefix).map(|(_, uri)| uri.as_str());
        if let Some(req) = el.attr("requiredextensions") {
            for prefix in req.split_ascii_whitespace() {
                let uri = ns_of(prefix).ok_or_else(|| {
                    malformed(format!("requiredextensions names prefix {}, which the model does not declare", quoted(prefix)))
                })?;
                if ![NS_CORE, NS_MATERIAL, NS_PRODUCTION, NS_TRIANGLE_SETS].contains(&uri) {
                    let what = UNREAD_EXTENSIONS.iter().find(|(u, _)| *u == uri).map_or("an unknown", |(_, n)| *n);
                    return Err(unsupported(format!(
                        "the model requires the {what} extension ({}), which this converter does not implement",
                        truncate(uri, 120)
                    )));
                }
            }
        }
        if let Some(rec) = el.attr("recommendedextensions") {
            for prefix in rec.split_ascii_whitespace() {
                let uri = ns_of(prefix).unwrap_or("");
                if ![NS_CORE, NS_MATERIAL, NS_PRODUCTION, NS_TRIANGLE_SETS].contains(&uri) {
                    self.warnings.push(format!(
                        "the model recommends the extension {}, which is not read",
                        quoted(if uri.is_empty() { prefix } else { uri })
                    ));
                }
            }
        }
        Ok(())
    }

    fn object(&mut self, el: &Element) -> Result<Ctx, MeshError> {
        let id = self.id_attr(el, "id")?;
        let kind = match el.attr("type") {
            None => ObjKind::Model,
            Some(t) => ObjKind::parse(t).ok_or_else(|| malformed(format!("object {id} type {} is not a 3MF object type", quoted(t))))?,
        };
        if el.attr_ns(NS_SLICE, "slicestackid").is_some() {
            self.warnings.push(format!("object {id} carries a slice stack, which is not read; its mesh is"));
        }
        let o = self.part.objects.len();
        self.register(id, Res::Object(o))?;
        self.part.objects.push(Obj {
            id,
            kind,
            name: el.attr("name").map(clean_text).filter(|n| !n.is_empty()),
            pid: Self::opt_index(el, "pid")?,
            pindex: Self::opt_index(el, "pindex")?,
            shape: Shape::Empty,
        });
        Ok(Ctx::Object(o))
    }

    fn start(&mut self, el: &Element) -> Result<(), MeshError> {
        let parent = self.stack.last().copied().unwrap_or(Ctx::Document);
        let core = el.ns.as_deref() == Some(NS_CORE);
        let mat = el.ns.as_deref() == Some(NS_MATERIAL);
        let name = el.name.as_str();
        let ctx = match parent {
            Ctx::Skip => Ctx::Skip,
            Ctx::Document => {
                if std::mem::replace(&mut self.seen_root, true) {
                    return Err(malformed("the model part has more than one root element"));
                }
                if !(core && name == "model") {
                    return Err(malformed(format!(
                        "{}'s root element is <{name}>{}, not a 3MF <model>",
                        self.part.name,
                        el.ns.as_deref().map(|n| format!(" in {}", truncate(n, 80))).unwrap_or_default()
                    )));
                }
                self.model(el)?;
                Ctx::Model
            }
            Ctx::Model if core => match name {
                "metadata" if el.attr("name").map(str::trim) == Some("Title") => Ctx::Title,
                "metadata" => Ctx::Skip,
                "resources" => {
                    if std::mem::replace(&mut self.part.saw_resources, true) {
                        return Err(malformed("the model has more than one <resources>"));
                    }
                    Ctx::Resources
                }
                "build" => {
                    if std::mem::replace(&mut self.part.saw_build, true) {
                        return Err(malformed("the model has more than one <build>"));
                    }
                    Ctx::Build
                }
                _ => return Err(Self::unexpected(el, parent)),
            },
            Ctx::Resources if core => match name {
                "basematerials" => {
                    let id = self.id_attr(el, "id")?;
                    let g = self.part.bases.len();
                    self.register(id, Res::Bases(g))?;
                    self.part.bases.push(Vec::new());
                    Ctx::Bases(g)
                }
                "object" => self.object(el)?,
                // assimp 6 writes its metadata here; it is out of place, and
                // harmless.
                "metadata" => Ctx::Skip,
                _ => return Err(Self::unexpected(el, parent)),
            },
            Ctx::Resources if mat => match name {
                "colorgroup" => {
                    let id = self.id_attr(el, "id")?;
                    let g = self.part.colors.len();
                    self.register(id, Res::Colors(g))?;
                    self.part.colors.push(Vec::new());
                    Ctx::Colors(g)
                }
                "texture2d" => {
                    let id = self.id_attr(el, "id")?;
                    let path = el.attr("path").ok_or_else(|| malformed(format!("texture2d {id} has no path")))?;
                    let wrap = |a: &str| {
                        wrap_style(el.attr(a))
                            .ok_or_else(|| malformed(format!("texture2d {id} {a} {} is not a tile style", quoted(el.attr(a).unwrap_or("")))))
                    };
                    let filter = match el.attr("filter").map(str::trim) {
                        None | Some("auto") => TexFilter::Auto,
                        Some("linear") => TexFilter::Linear,
                        Some("nearest") => TexFilter::Nearest,
                        Some(f) => return Err(malformed(format!("texture2d {id} filter {} is not a 3MF filter", quoted(f)))),
                    };
                    let t = Tex2d { path: path.to_string(), wrap_u: wrap("tilestyleu")?, wrap_v: wrap("tilestylev")?, filter };
                    let k = self.part.textures.len();
                    self.register(id, Res::Texture(k))?;
                    self.part.textures.push(t);
                    Ctx::Skip
                }
                "texture2dgroup" => {
                    let id = self.id_attr(el, "id")?;
                    let texid = self.id_attr(el, "texid")?;
                    let g = self.part.texcoords.len();
                    self.register(id, Res::TexCoords(g))?;
                    self.part.texcoords.push((texid, Vec::new()));
                    Ctx::TexCoords(g)
                }
                "compositematerials" => {
                    let id = self.id_attr(el, "id")?;
                    let matid = self.id_attr(el, "matid")?;
                    let raw = el.attr("matindices").ok_or_else(|| malformed(format!("compositematerials {id} has no matindices")))?;
                    let matindices = raw
                        .split_ascii_whitespace()
                        .map(parse_index)
                        .collect::<Option<Vec<u32>>>()
                        .filter(|v| !v.is_empty())
                        .ok_or_else(|| malformed(format!("compositematerials {id} matindices {} are not indices", quoted(raw))))?;
                    let g = self.part.composites.len();
                    self.register(id, Res::Composite(g))?;
                    self.part.composites.push(Composite { matid, matindices, mixes: Vec::new() });
                    Ctx::Composite(g)
                }
                "multiproperties" => {
                    let id = self.id_attr(el, "id")?;
                    let raw = el.attr("pids").ok_or_else(|| malformed(format!("multiproperties {id} has no pids")))?;
                    let pids = raw
                        .split_ascii_whitespace()
                        .map(parse_index)
                        .collect::<Option<Vec<u32>>>()
                        .filter(|v| !v.is_empty())
                        .ok_or_else(|| malformed(format!("multiproperties {id} pids {} are not ids", quoted(raw))))?;
                    let g = self.part.multis.len();
                    self.register(id, Res::Multi(g))?;
                    self.part.multis.push(Multi { pids, entries: Vec::new() });
                    Ctx::Multi(g)
                }
                _ => {
                    // Display properties are rendering hints for a surface
                    // model this scene does not have.
                    if let Some(id) = el.attr("id").and_then(parse_index) {
                        self.register(id, Res::Ignored)?;
                    }
                    self.warnings.push(format!("materials-extension <{name}> resources are not read"));
                    Ctx::Skip
                }
            },
            Ctx::Bases(g) if core && name == "base" => {
                self.budget.property()?;
                let c = el.attr("displaycolor").ok_or_else(|| malformed("<base> has no displaycolor"))?;
                let c = parse_color(c).ok_or_else(|| malformed(format!("displaycolor {} is not #RRGGBB[AA]", quoted(c))))?;
                let name = el.attr("name").map(clean_text).unwrap_or_default();
                self.part.bases[g].push((name, c));
                Ctx::Skip
            }
            Ctx::Colors(g) if mat && name == "color" => {
                self.budget.property()?;
                let c = el.attr("color").ok_or_else(|| malformed("<color> has no color"))?;
                let c = parse_color(c).ok_or_else(|| malformed(format!("color {} is not #RRGGBB[AA]", quoted(c))))?;
                self.part.colors[g].push(c);
                Ctx::Skip
            }
            Ctx::TexCoords(g) if mat && name == "tex2coord" => {
                self.budget.property()?;
                let coord = |a: &str| {
                    let v = el.attr(a).ok_or_else(|| malformed(format!("<tex2coord> has no {a}")))?;
                    parse_f32(v).ok_or_else(|| malformed(format!("tex2coord {a} {} is not a finite number", quoted(v))))
                };
                let uv = [coord("u")?, coord("v")?];
                self.part.texcoords[g].1.push(uv);
                Ctx::Skip
            }
            Ctx::Composite(g) if mat && name == "composite" => {
                self.budget.property()?;
                let raw = el.attr("values").ok_or_else(|| malformed("<composite> has no values"))?;
                let values = raw
                    .split_ascii_whitespace()
                    .take(1024)
                    .map(parse_f32)
                    .collect::<Option<Vec<f32>>>()
                    .ok_or_else(|| malformed(format!("composite values {} are not numbers", quoted(raw))))?;
                self.part.composites[g].mixes.push(values);
                Ctx::Skip
            }
            Ctx::Multi(g) if mat && name == "multi" => {
                self.budget.property()?;
                let raw = el.attr("pindices").ok_or_else(|| malformed("<multi> has no pindices"))?;
                let idx = raw
                    .split_ascii_whitespace()
                    .take(1024)
                    .map(parse_index)
                    .collect::<Option<Vec<u32>>>()
                    .ok_or_else(|| malformed(format!("multi pindices {} are not indices", quoted(raw))))?;
                self.part.multis[g].entries.push(idx);
                Ctx::Skip
            }
            Ctx::Object(o) if core => match name {
                // An object is one mesh or one set of components (Core §4).
                // A second of the same kind is out of schema but not out of
                // meaning — more triangles of the object, indexing their own
                // vertices, or more parts — and assimp 6 writes one <mesh> per
                // primitive, so it is read, with a warning. A mesh beside
                // components has no single meaning and is refused.
                "mesh" | "components" => {
                    let obj = &mut self.part.objects[o];
                    let id = obj.id;
                    match (&obj.shape, name) {
                        (Shape::Empty, "mesh") => obj.shape = Shape::Mesh { vertices: Vec::new(), triangles: Vec::new() },
                        (Shape::Empty, _) => obj.shape = Shape::Components(Vec::new()),
                        (Shape::Mesh { .. }, "mesh") | (Shape::Components(_), "components") => {
                            self.warnings.push(format!("object {id} has more than one <{name}>; all were read as one object"));
                        }
                        _ => return Err(malformed(format!("object {id} has both a <mesh> and <components>"))),
                    }
                    if let Shape::Mesh { vertices, .. } = &self.part.objects[o].shape {
                        self.vertex_base = vertices.len() as u32;
                        Ctx::Mesh(o)
                    } else {
                        Ctx::Components(o)
                    }
                }
                "metadatagroup" => Ctx::Skip,
                _ => return Err(Self::unexpected(el, parent)),
            },
            Ctx::Mesh(o) if core => match name {
                "vertices" => Ctx::Vertices(o),
                "triangles" => Ctx::Triangles(o),
                _ => return Err(Self::unexpected(el, parent)),
            },
            Ctx::Mesh(o) if el.ns.as_deref() == Some(NS_BEAM_LATTICE) => {
                let id = self.part.objects[o].id;
                self.warnings.push(format!("object {id} has a beam lattice, which is not read; its triangle mesh is"));
                Ctx::Skip
            }
            Ctx::Vertices(o) if core && name == "vertex" => {
                self.budget.vertex()?;
                let mut v = [0f32; 3];
                for (k, a) in ["x", "y", "z"].iter().enumerate() {
                    let s = el.attr(a).ok_or_else(|| malformed(format!("a vertex of object {} has no {a}", self.part.objects[o].id)))?;
                    v[k] = parse_f32(s).ok_or_else(|| malformed(format!("vertex {a} {} is not a finite number", quoted(s))))?;
                }
                if let Shape::Mesh { vertices, .. } = &mut self.part.objects[o].shape {
                    vertices.push(v);
                }
                Ctx::Skip
            }
            Ctx::Triangles(o) if core && name == "triangle" => {
                self.budget.triangle()?;
                let mut v = [0u32; 3];
                for (k, a) in ["v1", "v2", "v3"].iter().enumerate() {
                    let s = el.attr(a).ok_or_else(|| malformed(format!("a triangle of object {} has no {a}", self.part.objects[o].id)))?;
                    v[k] = parse_index(s)
                        .and_then(|i| i.checked_add(self.vertex_base))
                        .ok_or_else(|| malformed(format!("triangle {a} {} is not an index", quoted(s))))?;
                }
                let t = Tri {
                    v,
                    pid: Self::opt_index(el, "pid")?,
                    p: [Self::opt_index(el, "p1")?, Self::opt_index(el, "p2")?, Self::opt_index(el, "p3")?],
                };
                if let Shape::Mesh { triangles, .. } = &mut self.part.objects[o].shape {
                    triangles.push(t);
                }
                Ctx::Skip
            }
            Ctx::Components(o) if core && name == "component" => {
                let c = Comp { objectid: self.id_attr(el, "objectid")?, path: Self::path_attr(el), transform: Self::transform_attr(el)? };
                if let Shape::Components(list) = &mut self.part.objects[o].shape {
                    if list.len() >= MAX_NODES {
                        return Err(unsupported(format!("object {} has more than {MAX_NODES} components", self.part.objects[o].id)));
                    }
                    list.push(c);
                }
                Ctx::Skip
            }
            Ctx::Build if core && name == "item" => {
                if self.part.items.len() >= MAX_NODES {
                    return Err(unsupported(format!("the build has more than {MAX_NODES} items")));
                }
                let it = Item { objectid: self.id_attr(el, "objectid")?, path: Self::path_attr(el), transform: Self::transform_attr(el)? };
                self.part.items.push(it);
                Ctx::Skip
            }
            // A core element where the schema puts none is a broken file; an
            // element from any other namespace is an extension, ignored.
            _ if core => return Err(Self::unexpected(el, parent)),
            _ => {
                if el.ns.as_deref() == Some(NS_SLICE) {
                    self.warnings.push("slice stacks are not read; meshes are".to_string());
                }
                Ctx::Skip
            }
        };
        self.stack.push(ctx);
        Ok(())
    }

    fn end(&mut self) {
        if let Some(Ctx::Title) = self.stack.pop() {
            if let Some(t) = self.title.take() {
                self.part.title = Some(t);
            }
        }
    }

    fn text(&mut self, t: &str) {
        if self.stack.last() == Some(&Ctx::Title) {
            let title = self.title.get_or_insert_with(String::new);
            if title.chars().count() < MAX_TEXT {
                title.push_str(&clean_text(t));
            }
        }
    }
}

/// "DTD content MUST NOT be used ... consumers SHOULD treat the presence of
/// DTD content as an error" (Core §2.3.2). The XML layer never expands a DTD,
/// so this is conformance, not safety: a DOCTYPE can stand only in the prolog
/// — after a byte-order mark, the declaration, comments, processing
/// instructions and whitespace — and is looked for exactly there.
fn refuse_dtd(name: &str, bytes: &[u8]) -> Result<(), MeshError> {
    let find = |b: &[u8], pat: &[u8]| b.windows(pat.len()).position(|w| w == pat);
    let mut b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    loop {
        let start = b.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(b.len());
        b = &b[start..];
        let skip = if b.starts_with(b"<?") {
            find(b, b"?>").map(|e| e + 2)
        } else if b.starts_with(b"<!--") {
            find(b, b"-->").map(|e| e + 3)
        } else if b.starts_with(b"<!DOCTYPE") {
            return Err(malformed(format!("{name} declares a DTD, which 3MF forbids")));
        } else {
            return Ok(());
        };
        match skip {
            Some(n) => b = &b[n..],
            None => return Ok(()),
        }
    }
}

fn parse_part(name: &str, bytes: &[u8], budget: &mut Budget, warnings: &mut Warnings) -> Result<Part, MeshError> {
    refuse_dtd(name, bytes)?;
    let part = Part {
        name: name.to_string(),
        unit: 1e-3,
        title: None,
        ids: HashMap::new(),
        objects: Vec::new(),
        bases: Vec::new(),
        colors: Vec::new(),
        textures: Vec::new(),
        texcoords: Vec::new(),
        composites: Vec::new(),
        multis: Vec::new(),
        items: Vec::new(),
        saw_resources: false,
        saw_build: false,
        texture_rels: None,
    };
    let mut p = PartParser { part, stack: Vec::new(), seen_root: false, title: None, vertex_base: 0, budget, warnings };
    xml::stream(bytes, |ev| {
        match ev {
            XmlEvent::Start(el) => p.start(el)?,
            XmlEvent::End(_) => p.end(),
            XmlEvent::Text(t) => p.text(t),
        }
        Ok(true)
    })?;
    if !p.seen_root {
        return Err(malformed(format!("{name} has no root element")));
    }
    Ok(p.part)
}

// ─── reader: scene ──────────────────────────────────────────────────────────

/// One triangle's resolved appearance: the group it names (an index into the
/// part's list of that kind) and, for groups that interpolate, the entry at
/// each corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Prop {
    Plain,
    Base { group: usize, index: u32 },
    Colors { group: usize, idx: [u32; 3] },
    Coords { group: usize, idx: [u32; 3] },
    Composite { group: usize, idx: [u32; 3] },
}

/// The primitive a triangle lands in: one per base material, one per
/// interpolating group, one for triangles with no properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PrimKey {
    Plain,
    Base(usize, u32),
    Colors(usize),
    Coords(usize),
    Composite(usize),
}

struct PrimBuild {
    key: PrimKey,
    prim: Primitive,
    corner: HashMap<(u32, u32), u32>,
}

/// An object in a part: `(part, object)`.
type ObjRef = (usize, usize);
/// An object placed by a reference: `(part, object, transform)`.
type Placed = (usize, usize, Mat4);

struct Builder<'a> {
    pkg: Pkg<'a>,
    parts: Vec<Part>,
    part_of: HashMap<String, usize>,
    budget: Budget,
    warnings: Warnings,
    scene: Scene,
    meshes: HashMap<ObjRef, Option<usize>>,
    children: HashMap<ObjRef, Vec<Placed>>,
    base_materials: HashMap<(usize, usize, u32), usize>,
    tex_materials: HashMap<(usize, usize), Option<usize>>,
    images: HashMap<String, Option<usize>>,
    composite_colors: HashMap<(usize, usize), Vec<[f32; 4]>>,
    color_material: Option<usize>,
    image_bytes: usize,
    ir_vertices: usize,
    ir_indices: usize,
}

impl<'a> Builder<'a> {
    fn load_part(&mut self, name: &str) -> Result<usize, MeshError> {
        let key = name.to_ascii_lowercase();
        if let Some(&i) = self.part_of.get(&key) {
            return Ok(i);
        }
        if self.parts.len() >= MAX_MODEL_PARTS {
            return Err(unsupported(format!("the package references more than {MAX_MODEL_PARTS} model parts")));
        }
        let bytes = self.pkg.read(name)?.ok_or_else(|| malformed(format!("the package has no part {}", quoted(name))))?;
        let part = parse_part(name, &bytes, &mut self.budget, &mut self.warnings)?;
        self.parts.push(part);
        let i = self.parts.len() - 1;
        self.part_of.insert(key, i);
        Ok(i)
    }

    /// The object a reference names: in the referring part, or — through the
    /// production extension's `p:path` — in another model part.
    fn resolve(&mut self, from: usize, path: Option<&str>, objectid: u32) -> Result<(usize, usize), MeshError> {
        let part = match path {
            None => from,
            Some(p) => {
                let name = part_name(p, &self.parts[from].name)
                    .ok_or_else(|| malformed(format!("p:path {} is not a part of this package", quoted(p))))?;
                self.load_part(&name)?
            }
        };
        match self.parts[part].ids.get(&objectid) {
            Some(Res::Object(o)) => Ok((part, *o)),
            Some(_) => Err(malformed(format!("resource {objectid} in {} is not an object", self.parts[part].name))),
            None => Err(malformed(format!("{} has no object with id {objectid}", self.parts[part].name))),
        }
    }

    /// Whether an object may be drawn, or why not.
    fn drawable(&mut self, part: usize, obj: usize) -> Result<bool, MeshError> {
        let o = &self.parts[part].objects[obj];
        match o.kind {
            ObjKind::Model | ObjKind::Surface => Ok(true),
            // Build items "MUST NOT reference objects of type other, either
            // directly or recursively" (Core §3.4.3.1).
            ObjKind::Other => Err(malformed(format!("the build references object {} of type \"other\"", o.id))),
            ObjKind::Support | ObjKind::SolidSupport => {
                let id = o.id;
                self.warnings.push(format!("object {id} is a print support, not part of the model; it was skipped"));
                Ok(false)
            }
        }
    }

    /// An object's components, resolved, with transforms carrying any change
    /// of unit between parts. Cached: an object's components are resolved once
    /// however many times it is instanced.
    fn children_of(&mut self, part: usize, obj: usize) -> Result<Vec<Placed>, MeshError> {
        if let Some(c) = self.children.get(&(part, obj)) {
            return Ok(c.clone());
        }
        let comps: Vec<(u32, Option<String>, Mat4)> = match &self.parts[part].objects[obj].shape {
            Shape::Components(list) => list.iter().map(|c| (c.objectid, c.path.clone(), c.transform)).collect(),
            _ => Vec::new(),
        };
        let mut out = Vec::with_capacity(comps.len());
        for (objectid, path, transform) in comps {
            let (p, o) = self.resolve(part, path.as_deref(), objectid)?;
            if !self.drawable(p, o)? {
                continue;
            }
            let ratio = self.parts[p].unit / self.parts[part].unit;
            let t = if ratio == 1.0 { transform } else { math::mat_mul(&transform, &math::scaling([ratio; 3])) };
            out.push((p, o, t));
        }
        self.children.insert((part, obj), out.clone());
        Ok(out)
    }

    /// Refuse component cycles before expanding anything: depth-first over
    /// objects (not instances), with an explicit stack.
    fn check_acyclic(&mut self, roots: &[Placed]) -> Result<(), MeshError> {
        // 1: on the current path; 2: finished. Each frame holds its object's
        // children, fetched once: re-fetching them per step would make an
        // object with n components cost n² here.
        let mut state: HashMap<ObjRef, u8> = HashMap::new();
        for &(p, o, _) in roots {
            if state.contains_key(&(p, o)) {
                continue;
            }
            state.insert((p, o), 1);
            let kids = self.children_of(p, o)?;
            let mut stack: Vec<(ObjRef, Vec<Placed>, usize)> = vec![((p, o), kids, 0)];
            while let Some((key, kids, next)) = stack.last_mut() {
                let key = *key;
                match kids.get(*next).copied() {
                    Some((cp, co, _)) => {
                        *next += 1;
                        match state.get(&(cp, co)) {
                            Some(1) => {
                                return Err(malformed(format!(
                                    "components form a cycle through object {}",
                                    self.parts[cp].objects[co].id
                                )))
                            }
                            Some(_) => {}
                            None => {
                                if stack.len() >= MAX_COMPONENT_DEPTH {
                                    return Err(unsupported(format!(
                                        "components nest more than {MAX_COMPONENT_DEPTH} levels deep"
                                    )));
                                }
                                state.insert((cp, co), 1);
                                let grandkids = self.children_of(cp, co)?;
                                stack.push(((cp, co), grandkids, 0));
                            }
                        }
                    }
                    None => {
                        state.insert(key, 2);
                        stack.pop();
                    }
                }
            }
        }
        Ok(())
    }

    fn build(&mut self, root: usize) -> Result<(), MeshError> {
        if !self.parts[root].saw_resources {
            return Err(malformed("the model has no <resources>"));
        }
        if !self.parts[root].saw_build {
            return Err(malformed("the model has no <build>"));
        }
        self.scene.name = self.parts[root].title.clone().filter(|t| !t.trim().is_empty());
        let unit = self.parts[root].unit;
        let items: Vec<(u32, Option<String>, Mat4)> =
            self.parts[root].items.iter().map(|i| (i.objectid, i.path.clone(), i.transform)).collect();
        let mut roots = Vec::with_capacity(items.len());
        for (objectid, path, transform) in items {
            let (p, o) = self.resolve(root, path.as_deref(), objectid)?;
            if !self.drawable(p, o)? {
                continue;
            }
            // Z-up file units to the scene's Y-up metres, at the root only:
            // everything below is relative to it.
            let ratio = self.parts[p].unit / unit;
            let local = if ratio == 1.0 { transform } else { math::mat_mul(&transform, &math::scaling([ratio; 3])) };
            let to_scene = if unit == 1.0 { Z_UP_TO_Y_UP } else { math::mat_mul(&Z_UP_TO_Y_UP, &math::scaling([unit; 3])) };
            roots.push((p, o, math::mat_mul(&to_scene, &local)));
        }
        self.check_acyclic(&roots)?;

        // Expand instances depth-first with an explicit stack; children are
        // pushed in reverse so they come out (and are attached) in order.
        let mut stack: Vec<(usize, usize, Mat4, Option<usize>, usize)> =
            roots.into_iter().rev().map(|(p, o, t)| (p, o, t, None, 1)).collect();
        while let Some((p, o, t, parent, depth)) = stack.pop() {
            if depth > MAX_COMPONENT_DEPTH {
                return Err(unsupported(format!("components nest more than {MAX_COMPONENT_DEPTH} levels deep")));
            }
            if self.scene.nodes.len() >= MAX_NODES {
                return Err(unsupported(format!(
                    "the components expand to more than {MAX_NODES} placed parts"
                )));
            }
            let mesh = match self.parts[p].objects[o].shape {
                Shape::Mesh { .. } => self.mesh_for(p, o)?,
                Shape::Components(_) => None,
                Shape::Empty => {
                    let id = self.parts[p].objects[o].id;
                    self.warnings.push(format!("object {id} has neither a mesh nor components"));
                    None
                }
            };
            let node = Node {
                name: self.parts[p].objects[o].name.clone(),
                transform: Transform::from_matrix(t),
                mesh,
                ..Default::default()
            };
            self.scene.nodes.push(node);
            let me = self.scene.nodes.len() - 1;
            match parent {
                Some(q) => self.scene.nodes[q].children.push(me),
                None => self.scene.roots.push(me),
            }
            let kids = self.children_of(p, o)?;
            for (cp, co, ct) in kids.into_iter().rev() {
                stack.push((cp, co, ct, Some(me), depth + 1));
            }
        }
        Ok(())
    }

    /// Resolve a triangle's property fields against the object's defaults
    /// (Core §4.1.4.1): `pid` falls back to the object's, `p1` to its
    /// `pindex`, and a missing `p2` or `p3` means `p1` for the whole triangle.
    fn property(&mut self, part: usize, obj: usize, t: &Tri) -> Result<Prop, MeshError> {
        let o = &self.parts[part].objects[obj];
        let Some(pid) = t.pid.or(o.pid) else { return Ok(Prop::Plain) };
        let p1 = t.p[0].or(o.pindex).ok_or_else(|| {
            malformed(format!("a triangle of object {} names property group {pid} but no index into it", o.id))
        })?;
        let idx = match (t.p[1], t.p[2]) {
            (Some(p2), Some(p3)) => [p1, p2, p3],
            _ => [p1; 3],
        };
        let res = self.parts[part].ids.get(&pid).copied();
        let oid = o.id;
        self.group_property(part, oid, pid, res, idx, true)
    }

    fn group_property(
        &mut self,
        part: usize,
        oid: u32,
        pid: u32,
        res: Option<Res>,
        idx: [u32; 3],
        allow_multi: bool,
    ) -> Result<Prop, MeshError> {
        let pt = &self.parts[part];
        let check = |len: usize, what: &str| -> Result<(), MeshError> {
            match idx.iter().find(|i| **i as usize >= len) {
                Some(bad) => Err(malformed(format!(
                    "a triangle of object {oid} uses entry {bad} of {what} {pid}, which has {len}"
                ))),
                None => Ok(()),
            }
        };
        Ok(match res {
            // Base materials cannot form gradients: p1 for the whole triangle.
            Some(Res::Bases(g)) => {
                check(pt.bases[g].len(), "basematerials")?;
                Prop::Base { group: g, index: idx[0] }
            }
            Some(Res::Colors(g)) => {
                check(pt.colors[g].len(), "colorgroup")?;
                Prop::Colors { group: g, idx }
            }
            Some(Res::TexCoords(g)) => {
                check(pt.texcoords[g].1.len(), "texture2dgroup")?;
                Prop::Coords { group: g, idx }
            }
            Some(Res::Composite(g)) => {
                check(pt.composites[g].mixes.len(), "compositematerials")?;
                Prop::Composite { group: g, idx }
            }
            Some(Res::Multi(g)) if allow_multi => {
                check(pt.multis[g].entries.len(), "multiproperties")?;
                self.multi_property(part, oid, pid, g, idx)?
            }
            _ => {
                return Err(malformed(format!(
                    "a triangle of object {oid} names {pid} as its property group, which is not one"
                )))
            }
        })
    }

    /// A multi-property stack carried as ONE of its layers: the last colour
    /// layer (a colour or texture group) when there is one — that is what the
    /// layers above a material are for — else the first. Blending the layers
    /// would need a texture times colour the scene cannot express per corner.
    fn multi_property(&mut self, part: usize, oid: u32, pid: u32, g: usize, idx: [u32; 3]) -> Result<Prop, MeshError> {
        let pids = self.parts[part].multis[g].pids.clone();
        let kinds: Vec<Option<Res>> = pids.iter().map(|p| self.parts[part].ids.get(p).copied()).collect();
        let layer = kinds
            .iter()
            .rposition(|k| matches!(k, Some(Res::Colors(_)) | Some(Res::TexCoords(_))))
            .unwrap_or(0);
        if pids.len() > 1 {
            self.warnings.push(format!(
                "multiproperties {pid} layers {} property groups; only group {} is carried, the layers are not blended",
                pids.len(),
                pids[layer]
            ));
        }
        let entries = &self.parts[part].multis[g].entries;
        // A short pindices list defaults to index 0 (Materials §5.1).
        let corner = idx.map(|i| entries[i as usize].get(layer).copied().unwrap_or(0));
        self.group_property(part, oid, pids[layer], kinds[layer], corner, false)
    }

    /// The display colours a composite group's entries mix to, linear RGBA:
    /// "the displaycolor of the basematerials ... SHOULD be mixed in the same
    /// proportions" (Materials §4). The colour space of that mix is not
    /// specified; it is done in linear light, as the spec's blending is.
    fn composite_colors(&mut self, part: usize, g: usize) -> Result<Vec<[f32; 4]>, MeshError> {
        if let Some(c) = self.composite_colors.get(&(part, g)) {
            return Ok(c.clone());
        }
        let pt = &self.parts[part];
        let comp = &pt.composites[g];
        let bases = match pt.ids.get(&comp.matid) {
            Some(Res::Bases(b)) => &pt.bases[*b],
            _ => return Err(malformed(format!("compositematerials matid {} is not a basematerials group", comp.matid))),
        };
        let mut constituents = Vec::with_capacity(comp.matindices.len());
        for &m in &comp.matindices {
            let (_, c) = bases.get(m as usize).ok_or_else(|| {
                malformed(format!("compositematerials names base {m} of a group with {}", bases.len()))
            })?;
            constituents.push([decode_srgb8(c[0]), decode_srgb8(c[1]), decode_srgb8(c[2]), c[3] as f32 / 255.0]);
        }
        let colors: Vec<[f32; 4]> = comp
            .mixes
            .iter()
            .map(|values| {
                // Missing values are zero, extra ones ignored, and an all-zero
                // mix is an even one (Materials §4.1).
                let w: Vec<f32> = (0..constituents.len()).map(|i| values.get(i).copied().unwrap_or(0.0).max(0.0)).collect();
                let sum: f32 = w.iter().sum();
                let mut out = [0f32; 4];
                for (i, c) in constituents.iter().enumerate() {
                    let wi = if sum > 0.0 { w[i] / sum } else { 1.0 / constituents.len() as f32 };
                    for k in 0..4 {
                        out[k] += wi * c[k];
                    }
                }
                out
            })
            .collect();
        self.warnings.push("composite materials are carried as the mix of their base display colours".to_string());
        self.composite_colors.insert((part, g), colors.clone());
        Ok(colors)
    }

    fn base_material(&mut self, part: usize, group: usize, index: u32) -> usize {
        if let Some(&m) = self.base_materials.get(&(part, group, index)) {
            return m;
        }
        let (name, c) = self.parts[part].bases[group][index as usize].clone();
        self.scene.materials.push(Material {
            name: (!name.is_empty()).then_some(name),
            base_color: [decode_srgb8(c[0]), decode_srgb8(c[1]), decode_srgb8(c[2]), c[3] as f32 / 255.0],
            metallic: 0.0,
            roughness: 1.0,
            ..Default::default()
        });
        let m = self.scene.materials.len() - 1;
        self.base_materials.insert((part, group, index), m);
        m
    }

    /// The material for a texture coordinate group's texture, or `None` when
    /// its image is missing or not an image 3MF allows.
    fn texture_material(&mut self, part: usize, coords: usize) -> Result<Option<usize>, MeshError> {
        let texid = self.parts[part].texcoords[coords].0;
        let tex = match self.parts[part].ids.get(&texid) {
            Some(Res::Texture(t)) => *t,
            _ => return Err(malformed(format!("texture2dgroup texid {texid} is not a texture2d"))),
        };
        if let Some(&m) = self.tex_materials.get(&(part, tex)) {
            return Ok(m);
        }
        let t = &self.parts[part].textures[tex];
        let (path, sampler) = (t.path.clone(), filter_sampler(t.filter, t.wrap_u, t.wrap_v));
        let name = part_name(&path, &self.parts[part].name)
            .ok_or_else(|| malformed(format!("texture path {} is not a part of this package", quoted(&path))))?;
        // The path is how a texture is found; the relationship is a packaging
        // duty on top ("MUST also be the target of a 3D Texture relationship",
        // Materials §6). A missing one does not change which image is meant.
        if !self.texture_rels(part)?.contains(&name.to_ascii_lowercase()) {
            self.warnings.push(format!(
                "texture {} is not the target of a 3D Texture relationship from {}; it was read by its path",
                quoted(&name),
                self.parts[part].name
            ));
        }
        let image = self.image(&name)?;
        let m = image.map(|image| {
            self.scene.textures.push(Texture { name: None, image, sampler });
            let texture = self.scene.textures.len() - 1;
            self.scene.materials.push(textured_material(texture));
            self.scene.materials.len() - 1
        });
        self.tex_materials.insert((part, tex), m);
        Ok(m)
    }

    /// The part's 3D Texture relationship targets, from `<dir>/_rels/<file>.rels`.
    fn texture_rels(&mut self, part: usize) -> Result<&[String], MeshError> {
        if self.parts[part].texture_rels.is_none() {
            let source = self.parts[part].name.clone();
            let (dir, file) = source.rsplit_once('/').unwrap_or(("", source.as_str()));
            let rels_name = if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
            let mut targets = Vec::new();
            if let Some(bytes) = self.pkg.read(&rels_name)? {
                let doc = xml::parse(&bytes)?;
                for r in doc.children.iter().filter(|r| r.is(NS_RELATIONSHIPS, "Relationship")) {
                    if r.attr("Type").map(str::trim) == Some(REL_TEXTURE) {
                        if let Some(t) = r.attr("Target").and_then(|t| part_name(t, &source)) {
                            targets.push(t.to_ascii_lowercase());
                        }
                    }
                }
            }
            self.parts[part].texture_rels = Some(targets);
        }
        Ok(self.parts[part].texture_rels.as_deref().unwrap_or(&[]))
    }

    fn image(&mut self, name: &str) -> Result<Option<usize>, MeshError> {
        let key = name.to_ascii_lowercase();
        if let Some(&i) = self.images.get(&key) {
            return Ok(i);
        }
        let out = match self.pkg.read(name)? {
            None => {
                self.warnings.push(format!("texture part {} is missing; its triangles are untextured", quoted(name)));
                None
            }
            Some(data) => {
                self.image_bytes = self.image_bytes.saturating_add(data.len());
                if self.image_bytes > MAX_IMAGE_BYTES {
                    return Err(unsupported(format!("the package's textures exceed {MAX_IMAGE_BYTES} bytes")));
                }
                match ImageFormat::sniff(&data).filter(|f| f.is_universal()) {
                    Some(format) => {
                        let stem = name.rsplit('/').next().unwrap_or(name);
                        let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
                        self.scene.images.push(Image { name: Some(stem.to_string()), format, data });
                        Some(self.scene.images.len() - 1)
                    }
                    None => {
                        self.warnings.push(format!(
                            "texture part {} is not a PNG or JPEG; its triangles are untextured",
                            quoted(name)
                        ));
                        None
                    }
                }
            }
        };
        self.images.insert(key, out);
        Ok(out)
    }

    /// One mesh object as a scene mesh: a primitive per base material and per
    /// interpolating group, vertices split wherever a corner's colour or
    /// coordinate differs from another use of the same vertex.
    fn mesh_for(&mut self, part: usize, obj: usize) -> Result<Option<usize>, MeshError> {
        if let Some(&m) = self.meshes.get(&(part, obj)) {
            return Ok(m);
        }
        let shape = std::mem::replace(&mut self.parts[part].objects[obj].shape, Shape::Empty);
        let result = self.mesh_from_shape(part, obj, &shape);
        self.parts[part].objects[obj].shape = shape;
        let mesh = result?;
        self.meshes.insert((part, obj), mesh);
        Ok(mesh)
    }

    fn mesh_from_shape(&mut self, part: usize, obj: usize, shape: &Shape) -> Result<Option<usize>, MeshError> {
        let Shape::Mesh { vertices, triangles } = shape else { return Ok(None) };
        let oid = self.parts[part].objects[obj].id;
        let mut prims: Vec<PrimBuild> = Vec::new();
        let mut slot: HashMap<PrimKey, usize> = HashMap::new();
        let mut repeated = 0usize;
        for t in triangles {
            if let Some(bad) = t.v.iter().find(|v| **v as usize >= vertices.len()) {
                return Err(malformed(format!(
                    "a triangle of object {oid} uses vertex {bad} of {}",
                    vertices.len()
                )));
            }
            if t.v[0] == t.v[1] || t.v[1] == t.v[2] || t.v[0] == t.v[2] {
                repeated += 1;
                continue;
            }
            let prop = self.property(part, obj, t)?;
            let key = match prop {
                Prop::Plain => PrimKey::Plain,
                Prop::Base { group, index } => PrimKey::Base(group, index),
                Prop::Colors { group, .. } => PrimKey::Colors(group),
                Prop::Coords { group, .. } => PrimKey::Coords(group),
                Prop::Composite { group, .. } => PrimKey::Composite(group),
            };
            let k = match slot.get(&key) {
                Some(&k) => k,
                None => {
                    let mut prim = Primitive::default();
                    match key {
                        PrimKey::Base(g, i) => prim.material = Some(self.base_material(part, g, i)),
                        PrimKey::Coords(g) => {
                            prim.material = self.texture_material(part, g)?;
                            prim.uvs = vec![Vec::new()];
                        }
                        PrimKey::Colors(_) | PrimKey::Composite(_) => {
                            prim.colors = Some(Vec::new());
                            prim.material = Some(*self.color_material.get_or_insert_with(|| {
                                self.scene.materials.push(color_material());
                                self.scene.materials.len() - 1
                            }));
                        }
                        PrimKey::Plain => {}
                    }
                    prims.push(PrimBuild { key, prim, corner: HashMap::new() });
                    slot.insert(key, prims.len() - 1);
                    prims.len() - 1
                }
            };
            let composite = match prop {
                Prop::Composite { group, .. } => Some(self.composite_colors(part, group)?),
                _ => None,
            };
            for c in 0..3 {
                let v = t.v[c];
                let entry = match prop {
                    Prop::Colors { idx, .. } | Prop::Coords { idx, .. } | Prop::Composite { idx, .. } => idx[c],
                    Prop::Plain | Prop::Base { .. } => u32::MAX,
                };
                let pb = &mut prims[k];
                let i = match pb.corner.get(&(v, entry)) {
                    Some(&i) => i,
                    None => {
                        self.ir_vertices += 1;
                        if self.ir_vertices > MAX_SCENE_VERTICES {
                            return Err(unsupported(format!(
                                "splitting vertices by colour and texture coordinate passes {MAX_SCENE_VERTICES}"
                            )));
                        }
                        let i = pb.prim.positions.len() as u32;
                        pb.prim.positions.push(vertices[v as usize]);
                        match (pb.key, &composite) {
                            (PrimKey::Colors(g), _) => {
                                let col = self.parts[part].colors[g][entry as usize];
                                // Colour groups are opaque outside a
                                // multi-property layer (Materials §2.1).
                                let rgba = [decode_srgb8(col[0]), decode_srgb8(col[1]), decode_srgb8(col[2]), 1.0];
                                if let Some(cs) = pb.prim.colors.as_mut() {
                                    cs.push(rgba);
                                }
                            }
                            (PrimKey::Composite(_), Some(mix)) => {
                                if let Some(cs) = pb.prim.colors.as_mut() {
                                    cs.push(mix[entry as usize]);
                                }
                            }
                            (PrimKey::Coords(g), _) => {
                                let uv = self.parts[part].texcoords[g].1[entry as usize];
                                // 3MF's origin is the lower-left of the image,
                                // the scene's the upper-left.
                                if let Some(set) = pb.prim.uvs.first_mut() {
                                    set.push([uv[0], 1.0 - uv[1]]);
                                }
                            }
                            _ => {}
                        }
                        pb.corner.insert((v, entry), i);
                        i
                    }
                };
                pb.prim.indices.push(i);
            }
            self.ir_indices += 3;
            if self.ir_indices > MAX_SCENE_INDICES {
                return Err(unsupported(format!("the model passes {MAX_SCENE_INDICES} indices")));
            }
        }
        if repeated > 0 {
            self.warnings.push(format!(
                "object {oid} has {repeated} triangle(s) that repeat a vertex; they enclose nothing and were skipped"
            ));
        }
        let primitives: Vec<Primitive> = prims.into_iter().map(|p| p.prim).filter(|p| !p.indices.is_empty()).collect();
        if primitives.is_empty() {
            self.warnings.push(format!("object {oid} has no triangles"));
            return Ok(None);
        }
        let name = self.parts[part].objects[obj].name.clone();
        self.scene.meshes.push(SceneMesh { name, primitives, ..Default::default() });
        Ok(Some(self.scene.meshes.len() - 1))
    }
}

/// Read a 3MF package into a scene.
///
/// 3MF is self-contained — "3MF Documents MUST NOT reference resources
/// external to the 3MF Document package" (Core §2.1.1) — so `sidecars` is
/// never consulted.
pub fn read_scene(bytes: &[u8], sidecars: &dyn Sidecars) -> Result<SceneRead, MeshError> {
    let _ = sidecars;
    if !zip::looks_like_zip(bytes) {
        return Err(not_3mf("no ZIP local file header at the start; a 3MF document is a ZIP package"));
    }
    let pkg = Pkg::new(Archive::parse(bytes)?)?;
    let root = start_part(&pkg)?;
    let mut b = Builder {
        pkg,
        parts: Vec::new(),
        part_of: HashMap::new(),
        budget: Budget::default(),
        warnings: Warnings::default(),
        scene: Scene::default(),
        meshes: HashMap::new(),
        children: HashMap::new(),
        base_materials: HashMap::new(),
        tex_materials: HashMap::new(),
        images: HashMap::new(),
        composite_colors: HashMap::new(),
        color_material: None,
        image_bytes: 0,
        ir_vertices: 0,
        ir_indices: 0,
    };
    let root_part = b.load_part(&root)?;
    b.build(root_part)?;
    b.scene.validate()?;
    Ok(SceneRead { scene: b.scene, warnings: b.warnings.into_vec() })
}

/// Read a 3MF into the flat mesh: every placed part in world space, merged.
pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    let r = read_scene(bytes, &NoSidecars)?;
    let mesh = pose::flatten(&r.scene)?;
    if mesh.indices.is_empty() {
        return Err(MeshError::Empty);
    }
    Ok(mesh)
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::fixtures;
    use super::super::scene::{AlphaMode, UvTransform};
    use super::*;

    // ── builders ────────────────────────────────────────────────────────────

    const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/><Default Extension="png" ContentType="image/png"/></Types>"#;
    const PACKAGE_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>"#;

    /// A closed unit tetrahedron, counter-clockwise outward, as hand-written
    /// 3MF markup in the core namespace (default prefix).
    const TETRA: &str = r#"<mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/><vertex x="0" y="0" z="1"/></vertices><triangles><triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/><triangle v1="0" v2="3" v3="2"/><triangle v1="1" v2="2" v3="3"/></triangles></mesh>"#;

    fn zip_of(entries: &[(&str, &[u8])], how: Store) -> Vec<u8> {
        let mut z = zip::Writer::new();
        for (name, data) in entries {
            z.add(name, data, how).unwrap();
        }
        z.finish()
    }

    /// A conforming package around a hand-written model part, plus extra parts.
    fn package_with(model: &str, extra: &[(&str, &[u8])]) -> Vec<u8> {
        let mut entries: Vec<(&str, &[u8])> = vec![
            ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
            ("_rels/.rels", PACKAGE_RELS.as_bytes()),
            ("3D/3dmodel.model", model.as_bytes()),
        ];
        entries.extend_from_slice(extra);
        zip_of(&entries, Store::Deflated)
    }

    fn package(model: &str) -> Vec<u8> {
        package_with(model, &[])
    }

    /// `<model>` in millimetres (the default) with the materials namespace
    /// bound to `m`, around resources and build markup.
    fn model(resources: &str, build: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><model unit="millimeter" xml:lang="en-US" xmlns="{NS_CORE}" xmlns:m="{NS_MATERIAL}"><resources>{resources}</resources><build>{build}</build></model>"#
        )
    }

    fn read_ok(bytes: &[u8]) -> SceneRead {
        read_scene(bytes, &NoSidecars).unwrap_or_else(|e| panic!("read failed: {e}"))
    }

    fn read_err(bytes: &[u8]) -> MeshError {
        match read_scene(bytes, &NoSidecars) {
            Ok(r) => panic!("expected an error, read {} triangles", r.scene.triangle_count()),
            Err(e) => e,
        }
    }

    fn malformed_text(e: MeshError) -> String {
        match e {
            MeshError::Malformed(d) => d,
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    fn unsupported_text(e: MeshError) -> String {
        match e {
            MeshError::Unsupported(d) => d,
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    fn written(scene: &Scene) -> Vec<u8> {
        write_scene(scene).unwrap_or_else(|e| panic!("write failed: {e}")).files.remove(0).bytes
    }

    fn part_text(bytes: &[u8], name: &str) -> String {
        let a = Archive::parse(bytes).unwrap();
        String::from_utf8(a.read_name(name).unwrap().unwrap_or_else(|| panic!("no part {name}"))).unwrap()
    }

    fn model_dom(bytes: &[u8]) -> Element {
        xml::parse(part_text(bytes, MODEL_PART).as_bytes()).unwrap()
    }

    fn resources(dom: &Element) -> &Element {
        dom.child("resources").unwrap()
    }

    fn one_triangle_scene(material: Option<Material>) -> Scene {
        let mut s = Scene::default();
        let mut p = Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        if let Some(m) = material {
            s.materials.push(m);
            p.material = Some(0);
        }
        s.meshes.push(SceneMesh { name: Some("tri".into()), primitives: vec![p], ..Default::default() });
        s.add_root(Node { mesh: Some(0), ..Default::default() });
        s
    }

    // ── comparing scenes by what they draw ──────────────────────────────────

    /// One triangle corner as a renderer sees it: world position, texture
    /// coordinate, vertex colour, material colour and the texture's bytes.
    #[derive(Debug, Clone)]
    struct Corner {
        pos: [f32; 3],
        uv: Option<[f32; 2]>,
        color: Option<[f32; 4]>,
        base: [f32; 4],
        image: Option<Vec<u8>>,
    }

    fn drawn(s: &Scene) -> Vec<[Corner; 3]> {
        let mut out = Vec::new();
        for wp in pose::world_primitives(s, &pose::default_pose(s)).unwrap() {
            let mat = wp.material.and_then(|m| s.materials.get(m));
            let base = mat.map_or([1.0; 4], |m| m.base_color);
            let image = mat
                .and_then(|m| m.base_color_texture.as_ref())
                .map(|r| s.images[s.textures[r.texture].image].data.clone());
            for t in wp.indices.chunks_exact(3) {
                let c = |i: u32| Corner {
                    pos: wp.positions[i as usize],
                    uv: wp.uvs.first().map(|set| set[i as usize]),
                    color: wp.colors.as_ref().map(|cs| cs[i as usize]),
                    base,
                    image: image.clone(),
                };
                out.push([c(t[0]), c(t[1]), c(t[2])]);
            }
        }
        out
    }

    fn near(a: &[f32], b: &[f32], tol: f32) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
    }

    fn same_corner(a: &Corner, b: &Corner) -> bool {
        near(&a.pos, &b.pos, 1e-5)
            && match (a.uv, b.uv) {
                (None, None) => true,
                (Some(x), Some(y)) => near(&x, &y, 1e-6),
                _ => false,
            }
            && match (a.color, b.color) {
                (None, None) => true,
                (Some(x), Some(y)) => near(&x, &y, 1e-6),
                _ => false,
            }
            && near(&a.base, &b.base, 1e-6)
            && a.image == b.image
    }

    /// Every triangle of `want` drawn by `got`, corner for corner in the same
    /// cyclic order (so winding counts), and nothing else.
    fn assert_draws_the_same(want: &Scene, got: &Scene, ctx: &str) {
        let w = drawn(want);
        let mut g = drawn(got);
        assert_eq!(w.len(), g.len(), "{ctx}: triangle count");
        for (i, tw) in w.iter().enumerate() {
            let hit = g.iter().position(|tg| {
                (0..3).any(|r| (0..3).all(|k| same_corner(&tw[k], &tg[(k + r) % 3])))
            });
            match hit {
                Some(j) => {
                    g.swap_remove(j);
                }
                None => panic!("{ctx}: triangle {i} {:?} is not drawn by the read-back scene", [tw[0].pos, tw[1].pos, tw[2].pos]),
            }
        }
    }

    fn signed_volume(s: &Scene) -> f64 {
        let mut v = 0.0;
        for t in drawn(s) {
            let p = |k: usize| t[k].pos.map(|x| x as f64);
            let (a, b, c) = (p(0), p(1), p(2));
            v += (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0;
        }
        v
    }

    // ── round trips ─────────────────────────────────────────────────────────

    #[test]
    fn every_fixture_reads_back_as_exactly_what_prepare_promised() {
        for (name, fixture) in fixtures::all() {
            let want = prepare(&fixture).unwrap();
            let r = read_ok(&written(&fixture));
            assert!(r.warnings.is_empty(), "{name}: our own file warned: {:?}", r.warnings);
            assert_draws_the_same(&want, &r.scene, name);
            assert_eq!(r.scene.name, want.name, "{name}: title");
            assert_eq!(r.scene.textures.len(), want.textures.len(), "{name}: textures");
            for (a, b) in r.scene.textures.iter().zip(&want.textures) {
                assert_eq!(a.sampler, b.sampler, "{name}: sampler");
                assert_eq!(r.scene.images[a.image], want.images[b.image], "{name}: image");
            }
            let names = |s: &Scene| {
                let mut v: Vec<Option<String>> = s.materials.iter().map(|m| m.name.clone()).collect();
                v.sort();
                v
            };
            assert_eq!(names(&r.scene), names(&want), "{name}: material names");
        }
    }

    #[test]
    fn prepare_is_idempotent() {
        for (name, fixture) in fixtures::all() {
            let once = prepare(&fixture).unwrap();
            assert_eq!(prepare(&once).unwrap(), once, "{name}");
        }
    }

    #[test]
    fn prepare_poses_skins_and_morphs_exactly_as_the_shared_evaluator() {
        // The expected picture of every conversion INTO 3MF is rendered from
        // `prepare`, so a wrongly posed skin here would be wrong in both the
        // file and its expectation, and pass. Anchor the pose to the evaluator
        // every other static container uses.
        for (name, fixture) in fixtures::all() {
            let ours = prepare(&fixture).unwrap();
            let shared = pose::bake_default(&fixture).unwrap();
            let a = drawn(&ours);
            let b = drawn(&shared);
            assert_eq!(a.len(), b.len(), "{name}");
            for (x, y) in a.iter().zip(&b) {
                for k in 0..3 {
                    assert!(near(&x[k].pos, &y[k].pos, 1e-5), "{name}: {:?} vs {:?}", x[k].pos, y[k].pos);
                }
            }
        }
    }

    #[test]
    fn instancing_survives_a_round_trip() {
        // The textured fixture draws its box twice, once mirrored: one object,
        // two placements, and the read-back scene shares one mesh between two
        // nodes rather than holding two copies.
        let r = read_ok(&written(&fixtures::textured()));
        let users: Vec<usize> = r.scene.nodes.iter().filter_map(|n| n.mesh).collect();
        assert_eq!(users.len(), 4, "box, wedge, panel, mirrored box");
        assert_eq!(r.scene.meshes.len(), 3, "the mirrored box reuses the box mesh");
        let world = r.scene.world_matrices();
        let mirrored = r.scene.nodes.iter().enumerate().filter(|(i, n)| n.mesh.is_some() && math::is_mirroring(&world[*i])).count();
        assert_eq!(mirrored, 1, "the mirror stays a placement");
        assert!(signed_volume(&r.scene).abs() > 0.0);
    }

    #[test]
    fn writing_is_deterministic() {
        for (name, fixture) in fixtures::all() {
            assert_eq!(written(&fixture), written(&fixture), "{name}");
        }
    }

    // ── package and model structure ─────────────────────────────────────────

    #[test]
    fn writes_the_parts_and_relationships_consumers_look_for() {
        let bytes = written(&fixtures::textured());
        let a = Archive::parse(&bytes).unwrap();
        let names: Vec<&str> = a.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["[Content_Types].xml", "_rels/.rels", "3D/3dmodel.model", "3D/_rels/3dmodel.model.rels", "3D/Textures/quadrants.png"]
        );
        let types = xml::parse(part_text(&bytes, CONTENT_TYPES_PART).as_bytes()).unwrap();
        let ext = |e: &str| {
            types.children.iter().find(|d| d.attr("Extension") == Some(e)).and_then(|d| d.attr("ContentType")).map(str::to_string)
        };
        assert_eq!(ext("rels").as_deref(), Some(CT_RELATIONSHIPS));
        assert_eq!(ext("model").as_deref(), Some(CT_MODEL));
        assert_eq!(ext("png").as_deref(), Some("image/png"));

        let rels = xml::parse(part_text(&bytes, PACKAGE_RELS_PART).as_bytes()).unwrap();
        let start = rels.child("Relationship").unwrap();
        assert_eq!(start.attr("Type"), Some(REL_START_PART));
        assert_eq!(start.attr("Target"), Some("/3D/3dmodel.model"));

        // three.js keys textures by relationship target, then looks them up
        // by texture2d path: the two must be the same string.
        let mrels = xml::parse(part_text(&bytes, MODEL_RELS_PART).as_bytes()).unwrap();
        let tex_rel = mrels.child("Relationship").unwrap();
        assert_eq!(tex_rel.attr("Type"), Some(REL_TEXTURE));
        let dom = model_dom(&bytes);
        let tex = resources(&dom).child("texture2d").unwrap();
        assert_eq!(tex.attr("path"), tex_rel.attr("Target"));
        assert_eq!(tex.attr("contenttype"), Some("image/png"));
        assert_eq!(tex.attr("tilestyleu"), Some("wrap"));
        assert_eq!(tex.attr("filter"), Some("auto"));
        // Stored: PNG does not deflate.
        assert!(a.find("3D/Textures/quadrants.png").unwrap().is_stored());
    }

    #[test]
    fn the_model_is_in_metres_z_up_and_requires_the_materials_it_uses() {
        let dom = model_dom(&written(&fixtures::textured()));
        assert!(dom.is(NS_CORE, "model"));
        assert_eq!(dom.attr("unit"), Some("meter"));
        assert_eq!(dom.attr_ns("http://www.w3.org/XML/1998/namespace", "lang").or(dom.attr("lang")), Some("und"));
        assert_eq!(dom.attr("requiredextensions"), Some("m"));
        assert!(dom.ns_decls.contains(&("m".to_string(), NS_MATERIAL.to_string())));
        let meta: Vec<(Option<&str>, &str)> =
            dom.children_named("metadata").map(|m| (m.attr("name"), m.text.as_str())).collect();
        assert_eq!(meta, [(Some("Title"), "textured"), (Some("Application"), APPLICATION)]);

        // One build item, turning +Y up into +Z up: (0, 1, 0) lands on (0, 0, 1).
        let build = dom.child("build").unwrap();
        let items: Vec<&Element> = build.children_named("item").collect();
        assert_eq!(items.len(), 1);
        let m = parse_matrix(items[0].attr("transform").unwrap()).unwrap();
        let up = math::transform_point_f64(&m, [0.0, 1.0, 0.0]);
        assert!(near(&up.map(|v| v as f32), &[0.0, 0.0, 1.0], 1e-12), "{up:?}");

        // Base materials are core: a model with nothing else needs no extension.
        let plain = model_dom(&written(&fixtures::morph()));
        assert_eq!(plain.attr("requiredextensions"), None);
        assert!(plain.ns_decls.iter().all(|(_, uri)| uri != NS_MATERIAL));
    }

    #[test]
    fn vertex_numbers_are_written_unscaled_and_exact() {
        let mut s = one_triangle_scene(None);
        s.meshes[0].primitives[0].positions = vec![[0.1, -2.5e-7, 1234.5], [1.0 / 3.0, 7e12, 0.0], [-0.0, 1.0, 2.0]];
        let dom = model_dom(&written(&s));
        let v: Vec<[f32; 3]> = resources(&dom)
            .descendants_named("vertex")
            .iter()
            .map(|e| ["x", "y", "z"].map(|a| e.attr(a).unwrap().parse::<f32>().unwrap()))
            .collect();
        assert_eq!(v[0], [0.1, -2.5e-7, 1234.5]);
        assert_eq!(v[1], [1.0 / 3.0, 7e12, 0.0]);
        let read = read(&written(&s)).unwrap();
        assert_eq!(read.positions[1], [1.0 / 3.0, 7e12, 0.0]);
    }

    #[test]
    fn closed_meshes_are_welded_into_manifold_counter_clockwise_shells() {
        // The fixture box has 24 vertices (one per face corner, for its UVs
        // and normals); 3MF counts edges by vertex index, so a box that is not
        // welded back to 8 corners is twelve loose triangles to a slicer.
        let dom = model_dom(&written(&fixtures::textured()));
        let box_obj = resources(&dom).children_named("object").find(|o| o.attr("name") == Some("box")).unwrap();
        let verts: Vec<[f64; 3]> = box_obj
            .descendants_named("vertex")
            .iter()
            .map(|e| ["x", "y", "z"].map(|a| e.attr(a).unwrap().parse::<f64>().unwrap()))
            .collect();
        let tris: Vec<[usize; 3]> = box_obj
            .descendants_named("triangle")
            .iter()
            .map(|e| ["v1", "v2", "v3"].map(|a| e.attr(a).unwrap().parse::<usize>().unwrap()))
            .collect();
        assert_eq!((verts.len(), tris.len()), (8, 12));
        // Manifold and consistently oriented: every directed edge once, and
        // its reverse once.
        let mut edges: Vec<(usize, usize)> = tris.iter().flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])]).collect();
        edges.sort();
        let before = edges.len();
        edges.dedup();
        assert_eq!(edges.len(), before, "a directed edge is used twice");
        for &(a, b) in &edges {
            assert!(edges.binary_search(&(b, a)).is_ok(), "edge {a}-{b} has no twin");
        }
        // Outward: positive volume by (B−A)×(C−A), the spec's normal.
        let vol: f64 = tris
            .iter()
            .map(|t| {
                let (a, b, c) = (verts[t[0]], verts[t[1]], verts[t[2]]);
                a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])
            })
            .sum::<f64>()
            / 6.0;
        assert!((vol - 1.0).abs() < 1e-9, "unit box volume {vol}");
        // Every textured triangle carries its group and all three indices.
        assert!(box_obj.descendants_named("triangle").iter().all(|t| t.attr("p3").is_some() && t.attr("pid").is_some()));
        assert!(box_obj.attr("pid").is_some() && box_obj.attr("pindex").is_some(), "objects with properties MUST name defaults");
    }

    #[test]
    fn a_mirrored_placement_stays_a_transform_and_a_sheared_one_is_baked() {
        let mut s = one_triangle_scene(None);
        s.nodes[0].transform =
            Transform::Trs { translation: [1.0, 0.0, 0.0], rotation: math::QUAT_IDENTITY, scale: [-1.0, 1.0, 1.0] };
        let mut sheared = math::IDENTITY;
        sheared[4] = 0.75; // x += 0.75 y
        sheared[12] = 3.0;
        s.add_root(Node { mesh: Some(0), transform: Transform::Matrix(sheared), ..Default::default() });
        s.validate().unwrap();

        let prepared = prepare(&s).unwrap();
        assert_eq!(prepared.meshes.len(), 2, "the sheared instance gets its own baked mesh");
        let dom = model_dom(&written(&s));
        let comps = resources(&dom).descendants_named("component");
        assert_eq!(comps.len(), 2);
        let dets: Vec<f64> = comps
            .iter()
            .map(|c| c.attr("transform").map_or(1.0, |t| math::det3(&math::upper3(&parse_matrix(t).unwrap()))))
            .collect();
        assert!(dets.contains(&-1.0), "mirror kept as a transform: {dets:?}");
        assert!(dets.iter().all(|d| (d.abs() - 1.0).abs() < 1e-12), "no shear left in any placement: {dets:?}");
        let back = read_ok(&written(&s)).scene;
        assert_draws_the_same(&prepared, &back, "mirror and shear");
        assert!((signed_volume(&back) - signed_volume(&prepared)).abs() < 1e-9);
    }

    // ── colour ──────────────────────────────────────────────────────────────

    #[test]
    fn base_colours_are_eight_digit_srgb_from_linear() {
        let s = one_triangle_scene(Some(Material {
            name: Some("half".into()),
            base_color: [0.5, 1.0, 0.0, 0.5],
            ..Default::default()
        }));
        let dom = model_dom(&written(&s));
        let base = resources(&dom).descendants_named("base")[0];
        // linear 0.5 is sRGB 0.7354 → 187.5 → 0xBC; alpha is linear: 0x80.
        assert_eq!(base.attr("displaycolor"), Some("#BCFF0080"));
        assert_eq!(base.attr("name"), Some("half"));
        let back = read_ok(&written(&s)).scene;
        let c = back.materials[0].base_color;
        assert!(near(&c, &[srgb_to_linear(188.0 / 255.0), 1.0, 0.0, 128.0 / 255.0], 1e-7), "{c:?}");
        assert_eq!(back.materials[0].metallic, 0.0, "a 3MF colour is matte, not glTF's default metal");
    }

    #[test]
    fn vertex_colours_absorb_the_base_colour_and_become_opaque() {
        let mut s = one_triangle_scene(Some(Material { base_color: [1.0, 0.0, 0.0, 1.0], ..Default::default() }));
        s.meshes[0].primitives[0].colors = Some(vec![[0.5, 0.5, 0.5, 0.25], [1.0, 1.0, 1.0, 1.0], [0.0, 1.0, 0.0, 1.0]]);
        let dom = model_dom(&written(&s));
        let colors: Vec<&str> = resources(&dom).descendants_named("color").iter().map(|c| c.attr("color").unwrap()).collect();
        assert_eq!(colors, ["#BC0000FF", "#FF0000FF", "#000000FF"]);
        assert!(resources(&dom).child("basematerials").is_none(), "the colour group replaces the material");
        let losses = losses(&s);
        assert!(losses.iter().any(|l| l.what.contains("alpha")), "{losses:?}");
        let back = read_ok(&written(&s)).scene;
        let cs = back.meshes[0].primitives[0].colors.as_ref().unwrap();
        assert!(cs.iter().all(|c| c[3] == 1.0));
    }

    #[test]
    fn a_texture_wins_over_tint_and_vertex_colours_and_says_so() {
        let mut s = fixtures::textured();
        // Tint the box and give it vertex colours: neither can ride along.
        s.materials[0].base_color = [0.5, 0.5, 1.0, 1.0];
        let n = s.meshes[0].primitives[0].positions.len();
        s.meshes[0].primitives[0].colors = Some(vec![[0.2, 0.3, 0.4, 1.0]; n]);
        let l = losses(&s);
        assert!(l.iter().any(|x| x.what.contains("base-colour factor")), "{l:?}");
        assert!(l.iter().any(|x| x.what.contains("vertex colours on 1 textured")), "{l:?}");
        let p = prepare(&s).unwrap();
        let boxp = &p.meshes[0].primitives[0];
        assert!(boxp.colors.is_none());
        assert_eq!(p.materials[boxp.material.unwrap()].base_color, [1.0; 4]);
        assert_draws_the_same(&p, &read_ok(&written(&s)).scene, "textured with tint");
    }

    #[test]
    fn texture_coordinates_flip_into_the_lower_left_origin() {
        let mut s = fixtures::skinned();
        s.skins.clear();
        s.nodes[3].skin = None;
        s.animations.clear();
        for p in s.meshes[0].primitives.iter_mut() {
            p.skin = None;
        }
        s.meshes[0].primitives[0].uvs[0][0] = [0.25, 0.1];
        let dom = model_dom(&written(&s));
        let first = resources(&dom).descendants_named("tex2coord")[0];
        assert_eq!((first.attr("u"), first.attr("v")), (Some("0.25"), Some("0.9")));
    }

    #[test]
    fn a_texture_transform_is_baked_into_the_coordinates() {
        let mut s = fixtures::textured();
        s.materials[0].base_color_texture.as_mut().unwrap().transform =
            Some(UvTransform { offset: [0.5, 0.25], rotation: 0.0, scale: [2.0, 3.0] });
        let p = prepare(&s).unwrap();
        let mat = &p.materials[p.meshes[0].primitives[0].material.unwrap()];
        assert!(mat.base_color_texture.as_ref().unwrap().transform.is_none());
        // Corner uv (0, 1) → (0·2 + 0.5, 1·3 + 0.25).
        assert_eq!(p.meshes[0].primitives[0].uvs[0][0], [0.5, 3.25]);
        assert_draws_the_same(&p, &read_ok(&written(&s)).scene, "texture transform");
    }

    #[test]
    fn untextured_uvs_and_unusable_textures_are_reported() {
        let l = losses(&fixtures::cube());
        assert!(l.iter().any(|x| x.what.contains("texture coordinates on 1 untextured")), "{l:?}");
        let mut s = fixtures::textured();
        s.materials[0].base_color_texture.as_mut().unwrap().uv_set = 1;
        let l = losses(&s);
        assert!(l.iter().any(|x| x.what.contains("without UV set 0")), "{l:?}");
        let p = prepare(&s).unwrap();
        assert!(p.materials.iter().all(|m| m.base_color_texture.is_none()));
    }

    #[test]
    fn webp_textures_are_dropped_to_the_base_colour() {
        let mut s = fixtures::textured();
        s.images[0] = Image { name: None, format: ImageFormat::Webp, data: b"RIFF\0\0\0\0WEBPVP8 ".to_vec() };
        s.materials[0].base_color = [0.0, 0.5, 0.0, 1.0];
        let p = prepare(&s).unwrap();
        assert!(p.images.is_empty() && p.textures.is_empty());
        let m = &p.materials[p.meshes[0].primitives[0].material.unwrap()];
        assert_eq!(m.base_color, quantized([0.0, 0.5, 0.0, 1.0]));
        let bytes = written(&s);
        let a = Archive::parse(&bytes).unwrap();
        assert!(a.entries.iter().all(|e| !e.name.starts_with("3D/Textures")));
    }

    #[test]
    fn samplers_map_to_the_closest_3mf_filter_and_wrap() {
        let mut s = Sampler { wrap_s: Wrap::MirroredRepeat, wrap_t: Wrap::ClampToEdge, ..Default::default() };
        assert_eq!(tex_filter(&s), TexFilter::Auto);
        s.mag_filter = Some(Filter::Nearest);
        s.min_filter = Some(Filter::Nearest);
        s.mipmaps = false;
        assert_eq!(tex_filter(&s), TexFilter::Nearest);
        assert_eq!(representable_sampler(&s), s, "NEAREST/NEAREST survives exactly");
        s.mipmaps = true;
        assert_eq!(tex_filter(&s), TexFilter::Nearest, "a pixel-art magnifier stays sharp");
        s.mag_filter = Some(Filter::Linear);
        s.min_filter = Some(Filter::Linear);
        s.mipmaps = false;
        assert_eq!(tex_filter(&s), TexFilter::Linear);
        s.mipmaps = true;
        s.mipmap_filter = Filter::Linear;
        assert_eq!(tex_filter(&s), TexFilter::Auto, "trilinear is what auto renders as");
        assert_eq!(effective_filters(&s), effective_filters(&representable_sampler(&s)));

        let mut f = fixtures::textured();
        f.textures[0].sampler = Sampler {
            wrap_s: Wrap::MirroredRepeat,
            wrap_t: Wrap::ClampToEdge,
            mag_filter: Some(Filter::Nearest),
            min_filter: Some(Filter::Linear),
            mipmaps: true,
            mipmap_filter: Filter::Nearest,
        };
        let l = losses(&f);
        assert!(l.iter().any(|x| x.what.contains("sampling filter")), "{l:?}");
        let dom = model_dom(&written(&f));
        let tex = resources(&dom).child("texture2d").unwrap();
        assert_eq!((tex.attr("tilestyleu"), tex.attr("tilestylev"), tex.attr("filter")), (Some("mirror"), Some("clamp"), Some("nearest")));
        let back = read_ok(&written(&f)).scene;
        assert_eq!(back.textures[0].sampler, representable_sampler(&f.textures[0].sampler));
    }

    #[test]
    fn names_and_titles_survive_escaping() {
        let mut s = one_triangle_scene(Some(Material { name: Some("a<b>&\"c'\td\ne".into()), ..Default::default() }));
        s.name = Some("Fish & <Chips>".into());
        s.meshes[0].name = Some("x\"y".into());
        let back = read_ok(&written(&s)).scene;
        assert_eq!(back.name.as_deref(), Some("Fish & <Chips>"));
        assert_eq!(back.materials[0].name.as_deref(), Some("a<b>&\"c'\td\ne"));
        assert_eq!(back.meshes[0].name.as_deref(), Some("x\"y"));
    }

    #[test]
    fn grouping_nodes_skins_and_degenerate_triangles_are_reported() {
        let l = losses(&fixtures::textured());
        assert!(l.iter().any(|x| x.what.contains("1 node(s) without geometry")), "{l:?}");
        let mut s = one_triangle_scene(None);
        s.meshes[0].primitives[0].positions.push([0.0, 1.0, 0.0]);
        s.meshes[0].primitives[0].indices.extend_from_slice(&[0, 2, 3]);
        let l = losses(&s);
        assert!(l.iter().any(|x| x.what.contains("1 degenerate")), "{l:?}");
        assert_eq!(prepare(&s).unwrap().triangle_count(), 1);
        assert_eq!(read_ok(&written(&s)).scene.triangle_count(), 1);
    }

    #[test]
    fn the_flat_api_round_trips_geometry_bit_for_bit() {
        let mesh = pose::flatten(&fixtures::cube()).unwrap();
        let back = read(&write(&mesh).unwrap()).unwrap();
        assert_eq!(back.positions.len(), 8);
        let tris = |m: &Mesh| {
            let mut v: Vec<[[u32; 3]; 3]> = m.to_soup().iter().map(|t| t.map(|p| p.map(f32::to_bits))).collect();
            for t in v.iter_mut() {
                let k = (0..3).min_by_key(|&k| t[k]).unwrap();
                t.rotate_left(k);
            }
            v.sort();
            v
        };
        assert_eq!(tris(&back), tris(&mesh));
        assert_eq!(back.base_color, mesh.base_color.map(quantized));
        assert_eq!(back.uvs, None, "a texture-less 3MF holds no coordinates");
        assert_eq!(back.normals, None);
    }

    // ── reading files we did not write ──────────────────────────────────────

    /// The reference model of the research notes (a textured cube, a colour
    /// group tetrahedron, a base-material tetrahedron with one triangle's
    /// material overridden, all placed by components under a translated build
    /// item), hand-written against the 3MF Core and Materials specifications.
    /// three.js r183 and assimp 6 both load it.
    fn reference_model() -> String {
        r##"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:m="http://schemas.microsoft.com/3dmanufacturing/material/2015/02" xmlns:lg="urn:example:litegen">
  <metadata name="Title">litegen reference</metadata>
  <metadata name="lg:SourceFormat">glTF 2.0</metadata>
  <resources>
    <basematerials id="1"><base name="Red" displaycolor="#FF0000FF"/><base name="White" displaycolor="#FFFFFFFF"/></basematerials>
    <m:colorgroup id="5"><m:color color="#FF0000"/><m:color color="#00FF00"/><m:color color="#0000FF"/><m:color color="#FFFF00"/></m:colorgroup>
    <m:texture2d id="6" path="/3D/Texture/basecolor.png" contenttype="image/png" tilestyleu="wrap" tilestylev="wrap" filter="auto"/>
    <m:texture2dgroup id="7" texid="6"><m:tex2coord u="0" v="0"/><m:tex2coord u="1" v="0"/><m:tex2coord u="1" v="1"/><m:tex2coord u="0" v="1"/></m:texture2dgroup>
    <object id="2" type="model" name="TexturedCube" pid="7" pindex="0">
      <mesh>
        <vertices><vertex x="0" y="0" z="0"/><vertex x="20" y="0" z="0"/><vertex x="20" y="20" z="0"/><vertex x="0" y="20" z="0"/><vertex x="0" y="0" z="20"/><vertex x="20" y="0" z="20"/><vertex x="20" y="20" z="20"/><vertex x="0" y="20" z="20"/></vertices>
        <triangles>
          <triangle v1="0" v2="1" v3="5" p1="0" p2="1" p3="2"/><triangle v1="0" v2="5" v3="4" p1="0" p2="2" p3="3"/>
          <triangle v1="1" v2="2" v3="6" p1="0" p2="1" p3="2"/><triangle v1="1" v2="6" v3="5" p1="0" p2="2" p3="3"/>
          <triangle v1="2" v2="3" v3="7" p1="0" p2="1" p3="2"/><triangle v1="2" v2="7" v3="6" p1="0" p2="2" p3="3"/>
          <triangle v1="3" v2="0" v3="4" p1="0" p2="1" p3="2"/><triangle v1="3" v2="4" v3="7" p1="0" p2="2" p3="3"/>
          <triangle v1="4" v2="5" v3="6" p1="0" p2="1" p3="2"/><triangle v1="4" v2="6" v3="7" p1="0" p2="2" p3="3"/>
          <triangle v1="1" v2="0" v3="3" p1="0" p2="1" p3="2"/><triangle v1="1" v2="3" v3="2" p1="0" p2="2" p3="3"/>
        </triangles>
      </mesh>
    </object>
    <object id="3" type="model" name="ColoredTetra" pid="5" pindex="0">
      <mesh>
        <vertices><vertex x="0" y="0" z="0"/><vertex x="20" y="0" z="0"/><vertex x="10" y="20" z="0"/><vertex x="10" y="7" z="20"/></vertices>
        <triangles>
          <triangle v1="0" v2="2" v3="1" p1="0" p2="2" p3="1"/><triangle v1="0" v2="1" v3="3" p1="0" p2="1" p3="3"/>
          <triangle v1="1" v2="2" v3="3" p1="1" p2="2" p3="3"/><triangle v1="2" v2="0" v3="3" p1="2" p2="0" p3="3"/>
        </triangles>
      </mesh>
    </object>
    <object id="4" type="model" name="BaseMatTetra" pid="1" pindex="0">
      <mesh>
        <vertices><vertex x="0" y="0" z="0"/><vertex x="20" y="0" z="0"/><vertex x="10" y="20" z="0"/><vertex x="10" y="7" z="20"/></vertices>
        <triangles>
          <triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/><triangle v1="1" v2="2" v3="3"/>
          <triangle v1="2" v2="0" v3="3" pid="1" p1="1"/>
        </triangles>
      </mesh>
    </object>
    <object id="10" type="model" name="Assembly">
      <components>
        <component objectid="2"/>
        <component objectid="3" transform="1 0 0 0 1 0 0 0 1 30 0 0"/>
        <component objectid="4" transform="1 0 0 0 1 0 0 0 1 60 0 0"/>
      </components>
    </object>
  </resources>
  <build><item objectid="10" transform="1 0 0 0 1 0 0 0 1 50 50 0"/></build>
</model>"##
            .to_string()
    }

    /// The model part's relationships, naming each texture as the spec
    /// requires; `targets` as written (absolute or relative to the model).
    fn model_rels(targets: &[&str]) -> String {
        let mut x = format!(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="{NS_RELATIONSHIPS}">"#);
        for (i, t) in targets.iter().enumerate() {
            let _ = write!(x, r#"<Relationship Id="t{i}" Target="{t}" Type="{REL_TEXTURE}"/>"#);
        }
        x + "</Relationships>"
    }

    fn reference_package() -> Vec<u8> {
        let png = fixtures::quadrant_png();
        let rels = model_rels(&["/3D/Texture/basecolor.png"]);
        package_with(&reference_model(), &[("3D/_rels/3dmodel.model.rels", rels.as_bytes()), ("3D/Texture/basecolor.png", &png)])
    }

    fn bounds(s: &Scene) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for t in drawn(s) {
            for c in t {
                for k in 0..3 {
                    lo[k] = lo[k].min(c.pos[k]);
                    hi[k] = hi[k].max(c.pos[k]);
                }
            }
        }
        (lo, hi)
    }

    #[test]
    fn reads_the_hand_written_reference_model() {
        let r = read_ok(&reference_package());
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let s = &r.scene;
        assert_eq!(s.name.as_deref(), Some("litegen reference"));
        assert_eq!(s.triangle_count(), 12 + 4 + 4);
        // Millimetres to metres, Z-up to Y-up: file x ∈ [50, 130], y ∈ [50,
        // 70], z ∈ [0, 20] becomes x ∈ [.05, .13], y ∈ [0, .02], z ∈ [−.07, −.05].
        let (lo, hi) = bounds(s);
        assert!(near(&lo, &[0.05, 0.0, -0.07], 1e-6) && near(&hi, &[0.13, 0.02, -0.05], 1e-6), "{lo:?} {hi:?}");
        // The cube's texture: lower-left (0, 0) is the scene's (0, 1).
        let cube = &s.meshes[s.nodes.iter().find(|n| n.name.as_deref() == Some("TexturedCube")).unwrap().mesh.unwrap()];
        assert_eq!(cube.primitives.len(), 1);
        let p = &cube.primitives[0];
        let mat = &s.materials[p.material.unwrap()];
        let tex = &s.textures[mat.base_color_texture.as_ref().unwrap().texture];
        assert_eq!(s.images[tex.image].data, fixtures::quadrant_png());
        assert!(p.uvs[0].contains(&[0.0, 1.0]) && p.uvs[0].contains(&[1.0, 0.0]));
        // The colour tetrahedron: six-digit colours are opaque, decoded to linear.
        let tetra = &s.meshes[s.nodes.iter().find(|n| n.name.as_deref() == Some("ColoredTetra")).unwrap().mesh.unwrap()];
        let cs = tetra.primitives[0].colors.as_ref().unwrap();
        assert!(cs.contains(&[1.0, 0.0, 0.0, 1.0]) && cs.contains(&[1.0, 1.0, 0.0, 1.0]));
        // The base-material tetrahedron: one triangle overrides the object's
        // default, so two primitives, Red ×3 and White ×1.
        let bm = &s.meshes[s.nodes.iter().find(|n| n.name.as_deref() == Some("BaseMatTetra")).unwrap().mesh.unwrap()];
        let split: Vec<(Option<&str>, usize)> = bm
            .primitives
            .iter()
            .map(|p| (s.materials[p.material.unwrap()].name.as_deref(), p.triangle_count()))
            .collect();
        assert_eq!(split, [(Some("Red"), 3), (Some("White"), 1)]);
        assert!(signed_volume(s) > 0.0, "outward shells stay outward through the axis change");
    }

    #[test]
    fn namespaces_resolve_by_uri_whatever_the_prefix() {
        let doc = format!(
            r##"<c:model xmlns:c="{NS_CORE}" xmlns:mat="{NS_MATERIAL}" c:unit="ignored-because-prefixed" unit="meter"><c:resources><mat:colorgroup id="1"><mat:color color="#FF8000"/></mat:colorgroup><c:object id="2" pid="1" pindex="0">{}</c:object></c:resources><c:build><c:item objectid="2"/></c:build></c:model>"##,
            TETRA.replace("<", "<c:").replace("<c:/", "</c:")
        );
        let r = read_ok(&package(&doc));
        let p = &r.scene.meshes[0].primitives[0];
        assert_eq!(p.triangle_count(), 4);
        assert_eq!(p.colors.as_ref().unwrap()[0], [1.0, decode_srgb8(0x80), 0.0, 1.0]);
        // And a document that uses the conventional `m:` prefix for some
        // other namespace does not get its elements read as materials.
        let impostor = format!(
            r##"<model xmlns="{NS_CORE}" xmlns:m="urn:not-materials"><resources><m:colorgroup id="1"><m:color color="#FF8000"/></m:colorgroup><object id="2">{TETRA}</object></resources><build><item objectid="2"/></build></model>"##
        );
        let r = read_ok(&package(&impostor));
        assert!(r.scene.meshes[0].primitives[0].colors.is_none());
    }

    #[test]
    fn every_unit_converts_to_metres() {
        for (unit, scale) in [
            ("micron", 1e-6f64),
            ("millimeter", 1e-3),
            ("centimeter", 1e-2),
            ("inch", 0.0254),
            ("foot", 0.3048),
            ("meter", 1.0),
        ] {
            let doc = format!(
                r#"<model unit="{unit}" xmlns="{NS_CORE}"><resources><object id="1">{TETRA}</object></resources><build><item objectid="1"/></build></model>"#
            );
            let (lo, hi) = bounds(&read_ok(&package(&doc)).scene);
            // File x ∈ [0, 1], y ∈ [0, 1], z ∈ [0, 1] → scene x, y = z, z = −y.
            let s = scale as f32;
            assert!(near(&lo, &[0.0, 0.0, -s], s * 1e-6) && near(&hi, &[s, s, 0.0], s * 1e-6), "{unit}: {lo:?} {hi:?}");
        }
        let doc = format!(r#"<model unit="furlong" xmlns="{NS_CORE}"><resources/><build/></model>"#);
        assert!(malformed_text(read_err(&package(&doc))).contains("furlong"));
    }

    #[test]
    fn nested_components_place_one_mesh_many_times() {
        let doc = model(
            &format!(
                r#"<object id="1" name="leaf">{TETRA}</object>
                <object id="2" name="pair"><components><component objectid="1" transform="1 0 0 0 1 0 0 0 1 10 0 0"/><component objectid="1" transform="1 0 0 0 1 0 0 0 1 -10 0 0"/></components></object>
                <object id="3" name="quad"><components><component objectid="2"/><component objectid="2" transform="1 0 0 0 1 0 0 0 1 0 0 50"/></components></object>"#
            ),
            r#"<item objectid="3"/>"#,
        );
        let s = read_ok(&package(&doc)).scene;
        assert_eq!(s.meshes.len(), 1, "one mesh, instanced");
        assert_eq!(s.nodes.iter().filter(|n| n.mesh.is_some()).count(), 4);
        assert_eq!(s.triangle_count(), 4);
        let (lo, hi) = bounds(&s);
        // x ∈ [−10, 11] mm; z_file ∈ [0, 51] → y_scene; y_file ∈ [0, 1] → −z_scene.
        assert!(near(&lo, &[-0.010, 0.0, -0.001], 1e-7) && near(&hi, &[0.011, 0.051, 0.0], 1e-7), "{lo:?} {hi:?}");
        assert_eq!(s.nodes[s.roots[0]].name.as_deref(), Some("quad"));
    }

    #[test]
    fn the_production_extension_reads_objects_from_other_parts() {
        let sub = format!(
            r#"<model unit="millimeter" xmlns="{NS_CORE}"><resources><object id="1" name="far">{TETRA}</object></resources><build/></model>"#
        );
        let root = format!(
            r#"<model unit="millimeter" xmlns="{NS_CORE}" xmlns:p="{NS_PRODUCTION}" requiredextensions="p"><resources><object id="5" p:UUID="00000001-61cb-4c03-9d28-80fed5dfa1dc"><components><component p:path="/3D/Objects/object_1.model" objectid="1" transform="1 0 0 0 1 0 0 0 1 5 0 0"/></components></object></resources><build p:UUID="2c7c17d8-22b5-4d84-8835-1976022ea369"><item objectid="5" printable="1"/></build></model>"#
        );
        let r = read_ok(&package_with(&root, &[("3D/Objects/object_1.model", sub.as_bytes()), ("Metadata/model_settings.config", b"<config/>")]));
        assert_eq!(r.scene.triangle_count(), 4);
        assert!(r.scene.nodes.iter().any(|n| n.name.as_deref() == Some("far")));
        let (lo, _) = bounds(&r.scene);
        assert!(near(&lo, &[0.005, 0.0, -0.001], 1e-7), "{lo:?}");

        let missing = read_err(&package(&root));
        assert!(malformed_text(missing).contains("object_1.model"));
    }

    #[test]
    fn triangle_properties_follow_the_specs_defaults_and_split_vertices() {
        // Object default pid/pindex; a triangle with p1 only colours its whole
        // face; a vertex seen with two colours becomes two scene vertices.
        let doc = model(
            r##"<m:colorgroup id="1"><m:color color="#FF0000"/><m:color color="#00FF00"/><m:color color="#0000FF"/></m:colorgroup>
                <object id="2" pid="1" pindex="2"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/><vertex x="0" y="0" z="1"/></vertices>
                <triangles><triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3" p1="0"/><triangle v1="0" v2="3" v3="2" p1="0" p2="1" p3="1"/><triangle v1="1" v2="2" v3="3" p1="1" p3="0"/></triangles></mesh></object>"##,
            r#"<item objectid="2"/>"#,
        );
        let s = read_ok(&package(&doc)).scene;
        let p = &s.meshes[0].primitives[0];
        let colors = p.colors.as_ref().unwrap();
        let face = |t: usize| [0, 1, 2].map(|k| colors[p.indices[t * 3 + k] as usize]);
        let (r, g, b) = ([1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(face(0), [b, b, b], "object default pindex");
        assert_eq!(face(1), [r, r, r], "p1 alone colours the whole face");
        assert_eq!(face(2), [r, g, g], "per-corner colours");
        assert_eq!(face(3), [g, g, g], "p3 without p2 means p1 everywhere");
        assert!(p.positions.len() > 4, "vertex 0 is red in one face and blue in another");
    }

    #[test]
    fn composite_materials_mix_and_multiproperties_carry_one_layer_with_warnings() {
        let doc = model(
            &format!(
                r##"<basematerials id="1"><base name="A" displaycolor="#FF0000"/><base name="B" displaycolor="#0000FF"/></basematerials>
                <m:colorgroup id="2"><m:color color="#00FF00"/></m:colorgroup>
                <m:compositematerials id="3" matid="1" matindices="0 1"><m:composite values="1 3"/><m:composite values="0 0"/></m:compositematerials>
                <m:multiproperties id="4" pids="1 2"><m:multi pindices="0 0"/></m:multiproperties>
                <object id="5" pid="3" pindex="0">{TETRA}</object>
                <object id="6" pid="4" pindex="0">{TETRA}</object>"##
            ),
            r#"<item objectid="5"/><item objectid="6" transform="1 0 0 0 1 0 0 0 1 5 0 0"/>"#,
        );
        let r = read_ok(&package(&doc));
        assert!(r.warnings.iter().any(|w| w.contains("composite")), "{:?}", r.warnings);
        assert!(r.warnings.iter().any(|w| w.contains("multiproperties")), "{:?}", r.warnings);
        let mixed = r.scene.meshes[0].primitives[0].colors.as_ref().unwrap()[0];
        assert!(near(&mixed, &[0.25, 0.0, 0.75, 1.0], 1e-6), "a 1:3 mix of red and blue: {mixed:?}");
        let layered = r.scene.meshes[1].primitives[0].colors.as_ref().unwrap()[0];
        assert_eq!(layered, [0.0, 1.0, 0.0, 1.0], "the colour layer over the material");
    }

    #[test]
    fn supports_are_skipped_and_type_other_is_refused() {
        let doc = model(
            &format!(r#"<object id="1">{TETRA}</object><object id="2" type="support">{TETRA}</object>"#),
            r#"<item objectid="1"/><item objectid="2"/>"#,
        );
        let r = read_ok(&package(&doc));
        assert_eq!(r.scene.triangle_count(), 4);
        assert!(r.warnings.iter().any(|w| w.contains("support")), "{:?}", r.warnings);
        let doc = model(&format!(r#"<object id="1" type="other">{TETRA}</object>"#), r#"<item objectid="1"/>"#);
        assert!(malformed_text(read_err(&package(&doc))).contains("other"));
    }

    #[test]
    fn slicer_and_unknown_extension_markup_is_ignored() {
        // The shape of a Bambu Studio/PrusaSlicer project: vendor namespaces,
        // unprefixed paint attributes, metadata in vendor namespaces, triangle
        // sets, markup-compatibility attributes, a BOM, a project config part.
        let doc = format!(
            "\u{FEFF}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"{NS_CORE}\" xmlns:BambuStudio=\"http://schemas.bambulab.com/package/2021\" xmlns:slic3rpe=\"http://schemas.slic3r.org/3mf/2017/06\" xmlns:t=\"{NS_TRIANGLE_SETS}\" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" mc:Ignorable=\"slic3rpe\"><metadata name=\"BambuStudio:3mfVersion\">1</metadata><metadata name=\"Application\">BambuStudio-01.09</metadata><resources><object id=\"1\" type=\"model\" slic3rpe:volume=\"1\"><mesh><vertices><vertex x=\"0\" y=\"0\" z=\"0\"/><vertex x=\"1\" y=\"0\" z=\"0\"/><vertex x=\"0\" y=\"1\" z=\"0\"/><vertex x=\"0\" y=\"0\" z=\"1\"/></vertices><triangles><triangle v1=\"0\" v2=\"2\" v3=\"1\" paint_color=\"4\"/><triangle v1=\"0\" v2=\"1\" v3=\"3\"/><triangle v1=\"0\" v2=\"3\" v3=\"2\"/><triangle v1=\"1\" v2=\"2\" v3=\"3\"/></triangles><t:trianglesets><t:triangleset name=\"a\" identifier=\"a\"><t:ref index=\"0\"/></t:triangleset></t:trianglesets></mesh></object></resources><build><item objectid=\"1\" printable=\"1\"/></build></model>"
        );
        let r = read_ok(&package_with(&doc, &[("Metadata/project_settings.config", b"{ \"x\": 1 }")]));
        assert_eq!(r.scene.triangle_count(), 4);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn required_extensions_are_honoured_by_namespace() {
        let with = |decls: &str, req: &str| {
            format!(
                r#"<model xmlns="{NS_CORE}" {decls} requiredextensions="{req}"><resources><object id="1">{TETRA}</object></resources><build><item objectid="1"/></build></model>"#
            )
        };
        let ok = with(&format!(r#"xmlns:mat="{NS_MATERIAL}" xmlns:p="{NS_PRODUCTION}""#), "mat p");
        assert_eq!(read_ok(&package(&ok)).scene.triangle_count(), 4);
        let beams = with(&format!(r#"xmlns:b="{NS_BEAM_LATTICE}""#), "b");
        assert!(unsupported_text(read_err(&package(&beams))).contains("beam lattice"));
        let future = with(r#"xmlns:x="urn:some-future-extension""#, "x");
        assert!(unsupported_text(read_err(&package(&future))).contains("future"));
        let undeclared = with("", "m");
        assert!(malformed_text(read_err(&package(&undeclared))).contains("does not declare"));
    }

    #[test]
    fn textures_resolve_through_relative_and_differently_cased_part_names() {
        let doc = model(
            r#"<m:texture2d id="1" path="Textures/Pic.PNG" contenttype="image/png" tilestyleu="clamp" tilestylev="mirror" filter="nearest"/>
                <m:texture2dgroup id="2" texid="1"><m:tex2coord u="0" v="0"/><m:tex2coord u="1" v="0"/><m:tex2coord u="0" v="1"/></m:texture2dgroup>
                <object id="3" pid="2" pindex="0"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2" p1="0" p2="1" p3="2"/></triangles></mesh></object>"#,
            r#"<item objectid="3"/>"#,
        );
        let png = fixtures::bands_png();
        // The relationship's target relative to the model part — legal OPC,
        // and the case three.js r183 cannot resolve (it keys textures by the
        // target string as written).
        let rels = model_rels(&["Textures/PIC.png"]);
        let r = read_ok(&package_with(&doc, &[("3D/_rels/3dmodel.model.rels", rels.as_bytes()), ("3D/textures/pic.png", &png)]));
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let s = r.scene;
        let t = &s.textures[0];
        assert_eq!(s.images[t.image].data, png);
        assert_eq!((t.sampler.wrap_s, t.sampler.wrap_t), (Wrap::ClampToEdge, Wrap::MirroredRepeat));
        assert_eq!(t.sampler.mag_filter, Some(Filter::Nearest));
        // A texture part that is missing leaves the triangles untextured, with
        // a warning, rather than failing a model whose geometry is fine.
        let r = read_ok(&package(&doc));
        assert!(r.scene.textures.is_empty());
        assert!(r.warnings.iter().any(|w| w.contains("missing")), "{:?}", r.warnings);
        // A texture that is present but not declared by a relationship is read
        // by its path, and the omission named.
        let r = read_ok(&package_with(&doc, &[("3D/Textures/Pic.PNG", &png)]));
        assert_eq!(r.scene.textures.len(), 1);
        assert!(r.warnings.iter().any(|w| w.contains("relationship")), "{:?}", r.warnings);
    }

    #[test]
    fn reads_what_assimp_writes() {
        // See testdata/3mf/README.md: our textured fixture through assimp 6's
        // exporter, with its ZIP64 end record, misplaced metadata, untransformed
        // items, millimetre label over metre values and linear colours.
        let r = read_ok(include_bytes!("testdata/3mf/assimp_textured.3mf"));
        let s = &r.scene;
        assert_eq!(s.triangle_count(), 64);
        assert_eq!((s.roots.len(), s.meshes.len()), (4, 4), "one item and one object per mesh");
        let (lo, hi) = bounds(s);
        // Local extents x, y ∈ [−0.8, 0.8], z ∈ [−0.5, 0.5], read as mm, Z-up.
        assert!(near(&lo, &[-0.0008, -0.0005, -0.0008], 1e-9) && near(&hi, &[0.0008, 0.0005, 0.0008], 1e-9), "{lo:?} {hi:?}");
        let wedge = s.materials.iter().find(|m| m.name.as_deref() == Some("wedge_mat")).unwrap();
        assert_eq!(wedge.base_color, [decode_srgb8(0xe5), decode_srgb8(0x4c), decode_srgb8(0x19), 1.0]);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn a_second_mesh_in_one_object_indexes_its_own_vertices() {
        // assimp writes one <mesh> per primitive; out of schema, plain meaning.
        let second = TETRA.replace(r#"x="1""#, r#"x="5""#);
        let doc = model(&format!(r#"<object id="1">{TETRA}{second}</object>"#), r#"<item objectid="1"/>"#);
        let r = read_ok(&package(&doc));
        assert_eq!(r.scene.triangle_count(), 8);
        assert!(r.warnings.iter().any(|w| w.contains("more than one <mesh>")), "{:?}", r.warnings);
        let (_, hi) = bounds(&r.scene);
        assert!((hi[0] - 0.005).abs() < 1e-9, "the second mesh's own vertex 1 is at x = 5 mm: {hi:?}");
    }

    // ── refusing what is not 3MF, or is broken ──────────────────────────────

    #[test]
    fn other_containers_are_not_this_format() {
        let not = |bytes: &[u8]| matches!(read_scene(bytes, &NoSidecars), Err(MeshError::NotThisFormat { expected: MeshFormat::ThreeMf, .. }));
        assert!(not(b"solid cube\nendsolid\n"));
        assert!(not(b""));
        // A USDZ-shaped ZIP: no OPC relationships at all.
        assert!(not(&zip_of(&[("model.usda", b"#usda 1.0\n")], Store::Stored { align: 64 })));
        // An OPC package that is some other document (a .docx).
        let docx_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
        assert!(not(&zip_of(&[("_rels/.rels", docx_rels.as_bytes()), ("word/document.xml", b"<w/>")], Store::Deflated)));
    }

    #[test]
    fn broken_packages_are_malformed_and_say_why() {
        let cases: Vec<(String, Vec<u8>, &str)> = vec![
            ("no model part".into(), zip_of(&[("_rels/.rels", PACKAGE_RELS.as_bytes())], Store::Deflated), "no part"),
            (
                "unnamed model part".into(),
                zip_of(
                    &[
                        ("_rels/.rels", PACKAGE_RELS.replace(REL_START_PART, "urn:something-else").as_bytes()),
                        ("3D/3dmodel.model", b"<model/>"),
                    ],
                    Store::Deflated,
                ),
                "no StartPart",
            ),
            (
                "two start parts".into(),
                zip_of(
                    &[(
                        "_rels/.rels",
                        PACKAGE_RELS.replace("</Relationships>", r#"<Relationship Target="/3D/b.model" Id="r2" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>"#).as_bytes(),
                    )],
                    Store::Deflated,
                ),
                "more than one",
            ),
            ("root".into(), package(r#"<notmodel xmlns="urn:x"/>"#), "not a 3MF <model>"),
            ("no build".into(), package(&format!(r#"<model xmlns="{NS_CORE}"><resources/></model>"#)), "no <build>"),
            ("no resources".into(), package(&format!(r#"<model xmlns="{NS_CORE}"><build/></model>"#)), "no <resources>"),
            ("dup id".into(), package(&model(&format!(r#"<object id="1">{TETRA}</object><object id="1">{TETRA}</object>"#), "")), "defined twice"),
            (
                "mesh and components".into(),
                package(&model(&format!(r#"<object id="1">{TETRA}<components><component objectid="1"/></components></object>"#), r#"<item objectid="1"/>"#)),
                "both a <mesh> and <components>",
            ),
            ("missing object".into(), package(&model(&format!(r#"<object id="1">{TETRA}</object>"#), r#"<item objectid="9"/>"#)), "no object with id 9"),
            (
                "vertex index".into(),
                package(&model(&format!(r#"<object id="1">{}</object>"#, TETRA.replace(r#"v3="3""#, r#"v3="4000000000""#)), r#"<item objectid="1"/>"#)),
                "not an index",
            ),
            (
                "vertex range".into(),
                package(&model(&format!(r#"<object id="1">{}</object>"#, TETRA.replace(r#"v3="3""#, r#"v3="9""#)), r#"<item objectid="1"/>"#)),
                "uses vertex 9",
            ),
            (
                "property range".into(),
                package(&model(&format!(r##"<basematerials id="2"><base name="a" displaycolor="#FFFFFF"/></basematerials><object id="1" pid="2" pindex="3">{TETRA}</object>"##), r#"<item objectid="1"/>"#)),
                "entry 3",
            ),
            (
                "pid without index".into(),
                package(&model(&format!(r##"<basematerials id="2"><base name="a" displaycolor="#FFFFFF"/></basematerials><object id="1" pid="2">{TETRA}</object>"##), r#"<item objectid="1"/>"#)),
                "no index",
            ),
            (
                "pid is an object".into(),
                package(&model(&format!(r#"<object id="1" pid="1" pindex="0">{TETRA}</object>"#), r#"<item objectid="1"/>"#)),
                "not one",
            ),
            ("transform".into(), package(&model(&format!(r#"<object id="1">{TETRA}</object>"#), r#"<item objectid="1" transform="1 0 0 0 1 0 0 0 1 0 0"/>"#)), "twelve numbers"),
            ("colour".into(), package(&model(r#"<basematerials id="2"><base name="a" displaycolor="red"/></basematerials>"#, "")), "#RRGGBB"),
            (
                "nan".into(),
                package(&model(&format!(r#"<object id="1">{}</object>"#, TETRA.replace(r#"x="1""#, r#"x="NaN""#)), r#"<item objectid="1"/>"#)),
                "finite",
            ),
            ("misplaced".into(), package(&model(r#"<vertex x="0" y="0" z="0"/>"#, "")), "cannot appear"),
            ("texid".into(), package(&model(r#"<m:texture2dgroup id="2" texid="7"><m:tex2coord u="0" v="0"/></m:texture2dgroup><object id="1" pid="2" pindex="0"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object>"#, r#"<item objectid="1"/>"#)), "texid 7"),
            ("not utf-8".into(), package_with("", &[]).into_iter().collect::<Vec<u8>>(), "root element"),
            (
                "dtd".into(),
                package(&format!(
                    "\u{FEFF}<?xml version=\"1.0\"?>\n<!-- a comment -->\n<!DOCTYPE model [ <!ELEMENT model ANY> ]>\n{}",
                    model(&format!(r#"<object id="1">{TETRA}</object>"#), r#"<item objectid="1"/>"#).replace(r#"<?xml version="1.0" encoding="UTF-8"?>"#, "")
                )),
                "declares a DTD",
            ),
        ];
        for (name, bytes, needle) in cases {
            let e = read_err(&bytes);
            let text = e.to_string();
            assert!(text.contains(needle), "{name}: {text}");
            assert!(matches!(e, MeshError::Malformed(_)), "{name}: {e:?}");
        }
        // Non-UTF-8 model XML.
        let mut utf16 = vec![0xFF, 0xFE];
        for u in "<model/>".encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        let e = read_err(&package_with("", &[]).into_iter().collect::<Vec<u8>>());
        let _ = e;
        let bytes = zip_of(
            &[("_rels/.rels", PACKAGE_RELS.as_bytes()), ("3D/3dmodel.model", &utf16)],
            Store::Deflated,
        );
        assert!(read_err(&bytes).to_string().contains("UTF-8"));
    }

    #[test]
    fn component_cycles_are_refused() {
        for (resources, item) in [
            (r#"<object id="1"><components><component objectid="1"/></components></object>"#.to_string(), "1"),
            (
                r#"<object id="1"><components><component objectid="2"/></components></object><object id="2"><components><component objectid="3"/></components></object><object id="3"><components><component objectid="1"/></components></object>"#.to_string(),
                "2",
            ),
        ] {
            let doc = model(&resources, &format!(r#"<item objectid="{item}"/>"#));
            assert!(malformed_text(read_err(&package(&doc))).contains("cycle"), "{resources}");
        }
    }

    #[test]
    fn deep_and_exponential_component_graphs_are_bounded() {
        // A chain deeper than any assembly: refused, not walked.
        let mut res = format!(r#"<object id="1">{TETRA}</object>"#);
        for i in 2..=(MAX_COMPONENT_DEPTH as u32 + 2) {
            let _ = write!(res, r#"<object id="{i}"><components><component objectid="{}"/></components></object>"#, i - 1);
        }
        let doc = model(&res, &format!(r#"<item objectid="{}"/>"#, MAX_COMPONENT_DEPTH + 2));
        assert!(unsupported_text(read_err(&package(&doc))).contains("deep"));

        // Thirty levels of "two of the level below": 2^30 placements from a
        // few kilobytes. Must fail fast on the node ceiling.
        let mut res = format!(r#"<object id="1">{TETRA}</object>"#);
        for i in 2..=31u32 {
            let _ = write!(
                res,
                r#"<object id="{i}"><components><component objectid="{p}"/><component objectid="{p}" transform="1 0 0 0 1 0 0 0 1 1 0 0"/></components></object>"#,
                p = i - 1
            );
        }
        let bytes = package(&model(&res, r#"<item objectid="31"/>"#));
        assert!(bytes.len() < 16 * 1024);
        let started = std::time::Instant::now();
        assert!(unsupported_text(read_err(&bytes)).contains("placed parts"));
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
    }

    #[test]
    fn hostile_part_names_are_refused() {
        // A ZIP entry that climbs out of the package.
        let evil = zip_of(&[("_rels/.rels", PACKAGE_RELS.as_bytes()), ("../evil.model", b"x")], Store::Deflated);
        assert!(read_scene(&evil, &NoSidecars).is_err());
        // Two entries that are one OPC part.
        let twice = zip_of(
            &[("_rels/.rels", PACKAGE_RELS.as_bytes()), ("3D/3dmodel.model", b"a"), ("3D/3DModel.model", b"b")],
            Store::Deflated,
        );
        assert!(malformed_text(read_err(&twice)).contains("differ only in case"));
        // References that leave the package.
        let tex = model(
            r#"<m:texture2d id="1" path="/../../etc/passwd" contenttype="image/png"/><m:texture2dgroup id="2" texid="1"><m:tex2coord u="0" v="0"/></m:texture2dgroup><object id="3" pid="2" pindex="0"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object>"#,
            r#"<item objectid="3"/>"#,
        );
        assert!(malformed_text(read_err(&package(&tex))).contains("not a part"));
        let path = format!(
            r#"<model xmlns="{NS_CORE}" xmlns:p="{NS_PRODUCTION}"><resources/><build><item objectid="1" p:path="http://example.com/x.model"/></build></model>"#
        );
        assert!(malformed_text(read_err(&package(&path))).contains("not a part"));
    }

    #[test]
    fn a_zip_bomb_entry_is_refused() {
        // The model part deflates from a few kilobytes to megabytes, and the
        // directory lies that it is small: the inflater stops at the declared
        // size and the read fails instead of allocating.
        let filler = " ".repeat(4 << 20);
        let doc = model(&format!(r#"<object id="1">{TETRA}</object>{filler}"#), r#"<item objectid="1"/>"#);
        let mut bytes = package(&doc);
        let cd = bytes.windows(4).rposition(|w| w == [0x50, 0x4b, 0x01, 0x02]).unwrap();
        // The model part is the third entry; walk the central directory to it.
        let mut pos = bytes.windows(4).position(|w| w == [0x50, 0x4b, 0x01, 0x02]).unwrap();
        for _ in 0..2 {
            let n = u16::from_le_bytes([bytes[pos + 28], bytes[pos + 29]]) as usize;
            pos += 46 + n;
        }
        assert!(pos <= cd);
        bytes[pos + 24..pos + 28].copy_from_slice(&1000u32.to_le_bytes());
        assert!(read_scene(&bytes, &NoSidecars).is_err());
    }

    #[test]
    fn never_panics_on_truncation_or_corruption() {
        // A stored package, so the corruption lands in XML text rather than a
        // deflate stream that would reject it wholesale.
        let png = fixtures::quadrant_png();
        let stored = zip_of(
            &[
                ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
                ("_rels/.rels", PACKAGE_RELS.as_bytes()),
                ("3D/3dmodel.model", reference_model().as_bytes()),
                ("3D/Texture/basecolor.png", &png),
            ],
            Store::Stored { align: 1 },
        );
        assert!(read_scene(&stored, &NoSidecars).is_ok());
        for bytes in [stored, written(&fixtures::textured())] {
            let step = (bytes.len() / 400).max(1);
            for cut in (0..bytes.len()).step_by(step) {
                let _ = read_scene(&bytes[..cut], &NoSidecars);
            }
            for i in (0..bytes.len()).step_by(step) {
                for flip in [0x01u8, 0x20, 0x80] {
                    let mut v = bytes.clone();
                    v[i] ^= flip;
                    let _ = read_scene(&v, &NoSidecars);
                }
            }
        }
        // The XML layer itself, CRCs out of the way: every corruption of the
        // model text must come back as a result.
        let text = reference_model();
        let step = (text.len() / 300).max(1);
        for i in (0..text.len()).step_by(step) {
            let mut t = text.clone().into_bytes();
            for b in *b"<\"9- /" {
                t[i] = b;
                let _ = read_scene(&package_with(std::str::from_utf8(&t).unwrap_or(""), &[]), &NoSidecars);
            }
        }
    }

    #[test]
    fn our_own_output_is_refused_by_no_reader_of_ours_and_accepted_by_none_other() {
        let bytes = written(&fixtures::cube());
        for f in MeshFormat::ALL {
            if f != MeshFormat::ThreeMf {
                assert!(super::super::read(&bytes, f).is_err(), "the {f} reader accepted a 3MF");
            }
        }
        assert_eq!(super::super::read(&bytes, MeshFormat::ThreeMf).unwrap().triangle_count(), 12);
    }

    #[test]
    fn alpha_mode_and_double_sidedness_are_material_losses_not_silent() {
        let mut s = fixtures::textured();
        s.materials[1].alpha_mode = AlphaMode::Blend;
        s.materials[1].base_color[3] = 0.5;
        let w = super::super::write_scene(&s, MeshFormat::ThreeMf, super::super::WriteOptions::default()).unwrap();
        let text: Vec<String> = w.losses.iter().map(|l| l.to_string()).collect();
        assert!(text.iter().any(|t| t.contains("material properties other than colour")), "{text:?}");
        assert!(text.iter().any(|t| t.contains("vertex normals")), "{text:?}");
    }
}
