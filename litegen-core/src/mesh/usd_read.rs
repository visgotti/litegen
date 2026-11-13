//! A composed USD stage, read into the scene.
//!
//! # Conventions converted here, and nowhere else
//!
//! - **Up axis and units.** `upAxis = "Z"` stages are turned Y-up by a −90°
//!   rotation about X, and `metersPerUnit` scales to metres, both folded into
//!   the root transforms (and their animation) so no node is invented. An
//!   unauthored `metersPerUnit` means centimetres — USD's documented fallback —
//!   and is reported, because a file that omits it usually forgot it.
//! - **Transforms.** `xformOpOrder` lists ops outermost first, so the local
//!   matrix is `op[0] · op[1] · …` in column-vector terms; `rotateXYZ` applies
//!   X first (`Rz·Ry·Rx`), exactly `math::EulerOrder::Xyz`. Matrices are
//!   row-vector and row-major in USD, which is the same 16 numbers as glTF's
//!   column-major array.
//! - **UVs.** USD's `st` has its origin at the bottom-left; the scene's at the
//!   top-left. `uv = (s, 1 − t)`, and a `UsdTransform2d` on `st` becomes the
//!   conjugate `F ∘ T ∘ F` in uv space (derivation beside
//!   [`uv_transform_from_usd`]).
//! - **UV sets, by name.** A mesh's sets are `st`, then `st1` … `st7` as it
//!   has them (gaps closed), then any other primvar its material reads, eight
//!   at most (`Reader::uv_set_names`). With no `st`, the mesh's first
//!   `texCoord2*` primvar is set 0 instead. A material's bindings are
//!   renumbered to match, one IR material per layout of the meshes bound to it
//!   (`Reader::bind_material`).
//! - **Winding.** `orientation = "leftHanded"` reverses each face.
//! - **Skinning.** UsdSkel renders a skinned point as `skelWorld · J · bind⁻¹ ·
//!   geomBind · p`; with joints as nodes under the skeleton's node, that is the
//!   scene's `world(joint) · ibm · p` with `ibm = bind⁻¹ · geomBind`.
//! - **Time.** Time codes become seconds through `timeCodesPerSecond`.
//! - **Clips.** A default prim whose variant set `Animation` has a variant that
//!   authors `skel:animationSource` is Apple's clip layout (`usd_write`,
//!   "Clips"), and reads as one clip a variant: the authored selection's first,
//!   which the walk reads as any file's animation, then the others in the
//!   set's order. Each other is the stage composed again with that variant
//!   selected over the authored one (`Stage::recompose_selecting`, which shares
//!   the layers and parses nothing), read for its skeletons' joint animation
//!   and blend-shape weights alone: rigid transform samples are the selected
//!   clip's. A clip is named for its SkelAnimation (the original from
//!   `customData`), and ends where `customData.litegen.clipEnd` says, held on
//!   its last pose until then. At most 64 variants are read (a set of more is
//!   refused naming the cap), and the compositions share a budget of prims
//!   (`MAX_VARIANT_PRIMS`); a clip past it is left out with a warning. Any other
//!   variant set, an `Animation` set that switches nothing, and a file whose
//!   skeleton binds one of several SkelAnimations itself read as before: the
//!   bound animation, one clip.
//! - **Unlit.** A Material whose `customData.litegen` holds `bool unlit = 1`
//!   ([`UNLIT_KEY`], written beside the emission recipe, `usd_write`,
//!   "Unlit") reads as `unlit: true`, and its base colour and texture come
//!   from `emissiveColor`, as a lit material's come from `diffuseColor`: the
//!   value or the texture's `scale`, the texture's `rgb`, or a primvar reader's
//!   vertex colour; the diffuse is ignored, and the material has no emissive of
//!   its own. Opacity reads as for any material. The same recipe without the
//!   marker (a black diffuse and an emission, which is also a lamp or a
//!   screen), or with it false, reads exactly as it did: lit, the emission an
//!   emissive.
//! - **Points and lines.** A `Points` prim is a points primitive, a vertex for
//!   each of its `points`. A `BasisCurves` prim of `type = "linear"` is a
//!   lines primitive: a segment between each curve vertex and the next, and
//!   from the last to the first where `wrap = "periodic"` (`pinned` is a cubic
//!   basis's, so a linear curve reads it as nonperiodic). A `cubic` curve, and
//!   one with no `type` (the schema's fallback is cubic), is a spline and is
//!   warned. `widths`, and a curve's normals, which orient a ribbon, are not
//!   read. Primvars follow Ruling I10-R7: on a `Points` prim every point is an
//!   element, so `uniform` is one value per point; on a curve `uniform` is one
//!   per curve, and `varying`, `vertex` and `faceVarying` one per curve vertex.
//!   `OpenUSD@v26.11.1 pxr/usd/usdGeom/schema.usda` L1877 (Points) and
//!   L1594-1812 (BasisCurves).
//!
//! # Lights and cameras
//!
//! A `DistantLight`, a `SphereLight` and a `Camera` prim each read as its
//! node's light or camera. UsdLux lights shine, and cameras look, along the
//! prim's −Z with +Y up, as the scene's do, so nothing is turned. The
//! `litegen` dictionary of a prim's customData ([`LITEGEN_KEY`], which
//! `usd_write` writes) gives the scene's own values exactly, lengths in
//! scene units, when it reads and its type is the prim's. Otherwise, or
//! when it does not read (warned), the schema's attributes give them by
//! this mapping (usdLux/schema.usda and usdGeom/schema.usda, OpenUSD
//! v26.11.1; the lines are cited where each is read):
//! - A DistantLight is directional. A SphereLight that authors
//!   `inputs:shaping:cone:angle` is a spot, as Blender's importer reads one,
//!   and so is one that applies ShapingAPI without it, at the schema's
//!   fallback of 90°, a hemisphere. A cone wider than a spot's 90° makes it
//!   a point light, warned (Ruling R32). Any other SphereLight is a point
//!   light.
//! - The colour is `inputs:color`, linear. A component past 1 moves the
//!   peak into the intensity, so the colour stays within 0 to 1 (Ruling
//!   R8). A colour temperature other than 6500 K (white), an IES profile,
//!   a shaping focus, and a diffuse or specular multiplier other than 1
//!   are each warned and not applied.
//! - The intensity is photometric, as the schema defines it (Ruling R6,
//!   `usd_write::SPHERE_INTENSITY_PER_CANDELA`): `inputs:intensity` ×
//!   2^`inputs:exposure` nits. A normalised DistantLight's is its
//!   illuminance in lux, and an unnormalised one's is that times the
//!   light's `sizeFactor` for its `inputs:angle`. A normalised SphereLight's
//!   luminous intensity is that / 4 candela, and an unnormalised one's that
//!   × π × `inputs:radius`², the radius in metres. That is the default,
//!   `ReadOptions::usd_light_units` = `LightUnits::Photometric`.
//! - With `LightUnits::Blender` a light that has no litegen customData is
//!   read as Blender's USD importer reads it (usd_reader_light.cc L134-L148,
//!   Blender v5.2.0 and v5.2.3): its `energy` is `inputs:intensity` ×
//!   2^`inputs:exposure` (Ruling L3: the photometric reading folds the
//!   exposure in too) × π for a SphereLight and × 4 for a DistantLight,
//!   and the scene's light is the glTF light whose Blender import gives
//!   that energy (`scene::from_blender_energy`): candela = 170.75 × that
//!   intensity, lux = 2732 ×. `normalize`, `inputs:radius`, `inputs:angle`
//!   and the cone do not enter it: Blender's importer sends them to other
//!   fields of its light, so they are ignored here, where the photometric
//!   reading scales by them. The schema's fallbacks (1 for a SphereLight,
//!   50000 for a DistantLight) apply first. A value past f32 is dropped
//!   with the usual warning, as under `Photometric`, never clamped
//!   (Ruling L5). Without customData, a file written with `Blender` stores
//!   its energy over π in an f32, so a candela at exactly `f32::MAX` can
//!   read back one ulp over and is then dropped (Ruling L5). The litegen
//!   customData wins in both units.
//! - A spot's outer cone is its cone angle, and its inner cone that × (1 −
//!   `inputs:shaping:cone:softness`). A softness of 0, ShapingAPI's
//!   fallback, is a hard edge, softened (`Light::soften_hard_edge`).
//! - No light has a range: UsdLux has none.
//! - A perspective camera's yfov is 2·atan(verticalAperture /
//!   (2·focalLength)) and its aspect ratio horizontalAperture /
//!   verticalAperture. An orthographic camera's xmag and ymag are its
//!   apertures / 20 (`usd_write::APERTURE_PER_MAG`). The clipping range
//!   gives znear and zfar. Lengths come out in metres. An aperture offset
//!   is warned, and the view read centred.
//! - An unauthored attribute takes the schema's fallback. An animated one is
//!   read at its first sample, warned: the scene animates no light or camera
//!   property.
//!
//! A reader never refuses a stage over a light or camera: one that does not
//! read or breaks a bound (`Light::check`, `Camera::check`) is dropped with a
//! warning naming its prim and the bound, and its node stays. A light or
//! camera prim whose transform does not read is dropped whole, warned, with
//! what is under it, which is not read (Ruling R21), as every light and camera
//! prim was skipped before the scene carried them; an Xform's still refuses
//! the stage. Depth of field and a camera's exposure are not read (the spec's
//! non-goals).
//!
//! # What is not carried, and said so
//!
//! Lights that are not punctual (`RectLight`, `DiskLight`, `CylinderLight`,
//! `DomeLight`, ...), curves other than linear `BasisCurves` (cubic ones,
//! `NurbsCurves`, `HermiteCurves`), non-mesh gprims (`Cube`, `Sphere`, ...),
//! point instancers, guide and proxy purposes, invisible prims, vertex
//! animation (time-sampled `points`), blend-shape inbetweens, and shading
//! networks other than `UsdPreviewSurface` (MaterialX, MDL): each is a warning
//! naming the prim. A texture channel layout glTF cannot express (roughness and
//! metallic in two different images, opacity from a different image than the
//! base colour) keeps the factor and drops the texture, with a warning, because
//! splitting or packing images would mean re-encoding them. A clearcoat
//! input is the exception: a clearcoat or clearcoat roughness read from
//! another channel than glTF's (R and G), through a map that biases it, from
//! another shader, or outside 0 to 1 (NaN and infinities included) is left
//! out whole, value and map alike, with a warning, and reads as unauthored:
//! no clearcoat, or USD's roughness of 0.01. So is an ior KHR_materials_ior
//! cannot hold (it takes 0, or 1 and above), or one that comes from a
//! shader. A clearcoat (a factor above 0) beside an ior other than 1.5 is
//! kept, with a warning: USD's clearcoat reflects with that ior, and glTF's
//! with 1.5. An unconnected `clearcoat` of 0 and an `ior` of exactly 1.5,
//! which three.js's USDZExporter writes on every physical material, draw as
//! their absence does and read as unset, silently (Ruling A41).
//!
//! Every warning is clipped to `MAX_WARNING_CHARS` as it is pushed, and a
//! read keeps `MAX_WARNINGS` of them (Ruling A45).
//!
//! Ruling IDs ("Ruling A41") are mesh-gaps item 4's rulings, listed with
//! their costs in the status block of
//! `docs/superpowers/plans/2026-11-04-mesh-material-layers.md`. "Ruling R6"
//! and its kin are item 5's (lights and cameras), listed with their costs
//! in the "Rulings" section of
//! `docs/superpowers/plans/2026-11-04-mesh-lights-cameras.md`.

use std::collections::HashMap;
use std::rc::Rc;

use super::math::{self, Mat4};
use super::scene::{
    from_blender_energy, AlphaMode, AnimPath, Animation, Camera, Channel, Image, ImageFormat, Interpolation, Light, LightKind, Material,
    MorphTarget, Node, Primitive, Projection, Sampler, Scene, SceneMesh, Skin, SkinWeights, Texture, TextureRef,
    Topology, Transform, UvTransform, Wrap, MAX_SCENE_INDICES, MAX_SCENE_VERTICES,
};
use super::normalize::{channel_means, pack_metallic_roughness, MrSource};
use super::usd_stage::{short, split_property, Attr, Stage, Value};
use super::{LightUnits, MeshError};

/// Prims the compositions of a stage's other `Animation` variants may add up
/// to ([`Reader::read_other_clips`]): each variant composes the whole stage
/// again, so the budget is a multiple of one stage's ceiling, charged per
/// variant. A real character's clips cost a few thousand prims each.
const MAX_VARIANT_PRIMS: usize = 2 * super::usd_stage::MAX_PRIMS;
/// Connection hops followed through NodeGraph and Material interfaces.
const MAX_CONNECTION_HOPS: usize = 16;
/// Texture-coordinate sets read per mesh.
const MAX_UV_SETS: usize = 8;
/// Joint influences per point read (rounded up to a multiple of four).
const MAX_INFLUENCES: usize = 32;
/// Blend shapes per mesh.
const MAX_BLEND_SHAPES: usize = super::scene::MAX_MORPH_TARGETS;
/// Resampling rate for animation USD interpolates differently from the scene
/// (Euler angles, sheared stacks): keys this close keep the difference below
/// what a frame shows.
const RESAMPLE_FPS: f64 = 60.0;
/// The customData key the writer stores an original name under when the
/// prim name had to be sanitised.
pub const NAME_KEY: &str = "originalName";
/// The customData key of the dictionary in which the writer keeps a
/// light's or camera's own values, for an exact round trip (module docs,
/// "Lights and cameras").
pub const LITEGEN_KEY: &str = "litegen";
/// The entry of a Material prim's `litegen` dictionary that marks it unlit
/// (`bool unlit = 1`): `usd_write::write_material` writes it beside the
/// recipe that draws unlit, and the reader reads that recipe back as unlit
/// (glTF's `KHR_materials_unlit`) only when it is there.
pub const UNLIT_KEY: &str = "unlit";

/// A blend shape's point offsets, and its normal offsets when it has them,
/// one per mesh point.
type ShapeOffsets = (Vec<[f32; 3]>, Option<Vec<[f32; 3]>>);

fn malformed(detail: impl Into<String>) -> MeshError {
    MeshError::Malformed(format!("usd: {}", detail.into()))
}

fn unsupported(detail: impl Into<String>) -> MeshError {
    MeshError::Unsupported(format!("usd: {}", detail.into()))
}

pub type FetchBytes<'a> = dyn FnMut(&str) -> Option<Vec<u8>> + 'a;

/// Read a composed stage into a scene. `fetch` supplies textures by package
/// path. `light_units` is how the strength of a light that has no litegen
/// `customData` is read (module docs, "Lights and cameras").
pub fn read_stage(stage: &Stage, fetch: &mut FetchBytes<'_>, light_units: LightUnits) -> Result<(Scene, Vec<String>), MeshError> {
    let tcps = stage
        .meta("timeCodesPerSecond")
        .or_else(|| stage.meta("framesPerSecond"))
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(24.0);
    let mut r = Reader {
        st: stage,
        fetch,
        scene: Scene::default(),
        warnings: stage.warnings.clone(),
        tcps,
        mpu: meters_per_unit(stage).0,
        light_units,
        node_of: HashMap::new(),
        read_materials: HashMap::new(),
        materials: HashMap::new(),
        images: HashMap::new(),
        textures: HashMap::new(),
        skeletons: Vec::new(),
        skeleton_of: HashMap::new(),
        joint_anims: HashMap::new(),
        skinned: Vec::new(),
        morphs: Vec::new(),
        channels: Vec::new(),
        anim_names: Vec::new(),
        total_vertices: 0,
        total_indices: 0,
        morph_values: 0,
        skin_entries: 0,
        anim_keys: 0,
        prune_textures: false,
    };
    r.walk()?;
    r.bind_skeletons()?;
    r.finish_animation()?;
    r.read_other_clips()?;
    r.up_axis_and_units()?;
    let Reader { mut scene, warnings, prune_textures, .. } = r;
    if prune_textures {
        // The separate maps a packed one replaced, and the textures of a
        // dropped binding, unless something else uses them.
        super::pose::prune_unused_textures(&mut scene);
    }
    bake_uniform_texture_transforms(&mut scene);
    scene.validate()?;
    Ok((scene, warnings))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interp {
    Constant,
    Uniform,
    Vertex,
    FaceVarying,
}

/// A primvar's data with its interpolation and optional indices, checked
/// against the mesh topology.
struct Primvar {
    values: Vec<f32>,
    width: usize,
    interp: Interp,
    indices: Option<Vec<usize>>,
}

impl Primvar {
    /// The value index for face `f`, point `p`, face-vertex `fv`.
    fn index(&self, f: usize, p: usize, fv: usize) -> usize {
        let e = match self.interp {
            Interp::Constant => 0,
            Interp::Uniform => f,
            Interp::Vertex => p,
            Interp::FaceVarying => fv,
        };
        match &self.indices {
            Some(ix) => ix[e],
            None => e,
        }
    }

    fn get(&self, i: usize) -> &[f32] {
        &self.values[i * self.width..(i + 1) * self.width]
    }

    /// Whether IR vertices can be the USD points one for one.
    fn per_point(&self) -> bool {
        matches!(self.interp, Interp::Constant | Interp::Vertex)
    }
}

/// Raw skinning data of a mesh, attached once skeletons are known.
struct SkinnedMesh {
    node: usize,
    mesh: usize,
    prim: usize,
    /// Per primitive: the USD point each IR vertex came from.
    point_maps: Vec<Vec<u32>>,
    joint_indices: Vec<i64>,
    joint_weights: Vec<f32>,
    element_size: usize,
    constant: bool,
    point_count: usize,
}

/// A mesh with blend shapes, waiting for its skeleton's weights.
struct MorphMesh {
    node: usize,
    mesh: usize,
    prim: usize,
    /// `skel:blendShapes` names, in target order.
    names: Vec<String>,
}

struct SkeletonInfo {
    prim: usize,
    /// IR node per joint, in the skeleton's joint order.
    joint_nodes: Vec<usize>,
    joint_paths: Vec<String>,
    bind: Vec<Mat4>,
    /// The bound SkelAnimation, if any.
    anim: Option<usize>,
    /// Whether the skeleton sits under a SkelRoot (otherwise UsdSkel ignores it).
    rooted: bool,
}

struct Reader<'s, 'f> {
    st: &'s Stage,
    fetch: &'f mut FetchBytes<'f>,
    scene: Scene,
    warnings: Vec<String>,
    tcps: f64,
    /// The stage's metres per unit ([`meters_per_unit`]): a camera's
    /// clipping range and window, and a light's radius and range, come out
    /// in metres through it, as the root transforms do in
    /// [`Reader::up_axis_and_units`].
    mpu: f64,
    /// The units a light without litegen `customData` is read in
    /// ([`Reader::light_of`]).
    light_units: LightUnits,
    node_of: HashMap<usize, usize>,
    /// (material prim, doubleSided) → the material as read, its texture
    /// bindings numbered in its own read order, and what it reads.
    read_materials: HashMap<(usize, bool), (Material, MaterialReads)>,
    /// (material prim, doubleSided, the mesh set each of its reads lands on)
    /// → IR material: one per layout of the meshes bound to it
    /// ([`Reader::bind_material`]).
    materials: HashMap<(usize, bool, Vec<Option<u32>>), usize>,
    images: HashMap<String, Option<usize>>,
    /// (image, wrapS, wrapT) → texture. `Wrap` is not `Hash`; its code is.
    textures: HashMap<(usize, u8, u8), usize>,
    skeletons: Vec<SkeletonInfo>,
    /// Skeleton prim → its first place in `skeletons`: a mesh's skeleton is
    /// found through it, not by scanning the list for each mesh and clip.
    skeleton_of: HashMap<usize, usize>,
    /// The walked stage's SkelAnimations as [`JointAnim`]s, each read once
    /// for every skeleton bound to it.
    joint_anims: HashMap<usize, Rc<JointAnim>>,
    skinned: Vec<SkinnedMesh>,
    morphs: Vec<MorphMesh>,
    channels: Vec<Channel>,
    /// The names of the SkelAnimations the walk binds, up to two distinct
    /// ones: a single name names the clip ([`Reader::finish_animation`]).
    anim_names: Vec<String>,
    total_vertices: usize,
    total_indices: usize,
    /// Morph-target floats built so far (`scene::MAX_MORPH_VALUES`).
    morph_values: usize,
    /// Joint entries across the skins built so far (`scene::MAX_SKIN_ENTRIES`).
    skin_entries: usize,
    /// Animation keys built so far: densified ones, and every joint and
    /// blend-shape channel, charged before it is copied ([`Reader::charge_keys`]).
    anim_keys: usize,
    /// Textures may have been left unused: a metallic-roughness map was packed
    /// from separate ones, or a binding was dropped with a set past
    /// `MAX_UV_SETS`.
    prune_textures: bool,
}

/// What a material reads from the mesh beyond positions and normals.
#[derive(Debug, Clone, Default)]
struct MaterialReads {
    /// Texture-coordinate primvar names, in the order its textures first
    /// reference them: the numbering of the material's own bindings, until a
    /// mesh's sets renumber them by name ([`Reader::uv_set_names`]).
    uv_names: Vec<String>,
    /// A float3 primvar feeding the diffuse colour (vertex colours).
    color: Option<String>,
    /// A float primvar feeding opacity.
    opacity: Option<String>,
}

/// The segments of linear curves with these vertex `counts`: a curve of `c`
/// vertices has `c - 1`, or `c` when it is periodic (the last joins the
/// first), and a curve of fewer than 2 has none. A saturating sum, so counts
/// as large as `i64` allows are many segments, never a wrapped few.
pub(super) fn segment_count(counts: &[i64], periodic: bool) -> usize {
    counts
        .iter()
        .map(|&c| usize::try_from(c).unwrap_or(if c < 0 { 0 } else { usize::MAX }))
        .filter(|&c| c >= 2)
        .fold(0usize, |sum, c| sum.saturating_add(if periodic { c } else { c - 1 }))
}

/// The material a gprim that binds none is read with: Hydra draws it with its
/// displayColor (`from_primvar`, white here since the colours are the
/// primvar) or a fallback grey, lit as a plain diffuse surface, which is
/// UsdPreviewSurface's defaults. Single-sided; a prim that says `doubleSided`
/// is read with that set. Only another tool's file leaves a gprim unbound:
/// this writer binds a material to every primitive (the default one when the
/// scene names none), so what it writes never reads back as this.
fn unbound_material(from_primvar: bool) -> Material {
    let grey = 0.18;
    Material {
        name: Some("displayColor".into()),
        base_color: if from_primvar { [1.0; 4] } else { [grey, grey, grey, 1.0] },
        metallic: 0.0,
        roughness: 0.5,
        ..Default::default()
    }
}

/// Distinct warnings a read keeps; past this the rest are not listed.
const MAX_WARNINGS: usize = 500;
/// Characters a warning keeps, then "…" (Ruling A45): far past any this
/// reader writes, whose quoted paths `short` bounds, so only a name the file
/// spells at length (a shader output, a primvar) is cut.
const MAX_WARNING_CHARS: usize = 1000;

/// `msg`, clipped to [`MAX_WARNING_CHARS`] as it is pushed, unless the read
/// has its [`MAX_WARNINGS`] or already said the same.
fn warn_push(w: &mut Vec<String>, msg: String) {
    let msg = match msg.char_indices().nth(MAX_WARNING_CHARS) {
        Some((at, _)) => format!("{}…", &msg[..at]),
        None => msg,
    };
    if w.len() < MAX_WARNINGS && !w.contains(&msg) {
        w.push(msg);
    }
}

impl<'s, 'f> Reader<'s, 'f> {
    fn warn(&mut self, msg: String) {
        warn_push(&mut self.warnings, msg);
    }

    fn path(&self, p: usize) -> &'s str {
        &self.st.prims[p].path
    }

    fn type_of(&self, p: usize) -> &'s str {
        &self.st.prims[p].type_name
    }

    fn token(&self, p: usize, name: &str) -> Option<String> {
        self.st.attr(p, name).and_then(|a| a.rest().and_then(Value::as_str).map(str::to_string))
    }

    /// A shader input's string value, through any connection to a material
    /// or node-graph interface input (`inputs:varname.connect =
    /// </Mat.inputs:frame:stPrimvarName>`, as Blender and Reality Composer
    /// write it): what [`Reader::resolve`] does for every other input.
    fn input_token(&self, p: usize, name: &str) -> Option<String> {
        match self.resolve(p, name) {
            Resolved::Value(v, _) => v.as_str().map(str::to_string),
            _ => None,
        }
    }

    fn display_name(&self, p: usize) -> String {
        self.st.custom_string(p, NAME_KEY).unwrap_or_else(|| self.st.prims[p].name.clone())
    }

    fn seconds(&self, code: f64) -> f32 {
        (code / self.tcps) as f32
    }

    // ── namespace walk ──────────────────────────────────────────────────────

    fn walk(&mut self) -> Result<(), MeshError> {
        let st = self.st;
        // The default prim is the asset; a transform-less group there is the
        // scene itself, and its children are the scene's roots.
        let default = st
            .meta("defaultPrim")
            .and_then(Value::as_str)
            .and_then(|n| st.find(&format!("/{n}")));
        if let Some(d) = default {
            self.scene.name = Some(self.display_name(d));
        }
        // Which prims hold something drawn or placed, a camera or a light the
        // scene carries (prims are stored parents first, so one reverse pass
        // sees every child before its parent). A Scope or untyped prim with
        // nothing under it — the usual "Materials" scope — becomes no node,
        // rather than an empty one.
        let n = st.prims.len();
        let mut content = vec![false; n];
        for p in (0..n).rev() {
            let prim = &st.prims[p];
            if !prim.is_drawable() {
                continue;
            }
            let own = !matches!(
                prim.type_name.as_str(),
                "" | "Scope" | "Material" | "Shader" | "NodeGraph" | "GeomSubset" | "BlendShape" | "SkelAnimation"
                    | "RectLight" | "DiskLight" | "CylinderLight" | "DomeLight" | "DomeLight_1" | "GeometryLight"
                    | "PortalLight" | "PluginLight" | "RenderSettings" | "RenderProduct" | "RenderVar" | "RenderPass"
                    | "PhysicsScene"
            );
            content[p] = own || prim.children.iter().any(|c| content.get(*c).copied().unwrap_or(false));
        }
        let mut todo: Vec<(usize, Option<usize>)> = Vec::new();
        for &root in st.roots.iter().rev() {
            if Some(root) == default && self.is_plain_group(root) {
                for &c in st.prims[root].children.iter().rev() {
                    todo.push((c, None));
                }
            } else {
                todo.push((root, None));
            }
        }
        while let Some((p, parent)) = todo.pop() {
            let prim = &st.prims[p];
            if !prim.is_drawable() {
                continue;
            }
            let ty = prim.type_name.as_str();
            if matches!(ty, "" | "Scope") && !content[p] {
                continue;
            }
            match ty {
                "Material" | "Shader" | "NodeGraph" | "GeomSubset" | "BlendShape" | "SkelAnimation" => continue,
                "RenderSettings" | "RenderProduct" | "RenderVar" | "RenderPass" | "PhysicsScene" => continue,
                "RectLight" | "DiskLight" | "CylinderLight" | "DomeLight" | "DomeLight_1" | "GeometryLight" | "PortalLight"
                | "PluginLight" => {
                    self.warn(format!(
                        "{ty} {} is not carried: the scene holds directional, point and spot lights only",
                        short(self.path(p))
                    ));
                    continue;
                }
                "PointInstancer" => {
                    self.warn(format!("point instancer {} is not carried; its instances are missing", short(self.path(p))));
                    continue;
                }
                _ => {}
            }
            // A skeleton's visibility and purpose govern drawing its bones, not
            // its job: an invisible proxy Skeleton (Apple's glTF importer
            // writes exactly that) still poses and skins its meshes.
            if ty != "Skeleton" {
                if let Some(why) = self.hidden(p) {
                    self.warn(format!("{} is {why} and is not carried", short(self.path(p))));
                    continue;
                }
            }
            if ty == "Skeleton" && self.is_plain_group(p) {
                // A transform-less skeleton is a container for its joints,
                // which hang directly where it stands.
                self.read_skeleton(p, parent)?;
                for &c in st.prims[p].children.iter().rev() {
                    todo.push((c, parent));
                }
                continue;
            }
            let node = if matches!(ty, "DistantLight" | "SphereLight" | "Camera") {
                match self.light_or_camera_node(p, parent)? {
                    Some(node) => node,
                    // Dropped and warned, with what is under it (Ruling R21).
                    None => continue,
                }
            } else {
                self.make_node(p, parent)?
            };
            match ty {
                "Mesh" => self.read_mesh(p, node)?,
                "Points" => self.read_points(p, node)?,
                "BasisCurves" => self.read_curves(p, node)?,
                "Skeleton" => self.read_skeleton(p, Some(node))?,
                "DistantLight" | "SphereLight" => self.read_light(p, node),
                "Camera" => self.read_camera(p, node),
                "NurbsCurves" | "HermiteCurves" | "NurbsPatch" | "Cube" | "Sphere"
                | "Cylinder" | "Cylinder_1" | "Cone" | "Capsule" | "Capsule_1" | "Plane" | "TetMesh" | "Volume" => {
                    self.warn(format!("{ty} {} is not a mesh; its geometry is not carried", short(self.path(p))));
                }
                _ => {}
            }
            for &c in st.prims[p].children.iter().rev() {
                todo.push((c, Some(node)));
            }
        }
        Ok(())
    }

    /// [`Reader::make_node`] for a light or camera prim. A reader never
    /// refuses a stage over a light or camera (Ruling R21), so when the
    /// prim's transform does not read, it is dropped with a warning naming it
    /// and why, and the channels and warnings the attempt recorded are
    /// undone. The animation keys it charged stay charged (Ruling R22).
    /// `None` then: the walk skips the prim and what is under it unread, as
    /// it skipped every light and camera prim before the scene carried them.
    /// The node budget still refuses the stage, as it does for any prim.
    fn light_or_camera_node(&mut self, p: usize, parent: Option<usize>) -> Result<Option<usize>, MeshError> {
        if self.scene.nodes.len() >= super::scene::MAX_NODES {
            return self.make_node(p, parent).map(Some);
        }
        let (channels, warnings) = (self.channels.len(), self.warnings.len());
        let e = match self.make_node(p, parent) {
            Ok(node) => return Ok(Some(node)),
            Err(e) => e,
        };
        // `anim_keys` is not given back (Ruling R22). A stack that is not TRS
        // can fail at its last densified key, after every key is computed. A
        // failed prim makes no node, so with the charge returned, every
        // failing light would pay a whole budget again, which is the
        // amplification densify's budget closed (2026-10-12 review, finding
        // 11). Kept, the charge makes a later one get densify's refusal
        // before anything is allocated, and it is dropped like any other.
        // The cost, accepted as for MAX_NODES: a hostile dropped light can
        // use the budget up, so a later Xform whose animation must be
        // densified refuses the stage.
        self.channels.truncate(channels);
        self.warnings.truncate(warnings);
        let why = match &e {
            MeshError::Malformed(d) | MeshError::Unsupported(d) => d.strip_prefix("usd: ").unwrap_or(d).to_string(),
            other => other.to_string(),
        };
        let kind = if self.type_of(p) == "Camera" { "camera" } else { "light" };
        let under = if self.st.prims[p].children.iter().any(|&c| self.st.prims[c].is_drawable()) { "; the prims under it go with it" } else { "" };
        self.warn(format!("{kind} {} is dropped: its transform does not read ({why}){under}", short(self.path(p))));
        Ok(None)
    }

    /// An untransformed, unanimated group with no geometry of its own.
    fn is_plain_group(&self, p: usize) -> bool {
        matches!(self.type_of(p), "" | "Xform" | "Scope" | "SkelRoot" | "Skeleton")
            && self.st.attr(p, "xformOpOrder").and_then(|a| a.rest().and_then(Value::strs).map(|v| v.is_empty())).unwrap_or(true)
            && (self.type_of(p) == "Skeleton" || self.hidden(p).is_none())
    }

    /// Why a prim (and its subtree) is not drawn, if it is not.
    fn hidden(&self, p: usize) -> Option<&'static str> {
        if self.token(p, "visibility").as_deref() == Some("invisible") {
            return Some("invisible");
        }
        match self.token(p, "purpose").as_deref() {
            Some("guide") => Some("guide geometry"),
            Some("proxy") => Some("proxy geometry (the render representation is read instead)"),
            _ => None,
        }
    }

    fn make_node(&mut self, p: usize, parent: Option<usize>) -> Result<usize, MeshError> {
        if self.scene.nodes.len() >= super::scene::MAX_NODES {
            return Err(unsupported(format!("the stage has more than {} nodes", super::scene::MAX_NODES)));
        }
        let transform = self.xform(p, self.scene.nodes.len())?;
        let node = Node { name: Some(self.display_name(p)), transform, ..Default::default() };
        let i = self.scene.nodes.len();
        self.scene.nodes.push(node);
        match parent {
            Some(par) => self.scene.nodes[par].children.push(i),
            None => self.scene.roots.push(i),
        }
        self.node_of.insert(p, i);
        Ok(i)
    }

    // ── transforms ──────────────────────────────────────────────────────────

    /// The prim's local transform, recording channels for animated ops.
    fn xform(&mut self, p: usize, node: usize) -> Result<Transform, MeshError> {
        let st = self.st;
        let Some(order) = st.attr(p, "xformOpOrder") else { return Ok(Transform::default()) };
        let Some(names) = order.rest().and_then(Value::strs).map(|v| v.to_vec()) else {
            return Ok(Transform::default());
        };
        let mut ops: Vec<(String, bool, Attr)> = Vec::new();
        for raw in &names {
            if raw == "!resetXformStack!" {
                self.warn(format!(
                    "{} resets the transform stack; its transform is read relative to its parent instead",
                    short(self.path(p))
                ));
                continue;
            }
            let (name, invert) = match raw.strip_prefix("!invert!") {
                Some(n) => (n.to_string(), true),
                None => (raw.clone(), false),
            };
            let Some(a) = st.attr(p, &name) else {
                return Err(malformed(format!("{} lists {name:?} in xformOpOrder but has no such attribute", short(self.path(p)))));
            };
            ops.push((name, invert, a));
        }
        let rest = compose_ops(&ops, None).map_err(|e| malformed(format!("{}: {e}", short(self.path(p)))))?;
        if !rest.iter().all(|v| v.is_finite()) {
            return Err(malformed(format!("{} has a non-finite transform", short(self.path(p)))));
        }
        // The common translate/orient/scale stack IS a TRS: read it as one,
        // exactly, rather than through a matrix — a zero scale (a part that
        // pops in) has no decomposition, and an animated node must stay TRS.
        let rest_t = match trs_of(&ops) {
            Some(trs) => trs,
            None => Transform::from_matrix(rest),
        };
        if ops.iter().any(|(_, _, a)| a.animated()) {
            self.animate_ops(p, node, &ops)?;
        }
        Ok(rest_t)
    }

    fn animate_ops(&mut self, p: usize, node: usize, ops: &[(String, bool, Attr)]) -> Result<(), MeshError> {
        // The common TRS stack maps op for op onto channels, keeping each op's
        // own keys: USD interpolates translate and scale linearly and orient by
        // slerp, exactly as the scene does.
        if trs_shaped(ops) {
            for (name, _, a) in ops {
                if !a.animated() {
                    continue;
                }
                let (path, width) = match op_kind(name) {
                    "translate" => (AnimPath::Translation, 3),
                    "orient" => (AnimPath::Rotation, 4),
                    _ => (AnimPath::Scale, 3),
                };
                let mut times = Vec::with_capacity(a.samples.len());
                let mut values = Vec::with_capacity(a.samples.len() * width);
                for (t, v) in &a.samples {
                    let Some(tuple) = v.as_tuple().filter(|t| t.len() == width) else {
                        return Err(malformed(format!("{name} on {} has a sample of the wrong shape", short(self.path(p)))));
                    };
                    times.push(self.seconds(*t));
                    values.extend(tuple.iter().map(|x| *x as f32));
                }
                self.push_channel(Channel { node, path, interpolation: Interpolation::Linear, times, values });
            }
            return Ok(());
        }
        // Anything else is sampled as a whole at every op key, densified where
        // USD's per-op interpolation (Euler angles, matrices) differs from
        // decomposed TRS.
        let mut times: Vec<f64> = ops.iter().flat_map(|(_, _, a)| a.samples.iter().map(|(t, _)| *t)).collect();
        times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        times.dedup();
        let budget = super::scene::MAX_ANIMATION_KEYS.saturating_sub(self.anim_keys) / 3;
        let dense = densify(&times, self.tcps, budget)?;
        self.anim_keys = self.anim_keys.saturating_add(dense.len().saturating_mul(3));
        let mut t_out = Vec::with_capacity(dense.len());
        let (mut tr, mut ro, mut sc) = (Vec::new(), Vec::new(), Vec::new());
        let mut prev_q: Option<[f64; 4]> = None;
        for t in dense {
            let m = compose_ops(ops, Some(t)).map_err(|e| malformed(format!("{}: {e}", short(self.path(p)))))?;
            let Some((tt, mut q, s)) = math::decompose_near(&m, prev_q) else {
                self.warn(format!("{} animates a sheared transform, which the scene cannot hold; it is held at rest", short(self.path(p))));
                return Ok(());
            };
            if let Some(pq) = prev_q {
                if math::quat_dot(pq, q) < 0.0 {
                    q = [-q[0], -q[1], -q[2], -q[3]];
                }
            }
            prev_q = Some(q);
            t_out.push(self.seconds(t));
            tr.extend(tt.iter().map(|v| *v as f32));
            ro.extend(q.iter().map(|v| *v as f32));
            sc.extend(s.iter().map(|v| *v as f32));
        }
        for (path, values) in [(AnimPath::Translation, tr), (AnimPath::Rotation, ro), (AnimPath::Scale, sc)] {
            self.push_channel(Channel { node, path, interpolation: Interpolation::Linear, times: t_out.clone(), values });
        }
        Ok(())
    }

    fn push_channel(&mut self, mut ch: Channel) {
        // Equal times after the f32 rounding of seconds collapse to one key.
        let stride = if ch.times.is_empty() { 0 } else { ch.values.len() / ch.times.len() };
        let mut keep_t = Vec::with_capacity(ch.times.len());
        let mut keep_v = Vec::with_capacity(ch.values.len());
        for (i, t) in ch.times.iter().enumerate() {
            if keep_t.last().is_some_and(|l: &f32| *t <= *l) {
                continue;
            }
            keep_t.push(*t);
            keep_v.extend_from_slice(&ch.values[i * stride..(i + 1) * stride]);
        }
        ch.times = keep_t;
        ch.values = keep_v;
        if !ch.times.is_empty() {
            self.channels.push(ch);
        }
    }

    // ── lights and cameras ──────────────────────────────────────────────────

    /// The prim's `customData.litegen` entry (`LITEGEN_KEY`), from its
    /// strongest opinion that authors one, as `Stage::custom_string` takes a
    /// name.
    fn litegen_data(&self, p: usize) -> Option<&'s Value> {
        litegen_dict(self.st, p)
    }

    /// Whether prim `p` applies the API schema `api`: its `apiSchemas` list
    /// ops composed from the weakest opinion to the strongest, as
    /// `ListOp::apply` composes them (delete, then add, prepend and
    /// append), following `api` alone, so a long list costs one pass.
    fn applies(&self, p: usize, api: &str) -> bool {
        let st = self.st;
        let mut applied = false;
        for s in st.prims[p].sources.iter().rev() {
            let op = st.layers.get(s.layer).and_then(|l| l.specs.get(&s.path)).and_then(|spec| spec.api_schemas.as_ref());
            let Some(op) = op else { continue };
            let has = |items: &[String]| items.iter().any(|x| x == api);
            applied = match &op.explicit {
                Some(items) => has(items),
                None => (applied && !has(&op.delete)) || has(&op.add) || has(&op.prepend) || has(&op.append),
            };
        }
        applied
    }

    /// Attribute `name` of light or camera prim `p` at rest: its first
    /// sample when it is animated, which `notes` records (the scene
    /// animates no light or camera property), or `None` when it is
    /// unauthored or blocked, for the schema's fallback.
    fn lc_value(&self, p: usize, name: &str, notes: &mut Vec<String>) -> Option<Value> {
        let a = self.st.attr(p, name)?;
        if a.animated() {
            notes.push(format!("animates its {name}, which the scene does not carry; its first sample is used"));
        }
        a.rest().cloned()
    }

    /// [`Reader::lc_value`] as one number, or why it is not one, worded to
    /// follow `light /R/lamp is dropped: `.
    fn lc_number(&self, p: usize, name: &str, notes: &mut Vec<String>) -> Result<Option<f64>, String> {
        match self.lc_value(p, name, notes) {
            None => Ok(None),
            Some(v) => v.as_f64().map(Some).ok_or_else(|| format!("its {name} is not a number")),
        }
    }

    /// [`Reader::lc_value`] as `N` numbers, or why it is not.
    fn lc_numbers<const N: usize>(&self, p: usize, name: &str, notes: &mut Vec<String>) -> Result<Option<[f64; N]>, String> {
        match self.lc_value(p, name, notes) {
            None => Ok(None),
            Some(v) => match v.as_tuple().filter(|t| t.len() == N) {
                Some(t) => {
                    let mut out = [0.0; N];
                    out.copy_from_slice(t);
                    Ok(Some(out))
                }
                None => Err(format!("its {name} is not {N} numbers")),
            },
        }
    }

    /// [`Reader::lc_value`] as a bool, or why it is not one.
    fn lc_bool(&self, p: usize, name: &str, notes: &mut Vec<String>) -> Result<Option<bool>, String> {
        match self.lc_value(p, name, notes) {
            None => Ok(None),
            Some(v) => v.as_bool().map(Some).ok_or_else(|| format!("its {name} is not a bool")),
        }
    }

    /// Light prim `p` on node `node` ([`Reader::light_of`]). A reader never
    /// refuses a stage over a light: one that does not read or breaks a
    /// bound (`Light::check`) is dropped with a warning naming its prim and
    /// why, and its node stays; a hard-edged spot is softened instead
    /// (`Light::soften_hard_edge`). A light rides on its prim's node, which
    /// `make_node` charged to `MAX_NODES` before building it, so it holds
    /// a handful of values per node; its warnings go through `warn_push`'s
    /// cap.
    fn read_light(&mut self, p: usize, node: usize) {
        let label = format!("light {}", short(self.path(p)));
        let read = self.light_of(p).and_then(|(mut l, mut notes)| {
            notes.extend(l.soften_hard_edge());
            l.check().map(|()| (l, notes))
        });
        match read {
            Ok((l, notes)) => {
                for n in notes {
                    self.warn(format!("{label} {n}"));
                }
                self.scene.lights.push(l);
                self.scene.nodes[node].light = Some(self.scene.lights.len() - 1);
            }
            Err(why) => self.warn(format!("{label} is dropped: {why}")),
        }
    }

    /// Light prim `p`, a DistantLight or a SphereLight, as the scene's
    /// light, with what reading it should warn of (each worded to follow
    /// `light /R/lamp `), or why it does not read. Its bounds are
    /// `Light::check`'s to judge. The litegen dictionary gives it exactly
    /// when it reads ([`exact_light`]); otherwise the UsdLux attributes give
    /// it by the documented mapping (module docs, "Lights and cameras").
    fn light_of(&self, p: usize) -> Result<(Light, Vec<String>), String> {
        let mut notes = Vec::new();
        let distant = self.type_of(p) == "DistantLight";
        // A SphereLight that authors ShapingAPI's cone angle is a spot, as
        // Blender's importer reads one (usd_reader_light.cc L101-L118, v5.2.3),
        // and so is one that applies ShapingAPI with none: an unauthored
        // attribute takes the schema's fallback, here 90° (L735), a
        // hemisphere.
        let cone = if distant {
            None
        } else {
            let authored = self.lc_number(p, "inputs:shaping:cone:angle", &mut notes)?;
            authored.or_else(|| self.applies(p, "ShapingAPI").then_some(90.0))
        };
        let mut kind = match (distant, cone) {
            (true, _) => LightKind::Directional,
            (false, None) => LightKind::Point,
            (false, Some(_)) => LightKind::Spot { inner_cone: 0.0, outer_cone: 0.0 },
        };
        if let Some(data) = self.litegen_data(p) {
            match exact_light(data, kind, self.mpu) {
                Ok((light, more)) => {
                    notes.extend(more);
                    return Ok((light, notes));
                }
                Err(why) => notes.push(format!("has litegen customData that does not read ({why}), so its UsdLux attributes were read")),
            }
        }
        // The schema's fallbacks: white; an intensity of 50000 on a
        // DistantLight, 1 on any other light; no exposure; unnormalised; a
        // 0.53° sun and a sphere of radius 0.5 (usdLux/schema.usda L195,
        // L218, L258, L377, L1095, L1113, L1210).
        let color = self.lc_numbers::<3>(p, "inputs:color", &mut notes)?.unwrap_or([1.0; 3]);
        let intensity = self.lc_number(p, "inputs:intensity", &mut notes)?.unwrap_or(if distant { 50000.0 } else { 1.0 });
        let exposure = self.lc_number(p, "inputs:exposure", &mut notes)?.unwrap_or(0.0);
        // Read in both units, as under `Photometric`: the reader drops a light on any mistyped attribute, `Blender`'s `energy` ignores this one.
        let normalize = self.lc_bool(p, "inputs:normalize", &mut notes)?.unwrap_or(false);
        // Luminance in nits, times 2^exposure (L195-L238), and from it glTF's
        // units: a DistantLight's illuminance is its luminance times its
        // sizeFactor, which a normalised one divides out, leaving its
        // intensity in lux (L306-L337); a sphere of luminance L shows πr² from
        // every side, so its luminous intensity is L·πr², which normalising
        // by its area 4πr² turns into intensity / 4 (Ruling R6,
        // `usd_write::SPHERE_INTENSITY_PER_CANDELA`).
        let mut value = intensity * exposure.exp2();
        match self.light_units {
            LightUnits::Photometric => {
                if distant {
                    if !normalize {
                        value *= distant_size_factor(self.lc_number(p, "inputs:angle", &mut notes)?.unwrap_or(0.53));
                    }
                } else if normalize {
                    value /= f64::from(super::usd_write::SPHERE_INTENSITY_PER_CANDELA);
                } else {
                    let r = self.lc_number(p, "inputs:radius", &mut notes)?.unwrap_or(0.5) * self.mpu;
                    value *= std::f64::consts::PI * r * r;
                }
            }
            // Blender's importer: the `energy` is the intensity (with the
            // exposure folded in, Ruling L3) × π for a sphere and × 4 for a
            // distant light, whatever its `normalize`, `radius` and `angle`
            // (usd_reader_light.cc L138-L146; they go to other fields of its
            // light, L88-L130 and L200-L208), and the scene's light is the
            // glTF light whose Blender import gives that energy. A value
            // past f32 is not clamped: it is dropped below, as under
            // `Photometric` (Ruling L5).
            LightUnits::Blender => {
                let energy = value * super::usd_write::blender_energy_per_intensity(&kind);
                value = from_blender_energy(&kind, energy);
            }
        }
        if let Some(angle) = cone {
            if angle > 90.0 {
                notes.push(format!("has a cone of {}° from its axis, wider than a spot's 90°, so it was read as a point light", angle as f32));
                kind = LightKind::Point;
            } else {
                // Full out to (1 − softness)·angle, none past the angle,
                // softness clamped to 0 to 1 (L735-L797).
                let softness = self.lc_number(p, "inputs:shaping:cone:softness", &mut notes)?.unwrap_or(0.0);
                let outer = angle.to_radians();
                kind = LightKind::Spot { inner_cone: (outer * (1.0 - softness.clamp(0.0, 1.0))) as f32, outer_cone: outer as f32 };
            }
        }
        if self.lc_bool(p, "inputs:enableColorTemperature", &mut notes)?.unwrap_or(false) {
            // 6500 K is white whatever the colour space (L399-L429).
            let kelvin = self.lc_number(p, "inputs:colorTemperature", &mut notes)?.unwrap_or(6500.0);
            if kelvin != 6500.0 {
                notes.push(format!("has a colour temperature of {} K, which the scene does not carry; its colour was read without it", kelvin as f32));
            }
        }
        // What else shapes a UsdLux light that a punctual one has no room
        // for, read without it: an IES profile (the spec's non-goals warn IES
        // lights by name), ShapingAPI's focus, which "values < 0" leave off
        // (L675-L702), and the non-physical diffuse and specular multipliers
        // (L240-L257). An IES profile is ShapingAPI's `inputs:shaping:ies:file`
        // (L799).
        let ies = self.lc_value(p, "inputs:shaping:ies:file", &mut notes).and_then(|v| v.as_str().map(str::to_string));
        if let Some(file) = ies.filter(|f| !f.is_empty()) {
            notes.push(format!("has an IES profile, {}, which the scene does not carry; it was read without it", short(&file)));
        }
        if let Some(focus) = self.lc_number(p, "inputs:shaping:focus", &mut notes)?.filter(|f| *f > 0.0) {
            notes.push(format!("has a shaping focus of {}, which the scene does not carry; it was read without it", focus as f32));
        }
        for what in ["diffuse", "specular"] {
            if let Some(m) = self.lc_number(p, &format!("inputs:{what}"), &mut notes)?.filter(|m| *m != 1.0) {
                notes.push(format!("has a {what} multiplier of {}, which the scene does not carry; it was read without it", m as f32));
            }
        }
        // A colour past 1 moves its peak into the intensity, as Collada's
        // reader splits its colour (Ruling R8): glTF's stays within 0 to 1.
        let peak = color.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let (color, value) = if peak > 1.0 { (color.map(|c| c / peak), value * peak) } else { (color, value) };
        let light = Light { name: Some(self.display_name(p)), kind, color: color.map(|c| c as f32), intensity: value as f32, range: None };
        Ok((light, notes))
    }

    /// Camera prim `p` on node `node` ([`Reader::camera_of`]), kept or
    /// dropped as [`Reader::read_light`] keeps or drops a light.
    fn read_camera(&mut self, p: usize, node: usize) {
        let label = format!("camera {}", short(self.path(p)));
        match self.camera_of(p).and_then(|(c, notes)| c.check().map(|()| (c, notes))) {
            Ok((c, notes)) => {
                for n in notes {
                    self.warn(format!("{label} {n}"));
                }
                self.scene.cameras.push(c);
                self.scene.nodes[node].camera = Some(self.scene.cameras.len() - 1);
            }
            Err(why) => self.warn(format!("{label} is dropped: {why}")),
        }
    }

    /// Camera prim `p` as the scene's camera, with what reading it should
    /// warn of, or why it does not read: the litegen dictionary exactly
    /// when it reads ([`exact_camera`]), otherwise the UsdGeomCamera
    /// attributes by the documented mapping (module docs, "Lights and
    /// cameras").
    fn camera_of(&self, p: usize) -> Result<(Camera, Vec<String>), String> {
        let mut notes = Vec::new();
        let perspective = match self.lc_value(p, "projection", &mut notes) {
            None => true,
            Some(v) => match v.as_str() {
                Some("perspective") => true,
                Some("orthographic") => false,
                Some(other) => return Err(format!("its projection {:?} is not perspective or orthographic", short(other))),
                None => return Err("its projection is not a token".into()),
            },
        };
        if let Some(data) = self.litegen_data(p) {
            match exact_camera(data, perspective, self.mpu) {
                Ok(camera) => return Ok((camera, notes)),
                Err(why) => notes.push(format!("has litegen customData that does not read ({why}), so its UsdGeomCamera attributes were read")),
            }
        }
        // The schema's fallbacks (usdGeom/schema.usda L2352-L2373): the
        // 35 mm projector aperture, a 50 mm lens and a clipping range of 1
        // to 1e6, in tenths of a scene unit and in scene units.
        let width = self.lc_number(p, "horizontalAperture", &mut notes)?.unwrap_or(20.955);
        let height = self.lc_number(p, "verticalAperture", &mut notes)?.unwrap_or(15.2908);
        let clip = self.lc_numbers::<2>(p, "clippingRange", &mut notes)?.unwrap_or([1.0, 1e6]);
        let offset = [
            self.lc_number(p, "horizontalApertureOffset", &mut notes)?.unwrap_or(0.0),
            self.lc_number(p, "verticalApertureOffset", &mut notes)?.unwrap_or(0.0),
        ];
        if offset != [0.0; 2] {
            notes.push(format!(
                "has an aperture offset of ({}, {}), which the scene's cameras cannot hold; its view was read centred",
                offset[0] as f32, offset[1] as f32
            ));
        }
        let (znear, zfar) = ((clip[0] * self.mpu) as f32, (clip[1] * self.mpu) as f32);
        let projection = if perspective {
            // GfCamera's field of view and aspect ratio (gf/camera.cpp
            // L273-L298): ratios, so the units cancel.
            let focal = self.lc_number(p, "focalLength", &mut notes)?.unwrap_or(50.0);
            Projection::Perspective { yfov: (2.0 * (height / (2.0 * focal)).atan()) as f32, aspect: Some((width / height) as f32), znear, zfar: Some(zfar) }
        } else {
            let mag = |aperture: f64| (aperture * self.mpu / super::usd_write::APERTURE_PER_MAG) as f32;
            Projection::Orthographic { xmag: mag(width), ymag: mag(height), znear, zfar }
        };
        Ok((Camera { name: Some(self.display_name(p)), projection }, notes))
    }

    // ── meshes ──────────────────────────────────────────────────────────────

    fn primvar(&mut self, p: usize, attr_name: &str, width: usize, default_interp: Interp, counts: (usize, usize, usize)) -> Option<Primvar> {
        let st = self.st;
        let (src, a) = self.primvar_attr(p, attr_name)?;
        if a.animated() {
            self.warn(format!("{attr_name} on {} is animated; its first sample is used", short(self.path(p))));
        }
        let v = a.rest()?;
        let values: Vec<f32> = v.floats()?.into_owned();
        let w = if width == 0 { 1 } else { width };
        if !values.len().is_multiple_of(w) {
            self.warn(format!("{attr_name} on {} does not hold whole {w}-tuples; ignored", short(self.path(p))));
            return None;
        }
        let interp = match a.interpolation.as_deref() {
            None => default_interp,
            Some("constant") => Interp::Constant,
            Some("uniform") => Interp::Uniform,
            Some("vertex") | Some("varying") => Interp::Vertex,
            Some("faceVarying") => Interp::FaceVarying,
            Some(other) => {
                self.warn(format!("{attr_name} on {} has interpolation {other:?}; ignored", short(self.path(p))));
                return None;
            }
        };
        let (n_faces, n_points, n_fv) = counts;
        let expected = match interp {
            Interp::Constant => 1,
            Interp::Uniform => n_faces,
            Interp::Vertex => n_points,
            Interp::FaceVarying => n_fv,
        };
        let n_values = values.len() / w;
        let indices = match st.attr(src, &format!("{attr_name}:indices")).and_then(|a| a.rest().cloned()) {
            Some(Value::Ints(ix)) if !ix.is_empty() => {
                if ix.len() != expected || ix.iter().any(|i| *i < 0 || *i as usize >= n_values) {
                    self.warn(format!("{attr_name}:indices on {} does not match the mesh; the primvar is ignored", short(self.path(p))));
                    return None;
                }
                Some(ix.iter().map(|i| *i as usize).collect())
            }
            _ => None,
        };
        if indices.is_none() && n_values < expected {
            self.warn(format!(
                "{attr_name} on {} has {n_values} values where its interpolation needs {expected}; ignored",
                short(self.path(p))
            ));
            return None;
        }
        Some(Primvar { values, width: w, interp, indices })
    }

    /// A UsdGeomMesh as IR primitives: `faceVertexCounts`/`faceVertexIndices`
    /// under its `orientation`, holes and GeomSubsets, and each primitive's
    /// primvars by their interpolation (constant, uniform, vertex/varying,
    /// faceVarying), indexed or not. https://openusd.org/release/api/class_usd_geom_mesh.html
    /// (`#UsdGeom_Mesh_Primvars`) and https://openusd.org/release/api/class_usd_geom_primvar.html
    /// (`#Usd_InterpolationVals`); winding, https://openusd.org/release/api/usd_geom_page_front.html
    /// (`#UsdGeom_WindingOrder`).
    fn read_mesh(&mut self, p: usize, node: usize) -> Result<(), MeshError> {
        let st = self.st;
        let path = self.path(p).to_string();
        let points_attr = st.attr(p, "points");
        let Some(points_attr) = points_attr else {
            self.warn(format!("mesh {} has no points", short(&path)));
            return Ok(());
        };
        if points_attr.animated() {
            self.warn(format!("mesh {} animates its points (vertex animation), which is not carried; the first sample is used", short(&path)));
        }
        let points: Vec<f32> = match points_attr.rest().and_then(Value::floats) {
            Some(v) if v.len() % 3 == 0 => v.into_owned(),
            _ => return Err(malformed(format!("mesh {} points are not 3-vectors", short(&path)))),
        };
        let n_points = points.len() / 3;
        let ints = |name: &str| -> Vec<i64> { st.attr(p, name).and_then(|a| a.rest().and_then(Value::ints).map(|v| v.to_vec())).unwrap_or_default() };
        let counts = ints("faceVertexCounts");
        let fv_indices = ints("faceVertexIndices");
        if counts.iter().any(|c| *c < 0) {
            return Err(malformed(format!("mesh {} has a negative face size", short(&path))));
        }
        let total: usize = counts.iter().map(|c| *c as usize).fold(0usize, |a, b| a.saturating_add(b));
        if total != fv_indices.len() {
            return Err(malformed(format!(
                "mesh {} has {} face-vertex indices for faces totalling {total}",
                short(&path),
                fv_indices.len()
            )));
        }
        if let Some(bad) = fv_indices.iter().find(|i| **i < 0 || **i as usize >= n_points) {
            return Err(malformed(format!("mesh {} indexes point {bad} of {n_points}", short(&path))));
        }
        let n_faces = counts.len();
        let left = self.token(p, "orientation").as_deref() == Some("leftHanded");
        if let Some(s) = self.token(p, "subdivisionScheme") {
            if s != "none" && st.attr(p, "subdivisionScheme").is_some_and(|a| a.default.is_some()) {
                self.warn(format!("mesh {} asks for {s} subdivision; its control cage is carried unrefined", short(&path)));
            }
        }
        let mut hole = vec![false; counts.len()];
        for h in ints("holeIndices") {
            if let Some(slot) = usize::try_from(h).ok().and_then(|h| hole.get_mut(h)) {
                *slot = true;
            }
        }
        // Face starts, for face-varying data.
        let mut face_start = Vec::with_capacity(n_faces);
        let mut acc = 0usize;
        for c in &counts {
            face_start.push(acc);
            acc += *c as usize;
        }
        let topo = (n_faces, n_points, fv_indices.len());

        // Faces per material subset; the rest use the mesh's own binding.
        let mut groups: Vec<(Option<usize>, Vec<usize>)> = Vec::new();
        let mut covered = vec![false; n_faces];
        for &c in &st.prims[p].children {
            if self.type_of(c) != "GeomSubset" || !st.prims[c].is_drawable() {
                continue;
            }
            if self.token(c, "familyName").as_deref() != Some("materialBind") {
                continue;
            }
            if self.token(c, "elementType").as_deref().is_some_and(|e| e != "face") {
                continue;
            }
            let faces: Vec<usize> = st
                .attr(c, "indices")
                .and_then(|a| a.rest().and_then(Value::ints).map(|v| v.to_vec()))
                .unwrap_or_default()
                .into_iter()
                .filter_map(|i| usize::try_from(i).ok().filter(|i| *i < n_faces))
                .filter(|f| !std::mem::replace(&mut covered[*f], true))
                .collect();
            if faces.is_empty() {
                continue;
            }
            let binding = self.binding_of(c, false).or_else(|| self.binding_of(p, true));
            groups.push((binding, faces));
        }
        let rest: Vec<usize> = (0..n_faces).filter(|f| !covered[*f]).collect();
        if !rest.is_empty() {
            groups.push((self.binding_of(p, true), rest));
        }
        let normals = self.normals_of(p, topo);
        let mut warned_degenerate = false;
        self.read_geometry(p, node, Topology::Triangles, &points, topo, normals, groups, |this, faces: Vec<usize>| {
            // Triangles, with the corner (face, point, face-vertex) of each: a
            // convex face fanned, a concave one ear-clipped (below).
            let mut corners: Vec<(usize, usize, usize)> = Vec::new();
            let mut tri_count = 0usize;
            let mut fanned_concave = 0usize;
            for &f in &faces {
                if hole[f] {
                    continue;
                }
                let n = counts[f] as usize;
                if n < 3 {
                    if !warned_degenerate {
                        warned_degenerate = true;
                        this.warn(format!("mesh {} has faces with fewer than 3 vertices; they were skipped", short(&path)));
                    }
                    continue;
                }
                tri_count = tri_count.saturating_add(n - 2);
                if this.total_indices.saturating_add(tri_count.saturating_mul(3)) > MAX_SCENE_INDICES {
                    return Err(unsupported(format!("the stage has more than {MAX_SCENE_INDICES} triangle indices")));
                }
                let s = face_start[f];
                let corner = |k: usize| -> (usize, usize, usize) {
                    let k = if left { (n - k) % n } else { k };
                    (f, fv_indices[s + k] as usize, s + k)
                };
                if n == 3 {
                    corners.extend([corner(0), corner(1), corner(2)]);
                    continue;
                }
                // Fanned when convex, ear-clipped when not (`polygon`; three.js's
                // USD loader ear-clips too), in right-handed corner order.
                let order: Vec<(usize, usize, usize)> = (0..n).map(corner).collect();
                let pts: Option<Vec<[f64; 3]>> = order
                    .iter()
                    .map(|&(_, pt, _)| points.get(3 * pt..3 * pt + 3).map(|q| [f64::from(q[0]), f64::from(q[1]), f64::from(q[2])]))
                    .collect();
                let (tris, fanned) = match pts {
                    Some(pts) => super::polygon::triangles(&pts),
                    None => ((1..n - 1).map(|k| [0, k, k + 1]).collect(), false),
                };
                fanned_concave += usize::from(fanned);
                for tri in tris {
                    corners.extend([order[tri[0]], order[tri[1]], order[tri[2]]]);
                }
            }
            if fanned_concave > 0 {
                this.warn(format!(
                    "mesh {} has {fanned_concave} concave face(s) of more than {} corners; they were fan-triangulated",
                    short(&path),
                    super::polygon::MAX_EAR_CLIP
                ));
            }
            Ok((corners, faces.len() == n_faces))
        })
    }

    /// A `Points` prim (UsdGeomPoints, `OpenUSD@v26.11.1
    /// pxr/usd/usdGeom/schema.usda` L1877) as one points primitive: a vertex
    /// for each of its `points`, each named once. `widths` are not read (the
    /// writer sizes them from the scene), nor are `ids`. A primvar's
    /// interpolation counts follow Ruling I10-R7: every point is an element,
    /// so `constant` is one value and `uniform`, `varying`, `vertex` and
    /// `faceVarying` one per point.
    fn read_points(&mut self, p: usize, node: usize) -> Result<(), MeshError> {
        let st = self.st;
        let path = self.path(p).to_string();
        let Some(points_attr) = st.attr(p, "points") else {
            self.warn(format!("points prim {} has no points", short(&path)));
            return Ok(());
        };
        if points_attr.animated() {
            self.warn(format!("points prim {} animates its points, which is not carried; the first sample is used", short(&path)));
        }
        let points: Vec<f32> = match points_attr.rest().and_then(Value::floats) {
            Some(v) if v.len() % 3 == 0 => v.into_owned(),
            _ => return Err(malformed(format!("points prim {} points are not 3-vectors", short(&path)))),
        };
        let n_points = points.len() / 3;
        let topo = (n_points, n_points, n_points);
        let normals = self.normals_of(p, topo);
        let groups = vec![(self.binding_of(p, true), ())];
        self.read_geometry(p, node, Topology::Points, &points, topo, normals, groups, |this, ()| {
            // Charged before the corners are made.
            if this.total_indices.saturating_add(n_points) > MAX_SCENE_INDICES {
                return Err(unsupported(format!("the stage has more than {MAX_SCENE_INDICES} point indices")));
            }
            Ok(((0..n_points).map(|i| (i, i, i)).collect(), true))
        })
    }

    /// A `BasisCurves` prim (UsdGeomBasisCurves, `OpenUSD@v26.11.1
    /// pxr/usd/usdGeom/schema.usda` L1594-1812) of `type = "linear"` as one
    /// lines primitive: a segment between each curve vertex and the next,
    /// the last to the first where `wrap = "periodic"`. Any other `type`,
    /// and the schema's fallback `cubic` when it has none, are splines a
    /// segment cannot be, and warn; `pinned` is a cubic's wrap, so a linear
    /// curve reads it as nonperiodic. `curveVertexCounts` must account for
    /// every point, or the prim is malformed. A curve of fewer than 2
    /// vertices has no segment and is skipped, said once for the prim. Its
    /// `widths`, `normals` (they orient a ribbon) and `basis` are not read. A
    /// primvar's interpolation counts follow Ruling I10-R7: `uniform` is one
    /// value per curve, and `varying`, `vertex` and `faceVarying` one per
    /// curve vertex.
    fn read_curves(&mut self, p: usize, node: usize) -> Result<(), MeshError> {
        let st = self.st;
        let path = self.path(p).to_string();
        match self.token(p, "type").as_deref() {
            Some("linear") => {}
            Some(other) => {
                self.warn(format!("BasisCurves {} is {other}, not linear; only linear curves are carried, so it was left out", short(&path)));
                return Ok(());
            }
            None => {
                self.warn(format!(
                    "BasisCurves {} has no type, which USD reads as cubic; only linear curves are carried, so it was left out",
                    short(&path)
                ));
                return Ok(());
            }
        }
        let periodic = self.token(p, "wrap").as_deref() == Some("periodic");
        let Some(points_attr) = st.attr(p, "points") else {
            self.warn(format!("BasisCurves {} has no points", short(&path)));
            return Ok(());
        };
        if points_attr.animated() {
            self.warn(format!("BasisCurves {} animates its points, which is not carried; the first sample is used", short(&path)));
        }
        let points: Vec<f32> = match points_attr.rest().and_then(Value::floats) {
            Some(v) if v.len() % 3 == 0 => v.into_owned(),
            _ => return Err(malformed(format!("BasisCurves {} points are not 3-vectors", short(&path)))),
        };
        let n_points = points.len() / 3;
        let counts: Vec<i64> = st.attr(p, "curveVertexCounts").and_then(|a| a.rest().and_then(Value::ints).map(|v| v.to_vec())).unwrap_or_default();
        if counts.iter().any(|c| *c < 0) {
            return Err(malformed(format!("BasisCurves {} has a negative curve vertex count", short(&path))));
        }
        let total = counts.iter().map(|c| usize::try_from(*c).unwrap_or(usize::MAX)).fold(0usize, |a, b| a.saturating_add(b));
        if total != n_points {
            return Err(malformed(format!(
                "BasisCurves {} has curves of {total} vertices in all and {n_points} points",
                short(&path)
            )));
        }
        let topo = (counts.len(), n_points, n_points);
        let groups = vec![(self.binding_of(p, true), ())];
        let mut warned_short = false;
        self.read_geometry(p, node, Topology::Lines, &points, topo, None, groups, |this, ()| {
            // Charged from the counts, before any segment is made.
            let segments = segment_count(&counts, periodic);
            if this.total_indices.saturating_add(segments.saturating_mul(2)) > MAX_SCENE_INDICES {
                return Err(unsupported(format!("the stage has more than {MAX_SCENE_INDICES} line indices")));
            }
            let mut corners: Vec<(usize, usize, usize)> = Vec::with_capacity(segments * 2);
            let mut start = 0usize;
            for (curve, &c) in counts.iter().enumerate() {
                let c = c as usize;
                if c < 2 {
                    if !warned_short {
                        warned_short = true;
                        this.warn(format!("BasisCurves {} has curves of fewer than 2 vertices; they were skipped", short(&path)));
                    }
                } else {
                    for k in 0..c - 1 {
                        corners.extend([(curve, start + k, start + k), (curve, start + k + 1, start + k + 1)]);
                    }
                    if periodic {
                        corners.extend([(curve, start + c - 1, start + c - 1), (curve, start, start)]);
                    }
                }
                start += c;
            }
            Ok((corners, true))
        })
    }

    /// A Mesh's or Points prim's normals: `primvars:normals` wins over
    /// `normals`. A curve's are not read (a linear BasisCurves' orient a
    /// ribbon; they are not shading normals).
    fn normals_of(&mut self, p: usize, topo: (usize, usize, usize)) -> Option<Primvar> {
        if self.primvar_attr(p, "primvars:normals").is_some() {
            self.primvar(p, "primvars:normals", 3, Interp::Constant, topo)
        } else {
            self.primvar(p, "normals", 3, Interp::Vertex, topo)
        }
    }

    /// What a Mesh, a Points prim and a linear BasisCurves prim share once each
    /// has its topology: the IR primitives of one `topology` over `groups`
    /// (one per material binding), their vertices, primvars and materials, the
    /// blend shapes and the skin. `corners_of` turns a group into its corners
    /// `(element, point, element-vertex)`, one per index, and says whether the
    /// group is every element of the prim (the points themselves are then the
    /// vertices when every primvar is per point). `topo` counts the elements,
    /// points and element-vertices primvars are checked against.
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn read_geometry<G>(
        &mut self,
        p: usize,
        node: usize,
        topology: Topology,
        points: &[f32],
        topo: (usize, usize, usize),
        normals: Option<Primvar>,
        groups: Vec<(Option<usize>, G)>,
        mut corners_of: impl FnMut(&mut Self, G) -> Result<(Vec<(usize, usize, usize)>, bool), MeshError>,
    ) -> Result<(), MeshError> {
        let st = self.st;
        let path = self.path(p).to_string();
        let n_points = points.len() / 3;
        let double_sided = st.attr(p, "doubleSided").and_then(|a| a.rest().and_then(Value::as_bool)).unwrap_or(false);
        let has_skin = self.skel_binding(p, "primvars:skel:jointIndices").is_some()
            && self.skel_binding(p, "primvars:skel:jointWeights").is_some();

        let mut mesh = SceneMesh { name: Some(self.display_name(p)), ..Default::default() };
        let mut point_maps: Vec<Vec<u32>> = Vec::new();
        for (binding, group) in groups {
            // The bound material as read; its IR material waits for the
            // mesh's sets, which renumber its bindings.
            let (mut material, read, reads) = match binding {
                Some(m) => {
                    let (mat, reads) = self.material(m, double_sided)?;
                    (None, Some((m, mat)), reads)
                }
                None => {
                    // An unbound gprim draws with its displayColor, or Hydra's
                    // fallback grey when it has none.
                    let has_color = self.primvar_attr(p, "primvars:displayColor").is_some();
                    let mi = self.fallback_material(double_sided, has_color);
                    let reads = if has_color {
                        MaterialReads { color: Some("displayColor".into()), opacity: Some("displayOpacity".into()), ..Default::default() }
                    } else {
                        MaterialReads::default()
                    };
                    (Some(mi), None, reads)
                }
            };
            // Texture coordinates, numbered by name.
            let mut uv_names = self.uv_set_names(p, &reads.uv_names);
            for n in reads.uv_names.iter().filter(|n| !uv_names.contains(n)) {
                self.warn(format!(
                    "mesh {} needs more than {MAX_UV_SETS} texture-coordinate sets; {n:?}, which its material reads, is dropped with the textures on it",
                    short(&path)
                ));
            }
            let mut uv_pvs: Vec<Option<Primvar>> = Vec::new();
            for n in &uv_names {
                let pv = self.primvar(p, &format!("primvars:{n}"), 2, Interp::Constant, topo);
                if pv.is_none() && reads.uv_names.contains(n) {
                    self.warn(format!("mesh {} has no texture coordinates {n:?} for its material; (0, 0) is used", short(&path)));
                }
                uv_pvs.push(pv);
            }
            // A last set that did not read (declared without a value, or
            // ignored as malformed) is not kept as zeros when nothing reads it.
            while uv_pvs.last().is_some_and(|u| u.is_none()) && uv_names.last().is_some_and(|n| !reads.uv_names.contains(n)) {
                uv_pvs.pop();
                uv_names.pop();
            }
            if let Some((m, mat)) = read {
                material = Some(self.bind_material(m, double_sided, mat, &reads, &uv_names));
            }
            // A displayColor the bound material never reads is not drawn by
            // USD either: the material is.
            let color = match &reads.color {
                Some(n) => self.primvar(p, &format!("primvars:{n}"), 3, Interp::Constant, topo),
                None => None,
            };
            let opacity = match &reads.opacity {
                Some(n) => self.primvar(p, &format!("primvars:{n}"), 1, Interp::Constant, topo),
                None => None,
            };

            let per_point = normals.as_ref().is_none_or(Primvar::per_point)
                && uv_pvs.iter().flatten().all(Primvar::per_point)
                && color.as_ref().is_none_or(Primvar::per_point)
                && opacity.as_ref().is_none_or(Primvar::per_point);

            // The topology's corners: what a Mesh, Points or BasisCurves prim
            // says (the caller's closure), charged before it allocates.
            let (corners, single_group) = corners_of(self, group)?;
            if corners.is_empty() {
                continue;
            }

            // IR vertices: the points themselves when every attribute is per
            // point (exact round trip of our own files), else one per distinct
            // combination of the attribute values each corner uses.
            let mut prim = Primitive { material, topology, ..Default::default() };
            let mut point_of: Vec<u32> = Vec::new();
            if per_point && single_group {
                if self.total_vertices.saturating_add(n_points) > MAX_SCENE_VERTICES {
                    return Err(unsupported(format!("the stage has more than {MAX_SCENE_VERTICES} vertices")));
                }
                point_of = (0..n_points as u32).collect();
                prim.indices = corners.iter().map(|c| c.1 as u32).collect();
                prim.positions = points.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
                let fill3 = |pv: &Primvar| -> Vec<[f32; 3]> {
                    (0..n_points).map(|i| { let v = pv.get(pv.index(0, i, 0)); [v[0], v[1], v[2]] }).collect()
                };
                if let Some(n) = &normals {
                    prim.normals = Some(fill3(n));
                }
                for pv in &uv_pvs {
                    prim.uvs.push(match pv {
                        Some(u) => (0..n_points).map(|i| { let v = u.get(u.index(0, i, 0)); [v[0], 1.0 - v[1]] }).collect(),
                        None => vec![[0.0, 0.0]; n_points],
                    });
                }
                if color.is_some() || opacity.is_some() {
                    prim.colors = Some(
                        (0..n_points)
                            .map(|i| {
                                let c = color.as_ref().map(|c| c.get(c.index(0, i, 0)).to_vec()).unwrap_or_else(|| vec![1.0; 3]);
                                let a = opacity.as_ref().map(|o| o.get(o.index(0, i, 0))[0]).unwrap_or(1.0);
                                [c[0], c[1], c[2], a]
                            })
                            .collect(),
                    );
                }
            } else {
                let mut seen: HashMap<Vec<u32>, u32> = HashMap::new();
                let mut normals_out = Vec::new();
                let mut uvs_out: Vec<Vec<[f32; 2]>> = vec![Vec::new(); uv_pvs.len()];
                let mut colors_out = Vec::new();
                for &(f, pt, fv) in &corners {
                    let mut key: Vec<u32> = Vec::with_capacity(5);
                    key.push(pt as u32);
                    let ni = normals.as_ref().map(|n| n.index(f, pt, fv));
                    key.push(ni.map_or(u32::MAX, |i| i as u32));
                    let uvi: Vec<Option<usize>> = uv_pvs.iter().map(|u| u.as_ref().map(|u| u.index(f, pt, fv))).collect();
                    key.extend(uvi.iter().map(|i| i.map_or(u32::MAX, |i| i as u32)));
                    let ci = color.as_ref().map(|c| c.index(f, pt, fv));
                    let oi = opacity.as_ref().map(|o| o.index(f, pt, fv));
                    key.push(ci.map_or(u32::MAX, |i| i as u32));
                    key.push(oi.map_or(u32::MAX, |i| i as u32));
                    let next = point_of.len() as u32;
                    let v = *seen.entry(key).or_insert(next);
                    if v == next {
                        if self.total_vertices.saturating_add(point_of.len() + 1) > MAX_SCENE_VERTICES {
                            return Err(unsupported(format!("the stage has more than {MAX_SCENE_VERTICES} vertices")));
                        }
                        point_of.push(pt as u32);
                        prim.positions.push([points[pt * 3], points[pt * 3 + 1], points[pt * 3 + 2]]);
                        if let (Some(n), Some(i)) = (&normals, ni) {
                            let v = n.get(i);
                            normals_out.push([v[0], v[1], v[2]]);
                        }
                        for (k, u) in uv_pvs.iter().enumerate() {
                            uvs_out[k].push(match (u, uvi[k]) {
                                (Some(u), Some(i)) => {
                                    let v = u.get(i);
                                    [v[0], 1.0 - v[1]]
                                }
                                _ => [0.0, 0.0],
                            });
                        }
                        if color.is_some() || opacity.is_some() {
                            let c = match (&color, ci) {
                                (Some(c), Some(i)) => c.get(i).to_vec(),
                                _ => vec![1.0; 3],
                            };
                            let a = match (&opacity, oi) {
                                (Some(o), Some(i)) => o.get(i)[0],
                                _ => 1.0,
                            };
                            colors_out.push([c[0], c[1], c[2], a]);
                        }
                    }
                    prim.indices.push(v);
                }
                if normals.is_some() {
                    prim.normals = Some(normals_out);
                }
                prim.uvs = uvs_out;
                if color.is_some() || opacity.is_some() {
                    prim.colors = Some(colors_out);
                }
            }
            self.total_vertices += prim.positions.len();
            self.total_indices += prim.indices.len();
            point_maps.push(point_of);
            mesh.primitives.push(prim);
        }
        if mesh.primitives.is_empty() {
            return Ok(());
        }
        // Blend shapes, as morph targets over the IR vertices.
        let shape_names: Vec<String> = st.attr(p, "skel:blendShapes").and_then(|a| a.rest().and_then(Value::strs).map(|v| v.to_vec())).unwrap_or_default();
        let shape_prims = st.rel(p, "skel:blendShapeTargets");
        if !shape_names.is_empty() || !shape_prims.is_empty() {
            if shape_names.len() != shape_prims.len() {
                self.warn(format!(
                    "mesh {} names {} blend shapes but targets {}; its blend shapes were dropped",
                    short(&path),
                    shape_names.len(),
                    shape_prims.len()
                ));
            } else if shape_names.len() > MAX_BLEND_SHAPES {
                return Err(unsupported(format!("mesh {} has {} blend shapes", short(&path), shape_names.len())));
            } else {
                let mut target_names = Vec::new();
                for (k, tp) in shape_prims.iter().enumerate() {
                    let Some(bp) = st.find(tp) else {
                        self.warn(format!("blend shape {} of mesh {} does not exist; it reads as zero", short(tp), short(&path)));
                        for prim in mesh.primitives.iter_mut() {
                            prim.targets.push(MorphTarget { positions: vec![[0.0; 3]; prim.positions.len()], normals: None });
                        }
                        target_names.push(shape_names[k].clone());
                        continue;
                    };
                    target_names.push(self.display_name(bp));
                    // A shape is a points-length array per primitive however
                    // sparse its offsets: charged before it is built.
                    let need = (n_points + mesh.primitives.iter().map(|p| p.positions.len()).sum::<usize>()).saturating_mul(6);
                    self.morph_values = self.morph_values.saturating_add(need);
                    if self.morph_values > super::scene::MAX_MORPH_VALUES {
                        return Err(unsupported(format!(
                            "mesh {}'s blend shapes expand past {} values",
                            short(&path),
                            super::scene::MAX_MORPH_VALUES
                        )));
                    }
                    let (offsets, normal_offsets) = self.blend_shape(bp, n_points)?;
                    for (pi, prim) in mesh.primitives.iter_mut().enumerate() {
                        let map = &point_maps[pi];
                        let positions = map.iter().map(|&pt| offsets[pt as usize]).collect();
                        let normals = normal_offsets.as_ref().map(|no| map.iter().map(|&pt| no[pt as usize]).collect());
                        prim.targets.push(MorphTarget { positions, normals });
                    }
                }
                // The scene keeps target normals all-or-nothing per primitive.
                for prim in mesh.primitives.iter_mut() {
                    if prim.targets.iter().any(|t| t.normals.is_some()) {
                        let n = prim.positions.len();
                        for t in prim.targets.iter_mut() {
                            if t.normals.is_none() {
                                t.normals = Some(vec![[0.0; 3]; n]);
                            }
                        }
                    }
                }
                mesh.target_names = target_names;
                self.morphs.push(MorphMesh { node, mesh: self.scene.meshes.len(), prim: p, names: shape_names });
            }
        }
        let mi = self.scene.meshes.len();
        self.scene.meshes.push(mesh);
        self.scene.nodes[node].mesh = Some(mi);

        if has_skin {
            let ja = self.skel_binding(p, "primvars:skel:jointIndices");
            let wa = self.skel_binding(p, "primvars:skel:jointWeights");
            let ji: Vec<i64> = ja.as_ref().and_then(|a| a.rest().and_then(Value::ints).map(|v| v.to_vec())).unwrap_or_default();
            let jw: Vec<f32> = wa.as_ref().and_then(|a| a.rest().and_then(Value::floats).map(|v| v.into_owned())).unwrap_or_default();
            let element_size = ja.as_ref().and_then(|a| a.element_size).unwrap_or(1).max(1) as usize;
            let constant = ja.as_ref().and_then(|a| a.interpolation.as_deref()) == Some("constant");
            if element_size > MAX_INFLUENCES {
                return Err(unsupported(format!("mesh {} has {element_size} joint influences per point", short(&path))));
            }
            let expected = if constant { element_size } else { element_size.saturating_mul(n_points) };
            if ji.len() != expected || jw.len() != expected {
                self.warn(format!(
                    "mesh {} has {}/{} joint indices/weights where {expected} are needed; it is not skinned",
                    short(&path),
                    ji.len(),
                    jw.len()
                ));
            } else {
                self.skinned.push(SkinnedMesh {
                    node,
                    mesh: mi,
                    prim: p,
                    point_maps,
                    joint_indices: ji,
                    joint_weights: jw,
                    element_size,
                    constant,
                    point_count: n_points,
                });
            }
        }
        Ok(())
    }

    /// The texture-coordinate sets of mesh `p`, by name, in uv-set order:
    /// `primvars:st`, then `st1` … `st7` as the mesh has them, in suffix order
    /// with gaps closed; then the other primvars its material reads
    /// (`material_reads`, Blender's `UVMap`), in the material's read order; at
    /// most `MAX_UV_SETS`. With no `st`, the mesh's first `texCoord2*`
    /// primvar is set 0 instead, as it always was. A read the mesh lacks still
    /// takes its place after the mesh's own sets (it reads as (0, 0)), so every
    /// binding has a set. Our own files write `st`, `st1`, … in set order, so
    /// they read back as written.
    fn uv_set_names(&self, p: usize, material_reads: &[String]) -> Vec<String> {
        let st = self.st;
        let has = |name: &str| st.attr(p, &format!("primvars:{name}")).is_some();
        let mut out: Vec<String> = Vec::new();
        if has("st") {
            out.push("st".to_string());
        } else {
            // The first texture-coordinate primvar, by role.
            for n in st.prop_names(p) {
                let Some(name) = n.strip_prefix("primvars:") else { continue };
                if !name.contains(':') && st.attr(p, &n).is_some_and(|a| a.type_name.starts_with("texCoord2")) {
                    out.push(name.to_string());
                    break;
                }
            }
        }
        // `st` is set 0, so `st1` … `st7` complete the eight.
        for k in 1..MAX_UV_SETS {
            let name = format!("st{k}");
            if has(&name) && !out.contains(&name) {
                out.push(name);
            }
        }
        for name in material_reads {
            if !out.contains(name) {
                out.push(name.clone());
            }
        }
        out.truncate(MAX_UV_SETS);
        out
    }

    fn blend_shape(&mut self, bp: usize, n_points: usize) -> Result<ShapeOffsets, MeshError> {
        let st = self.st;
        let path = self.path(bp).to_string();
        let floats = |name: &str| st.attr(bp, name).and_then(|a| a.rest().and_then(Value::floats).map(|v| v.into_owned()));
        let offsets = floats("offsets").unwrap_or_default();
        let normal_offsets = floats("normalOffsets");
        let indices: Option<Vec<i64>> = st.attr(bp, "pointIndices").and_then(|a| a.rest().and_then(Value::ints).map(|v| v.to_vec()));
        if st.prop_names(bp).iter().any(|n| n.starts_with("inbetweens:")) {
            self.warn(format!("blend shape {} has inbetween shapes, which are not carried", short(&path)));
        }
        let dense = |flat: &[f32], what: &str| -> Result<Vec<[f32; 3]>, MeshError> {
            if !flat.len().is_multiple_of(3) {
                return Err(malformed(format!("{what} of blend shape {} are not 3-vectors", short(&path))));
            }
            let n = flat.len() / 3;
            let mut out = vec![[0.0f32; 3]; n_points];
            match &indices {
                Some(ix) => {
                    if ix.len() != n {
                        return Err(malformed(format!("blend shape {} has {} point indices for {n} {what}", short(&path), ix.len())));
                    }
                    for (k, &i) in ix.iter().enumerate() {
                        let i = usize::try_from(i).ok().filter(|i| *i < n_points).ok_or_else(|| malformed(format!("blend shape {} indexes point {i}", short(&path))))?;
                        out[i] = [flat[k * 3], flat[k * 3 + 1], flat[k * 3 + 2]];
                    }
                }
                None => {
                    if n != n_points {
                        return Err(malformed(format!("blend shape {} has {n} {what} for {n_points} points", short(&path))));
                    }
                    for (k, o) in out.iter_mut().enumerate() {
                        *o = [flat[k * 3], flat[k * 3 + 1], flat[k * 3 + 2]];
                    }
                }
            }
            Ok(out)
        };
        let o = dense(&offsets, "offsets")?;
        let n = match normal_offsets {
            Some(no) if !no.is_empty() => Some(dense(&no, "normal offsets")?),
            _ => None,
        };
        Ok((o, n))
    }

    // ── materials ───────────────────────────────────────────────────────────

    /// The material bound to prim `p` (inherited from ancestors when
    /// `inherit`): the preview-purpose binding first, then the all-purpose one.
    fn binding_of(&mut self, p: usize, inherit: bool) -> Option<usize> {
        let st = self.st;
        let mut cur = Some(p);
        while let Some(c) = cur {
            for rel in ["material:binding:preview", "material:binding"] {
                let targets = st.rel(c, rel);
                if let Some(t) = targets.first() {
                    match st.find(t) {
                        Some(m) if st.prims[m].type_name == "Material" => return Some(m),
                        _ => {
                            self.warn(format!("{} binds {}, which is not a material; ignored", short(self.path(c)), short(t)));
                        }
                    }
                }
            }
            if st.prop_names(c).iter().any(|n| n.starts_with("material:binding:collection:")) {
                self.warn(format!("{} binds materials through collections, which are not read", short(self.path(c))));
            }
            if !inherit {
                return None;
            }
            cur = st.prims[c].parent;
        }
        None
    }

    fn fallback_material(&mut self, double_sided: bool, from_primvar: bool) -> usize {
        let m = Material { double_sided, ..unbound_material(from_primvar) };
        if let Some(i) = self.scene.materials.iter().position(|x| *x == m) {
            return i;
        }
        self.scene.materials.push(m);
        self.scene.materials.len() - 1
    }

    /// Material prim `m` as read, once per (prim, doubleSided): its texture
    /// bindings numbered in its own read order (`MaterialReads::uv_names`).
    fn material(&mut self, m: usize, double_sided: bool) -> Result<(Material, MaterialReads), MeshError> {
        if let Some(read) = self.read_materials.get(&(m, double_sided)) {
            return Ok(read.clone());
        }
        let (mut mat, reads) = self.read_material(m)?;
        mat.double_sided = double_sided;
        self.read_materials.insert((m, double_sided), (mat.clone(), reads.clone()));
        Ok((mat, reads))
    }

    /// The IR material of `mat`, read from material prim `m`, on a mesh whose
    /// sets are `uv_names`: each binding renumbered from the material's read
    /// order to its primvar's place among the mesh's sets, and dropped when
    /// that primvar has none (past `MAX_UV_SETS`). Meshes that put the reads
    /// at the same places share one, pushed at first use, which every mesh of
    /// one of our own files does; another layout gets its own copy.
    fn bind_material(&mut self, m: usize, double_sided: bool, mut mat: Material, reads: &MaterialReads, uv_names: &[String]) -> usize {
        let layout: Vec<Option<u32>> = reads.uv_names.iter().map(|n| uv_names.iter().position(|u| u == n).map(|i| i as u32)).collect();
        let key = (m, double_sided, layout);
        if let Some(i) = self.materials.get(&key) {
            return *i;
        }
        let layout = &key.2;
        for (_, slot) in mat.texture_slots_mut() {
            let Some(r) = slot.as_mut() else { continue };
            match layout.get(r.uv_set as usize).copied().flatten() {
                Some(set) => r.uv_set = set,
                None => {
                    *slot = None;
                    self.prune_textures = true;
                }
            }
        }
        let i = self.scene.materials.len();
        self.scene.materials.push(mat);
        self.materials.insert(key, i);
        i
    }

    /// Follow a shader input (or interface attribute) to its source: a value,
    /// or an output of a shader.
    fn resolve(&self, prim: usize, attr: &str) -> Resolved {
        let st = self.st;
        let mut cur = (prim, attr.to_string());
        for _ in 0..MAX_CONNECTION_HOPS {
            let Some(a) = st.attr(cur.0, &cur.1) else { return Resolved::None };
            if let Some(target) = a.connections.first() {
                let Some((tp, prop)) = split_property(target) else { return Resolved::None };
                let Some(tprim) = st.find(tp) else { return Resolved::None };
                let ty = st.prims[tprim].type_name.as_str();
                if ty == "Shader" && prop.starts_with("outputs:") {
                    return Resolved::Output(tprim, prop["outputs:".len()..].to_string());
                }
                // Through a NodeGraph's output or a Material/NodeGraph
                // interface input.
                cur = (tprim, prop.to_string());
                continue;
            }
            return match a.rest() {
                Some(v) => Resolved::Value(v.clone(), a.layer),
                None => Resolved::None,
            };
        }
        Resolved::None
    }

    fn surface_shader(&mut self, m: usize) -> Option<usize> {
        let st = self.st;
        let mut out = None;
        for name in ["outputs:surface", "outputs:ri:surface", "outputs:mtlx:surface"] {
            if let Resolved::Output(s, _) = self.resolve(m, name) {
                out = Some(s);
                break;
            }
        }
        if out.is_none() {
            // An unconnected material: a lone preview surface child is what it
            // means.
            out = st.prims[m].children.iter().copied().find(|&c| self.token(c, "info:id").as_deref() == Some("UsdPreviewSurface"));
        }
        out
    }

    /// Whether material prim `m` carries litegen's unlit marker, `bool unlit`
    /// in its `customData.litegen` ([`UNLIT_KEY`]): true, and nothing else
    /// (a value that is not a boolean, or false, is no marker). A marker of
    /// another type is named, since it was meant as one.
    fn marked_unlit(&mut self, m: usize, mpath: &str) -> bool {
        let Some(d) = litegen_dict(self.st, m).and_then(Value::dict) else { return false };
        let marks: Vec<&Value> = d.iter().filter(|(k, _)| k == UNLIT_KEY).map(|(_, v)| v).collect();
        if marks.iter().any(|v| matches!(v, Value::Bool(true))) {
            return true;
        }
        if marks.iter().any(|v| !matches!(v, Value::Bool(_))) {
            self.warn(format!("material {} marks unlit with a value that is not a bool, so it reads lit", short(mpath)));
        }
        false
    }

    fn read_material(&mut self, m: usize) -> Result<(Material, MaterialReads), MeshError> {
        let mpath = self.path(m).to_string();
        let mut mat = Material { name: Some(self.display_name(m)), metallic: 0.0, roughness: 0.5, ..Default::default() };
        // UsdPreviewSurface's own defaults differ from glTF's: diffuse 0.18
        // grey, roughness 0.5, metallic 0.
        mat.base_color = [0.18, 0.18, 0.18, 1.0];
        let mut reads = MaterialReads::default();
        let Some(s) = self.surface_shader(m) else {
            self.warn(format!("material {} has no UsdPreviewSurface; a default surface is used", short(&mpath)));
            return Ok((mat, reads));
        };
        let id = self.token(s, "info:id").unwrap_or_default();
        if id != "UsdPreviewSurface" {
            self.warn(format!(
                "material {} uses shader {:?}, not UsdPreviewSurface; a default surface is used",
                short(&mpath),
                short(&id)
            ));
            return Ok((mat, reads));
        }
        let mut uv_names: Vec<String> = Vec::new();
        // A material litegen marked unlit (`customData.litegen.unlit`, written
        // beside the emission recipe, `usd_write.rs`, "Unlit") has its base
        // colour where the recipe puts it, in the emission, and no emissive
        // of its own. Without the marker the same recipe is a lit black
        // material that emits, read as it is.
        let unlit = self.marked_unlit(m, &mpath);
        mat.unlit = unlit;
        let base_input = if unlit { "inputs:emissiveColor" } else { "inputs:diffuseColor" };
        // Diffuse colour and opacity.
        let mut opacity_connected = false;
        match self.resolve(s, base_input) {
            Resolved::Value(v, _) => {
                if let Some(t) = v.as_tuple().filter(|t| t.len() >= 3) {
                    mat.base_color[..3].copy_from_slice(&[t[0] as f32, t[1] as f32, t[2] as f32]);
                }
            }
            Resolved::Output(t, out) => match self.token(t, "info:id").as_deref() {
                Some("UsdUVTexture") => {
                    if out != "rgb" {
                        self.warn(format!("material {} takes its colour from texture channel {out:?}; read as rgb", short(&mpath)));
                    }
                    // The texture's scale is glTF's factor, and stands alone
                    // when the image itself is missing (already warned).
                    let scale = self.float4(t, "inputs:scale", [1.0; 4]);
                    mat.base_color[..3].copy_from_slice(&scale[..3]);
                    if let Some(tex) = self.texture(t, &mut uv_names, true)? {
                        mat.base_color_texture = Some(tex.r);
                    }
                }
                Some(id) if id.starts_with("UsdPrimvarReader") => {
                    reads.color = self.input_token(t, "inputs:varname");
                    mat.base_color[..3].copy_from_slice(&[1.0, 1.0, 1.0]);
                }
                other => self.warn(format!("material {} colour comes from {:?}, which is not read", short(&mpath), other.unwrap_or(""))),
            },
            Resolved::None => {}
        }
        match self.resolve(s, "inputs:opacity") {
            Resolved::Value(v, _) => {
                if let Some(a) = v.as_f64() {
                    mat.base_color[3] = a as f32;
                }
            }
            Resolved::Output(t, out) => {
                opacity_connected = true;
                match self.token(t, "info:id").as_deref() {
                    Some("UsdUVTexture") => {
                        let same = mat.base_color_texture.is_some()
                            && self.texture_file(t) == self.texture_file_of_base(s, base_input);
                        if out == "a" && same {
                            let scale = self.float4(t, "inputs:scale", [1.0; 4]);
                            mat.base_color[3] = scale[3];
                        } else {
                            self.warn(format!(
                                "material {} takes opacity from channel {out:?} of a texture other than its base colour's alpha; opacity is not carried",
                                short(&mpath)
                            ));
                            opacity_connected = false;
                        }
                    }
                    Some(id) if id.starts_with("UsdPrimvarReader") => {
                        reads.opacity = self.input_token(t, "inputs:varname");
                        mat.base_color[3] = 1.0;
                    }
                    _ => {
                        opacity_connected = false;
                        self.warn(format!("material {} opacity comes from a shader that is not read", short(&mpath)));
                    }
                }
            }
            Resolved::None => {}
        }
        let threshold = match self.resolve(s, "inputs:opacityThreshold") {
            Resolved::Value(v, _) => v.as_f64().unwrap_or(0.0) as f32,
            _ => 0.0,
        };
        mat.alpha_mode = if threshold > 0.0 && (opacity_connected || mat.base_color[3] < 1.0 || reads.opacity.is_some()) {
            mat.alpha_cutoff = threshold;
            AlphaMode::Mask
        } else if opacity_connected || reads.opacity.is_some() || mat.base_color[3] < 1.0 {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        };

        // Metallic and roughness: glTF packs them as B and G of one image.
        let metal = self.resolve(s, "inputs:metallic");
        let rough = self.resolve(s, "inputs:roughness");
        if let Resolved::Value(v, _) = &metal {
            mat.metallic = v.as_f64().unwrap_or(0.0) as f32;
        }
        if let Resolved::Value(v, _) = &rough {
            mat.roughness = v.as_f64().unwrap_or(0.5) as f32;
        }
        match (&metal, &rough) {
            (Resolved::Output(mt, mo), Resolved::Output(rt, ro))
                if mo == "b" && ro == "g" && self.texture_file(*mt) == self.texture_file(*rt) =>
            {
                if let Some(tex) = self.texture(*rt, &mut uv_names, false)? {
                    mat.roughness = tex.scale[1];
                    mat.metallic = self.float4(*mt, "inputs:scale", [1.0; 4])[2];
                    mat.metallic_roughness_texture = Some(tex.r);
                }
            }
            (Resolved::Output(..), _) | (_, Resolved::Output(..)) => {
                // Separate or differently packed images: re-packed as glTF's
                // one map (`normalize::pack_metallic_roughness`).
                let metal_side = self.mr_side(&metal, mat.metallic, &mut uv_names)?;
                let rough_side = self.mr_side(&rough, mat.roughness, &mut uv_names)?;
                self.pack_mr(&mut mat, metal_side, rough_side, &mpath);
            }
            _ => {}
        }
        // Normal map.
        if let Resolved::Output(t, out) = self.resolve(s, "inputs:normal") {
            if self.token(t, "info:id").as_deref() == Some("UsdUVTexture") && out == "rgb" {
                let scale = self.float4(t, "inputs:scale", [1.0; 4]);
                let bias = self.float4(t, "inputs:bias", [0.0; 4]);
                // (2s, 2s, 2, 1) with bias (-s, -s, -1, 0) is glTF's normal
                // scale s; anything else is not a tangent-space normal map
                // glTF can express.
                let s_xy = scale[0] / 2.0;
                let ok = (scale[2] - 2.0).abs() < 1e-3
                    && (scale[1] - scale[0]).abs() < 1e-3
                    && (bias[0] + s_xy).abs() < 1e-3
                    && (bias[1] + s_xy).abs() < 1e-3
                    && (bias[2] + 1.0).abs() < 1e-3;
                if ok {
                    if let Some(tex) = self.texture(t, &mut uv_names, false)? {
                        mat.normal_texture = Some(tex.r);
                        mat.normal_scale = s_xy;
                    }
                } else {
                    self.warn(format!("material {} has a normal map with scale/bias glTF cannot express; it is not carried", short(&mpath)));
                }
            }
        }
        // Occlusion (R).
        if let Resolved::Output(t, out) = self.resolve(s, "inputs:occlusion") {
            if self.token(t, "info:id").as_deref() == Some("UsdUVTexture") {
                if out != "r" {
                    self.warn(format!("material {} reads occlusion from channel {out:?}; glTF reads R, so it is not carried", short(&mpath)));
                } else if let Some(tex) = self.texture(t, &mut uv_names, false)? {
                    let bias = self.float4(t, "inputs:bias", [0.0; 4]);
                    let strength = tex.scale[0];
                    if (bias[0] - (1.0 - strength)).abs() > 1e-3 && bias[0] != 0.0 {
                        self.warn(format!("material {} biases its occlusion in a way glTF cannot express; strength 1 is used", short(&mpath)));
                    }
                    mat.occlusion_texture = Some(tex.r);
                    mat.occlusion_strength = if (bias[0] - (1.0 - strength)).abs() <= 1e-3 { strength } else { 1.0 };
                }
            }
        }
        // Emission: the base colour of an unlit material, read above.
        match self.resolve(s, "inputs:emissiveColor") {
            _ if unlit => {}
            Resolved::Value(v, _) => {
                if let Some(t) = v.as_tuple().filter(|t| t.len() >= 3) {
                    mat.emissive = [t[0] as f32, t[1] as f32, t[2] as f32];
                }
            }
            Resolved::Output(t, _) => {
                if self.token(t, "info:id").as_deref() == Some("UsdUVTexture") {
                    let scale = self.float4(t, "inputs:scale", [1.0; 4]);
                    mat.emissive = [scale[0], scale[1], scale[2]];
                    if let Some(tex) = self.texture(t, &mut uv_names, true)? {
                        mat.emissive_texture = Some(tex.r);
                    }
                }
            }
            Resolved::None => {}
        }
        if let Resolved::Value(v, _) = self.resolve(s, "inputs:useSpecularWorkflow") {
            if v.as_i64() == Some(1) {
                self.warn(format!("material {} uses the specular workflow, which glTF's metallic-roughness model does not carry", short(&mpath)));
            }
        }
        // Clearcoat, UsdPreviewSurface's second specular lobe: the layer is
        // there when `clearcoat` is. An unauthored roughness is USD's own
        // 0.01, and the coat's normal is the base normal map, which
        // UsdPreviewSurface's clearcoat uses (`usd::usd_coat_normal`). An
        // unconnected 0 draws no coat, and three.js's USDZExporter writes it
        // on every physical material (USDZExporter.js:1063-1118, r183): it
        // reads as no layer, silently, its roughness unread, as OBJ's, FBX's
        // and Collada's readers read their exporters' defaults (Ruling A41).
        let no_coat = matches!(self.resolve(s, "inputs:clearcoat"), Resolved::Value(v, _) if v.as_f64() == Some(0.0));
        let coat = if no_coat { None } else { self.coat_input(s, "clearcoat", 0, &mpath, &mut uv_names)? };
        if let Some((factor, texture)) = coat {
            let (roughness, roughness_texture) = self.coat_input(s, "clearcoatRoughness", 1, &mpath, &mut uv_names)?.unwrap_or((0.01, None));
            let (normal, normal_scale) = super::usd::usd_coat_normal(&mat);
            let normal_texture = normal.cloned();
            mat.clearcoat = Some(super::scene::Clearcoat { factor, texture, roughness, roughness_texture, normal_texture, normal_scale });
        }
        // The ior as KHR_materials_ior holds it, 0 or 1 and above: anything
        // else is left out rather than refused by validation. Exactly 1.5,
        // glTF's default and UsdPreviewSurface's, which three.js's exporter
        // writes on every physical material, reads as unset, silently, as the
        // other readers read an exporter's default (Ruling A41).
        match self.resolve(s, "inputs:ior") {
            Resolved::Value(v, _) => {
                if let Some(x) = v.as_f64() {
                    let ior = x as f32;
                    if !super::scene::ior_in_schema(ior) {
                        self.warn(format!(
                            "material {} has an ior of {ior}, which KHR_materials_ior cannot hold (0, or 1 and above); it is not carried",
                            short(&mpath)
                        ));
                    } else if ior != super::scene::DEFAULT_IOR {
                        mat.ior = Some(ior);
                    }
                }
            }
            Resolved::Output(..) => {
                self.warn(format!("material {} ior comes from a shader; glTF's ior is a number, so it is not carried", short(&mpath)));
            }
            Resolved::None => {}
        }
        // USD's ior also sets its clearcoat's reflectance, where glTF's
        // clearcoat keeps 1.5 (`usd::losses` names the same on writing).
        if let (Some(c), Some(ior)) = (&mat.clearcoat, mat.ior) {
            if c.factor > 0.0 && ior != super::scene::DEFAULT_IOR {
                self.warn(format!(
                    "material {} has a clearcoat and an ior of {ior}: USD's clearcoat reflects with that ior, glTF's with 1.5",
                    short(&mpath)
                ));
            }
        }
        reads.uv_names = uv_names;
        Ok((mat, reads))
    }

    /// One of a clearcoat's numbers as glTF holds it, with its map: a value,
    /// or channel `channel` (0 is R, glTF's clearcoat; 1 is G, its roughness)
    /// of a UsdUVTexture times the texture's scale. `None` when unauthored,
    /// and, with a warning, when glTF cannot hold it: read from another
    /// channel or another shader, from a map that biases that channel (glTF's
    /// factor only scales its map), or outside 0 to 1, a non-finite number
    /// included. All of that is checked before the map is read, so a value
    /// left out leaves no texture behind.
    fn coat_input(&mut self, s: usize, input: &str, channel: usize, mpath: &str, uv_names: &mut Vec<String>) -> Result<Option<(f32, Option<TextureRef>)>, MeshError> {
        let name = ["r", "g", "b", "a"][channel];
        let (value, map) = match self.resolve(s, &format!("inputs:{input}")) {
            Resolved::Value(v, _) => match v.as_f64() {
                Some(x) => (x, None),
                None => return Ok(None),
            },
            Resolved::Output(t, out) if self.token(t, "info:id").as_deref() == Some("UsdUVTexture") => {
                if out != name {
                    self.warn(format!(
                        "material {} reads {input} from channel {out:?}; glTF reads it from {}, so it is not carried",
                        short(mpath),
                        name.to_uppercase()
                    ));
                    return Ok(None);
                }
                if self.float4(t, "inputs:bias", [0.0; 4])[channel] != 0.0 {
                    self.warn(format!("material {} biases its {input} map, which glTF cannot express; it is not carried", short(mpath)));
                    return Ok(None);
                }
                (f64::from(self.float4(t, "inputs:scale", [1.0; 4])[channel]), Some(t))
            }
            Resolved::Output(..) => {
                self.warn(format!("material {} {input} comes from a shader that is not read", short(mpath)));
                return Ok(None);
            }
            Resolved::None => return Ok(None),
        };
        // USD's floats are f32: the value as one, not widened.
        let value = value as f32;
        if !(0.0..=1.0).contains(&value) {
            self.warn(format!("material {} has a {input} of {value}, which glTF cannot hold (0 to 1); it is not carried", short(mpath)));
            return Ok(None);
        }
        let texture = match map {
            Some(t) => self.texture(t, uv_names, false)?.map(|tex| tex.r),
            None => None,
        };
        Ok(Some((value, texture)))
    }

    fn float4(&self, t: usize, name: &str, default: [f32; 4]) -> [f32; 4] {
        match self.resolve(t, name) {
            Resolved::Value(v, _) => match v.as_tuple() {
                Some(x) if x.len() == 4 => [x[0] as f32, x[1] as f32, x[2] as f32, x[3] as f32],
                _ => default,
            },
            _ => default,
        }
    }

    fn texture_file(&self, t: usize) -> Option<String> {
        match self.resolve(t, "inputs:file") {
            Resolved::Value(Value::Asset(a), layer) => self.st.resolve_asset(layer, &a),
            _ => None,
        }
    }

    fn texture_file_of_base(&self, surface: usize, base_input: &str) -> Option<String> {
        match self.resolve(surface, base_input) {
            Resolved::Output(t, _) => self.texture_file(t),
            _ => None,
        }
    }

    /// One separately mapped metallic or roughness input; `constant` is what
    /// it reads as with no usable map.
    fn mr_side(&mut self, r: &Resolved, constant: f32, uv_names: &mut Vec<String>) -> Result<MrSide, MeshError> {
        let Resolved::Output(t, ch) = r else { return Ok(MrSide::Constant(constant)) };
        let channel = match ch.as_str() {
            "g" => 1,
            "b" => 2,
            "a" => 3,
            _ => 0,
        };
        let scale = self.float4(*t, "inputs:scale", [1.0; 4])[channel];
        let bias = self.float4(*t, "inputs:bias", [0.0; 4])[channel];
        Ok(match self.texture(*t, uv_names, false)? {
            Some(tex) => MrSide::Map { tex, channel, scale, bias },
            // `texture` has said why there is no map.
            None => MrSide::Constant(constant),
        })
    }

    /// Metallic and roughness from separate sources, packed into one map; when
    /// they cannot be (a map that does not decode, or two maps read through
    /// different coordinates), each keeps its map's mean.
    fn pack_mr(&mut self, mat: &mut Material, metal: MrSide, rough: MrSide, mpath: &str) {
        let maps: Vec<&TexRead> = [&metal, &rough]
            .into_iter()
            .filter_map(|s| match s {
                MrSide::Map { tex, .. } => Some(tex),
                MrSide::Constant(_) => None,
            })
            .collect();
        let Some(first) = maps.first().map(|t| t.r.clone()) else {
            for (side, slot) in [(&metal, &mut mat.metallic), (&rough, &mut mat.roughness)] {
                if let MrSide::Constant(c) = side {
                    *slot = *c;
                }
            }
            return;
        };
        let same_mapping = maps.iter().all(|t| t.r.uv_set == first.uv_set && t.r.transform == first.transform);
        let scene = &self.scene;
        let source = |s: &MrSide| match s {
            MrSide::Constant(c) => MrSource::Constant(*c),
            MrSide::Map { tex, channel, scale, bias } => {
                let img = &scene.images[scene.textures[tex.r.texture].image];
                MrSource::Map { data: &img.data, format: img.format, channel: *channel, scale: *scale, bias: *bias }
            }
        };
        let packed = if same_mapping { pack_metallic_roughness(&source(&metal), &source(&rough)) } else { None };
        if let Some(png) = packed {
            let sampler = self.scene.textures[first.texture].sampler.clone();
            let name = Some(format!("{}_metallicRoughness", mat.name.as_deref().unwrap_or("material")));
            self.scene.images.push(Image { name: name.clone(), format: ImageFormat::Png, data: png });
            self.scene.textures.push(Texture { name, image: self.scene.images.len() - 1, sampler });
            mat.metallic_roughness_texture = Some(TextureRef { texture: self.scene.textures.len() - 1, ..first });
            (mat.metallic, mat.roughness) = (1.0, 1.0);
            self.prune_textures = true;
            return;
        }
        for (side, slot, what) in [(&metal, &mut mat.metallic, "metallic"), (&rough, &mut mat.roughness, "roughness")] {
            *slot = match side {
                MrSide::Constant(c) => *c,
                MrSide::Map { tex, channel, scale, bias } => {
                    let img = &self.scene.images[self.scene.textures[tex.r.texture].image];
                    // A NaN stays NaN, which validation refuses; a map that
                    // does not decode reads as USD's default.
                    match channel_means(&img.data, img.format) {
                        Some(means) => (means[*channel] * scale + bias).clamp(0.0, 1.0),
                        None if what == "metallic" => 0.0,
                        None => 0.5,
                    }
                }
            };
        }
        self.prune_textures = true;
        self.warn(format!(
            "material {} maps metallic and roughness so they cannot share glTF's one map; each keeps its map's mean",
            short(mpath)
        ));
    }

    /// A UsdUVTexture as a texture reference: its image, wrap modes, uv set
    /// and transform, plus its scale (the glTF factor). The uv set is its
    /// primvar's place in `uv_names`, the material's own read order, which
    /// [`Reader::bind_material`] renumbers for each mesh.
    ///
    /// UsdPreviewSurface 2.x, https://openusd.org/release/spec_usdpreviewsurface.html:
    /// UsdUVTexture (`#texture-reader`: `file`, wraps, `scale`/`bias`,
    /// `sourceColorSpace`), the UsdPrimvarReader that names its uv set
    /// (`#primvar-reader`, `inputs:varname`), and UsdTransform2d (`#transform2d`:
    /// scale, then rotate counter-clockwise in degrees, then translate).
    fn texture(&mut self, t: usize, uv_names: &mut Vec<String>, color: bool) -> Result<Option<TexRead>, MeshError> {
        let tpath = self.path(t).to_string();
        let Some(file) = self.texture_file(t) else {
            self.warn(format!("texture {} names no file inside the package; dropped", short(&tpath)));
            return Ok(None);
        };
        let space = self.input_token(t, "inputs:sourceColorSpace").unwrap_or_else(|| "auto".into());
        if color && space == "raw" {
            self.warn(format!("colour texture {} is marked raw (linear); glTF reads colour textures as sRGB", short(&tpath)));
        }
        if !color && space == "sRGB" {
            self.warn(format!("data texture {} is marked sRGB; glTF reads data textures as linear", short(&tpath)));
        }
        let image = match self.images.get(&file) {
            Some(i) => *i,
            None => {
                let i = self.load_image(&file, &tpath);
                self.images.insert(file.clone(), i);
                i
            }
        };
        let Some(image) = image else { return Ok(None) };
        let wrap = |w: Option<String>, this: &mut Self| -> Wrap {
            match w.as_deref() {
                Some("repeat") => Wrap::Repeat,
                Some("mirror") => Wrap::MirroredRepeat,
                Some("clamp") => Wrap::ClampToEdge,
                // `black` and the unauthored `useMetadata` show black outside
                // [0, 1] in Hydra; clamping is the closest glTF has.
                Some("black") | Some("useMetadata") | None => Wrap::ClampToEdge,
                Some(other) => {
                    this.warn(format!("texture {} has wrap mode {other:?}; clamped", short(&tpath)));
                    Wrap::ClampToEdge
                }
            }
        };
        let ws = self.input_token(t, "inputs:wrapS");
        let wt = self.input_token(t, "inputs:wrapT");
        let wrap_s = wrap(ws, self);
        let wrap_t = wrap(wt, self);
        let code = |w: Wrap| -> u8 {
            match w {
                Wrap::Repeat => 0,
                Wrap::ClampToEdge => 1,
                Wrap::MirroredRepeat => 2,
            }
        };
        let key = (image, code(wrap_s), code(wrap_t));
        let texture = match self.textures.get(&key) {
            Some(i) => *i,
            None => {
                self.scene.textures.push(Texture {
                    name: self.scene.images[image].name.clone(),
                    image,
                    sampler: Sampler { wrap_s, wrap_t, ..Default::default() },
                });
                let i = self.scene.textures.len() - 1;
                self.textures.insert(key, i);
                i
            }
        };
        // Coordinates: a primvar reader, optionally through a UsdTransform2d.
        let mut transform = None;
        let mut varname = None;
        if let Resolved::Output(r, _) = self.resolve(t, "inputs:st") {
            match self.token(r, "info:id").as_deref() {
                Some("UsdTransform2d") => {
                    let rot = match self.resolve(r, "inputs:rotation") {
                        Resolved::Value(v, _) => v.as_f64().unwrap_or(0.0),
                        _ => 0.0,
                    };
                    let pair = |name: &str, d: [f64; 2]| -> [f64; 2] {
                        match self.resolve(r, name) {
                            Resolved::Value(v, _) => match v.as_tuple() {
                                Some(x) if x.len() == 2 => [x[0], x[1]],
                                _ => d,
                            },
                            _ => d,
                        }
                    };
                    let scale = pair("inputs:scale", [1.0, 1.0]);
                    let translation = pair("inputs:translation", [0.0, 0.0]);
                    let tr = uv_transform_from_usd(rot, scale, translation);
                    if !tr.is_identity() {
                        transform = Some(tr);
                    }
                    if let Resolved::Output(pr, _) = self.resolve(r, "inputs:in") {
                        varname = self.input_token(pr, "inputs:varname");
                    }
                }
                Some(id) if id.starts_with("UsdPrimvarReader") => varname = self.input_token(r, "inputs:varname"),
                _ => {}
            }
        }
        let varname = varname.unwrap_or_else(|| "st".to_string());
        let uv_set = match uv_names.iter().position(|n| *n == varname) {
            Some(i) => i,
            None => {
                uv_names.push(varname);
                uv_names.len() - 1
            }
        };
        let scale = self.float4(t, "inputs:scale", [1.0; 4]);
        Ok(Some(TexRead { r: TextureRef { texture, uv_set: uv_set as u32, transform }, scale }))
    }

    fn load_image(&mut self, file: &str, tpath: &str) -> Option<usize> {
        let Some(bytes) = (self.fetch)(file) else {
            self.warn(format!("texture file {file:?} of {} was not supplied; its texture was dropped", short(tpath)));
            return None;
        };
        match ImageFormat::sniff(&bytes) {
            Some(format) => {
                let stem = file.rsplit('/').next().unwrap_or(file);
                let name = stem.rsplit_once('.').map_or(stem, |(s, _)| s).to_string();
                self.scene.images.push(Image { name: Some(name), format, data: bytes });
                Some(self.scene.images.len() - 1)
            }
            None => {
                self.warn(format!("texture file {file:?} is not PNG, JPEG, WebP or KTX2; its texture was dropped"));
                None
            }
        }
    }

    // ── skeletons ───────────────────────────────────────────────────────────

    /// Read a skeleton's joints as nodes under `attach` (a root when `None`).
    fn read_skeleton(&mut self, p: usize, attach: Option<usize>) -> Result<(), MeshError> {
        let st = self.st;
        let path = self.path(p).to_string();
        let joints: Vec<String> = st.attr(p, "joints").and_then(|a| a.rest().and_then(Value::strs).map(|v| v.to_vec())).unwrap_or_default();
        if joints.len() > super::scene::MAX_JOINTS {
            return Err(unsupported(format!("skeleton {} has {} joints", short(&path), joints.len())));
        }
        let names: Vec<String> = st.attr(p, "jointNames").and_then(|a| a.rest().and_then(Value::strs).map(|v| v.to_vec())).unwrap_or_default();
        let mats = |name: &str| -> Option<Vec<Mat4>> {
            let v = st.attr(p, name)?.rest()?.doubles()?.into_owned();
            if v.len() != joints.len() * 16 {
                return None;
            }
            Some(v.chunks_exact(16).map(|c| { let mut m = [0.0; 16]; m.copy_from_slice(c); m }).collect())
        };
        let bind = match mats("bindTransforms") {
            Some(b) => b,
            None => {
                if !joints.is_empty() {
                    self.warn(format!("skeleton {} has no valid bindTransforms; identity is used", short(&path)));
                }
                vec![math::IDENTITY; joints.len()]
            }
        };
        if bind.iter().flatten().any(|v| !v.is_finite()) {
            return Err(malformed(format!("skeleton {} has a non-finite bind transform", short(&path))));
        }
        // Parents from the joint paths.
        let parent_of = joint_parents(&joints);
        let rest = match mats("restTransforms") {
            Some(r) => r,
            None => {
                if !joints.is_empty() {
                    self.warn(format!("skeleton {} has no restTransforms; the bind pose is used", short(&path)));
                }
                (0..joints.len())
                    .map(|j| match parent_of[j] {
                        Some(pj) => math::invert(&bind[pj]).map(|inv| math::mat_mul(&inv, &bind[j])).unwrap_or(math::IDENTITY),
                        None => bind[j],
                    })
                    .collect()
            }
        };
        let mut joint_nodes = Vec::with_capacity(joints.len());
        for (j, jp) in joints.iter().enumerate() {
            if self.scene.nodes.len() >= super::scene::MAX_NODES {
                return Err(unsupported("too many nodes"));
            }
            let name = names.get(j).cloned().unwrap_or_else(|| jp.rsplit('/').next().unwrap_or(jp).to_string());
            if !rest[j].iter().all(|v| v.is_finite()) {
                return Err(malformed(format!("skeleton {} has a non-finite rest transform", short(&path))));
            }
            self.scene.nodes.push(Node { name: Some(name), transform: Transform::from_matrix(rest[j]), ..Default::default() });
            joint_nodes.push(self.scene.nodes.len() - 1);
        }
        for (j, &n) in joint_nodes.iter().enumerate() {
            match parent_of[j] {
                Some(pj) => self.scene.nodes[joint_nodes[pj]].children.push(n),
                None => match attach {
                    Some(ap) => self.scene.nodes[ap].children.push(n),
                    None => self.scene.roots.push(n),
                },
            }
        }
        // UsdSkel only poses skeletons inside a SkelRoot.
        let mut rooted = false;
        let mut cur = st.prims[p].parent;
        while let Some(c) = cur {
            if st.prims[c].type_name == "SkelRoot" {
                rooted = true;
                break;
            }
            cur = st.prims[c].parent;
        }
        if !rooted && !joints.is_empty() {
            self.warn(format!("skeleton {} is not inside a SkelRoot, so USD does not pose it or skin to it", short(&path)));
        }
        // Animation source: on the skeleton, or inherited from an ancestor.
        let mut anim = None;
        let mut cur = Some(p);
        while let Some(c) = cur {
            if let Some(t) = st.rel(c, "skel:animationSource").first() {
                anim = st.find(t).filter(|a| st.prims[*a].type_name == "SkelAnimation");
                if anim.is_none() {
                    self.warn(format!("skeleton {} names animation {}, which is not a SkelAnimation", short(&path), short(t)));
                }
                break;
            }
            cur = st.prims[c].parent;
        }
        if let Some(a) = anim {
            let ja = match self.joint_anims.get(&a) {
                Some(ja) => Rc::clone(ja),
                None => {
                    let ja = Rc::new(self.joint_anim(st, a));
                    self.joint_anims.insert(a, Rc::clone(&ja));
                    ja
                }
            };
            self.animate_joints(&ja, &joints, &joint_nodes, true)?;
            let name = self.display_name(a);
            // Only whether they share one name is read, so the list stops at
            // two: a check per skeleton, not a scan of every name so far.
            if self.anim_names.len() < 2 && !self.anim_names.contains(&name) {
                self.anim_names.push(name);
            }
        }
        self.skeleton_of.entry(p).or_insert(self.skeletons.len());
        self.skeletons.push(SkeletonInfo { prim: p, joint_nodes, joint_paths: joints, bind, anim, rooted });
        Ok(())
    }

    /// Animation `a` of `st` as a [`JointAnim`]: read from the stage once for
    /// each animation and composition ([`Reader::read_skeleton`] keeps the
    /// walked stage's, [`Reader::read_clip`] its composition's).
    fn joint_anim(&self, st: &Stage, a: usize) -> JointAnim {
        let names: Vec<String> = st.attr(a, "joints").and_then(|x| x.rest().and_then(Value::strs).map(|v| v.to_vec())).unwrap_or_default();
        let listed = names.len();
        let mut place = HashMap::with_capacity(listed);
        for (k, n) in names.into_iter().enumerate() {
            place.insert(n, k);
        }
        // Read only for an animation that lists no joints ([`Reader::animate_joints`]).
        let (weighs, ours) = if listed == 0 { (st.attr(a, "blendShapeWeights").is_some(), litegen_dict(st, a).is_some()) } else { (false, false) };
        // UsdSkel poses joints from an animation only when its translations,
        // rotations and scales all resolve, with their schema types
        // (usd/usdSkel/animQueryImpl.cpp#L118-L171, OpenUSD v26.11.1);
        // otherwise it shows the rest pose (usd/usdSkel/skeletonQuery.cpp#L92-L128).
        // Blend shape weights are independent of them.
        let mut missing = Vec::new();
        let mut attrs = Vec::new();
        let kinds = [("translations", "float3[]", AnimPath::Translation, 3), ("rotations", "quatf[]", AnimPath::Rotation, 4), ("scales", "half3[]", AnimPath::Scale, 3)];
        for (name, ty, path, width) in kinds.into_iter().filter(|_| listed > 0) {
            let Some(at) = st.attr(a, name).filter(|at| (at.type_name.is_empty() || at.type_name == ty) && at.rest().is_some()) else {
                missing.push(name);
                continue;
            };
            let shaped = |v: &Value| -> Option<Vec<f32>> {
                let f = v.floats()?.into_owned();
                (f.len() == listed * width).then_some(f)
            };
            let (mut times, mut samples, mut misshaped) = (Vec::new(), Vec::new(), false);
            if at.animated() {
                for (t, v) in &at.samples {
                    match shaped(v) {
                        Some(f) => {
                            times.push(self.seconds(*t));
                            samples.push(f);
                        }
                        None => misshaped = true,
                    }
                }
            }
            let rest = at.timeless().map(|v| shaped(v).ok_or(()));
            attrs.push(JointAttr { name, path, width, rest, animated: at.animated(), times, samples, misshaped });
        }
        JointAnim { path: st.prims[a].path.clone(), listed, place, weighs, ours, missing, attrs }
    }

    /// Joint animation `ja` as channels on a skeleton's joints (`joints`, its
    /// paths, and `joint_nodes`, their nodes). `selected` says it is the
    /// composition the file selects: only that one sets the joints' rest
    /// values, which every clip shares. A skeleton costs its own joints: the
    /// ones both list are found through the skeleton's names and taken in
    /// the animation's order, as a scan of its list met them.
    fn animate_joints(&mut self, ja: &JointAnim, joints: &[String], joint_nodes: &[usize], selected: bool) -> Result<(), MeshError> {
        let apath = short(&ja.path);
        if ja.listed == 0 {
            // It poses no joint, and USD shows the skeleton at rest. A clip
            // of blend shapes alone, or litegen's own clip that leaves this
            // skeleton alone (`customData.litegen`), means to; any other
            // animates nothing, and is named.
            if !ja.weighs && !ja.ours {
                self.warn(format!("animation {apath} lists no joints and no blend-shape weights, so it animates nothing; its skeleton stays at rest"));
            }
            return Ok(());
        }
        // (the animation's place, the skeleton's first place) of each name
        // both list. A name the animation lists twice is taken at its later
        // place, whose rest value the scan wrote last, and makes one channel
        // where the scan made one for each place: two channels on one node
        // and path, which glTF does not allow.
        let mut both: Vec<(usize, usize)> = first_places(joints).into_iter().filter_map(|(name, j)| ja.place.get(name).map(|&k| (k, j))).collect();
        both.sort_unstable();
        if both.len() < ja.place.len() {
            self.warn(format!("animation {apath} names joints its skeleton lacks; they are ignored"));
        }
        if !ja.missing.is_empty() {
            self.warn(format!("animation {apath} has no usable {}; USD poses its skeleton at rest, and so does this reader", ja.missing.join(" or ")));
            return Ok(());
        }
        for at in &ja.attrs {
            let width = at.width;
            match &at.rest {
                // Rest values: the animation's own, which override
                // restTransforms for the joints it lists.
                Some(Ok(v)) if selected => {
                    for &(k, j) in &both {
                        let node = joint_nodes[j];
                        let (mut t, mut r, mut s) = self.scene.nodes[node].transform.trs().unwrap_or(([0.0; 3], math::QUAT_IDENTITY, [1.0; 3]));
                        let x = &v[k * width..(k + 1) * width];
                        match at.path {
                            AnimPath::Translation => t = [x[0] as f64, x[1] as f64, x[2] as f64],
                            AnimPath::Rotation => r = math::quat_normalize([x[0] as f64, x[1] as f64, x[2] as f64, x[3] as f64]),
                            _ => s = [x[0] as f64, x[1] as f64, x[2] as f64],
                        }
                        self.scene.nodes[node].transform = Transform::Trs { translation: t, rotation: r, scale: s };
                    }
                }
                Some(Err(())) => {
                    self.warn(format!("{} of {apath} does not hold one value per joint; ignored", at.name));
                    continue;
                }
                _ => {}
            }
            if !at.animated {
                continue;
            }
            if at.misshaped {
                self.warn(format!("a sample of {} on {apath} does not hold one value per joint; skipped", at.name));
            }
            for &(k, j) in &both {
                let node = joint_nodes[j];
                // A joint that holds still at its rest value is not animated.
                let rest_vals: Vec<f32> = match (self.scene.nodes[node].transform.trs(), at.path) {
                    (Some((t, _, _)), AnimPath::Translation) => t.iter().map(|v| *v as f32).collect(),
                    (Some((_, r, _)), AnimPath::Rotation) => r.iter().map(|v| *v as f32).collect(),
                    (Some((_, _, s)), _) => s.iter().map(|v| *v as f32).collect(),
                    _ => Vec::new(),
                };
                let still = at.samples.iter().all(|f| {
                    let c = &f[k * width..(k + 1) * width];
                    c.iter().zip(&rest_vals).all(|(a, b)| (a - b).abs() <= 1e-6 * b.abs().max(1.0))
                        || (at.path == AnimPath::Rotation && rest_vals.len() == 4 && c.iter().zip(&rest_vals).all(|(a, b)| (a + b).abs() <= 1e-6))
                });
                if still {
                    continue;
                }
                self.charge_keys(at.times.len())?;
                let mut values: Vec<f32> = at.samples.iter().flat_map(|f| f[k * width..(k + 1) * width].iter().copied()).collect();
                if at.path == AnimPath::Rotation {
                    for q in values.chunks_exact_mut(4) {
                        let n = math::quat_normalize([q[0] as f64, q[1] as f64, q[2] as f64, q[3] as f64]);
                        for (o, v) in q.iter_mut().zip(n) {
                            *o = v as f32;
                        }
                    }
                }
                self.push_channel(Channel { node, path: at.path, interpolation: Interpolation::Linear, times: at.times.clone(), values });
            }
        }
        Ok(())
    }

    /// The skeleton a prim is bound to: `skel:skeleton` on it or its nearest
    /// ancestor that has one.
    /// UsdSkel binding property `name` of mesh `p`: its own, else the nearest
    /// ancestor's inside its SkelRoot, as the skinning cache inherits
    /// skel:joints, joint influences and geomBindTransform
    /// (usd/usdSkel/cacheImpl.cpp#L241-L328, OpenUSD v26.11.1). Joint
    /// influences pass down only constant, as the imaging path flattens them
    /// (usdImaging/usdSkelImaging/jointInfluencesData.cpp#L28-L41). A blocked
    /// value counts as none.
    fn skel_binding(&self, p: usize, name: &str) -> Option<Attr> {
        let st = self.st;
        let mut cur = Some(p);
        while let Some(c) = cur {
            if let Some(a) = st.attr(c, name).filter(|a| a.rest().is_some()) {
                let influences = name.starts_with("primvars:skel:joint");
                if c == p || !influences || a.interpolation.as_deref() == Some("constant") {
                    return Some(a);
                }
            }
            if st.prims[c].type_name == "SkelRoot" {
                return None;
            }
            cur = st.prims[c].parent;
        }
        None
    }

    /// Primvar `name` as gprim `p` draws it, and the prim that authors it:
    /// its own when it authors a value, of any interpolation, else the nearest
    /// ancestor's constant one (usd/usdGeom/primvarsAPI.h#L331-L333; Hydra
    /// flattens them alike, imaging/hd/flattenedPrimvarsDataSourceProvider.cpp
    /// #L168-L190). A blocked value counts as none, so an ancestor's shows
    /// through; an ancestor's non-constant primvar is not inherited.
    fn primvar_attr(&self, p: usize, name: &str) -> Option<(usize, Attr)> {
        let st = self.st;
        if let Some(a) = st.attr(p, name).filter(|a| a.rest().is_some()) {
            return Some((p, a));
        }
        if !name.starts_with("primvars:") {
            return None;
        }
        let mut cur = st.prims[p].parent;
        while let Some(c) = cur {
            let inherited = st.attr(c, name).filter(|a| a.rest().is_some() && a.interpolation.as_deref().unwrap_or("constant") == "constant");
            if let Some(a) = inherited {
                return Some((c, a));
            }
            cur = st.prims[c].parent;
        }
        None
    }

    fn bound_skeleton(&self, p: usize) -> Option<usize> {
        let st = self.st;
        let mut cur = Some(p);
        while let Some(c) = cur {
            if let Some(t) = st.rel(c, "skel:skeleton").first() {
                let k = st.find(t)?;
                return self.skeleton_of.get(&k).copied();
            }
            cur = st.prims[c].parent;
        }
        None
    }

    /// Bind each mesh with joint influences to its skeleton, as UsdSkel's linear
    /// blend skinning does: a point goes to skeleton space through the
    /// geomBindTransform, then through each joint's inverse bind and posed world
    /// transform, weighted (https://openusd.org/release/api/_usd_skel__intro.html#UsdSkel_PointSkinning;
    /// binding properties, https://openusd.org/release/api/_usd_skel__schemas.html#UsdSkel_BindingAPI_GeomBindTransform).
    fn bind_skeletons(&mut self) -> Result<(), MeshError> {
        let st = self.st;
        // Skins, keyed by skeleton and geomBindTransform.
        let mut skins: HashMap<(usize, [u64; 16]), usize> = HashMap::new();
        // Each skeleton's joints by path, for the skel:joints of every mesh
        // bound to it: built for its first such mesh, not for each.
        let mut by_path: HashMap<usize, HashMap<String, usize>> = HashMap::new();
        let skinned = std::mem::take(&mut self.skinned);
        for sm in skinned {
            let path = self.path(sm.prim).to_string();
            let Some(k) = self.bound_skeleton(sm.prim) else {
                self.warn(format!("mesh {} has joint influences but no skeleton; it is not skinned", short(&path)));
                continue;
            };
            if !self.skeletons[k].rooted {
                continue;
            }
            let n_joints = self.skeletons[k].joint_paths.len();
            // skel:joints remaps the mesh's indices onto the skeleton's order.
            let local: Option<Vec<String>> = self.skel_binding(sm.prim, "skel:joints").and_then(|a| a.rest().and_then(Value::strs).map(|v| v.to_vec()));
            let remap: Vec<usize> = match &local {
                Some(l) => {
                    // A path listed twice resolves to its last place, as it did.
                    let table = by_path.entry(k).or_insert_with(|| self.skeletons[k].joint_paths.iter().enumerate().map(|(i, s)| (s.clone(), i)).collect());
                    let mut out = Vec::with_capacity(l.len());
                    for j in l {
                        match table.get(j.as_str()).copied() {
                            Some(i) => out.push(i),
                            None => {
                                self.warn(format!("mesh {} names joint {} its skeleton lacks; it is not skinned", short(&path), short(j)));
                                out.clear();
                                break;
                            }
                        }
                    }
                    if out.is_empty() {
                        continue;
                    }
                    out
                }
                None => (0..n_joints).collect(),
            };
            let g: Mat4 = match self.skel_binding(sm.prim, "primvars:skel:geomBindTransform").and_then(|a| a.rest().and_then(Value::as_tuple).map(|t| t.to_vec())) {
                Some(t) if t.len() == 16 && t.iter().all(|v| v.is_finite()) => {
                    let mut m = [0.0; 16];
                    m.copy_from_slice(&t);
                    m
                }
                _ => math::IDENTITY,
            };
            let key = (k, g.map(f64::to_bits));
            let skin = match skins.get(&key) {
                Some(s) => *s,
                None => {
                    self.skin_entries = self.skin_entries.saturating_add(self.skeletons[k].joint_nodes.len());
                    if self.skin_entries > super::scene::MAX_SKIN_ENTRIES {
                        return Err(unsupported(format!(
                            "the meshes bound to skeleton {} need more than {} skin joint entries (one skin per distinct geomBindTransform)",
                            short(self.path(self.skeletons[k].prim)),
                            super::scene::MAX_SKIN_ENTRIES
                        )));
                    }
                    let info = &self.skeletons[k];
                    let mut ibms = Vec::with_capacity(info.bind.len());
                    for b in &info.bind {
                        let inv = math::invert(b).ok_or_else(|| malformed(format!("skeleton {} has a singular bind transform", short(self.path(info.prim)))))?;
                        ibms.push(math::mat_mul(&inv, &g));
                    }
                    // Joints no other joint holds as a child: one pass over
                    // their children, not every joint against every other.
                    let held: std::collections::HashSet<usize> =
                        info.joint_nodes.iter().flat_map(|o| self.scene.nodes[*o].children.iter().copied()).collect();
                    let roots: Vec<usize> = info.joint_nodes.iter().copied().filter(|j| !held.contains(j)).collect();
                    self.scene.skins.push(Skin {
                        name: Some(self.display_name(info.prim)),
                        joints: info.joint_nodes.clone(),
                        inverse_bind_matrices: ibms,
                        skeleton: (roots.len() == 1).then(|| roots[0]),
                    });
                    let s = self.scene.skins.len() - 1;
                    skins.insert(key, s);
                    s
                }
            };
            let es = sm.element_size;
            let influences = es.div_ceil(4) * 4;
            let mesh = &mut self.scene.meshes[sm.mesh];
            let mut bad = false;
            for (pi, prim) in mesh.primitives.iter_mut().enumerate() {
                let map = &sm.point_maps[pi];
                let mut joints = Vec::with_capacity(map.len() * influences);
                let mut weights = Vec::with_capacity(map.len() * influences);
                for &pt in map {
                    let base = if sm.constant { 0 } else { pt as usize * es };
                    if !sm.constant && pt as usize >= sm.point_count {
                        bad = true;
                        break;
                    }
                    for k2 in 0..influences {
                        if k2 < es {
                            let ji = sm.joint_indices[base + k2];
                            let w = sm.joint_weights[base + k2];
                            let mapped = usize::try_from(ji).ok().and_then(|i| remap.get(i).copied());
                            match mapped {
                                Some(m) if m < n_joints => {
                                    joints.push(m as u16);
                                    weights.push(if w.is_finite() { w } else { 0.0 });
                                }
                                _ => {
                                    if w != 0.0 {
                                        bad = true;
                                    }
                                    joints.push(0);
                                    weights.push(0.0);
                                }
                            }
                        } else {
                            joints.push(0);
                            weights.push(0.0);
                        }
                    }
                }
                prim.skin = Some(SkinWeights { influences, joints, weights });
            }
            if bad {
                return Err(malformed(format!("mesh {} has a joint index outside its skeleton", short(&path))));
            }
            self.scene.nodes[sm.node].skin = Some(skin);
        }
        // Blend-shape weights from each mesh's skeleton's animation.
        // The meshes stay listed: the clips other than the selected one read
        // their weights again ([`Reader::read_clip`]).
        let morphs = std::mem::take(&mut self.morphs);
        let mut weights: HashMap<usize, Option<ShapeWeights>> = HashMap::new();
        for mm in &morphs {
            let Some(k) = self.bound_skeleton(mm.prim) else { continue };
            if !self.skeletons[k].rooted {
                continue;
            }
            let Some(a) = self.skeletons[k].anim else { continue };
            if let Some(sw) = weights.entry(a).or_insert_with(|| shape_weights(st, a)) {
                self.read_weights(sw, mm, true)?;
            }
        }
        self.morphs = morphs;
        Ok(())
    }

    /// Mesh `mm`'s blend-shape weights from animation `a` of `st`: the rest
    /// weights when it is the `selected` composition (they are the mesh's,
    /// whatever the clip), and a channel when it animates them.
    fn read_weights(&mut self, sw: &ShapeWeights, mm: &MorphMesh, selected: bool) -> Result<(), MeshError> {
        let map: Vec<Option<usize>> = mm.names.iter().map(|n| sw.place.get(n.as_str()).copied()).collect();
        let pick = |f: &[f32]| -> Vec<f32> { map.iter().map(|m| m.map_or(0.0, |i| f[i])).collect() };
        if selected {
            if let Some(w) = sw.rest.as_deref().map(pick) {
                if w.iter().any(|x| *x != 0.0) {
                    self.scene.meshes[mm.mesh].weights = w;
                }
            }
        }
        if sw.animated {
            self.charge_keys(sw.samples.len())?;
            let mut times = Vec::new();
            let mut values = Vec::new();
            for (t, f) in &sw.samples {
                times.push(self.seconds(*t));
                values.extend(pick(f));
            }
            self.push_channel(Channel { node: mm.node, path: AnimPath::Weights, interpolation: Interpolation::Linear, times, values });
        }
        Ok(())
    }

    /// Charges the `n` keys a joint or blend-shape channel is about to hold
    /// to `anim_keys`, before its values are copied. Every skeleton or mesh
    /// an animation drives gets its own copy, so a crafted file with many
    /// sharing one long animation would build gigabytes before the finished
    /// clips are held to `MAX_ANIMATION_KEYS`; this refuses it as it goes.
    fn charge_keys(&mut self, n: usize) -> Result<(), MeshError> {
        self.anim_keys = self.anim_keys.saturating_add(n);
        if self.anim_keys > super::scene::MAX_ANIMATION_KEYS {
            return Err(unsupported("the joint and blend-shape channels need more keys than this converter will hold"));
        }
        Ok(())
    }

    // ── clips ───────────────────────────────────────────────────────────────

    /// The default prim's `Animation` variants other than the selected one, as
    /// clips after it. The selected variant is what the walk read, so its
    /// clip is the first; each other is one more composition of the stage
    /// with that variant selected (`Stage::recompose_selecting`: the layers
    /// are shared, nothing is parsed again), of which only the skeletons'
    /// animation is read, since a rigid transform is no variant's (the
    /// writer's layout, `usd_write`). Their order is the set's. The
    /// compositions are charged to `MAX_VARIANT_PRIMS`; past it the clips
    /// that remain are left out, with a warning.
    fn read_other_clips(&mut self) -> Result<(), MeshError> {
        let st = self.st;
        let Some(av) = st.animation_variants()? else { return Ok(()) };
        // The clip the file selects ends where its SkelAnimations say.
        let bound: Vec<usize> = self.skeletons.iter().filter_map(|s| s.anim).collect();
        if let (Some(end), Some(first)) = (clip_end(st, &bound), self.scene.animations.first_mut()) {
            hold_until(&mut first.channels, end);
        }
        let mut keys: usize = self.scene.animations.iter().flat_map(|a| &a.channels).map(|c| c.times.len()).fold(0, usize::saturating_add);
        let mut composed = 0usize;
        for variant in av.variants.iter().filter(|v| **v != av.selected) {
            // Estimated by the stage as it stands, then corrected by the
            // composition's own count.
            composed = composed.saturating_add(st.prims.len());
            if composed > MAX_VARIANT_PRIMS {
                self.warn(format!(
                    "the {} variants of {} compose to more than {MAX_VARIANT_PRIMS} prims together; the clips from {variant:?} on were left out",
                    av.variants.len(),
                    short(&av.prim)
                ));
                break;
            }
            let vs = match st.recompose_selecting(&av.prim, super::usd_stage::ANIMATION_SET, variant) {
                Ok(vs) => vs,
                Err(e) => {
                    self.warn(format!("variant {variant:?} of {} could not be composed ({e}); it is not read as a clip", short(&av.prim)));
                    continue;
                }
            };
            composed = composed.saturating_add(vs.prims.len().saturating_sub(st.prims.len()));
            for w in &vs.warnings {
                self.warn(w.clone());
            }
            let Some(clip) = self.read_clip(&vs, variant)? else { continue };
            keys = clip.channels.iter().map(|c| c.times.len()).fold(keys, usize::saturating_add);
            if keys > super::scene::MAX_ANIMATION_KEYS {
                return Err(unsupported("the animations have more keys than this converter will hold"));
            }
            self.scene.animations.push(clip);
        }
        Ok(())
    }

    /// The clip `vs`, a composition with another variant selected, holds:
    /// each skeleton's joint animation and each blend-shape mesh's weights
    /// as that composition binds them, none of the rest values (the joints'
    /// are the selected clip's, which the writer shares across clips). Named
    /// for its SkelAnimation (the one name they share, restored from
    /// `customData` as everywhere), else for the variant. `None` when it
    /// animates nothing.
    fn read_clip(&mut self, vs: &Stage, variant: &str) -> Result<Option<Animation>, MeshError> {
        let outer = std::mem::take(&mut self.channels);
        let mut anims: Vec<usize> = Vec::new();
        let mut read: HashMap<usize, JointAnim> = HashMap::new();
        for k in 0..self.skeletons.len() {
            let Some(sp) = vs.find(self.path(self.skeletons[k].prim)) else { continue };
            let Some(a) = bound_animation(vs, sp) else { continue };
            anims.push(a);
            read.entry(a).or_insert_with(|| self.joint_anim(vs, a));
            // Lent, not copied, for each skeleton of each clip.
            let joints = std::mem::take(&mut self.skeletons[k].joint_paths);
            let nodes = std::mem::take(&mut self.skeletons[k].joint_nodes);
            let done = self.animate_joints(&read[&a], &joints, &nodes, false);
            self.skeletons[k].joint_paths = joints;
            self.skeletons[k].joint_nodes = nodes;
            done?;
        }
        let morphs = std::mem::take(&mut self.morphs);
        let mut weights: HashMap<usize, Option<ShapeWeights>> = HashMap::new();
        for mm in &morphs {
            let Some(k) = self.bound_skeleton(mm.prim) else { continue };
            if !self.skeletons[k].rooted {
                continue;
            }
            let Some(sp) = vs.find(self.path(self.skeletons[k].prim)) else { continue };
            let Some(a) = bound_animation(vs, sp) else { continue };
            if let Some(sw) = weights.entry(a).or_insert_with(|| shape_weights(vs, a)) {
                self.read_weights(sw, mm, false)?;
            }
        }
        self.morphs = morphs;
        let mut channels = std::mem::replace(&mut self.channels, outer);
        channels.retain(|c| c.path != AnimPath::Weights || self.scene.nodes[c.node].mesh.is_some_and(|m| self.scene.meshes[m].target_count() > 0));
        if channels.is_empty() {
            return Ok(None);
        }
        if let Some(lo) = super::pose::start_at_zero(&mut channels) {
            self.warn(format!("the clip {variant:?} starts at {lo}s; it was shifted to start at 0"));
        }
        if let Some(end) = clip_end(vs, &anims) {
            hold_until(&mut channels, end);
        }
        let mut names: Vec<String> = anims.iter().map(|&a| vs.custom_string(a, NAME_KEY).unwrap_or_else(|| vs.prims[a].name.clone())).collect();
        names.sort();
        names.dedup();
        let name = if names.len() == 1 { names.remove(0) } else { variant.to_string() };
        Ok(Some(Animation { name: Some(name), channels }))
    }

    fn finish_animation(&mut self) -> Result<(), MeshError> {
        let mut channels = std::mem::take(&mut self.channels);
        // Channels on nodes that ended up without the data they animate.
        channels.retain(|c| {
            c.path != AnimPath::Weights || self.scene.nodes[c.node].mesh.is_some_and(|m| self.scene.meshes[m].target_count() > 0)
        });
        if channels.is_empty() {
            return Ok(());
        }
        if let Some(lo) = super::pose::start_at_zero(&mut channels) {
            self.warn(format!("the animation starts at {lo}s; it was shifted to start at 0"));
        }
        let name = self
            .st
            .meta("customLayerData")
            .and_then(Value::dict)
            .and_then(|d| d.iter().find(|(k, _)| k == "animationName"))
            .and_then(|(_, v)| v.as_str().map(str::to_string))
            .or_else(|| (self.anim_names.len() == 1).then(|| self.anim_names[0].clone()));
        let mut total = 0usize;
        for c in &channels {
            total = total.saturating_add(c.times.len());
        }
        if total > super::scene::MAX_ANIMATION_KEYS {
            return Err(unsupported("the animation has more keys than this converter will hold"));
        }
        self.scene.animations.push(Animation { name, channels });
        Ok(())
    }

    fn up_axis_and_units(&mut self) -> Result<(), MeshError> {
        let st = self.st;
        let up = st.meta("upAxis").and_then(Value::as_str).unwrap_or("Y").to_string();
        let (mpu, why) = meters_per_unit(st);
        if let Some(why) = why {
            self.warn(why.into());
        }
        let rot = match up.as_str() {
            "Y" => math::QUAT_IDENTITY,
            "Z" => math::quat_from_axis_angle([1.0, 0.0, 0.0], -std::f64::consts::FRAC_PI_2),
            other => {
                self.warn(format!("upAxis {other:?} is not Y or Z; Y is assumed"));
                math::QUAT_IDENTITY
            }
        };
        if rot == math::QUAT_IDENTITY && mpu == 1.0 {
            return Ok(());
        }
        let c = math::compose([0.0; 3], rot, [mpu; 3]);
        let roots = self.scene.roots.clone();
        for r in &roots {
            let n = &mut self.scene.nodes[*r];
            n.transform = match n.transform.trs() {
                Some((t, q, s)) => Transform::Trs {
                    translation: math::transform_point_f64(&c, t),
                    rotation: math::quat_normalize(math::quat_mul(rot, q)),
                    scale: [s[0] * mpu, s[1] * mpu, s[2] * mpu],
                },
                None => Transform::Matrix(math::mat_mul(&c, &n.transform.matrix())),
            };
        }
        for a in self.scene.animations.iter_mut() {
            for ch in a.channels.iter_mut() {
                if !roots.contains(&ch.node) {
                    continue;
                }
                match ch.path {
                    AnimPath::Translation => {
                        for v in ch.values.chunks_exact_mut(3) {
                            let p = math::transform_point_f64(&c, [v[0] as f64, v[1] as f64, v[2] as f64]);
                            v.copy_from_slice(&[p[0] as f32, p[1] as f32, p[2] as f32]);
                        }
                    }
                    AnimPath::Rotation => {
                        for v in ch.values.chunks_exact_mut(4) {
                            let q = math::quat_mul(rot, [v[0] as f64, v[1] as f64, v[2] as f64, v[3] as f64]);
                            v.copy_from_slice(&[q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32]);
                        }
                    }
                    AnimPath::Scale => {
                        for v in ch.values.iter_mut() {
                            *v = (*v as f64 * mpu) as f32;
                        }
                    }
                    AnimPath::Weights => {}
                }
            }
        }
        Ok(())
    }
}

/// The stage's metres per unit, and the warning when it is not declared
/// (USD's fallback, centimetres) or is not a positive number (metres).
fn meters_per_unit(st: &Stage) -> (f64, Option<&'static str>) {
    match st.meta("metersPerUnit").and_then(Value::as_f64) {
        Some(m) if m.is_finite() && m > 0.0 => (m, None),
        Some(_) => (1.0, Some("metersPerUnit is not a positive number; metres are assumed")),
        None => (0.01, Some("the stage does not declare metersPerUnit; USD's fallback of centimetres was applied")),
    }
}

/// A DistantLight's `sizeFactor` for an `angle` in degrees
/// (usdLux/schema.usda L306-L331): with θ half the angle, clamped to 0..π,
/// 1 at θ = 0, π·sin²θ up to π/2, and (2 − sin²θ)·π past it. A normalised
/// light's luminance is its intensity over this, so its illuminance is
/// its intensity (L334-L337); an unnormalised one's illuminance is its
/// intensity times this.
fn distant_size_factor(angle: f64) -> f64 {
    let theta = (angle.to_radians() / 2.0).clamp(0.0, std::f64::consts::PI);
    let s2 = theta.sin().powi(2);
    if theta == 0.0 {
        1.0
    } else if theta <= std::f64::consts::FRAC_PI_2 {
        std::f64::consts::PI * s2
    } else {
        (2.0 - s2) * std::f64::consts::PI
    }
}

/// Each name of `names` with its first place in the list: the table a lookup
/// by name goes through, so it costs one hash and not a scan of the list. A
/// name listed twice keeps its first place, the one `position` finds.
fn first_places(names: &[String]) -> HashMap<&str, usize> {
    let mut place = HashMap::with_capacity(names.len());
    for (i, n) in names.iter().enumerate() {
        place.entry(n.as_str()).or_insert(i);
    }
    place
}

/// Each joint's parent among `joints`, as UsdSkel's topology takes it from
/// the paths (usd/usdSkel/topology.cpp, `_GetParentIndex`): the nearest of
/// its ancestors that is listed (`a/b`, else `a`, for `a/b/c`), at its first
/// place, or `None`. The paths are sorted part by part, which puts each
/// path's descendants right after it, and one pass keeps the chain of
/// listed ancestors open on a stack. That costs a sort and the paths'
/// length, where a scan of the list for each ancestor of each joint took
/// minutes on 65,535 joints whose ancestors are not listed; and it holds no
/// more than the list, where a tree of the paths' parts would hold one node
/// for each `/` of a hostile path.
fn joint_parents(joints: &[String]) -> Vec<Option<usize>> {
    let mut order: Vec<usize> = (0..joints.len()).collect();
    // Stable: a path listed twice keeps its first place first.
    order.sort_by(|&a, &b| joints[a].split('/').cmp(joints[b].split('/')));
    let mut parent = vec![None; joints.len()];
    let mut open: Vec<usize> = Vec::new();
    for &j in &order {
        let path = joints[j].as_str();
        if let Some(&top) = open.last() {
            if joints[top] == path {
                // The same path again: the first place's parent.
                parent[j] = parent[top];
                continue;
            }
        }
        // `a` is an ancestor of `path` when `path` is `a/…`.
        while let Some(&top) = open.last() {
            let a = joints[top].as_str();
            if path.len() > a.len() && path.starts_with(a) && path.as_bytes()[a.len()] == b'/' {
                break;
            }
            open.pop();
        }
        parent[j] = open.last().copied();
        open.push(j);
    }
    parent
}

/// A SkelAnimation's joint motion as every skeleton bound to it reads it,
/// read from the stage and shaped once for each animation and composition
/// ([`Reader::joint_anim`]): a skeleton then costs its own joints, not the
/// animation's whole list and samples. Skeletons that share one animation
/// (inherited from a SkelRoot, say) had it copied and scanned for each,
/// which a hostile file made take minutes.
struct JointAnim {
    path: String,
    /// How many joints it lists.
    listed: usize,
    /// Each name it lists, at its later place when it lists one twice.
    place: HashMap<String, usize>,
    /// Whether it weighs blend shapes, and whether it is a clip of litegen's
    /// own (`customData.litegen`): read only when it lists no joints.
    weighs: bool,
    ours: bool,
    /// The transform attributes that do not resolve with their schema types,
    /// which leave the skeleton at rest.
    missing: Vec<&'static str>,
    /// Its translations, rotations and scales, when none is missing.
    attrs: Vec<JointAttr>,
}

/// One of a [`JointAnim`]'s transform attributes, whose values hold an
/// element of `width` for each listed joint.
struct JointAttr {
    name: &'static str,
    path: AnimPath,
    width: usize,
    /// Its timeless value; `Err` when it does not hold one per joint.
    rest: Option<Result<Vec<f32>, ()>>,
    animated: bool,
    /// The samples that hold one per joint, with their times in seconds.
    times: Vec<f32>,
    samples: Vec<Vec<f32>>,
    /// Whether a sample did not, and was skipped.
    misshaped: bool,
}

/// What every mesh an animation weighs reads of its blend shapes: each
/// name's first place in `blendShapes`, and `blendShapeWeights`' rest value
/// and samples that hold one weight per name. Read once for each animation
/// and composition, not once for each mesh: a hostile file's meshes times
/// its names took minutes.
struct ShapeWeights {
    place: HashMap<String, usize>,
    rest: Option<Vec<f32>>,
    animated: bool,
    /// The samples that hold one weight per name: time code, weights.
    samples: Vec<(f64, Vec<f32>)>,
}

/// Animation `a`'s [`ShapeWeights`] in `st`, or `None` when it has no
/// `blendShapeWeights`.
fn shape_weights(st: &Stage, a: usize) -> Option<ShapeWeights> {
    let names: Vec<String> = st.attr(a, "blendShapes").and_then(|x| x.rest().and_then(Value::strs).map(|v| v.to_vec())).unwrap_or_default();
    let w = st.attr(a, "blendShapeWeights")?;
    let count = names.len();
    let mut place = HashMap::with_capacity(count);
    for (i, n) in names.into_iter().enumerate() {
        place.entry(n).or_insert(i);
    }
    let fit = |v: &Value| v.floats().filter(|f| f.len() == count).map(|f| f.into_owned());
    let samples = if w.animated() { w.samples.iter().filter_map(|(t, v)| Some((*t, fit(v)?))).collect() } else { Vec::new() };
    Some(ShapeWeights { place, rest: w.timeless().and_then(fit), animated: w.animated(), samples })
}

/// The SkelAnimation skeleton prim `p` of `st` plays: `skel:animationSource`
/// on it, or on the nearest ancestor that has one.
fn bound_animation(st: &Stage, p: usize) -> Option<usize> {
    let mut cur = Some(p);
    while let Some(c) = cur {
        if let Some(t) = st.rel(c, "skel:animationSource").first() {
            return st.find(t).filter(|a| st.prims[*a].type_name == "SkelAnimation");
        }
        cur = st.prims[c].parent;
    }
    None
}

/// Where the clip of SkelAnimations `anims` of `st` ends, in seconds: the
/// latest `customData.litegen.clipEnd` they author (`usd_write` records each
/// clip's end there, since a SkelAnimation holds only the keys of its joints).
/// A value that is not a positive finite number is no end.
fn clip_end(st: &Stage, anims: &[usize]) -> Option<f32> {
    anims
        .iter()
        .filter_map(|&a| litegen_dict(st, a))
        .filter_map(Value::dict)
        .filter_map(|d| entry_number(d, "clipEnd").ok().flatten())
        .map(|e| e as f32)
        .filter(|e| e.is_finite() && *e > 0.0)
        .reduce(f32::max)
}

/// `channels` held still until `end` seconds: the channel that ends last gets
/// one more key, at `end`, with its last value, when `end` is later than it.
fn hold_until(channels: &mut [Channel], end: f32) {
    let Some(last) = channels
        .iter_mut()
        .filter(|c| !c.times.is_empty())
        .reduce(|a, b| if b.times.last() > a.times.last() { b } else { a })
    else {
        return;
    };
    if last.times.last().is_some_and(|t| end > *t) {
        let stride = last.values.len() / last.times.len();
        let tail = last.values[last.values.len() - stride..].to_vec();
        last.times.push(end);
        last.values.extend(tail);
    }
}

/// Prim `p` of `st`'s `customData.litegen` entry (`LITEGEN_KEY`), from its
/// strongest opinion that authors one, as `Stage::custom_string` takes a name.
fn litegen_dict(st: &Stage, p: usize) -> Option<&Value> {
    for s in &st.prims[p].sources {
        let Some(data) = st.layers.get(s.layer).and_then(|l| l.specs.get(&s.path)).and_then(|spec| spec.custom_data.as_ref()) else {
            continue;
        };
        if let Some((_, v)) = data.iter().find(|(k, _)| k == LITEGEN_KEY) {
            return Some(v);
        }
    }
    None
}

/// The entry `key` of a litegen dictionary as a number: `Ok(None)` when it
/// has none, or why it is not one.
fn entry_number(d: &[(String, Value)], key: &str) -> Result<Option<f64>, String> {
    match d.iter().find(|(k, _)| k == key) {
        None => Ok(None),
        Some((_, v)) => v.as_f64().map(Some).ok_or_else(|| format!("its {key} is not a number")),
    }
}

/// A litegen dictionary's `type` and `name`, the type checked against the
/// one the prim gives (`wanted`), so a tool that changed the prim without
/// it is noticed.
fn entry_kind(d: &[(String, Value)], wanted: &str) -> Result<Option<String>, String> {
    let text = |key: &str| d.iter().find(|(k, _)| k == key).and_then(|(_, v)| v.as_str());
    let ty = text("type").ok_or("it has no type")?;
    if ty != wanted {
        return Err(format!("its type {:?} is not the prim's, {wanted}", short(ty)));
    }
    Ok(text("name").map(str::to_string))
}

/// The light a litegen dictionary holds: the scene's own name, colour,
/// intensity, range and cones, as the writer wrote them (`usd_write`), the
/// range in scene units. `kind` comes from the prim, whose type the
/// dictionary must name, with any cones.
fn exact_light(data: &Value, kind: LightKind, mpu: f64) -> Result<(Light, Vec<String>), String> {
    let d = data.dict().ok_or("it is not a dictionary")?;
    let need = |key: &str| entry_number(d, key)?.ok_or_else(|| format!("it has no {key}"));
    let wanted = match kind {
        LightKind::Directional => "directional",
        LightKind::Point => "point",
        LightKind::Spot { .. } => "spot",
    };
    let name = entry_kind(d, wanted)?;
    let color = d.iter().find(|(k, _)| k == "color").and_then(|(_, v)| v.as_tuple()).filter(|t| t.len() == 3).ok_or("its color is not three numbers")?;
    let intensity = need("intensity")?;
    let kind = match kind {
        LightKind::Spot { .. } => LightKind::Spot { inner_cone: need("innerConeAngle")? as f32, outer_cone: need("outerConeAngle")? as f32 },
        other => other,
    };
    let mut notes = Vec::new();
    let range = match (kind, entry_number(d, "range")?) {
        (LightKind::Directional, Some(r)) => {
            notes.push(format!("is directional, so the range {} its litegen customData gives is ignored", r as f32));
            None
        }
        (_, r) => r.map(|r| (r * mpu) as f32),
    };
    let light = Light { name, kind, color: [color[0] as f32, color[1] as f32, color[2] as f32], intensity: intensity as f32, range };
    Ok((light, notes))
}

/// The camera a litegen dictionary holds: the scene's own name and
/// projection, as the writer wrote them, an unset aspect ratio or far plane
/// left out, lengths in scene units. `perspective` comes from the prim's
/// `projection`, which the dictionary's type must name.
fn exact_camera(data: &Value, perspective: bool, mpu: f64) -> Result<Camera, String> {
    let d = data.dict().ok_or("it is not a dictionary")?;
    let need = |key: &str| entry_number(d, key)?.ok_or_else(|| format!("it has no {key}"));
    let length = |key: &str| need(key).map(|v| (v * mpu) as f32);
    let name = entry_kind(d, if perspective { "perspective" } else { "orthographic" })?;
    let projection = if perspective {
        Projection::Perspective {
            yfov: need("yfov")? as f32,
            aspect: entry_number(d, "aspectRatio")?.map(|a| a as f32),
            znear: length("znear")?,
            zfar: entry_number(d, "zfar")?.map(|f| (f * mpu) as f32),
        }
    } else {
        Projection::Orthographic { xmag: length("xmag")?, ymag: length("ymag")?, znear: length("znear")?, zfar: length("zfar")? }
    };
    Ok(Camera { name, projection })
}

/// A separately mapped metallic or roughness input.
enum MrSide {
    Constant(f32),
    Map { tex: TexRead, channel: usize, scale: f32, bias: f32 },
}

struct TexRead {
    /// `uv_set` is in the material's read order (`MaterialReads::uv_names`).
    r: TextureRef,
    /// The texture's `inputs:scale`: glTF's factor.
    scale: [f32; 4],
}

enum Resolved {
    Value(Value, usize),
    Output(usize, String),
    None,
}

/// Whether an op stack is translate, orient, scale (each optional, in that
/// order, none inverted): glTF's own TRS.
fn trs_shaped(ops: &[(String, bool, Attr)]) -> bool {
    let kinds: Vec<&str> = ops.iter().map(|(n, _, _)| op_kind(n)).collect();
    ops.iter().all(|(_, inv, _)| !inv)
        && matches!(
            kinds.as_slice(),
            ["translate"] | ["orient"] | ["scale"] | ["translate", "orient"] | ["translate", "scale"] | ["orient", "scale"]
                | ["translate", "orient", "scale"]
        )
}

/// The rest TRS of a TRS-shaped op stack, read from the op values.
fn trs_of(ops: &[(String, bool, Attr)]) -> Option<Transform> {
    if !trs_shaped(ops) {
        return None;
    }
    let (mut translation, mut rotation, mut scale) = ([0.0; 3], math::QUAT_IDENTITY, [1.0; 3]);
    for (name, _, a) in ops {
        let Some(v) = a.timeless() else { continue };
        match op_kind(name) {
            "translate" => {
                let t = v.as_tuple().filter(|t| t.len() == 3)?;
                translation = [t[0], t[1], t[2]];
            }
            "orient" => {
                let q = v.as_tuple().filter(|t| t.len() == 4)?;
                rotation = math::quat_normalize([q[0], q[1], q[2], q[3]]);
            }
            _ => {
                scale = match v {
                    Value::Tuple(t) if t.len() == 3 => [t[0], t[1], t[2]],
                    other => {
                        let d = other.as_f64()?;
                        [d, d, d]
                    }
                };
            }
        }
    }
    Some(Transform::Trs { translation, rotation, scale })
}

/// The kind of an xformOp attribute name: `xformOp:rotateXYZ:pivot` →
/// `rotateXYZ`.
fn op_kind(name: &str) -> &str {
    name.strip_prefix("xformOp:").map_or(name, |r| r.split(':').next().unwrap_or(r))
}

fn op_value(a: &Attr, time: Option<f64>, quat: bool) -> Option<Value> {
    match time {
        None => a.timeless().cloned(),
        Some(t) => {
            if a.samples.is_empty() {
                return a.rest().cloned();
            }
            Some(sample_value(&a.samples, t, quat))
        }
    }
}

/// USD's linear interpolation of one attribute's samples at `t` (held outside
/// the sampled range): component-wise for vectors and matrices, slerp for
/// quaternions — exactly what `UsdAttribute::Get` returns.
fn sample_value(samples: &[(f64, Value)], t: f64, quat: bool) -> Value {
    let n = samples.len();
    if t <= samples[0].0 || n == 1 {
        return samples[0].1.clone();
    }
    if t >= samples[n - 1].0 {
        return samples[n - 1].1.clone();
    }
    let k = samples.partition_point(|(st, _)| *st <= t) - 1;
    let (t0, a) = &samples[k];
    let (t1, b) = &samples[k + 1];
    let u = (t - t0) / (t1 - t0);
    match (a, b) {
        (Value::Tuple(x), Value::Tuple(y)) if x.len() == y.len() => {
            if quat && x.len() == 4 {
                let q = math::slerp([x[0], x[1], x[2], x[3]], [y[0], y[1], y[2], y[3]], u);
                return Value::Tuple(q.to_vec());
            }
            Value::Tuple(x.iter().zip(y).map(|(p, q)| p + (q - p) * u).collect())
        }
        (Value::Float(x), Value::Float(y)) => Value::Float(x + (y - x) * u),
        _ => a.clone(),
    }
}

/// The local matrix of an op stack at `time` (or at rest).
fn compose_ops(ops: &[(String, bool, Attr)], time: Option<f64>) -> Result<Mat4, String> {
    let mut m = math::IDENTITY;
    for (name, invert, a) in ops {
        let kind = op_kind(name);
        let v = op_value(a, time, kind == "orient");
        let op = match (kind, &v) {
            (_, None) => math::IDENTITY,
            ("translate", Some(v)) => {
                let t = v.as_tuple().filter(|t| t.len() == 3).ok_or(format!("{name} is not a 3-vector"))?;
                math::translation([t[0], t[1], t[2]])
            }
            ("translateX" | "translateY" | "translateZ", Some(v)) => {
                let d = v.as_f64().ok_or(format!("{name} is not a number"))?;
                let mut t = [0.0; 3];
                t[(kind.as_bytes()[9] - b'X') as usize] = d;
                math::translation(t)
            }
            ("scale", Some(v)) => {
                let s = match v {
                    Value::Tuple(t) if t.len() == 3 => [t[0], t[1], t[2]],
                    other => {
                        let d = other.as_f64().ok_or(format!("{name} is not a 3-vector"))?;
                        [d, d, d]
                    }
                };
                math::scaling(s)
            }
            ("scaleX" | "scaleY" | "scaleZ", Some(v)) => {
                let d = v.as_f64().ok_or(format!("{name} is not a number"))?;
                let mut s = [1.0; 3];
                s[(kind.as_bytes()[5] - b'X') as usize] = d;
                math::scaling(s)
            }
            ("rotateX" | "rotateY" | "rotateZ", Some(v)) => {
                let deg = v.as_f64().ok_or(format!("{name} is not a number"))?;
                let mut axis = [0.0; 3];
                axis[(kind.as_bytes()[6] - b'X') as usize] = 1.0;
                math::quat_to_mat(math::quat_from_axis_angle(axis, deg.to_radians()))
            }
            ("rotateXYZ" | "rotateXZY" | "rotateYXZ" | "rotateYZX" | "rotateZXY" | "rotateZYX", Some(v)) => {
                let e = v.as_tuple().filter(|t| t.len() == 3).ok_or(format!("{name} is not a 3-vector"))?;
                let order = match kind {
                    "rotateXYZ" => math::EulerOrder::Xyz,
                    "rotateXZY" => math::EulerOrder::Xzy,
                    "rotateYXZ" => math::EulerOrder::Yxz,
                    "rotateYZX" => math::EulerOrder::Yzx,
                    "rotateZXY" => math::EulerOrder::Zxy,
                    _ => math::EulerOrder::Zyx,
                };
                math::quat_to_mat(math::euler_to_quat([e[0].to_radians(), e[1].to_radians(), e[2].to_radians()], order))
            }
            ("orient", Some(v)) => {
                let q = v.as_tuple().filter(|t| t.len() == 4).ok_or(format!("{name} is not a quaternion"))?;
                math::quat_to_mat(math::quat_normalize([q[0], q[1], q[2], q[3]]))
            }
            ("transform", Some(v)) => {
                let t = v.as_tuple().filter(|t| t.len() == 16).ok_or(format!("{name} is not a 4x4 matrix"))?;
                let mut mm = [0.0; 16];
                mm.copy_from_slice(t);
                mm
            }
            _ => return Err(format!("unknown xformOp {name:?}")),
        };
        let op = if *invert { math::invert(&op).ok_or(format!("{name} cannot be inverted"))? } else { op };
        m = math::mat_mul(&m, &op);
    }
    Ok(m)
}

/// Key times plus evenly spaced samples between them, for animation whose
/// USD interpolation the scene's cannot reproduce from the keys alone.
/// Refused, before anything is allocated, when that is more than `budget`
/// samples: each window is capped, but the rate comes from the file, and
/// 3,000 windows at 1e-9 time codes per second were 30M samples.
fn densify(times: &[f64], tcps: f64, budget: usize) -> Result<Vec<f64>, MeshError> {
    let step = tcps / RESAMPLE_FPS;
    let per_window = |w: &[f64]| ((w[1] - w[0]) / step).ceil().clamp(1.0, 10_000.0) as usize;
    let total = times.windows(2).map(per_window).fold(1usize, usize::saturating_add);
    if total > budget {
        return Err(unsupported(format!("the animation densifies to {total} keys, more than this converter will hold")));
    }
    let mut out = Vec::with_capacity(total);
    for w in times.windows(2) {
        out.push(w[0]);
        let n = per_window(w);
        for i in 1..n {
            out.push(w[0] + (w[1] - w[0]) * i as f64 / n as f64);
        }
    }
    if let Some(l) = times.last() {
        out.push(*l);
    }
    Ok(out)
}

/// A UsdTransform2d (degrees, scale, translation; `in * scale * rotate +
/// translation`, applied to `st`) as the scene's `KHR_texture_transform` on
/// `uv = F(st)`, `F(s, t) = (s, 1 − t)`.
///
/// The scene's transform is `K(uv) = o + R(θ)·S·uv` with `R(θ) = [[c, s], [−s,
/// c]]` (see `scene::UvTransform::apply`). USD's is `U(st) = Rccw(φ)·S·st + t`.
/// Sampling must agree: `K = F ∘ U ∘ F`. Writing `F(x) = D·x + e₂` with `D =
/// diag(1, −1)`: the linear part is `D·Rccw(φ)·S·D = Rccw(−φ)·S = R(φ)·S`, so
/// θ = φ; the offset is `D·t + e₂ − D·Rccw(φ)·S·e₂`, i.e. `o = (tx − sy·sin φ,
/// 1 − ty − sy·cos φ)`.
pub fn uv_transform_from_usd(rotation_deg: f64, scale: [f64; 2], translation: [f64; 2]) -> UvTransform {
    let phi = rotation_deg.to_radians();
    let (s, c) = phi.sin_cos();
    UvTransform {
        offset: [(translation[0] - scale[1] * s) as f32, (1.0 - translation[1] - scale[1] * c) as f32],
        rotation: phi as f32,
        scale: [scale[0] as f32, scale[1] as f32],
    }
}

/// The inverse of [`uv_transform_from_usd`]: `(degrees, scale, translation)`.
#[cfg(test)]
fn uv_transform_to_usd(t: &UvTransform) -> (f64, [f64; 2], [f64; 2]) {
    let phi = t.rotation as f64;
    let (s, c) = phi.sin_cos();
    let sy = t.scale[1] as f64;
    (phi.to_degrees(), [t.scale[0] as f64, sy], [t.offset[0] as f64 + sy * s, 1.0 - t.offset[1] as f64 - sy * c])
}

/// Bake texture transforms into UVs when that is exact: every texture a
/// material samples from one uv set uses the same transform. Files that keep
/// glTF's V-down data and flip it in the shading network (Apple's importer)
/// then read back as the plain coordinates they started as.
fn bake_uniform_texture_transforms(scene: &mut Scene) {
    let uniform = scene.materials.iter().all(|m| {
        let refs: Vec<&TextureRef> = m.texture_refs().map(|(_, r)| r).collect();
        refs.iter().all(|a| refs.iter().all(|b| a.uv_set != b.uv_set || a.transform == b.transform))
    });
    if uniform && scene.materials.iter().any(|m| m.texture_refs().any(|(_, r)| r.transform.is_some())) {
        super::pose::bake_texture_transforms(scene);
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures;
    use super::super::package::{MemorySidecars, PackageFile};
    use super::*;

    /// `joint_parents` gives what scanning the list for each ancestor gave
    /// (the reader before 2026-11-10, kept here as the oracle): the nearest
    /// listed ancestor at its first place, across a parent listed after its
    /// child, an ancestor that is not listed, a part that sorts before `/`
    /// (`a-b`), an empty part, an empty path and a leading `/`, and paths
    /// listed twice. The regression it would catch: the sort or the stack
    /// losing a parent, so that a joint reads as a root and its mesh moves.
    #[test]
    fn joint_parents_are_the_nearest_listed_ancestors() {
        let scan = |joints: &[String]| -> Vec<Option<usize>> {
            joints
                .iter()
                .map(|j| {
                    let mut cur = j.as_str();
                    while let Some(i) = cur.rfind('/') {
                        cur = &cur[..i];
                        if let Some(pi) = joints.iter().position(|o| o == cur) {
                            return Some(pi);
                        }
                    }
                    None
                })
                .collect()
        };
        let cases: [&[&str]; 9] = [
            &["a", "a/b", "a/b/c"],
            &["a/b/c", "a/b", "a"],
            &["x/a", "x/b", "y"],
            &["a", "a-b", "a/c", "a-b/d", "a/c/e"],
            &["a//b", "a/", "a", "a/b"],
            &["", "/a", "/a/b", "a"],
            &["a", "a/b", "a", "a/b/c", "a/b"],
            &["a/b/c/d", "a"],
            &[],
        ];
        for joints in cases {
            let joints: Vec<String> = joints.iter().map(|s| s.to_string()).collect();
            assert_eq!(joint_parents(&joints), scan(&joints), "{joints:?}");
        }
        let chain: Vec<String> = ["a/b/c", "a/b", "a"].map(String::from).to_vec();
        assert_eq!(joint_parents(&chain), [Some(1), Some(2), None]);
    }

    #[test]
    fn usd_texture_transforms_match_the_scene_convention_numerically() {
        // Sampling through USD's transform on st and through the converted
        // scene transform on uv must land on the same texel, for rotations,
        // non-uniform scales and offsets alike.
        let cases = [(0.0, [1.0, 1.0], [0.0, 0.0]), (30.0, [2.0, 0.5], [0.25, -0.1]), (-110.0, [1.0, -1.0], [0.0, 1.0])];
        for (deg, scale, tr) in cases {
            let k = uv_transform_from_usd(deg, scale, tr);
            for uv in [[0.1f32, 0.2], [0.9, 0.35], [-0.4, 1.7]] {
                // USD: st = F(uv), st' = Rccw(φ)·S·st + t, texel uv' = F(st').
                let (s, t) = (uv[0] as f64, 1.0 - uv[1] as f64);
                let (sx, sy) = (s * scale[0], t * scale[1]);
                let phi = deg.to_radians();
                let (sn, cs) = phi.sin_cos();
                let st2 = [cs * sx - sn * sy + tr[0], sn * sx + cs * sy + tr[1]];
                let want = [st2[0], 1.0 - st2[1]];
                let got = k.apply(uv);
                assert!((got[0] as f64 - want[0]).abs() < 1e-5 && (got[1] as f64 - want[1]).abs() < 1e-5, "{deg} {uv:?}: {got:?} vs {want:?}");
            }
            let (d2, s2, t2) = uv_transform_to_usd(&k);
            assert!((d2 - deg).abs() < 1e-4 && (s2[0] - scale[0]).abs() < 1e-6 && (t2[0] - tr[0]).abs() < 1e-5 && (t2[1] - tr[1]).abs() < 1e-5);
        }
    }

    #[test]
    fn op_stacks_compose_outermost_first() {
        let attr = |v: Value| Attr { default: Some(v), ..Default::default() };
        let ops = vec![
            ("xformOp:translate".to_string(), false, attr(Value::Tuple(vec![1.0, 0.0, 0.0]))),
            ("xformOp:rotateXYZ".to_string(), false, attr(Value::Tuple(vec![0.0, 0.0, 90.0]))),
            ("xformOp:scale".to_string(), false, attr(Value::Tuple(vec![2.0, 2.0, 2.0]))),
        ];
        let m = compose_ops(&ops, None).unwrap();
        // (1,0,0) scaled to (2,0,0), rotated 90° about Z to (0,2,0), moved to (1,2,0).
        let p = math::transform_point(&m, [1.0, 0.0, 0.0]);
        assert!((p[0] - 1.0).abs() < 1e-6 && (p[1] - 2.0).abs() < 1e-6, "{p:?}");
        // !invert! undoes a pivot.
        let ops = vec![
            ("xformOp:translate:pivot".to_string(), false, attr(Value::Tuple(vec![0.0, 5.0, 0.0]))),
            ("xformOp:rotateX".to_string(), false, attr(Value::Float(180.0))),
            ("xformOp:translate:pivot".to_string(), true, attr(Value::Tuple(vec![0.0, 5.0, 0.0]))),
        ];
        let m = compose_ops(&ops, None).unwrap();
        let p = math::transform_point(&m, [0.0, 5.0, 0.0]);
        assert!((p[1] - 5.0).abs() < 1e-6, "the pivot stays put: {p:?}");
    }

    /// 2026-10-12 review, finding 11: densifying capped samples per key
    /// window (10,000) but not in total, and the time-code rate comes from
    /// the file — 3,000 windows at timeCodesPerSecond 1e-9 asked for 30M
    /// samples before the key total was ever checked.
    #[test]
    fn densified_samples_count_against_the_key_budget() {
        let times: Vec<f64> = (0..3000).map(|i| f64::from(i) * 1000.0).collect();
        assert!(densify(&times, 1e-9, super::super::scene::MAX_ANIMATION_KEYS).is_err());
        let fine = densify(&[0.0, 30.0], 30.0, super::super::scene::MAX_ANIMATION_KEYS).unwrap();
        assert_eq!(fine.len(), 1 + RESAMPLE_FPS as usize, "one second, densified at RESAMPLE_FPS: {fine:?}");
    }

    // ── uv sets, numbered by name ───────────────────────────────────────────

    /// A hand-written layer: `body` inside the default prim `/R`, with two
    /// images beside it, `base.png` (the quadrants) and `occ.png` (the bands).
    fn read_layer(body: &str) -> super::super::SceneRead {
        let text = format!("#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n)\ndef Xform \"R\"\n{{\n{body}\n}}\n");
        let files = vec![
            PackageFile { path: "base.png".into(), bytes: fixtures::quadrant_png() },
            PackageFile { path: "occ.png".into(), bytes: fixtures::bands_png() },
        ];
        super::super::usd::read_usda(text.as_bytes(), &MemorySidecars { files: &files }).unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    /// A quad mesh `name` bound to `/R/<material>`, with one vertex-
    /// interpolated `texCoord2f[]` primvar per `(primvar, tag)`. Point `i`
    /// holds `(tag, i)`, so the set it reads back as, [`tagged`]`(tag)`, names
    /// its source.
    fn quad(name: &str, material: &str, sets: &[(&str, f32)]) -> String {
        let mut out = format!(
            "def Mesh \"{name}\" (prepend apiSchemas = [\"MaterialBindingAPI\"]) {{\n\
             int[] faceVertexCounts = [4]\n\
             int[] faceVertexIndices = [0, 1, 2, 3]\n\
             point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)]\n\
             rel material:binding = </R/{material}>\n"
        );
        for (n, tag) in sets {
            out.push_str(&format!("texCoord2f[] primvars:{n} = [({tag}, 0), ({tag}, 1), ({tag}, 2), ({tag}, 3)] (interpolation = \"vertex\")\n"));
        }
        out.push_str("}\n");
        out
    }

    /// The uv set a [`quad`] primvar tagged `tag` reads as, V flipped.
    fn tagged(tag: f32) -> Vec<[f32; 2]> {
        (0..4).map(|i| [tag, 1.0 - i as f32]).collect()
    }

    /// A material `/R/<name>` whose base colour samples primvar `base` from
    /// `base.png` and, when given, whose occlusion samples `occlusion` from
    /// `occ.png`.
    fn material(name: &str, base: &str, occlusion: Option<&str>) -> String {
        let reader = |shader: &str, var: &str| {
            format!(
                "def Shader \"{shader}\" {{ uniform token info:id = \"UsdPrimvarReader_float2\"\n\
                 string inputs:varname = \"{var}\"\n\
                 float2 outputs:result }}\n"
            )
        };
        let texture = |shader: &str, file: &str, reader: &str, output: &str| {
            format!(
                "def Shader \"{shader}\" {{ uniform token info:id = \"UsdUVTexture\"\n\
                 asset inputs:file = @{file}@\n\
                 float2 inputs:st.connect = </R/{name}/{reader}.outputs:result>\n\
                 {output} }}\n"
            )
        };
        let mut out = format!(
            "def Material \"{name}\" {{\n\
             token outputs:surface.connect = </R/{name}/PS.outputs:surface>\n\
             def Shader \"PS\" {{ uniform token info:id = \"UsdPreviewSurface\"\n\
             color3f inputs:diffuseColor.connect = </R/{name}/Base.outputs:rgb>\n"
        );
        if occlusion.is_some() {
            out.push_str(&format!("float inputs:occlusion.connect = </R/{name}/Occ.outputs:r>\n"));
        }
        out.push_str("token outputs:surface }\n");
        out.push_str(&reader("BaseSt", base));
        out.push_str(&texture("Base", "base.png", "BaseSt", "float3 outputs:rgb"));
        if let Some(var) = occlusion {
            out.push_str(&reader("OccSt", var));
            out.push_str(&texture("Occ", "occ.png", "OccSt", "float outputs:r"));
        }
        out.push_str("}\n");
        out
    }

    fn uv_set_of(r: &Option<TextureRef>) -> Option<u32> {
        r.as_ref().map(|t| t.uv_set)
    }

    /// Spec decision 1: `st`, `st1`, `st2` and `st3`, read by a material
    /// through `st3` (base colour) and `st1` (occlusion), stay four sets in
    /// suffix order, and each binding names its own. The reads used to be the
    /// sets, in read order: two, `st3` as set 0.
    #[test]
    fn usd_sets_are_numbered_by_name() {
        let r = read_layer(&(quad("M", "Mat", &[("st", 0.0), ("st1", 1.0), ("st2", 2.0), ("st3", 3.0)]) + &material("Mat", "st3", Some("st1"))));
        let s = &r.scene;
        let p = &s.meshes[0].primitives[0];
        assert_eq!(p.uvs, vec![tagged(0.0), tagged(1.0), tagged(2.0), tagged(3.0)], "{:?}", r.warnings);
        let m = &s.materials[p.material.unwrap()];
        assert_eq!((uv_set_of(&m.base_color_texture), uv_set_of(&m.occlusion_texture)), (Some(3), Some(1)));
        let image = |t: &Option<TextureRef>| t.as_ref().map(|t| s.images[s.textures[t.texture].image].data.clone());
        assert_eq!(image(&m.base_color_texture), Some(fixtures::quadrant_png()));
        assert_eq!(image(&m.occlusion_texture), Some(fixtures::bands_png()));
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    /// `st` and `st2` with no `st1`: sets 0 and 1, and the texture on `st2` is
    /// on set 1.
    #[test]
    fn a_gap_in_the_suffixes_closes() {
        let r = read_layer(&(quad("M", "Mat", &[("st", 0.0), ("st2", 2.0)]) + &material("Mat", "st2", None)));
        let p = &r.scene.meshes[0].primitives[0];
        assert_eq!(p.uvs, vec![tagged(0.0), tagged(2.0)], "{:?}", r.warnings);
        assert_eq!(uv_set_of(&r.scene.materials[p.material.unwrap()].base_color_texture), Some(1));
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    /// Blender's `primvars:UVMap`, the mesh's only set, read by its material:
    /// one set with the texture on it, as before, typed `texCoord2f[]` (which
    /// makes it the first texture-coordinate primvar too) or plain `float2[]`.
    /// Read by nothing (the mesh unbound), a `texCoord2f[]` one is still the
    /// mesh's set 0.
    #[test]
    fn uvmap_still_reads() {
        let mesh = quad("M", "Mat", &[("UVMap", 7.0)]);
        for mesh in [mesh.clone(), mesh.replace("texCoord2f[] primvars:UVMap", "float2[] primvars:UVMap")] {
            let r = read_layer(&(mesh + &material("Mat", "UVMap", None)));
            let p = &r.scene.meshes[0].primitives[0];
            assert_eq!(p.uvs, vec![tagged(7.0)], "{:?}", r.warnings);
            assert_eq!(uv_set_of(&r.scene.materials[p.material.unwrap()].base_color_texture), Some(0));
            assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        }
        let r = read_layer(&mesh.replace("rel material:binding = </R/Mat>\n", ""));
        assert_eq!(r.scene.meshes[0].primitives[0].uvs, vec![tagged(7.0)], "{:?}", r.warnings);
    }

    /// `st` and `st1` with the texture on `st1`: two sets, the texture on set
    /// 1. It used to be one set, `st1` as set 0.
    #[test]
    fn a_material_reading_only_st1_keeps_both() {
        let r = read_layer(&(quad("M", "Mat", &[("st", 0.0), ("st1", 1.0)]) + &material("Mat", "st1", None)));
        let p = &r.scene.meshes[0].primitives[0];
        assert_eq!(p.uvs, vec![tagged(0.0), tagged(1.0)], "{:?}", r.warnings);
        assert_eq!(uv_set_of(&r.scene.materials[p.material.unwrap()].base_color_texture), Some(1));
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    /// Indexed primvars keep their values and indices paired after the
    /// renumbering. Two quads share an edge: `st` is per point and indexed,
    /// `st1` face-varying, and `st2`, which the material reads, face-varying
    /// and indexed in another order. Every IR vertex is one corner's
    /// combination of position and the three sets, and every corner's is one
    /// IR vertex.
    #[test]
    fn indexed_extra_primvars_stay_paired() {
        let points = [[0.0f32, 0.0], [1.0, 0.0], [2.0, 0.0], [0.0, 1.0], [1.0, 1.0], [2.0, 1.0]];
        let corners = [0usize, 1, 4, 3, 1, 2, 5, 4];
        let st = [[0.0f32, 0.5], [0.0, 0.25]];
        let st_indices = [0usize, 1, 1, 0, 0, 1];
        let st1: Vec<[f32; 2]> = (0..corners.len()).map(|k| [1.0, k as f32]).collect();
        let st2 = [[2.0f32, 0.125], [2.0, 0.375], [2.0, 0.625]];
        let st2_indices = [2usize, 0, 1, 1, 0, 2, 2, 1];
        let pairs = |v: &[[f32; 2]]| v.iter().map(|t| format!("({}, {})", t[0], t[1])).collect::<Vec<_>>().join(", ");
        let ints = |v: &[usize]| v.iter().map(usize::to_string).collect::<Vec<_>>().join(", ");
        let mesh = format!(
            "def Mesh \"M\" (prepend apiSchemas = [\"MaterialBindingAPI\"]) {{\n\
             int[] faceVertexCounts = [4, 4]\n\
             int[] faceVertexIndices = [{}]\n\
             point3f[] points = [{}]\n\
             texCoord2f[] primvars:st = [{}] (interpolation = \"vertex\")\n\
             int[] primvars:st:indices = [{}]\n\
             texCoord2f[] primvars:st1 = [{}] (interpolation = \"faceVarying\")\n\
             texCoord2f[] primvars:st2 = [{}] (interpolation = \"faceVarying\")\n\
             int[] primvars:st2:indices = [{}]\n\
             rel material:binding = </R/Mat>\n\
             }}\n",
            ints(&corners),
            points.iter().map(|q| format!("({}, {}, 0)", q[0], q[1])).collect::<Vec<_>>().join(", "),
            pairs(&st),
            ints(&st_indices),
            pairs(&st1),
            pairs(&st2),
            ints(&st2_indices),
        );
        let r = read_layer(&(mesh + &material("Mat", "st2", None)));
        let p = &r.scene.meshes[0].primitives[0];
        assert_eq!(p.uvs.len(), 3, "st, st1 and st2: {:?}", r.warnings);
        assert_eq!(uv_set_of(&r.scene.materials[p.material.unwrap()].base_color_texture), Some(2));
        let flip = |t: [f32; 2]| [t[0], 1.0 - t[1]];
        let key = |pos: [f32; 3], uvs: [[f32; 2]; 3]| -> Vec<u32> { pos.iter().chain(uvs.iter().flatten()).map(|x| x.to_bits()).collect() };
        let mut want: Vec<Vec<u32>> = (0..corners.len())
            .map(|k| {
                let pt = corners[k];
                key([points[pt][0], points[pt][1], 0.0], [flip(st[st_indices[pt]]), flip(st1[k]), flip(st2[st2_indices[k]])])
            })
            .collect();
        let mut got: Vec<Vec<u32>> = (0..p.positions.len()).map(|v| key(p.positions[v], [p.uvs[0][v], p.uvs[1][v], p.uvs[2][v]])).collect();
        want.sort();
        got.sort();
        assert_eq!(got, want, "each set's values follow its own indices");
        assert_eq!(p.triangle_count(), 4);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    /// Ruling T1a: one material read through `st1`, bound to meshes that put
    /// `st1` in different places (A and C: `st`, `st1`; B: `st1` alone), is one
    /// IR material per layout, and each mesh's texture samples its own `st1`.
    #[test]
    fn a_material_on_two_layouts_gets_one_copy_per_layout() {
        let r = read_layer(
            &[
                quad("A", "Mat", &[("st", 0.0), ("st1", 1.0)]),
                quad("B", "Mat", &[("st1", 5.0)]),
                quad("C", "Mat", &[("st", 0.0), ("st1", 1.0)]),
                material("Mat", "st1", None),
            ]
            .concat(),
        );
        let s = &r.scene;
        let prim = |name: &str| {
            let n = s.nodes.iter().find(|n| n.name.as_deref() == Some(name)).unwrap_or_else(|| panic!("no node {name}"));
            &s.meshes[n.mesh.unwrap()].primitives[0]
        };
        let (a, b, c) = (prim("A"), prim("B"), prim("C"));
        assert_eq!(s.materials.len(), 2, "one per layout: {:?}", s.materials);
        assert_eq!(a.material, c.material, "one layout, one material");
        assert_ne!(a.material, b.material);
        for (p, sets, st1) in [(a, 2, 1.0), (b, 1, 5.0), (c, 2, 1.0)] {
            assert_eq!(p.uvs.len(), sets);
            let m = &s.materials[p.material.unwrap()];
            assert_eq!(m.name.as_deref(), Some("Mat"));
            let t = m.base_color_texture.as_ref().expect("the texture");
            assert_eq!(p.uvs[t.uv_set as usize], tagged(st1), "the texture samples the mesh's own st1");
        }
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    /// Ruling T1b: a set the material reads that the mesh lacks takes a slot
    /// after the mesh's own, read as (0, 0) with a warning; a read that would
    /// be a ninth set is dropped, with its textures and a warning. No binding
    /// points past the sets.
    #[test]
    fn missing_and_ninth_reads_never_point_past_the_sets() {
        let r = read_layer(&(quad("M", "Mat", &[("st", 0.0)]) + &material("Mat", "st5", None)));
        let p = &r.scene.meshes[0].primitives[0];
        assert_eq!(p.uvs, vec![tagged(0.0), vec![[0.0, 0.0]; 4]], "{:?}", r.warnings);
        assert_eq!(uv_set_of(&r.scene.materials[p.material.unwrap()].base_color_texture), Some(1));
        assert!(r.warnings.iter().any(|w| w.contains("no texture coordinates \"st5\"")), "{:?}", r.warnings);

        let names: Vec<String> = (0..8).map(|k| if k == 0 { "st".to_string() } else { format!("st{k}") }).collect();
        let mut sets: Vec<(&str, f32)> = names.iter().enumerate().map(|(k, n)| (n.as_str(), k as f32)).collect();
        sets.push(("UVMap", 9.0));
        let r = read_layer(&(quad("M", "Mat", &sets) + &material("Mat", "st2", Some("UVMap"))));
        let s = &r.scene;
        let p = &s.meshes[0].primitives[0];
        assert_eq!(p.uvs, (0..8).map(|k| tagged(k as f32)).collect::<Vec<_>>(), "{:?}", r.warnings);
        let m = &s.materials[p.material.unwrap()];
        assert_eq!(uv_set_of(&m.base_color_texture), Some(2));
        assert!(m.occlusion_texture.is_none(), "a binding past the sets is dropped");
        assert!(r.warnings.iter().any(|w| w.contains("\"UVMap\"") && w.contains("dropped")), "{:?}", r.warnings);
        assert_eq!(s.images.len(), 1, "the dropped binding's image is not carried");
    }

    // ── material layers ─────────────────────────────────────────────────────

    /// A quad on `st` bound to `/R/Mat`, whose preview surface has the
    /// `inputs` lines. They may connect to four UsdUVTextures on `st`:
    /// `Coat` (`occ.png`) scales R by 0.5 and G by 0.25, `Hot` (the same
    /// file) scales both by 2, `Biased` (the same file) also biases R by 0.5
    /// and G by 0.25, and `N` (`base.png`) is a normal map at glTF's scale
    /// 0.5.
    fn surface(inputs: &str) -> String {
        let texture = |name: &str, file: &str, scale: &str, bias: &str, outputs: &str| {
            format!(
                "def Shader \"{name}\" {{ uniform token info:id = \"UsdUVTexture\"\n\
                 asset inputs:file = @{file}@\n\
                 float2 inputs:st.connect = </R/Mat/St.outputs:result>\n\
                 token inputs:sourceColorSpace = \"raw\"\n\
                 float4 inputs:scale = {scale}\n\
                 {bias}\n\
                 {outputs} }}\n"
            )
        };
        [
            quad("M", "Mat", &[("st", 0.0)]),
            format!(
                "def Material \"Mat\" {{\n\
                 token outputs:surface.connect = </R/Mat/PS.outputs:surface>\n\
                 def Shader \"PS\" {{ uniform token info:id = \"UsdPreviewSurface\"\n\
                 {inputs}\n\
                 token outputs:surface }}\n\
                 def Shader \"St\" {{ uniform token info:id = \"UsdPrimvarReader_float2\"\n\
                 string inputs:varname = \"st\"\n\
                 float2 outputs:result }}\n"
            ),
            texture("Coat", "occ.png", "(0.5, 0.25, 1, 1)", "", "float outputs:r\nfloat outputs:g"),
            texture("Hot", "occ.png", "(2, 2, 1, 1)", "", "float outputs:r\nfloat outputs:g"),
            texture("Biased", "occ.png", "(0.5, 0.5, 1, 1)", "float4 inputs:bias = (0.5, 0.25, 0, 0)", "float outputs:r\nfloat outputs:g"),
            texture("N", "base.png", "(1, 1, 2, 1)", "float4 inputs:bias = (-0.5, -0.5, -1, 0)", "float3 outputs:rgb"),
            "}\n".to_string(),
        ]
        .concat()
    }

    /// The material [`surface`]'s quad draws with.
    fn surface_material(r: &super::super::SceneRead) -> &Material {
        &r.scene.materials[r.scene.meshes[0].primitives[0].material.expect("bound")]
    }

    /// UsdPreviewSurface's clearcoat as glTF holds it: its factor and its
    /// roughness are values, or a UsdUVTexture's R and G times its scale. An
    /// unauthored roughness is USD's own 0.01, not glTF's 0, and the coat
    /// takes the base normal map, which UsdPreviewSurface's clearcoat uses.
    /// The layer is there when `clearcoat` is, except an unconnected 0, which
    /// draws no coat and reads as none, silently, its roughness unread
    /// (Ruling A41): three.js's exporter writes `clearcoat = 0` on every
    /// physical material.
    #[test]
    fn clearcoat_reads_as_usd_defines_it() {
        use super::super::scene::Clearcoat;
        let map = Some(TextureRef { texture: 0, uv_set: 0, transform: None });
        let coat = |factor, roughness| Some(Clearcoat { factor, roughness, ..Default::default() });
        let cases = [
            ("float inputs:clearcoat = 0.75", coat(0.75, 0.01)),
            ("float inputs:clearcoat = 0", None),
            ("float inputs:clearcoat = 0\nfloat inputs:clearcoatRoughness = 0", None),
            ("float inputs:clearcoat = 0\nfloat inputs:clearcoatRoughness.connect = </R/Mat/Coat.outputs:r>", None),
            ("float inputs:clearcoat = 1\nfloat inputs:clearcoatRoughness = 0", coat(1.0, 0.0)),
            (
                "float inputs:clearcoat.connect = </R/Mat/Coat.outputs:r>\nfloat inputs:clearcoatRoughness.connect = </R/Mat/Coat.outputs:g>",
                Some(Clearcoat { factor: 0.5, texture: map.clone(), roughness: 0.25, roughness_texture: map.clone(), ..Default::default() }),
            ),
            (
                "normal3f inputs:normal.connect = </R/Mat/N.outputs:rgb>\nfloat inputs:clearcoat = 1\nfloat inputs:clearcoatRoughness = 0.2",
                Some(Clearcoat { factor: 1.0, roughness: 0.2, normal_texture: map.clone(), normal_scale: 0.5, ..Default::default() }),
            ),
            ("float inputs:clearcoatRoughness = 0.5", None),
        ];
        for (inputs, want) in cases {
            let r = read_layer(&surface(inputs));
            assert!(r.warnings.is_empty(), "{inputs}: {:?}", r.warnings);
            assert_eq!(surface_material(&r).clearcoat, want, "{inputs}");
        }
    }

    /// A clearcoat glTF cannot hold is left out with a warning, and the file
    /// reads: a factor outside 0 to 1, or not finite, leaves no layer; such a
    /// roughness leaves USD's 0.01. So does one read from another channel
    /// than glTF's (R for the factor, G for the roughness), or from a map
    /// that biases it (glTF's factor only scales). Each is checked before its
    /// map is read, so no texture is left over.
    #[test]
    fn a_clearcoat_gltf_cannot_hold_is_left_out_with_a_warning() {
        use super::super::scene::Clearcoat;
        let unmapped = Some(Clearcoat { factor: 1.0, roughness: 0.01, ..Default::default() });
        let cases = [
            ("float inputs:clearcoat = 1.5", None, "a clearcoat of 1.5"),
            ("float inputs:clearcoat = nan", None, "a clearcoat of NaN"),
            ("float inputs:clearcoat.connect = </R/Mat/Hot.outputs:r>", None, "a clearcoat of 2"),
            ("float inputs:clearcoat.connect = </R/Mat/Coat.outputs:g>", None, "clearcoat from channel \"g\""),
            ("float inputs:clearcoat.connect = </R/Mat/St.outputs:result>", None, "clearcoat comes from a shader"),
            ("float inputs:clearcoat = 1\nfloat inputs:clearcoatRoughness = 2", unmapped.clone(), "a clearcoatRoughness of 2"),
            ("float inputs:clearcoat = 1\nfloat inputs:clearcoatRoughness = -inf", unmapped.clone(), "a clearcoatRoughness of -inf"),
            (
                "float inputs:clearcoat = 1\nfloat inputs:clearcoatRoughness.connect = </R/Mat/Coat.outputs:r>",
                unmapped.clone(),
                "clearcoatRoughness from channel \"r\"",
            ),
            ("float inputs:clearcoat.connect = </R/Mat/Biased.outputs:r>", None, "biases its clearcoat map"),
            (
                "float inputs:clearcoat = 1\nfloat inputs:clearcoatRoughness.connect = </R/Mat/Biased.outputs:g>",
                unmapped.clone(),
                "biases its clearcoatRoughness map",
            ),
        ];
        for (inputs, want, warning) in cases {
            let r = read_layer(&surface(inputs));
            assert_eq!(surface_material(&r).clearcoat, want, "{inputs}");
            assert!(r.warnings.iter().any(|w| w.contains(warning)), "{inputs}: no warning has {warning:?}: {:?}", r.warnings);
            assert!(r.scene.images.is_empty() && r.scene.textures.is_empty(), "{inputs}: a map was read");
        }
    }

    /// `ior` as KHR_materials_ior holds it: 0 (its special value) or 1 and
    /// above. Any other number, a non-finite one included, is left out with
    /// a warning, and so is an ior a shader computes (glTF's is a number):
    /// the file reads, where validation would refuse the scene. An ior of
    /// exactly 1.5, glTF's and UsdPreviewSurface's default, which three.js's
    /// exporter writes on every physical material, reads as unset, silently
    /// (Ruling A41).
    #[test]
    fn an_ior_gltf_cannot_hold_is_left_out_with_a_warning() {
        let cases = [
            ("float inputs:ior = 1.45", Some(1.45), None),
            ("float inputs:ior = 1", Some(1.0), None),
            ("float inputs:ior = 0", Some(0.0), None),
            ("float inputs:ior = 1.5", None, None),
            ("", None, None),
            ("float inputs:ior = 0.5", None, Some("an ior of 0.5")),
            ("float inputs:ior = -1", None, Some("an ior of -1")),
            ("float inputs:ior = nan", None, Some("an ior of NaN")),
            ("float inputs:ior = inf", None, Some("an ior of inf")),
            ("float inputs:ior.connect = </R/Mat/Coat.outputs:r>", None, Some("ior comes from a shader")),
        ];
        for (inputs, want, warning) in cases {
            let r = read_layer(&surface(inputs));
            assert_eq!(surface_material(&r).ior, want, "{inputs}");
            match warning {
                Some(w) => assert!(r.warnings.iter().any(|x| x.contains(w)), "{inputs}: no warning has {w:?}: {:?}", r.warnings),
                None => assert!(r.warnings.is_empty(), "{inputs}: {:?}", r.warnings),
            }
            assert!(r.scene.images.is_empty(), "{inputs}: a map was read");
        }
    }

    /// A clearcoat over an authored ior other than 1.5 reads, both of them,
    /// with a warning: UsdPreviewSurface's ior is "to be used for translucent
    /// objects and objects with specular components, including the clearcoat
    /// if clearcoat > 0", and glTF's clearcoat keeps 1.5. There is none for
    /// an ior of 1.5, which reads as unset, an unauthored one, or a
    /// clearcoat of 0, which reads as no layer (Ruling A41).
    #[test]
    fn a_clearcoat_over_an_ior_other_than_1_5_is_warned_about() {
        let cases = [
            ("float inputs:clearcoat = 1\nfloat inputs:ior = 1.45", true, Some(1.45), Some("a clearcoat and an ior of 1.45:")),
            ("float inputs:clearcoat = 1\nfloat inputs:ior = 0", true, Some(0.0), Some("a clearcoat and an ior of 0:")),
            ("float inputs:clearcoat.connect = </R/Mat/Coat.outputs:r>\nfloat inputs:ior = 2", true, Some(2.0), Some("a clearcoat and an ior of 2:")),
            ("float inputs:clearcoat = 1\nfloat inputs:ior = 1.5", true, None, None),
            ("float inputs:clearcoat = 1", true, None, None),
            ("float inputs:clearcoat = 0\nfloat inputs:ior = 1.45", false, Some(1.45), None),
        ];
        for (inputs, coated, ior, warning) in cases {
            let r = read_layer(&surface(inputs));
            let m = surface_material(&r);
            assert_eq!((m.clearcoat.is_some(), m.ior), (coated, ior), "{inputs}");
            match warning {
                Some(w) => assert!(r.warnings.len() == 1 && r.warnings[0].contains(w), "{inputs}: not one warning with {w:?}: {:?}", r.warnings),
                None => assert!(r.warnings.is_empty(), "{inputs}: {:?}", r.warnings),
            }
        }
    }

    /// USD's floats are f32, and a crate stores them so: a layer warning
    /// prints a value as the f32 it is, not widened to an f64's digits
    /// (1.1 as an f32 is 1.100000023841858 as an f64).
    #[test]
    fn layer_warnings_print_values_as_f32() {
        for (inputs, shown) in [
            ("float inputs:clearcoat = 1.100000023841858", "a clearcoat of 1.1,"),
            ("float inputs:clearcoat = 1\nfloat inputs:clearcoatRoughness = 1.100000023841858", "a clearcoatRoughness of 1.1,"),
            ("float inputs:ior = 0.699999988079071", "an ior of 0.7,"),
        ] {
            let r = read_layer(&surface(inputs));
            assert!(r.warnings.iter().any(|w| w.contains(shown)), "{inputs}: no warning has {shown:?}: {:?}", r.warnings);
        }
    }

    /// A warning quotes what the file names, and a connection target has no
    /// length bound, so a hostile file can name a shader's output at length.
    /// Every message is clipped as it is pushed (Ruling A45): a 100 KB channel
    /// name comes out cut to at most 1,000 characters, ending in "…", while
    /// an ordinary warning stays whole.
    #[test]
    fn a_hostile_name_in_a_warning_is_clipped() {
        let long = "g".repeat(100_000);
        let r = read_layer(&surface(&format!("float inputs:clearcoat.connect = </R/Mat/Coat.outputs:{long}>")));
        let w = r.warnings.iter().find(|w| w.contains("reads clearcoat from channel")).expect("the channel warning");
        let n = w.chars().count();
        assert!(n <= 1_001 && w.ends_with('…'), "{n} characters: {}", w.chars().take(160).collect::<String>());
        let r = read_layer(&surface("float inputs:clearcoat.connect = </R/Mat/Coat.outputs:g>"));
        assert_eq!(r.warnings, ["material /R/Mat reads clearcoat from channel \"g\"; glTF reads it from R, so it is not carried"]);
    }
}
