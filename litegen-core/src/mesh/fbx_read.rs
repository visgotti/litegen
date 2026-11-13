//! FBX document → scene.
//!
//! # The object graph
//!
//! An FBX document is a flat list of objects (`Objects`) and a list of edges
//! between them (`Connections`): `OO` links an object to an object, `OP` links
//! it to a named property of one. Everything structural is an edge — a model's
//! parent, its geometry, its materials (in connection order: a polygon's
//! material index is a position in that list), a texture's slot on a material,
//! a deformer's geometry, a cluster's bone, a curve's property. So the reader
//! indexes both lists once and asks the graph questions; nothing is inferred
//! from file order except where the format itself uses it.
//!
//! # What is carried, what is warned about, what is refused
//!
//! Carried: the model hierarchy with the full FBX transform (pivots, offsets,
//! pre/post rotation, rotation order, inherit type), geometric transforms
//! (baked into the geometry that has them, never into children), polygon
//! meshes with normals, tangents, every UV set, vertex colours and per-polygon
//! materials, line geometry (below), Phong/Lambert materials (plus Stingray,
//! Arnold/Standard Surface, 3ds Max Physical, OpenPBR and 3ds Max PBR
//! properties when
//! present, with their layers), embedded or sidecar textures, skin clusters,
//! blend shapes (an in-between shape becomes a target of its own, weighted
//! piecewise-linearly from its channel's weight), the active animation
//! stack's transform and blend-shape curves, and the lights and cameras the
//! models place (below). Warned about and left out: camera switchers and
//! stereo camera rigs (as empty nodes), area and volume lights, a non-black
//! ambient light (`GlobalSettings` `AmbientColor`, Ruling R15), NURBS,
//! patches and curves, other animation stacks and layers, animated properties
//! the scene does not model (a light's or camera's own curves among them),
//! texture slots with no metallic-roughness equivalent. Refused:
//! indices out of range, arrays that disagree with their mapping, a cyclic
//! hierarchy — anything that would otherwise become a plausible but wrong
//! model.
//!
//! # Line geometry
//!
//! A `Geometry` of subclass `Line` reads as a lines primitive: `Points` (three
//! doubles each) and `PointsIndex`, whose entries run through the points in
//! strips, a negative entry `-(e + 1)` naming point `e` and ending its strip
//! as `PolygonVertexIndex` ends a polygon. A strip of c points is c - 1
//! segments and none joins two strips; a strip of one point is nothing. A
//! trailing strip with no end stops at the array's end, with no closing
//! segment (Ruling I10-R10: assimp wraps it back to the first index, the FBX
//! SDK always ends a strip). Points no segment names are not carried. The
//! model's geometric transform and first material apply, as to a mesh; the
//! geometry's layer elements are not read. A model's several geometries keep
//! the first, a `Line` counting as one. Nothing is read as a point
//! primitive: FBX has no point-cloud class, and the writer holds neither
//! points nor lines (`fbx::capabilities`), so `fbx::read`, the flat
//! triangle-only path, finds a file of lines empty. Sources: assimp v6.0.5
//! `code/AssetLib/FBX/FBXMeshGeometry.cpp` L716-728 and `FBXConverter.cpp`
//! L1030-1068, and the FBX SDK's `FbxLine`
//! (https://help.autodesk.com/cloudhelp/2020/ENU/FBX-API-Reference/cpp_ref/class_fbx_line.html).
//!
//! # Material layers
//!
//! Arnold's aiStandardSurface and Maya's Standard Surface, 3ds Max's
//! Physical Material and 3ds Max's OpenPBR material are read for glTF's
//! clearcoat, transmission, ior, specular, sheen and iridescence layers.
//! `LayerNames` lists each family's properties, which are those of real
//! exports in ufbx v0.23.1's test data, and the sources of their meanings
//! and units. A layer is read when its weight is above 0 (a Standard Surface
//! thin film, which has none, when its thickness is), so a material at its
//! family's defaults reads none. Specular is the exception: it is read when
//! its weight is other than 1, glTF's default, 0 included (Ruling A24f). An
//! ior at glTF's 1.5 or at the family's documented default reads as unset,
//! as exporter defaults do in the OBJ and Collada readers, so a default
//! material gains no `KHR_materials_ior`: Standard Surface's 1.52
//! (`reference/standard_surface.osl` L162 at Autodesk/standard-surface
//! v1.0.1.1, and Maya's own default in ufbx's
//! `data/maya_arnold_textures_7500_ascii.fbx`), 3ds Max Physical's 1.52 (the
//! MAXScript reference's Physical_Material `trans_ior`) and OpenPBR's 1.5
//! (OpenPBR v1.1.1, `specular_ior`). A value outside its glTF field's range
//! is left out with a warning, never clamped. Warned about and left out,
//! since glTF has no slot of the same meaning:
//! - anisotropy (each family's anisotropic roughness differs from glTF's);
//! - a solid surface's refraction, absorption and dispersion (glTF's volume
//!   needs a thickness, which none of them stores; a thin-walled or opaque
//!   surface renders no dispersion, so it says nothing of it);
//! - a transmission roughness of its own, Standard Surface's
//!   `transmissionExtraRoughness` other than 0 and 3ds Max Physical's
//!   unlocked `trans_roughness` other than the roughness (glTF's
//!   transmission takes the base roughness);
//! - OpenPBR's fuzz (a different model from glTF's sheen);
//! - a coat's colour other than white, its anisotropy, and an ior other
//!   than 1.5 and the family's default (Physical 1.52, `coat_ior`; OpenPBR
//!   1.6);
//! - what a coat does to the layers under it, other than the family's
//!   default: Standard Surface's `coatAffectColor` and
//!   `coatAffectRoughness` (0), Physical's `coat_affect_color` and
//!   `coat_affect_roughness` (0.5) and OpenPBR's `coat_darkening` (1);
//! - the specular colours and OpenPBR's specular weight;
//! - a transmission tint other than the base colour glTF tints with;
//! - textures on layer properties.
//!
//! # Curves
//!
//! Keys are constant, linear or cubic. The SDK stores user tangents but
//! computes auto and TCB tangents on load, from flags whose meaning Autodesk
//! does not publish; this reader follows ufbx's reconstruction of them
//! (auto-tangent blending and bias, clamped and clamped-progressive,
//! one-sided slopes at the end keys, weighted handles as a time-domain
//! Bézier), which ufbx's test suite checks against curves evaluated by Maya
//! and MotionBuilder. Cubic segments are resampled into linear keys: at
//! least `RESAMPLE_HZ`, halving any piece whose chord strays from the curve
//! by more than `RESAMPLE_TOLERANCE` of the curve's range.
//!
//! A material's notes quote its name clipped to `MAX_QUOTED_NAME`
//! characters (Ruling A45).
//!
//! # Lights and cameras
//!
//! A model's `NodeAttribute` of class `Light` or `Camera` (the attribute's
//! class decides, as ufbx and Blender read it: ufbx.c L14996-L14999 at
//! v0.23.1, import_fbx.py L3340-L3364 at Blender 5.2.3) becomes a scene
//! light or camera, read once however many models place it, on the model's
//! node, whose frame loses FBX's turn (`fbx::Turn`, `fbx::reframe`). A model
//! that also holds a mesh or a skin joint, or holds a light and a camera,
//! keeps those (the camera rather than the light if it is a `Camera` model)
//! and gives the other a child node of its own, `{model}_light` or
//! `{model}_camera`, which carries the turn back. Only a model's first light
//! and first camera are read. Properties come from the attribute, then its
//! class's `PropertyTemplate` (`FbxLight`, `FbxCamera`), as Blender's
//! importer looks them up (import_fbx.py L3222-L3244); where neither says,
//! ufbx's defaults hold. Each rule is ufbx's reading (Blender 5.2's default
//! FBX importer is ufbx):
//!
//! - **Lights** (ufbx.c L23047-L23065): `LightType` 0 point, 1 directional,
//!   2 spot; `Color` in the caller's `FbxColorSpace`, as material colours
//!   (Ruling R14): decoded from sRGB by default, as three.js reads it
//!   (`FBXLoader.js` L1196 at r183), and past 1 unclamped
//!   ([`light_color`]), or taken as linear, as Blender (import_fbx.py
//!   L2250), ufbx and assimp read it; `Intensity` divided by 100 ("FBX
//!   seems to store intensities 100x"), as Blender's importer divides it
//!   too (L2251). Blender takes the quotient as its own radiometric watts,
//!   though, where its glTF importer converts candela and lux at 683 lm/W
//!   (`fbx_write`'s light notes cite it), so with `LightUnits::Photometric`,
//!   the default, a light Blender exported reads here about 54× (point,
//!   spot) or 683× (sun) dimmer than the same light through glTF: Ruling
//!   R28's export brought `staged`'s 3 lux sun back as 0.0044. With
//!   `ReadOptions::fbx_light_units` = `LightUnits::Blender`, `Intensity` /
//!   100 is read as that `energy`, and the light is the candela or lux whose
//!   glTF import gives it (`scene::from_blender_energy`), whatever the cone.
//!   No exposure is read: Blender's exporter bakes 2^exposure into
//!   `Intensity` and writes none, and its importer reads none. A candela or
//!   lux past f32 is dropped with the warning an `Intensity` past f32 gets
//!   under `Photometric`. A spot's `InnerAngle`, else `HotSpot`,
//!   and `OuterAngle`, else `ConeAngle`, else `Cone angle`, are full cones
//!   in degrees, so halved (Blender writes `spot_size`, the full cone,
//!   export_fbx_bin.py L624-L626). A cone past 180°, wider than a
//!   hemisphere, is read as a point light, warned (Ruling R32). A colour
//!   past 1 has its brightest component moved into the intensity (Ruling
//!   R8); a hard edge is softened (`Light::soften_hard_edge`, Ruling R5).
//!   The range is the SDK's far attenuation, `FarAttenuationEnd` while
//!   `EnableFarAttenuation` is on (the names assimp's FBX document lists with
//!   the SDK's defaults, FBXDocument.h L315-L317 at v6.0.5; neither ufbx nor
//!   Blender reads a range), in file units, and a directional light's is
//!   ignored with a warning, as glTF's reader ignores one. A stated
//!   `DecayType` other than 2, quadratic, is warned about: the scene's lights
//!   fall off with the inverse square. Area and volume lights (`LightType` 3
//!   and 4) are dropped with a warning.
//! - **Cameras** (ufbx.c L23087-L23253): `CameraProjectionType` 0 perspective,
//!   1 orthographic; the aspect ratio from `AspectWidth` and `AspectHeight`
//!   under `AspectRatioMode`, else the film's; the film gate (`FilmWidth`,
//!   `FilmHeight`, `FilmSqueezeRatio`, `ApertureFormat`) fitted by `GateFit`;
//!   yfov by `ApertureMode` ([`perspective_yfov`]). `NearPlane` and `FarPlane`
//!   are in file units. An orthographic view is 30 × `OrthoZoom` file units
//!   across its gate fit (ufbx.c L21353-L21357, L23109), except in a file
//!   Blender wrote, where `OrthoZoom` is Blender's `ortho_scale`, the extent
//!   in metres ([`blender_exporter`]).
//!
//! A light's direction and a camera's frame are its model's: ufbx measures a
//! light's from `node_to_world`, not from the geometric transform (ufbx.h
//! L1384-L1388). A light or camera that breaks a bound (`Light::check`,
//! `Camera::check`) is dropped with a warning naming it and the bound, and
//! its node stays: a reader never refuses a file over one.
//!
//! Ruling IDs ("Ruling A24b") are mesh-gaps item 4's rulings, listed with
//! their costs in the status block of
//! `docs/superpowers/plans/2026-11-04-mesh-material-layers.md`; R5, R8, R14
//! and R15 are the rulings of mesh-gaps item 5, lights and cameras, listed
//! with their costs in the "Rulings" section of
//! `docs/superpowers/plans/2026-11-04-mesh-lights-cameras.md`.

use super::fbx::{decode_color, malformed, srgb_decode, unsupported, TICKS_PER_SECOND, TICKS_PER_SECOND_2019};
use super::{FbxColorSpace, LightUnits};
use super::fbx_dom::{split_object_name, ArrayRef, Dom, Prop, RecId, ROOT};
use super::math::{self, EulerOrder, Mat4};
use super::package::{self, Sidecars};
use super::scene::{
    AlphaMode, AnimPath, Animation, Channel, Image, ImageFormat, Interpolation, Material, MorphTarget, Node,
    Primitive, Sampler, Scene, SceneMesh, Skin, SkinWeights, Texture, TextureRef, Transform, UvTransform, Wrap,
    MAX_ANIMATION_KEYS, MAX_JOINTS, MAX_MORPH_TARGETS, MAX_NODES, MAX_SCENE_INDICES, MAX_SCENE_VERTICES,
};
use super::scene::{Clearcoat, Iridescence, MaterialLayer, Sheen, Specular, Transmission};
use super::scene::{from_blender_energy, Camera, Light, LightKind, Projection};
use super::fbx::{reframe, Turn};
use super::{MeshError, SceneRead};
use std::collections::{BTreeMap, HashMap, HashSet};

/// UV sets read per geometry; the scene's own ceiling (see `glb.rs`).
const MAX_UV_SETS: usize = 8;
/// Influences kept per vertex. Vendors ship 8–10 (Meshy); 32 is past anything
/// a skinning shader evaluates, and glTF's reader stops there too.
const MAX_INFLUENCES: usize = 32;
use super::scene::MAX_MORPH_VALUES;
/// Objects in one document.
const MAX_OBJECTS: usize = 2_000_000;
/// Rate at which curves whose interpolation the scene cannot express exactly
/// (cubic segments) are resampled into linear keys, at the least.
const RESAMPLE_HZ: f64 = 30.0;
/// How far, as a fraction of a curve's key range, a resampled cubic may
/// stray from the chord between two keys at its midpoint before the chord
/// is halved; at most `REFINE_DEPTH` times.
const RESAMPLE_TOLERANCE: f64 = 1.0e-3;
const REFINE_DEPTH: u32 = 6;
/// Largest Euler change, in degrees, a linear key may span when more than one
/// axis moves at once: componentwise Euler interpolation and slerp agree to
/// second order over a step this size.
const MULTI_AXIS_STEP_DEG: f64 = 10.0;
/// Largest single-axis change a linear key may span: under 180 degrees slerp
/// follows the same arc as the Euler curve, and at 180 it would take the
/// shorter way round.
const SINGLE_AXIS_STEP_DEG: f64 = 90.0;
/// Largest Euler change a key may span when the node's translation depends
/// on its rotation (pivots, offsets, inherit types): the translation then
/// moves on an arc, which linear keys approximate by chords — 5 degrees keeps
/// the chord within 0.1% of the arm's length.
const PIVOT_STEP_DEG: f64 = 5.0;
/// How far before a jump a held value is re-keyed when a constant segment is
/// written into a linear channel.
const HOLD_EPSILON: f64 = 1.0e-3;
/// Subdivisions per segment, and keys per channel, before the input is
/// treated as hostile.
const MAX_SUBDIVISIONS: usize = 1000;
/// Matrix compositions spent re-evaluating ancestor chains for nodes whose
/// animation must go through world space (each sample walks the chain). A
/// deep rig keyed every frame needs a few million; a hostile chain of
/// thousands of nodes each keyed thousands of times would need billions.
const MAX_CHAIN_WORK: usize = 50_000_000;

fn finite3(v: [f64; 3]) -> bool {
    v.iter().all(|c| c.is_finite())
}

/// `Properties70` as a map, first entry winning: name → the values after
/// the four header strings (name, type, label, flags).
type PMap<'a> = HashMap<String, Vec<Prop<'a>>>;

/// An object's properties with its class template as fallback.
#[derive(Clone, Copy)]
struct Props<'m, 'a> {
    own: &'m PMap<'a>,
    tmpl: Option<&'m PMap<'a>>,
}

impl<'a> Props<'_, 'a> {
    fn get(&self, name: &str) -> Option<&[Prop<'a>]> {
        self.own.get(name).or_else(|| self.tmpl.and_then(|t| t.get(name))).map(|v| v.as_slice())
    }

    fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    fn f64(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(|v| v.first()).and_then(|p| p.as_f64())
    }

    fn i64(&self, name: &str) -> Option<i64> {
        self.get(name).and_then(|v| v.first()).and_then(|p| p.as_i64())
    }

    fn vec3(&self, name: &str) -> Option<[f64; 3]> {
        let v = self.get(name)?;
        Some([v.first()?.as_f64()?, v.get(1)?.as_f64()?, v.get(2)?.as_f64()?])
    }

    fn vec4(&self, name: &str) -> Option<[f64; 4]> {
        let v = self.get(name)?;
        Some([v.first()?.as_f64()?, v.get(1)?.as_f64()?, v.get(2)?.as_f64()?, v.get(3)?.as_f64()?])
    }

    fn string(&self, name: &str) -> Option<String> {
        self.get(name).and_then(|v| v.first()).and_then(|p| p.as_str()).map(|s| s.into_owned())
    }

    /// A finite vector, or `default` when absent; a non-finite one is the
    /// file's error.
    fn vec3_or(&self, name: &str, default: [f64; 3], what: &str) -> Result<[f64; 3], MeshError> {
        match self.vec3(name) {
            None => Ok(default),
            Some(v) if finite3(v) => Ok(v),
            Some(_) => Err(malformed(format!("{what}: {name} is not finite"))),
        }
    }
}

#[derive(Debug, Clone)]
struct Obj {
    rec: RecId,
    kind: String,
    name: String,
    sub: String,
}

/// One edge: the other object's id and the property (for `OP`).
type Edge = (i64, Option<String>);

/// A model's static transform, everything that feeds `L`.
#[derive(Debug, Clone)]
struct Xf {
    t: [f64; 3],
    r: [f64; 3],
    s: [f64; 3],
    roff: [f64; 3],
    rp: [f64; 3],
    soff: [f64; 3],
    sp: [f64; 3],
    pre: [f64; 3],
    post: [f64; 3],
    order: EulerOrder,
    inherit: i64,
    /// Geometric transform, applied to this model's own geometry only.
    geo: Mat4,
}

impl Xf {
    fn has_pivots(&self) -> bool {
        [self.roff, self.rp, self.soff, self.sp].iter().any(|v| *v != [0.0; 3])
    }

    /// `Rpre · R · Rpost⁻¹` as a quaternion: what the node's rotation is.
    fn rotation(&self, r: [f64; 3]) -> [f64; 4] {
        let pre = math::euler_to_quat(rad(self.pre), EulerOrder::Xyz);
        let rot = math::euler_to_quat(rad(r), self.order);
        let post = math::euler_to_quat(rad(self.post), EulerOrder::Xyz);
        let post_inv = [-post[0], -post[1], -post[2], post[3]];
        math::quat_normalize(math::quat_mul(pre, math::quat_mul(rot, post_inv)))
    }

    /// `T·Roff·Rp·Rpre·R·Rpost⁻¹·Rp⁻¹·Soff·Sp·S·Sp⁻¹` with the given T, R, S.
    fn local(&self, t: [f64; 3], r: [f64; 3], s: [f64; 3]) -> Mat4 {
        let neg = |v: [f64; 3]| [-v[0], -v[1], -v[2]];
        let rq = math::quat_to_mat(self.rotation(r));
        let mut m = math::translation(t);
        for x in [
            math::translation(self.roff),
            math::translation(self.rp),
            rq,
            math::translation(neg(self.rp)),
            math::translation(self.soff),
            math::translation(self.sp),
            math::scaling(s),
            math::translation(neg(self.sp)),
        ] {
            m = math::mat_mul(&m, &x);
        }
        m
    }
}

fn rad(v: [f64; 3]) -> [f64; 3] {
    [v[0].to_radians(), v[1].to_radians(), v[2].to_radians()]
}

fn euler_order(v: i64) -> Option<EulerOrder> {
    Some(match v {
        0 => EulerOrder::Xyz,
        1 => EulerOrder::Xzy,
        2 => EulerOrder::Yzx,
        3 => EulerOrder::Yxz,
        4 => EulerOrder::Zxy,
        5 => EulerOrder::Zyx,
        _ => return None,
    })
}

/// The upper 3x3 of `m` with its columns' lengths removed, and those lengths.
fn rotation_part(m: &Mat4) -> Mat4 {
    let mut out = math::IDENTITY;
    for c in 0..3 {
        let len = (m[c * 4] * m[c * 4] + m[c * 4 + 1] * m[c * 4 + 1] + m[c * 4 + 2] * m[c * 4 + 2]).sqrt();
        for r in 0..3 {
            out[c * 4 + r] = if len > 0.0 { m[c * 4 + r] / len } else { 0.0 };
        }
    }
    out
}

fn without_translation(m: &Mat4) -> Mat4 {
    let mut out = *m;
    out[12] = 0.0;
    out[13] = 0.0;
    out[14] = 0.0;
    out
}

fn is_scaled_identity(m: &Mat4) -> bool {
    let d = m[0];
    let tol = 1e-9 * d.abs().max(1.0);
    (m[5] - d).abs() < tol
        && (m[10] - d).abs() < tol
        && [1, 2, 4, 6, 8, 9].iter().all(|i| m[*i].abs() < tol)
}

/// A model's world matrix under FBX's inherit types, as the SDK's
/// Transformations sample (and three.js, which ports it) computes it.
///
/// - 0, `eInheritRrSs`: the parent's scale is applied after the child's
///   rotation (`Rparent·Rlocal·Sparent·Slocal`), so a non-uniformly scaled
///   parent does not shear its rotated children;
/// - 1, `eInheritRSrs`: ordinary composition, glTF's;
/// - 2, `eInheritRrs`: as 0 but without the parent's own local scale —
///   Maya's "segment scale compensate", which Mixamo writes on every joint.
fn fbx_world(parent: &Mat4, parent_local: &Mat4, local: &Mat4, inherit: i64) -> Mat4 {
    if inherit == 1 {
        return math::mat_mul(parent, local);
    }
    let p_rs = without_translation(parent);
    let p_r = rotation_part(&p_rs);
    // A true inverse: under a sheared parent the column-normalised rotation
    // is not orthogonal, and its transpose is not its inverse (the SDK's
    // sample and three.js's FBXLoader invert it).
    let p_r_inv = math::invert(&p_r).unwrap_or_else(|| math::transpose(&p_r));
    let p_s = math::mat_mul(&p_r_inv, &p_rs);
    let l_rs = without_translation(local);
    let l_r = rotation_part(&l_rs);
    let l_s = math::mat_mul(&math::transpose(&l_r), &l_rs);
    let rs = if inherit == 0 {
        math::mat_mul(&p_r, &math::mat_mul(&l_r, &math::mat_mul(&p_s, &l_s)))
    } else {
        let pl = without_translation(parent_local);
        let len = |c: usize| (pl[c * 4] * pl[c * 4] + pl[c * 4 + 1] * pl[c * 4 + 1] + pl[c * 4 + 2] * pl[c * 4 + 2]).sqrt();
        let inv_ls = math::scaling([1.0 / len(0).max(1e-300), 1.0 / len(1).max(1e-300), 1.0 / len(2).max(1e-300)]);
        math::mat_mul(&p_r, &math::mat_mul(&l_r, &math::mat_mul(&math::mat_mul(&p_s, &inv_ls), &l_s)))
    };
    let t = math::transform_point_f64(parent, [local[12], local[13], local[14]]);
    let mut out = rs;
    out[12] = t[0];
    out[13] = t[1];
    out[14] = t[2];
    out
}

/// Decompose a matrix into TRS, folding any shear the matrix carries into
/// the nearest rotation (Gram-Schmidt) when it has no exact TRS form.
fn trs_lossy(m: &Mat4) -> ([f64; 3], [f64; 4], [f64; 3]) {
    math::decompose_lossy(m)
}

// ─── layer elements ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mapping {
    PolygonVertex,
    Vertex,
    Polygon,
    AllSame,
}

/// One decoded layer element: values, an optional index, a mapping.
struct Layer {
    mapping: Mapping,
    values: Vec<f64>,
    index: Option<Vec<i32>>,
    stride: usize,
}

impl Layer {
    /// The value for a corner: `corner` indexes `PolygonVertexIndex`, `poly`
    /// the polygon, `cp` the control point.
    fn get(&self, corner: usize, poly: usize, cp: usize, what: &str) -> Result<&[f64], MeshError> {
        let element = match self.mapping {
            Mapping::PolygonVertex => corner,
            Mapping::Vertex => cp,
            Mapping::Polygon => poly,
            Mapping::AllSame => 0,
        };
        let direct = match &self.index {
            Some(ix) => {
                let i = *ix
                    .get(element)
                    .ok_or_else(|| malformed(format!("{what} index array has no entry {element}")))?;
                usize::try_from(i).map_err(|_| malformed(format!("{what} index {i} is negative")))?
            }
            None => element,
        };
        let start = direct.checked_mul(self.stride).ok_or_else(|| malformed(format!("{what} index overflows")))?;
        self.values
            .get(start..start + self.stride)
            .ok_or_else(|| malformed(format!("{what} has no element {direct} ({} values)", self.values.len())))
    }
}

/// The segments of a `Line` geometry's `PointsIndex`, each handed to `each`
/// as its two control points: consecutive entries make a segment, and an
/// entry `-(e + 1)` names point `e` and ends the strip, so no segment joins it
/// to the entry after. A strip of c points gives c - 1 segments and one of
/// fewer than 2 gives none. A trailing strip with no end stops at the array's
/// end, with no closing segment (Ruling I10-R10: assimp wraps it back to the
/// first index, the FBX SDK always ends a strip). An entry that names a point
/// outside `0..cps` is malformed, as a polygon corner's is.
fn walk_strips(index: &[i32], cps: usize, what: &str, mut each: impl FnMut(usize, usize)) -> Result<(), MeshError> {
    let mut previous: Option<usize> = None;
    for (i, &raw) in index.iter().enumerate() {
        let (cp, end) = if raw < 0 { (!raw as usize, true) } else { (raw as usize, false) };
        if cp >= cps {
            return Err(malformed(format!("{what} point index {i} names point {cp} of {cps}")));
        }
        if let Some(p) = previous {
            each(p, cp);
        }
        previous = if end { None } else { Some(cp) };
    }
    Ok(())
}

/// Ceiling-free count check before any per-corner work: an attribute array
/// shorter than its mapping needs is malformed, found here rather than at
/// the first missing corner.
fn check_layer(l: &Layer, corners: usize, polys: usize, cps: usize, what: &str) -> Result<(), MeshError> {
    let need = match l.mapping {
        Mapping::PolygonVertex => corners,
        Mapping::Vertex => cps,
        Mapping::Polygon => polys,
        Mapping::AllSame => 1,
    };
    match &l.index {
        Some(ix) if ix.len() < need => Err(malformed(format!("{what} index has {} entries for {need} elements", ix.len()))),
        None if l.values.len() / l.stride.max(1) < need => Err(malformed(format!(
            "{what} has {} values for {need} elements of {}",
            l.values.len(),
            l.stride
        ))),
        _ => Ok(()),
    }
}

fn read_p70<'a>(dom: &Dom<'a>, rec: RecId) -> Result<PMap<'a>, MeshError> {
    let mut m: PMap<'a> = HashMap::new();
    if let Some(p70) = dom.child(rec, "Properties70") {
        for p in dom.children_named(p70, "P") {
            let props = dom.props(p)?;
            let Some(name) = props.first().and_then(|x| x.as_str()).map(|s| s.into_owned()) else {
                return Err(malformed(format!("a {} property has no name", dom.name(rec))));
            };
            let values: Vec<Prop<'a>> = props.into_iter().skip(4).collect();
            m.entry(name).or_insert(values);
        }
    }
    Ok(m)
}

// ─── curves ─────────────────────────────────────────────────────────────────

/// How a curve moves from one key to the next.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Seg {
    /// Hold the key's value (`next`: hold the NEXT key's value instead).
    Constant { next: bool },
    Linear,
    /// A cubic Bézier in (time, value): it leaves the key with slope `right`
    /// (value units per second) along a handle `wr` of the interval long, and
    /// reaches the next key with slope `next_left` along a handle `wl` long.
    /// At the default weights (a third each) this is plain Hermite.
    Cubic { right: f64, next_left: f64, wr: f64, wl: f64 },
}

/// A tangent handle's default length, as a fraction of its interval.
const DEFAULT_WEIGHT: f64 = 1.0 / 3.0;

/// The Bézier parameter at which a curve whose time handles end at `x1` and
/// `x2` (fractions of the interval, each in [0, 1], so the curve's time is
/// monotonic) reaches the time fraction `u`.
fn bezier_param(x1: f64, x2: f64, u: f64) -> f64 {
    let x = |s: f64| {
        let r = 1.0 - s;
        3.0 * r * r * s * x1 + 3.0 * r * s * s * x2 + s * s * s
    };
    let dx = |s: f64| {
        let r = 1.0 - s;
        3.0 * (r * r * x1 + 2.0 * r * s * (x2 - x1) + s * s * (1.0 - x2))
    };
    // Newton's method kept inside a shrinking bracket: where the slope is
    // flat (a zero-length handle) a step that would leave the bracket
    // bisects instead.
    let (mut lo, mut hi, mut s) = (0.0f64, 1.0f64, u);
    for _ in 0..64 {
        let e = x(s) - u;
        if e.abs() <= 1e-12 {
            break;
        }
        if e < 0.0 {
            lo = s;
        } else {
            hi = s;
        }
        let d = dx(s);
        let next = s - e / d;
        s = if d > 0.0 && next > lo && next < hi { next } else { 0.5 * (lo + hi) };
    }
    s
}

/// One decoded animation curve.
#[derive(Debug, Clone)]
struct Curve {
    times: Vec<f64>,
    values: Vec<f64>,
    /// Per key, the segment that starts there (the last entry is unused).
    seg: Vec<Seg>,
    /// The resampling tolerance: `RESAMPLE_TOLERANCE` of the key range.
    tol: f64,
}

impl Curve {
    fn eval(&self, t: f64) -> f64 {
        let n = self.times.len();
        if n == 0 {
            return 0.0;
        }
        // Right-continuous: where several keys share a time (a jump), the
        // value AT that time is the last of them, and before it the first.
        if t < self.times[0] {
            return self.values[0];
        }
        if t >= self.times[n - 1] {
            return self.values[n - 1];
        }
        let i = self.times.partition_point(|x| *x <= t).saturating_sub(1).min(n - 2);
        let (t0, t1) = (self.times[i], self.times[i + 1]);
        let (v0, v1) = (self.values[i], self.values[i + 1]);
        // At a key, its own value — even where the segment it starts holds
        // the next key's value from just after it (as ufbx evaluates).
        if t == t0 {
            return v0;
        }
        let dt = t1 - t0;
        if dt <= 0.0 {
            return v1;
        }
        let u = (t - t0) / dt;
        match self.seg[i] {
            Seg::Constant { next } => {
                if next {
                    v1
                } else {
                    v0
                }
            }
            Seg::Linear => v0 + (v1 - v0) * u,
            Seg::Cubic { right, next_left, wr, wl } => {
                let s = if wr == DEFAULT_WEIGHT && wl == DEFAULT_WEIGHT { u } else { bezier_param(wr, 1.0 - wl, u) };
                let (y1, y2) = (v0 + right * wr * dt, v1 - next_left * wl * dt);
                let r = 1.0 - s;
                r * r * r * v0 + 3.0 * (r * r * s * y1 + r * s * s * y2) + s * s * s * v1
            }
        }
    }

    /// The value from just after `t`: what a stepped channel holds from `t`
    /// on. It differs from `eval` only at a key whose segment holds the next
    /// key's value.
    fn eval_right(&self, t: f64) -> f64 {
        let n = self.times.len();
        if n >= 2 && t >= self.times[0] && t < self.times[n - 1] {
            let i = self.times.partition_point(|x| *x <= t).saturating_sub(1).min(n - 2);
            if self.times[i] == t && self.times[i + 1] > t && self.seg[i] == (Seg::Constant { next: true }) {
                return self.values[i + 1];
            }
        }
        self.eval(t)
    }

    /// `eval`, or `eval_right` for a stepped channel.
    fn at(&self, t: f64, step: bool) -> f64 {
        if step {
            self.eval_right(t)
        } else {
            self.eval(t)
        }
    }

    /// The segment covering the open interval `(a, b)`, if any key pair does.
    fn seg_over(&self, a: f64, b: f64) -> Option<Seg> {
        let n = self.times.len();
        if n < 2 || b <= self.times[0] || a >= self.times[n - 1] {
            return None;
        }
        let i = self.times.partition_point(|x| *x <= a).saturating_sub(1).min(n - 2);
        Some(self.seg[i])
    }

    fn all_constant(&self) -> bool {
        self.times.len() < 2 || self.seg[..self.seg.len() - 1].iter().all(|s| matches!(s, Seg::Constant { .. }))
    }
}

/// The flag bits of `KeyAttrFlags` (the SDK's `FbxAnimCurveDef`).
const FLAG_CONSTANT: i32 = 0x0000_0002;
const FLAG_CUBIC: i32 = 0x0000_0008;
/// With CONSTANT: hold the next key's value. With CUBIC: auto tangents.
const FLAG_CONSTANT_NEXT: i32 = 0x0000_0100;
const FLAG_TANGENT_TCB: i32 = 0x0000_0200;
const FLAG_TANGENT_USER: i32 = 0x0000_0400;
const FLAG_CLAMP: i32 = 0x0000_1000;
const FLAG_TIME_INDEPENDENT: i32 = 0x0000_2000;
const FLAG_CLAMP_PROGRESSIVE: i32 = 0x0000_4000;
const FLAG_WEIGHTED_RIGHT: i32 = 0x0100_0000;
const FLAG_WEIGHTED_NEXT_LEFT: i32 = 0x0200_0000;

/// The slope of an auto tangent at a key with neighbours on both sides.
///
/// The SDK computes auto tangents on load rather than storing them; these
/// rules are ufbx's reconstruction (`ufbxi_solve_auto_tangent` in ufbx.c),
/// which its test suite checks against curves evaluated by Maya and
/// MotionBuilder. `bias` is the key's auto bias (MotionBuilder), in percent.
#[allow(clippy::too_many_arguments)]
fn auto_tangent(t: [f64; 3], v: [f64; 3], wl: f64, wr: f64, bias: f64, flags: i32) -> f64 {
    let ([tp, t, tn], [vp, v, vn]) = (t, v);
    // Clamped: flat where a neighbour has the same value.
    if flags & FLAG_CLAMP != 0 && (vp - v).abs().min((vn - v).abs()) <= 0.0 {
        return 0.0;
    }
    let mut slope = (vn - vp) / (tn - tp);
    if flags & FLAG_TIME_INDEPENDENT == 0 {
        let (sl, sr) = ((v - vp) / (t - tp), (vn - v) / (tn - t));
        let delta = (t - tp) / (tn - tp);
        slope = slope * 0.5 + (sl * (1.0 - delta) + sr * delta) * 0.5;
        let bw = bias.abs() / 100.0;
        if bw > 1e-4 {
            let target = if bias > 0.0 { sr } else { sl };
            let pull = target - slope;
            slope = slope * (1.0 - bw) + target * bw;
            // A bias past 500% adds an absolute slope as well.
            let extra = bw - 5.0;
            if extra > 0.0 {
                let sign = if pull.abs() > 1e-5 { pull } else { bias };
                slope += extra * extra * sign.signum() * 40.0;
            }
        }
    }
    // Clamped progressive: shorten the slope until neither handle passes
    // its neighbour's value (flat at a local extremum).
    if flags & FLAG_CLAMP_PROGRESSIVE != 0 {
        let sign = if slope >= 0.0 { 1.0 } else { -1.0 };
        let (rl, rr) = (wl * (t - tp), wr * (tn - t));
        let ml = if rl > 0.0 { sign * (v - vp) / rl } else { 0.0 };
        let mr = if rr > 0.0 { sign * (vn - v) / rr } else { 0.0 };
        let limit = |m: f64| if m > 0.0 { m } else { 0.0 };
        slope = sign * (sign * slope).min(limit(ml)).min(limit(mr));
    }
    slope
}

/// The slope of an auto tangent at a curve's first or last key: the line to
/// its one neighbour (`dv` over `dt`), as ufbx reconstructs it.
fn auto_tangent_end(dt: f64, dv: f64, bias: f64, flags: i32) -> f64 {
    if flags & FLAG_CLAMP_PROGRESSIVE != 0 || (flags & FLAG_CLAMP != 0 && dv.abs() <= 0.0) {
        return 0.0;
    }
    let mut slope = dv / dt;
    if flags & FLAG_TIME_INDEPENDENT == 0 {
        let extra = bias.abs() / 100.0 - 5.0;
        if extra > 0.0 {
            slope += extra * extra * bias.signum() * 40.0;
        }
    }
    slope
}

/// A Kochanek-Bartels (TCB) key's (left, right) slopes from its tension,
/// continuity and bias and the slopes to its neighbours. `edge`: the key
/// has only one neighbour.
fn tcb_slopes(tension: f64, continuity: f64, bias: f64, sl: f64, sr: f64, edge: bool) -> (f64, f64) {
    let k = if edge { 1.0 } else { 0.5 } * (1.0 - tension);
    let (bp, bm, cp, cm) = (1.0 + bias, 1.0 - bias, 1.0 + continuity, 1.0 - continuity);
    (k * bp * cm * sl + k * bm * cp * sr, k * bp * cp * sl + k * bm * cm * sr)
}

// ─── the reader ─────────────────────────────────────────────────────────────

/// Which of FBX's material conventions carries a material's PBR factors.
///
/// FBX has no PBR material of its own (research notes §4.2), so each DCC
/// stores one in its own properties beside, or instead of, the classic Phong
/// ones — which are then only an approximation the DCC filled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flavour {
    /// Maya's Stingray PBS (`Maya|base_color`, …), which this module's
    /// writer also emits.
    Stingray,
    /// Maya's Arnold aiStandardSurface / Autodesk Standard Surface
    /// (`Maya|baseColor`, `Maya|metalness`, `Maya|specularRoughness`, …).
    Arnold,
    /// 3ds Max's Physical Material (`3dsMax|Parameters|base_color`, …).
    MaxPhysical,
    /// 3ds Max's OpenPBR material (`3dsMax|Parameters|base_metalness`, …).
    MaxOpenPbr,
    /// 3ds Max's "PBR Material (Metal/Rough)" or glTF material
    /// (`3dsMax|main|…`), with the property its base colour is spelled as.
    MaxPbr(&'static str),
    /// Only the classic Lambert/Phong properties.
    Phong,
}

/// How a material's maps combine with its colours (see `build_material`).
#[derive(Debug, Clone, Copy)]
struct MapUse {
    /// When a base-colour map is present, the factor that replaces the
    /// stored colour (`None`: the colour multiplies the map).
    base: Option<f32>,
    /// The same for emission.
    emissive: Option<f32>,
    base_used: bool,
    emissive_used: bool,
    normal_used: bool,
    ao_used: bool,
}

impl MapUse {
    /// Every connected map used, every colour a multiplier.
    const ALL: MapUse = MapUse {
        base: None,
        emissive: None,
        base_used: true,
        emissive_used: true,
        normal_used: true,
        ao_used: true,
    };
}

impl Flavour {
    /// The family's layer properties, for the flavours that have layers.
    fn layers(self) -> Option<&'static LayerNames> {
        match self {
            Flavour::Arnold => Some(&STANDARD_SURFACE),
            Flavour::MaxPhysical => Some(&MAX_PHYSICAL),
            Flavour::MaxOpenPbr => Some(&OPENPBR),
            Flavour::Stingray | Flavour::MaxPbr(_) | Flavour::Phong => None,
        }
    }
}

/// One DCC family's layer properties as its FBX exports name them, and the
/// defaults its documentation gives (the module doc's "Material layers"
/// says how they are read). The names are those of real exports in ufbx
/// v0.23.1's `data/`, and each family's are pinned by a test
/// (`arnold_layers_read`, `physical_layers_read`, `openpbr_layers_read`).
struct LayerNames {
    /// The family, as warnings name it.
    family: &'static str,
    /// Its colours are 3ds Max `ColorAndAlpha`s, decoded like its base
    /// colour; otherwise they are Maya's linear `Vector3D`s.
    max_colors: bool,
    coat: &'static str,
    /// The coat's roughness, with the family's default for it.
    coat_roughness: (&'static str, f64),
    /// A switch that makes the coat roughness a glossiness.
    coat_gloss: Option<&'static str>,
    coat_color: &'static str,
    /// The coat's ior, with the family's default, which goes unwarned.
    coat_ior: (&'static str, f64),
    coat_anisotropy: Option<&'static str>,
    /// What the coat does to the layers under it, which glTF's coat doesn't,
    /// each with its default, which goes unwarned (Ruling A24g).
    coat_affect: &'static [(&'static str, f64)],
    transmission: &'static str,
    /// The transmission's own roughness, which glTF's can't have (Ruling
    /// A24h).
    transmission_roughness: TransmissionRoughness,
    transmission_color: &'static str,
    transmission_depth: &'static str,
    thin_walled: &'static str,
    /// Whether a thin-walled sheet keeps its transmission colour as its tint
    /// when it also states a depth.
    thin_tint: bool,
    /// The base ior, with the family's documented default, which reads as
    /// unset (Ruling A24b).
    ior: (&'static str, f64),
    /// The weight that is glTF's `specularFactor`, if the family has one.
    specular: Option<&'static str>,
    /// A specular weight glTF has no slot for.
    specular_weight: Option<&'static str>,
    specular_color: &'static str,
    /// The specular anisotropy, with its value for none.
    anisotropy: (&'static str, f64),
    dispersion: Option<&'static str>,
    /// Weight, colour and roughness of a sheen that is glTF's, with the
    /// roughness's default.
    sheen: Option<(&'static str, &'static str, &'static str, f64)>,
    /// The weight of a fuzz layer, a different model from glTF's sheen.
    fuzz: Option<&'static str>,
    film: Film,
    /// The material properties whose textures feed a layer the reader
    /// reads. FBX layer textures are not carried yet.
    texture_slots: &'static [(&'static str, MaterialLayer)],
}

/// How a family states a transmission roughness apart from its specular
/// roughness. glTF's transmission takes the base roughness:
/// KHR_materials_transmission L123, "This microfacet lobe is exactly the same
/// as the specular lobe except sampled along the line of sight through the
/// surface".
enum TransmissionRoughness {
    /// Added to the specular roughness, 0 by default.
    Extra(&'static str),
    /// Its own roughness when its lock (on by default) is off, and a switch
    /// that makes it a glossiness.
    Unlocked { roughness: &'static str, lock: &'static str, invert: &'static str },
    None,
}

/// How a family states a thin film (glTF's iridescence).
enum Film {
    /// Thickness in nanometres, on when above 0, and the film's ior with its
    /// default.
    Thickness(&'static str, (&'static str, f64)),
    /// Weight, thickness in micrometres and ior, each with its default.
    Weighted { weight: &'static str, thickness: (&'static str, f64), ior: (&'static str, f64) },
    None,
}

/// Arnold's aiStandardSurface and Maya's Standard Surface (`Flavour::Arnold`).
/// The names are Maya's: `data/maya_arnold_properties_7500_ascii.fbx` and
/// its standardSurface twin `maya_osl_properties_7500_ascii.fbx`, whose
/// BindingTable binds each to its Standard Surface parameter
/// (`"Maya|coatRoughness"` → `coat_roughness`; ufbx.c L19570-L19618).
/// The meanings, defaults and units are the Standard Surface spec's,
/// v1.0.1.1 (`index.html`; "SS" below), and its `reference/standard_surface.osl`
/// at the same tag:
/// - coat → `clearcoatFactor` (L161), coat_roughness → its roughness, both
///   squared into the GGX alpha (L163, .osl L219; KHR_materials_clearcoat:
///   `α = clearcoatRoughness^2`); defaults 0.1 and ior 1.5 (L163, L166).
///   coat_affect_color and coat_affect_roughness darken and roughen the base
///   under the coat, default 0 (L142-L151, L168-L169);
/// - transmission → `transmissionFactor` (L276, L295); a depth of 0 tints
///   by the colour and no medium absorbs, a positive one tints white and
///   absorbs (L282, L289, L297). transmission_extra_roughness adds to the
///   specular roughness for the transmission alone, default 0 (L285, L301);
/// - specular_IOR → `ior` (L258, L303), whose default is 1.52 in the OSL
///   (L162) and in Maya (`data/maya_arnold_textures_7500_ascii.fbx`);
/// - specular → `specularFactor`: both scale the whole dielectric lobe (L246;
///   KHR_materials_specular: `dielectric_f90 = specular`);
/// - sheen × sheen_color → `sheenColorFactor`, √sheen_roughness →
///   `sheenRoughnessFactor`: MaterialX v1.39.5 feeds both to one
///   `sheen_bsdf`, glTF's roughness squared and SS's as it is
///   (`gltf_pbr.mtlx` L258-L271, `standard_surface.mtlx` L259-L262); the
///   roughness's default is 0.3 (L333). A sheen weight the file doesn't
///   state reads as off, the reference code's 0 (.osl L177,
///   `standard_surface.mtlx` L60), not the 0.8 of the spec's table (L331);
/// - thin_film_thickness (nanometres, L229) → the thickness glTF uses
///   without a texture, `iridescenceThicknessMaximum`, at factor 1;
///   thin_film_IOR (default 1.5, L230) → `iridescenceIor`. Both films sit
///   between vacuum and the base's ior (L216; KHR_materials_iridescence).
///
/// glTF's side, here and below, is the `KHR_materials_*` text at
/// KhronosGroup/glTF 9d23a5a.
const STANDARD_SURFACE: LayerNames = LayerNames {
    family: "Standard Surface",
    max_colors: false,
    coat: "Maya|coat",
    coat_roughness: ("Maya|coatRoughness", 0.1),
    coat_gloss: None,
    coat_color: "Maya|coatColor",
    coat_ior: ("Maya|coatIOR", 1.5),
    coat_anisotropy: Some("Maya|coatAnisotropy"),
    coat_affect: &[("Maya|coatAffectColor", 0.0), ("Maya|coatAffectRoughness", 0.0)],
    transmission: "Maya|transmission",
    transmission_roughness: TransmissionRoughness::Extra("Maya|transmissionExtraRoughness"),
    transmission_color: "Maya|transmissionColor",
    transmission_depth: "Maya|transmissionDepth",
    thin_walled: "Maya|thinWalled",
    thin_tint: false,
    ior: ("Maya|specularIOR", 1.52),
    specular: Some("Maya|specular"),
    specular_weight: None,
    specular_color: "Maya|specularColor",
    anisotropy: ("Maya|specularAnisotropy", 0.0),
    dispersion: Some("Maya|transmissionDispersion"),
    sheen: Some(("Maya|sheen", "Maya|sheenColor", "Maya|sheenRoughness", 0.3)),
    fuzz: None,
    film: Film::Thickness("Maya|thinFilmThickness", ("Maya|thinFilmIOR", 1.5)),
    texture_slots: &[
        ("Maya|coat", MaterialLayer::Clearcoat),
        ("Maya|coatRoughness", MaterialLayer::Clearcoat),
        ("Maya|coatColor", MaterialLayer::Clearcoat),
        ("Maya|coatIOR", MaterialLayer::Clearcoat),
        ("Maya|coatNormal", MaterialLayer::Clearcoat),
        ("Maya|transmission", MaterialLayer::Transmission),
        ("Maya|transmissionColor", MaterialLayer::Transmission),
        ("Maya|specularIOR", MaterialLayer::Ior),
        ("Maya|specular", MaterialLayer::Specular),
        ("Maya|specularColor", MaterialLayer::Specular),
        ("Maya|sheen", MaterialLayer::Sheen),
        ("Maya|sheenColor", MaterialLayer::Sheen),
        ("Maya|sheenRoughness", MaterialLayer::Sheen),
        ("Maya|thinFilmThickness", MaterialLayer::Iridescence),
        ("Maya|thinFilmIOR", MaterialLayer::Iridescence),
    ],
};

/// 3ds Max's Physical Material (`Flavour::MaxPhysical`). The names are
/// `data/max_physical_material_properties_7500_ascii.fbx`'s (ufbx.c
/// L19628-L19679). The meanings and defaults are Autodesk's: the 3ds Max
/// 2024 help's "Physical Material Parameters" (GUID-C1328905) and the
/// MAXScript reference's Physical_Material (3ds Max 2018, GUID-57562F6A):
/// - coating → `clearcoatFactor`, coat_roughness → its roughness, on the
///   material's roughness scale (0.0 "completely smooth and mirror-like",
///   1.0 "extremely rough"), as the base layer reads it, and a glossiness
///   when coat_roughness_inv is on; defaults 0 and coat ior 1.52.
///   coat_affect_color and coat_affect_roughness default to 0.5;
/// - transparency → `transmissionFactor`; a trans_depth of 0 tints at the
///   surface, a positive one absorbs, and a thin-walled surface has no
///   depth. trans_roughness is its own only when trans_roughness_lock (on
///   by default) is off, a glossiness when trans_roughness_inv is on
///   (MAXScript L81-L94);
/// - trans_ior → `ior` ("It defines both how much rays bend when entering
///   the medium, and also the angular dependency of the reflectivity when
///   set to Fresnel mode (the default)", MAXScript L74-L75), default 1.52.
///   The Custom Curve mode (brdf_mode false, with brdf_low, brdf_high and
///   brdf_curve; MAXScript L141-L163) is not read: its reflectance is read
///   as the ior's;
/// - reflectivity → `specularFactor`: the reflection is "the weight
///   multiplied by the angular function", default 1.
///
/// Its anisotropy is a roughness ratio, 1.0 being "not stretched".
const MAX_PHYSICAL: LayerNames = LayerNames {
    family: "3ds Max Physical",
    max_colors: true,
    coat: "3dsMax|Parameters|coating",
    coat_roughness: ("3dsMax|Parameters|coat_roughness", 0.0),
    coat_gloss: Some("3dsMax|Parameters|coat_roughness_inv"),
    coat_color: "3dsMax|Parameters|coat_color",
    coat_ior: ("3dsMax|Parameters|coat_ior", 1.52),
    coat_anisotropy: None,
    coat_affect: &[("3dsMax|Parameters|coat_affect_color", 0.5), ("3dsMax|Parameters|coat_affect_roughness", 0.5)],
    transmission: "3dsMax|Parameters|transparency",
    transmission_roughness: TransmissionRoughness::Unlocked {
        roughness: "3dsMax|Parameters|trans_roughness",
        lock: "3dsMax|Parameters|trans_roughness_lock",
        invert: "3dsMax|Parameters|trans_roughness_inv",
    },
    transmission_color: "3dsMax|Parameters|trans_color",
    transmission_depth: "3dsMax|Parameters|trans_depth",
    thin_walled: "3dsMax|Parameters|thin_walled",
    thin_tint: true,
    ior: ("3dsMax|Parameters|trans_ior", 1.52),
    specular: Some("3dsMax|Parameters|reflectivity"),
    specular_weight: None,
    specular_color: "3dsMax|Parameters|refl_color",
    anisotropy: ("3dsMax|Parameters|anisotropy", 1.0),
    dispersion: None,
    sheen: None,
    fuzz: None,
    film: Film::None,
    texture_slots: &[
        ("3dsMax|Parameters|coat_map", MaterialLayer::Clearcoat),
        ("3dsMax|Parameters|coat_rough_map", MaterialLayer::Clearcoat),
        ("3dsMax|Parameters|coat_color_map", MaterialLayer::Clearcoat),
        ("3dsMax|Parameters|coat_bump_map", MaterialLayer::Clearcoat),
        ("3dsMax|Parameters|transparency_map", MaterialLayer::Transmission),
        ("3dsMax|Parameters|trans_color_map", MaterialLayer::Transmission),
        ("3dsMax|Parameters|trans_ior_map", MaterialLayer::Ior),
        ("3dsMax|Parameters|reflectivity_map", MaterialLayer::Specular),
        ("3dsMax|Parameters|refl_color_map", MaterialLayer::Specular),
    ],
};

/// 3ds Max's OpenPBR material (`Flavour::MaxOpenPbr`). The names are
/// `data/max_openpbr_material_7700_ascii.fbx`'s (ufbx.c L19703-L19751). The
/// meanings, defaults and units are the OpenPBR Surface spec's, v1.1.1
/// (`index.html`):
/// - coat_weight → `clearcoatFactor`, coat_roughness r → alpha r², as
///   glTF's (L398-L401, L421); defaults 0, coat ior 1.6 and coat_darkening
///   1 (L936-L941);
/// - transmission_weight → `transmissionFactor` (L839); a depth of 0 tints
///   by the colour with no medium (L784), and a thin-walled sheet takes its
///   colour as its transmittance, its refraction un-deflected (L1259). Its
///   transmission has no roughness of its own;
/// - specular_ior → `ior` (L518, default 1.5);
/// - thin_film_weight → `iridescenceFactor`, thin_film_thickness in
///   micrometres (default 0.5) × 1000 → nanometres, as MaterialX v1.39.5's
///   `open_pbr_surface.mtlx` does (L298-L301), thin_film_ior (default 1.4)
///   → `iridescenceIor` (L853-L855, L881-L883).
///
/// specular_weight scales the reflectance through the ior and keeps f90,
/// and specular_color tints the whole Fresnel factor (L494-L507, L496),
/// neither as glTF's specular does; the anisotropy stretches the NDF
/// another way (L412-L415).
const OPENPBR: LayerNames = LayerNames {
    family: "OpenPBR",
    max_colors: true,
    coat: "3dsMax|Parameters|coat_weight",
    coat_roughness: ("3dsMax|Parameters|coat_roughness", 0.0),
    coat_gloss: None,
    coat_color: "3dsMax|Parameters|coat_color",
    coat_ior: ("3dsMax|Parameters|coat_ior", 1.6),
    coat_anisotropy: Some("3dsMax|Parameters|coat_roughness_anisotropy"),
    coat_affect: &[("3dsMax|Parameters|coat_darkening", 1.0)],
    transmission: "3dsMax|Parameters|transmission_weight",
    transmission_roughness: TransmissionRoughness::None,
    transmission_color: "3dsMax|Parameters|transmission_color",
    transmission_depth: "3dsMax|Parameters|transmission_depth",
    thin_walled: "3dsMax|Parameters|geometry_thin_walled",
    thin_tint: true,
    ior: ("3dsMax|Parameters|specular_ior", 1.5),
    specular: None,
    specular_weight: Some("3dsMax|Parameters|specular_weight"),
    specular_color: "3dsMax|Parameters|specular_color",
    anisotropy: ("3dsMax|Parameters|specular_roughness_anisotropy", 0.0),
    dispersion: Some("3dsMax|Parameters|transmission_dispersion_scale"),
    sheen: None,
    fuzz: Some("3dsMax|Parameters|fuzz_weight"),
    film: Film::Weighted {
        weight: "3dsMax|Parameters|thin_film_weight",
        thickness: ("3dsMax|Parameters|thin_film_thickness", 0.5),
        ior: ("3dsMax|Parameters|thin_film_ior", 1.4),
    },
    texture_slots: &[
        ("3dsMax|Parameters|coat_weight_map", MaterialLayer::Clearcoat),
        ("3dsMax|Parameters|coat_roughness_map", MaterialLayer::Clearcoat),
        ("3dsMax|Parameters|coat_color_map", MaterialLayer::Clearcoat),
        ("3dsMax|Parameters|coat_ior_map", MaterialLayer::Clearcoat),
        ("3dsMax|Parameters|coat_bump_map", MaterialLayer::Clearcoat),
        ("3dsMax|Parameters|geometry_coat_normal_map", MaterialLayer::Clearcoat),
        ("3dsMax|Parameters|transmission_weight_map", MaterialLayer::Transmission),
        ("3dsMax|Parameters|transmission_color_map", MaterialLayer::Transmission),
        ("3dsMax|Parameters|specular_ior_map", MaterialLayer::Ior),
        ("3dsMax|Parameters|thin_film_weight_map", MaterialLayer::Iridescence),
        ("3dsMax|Parameters|thin_film_thickness_map", MaterialLayer::Iridescence),
        ("3dsMax|Parameters|thin_film_ior_map", MaterialLayer::Iridescence),
    ],
};

/// A material's layer properties being read, and what the reader must say
/// about them.
struct LayerRead<'m, 'a> {
    props: Props<'m, 'a>,
    what: &'m str,
    family: &'static str,
    /// The space the file's colours are in, which warnings name them in.
    stored: &'static str,
    notes: Vec<String>,
}

impl LayerRead<'_, '_> {
    /// A number the file states, `None` when it states none. One that is not
    /// a finite 32-bit number is noted and ignored.
    fn number(&mut self, name: &str) -> Option<f64> {
        let v = self.props.f64(name)?;
        if (v as f32).is_finite() {
            return Some(v);
        }
        self.notes.push(format!("{} has {name} {}, which is not a finite number; it was ignored", self.what, shown(v)));
        None
    }

    /// A 0–1 weight; outside that it is noted and the layer left out, never
    /// clamped (Ruling A16).
    fn weight(&mut self, name: &str, layer: &str) -> Option<f64> {
        let v = self.number(name)?;
        if (0.0..=1.0).contains(&v) {
            return Some(v);
        }
        self.notes.push(format!("{} has {name} {}, outside 0 to 1; the {layer} layer was left out", self.what, shown(v)));
        None
    }

    /// A 0–1 value, or the family's default when the file states none or one
    /// outside 0 to 1 (noted).
    fn unit(&mut self, name: &str, default: f64) -> f64 {
        match self.number(name) {
            Some(v) if (0.0..=1.0).contains(&v) => v,
            Some(v) => {
                self.notes.push(format!("{} has {name} {}, outside 0 to 1; it was read as {}'s default {}", self.what, shown(v), self.family, shown(default)));
                default
            }
            None => default,
        }
    }

    /// A film's ior, or the family's default when the file states none or one
    /// below KHR_materials_iridescence's minimum of 1 (noted).
    fn film_ior(&mut self, (name, default): (&str, f64)) -> f32 {
        match self.number(name) {
            Some(v) if v >= 1.0 => v as f32,
            Some(v) => {
                self.notes.push(format!(
                    "{} has {name} {}, which KHR_materials_iridescence cannot hold (1 and above); it was read as {}'s default {}",
                    self.what,
                    shown(v),
                    self.family,
                    shown(default)
                ));
                default as f32
            }
            None => default as f32,
        }
    }

    /// An RGB colour as the file stores it (a 3ds Max colour's alpha aside).
    /// One that is not finite is noted and ignored.
    fn color(&mut self, name: &str) -> Option<[f64; 3]> {
        let c = self.props.vec3(name)?;
        if c.iter().all(|v| (*v as f32).is_finite()) {
            return Some(c);
        }
        self.notes.push(format!("{} has {name} {} {}, which is not a finite colour; it was ignored", self.what, self.stored, shown3(c)));
        None
    }

    fn not_carried(&mut self, name: &str, value: String, why: &str) {
        self.notes.push(format!("{} has {name} {value}; {why}, so it is not carried", self.what));
    }
}

/// A file's number as the scene would hold it, for warnings.
fn shown(v: f64) -> String {
    (v as f32).to_string()
}

/// Characters a note keeps of a name it quotes from the file, then "…"
/// (Ruling A45): well past any real one.
const MAX_QUOTED_NAME: usize = 256;

/// A name from the file as a note quotes it: at most [`MAX_QUOTED_NAME`]
/// characters, then "…".
fn quoted_name(name: &str) -> String {
    match name.char_indices().nth(MAX_QUOTED_NAME) {
        Some((at, _)) => format!("{}…", &name[..at]),
        None => name.to_string(),
    }
}

fn shown3(c: [f64; 3]) -> String {
    format!("({}, {}, {})", shown(c[0]), shown(c[1]), shown(c[2]))
}

/// Read the layers of `names`' family into `m`, whose base layer is read
/// (`base_textured`: a map drives its base colour), and return what glTF
/// can't hold, a warning each (Rulings A16 and A24–A24f).
fn read_layers(names: &LayerNames, props: Props<'_, '_>, what: &str, space: FbxColorSpace, base_textured: bool, m: &mut Material) -> Vec<String> {
    // Warnings name a stored colour in the space the file stores it in
    // (3ds Max's sRGB unless the caller reads them as linear; Maya's are
    // linear), and a colour glTF draws in linear.
    let stored = if names.max_colors && matches!(space, FbxColorSpace::Srgb) { "sRGB" } else { "linear" };
    let mut r = LayerRead { props, what, family: names.family, stored, notes: Vec::new() };
    let fam = names.family;
    let white = |c: &[f64; 3]| c.iter().all(|v| *v == 1.0);
    let decode = |c: [f64; 3]| if names.max_colors { c.map(|v| decode_color(v, space)) } else { c.map(|v| v as f32) };

    if let Some(factor) = r.weight(names.coat, "clearcoat").filter(|w| *w > 0.0) {
        let rough = r.unit(names.coat_roughness.0, names.coat_roughness.1);
        let gloss = names.coat_gloss.is_some_and(|g| props.f64(g).is_some_and(|v| v != 0.0));
        let roughness = (if gloss { 1.0 - rough } else { rough }) as f32;
        m.clearcoat = Some(Clearcoat { factor: factor as f32, roughness, ..Default::default() });
        // glTF's coat is white and isotropic, with an ior of 1.5, and leaves
        // the layers under it as they are (KHR_materials_clearcoat); a
        // family's defaults go unwarned (Rulings A24b, A24g).
        let mut parts = Vec::new();
        if let Some(c) = r.color(names.coat_color).filter(|c| !white(c)) {
            parts.push(format!("{} {stored} {}", names.coat_color, shown3(c)));
        }
        if let Some(v) = r.number(names.coat_ior.0).filter(|v| *v as f32 != 1.5 && *v as f32 != names.coat_ior.1 as f32) {
            parts.push(format!("{} {}", names.coat_ior.0, shown(v)));
        }
        if let Some(name) = names.coat_anisotropy {
            if let Some(v) = r.number(name).filter(|v| *v != 0.0) {
                parts.push(format!("{name} {}", shown(v)));
            }
        }
        for (name, default) in names.coat_affect {
            if let Some(v) = r.number(name).filter(|v| *v as f32 != *default as f32) {
                parts.push(format!("{name} {}", shown(v)));
            }
        }
        if !parts.is_empty() {
            let verb = if parts.len() == 1 { "is" } else { "are" };
            r.notes.push(format!(
                "{what}'s clearcoat was read as glTF's, white and isotropic with an ior of 1.5, leaving the layers under it as they are: its {} {verb} not carried",
                parts.join(", ")
            ));
        }
    }

    if let Some(factor) = r.weight(names.transmission, "transmission").filter(|w| *w > 0.0) {
        m.transmission = Some(Transmission { factor: factor as f32, texture: None });
        let thin = props.f64(names.thin_walled).is_some_and(|v| v != 0.0);
        let depth = r.number(names.transmission_depth).filter(|d| *d > 0.0);
        // glTF's volume needs a thickness, which none of the families
        // stores (Ruling A24c). Dispersion acts only in a refracting volume,
        // so it is named only here (A24a): a thin-walled sheet renders none
        // (SS L287, OpenPBR L1259; KHR_materials_dispersion L35).
        if !thin {
            let mut lost = Vec::new();
            if let Some(d) = depth {
                lost.push(format!("its absorption ({} {})", names.transmission_depth, shown(d)));
            }
            if let Some(name) = names.dispersion {
                if let Some(v) = r.number(name).filter(|v| *v > 0.0) {
                    lost.push(format!("its dispersion ({name} {})", shown(v)));
                }
            }
            let lost = match lost.len() {
                0 => String::new(),
                n => format!(", and {} {} not carried", lost.join(" and "), if n == 1 { "is" } else { "are" }),
            };
            r.notes.push(format!(
                "{what} reads thin-walled, with no refraction: glTF's volume needs a thickness, which {fam} materials do not store{lost}"
            ));
        }
        // glTF's transmission takes the base roughness (Ruling A24h).
        match names.transmission_roughness {
            TransmissionRoughness::Extra(name) => {
                if let Some(v) = r.number(name).filter(|v| *v != 0.0) {
                    r.not_carried(name, shown(v), "glTF's transmission takes the base roughness");
                }
            }
            TransmissionRoughness::Unlocked { roughness, lock, invert } => {
                let unlocked = props.f64(lock).is_some_and(|v| v == 0.0);
                if let Some(v) = r.number(roughness).filter(|_| unlocked) {
                    let own = if props.f64(invert).is_some_and(|v| v != 0.0) { 1.0 - v } else { v };
                    if (own as f32 - m.roughness).abs() > 1e-6 {
                        r.not_carried(
                            roughness,
                            shown(v),
                            &format!("with {lock} off it is apart from the roughness, {}, and glTF's transmission takes the base roughness", m.roughness),
                        );
                    }
                }
            }
            TransmissionRoughness::None => {}
        }
        // glTF tints transmitted light with the base colour
        // (KHR_materials_transmission, "Tinting"); the base colour stays as
        // read (Ruling A24d).
        let tint = if depth.is_some() && !(thin && names.thin_tint) {
            [1.0; 3]
        } else {
            r.color(names.transmission_color).map(decode).unwrap_or([1.0; 3])
        };
        let base = [m.base_color[0], m.base_color[1], m.base_color[2]];
        if base_textured || tint.iter().zip(base).any(|(t, b)| (t - b).abs() > 1e-6) {
            r.notes.push(format!(
                "{what}: glTF tints transmitted light with the base colour{}, linear {}, where {fam} tints it linear {}",
                if base_textured { " and its map" } else { "" },
                shown3(base.map(f64::from)),
                shown3(tint.map(f64::from))
            ));
        }
    }

    // glTF's default and the family's documented one read as unset
    // (Ruling A24b); anything else KHR_materials_ior holds (0, or 1 and
    // above) is carried.
    if let Some(v) = r.number(names.ior.0) {
        let ior = v as f32;
        if ior != super::scene::DEFAULT_IOR && ior != names.ior.1 as f32 {
            if super::scene::ior_in_schema(ior) {
                m.ior = Some(ior);
            } else {
                r.notes.push(format!(
                    "{what} has {} {}, which KHR_materials_ior cannot hold (0, or 1 and above); it is not carried",
                    names.ior.0,
                    shown(v)
                ));
            }
        }
    }

    // Ruling A24f.
    if let Some(name) = names.specular {
        if let Some(w) = r.weight(name, "specular").filter(|w| *w != 1.0) {
            m.specular = Some(Specular { factor: w as f32, ..Default::default() });
        }
    }
    if let Some(name) = names.specular_weight {
        if let Some(v) = r.number(name).filter(|v| *v != 1.0) {
            r.not_carried(name, shown(v), &format!("{fam}'s specular weight scales its reflectance through the ior, unlike glTF's"));
        }
    }
    if let Some(c) = r.color(names.specular_color).filter(|c| !white(c)) {
        r.not_carried(names.specular_color, format!("{stored} {}", shown3(c)), "glTF's specular colour tints only the reflectance at normal incidence");
    }

    // Rulings A24e and A24 (fuzz).
    if let Some(v) = r.number(names.anisotropy.0).filter(|v| *v != names.anisotropy.1) {
        r.not_carried(names.anisotropy.0, shown(v), &format!("{fam}'s anisotropic roughness differs from glTF's"));
    }
    if let Some(name) = names.fuzz {
        if let Some(v) = r.number(name).filter(|v| *v > 0.0) {
            r.not_carried(name, shown(v), &format!("{fam}'s fuzz is a different model from glTF's sheen"));
        }
    }

    if let Some((weight, color, roughness, rough_default)) = names.sheen {
        if let Some(w) = r.weight(weight, "sheen").filter(|w| *w > 0.0) {
            let c = r.color(color).unwrap_or([1.0; 3]);
            if c.iter().all(|v| (0.0..=1.0).contains(v)) {
                let rough = r.unit(roughness, rough_default);
                let color = c.map(|v| (v * w) as f32);
                if color.iter().any(|v| *v > 0.0) {
                    m.sheen = Some(Sheen { color, roughness: rough.sqrt() as f32, ..Default::default() });
                }
            } else {
                r.notes.push(format!("{what} has {color} {stored} {}, outside 0 to 1; the sheen layer was left out", shown3(c)));
            }
        }
    }

    match names.film {
        Film::Thickness(thickness, ior) => {
            if let Some(t) = r.number(thickness).filter(|t| *t != 0.0) {
                if t > 0.0 {
                    let ior = r.film_ior(ior);
                    m.iridescence = Some(Iridescence { factor: 1.0, ior, thickness_max: t as f32, ..Default::default() });
                } else {
                    r.notes.push(format!("{what} has {thickness} {}, a negative thickness; the iridescence layer was left out", shown(t)));
                }
            }
        }
        Film::Weighted { weight, thickness, ior } => {
            if let Some(w) = r.weight(weight, "iridescence").filter(|w| *w > 0.0) {
                let microns = match r.number(thickness.0) {
                    Some(t) if t >= 0.0 => t,
                    Some(t) => {
                        r.notes.push(format!(
                            "{what} has {} {}, a negative thickness; it was read as {fam}'s default {}",
                            thickness.0,
                            shown(t),
                            shown(thickness.1)
                        ));
                        thickness.1
                    }
                    None => thickness.1,
                };
                let nanometres = (microns * 1000.0) as f32;
                if nanometres.is_finite() {
                    let ior = r.film_ior(ior);
                    m.iridescence = Some(Iridescence { factor: w as f32, ior, thickness_max: nanometres, ..Default::default() });
                } else {
                    r.notes.push(format!("{what} has {} {}, too thick to hold; the iridescence layer was left out", thickness.0, shown(microns)));
                }
            }
        }
        Film::None => {}
    }
    r.notes
}

/// The global axis-and-unit conversion, as a matrix taking file space to
/// the scene's Y-up metres.
fn axis_unit_matrix(props: &Props<'_, '_>) -> Result<Mat4, MeshError> {
    let axis = |name: &str, default: i64| props.i64(name).unwrap_or(default);
    let (up, front, coord) = (axis("UpAxis", 1), axis("FrontAxis", 2), axis("CoordAxis", 0));
    let (us, fs, cs) = (axis("UpAxisSign", 1), axis("FrontAxisSign", 1), axis("CoordAxisSign", 1));
    let mut seen = [false; 3];
    for a in [up, front, coord] {
        match usize::try_from(a).ok().filter(|a| *a < 3) {
            Some(a) if !seen[a] => seen[a] = true,
            _ => {
                return Err(malformed(format!(
                    "GlobalSettings axes (up {up}, front {front}, coord {coord}) are not a permutation of X, Y, Z"
                )))
            }
        }
    }
    for s in [us, fs, cs] {
        if s != 1 && s != -1 {
            return Err(malformed(format!("GlobalSettings axis sign {s} is not +1 or -1")));
        }
    }
    let unit = props.f64("UnitScaleFactor").unwrap_or(1.0);
    if !(unit.is_finite() && unit > 0.0) {
        return Err(malformed(format!("UnitScaleFactor {unit} is not a positive number")));
    }
    // UnitScaleFactor is centimetres per file unit; the scene is in metres.
    let k = unit / 100.0;
    // Row r of the 3x3 is the file direction that becomes scene axis r: the
    // coord axis becomes +X, up +Y, front +Z.
    let mut m = math::IDENTITY;
    for c in 0..3 {
        for r in 0..3 {
            m[c * 4 + r] = 0.0;
        }
    }
    m[coord as usize * 4] = cs as f64 * k;
    m[up as usize * 4 + 1] = us as f64 * k;
    m[front as usize * 4 + 2] = fs as f64 * k;
    Ok(m)
}

fn is_identity(m: &Mat4) -> bool {
    m.iter().zip(math::IDENTITY.iter()).all(|(a, b)| (a - b).abs() < 1e-12)
}

fn mat_bits(m: &Mat4) -> [u64; 16] {
    let mut out = [0u64; 16];
    for (o, v) in out.iter_mut().zip(m) {
        *o = v.to_bits();
    }
    out
}

/// Decode the base64 of an ASCII `Content` property.
fn decode_base64(s: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in s {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' | b'"' | b',' => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

// ─── lights and cameras ─────────────────────────────────────────────────────

/// A light's stored `Color` component in the scene's linear terms (Ruling
/// R14): as it is when the caller reads FBX colours as linear, and else
/// decoded by the sRGB curve without clamping, as three.js decodes it
/// (`SRGBToLinear`, ColorManagement.js L205-L209 at r183), so that a colour
/// past 1 keeps the brightness Ruling R8 moves into the intensity, and a
/// negative one stays negative for `Light::check` to drop. Within 0 to 1 it
/// is `math::srgb_to_linear`'s curve.
fn light_color(stored: f64, space: FbxColorSpace) -> f64 {
    match space {
        FbxColorSpace::Linear => stored,
        FbxColorSpace::Srgb if stored <= 0.04045 => stored / 12.92,
        FbxColorSpace::Srgb => ((stored + 0.055) / 1.055).powf(2.4),
    }
}

/// A light attribute's properties (`unit` metres per file unit; `space`, how
/// its colour is stored; `units`, what its `Intensity` is in; `label` names it
/// in a warning) as the scene's light, with what reading it warns of, or why
/// it is dropped. ufbx's reading (ufbx.c L23047-L23065, v0.23.1) and its
/// defaults, in the scene's units (see the module doc).
fn read_light(
    props: Props<'_, '_>,
    name: Option<String>,
    unit: f64,
    space: FbxColorSpace,
    units: LightUnits,
    label: &str,
) -> Result<(Light, Vec<String>), String> {
    let mut notes = Vec::new();
    let raw = props.i64("LightType").unwrap_or(0);
    let kind = match raw {
        0 => LightKind::Point,
        1 => LightKind::Directional,
        2 => {
            // Full cones in degrees: halved, and in radians.
            let degrees = |names: &[&str]| names.iter().find_map(|n| props.f64(n)).unwrap_or(0.0);
            let (inner, outer) = (degrees(&["InnerAngle", "HotSpot"]), degrees(&["OuterAngle", "ConeAngle", "Cone angle"]));
            let outer_cone = (outer / 2.0).to_radians() as f32;
            if outer_cone.is_finite() && outer_cone > std::f32::consts::FRAC_PI_2 {
                // Ruling R32: wider than a hemisphere, it lights nearly
                // every way, so it is a point light, not dropped.
                notes.push(format!("{label} has a cone of {}° across, wider than a hemisphere (180°), so it was read as a point light", shown(outer)));
                LightKind::Point
            } else {
                LightKind::Spot { inner_cone: (inner / 2.0).to_radians() as f32, outer_cone }
            }
        }
        3 | 4 => {
            let what = if raw == 3 { "an area light" } else { "a volume light" };
            return Err(format!("it is {what} (LightType {raw}), and the scene holds point, directional and spot lights only"));
        }
        _ => {
            // Outside FBX's enum: ufbx's default (`ufbxi_find_enum`, ufbx.c
            // L11547-L11557).
            notes.push(format!("{label} has LightType {raw}, which FBX does not define; it was read as a point light, as ufbx reads it"));
            LightKind::Point
        }
    };
    let mut color = props.vec3("Color").unwrap_or([1.0; 3]).map(|c| light_color(c, space));
    let mut intensity = props.f64("Intensity").unwrap_or(100.0) / 100.0;
    // Ruling R8: a colour past 1 holds part of the brightness.
    let peak = color.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if peak > 1.0 && peak.is_finite() {
        color = color.map(|c| c / peak);
        intensity *= peak;
    }
    if units == LightUnits::Blender {
        // `Intensity` / 100 is Blender's `energy` (above), so the scene's
        // candela or lux is what Blender's glTF importer turns into it.
        // Blender's exporter bakes 2^exposure into `Intensity`, and its
        // importer reads no exposure; neither is read here. A candela or lux
        // past f32 is not clamped: it becomes inf below, and `Light::check`
        // drops the light with the warning `Photometric` units give.
        intensity = from_blender_energy(&kind, intensity);
    }
    let far = props.i64("EnableFarAttenuation").unwrap_or(0) != 0;
    let mut range = props.f64("FarAttenuationEnd").filter(|_| far).map(|r| r * unit);
    if kind == LightKind::Directional {
        if let Some(r) = range.take() {
            notes.push(format!("{label} is directional, so its range {} is ignored", shown(r)));
        }
    } else if let Some(d) = props.i64("DecayType").filter(|d| *d != 2) {
        let how = match d {
            0 => "no decay",
            1 => "linear decay",
            3 => "cubic decay",
            _ => "a decay FBX does not define",
        };
        notes.push(format!("{label} has DecayType {d} ({how}), but the scene's point and spot lights fall off with the inverse square, as glTF's do"));
    }
    let color = color.map(|c| c as f32);
    let mut light = Light { name, kind, color, intensity: intensity as f32, range: range.map(|r| r as f32) };
    if let Some(how) = light.soften_hard_edge() {
        notes.push(format!("{label} {how}"));
    }
    light.check()?;
    Ok((light, notes))
}

/// A camera attribute's properties as the scene's camera, with what reading
/// it warns of, or why it is dropped: ufbx's reading (ufbx.c
/// L23087-L23253, v0.23.1) and its defaults, in the scene's units. `blender`
/// says Blender's exporter wrote the file ([`blender_exporter`]).
fn read_camera(props: Props<'_, '_>, name: Option<String>, unit: f64, blender: bool, label: &str) -> Result<(Camera, Vec<String>), String> {
    let mut notes = Vec::new();
    let gate = Gate::of(&props);
    let near = (props.f64("NearPlane").unwrap_or(0.0) * unit) as f32;
    let far = (props.f64("FarPlane").unwrap_or(0.0) * unit) as f32;
    let raw = props.i64("CameraProjectionType").unwrap_or(0);
    let projection = if raw == 1 {
        // 30 file units per `OrthoZoom` (ufbx.c L21356, L23109; ufbx's
        // `maya_ortho_camera_size` test: Maya's orthographic widths), or,
        // from Blender's exporter, `ortho_scale`, the extent in metres:
        // ufbx.c L21353-L21354 undoes every unit scale for it, and
        // export_fbx_bin.py L702 writes it unscaled where L704-L705 scale
        // the clip planes.
        let per_zoom = if blender { 1.0 } else { 30.0 * unit };
        let [width, height] = gate.ortho_size(per_zoom * props.f64("OrthoZoom").unwrap_or(1.0));
        Projection::Orthographic { xmag: (width / 2.0) as f32, ymag: (height / 2.0) as f32, znear: near, zfar: far }
    } else {
        if raw != 0 {
            // ufbx's default for a value outside the enum (ufbx.c L11547-L11557).
            notes.push(format!("{label} has CameraProjectionType {raw}, which FBX does not define; it was read as perspective, as ufbx reads it"));
        }
        let yfov = perspective_yfov(&props, &gate) as f32;
        Projection::Perspective { yfov, aspect: Some(gate.aspect as f32), znear: near, zfar: Some(far) }
    };
    let camera = Camera { name, projection };
    camera.check()?;
    Ok((camera, notes))
}

/// A camera's film gate and aspect ratio as ufbx works them out (ufbx.c
/// L23087-L23219, v0.23.1).
struct Gate {
    /// Width over height: `AspectWidth` over `AspectHeight` (the legacy
    /// `AspectW` and `AspectH` before them), as `AspectRatioMode` relates
    /// them, else the film's.
    aspect: f64,
    /// `FilmWidth` and `FilmHeight` in inches, else `ApertureFormat`'s, the
    /// height times `FilmSqueezeRatio`.
    film: [f64; 2],
    /// `GateFit` with fill and overscan settled: 0 none or stretch (the film
    /// as it is), 1 fitted to its height, 2 fitted across.
    fit: i64,
}

impl Gate {
    fn of(props: &Props<'_, '_>) -> Gate {
        // A value outside an enum takes its default, as ufbx's
        // `ufbxi_find_enum` does (ufbx.c L11547-L11557).
        let choice = |name: &str, default: i64, max: i64| props.i64(name).filter(|v| (0..=max).contains(v)).unwrap_or(default);
        // `ApertureFormat`'s film sizes, inches (ufbx.c L23072-L23085).
        const FORMATS: [[f64; 2]; 12] = [
            [1.0, 1.0],
            [0.404, 0.295],
            [0.493, 0.292],
            [0.864, 0.630],
            [0.816, 0.612],
            [0.980, 0.735],
            [0.825, 0.446],
            [0.864, 0.732],
            [2.066, 0.906],
            [1.485, 0.991],
            [2.080, 1.480],
            [2.772, 2.072],
        ];
        let format = choice("ApertureFormat", 0, 11) as usize;
        let mut film = [props.f64("FilmWidth").unwrap_or(FORMATS[format][0]), props.f64("FilmHeight").unwrap_or(FORMATS[format][1])];
        let squeeze = props.f64("FilmSqueezeRatio").unwrap_or(if format == 7 { 2.0 } else { 1.0 });
        let mut w = props.f64("AspectWidth").or_else(|| props.f64("AspectW")).unwrap_or(0.0);
        let mut h = props.f64("AspectHeight").or_else(|| props.f64("AspectH")).unwrap_or(0.0);
        let film_known = film[0] > 0.0 && film[1] > 0.0;
        if w <= 0.0 && h <= 0.0 {
            w = if film[0] > 0.0 { film[0] } else { 1.0 };
            h = if film[1] > 0.0 { film[1] } else { 1.0 };
        } else if w <= 0.0 {
            w = if film_known { h / film[1] * film[0] } else { h };
        } else if h <= 0.0 {
            h = if film_known { w / film[0] * film[1] } else { w };
        }
        film[1] *= squeeze;
        // Fixed width: the height is relative to the width; fixed height,
        // the other way round.
        let aspect = match choice("AspectRatioMode", 0, 4) {
            3 => 1.0 / h,
            4 => w,
            _ => w / h,
        };
        let film_ratio = film[0] / film[1];
        let fit = match choice("GateFit", 0, 5) {
            3 => 1 + i64::from(aspect > film_ratio),
            4 => 1 + i64::from(aspect < film_ratio),
            5 => 0,
            fit => fit,
        };
        Gate { aspect, film, fit }
    }

    /// The gate's width and height, in inches (ufbx.c L23179-L23219).
    fn aperture(&self) -> [f64; 2] {
        match self.fit {
            1 => [self.film[1] * self.aspect, self.film[1]],
            2 => [self.film[0], self.film[0] / self.aspect],
            _ => self.film,
        }
    }

    /// An orthographic view's full width and height, given its `extent`
    /// along the gate fit (ufbx.c L23179-L23219: ufbx's own test,
    /// `maya_ortho_camera_400x200`, has a view fitted across 30 wide and 15
    /// high at 2:1).
    fn ortho_size(&self, extent: f64) -> [f64; 2] {
        match self.fit {
            1 => [extent * self.aspect, extent],
            2 => [extent, extent / self.aspect],
            _ => [extent, extent],
        }
    }
}

/// A perspective camera's vertical field of view, in radians, as ufbx
/// reads it for each `ApertureMode` (ufbx.c L23221-L23248, v0.23.1): 0,
/// horizontal and vertical, `FieldOfViewY`; 1, horizontal, `FieldOfView`
/// across, so tan(yfov/2) = tan(FieldOfView/2) / aspect; 2, vertical (the
/// default), `FieldOfView`; 3, focal length, tan(yfov/2) = h /
/// (2·`FocalLength`), with h the gate's height in millimetres, 25.4 to the
/// inch (ufbx's `UFBXI_MM_TO_INCH`, ufbx.c L5896, rounds 1/25.4 to
/// 0.0393700787).
fn perspective_yfov(props: &Props<'_, '_>, gate: &Gate) -> f64 {
    let angle = |name: &str| props.f64(name).unwrap_or(0.0).to_radians();
    match props.i64("ApertureMode").filter(|m| (0..=3).contains(m)).unwrap_or(2) {
        0 => angle("FieldOfViewY"),
        1 => 2.0 * ((angle("FieldOfView") / 2.0).tan() / gate.aspect).atan(),
        3 => 2.0 * (gate.aperture()[1] * 25.4 / (2.0 * props.f64("FocalLength").unwrap_or(0.0))).atan(),
        _ => angle("FieldOfView"),
    }
}

/// Whether `creator` names Blender's FBX exporter, by ufbx's two patterns
/// for it (`ufbxi_match_exporter`, ufbx.c L12084-L12089, v0.23.1): "blender"
/// in any case, then past one '-' (two for "blender-- ?.?.?"), spaces and a
/// version of two (three) dot-separated numbers, as in "Blender (stable FBX
/// IO) - 5.2.3 - 5.13.0". Its `OrthoZoom` is Blender's own `ortho_scale`.
fn blender_exporter(creator: &str) -> bool {
    fn past_dash(s: &str) -> Option<&str> {
        s.find('-').map(|i| &s[i + 1..])
    }
    let Some(rest) = creator.get(..7).filter(|p| p.eq_ignore_ascii_case("blender")).map(|_| &creator[7..]) else { return false };
    let version = |s: &str, parts: usize| {
        let mut s = s.trim_start_matches([' ', '\t']);
        for i in 0..parts {
            if i > 0 {
                match s.strip_prefix('.') {
                    Some(r) => s = r,
                    None => return false,
                }
            }
            let digits = s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            if digits == 0 {
                return false;
            }
            s = &s[digits..];
        }
        true
    };
    past_dash(rest).and_then(past_dash).is_some_and(|s| version(s, 3)) || past_dash(rest).is_some_and(|s| version(s, 2))
}

/// Per control point skin influences, compressed-row: control point `cp`'s
/// influences are `joints[offsets[cp]..offsets[cp + 1]]`.
#[derive(Debug, Clone, Default)]
struct Influences {
    offsets: Vec<u32>,
    joints: Vec<u16>,
    weights: Vec<f32>,
    width: usize,
}

impl Influences {
    fn of(&self, cp: usize) -> (&[u16], &[f32]) {
        let (a, b) = (self.offsets[cp] as usize, self.offsets[cp + 1] as usize);
        (&self.joints[a..b], &self.weights[a..b])
    }
}

/// A shape's offsets and, when it has them, its normal offsets, dense over
/// control points.
type Offsets = (Vec<[f32; 3]>, Option<Vec<[f32; 3]>>);

/// One morph target, dense over control points.
struct Target {
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
}

/// One FBX blend-shape channel, and the scene targets its shapes became.
#[derive(Debug, Clone)]
struct BlendChannel {
    id: i64,
    /// Each shape's full weight (a fraction; ascending): the channel weight
    /// at which that shape alone is fully on. The shapes are the targets
    /// `first..first + full.len()`.
    full: Vec<f64>,
    first: usize,
    /// `DeformPercent` as a fraction.
    default: f64,
}

/// Each shape's share of a channel at weight `w` (a fraction), as ufbx
/// evaluates in-between shapes (`ufbxi_update_blend_channel`): linear from
/// the base (weight 0) to the first shape and between consecutive shapes,
/// extrapolated past the ends. With one shape at full weight 1 this is `w`.
fn inbetween_weights(full: &[f64], w: f64) -> Vec<f64> {
    let n = full.len();
    let mut out = vec![0.0; n];
    // `None` is the base mesh, a shape of weight 0.
    let last_negative = full.iter().rposition(|t| *t < 0.0);
    let (mut prev, mut next): (Option<usize>, Option<usize>) = (None, None);
    if w > 0.0 {
        let from = last_negative.map_or(0, |i| i + 1);
        if let Some(i) = last_negative {
            prev = Some(i);
        }
        for (i, t) in full.iter().enumerate().skip(from) {
            prev = next;
            next = Some(i);
            if *t > w {
                break;
            }
        }
    } else {
        let from = last_negative.map_or(0, |i| i + 1);
        if from < n {
            prev = Some(from);
        }
        for (i, t) in full.iter().enumerate().take(from).rev() {
            prev = next;
            next = Some(i);
            if *t < w {
                break;
            }
        }
    }
    let at = |k: Option<usize>| k.map_or(0.0, |k| full[k]);
    let delta = at(next) - at(prev);
    if delta != 0.0 {
        let t = (w - at(prev)) / delta;
        if let Some(k) = prev {
            out[k] = 1.0 - t;
        }
        if let Some(k) = next {
            out[k] = t;
        }
    }
    out
}

struct Blend {
    channels: Vec<BlendChannel>,
    /// Per scene target.
    names: Vec<String>,
    defaults: Vec<f32>,
    targets: Vec<Target>,
}

/// What a geometry's corners carry, decoded once.
struct Layers {
    normal: Option<Layer>,
    tangent: Option<Layer>,
    binormal: Option<Layer>,
    uvs: Vec<Layer>,
    color: Option<Layer>,
}

/// A corner's identity: control point plus every attribute, bit for bit.
type CornerKey = [u32; 28];

#[derive(Default)]
struct PrimBuild {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    tangents: Vec<[f32; 4]>,
    uvs: Vec<Vec<[f32; 2]>>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
    cps: Vec<u32>,
    map: HashMap<CornerKey, u32>,
}

struct Reader<'d, 'a> {
    dom: &'d Dom<'a>,
    sidecars: &'d dyn Sidecars,
    /// How classic material colours and light colours are stored.
    colors: FbxColorSpace,
    /// The units light strengths are stored in.
    light_units: LightUnits,
    warnings: Vec<String>,
    warned: HashSet<String>,
    objects: HashMap<i64, Obj>,
    order: Vec<i64>,
    children: HashMap<i64, Vec<Edge>>,
    parents: HashMap<i64, Vec<Edge>>,
    templates: HashMap<String, PMap<'a>>,
    /// Each `PropertyTemplate` by its class (`FbxLight`, `FbxCamera`): the
    /// defaults of a light or camera attribute, which `templates` merges
    /// with every other `NodeAttribute` class's.
    class_templates: HashMap<String, PMap<'a>>,
    creator: String,
    tick_rate: f64,
    correction: Mat4,
    /// Metres per file unit, `UnitScaleFactor / 100`: the scale
    /// `correction` holds, for the lengths a light or camera states.
    unit: f64,
    /// Per light and camera attribute: its scene index, or `None` when it
    /// was dropped.
    light_map: HashMap<i64, Option<usize>>,
    camera_map: HashMap<i64, Option<usize>>,
    scene: Scene,
    node_of: HashMap<i64, usize>,
    model_of_node: Vec<i64>,
    model_parent: HashMap<i64, i64>,
    xf: HashMap<i64, Xf>,
    world: HashMap<i64, Mat4>,
    local: HashMap<i64, Mat4>,
    complex: HashSet<i64>,
    material_map: HashMap<i64, Option<usize>>,
    texture_map: HashMap<i64, Option<usize>>,
    image_map: HashMap<i64, Option<usize>>,
    image_bytes: usize,
    uv_names: HashMap<String, u32>,
    skin_map: HashMap<i64, Option<(usize, Influences)>>,
    blend_map: HashMap<i64, Option<std::rc::Rc<Blend>>>,
    mesh_map: HashMap<(i64, Vec<i64>, [u64; 16]), Option<usize>>,
    mesh_channels: Vec<(usize, Vec<BlendChannel>)>,
    curve_cache: HashMap<i64, Option<Curve>>,
    /// The first BindPose, model id → global matrix; read once, on demand.
    bind: Option<HashMap<i64, Mat4>>,
    total_vertices: usize,
    total_indices: usize,
    morph_values: usize,
    anim_keys: usize,
    chain_work: usize,
    non_mesh_geometry: usize,
}

/// Read a parsed document into a scene.
pub(super) fn read(dom: &Dom<'_>, sidecars: &dyn Sidecars, colors: FbxColorSpace, light_units: LightUnits) -> Result<SceneRead, MeshError> {
    let mut r = Reader::new(dom, sidecars);
    r.colors = colors;
    r.light_units = light_units;
    r.header()?;
    r.index()?;
    r.globals()?;
    r.models()?;
    r.geometry()?;
    r.animation()?;
    // After the animation: the frames of light and camera nodes turn at
    // every key as well as at rest.
    r.lights_and_cameras();
    r.finish()
}

impl<'d, 'a> Reader<'d, 'a> {
    fn new(dom: &'d Dom<'a>, sidecars: &'d dyn Sidecars) -> Self {
        Reader {
            dom,
            sidecars,
            colors: FbxColorSpace::default(),
            light_units: LightUnits::default(),
            warnings: Vec::new(),
            warned: HashSet::new(),
            objects: HashMap::new(),
            order: Vec::new(),
            children: HashMap::new(),
            parents: HashMap::new(),
            templates: HashMap::new(),
            class_templates: HashMap::new(),
            creator: String::new(),
            tick_rate: TICKS_PER_SECOND,
            correction: math::IDENTITY,
            unit: 1.0,
            light_map: HashMap::new(),
            camera_map: HashMap::new(),
            scene: Scene::default(),
            node_of: HashMap::new(),
            model_of_node: Vec::new(),
            model_parent: HashMap::new(),
            xf: HashMap::new(),
            world: HashMap::new(),
            local: HashMap::new(),
            complex: HashSet::new(),
            material_map: HashMap::new(),
            texture_map: HashMap::new(),
            image_map: HashMap::new(),
            image_bytes: 0,
            uv_names: HashMap::new(),
            skin_map: HashMap::new(),
            blend_map: HashMap::new(),
            mesh_map: HashMap::new(),
            mesh_channels: Vec::new(),
            curve_cache: HashMap::new(),
            bind: None,
            total_vertices: 0,
            total_indices: 0,
            morph_values: 0,
            anim_keys: 0,
            chain_work: 0,
            non_mesh_geometry: 0,
        }
    }

    fn warn(&mut self, key: &str, msg: String) {
        if self.warned.insert(key.to_string()) {
            self.warnings.push(msg);
        }
    }

    fn obj(&self, id: i64) -> Option<&Obj> {
        self.objects.get(&id)
    }

    fn kind(&self, id: i64) -> &str {
        self.objects.get(&id).map_or("", |o| o.kind.as_str())
    }

    fn sub(&self, id: i64) -> &str {
        self.objects.get(&id).map_or("", |o| o.sub.as_str())
    }

    fn name_of(&self, id: i64) -> String {
        self.objects.get(&id).map_or(String::new(), |o| o.name.clone())
    }

    fn kids(&self, id: i64) -> &[Edge] {
        self.children.get(&id).map_or(&[], |v| v.as_slice())
    }

    fn ups(&self, id: i64) -> &[Edge] {
        self.parents.get(&id).map_or(&[], |v| v.as_slice())
    }

    /// Objects of `kind` (and, if given, `sub`) connected object-to-object
    /// to `id`, in connection order.
    fn kids_of(&self, id: i64, kind: &str, sub: Option<&str>) -> Vec<i64> {
        self.kids(id)
            .iter()
            .filter(|(c, p)| p.is_none() && self.kind(*c) == kind && sub.is_none_or(|s| self.sub(*c) == s))
            .map(|(c, _)| *c)
            .collect()
    }

    fn p70(&self, id: i64) -> Result<PMap<'a>, MeshError> {
        match self.obj(id) {
            Some(o) => read_p70(self.dom, o.rec),
            None => Ok(HashMap::new()),
        }
    }

    fn props<'m>(&'m self, own: &'m PMap<'a>, kind: &str) -> Props<'m, 'a> {
        Props { own, tmpl: self.templates.get(kind) }
    }

    fn rec(&self, id: i64) -> Option<RecId> {
        self.obj(id).map(|o| o.rec)
    }

    fn array(&self, id: i64, name: &str) -> Result<Option<ArrayRef<'a>>, MeshError> {
        match self.rec(id) {
            Some(r) => self.dom.child_array(r, name),
            None => Ok(None),
        }
    }

    // ── document ────────────────────────────────────────────────────────────

    fn header(&mut self) -> Result<(), MeshError> {
        let dom = self.dom;
        let hdr = dom.child(ROOT, "FBXHeaderExtension");
        let int = |r: Option<RecId>, name: &str| -> Result<Option<i64>, MeshError> {
            match r {
                Some(r) => Ok(dom.child_prop(r, name)?.and_then(|p| p.as_i64())),
                None => Ok(None),
            }
        };
        if int(hdr, "EncryptionType")?.is_some_and(|e| e != 0) {
            return Err(unsupported("the file is encrypted"));
        }
        // SDK 2019.5's opt-in time base: FBXHeaderVersion 1004+ with a
        // TCDefinition other than 127 (the legacy value Mixamo writes).
        let version = int(hdr, "FBXHeaderVersion")?.unwrap_or(0);
        let flags = hdr.and_then(|h| dom.child(h, "OtherFlags"));
        let tc = int(flags, "TCDefinition")?;
        if version >= 1004 && tc.is_some_and(|t| t != 127) {
            self.tick_rate = TICKS_PER_SECOND_2019;
        }
        let creator = match dom.child_prop(ROOT, "Creator")? {
            Some(p) => p.as_str().map(|s| s.into_owned()),
            None => match hdr {
                Some(h) => dom.child_prop(h, "Creator")?.and_then(|p| p.as_str().map(|s| s.into_owned())),
                None => None,
            },
        };
        self.creator = creator.unwrap_or_default();
        Ok(())
    }

    fn index(&mut self) -> Result<(), MeshError> {
        let dom = self.dom;
        let Some(objects) = dom.child(ROOT, "Objects") else {
            return Err(malformed("the document has no Objects section"));
        };
        for rec in dom.children(objects) {
            if self.order.len() >= MAX_OBJECTS {
                return Err(unsupported(format!("more than {MAX_OBJECTS} objects")));
            }
            let props = dom.props(rec)?;
            let kind = dom.name(rec);
            let Some(id) = props.first().and_then(|p| p.as_i64()) else {
                return Err(malformed(format!("a {kind} object has no id")));
            };
            if id == 0 {
                return Err(malformed(format!("a {kind} object uses id 0, which is the scene root's")));
            }
            let name = props
                .get(1)
                .and_then(|p| p.as_bytes())
                .map(|b| split_object_name(b, dom.encoding).0)
                .unwrap_or_default();
            let sub = props.get(2).and_then(|p| p.as_str()).map(|s| s.into_owned()).unwrap_or_default();
            if self.objects.insert(id, Obj { rec, kind: kind.to_string(), name, sub }).is_some() {
                return Err(malformed(format!("two objects share id {id}")));
            }
            self.order.push(id);
        }
        let mut ignored = 0usize;
        if let Some(conns) = dom.child(ROOT, "Connections") {
            for c in dom.children_named(conns, "C") {
                let props = dom.props(c)?;
                let kind = props.first().and_then(|p| p.as_str()).map(|s| s.into_owned()).unwrap_or_default();
                let (Some(child), Some(parent)) = (props.get(1).and_then(|p| p.as_i64()), props.get(2).and_then(|p| p.as_i64()))
                else {
                    return Err(malformed("a connection does not name two objects"));
                };
                let prop = match kind.as_str() {
                    "OO" => None,
                    "OP" => Some(props.get(3).and_then(|p| p.as_str()).map(|s| s.into_owned()).unwrap_or_default()),
                    // Property-to-object and property-to-property links carry
                    // nothing the scene models (constraints, expressions).
                    "PO" | "PP" => {
                        ignored += 1;
                        continue;
                    }
                    other => return Err(malformed(format!("connection type {other:?} is not OO, OP, PO or PP"))),
                };
                if !self.objects.contains_key(&child) || (parent != 0 && !self.objects.contains_key(&parent)) {
                    // Links to objects that are not in the file are common in
                    // trimmed exports and mean nothing.
                    continue;
                }
                self.children.entry(parent).or_default().push((child, prop.clone()));
                self.parents.entry(child).or_default().push((parent, prop));
            }
        }
        if ignored > 0 {
            self.warn("pp", format!("{ignored} property-to-property connection(s) (constraints, expressions) were not carried"));
        }
        if let Some(defs) = dom.child(ROOT, "Definitions") {
            for ot in dom.children_named(defs, "ObjectType") {
                let Some(ty) = dom.first_prop(ot)?.and_then(|p| p.as_str().map(|s| s.into_owned())) else { continue };
                for pt in dom.children_named(ot, "PropertyTemplate") {
                    let m = read_p70(dom, pt)?;
                    if let Some(class) = dom.first_prop(pt)?.and_then(|p| p.as_str().map(|s| s.into_owned())) {
                        // Files before 7.4 spell the classes "KFbx…"
                        // (Blender's importer strips the K, import_fbx.py
                        // L3238-L3244).
                        let class = class.strip_prefix('K').filter(|c| c.starts_with("Fbx")).map_or(class.clone(), str::to_string);
                        self.class_templates.entry(class).or_insert_with(|| m.clone());
                    }
                    let entry = self.templates.entry(ty.clone()).or_default();
                    for (k, v) in m {
                        entry.entry(k).or_insert(v);
                    }
                }
            }
        }
        Ok(())
    }

    fn globals(&mut self) -> Result<(), MeshError> {
        let gs = match self.dom.child(ROOT, "GlobalSettings") {
            Some(g) => read_p70(self.dom, g)?,
            None => HashMap::new(),
        };
        let props = self.props(&gs, "GlobalSettings");
        let m = axis_unit_matrix(&props)?;
        // `axis_unit_matrix` has checked it is a positive number.
        let unit = props.f64("UnitScaleFactor").unwrap_or(1.0) / 100.0;
        // Ruling R15: the scene has no ambient light (the spec's
        // non-goals), so a file's non-black `AmbientColor` is named; black
        // is the usual default, and what this writer writes.
        let ambient = props.vec3("AmbientColor").filter(|c| c.iter().any(|v| *v != 0.0));
        self.unit = unit;
        self.correction = m;
        if let Some(c) = ambient {
            self.warn(
                "ambient",
                format!("the file's ambient light, AmbientColor {} in its GlobalSettings, is not carried: the scene holds directional, point and spot lights only", shown3(c)),
            );
        }
        Ok(())
    }

    // ── models ──────────────────────────────────────────────────────────────

    fn models(&mut self) -> Result<(), MeshError> {
        let models: Vec<i64> = self.order.iter().copied().filter(|id| self.kind(*id) == "Model").collect();
        for &m in &models {
            let parent = self
                .ups(m)
                .iter()
                .find(|(p, prop)| prop.is_none() && *p != 0 && self.kind(*p) == "Model")
                .map(|(p, _)| *p);
            if let Some(p) = parent {
                self.model_parent.insert(m, p);
            }
        }
        // Roots in the order they were connected to the scene root, then any
        // left unconnected, in file order.
        let mut roots: Vec<i64> = self
            .kids(0)
            .iter()
            .filter(|(c, p)| p.is_none() && self.kind(*c) == "Model" && !self.model_parent.contains_key(c))
            .map(|(c, _)| *c)
            .collect();
        let mut seen_root: HashSet<i64> = roots.iter().copied().collect();
        for &m in &models {
            if !self.model_parent.contains_key(&m) && seen_root.insert(m) {
                roots.push(m);
            }
        }
        // Depth-first, children in connection order, with an explicit stack:
        // hierarchy depth is the file author's choice.
        let mut order: Vec<i64> = Vec::new();
        let mut visited: HashSet<i64> = HashSet::new();
        let mut stack: Vec<i64> = roots.iter().rev().copied().collect();
        while let Some(m) = stack.pop() {
            if !visited.insert(m) {
                continue;
            }
            if order.len() >= MAX_NODES {
                return Err(unsupported(format!("the scene has more than {MAX_NODES} models")));
            }
            order.push(m);
            let kids: Vec<i64> = self
                .kids(m)
                .iter()
                .filter(|(c, p)| p.is_none() && self.model_parent.get(c) == Some(&m))
                .map(|(c, _)| *c)
                .collect();
            stack.extend(kids.into_iter().rev());
        }
        if order.len() != models.len() {
            return Err(malformed(format!(
                "{} model(s) are not reachable from the scene root: the model hierarchy has a cycle",
                models.len() - order.len()
            )));
        }
        // Camera switchers and stereo rigs; plain cameras and lights are
        // `lights_and_cameras`'.
        let mut rigs = 0usize;
        for &m in &order {
            let pm = self.p70(m)?;
            let xf = self.read_xf(m, &pm)?;
            let local = xf.local(xf.t, xf.r, xf.s);
            let parent = self.model_parent.get(&m).copied();
            let (pw, pl) = match parent {
                Some(p) => (self.world[&p], self.local[&p]),
                None => (math::IDENTITY, math::IDENTITY),
            };
            let world = fbx_world(&pw, &pl, &local, xf.inherit);
            // Inheritance that differs from plain composition makes the IR
            // local the parent-relative world rather than the FBX local.
            let inherit_matters = xf.inherit != 1 && parent.is_some() && {
                let p_rs = without_translation(&pw);
                let p_r = rotation_part(&p_rs);
                let p_s = math::mat_mul(&math::invert(&p_r).unwrap_or_else(|| math::transpose(&p_r)), &p_rs);
                let uniform = is_scaled_identity(&p_s);
                let local_unit = {
                    let len = |c: usize| (pl[c * 4] * pl[c * 4] + pl[c * 4 + 1] * pl[c * 4 + 1] + pl[c * 4 + 2] * pl[c * 4 + 2]).sqrt();
                    (0..3).all(|c| (len(c) - 1.0).abs() < 1e-9)
                };
                !(uniform && (xf.inherit == 0 || local_unit))
            };
            let transform = if inherit_matters {
                let inv = math::invert(&pw).ok_or_else(|| malformed(format!("model {:?}'s parent has a singular transform", self.name_of(m))))?;
                Transform::from_matrix(math::mat_mul(&inv, &world))
            } else if xf.has_pivots() {
                Transform::from_matrix(local)
            } else {
                Transform::Trs { translation: xf.t, rotation: xf.rotation(xf.r), scale: xf.s }
            };
            if inherit_matters || xf.has_pivots() {
                self.complex.insert(m);
            }
            if matches!(self.sub(m), "CameraSwitcher" | "CameraStereo") {
                rigs += 1;
            }
            let ir = self.scene.nodes.len();
            self.scene.nodes.push(Node { name: Some(self.name_of(m)).filter(|n| !n.is_empty()), transform, ..Default::default() });
            self.model_of_node.push(m);
            self.node_of.insert(m, ir);
            match parent {
                Some(p) => {
                    let pi = self.node_of[&p];
                    self.scene.nodes[pi].children.push(ir);
                }
                None => self.scene.roots.push(ir),
            }
            self.world.insert(m, world);
            self.local.insert(m, local);
            self.xf.insert(m, xf);
        }
        if rigs > 0 {
            self.warn("camrigs", format!("{rigs} camera switcher(s) and stereo camera rig(s) were read as empty nodes; the scene has neither"));
        }
        Ok(())
    }

    fn read_xf(&mut self, m: i64, pm: &PMap<'a>) -> Result<Xf, MeshError> {
        let props = self.props(pm, "Model");
        let what = format!("model {:?}", self.name_of(m));
        let v = |name: &str, d: [f64; 3]| props.vec3_or(name, d, &what);
        let (t, r, s) = (v("Lcl Translation", [0.0; 3])?, v("Lcl Rotation", [0.0; 3])?, v("Lcl Scaling", [1.0; 3])?);
        let (roff, rp, soff, sp) = (v("RotationOffset", [0.0; 3])?, v("RotationPivot", [0.0; 3])?, v("ScalingOffset", [0.0; 3])?, v("ScalingPivot", [0.0; 3])?);
        let (mut pre, mut post) = (v("PreRotation", [0.0; 3])?, v("PostRotation", [0.0; 3])?);
        let (gt, gr, gs) = (v("GeometricTranslation", [0.0; 3])?, v("GeometricRotation", [0.0; 3])?, v("GeometricScaling", [1.0; 3])?);
        // The SDK's rule, which ufbx (Godot, Blender 4.5) follows: with
        // RotationActive off, rotation order and pre/post rotation are
        // ignored and Lcl Rotation is XYZ. Stated nowhere (no property, no
        // template) it is on, as ufbx reads it (ufbx.c:22964 at 5955c5c,
        // `ufbxi_find_int(&node->props, ufbxi_RotationActive, 1)`); the
        // SDK's own templates say 0.
        let active = props.i64("RotationActive").unwrap_or(1) != 0 && props.i64("RotationSpaceForLimitOnly").unwrap_or(0) == 0;
        let raw_order = props.i64("RotationOrder").unwrap_or(0);
        let inherit_raw = props.i64("InheritType").unwrap_or(0);
        let order = if active {
            match euler_order(raw_order) {
                Some(o) => o,
                None => {
                    self.warn("spheric", format!("{what} uses rotation order {raw_order} (spheric XYZ), read as XYZ"));
                    EulerOrder::Xyz
                }
            }
        } else {
            pre = [0.0; 3];
            post = [0.0; 3];
            EulerOrder::Xyz
        };
        let inherit = if (0..=2).contains(&inherit_raw) {
            inherit_raw
        } else {
            self.warn("inherit", format!("{what} has InheritType {inherit_raw}, read as 1 (ordinary composition)"));
            1
        };
        let geo = math::mat_mul(
            &math::translation(gt),
            &math::mat_mul(&math::quat_to_mat(math::euler_to_quat(rad(gr), EulerOrder::Xyz)), &math::scaling(gs)),
        );
        Ok(Xf { t, r, s, roff, rp, soff, sp, pre, post, order, inherit, geo })
    }

    // ── geometry ────────────────────────────────────────────────────────────

    fn geometry(&mut self) -> Result<(), MeshError> {
        // UV set names, for textures that name the set they sample.
        let geoms: Vec<i64> = self.order.iter().copied().filter(|g| self.kind(*g) == "Geometry").collect();
        for g in geoms {
            let Some(rec) = self.rec(g) else { continue };
            for el in self.dom.children_named(rec, "LayerElementUV") {
                let idx = self.dom.first_prop(el)?.and_then(|p| p.as_i64()).unwrap_or(0);
                if let Some(name) = self.dom.child_prop(el, "Name")?.and_then(|p| p.as_str().map(|s| s.into_owned())) {
                    if let Ok(i) = u32::try_from(idx) {
                        self.uv_names.entry(name).or_insert(i);
                    }
                }
            }
        }
        for ni in 0..self.model_of_node.len() {
            let m = self.model_of_node[ni];
            let geoms = self.kids_of(m, "Geometry", None);
            // The geometries that draw: polygon meshes and `Line`s.
            let mut drawn: Vec<i64> = Vec::new();
            for g in geoms {
                let sub = self.sub(g).to_string();
                match sub.as_str() {
                    "Mesh" | "Line" => drawn.push(g),
                    "Shape" => {}
                    other => {
                        self.non_mesh_geometry += 1;
                        let what = format!("{other:?} geometry (NURBS, patches and curves)");
                        self.warn("nonmesh", format!("{what} is not carried: the scene models polygon meshes and line segments only"));
                    }
                }
            }
            let Some(&g) = drawn.first() else { continue };
            if drawn.len() > 1 {
                self.warn("multigeo", format!("model {:?} has {} geometries; only the first was read", self.name_of(m), drawn.len()));
            }
            let materials = self.kids_of(m, "Material", None);
            let geo = self.xf.get(&m).map_or(math::IDENTITY, |x| x.geo);
            let Some(mi) = self.mesh_for(g, &materials, &geo)? else { continue };
            self.scene.nodes[ni].mesh = Some(mi);
            if let Some(Some((si, _))) = self.skin_map.get(&g) {
                if self.scene.meshes[mi].primitives.iter().any(|p| p.skin.is_some()) {
                    self.scene.nodes[ni].skin = Some(*si);
                }
            }
        }
        Ok(())
    }

    fn layer(&mut self, el: RecId, data: &str, index_names: &[&str], stride: usize, what: &str) -> Result<Option<Layer>, MeshError> {
        let dom = self.dom;
        let s = |name: &str| -> Result<String, MeshError> {
            Ok(dom.child_prop(el, name)?.and_then(|p| p.as_str().map(|s| s.into_owned())).unwrap_or_default())
        };
        let mapping_s = s("MappingInformationType")?;
        let reference_s = s("ReferenceInformationType")?;
        let mapping = match mapping_s.as_str() {
            "ByPolygonVertex" => Mapping::PolygonVertex,
            // "ByVertice" is what every writer spells; the others are its
            // synonyms in the SDK's enum.
            "ByVertice" | "ByVertex" | "ByControlPoint" => Mapping::Vertex,
            "ByPolygon" => Mapping::Polygon,
            "AllSame" => Mapping::AllSame,
            "NoMappingInformation" | "" => return Ok(None),
            other => {
                self.warn(&format!("map-{what}-{other}"), format!("{what} mapped {other:?} is not carried"));
                return Ok(None);
            }
        };
        let Some(arr) = dom.child_array(el, data)? else { return Ok(None) };
        let mut values = dom.f64s(&arr, what)?;
        if values.len() % stride != 0 {
            return Err(malformed(format!("{what} has {} values, not a multiple of {stride}", values.len())));
        }
        // Zero for a direction or a coordinate; for a colour its neutral,
        // opaque white, so the corner is not made transparent.
        let fill = if data == "Colors" { 1.0 } else { 0.0 };
        let mut bad = 0usize;
        for e in values.chunks_exact_mut(stride) {
            if e.iter().any(|v| !v.is_finite()) {
                e.fill(fill);
                bad += 1;
            }
        }
        if bad > 0 {
            let read_as = if data == "Colors" { "opaque white" } else { "zero" };
            self.warn(&format!("nonfinite-{what}"), format!("{what} has {bad} non-finite element(s); each was read as {read_as}"));
        }
        let index = match reference_s.as_str() {
            "Direct" => None,
            "IndexToDirect" | "Index" => {
                let mut found = None;
                for n in index_names {
                    if let Some(a) = dom.child_array(el, n)? {
                        found = Some(dom.i32s(&a, what)?);
                        break;
                    }
                }
                Some(found.ok_or_else(|| malformed(format!("{what} is IndexToDirect but has no index array")))?)
            }
            other => return Err(malformed(format!("{what} reference type {other:?} is not Direct or IndexToDirect"))),
        };
        Ok(Some(Layer { mapping, values, index, stride }))
    }

    /// The IR mesh for geometry `g` as seen through a model with `materials`
    /// and geometric transform `geo`.
    fn mesh_for(&mut self, g: i64, materials: &[i64], geo: &Mat4) -> Result<Option<usize>, MeshError> {
        let key = (g, materials.to_vec(), mat_bits(geo));
        if let Some(m) = self.mesh_map.get(&key) {
            return Ok(*m);
        }
        let built = if self.sub(g) == "Line" { self.build_lines(g, materials, geo)? } else { self.build_mesh(g, materials, geo)? };
        let out = built.map(|(mesh, channels)| {
            self.scene.meshes.push(mesh);
            let i = self.scene.meshes.len() - 1;
            if !channels.is_empty() {
                self.mesh_channels.push((i, channels));
            }
            i
        });
        self.mesh_map.insert(key, out);
        Ok(out)
    }

    /// One Geometry as an IR mesh: control points, polygons (a negative
    /// `PolygonVertexIndex` closes one), and every LayerElement by its mapping
    /// (`ByControlPoint`, `ByPolygonVertex`, `ByPolygon`, `AllSame`) and reference
    /// (`Direct`, `IndexToDirect`). FBX has no public specification: the SDK's
    /// FbxLayerElement (https://help.autodesk.com/cloudhelp/2020/ENU/FBX-Developer-Help/files/cpp_ref/FBX_Developer_Help_cpp_ref_class_fbx_layer_element_html.html),
    /// ufbx v0.23.1 `ufbxi_read_mesh` and `ufbxi_read_vertex_element`
    /// (https://github.com/ufbx/ufbx/blob/26a482ae66871d7de36eb722aa060bce95bce274/ufbx.c#L13430-L13807,
    /// #L12737-L12922), and Blender v5.2.3's `blen_read_geom`.
    fn build_mesh(&mut self, g: i64, materials: &[i64], geo: &Mat4) -> Result<Option<(SceneMesh, Vec<BlendChannel>)>, MeshError> {
        let dom = self.dom;
        let gname = self.name_of(g);
        let what = format!("geometry {gname:?}");
        let Some(rec) = self.rec(g) else { return Ok(None) };
        let Some(va) = dom.child_array(rec, "Vertices")? else { return Ok(None) };
        let Some(pa) = dom.child_array(rec, "PolygonVertexIndex")? else { return Ok(None) };
        let mut verts = dom.f64s(&va, &format!("{what} Vertices"))?;
        if verts.len() % 3 != 0 {
            return Err(malformed(format!("{what} has {} vertex coordinates, not a multiple of 3", verts.len())));
        }
        // A vertex with a non-finite coordinate draws nothing a viewer can
        // show; its polygons are skipped below, and it reads as the origin.
        let off_vertex: Vec<bool> = verts.chunks_exact(3).map(|v| !v.iter().all(|c| c.is_finite())).collect();
        let any_off = off_vertex.iter().any(|b| *b);
        if any_off {
            for v in verts.iter_mut().filter(|v| !v.is_finite()) {
                *v = 0.0;
            }
        }
        let cps = verts.len() / 3;
        let pvi = dom.i32s(&pa, &format!("{what} PolygonVertexIndex"))?;
        // Polygons: the last corner of each is stored as -(index + 1).
        let mut polys: Vec<(usize, usize)> = Vec::new();
        let mut corner_cp: Vec<u32> = Vec::with_capacity(pvi.len());
        let mut start = 0usize;
        let mut triangles = 0usize;
        let mut degenerate = 0usize;
        let mut on_off_vertex = 0usize;
        for (i, &raw) in pvi.iter().enumerate() {
            let (cp, end) = if raw < 0 { (!raw, true) } else { (raw, false) };
            if cp as usize >= cps {
                return Err(malformed(format!("{what} polygon corner {i} names control point {cp} of {cps}")));
            }
            corner_cp.push(cp as u32);
            if end {
                let mut n = i + 1 - start;
                if any_off && corner_cp[start..=i].iter().any(|&c| off_vertex[c as usize]) {
                    // Skipped as a point polygon is (n < 3).
                    on_off_vertex += 1;
                    n = 0;
                } else if n >= 3 {
                    triangles += n - 2;
                } else {
                    degenerate += 1;
                }
                polys.push((start, n));
                start = i + 1;
            }
        }
        if on_off_vertex > 0 {
            self.warn(&format!("offvertex-{g}"), format!("{what} has {on_off_vertex} polygon(s) on a non-finite vertex; they were skipped"));
        }
        if start != pvi.len() {
            return Err(malformed(format!("{what}'s last polygon is not terminated by a negative index")));
        }
        if degenerate > 0 {
            self.warn(&format!("degen-{g}"), format!("{what} has {degenerate} point or edge polygon(s); they were skipped"));
        }
        if triangles == 0 {
            return Ok(None);
        }
        let new_indices = triangles.saturating_mul(3);
        if self.total_indices.saturating_add(new_indices) > MAX_SCENE_INDICES {
            return Err(unsupported(format!("the scene passes {MAX_SCENE_INDICES} indices at {what}")));
        }
        self.total_indices += new_indices;

        // Layer elements: the first normal, tangent, binormal and colour
        // element and every UV element, ordered by their typed index.
        let elements = |name: &str| -> Result<Vec<(i64, RecId)>, MeshError> {
            let mut v = Vec::new();
            for el in dom.children_named(rec, name) {
                v.push((dom.first_prop(el)?.and_then(|p| p.as_i64()).unwrap_or(0), el));
            }
            v.sort_by_key(|(i, _)| *i);
            Ok(v)
        };
        let corners = pvi.len();
        let npolys = polys.len();
        let mut layers = Layers { normal: None, tangent: None, binormal: None, uvs: Vec::new(), color: None };
        if let Some(&(_, el)) = elements("LayerElementNormal")?.first() {
            layers.normal = self.layer(el, "Normals", &["NormalsIndex", "NormalIndex"], 3, &format!("{what} normals"))?;
        }
        if layers.normal.is_some() {
            if let Some(&(_, el)) = elements("LayerElementTangent")?.first() {
                layers.tangent = self.layer(el, "Tangents", &["TangentsIndex", "TangentIndex"], 3, &format!("{what} tangents"))?;
            }
            if let Some(&(_, el)) = elements("LayerElementBinormal")?.first() {
                layers.binormal = self.layer(el, "Binormals", &["BinormalsIndex", "BinormalIndex"], 3, &format!("{what} binormals"))?;
            }
        }
        let uv_elements = elements("LayerElementUV")?;
        if uv_elements.len() > MAX_UV_SETS {
            self.warn(&format!("uvsets-{g}"), format!("{what} has {} UV sets; the first {MAX_UV_SETS} were read", uv_elements.len()));
        }
        for (k, &(_, el)) in uv_elements.iter().take(MAX_UV_SETS).enumerate() {
            if let Some(l) = self.layer(el, "UV", &["UVIndex"], 2, &format!("{what} UV set {k}"))? {
                layers.uvs.push(l);
            }
        }
        if let Some(&(_, el)) = elements("LayerElementColor")?.first() {
            layers.color = self.layer(el, "Colors", &["ColorIndex"], 4, &format!("{what} colours"))?;
        }
        for (l, w) in [(&layers.normal, "normals"), (&layers.tangent, "tangents"), (&layers.binormal, "binormals"), (&layers.color, "colours")] {
            if let Some(l) = l {
                check_layer(l, corners, npolys, cps, &format!("{what} {w}"))?;
            }
        }
        for (k, l) in layers.uvs.iter().enumerate() {
            check_layer(l, corners, npolys, cps, &format!("{what} UV set {k}"))?;
        }
        // Per-polygon material slots.
        let mut slot_of: Vec<i32> = vec![0; npolys];
        if let Some(&(_, el)) = elements("LayerElementMaterial")?.first() {
            let mapping = dom.child_prop(el, "MappingInformationType")?.and_then(|p| p.as_str().map(|s| s.into_owned())).unwrap_or_default();
            if let Some(arr) = dom.child_array(el, "Materials")? {
                let ms = dom.i32s(&arr, &format!("{what} material indices"))?;
                match mapping.as_str() {
                    "ByPolygon" => {
                        if ms.len() < npolys {
                            return Err(malformed(format!("{what} lists {} polygon materials for {npolys} polygons", ms.len())));
                        }
                        slot_of.copy_from_slice(&ms[..npolys]);
                    }
                    _ => {
                        let m0 = ms.first().copied().unwrap_or(0);
                        slot_of.iter_mut().for_each(|s| *s = m0);
                    }
                }
            }
        }
        let mut slot_material: Vec<Option<usize>> = Vec::with_capacity(materials.len());
        for &mid in materials {
            slot_material.push(self.material_for(mid)?);
        }
        let mut bad_slots = 0usize;
        let mut fanned_concave = 0usize;

        let skin = self.skin_for(g, cps)?;
        let blend = self.blend_for(g, cps)?;
        let ntargets = blend.as_ref().map_or(0, |b| b.targets.len());

        // Geometric transform: baked into this geometry and nothing else.
        let baking = !is_identity(geo);
        let nm = math::normal_matrix(geo);
        let lin = math::upper3(geo);
        let mirror = math::is_mirroring(geo);

        let mut builds: BTreeMap<i64, PrimBuild> = BTreeMap::new();
        let nsets = layers.uvs.len();
        for (p, &(start, n)) in polys.iter().enumerate() {
            if n < 3 {
                continue;
            }
            let slot = slot_of[p] as i64;
            let material = usize::try_from(slot).ok().and_then(|s| slot_material.get(s).copied());
            let key_slot = match material {
                Some(_) => slot,
                None => {
                    if !materials.is_empty() {
                        bad_slots += 1;
                    }
                    i64::MAX
                }
            };
            let b = builds.entry(key_slot).or_insert_with(|| PrimBuild { uvs: vec![Vec::new(); nsets], ..Default::default() });
            let mut vids: Vec<u32> = Vec::with_capacity(n);
            for (c, &cp) in corner_cp.iter().enumerate().skip(start).take(n) {
                let cp = cp as usize;
                let mut key: CornerKey = [0; 28];
                key[0] = cp as u32;
                let normal = match &layers.normal {
                    Some(l) => {
                        let v = l.get(c, p, cp, "normals")?;
                        Some([v[0] as f32, v[1] as f32, v[2] as f32])
                    }
                    None => None,
                };
                if let Some(nv) = normal {
                    key[1..4].copy_from_slice(&[nv[0].to_bits(), nv[1].to_bits(), nv[2].to_bits()]);
                }
                let tangent = match (&layers.tangent, normal) {
                    (Some(l), Some(nv)) => {
                        let t = l.get(c, p, cp, "tangents")?;
                        let t = [t[0] as f32, t[1] as f32, t[2] as f32];
                        // Handedness from the binormal: FBX keeps it there.
                        let w = match &layers.binormal {
                            Some(bl) => {
                                let bv = bl.get(c, p, cp, "binormals")?;
                                let cr = [nv[1] * t[2] - nv[2] * t[1], nv[2] * t[0] - nv[0] * t[2], nv[0] * t[1] - nv[1] * t[0]];
                                if cr[0] as f64 * bv[0] + cr[1] as f64 * bv[1] + cr[2] as f64 * bv[2] < 0.0 {
                                    -1.0
                                } else {
                                    1.0
                                }
                            }
                            None => 1.0,
                        };
                        Some([t[0], t[1], t[2], w])
                    }
                    _ => None,
                };
                if let Some(t) = tangent {
                    key[4..8].copy_from_slice(&[t[0].to_bits(), t[1].to_bits(), t[2].to_bits(), t[3].to_bits()]);
                }
                let color = match &layers.color {
                    Some(l) => {
                        let v = l.get(c, p, cp, "colours")?;
                        Some([srgb_decode(v[0]), srgb_decode(v[1]), srgb_decode(v[2]), v[3] as f32])
                    }
                    None => None,
                };
                if let Some(cv) = color {
                    key[8..12].copy_from_slice(&[cv[0].to_bits(), cv[1].to_bits(), cv[2].to_bits(), cv[3].to_bits()]);
                }
                let mut uv = [[0.0f32; 2]; MAX_UV_SETS];
                for (k, l) in layers.uvs.iter().enumerate() {
                    let v = l.get(c, p, cp, "uvs")?;
                    // FBX's V runs up the image, the scene's down.
                    uv[k] = [v[0] as f32, (1.0 - v[1]) as f32];
                    key[12 + 2 * k] = uv[k][0].to_bits();
                    key[13 + 2 * k] = uv[k][1].to_bits();
                }
                let next = b.positions.len() as u32;
                let id = *b.map.entry(key).or_insert(next);
                if id == next {
                    let pos = [verts[cp * 3], verts[cp * 3 + 1], verts[cp * 3 + 2]];
                    let pos = if baking {
                        let q = math::transform_point_f64(geo, pos);
                        [q[0] as f32, q[1] as f32, q[2] as f32]
                    } else {
                        [pos[0] as f32, pos[1] as f32, pos[2] as f32]
                    };
                    b.positions.push(pos);
                    b.cps.push(cp as u32);
                    if let Some(nv) = normal {
                        b.normals.push(if baking { math::transform_normal(&nm, nv) } else { nv });
                    }
                    if let Some(t) = tangent {
                        let t = if baking {
                            let d = math::transform_dir3(&lin, [t[0], t[1], t[2]]);
                            let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                            let d = if l > 0.0 { [d[0] / l, d[1] / l, d[2] / l] } else { [t[0], t[1], t[2]] };
                            [d[0], d[1], d[2], if mirror { -t[3] } else { t[3] }]
                        } else {
                            t
                        };
                        b.tangents.push(t);
                    }
                    if let Some(cv) = color {
                        b.colors.push(cv);
                    }
                    for (set, value) in b.uvs.iter_mut().zip(uv) {
                        set.push(value);
                    }
                }
                vids.push(id);
            }
            // Fanned when convex, ear-clipped when not (`polygon`; ufbx and
            // three.js's FBXLoader ear-clip too), keeping the polygon's
            // winding — reversed under a mirroring geometric transform.
            let (tris, fanned) = if n > 3 {
                let pts: Vec<[f64; 3]> = vids.iter().map(|&v| b.positions[v as usize].map(f64::from)).collect();
                super::polygon::triangles(&pts)
            } else {
                (vec![[0, 1, 2]], false)
            };
            fanned_concave += usize::from(fanned);
            for t in tris {
                if mirror {
                    b.indices.extend_from_slice(&[vids[t[0]], vids[t[2]], vids[t[1]]]);
                } else {
                    b.indices.extend_from_slice(&[vids[t[0]], vids[t[1]], vids[t[2]]]);
                }
            }
        }
        if fanned_concave > 0 {
            self.warn(
                &format!("fanned-{g}"),
                format!(
                    "{what} has {fanned_concave} concave polygon(s) of more than {} corners; they were fan-triangulated",
                    super::polygon::MAX_EAR_CLIP
                ),
            );
        }
        if bad_slots > 0 {
            self.warn(
                &format!("slots-{g}"),
                format!("{what} has {bad_slots} polygon(s) whose material index names no material; they use the default"),
            );
        }
        let mut prims = Vec::with_capacity(builds.len());
        for (slot, b) in builds {
            let vcount = b.positions.len();
            if self.total_vertices.saturating_add(vcount) > MAX_SCENE_VERTICES {
                return Err(unsupported(format!("the scene passes {MAX_SCENE_VERTICES} vertices at {what}")));
            }
            self.total_vertices += vcount;
            let skin_weights = skin.as_ref().map(|(_, inf)| {
                let width = inf.width;
                let mut joints = vec![0u16; vcount * width];
                let mut weights = vec![0.0f32; vcount * width];
                for (v, &cp) in b.cps.iter().enumerate() {
                    let (js, ws) = inf.of(cp as usize);
                    for k in 0..js.len().min(width) {
                        joints[v * width + k] = js[k];
                        weights[v * width + k] = ws[k];
                    }
                }
                SkinWeights { influences: width, joints, weights }
            });
            let mut targets = Vec::with_capacity(ntargets);
            if let Some(bl) = &blend {
                let need = ntargets.saturating_mul(vcount).saturating_mul(3);
                if self.morph_values.saturating_add(need) > MAX_MORPH_VALUES {
                    return Err(unsupported(format!("{what}'s blend shapes expand past {MAX_MORPH_VALUES} values")));
                }
                self.morph_values += need;
                for t in &bl.targets {
                    let positions: Vec<[f32; 3]> = b
                        .cps
                        .iter()
                        .map(|cp| {
                            let d = t.positions[*cp as usize];
                            if baking {
                                math::transform_dir3(&lin, d)
                            } else {
                                d
                            }
                        })
                        .collect();
                    let normals = t.normals.as_ref().map(|ns| {
                        b.cps
                            .iter()
                            .map(|cp| {
                                let d = ns[*cp as usize];
                                if baking {
                                    math::transform_dir3(&nm, d)
                                } else {
                                    d
                                }
                            })
                            .collect()
                    });
                    targets.push(MorphTarget { positions, normals });
                }
            }
            let material = if slot == i64::MAX { None } else { usize::try_from(slot).ok().and_then(|s| slot_material.get(s).copied().flatten()) };
            prims.push(Primitive {
                positions: b.positions,
                normals: layers.normal.is_some().then_some(b.normals),
                tangents: layers.tangent.is_some().then_some(b.tangents),
                uvs: b.uvs,
                colors: layers.color.is_some().then_some(b.colors),
                skin: skin_weights,
                indices: b.indices,
                material,
                targets,
                topology: super::scene::Topology::Triangles,
            });
        }
        let (weights, names, channels) = match &blend {
            Some(b) => (b.defaults.clone(), b.names.clone(), b.channels.clone()),
            None => (Vec::new(), Vec::new(), Vec::new()),
        };
        Ok(Some((
            SceneMesh { name: Some(gname).filter(|n| !n.is_empty()), primitives: prims, weights, target_names: names },
            channels,
        )))
    }

    /// One `Line` Geometry as an IR mesh of one lines primitive: `Points`,
    /// three doubles each, and `PointsIndex`, whose entries run through the
    /// points in strips ([`walk_strips`]), a negative entry `-(e + 1)` naming
    /// point `e` and ending its strip as `PolygonVertexIndex` ends a polygon.
    /// assimp v6.0.5 reads exactly these two arrays
    /// (`code/AssetLib/FBX/FBXMeshGeometry.cpp` L716-728) and makes a segment
    /// from each non-negative entry to the next, an end marker converted back
    /// to its point (`FBXConverter.cpp` L1030-1068); the FBX SDK's `FbxLine`
    /// (https://help.autodesk.com/cloudhelp/2020/ENU/FBX-API-Reference/cpp_ref/class_fbx_line.html)
    /// keeps the same two arrays, "certain points could be marked as end
    /// points" to leave gaps between segments.
    ///
    /// The points no segment names are not in the scene, as a polygon's
    /// unused control points are not; a segment on a point that is not finite
    /// is skipped and said. The model's geometric transform bakes in as a
    /// mesh's does, and its first material binds as a mesh's single material
    /// does. A Line's layer elements are not read.
    fn build_lines(&mut self, g: i64, materials: &[i64], geo: &Mat4) -> Result<Option<(SceneMesh, Vec<BlendChannel>)>, MeshError> {
        let dom = self.dom;
        let gname = self.name_of(g);
        let what = format!("geometry {gname:?}");
        let Some(rec) = self.rec(g) else { return Ok(None) };
        let Some(pa) = dom.child_array(rec, "Points")? else { return Ok(None) };
        let Some(ia) = dom.child_array(rec, "PointsIndex")? else { return Ok(None) };
        let points = dom.f64s(&pa, &format!("{what} Points"))?;
        if points.len() % 3 != 0 {
            return Err(malformed(format!("{what} has {} point coordinates, not a multiple of 3", points.len())));
        }
        let cps = points.len() / 3;
        let finite = |cp: usize| points[cp * 3..cp * 3 + 3].iter().all(|c| c.is_finite());
        let index = dom.i32s(&ia, &format!("{what} PointsIndex"))?;
        // A first reading checks every entry and counts what stays: the
        // segments, and the points they name.
        let (mut segments, mut off) = (0usize, 0usize);
        let mut named = vec![false; cps];
        walk_strips(&index, cps, &what, |a, b| {
            if finite(a) && finite(b) {
                segments += 1;
                named[a] = true;
                named[b] = true;
            } else {
                off += 1;
            }
        })?;
        if off > 0 {
            self.warn(&format!("offpoint-{g}"), format!("{what} has {off} line segment(s) on a non-finite point; they were skipped"));
        }
        if segments == 0 {
            return Ok(None);
        }
        // Charged before anything is built: two indices a segment, one vertex
        // a point named.
        let kept = named.iter().filter(|n| **n).count();
        let new_indices = segments.saturating_mul(2);
        if self.total_indices.saturating_add(new_indices) > MAX_SCENE_INDICES {
            return Err(unsupported(format!("the scene passes {MAX_SCENE_INDICES} indices at {what}")));
        }
        if self.total_vertices.saturating_add(kept) > MAX_SCENE_VERTICES {
            return Err(unsupported(format!("the scene passes {MAX_SCENE_VERTICES} vertices at {what}")));
        }
        self.total_indices += new_indices;
        self.total_vertices += kept;

        // Geometric transform: baked into this geometry and nothing else.
        let baking = !is_identity(geo);
        let mut slot = vec![0u32; cps];
        let mut positions: Vec<[f32; 3]> = Vec::with_capacity(kept);
        for cp in (0..cps).filter(|&cp| named[cp]) {
            slot[cp] = positions.len() as u32;
            let pos = [points[cp * 3], points[cp * 3 + 1], points[cp * 3 + 2]];
            let pos = if baking { math::transform_point_f64(geo, pos) } else { pos };
            positions.push([pos[0] as f32, pos[1] as f32, pos[2] as f32]);
        }
        let mut indices: Vec<u32> = Vec::with_capacity(new_indices);
        walk_strips(&index, cps, &what, |a, b| {
            if finite(a) && finite(b) {
                indices.extend_from_slice(&[slot[a], slot[b]]);
            }
        })?;
        let material = match materials.first() {
            Some(&m) => self.material_for(m)?,
            None => None,
        };
        let prim = Primitive {
            positions,
            normals: None,
            tangents: None,
            uvs: Vec::new(),
            colors: None,
            skin: None,
            indices,
            material,
            targets: Vec::new(),
            topology: super::scene::Topology::Lines,
        };
        let mesh = SceneMesh { name: Some(gname).filter(|n| !n.is_empty()), primitives: vec![prim], weights: Vec::new(), target_names: Vec::new() };
        Ok(Some((mesh, Vec::new())))
    }

    // ── skins ───────────────────────────────────────────────────────────────

    /// The IR skin of geometry `g` and its per-control-point influences.
    fn skin_for(&mut self, g: i64, cps: usize) -> Result<Option<(usize, Influences)>, MeshError> {
        if let Some(s) = self.skin_map.get(&g) {
            return Ok(s.clone());
        }
        let out = self.build_skin(g, cps)?;
        self.skin_map.insert(g, out.clone());
        Ok(out)
    }

    fn build_skin(&mut self, g: i64, cps: usize) -> Result<Option<(usize, Influences)>, MeshError> {
        let dom = self.dom;
        let skins = self.kids_of(g, "Deformer", Some("Skin"));
        let Some(&skin) = skins.first() else { return Ok(None) };
        if skins.len() > 1 {
            self.warn(&format!("skins-{g}"), format!("geometry {:?} has {} skins; only the first was read", self.name_of(g), skins.len()));
        }
        let clusters = self.kids_of(skin, "Deformer", Some("Cluster"));
        let mut joints: Vec<usize> = Vec::new();
        let mut ibms: Vec<Mat4> = Vec::new();
        let mut per_cp: Vec<Vec<(u16, f32)>> = Vec::new();
        let mut total = 0usize;
        if self.bind.is_none() {
            self.bind = Some(self.bind_pose()?);
        }
        let bind = self.bind.clone().unwrap_or_default();
        for c in clusters {
            let what = format!("cluster {:?}", self.name_of(c));
            let link = self.kids_of(c, "Model", None).first().copied();
            let Some(node) = link.and_then(|l| self.node_of.get(&l).copied()) else {
                self.warn(&format!("nolink-{c}"), format!("{what} is linked to no bone; its influences were dropped"));
                continue;
            };
            let link = link.unwrap_or(0);
            if joints.len() >= MAX_JOINTS {
                return Err(unsupported(format!("a skin has more than {MAX_JOINTS} joints")));
            }
            let matrix = |name: &str| -> Result<Option<Mat4>, MeshError> {
                let Some(a) = self.array(c, name)? else { return Ok(None) };
                let v = dom.f64s(&a, &format!("{what} {name}"))?;
                if v.len() != 16 || v.iter().any(|x| !x.is_finite()) {
                    return Err(malformed(format!("{what} {name} is not 16 finite numbers")));
                }
                let mut m = [0.0; 16];
                m.copy_from_slice(&v);
                Ok(Some(m).filter(|m| m.iter().any(|x| *x != 0.0)))
            };
            // Transform maps mesh space to bone space at bind time: glTF's
            // inverse bind matrix. Without it, rebuild it from the link's
            // bind matrix and the mesh's (research notes §5.2).
            let ibm = match matrix("Transform")? {
                Some(t) => t,
                None => {
                    let link_m = match matrix("TransformLink")? {
                        Some(m) => m,
                        None => bind.get(&link).copied().unwrap_or_else(|| self.world.get(&link).copied().unwrap_or(math::IDENTITY)),
                    };
                    let mesh_model = self.ups(g).iter().find(|(p, prop)| prop.is_none() && self.kind(*p) == "Model").map(|(p, _)| *p);
                    let mesh_bind = mesh_model
                        .and_then(|m| bind.get(&m).copied().or_else(|| self.world.get(&m).copied()))
                        .unwrap_or(math::IDENTITY);
                    let inv = math::invert(&link_m).ok_or_else(|| malformed(format!("{what} TransformLink is singular")))?;
                    math::mat_mul(&inv, &mesh_bind)
                }
            };
            let j = joints.len() as u16;
            joints.push(node);
            ibms.push(ibm);
            let idx = match self.array(c, "Indexes")? {
                Some(a) => dom.i32s(&a, &format!("{what} Indexes"))?,
                None => Vec::new(),
            };
            let wts = match self.array(c, "Weights")? {
                Some(a) => dom.f64s(&a, &format!("{what} Weights"))?,
                None => Vec::new(),
            };
            if idx.len() != wts.len() {
                return Err(malformed(format!("{what} has {} indexes and {} weights", idx.len(), wts.len())));
            }
            if per_cp.is_empty() && !idx.is_empty() {
                per_cp = vec![Vec::new(); cps];
            }
            total = total.saturating_add(idx.len());
            if total > MAX_SCENE_VERTICES.saturating_mul(MAX_INFLUENCES) {
                return Err(unsupported("the skin has more influences than this converter will hold"));
            }
            for (i, w) in idx.iter().zip(&wts) {
                let cp = usize::try_from(*i).ok().filter(|cp| *cp < cps).ok_or_else(|| {
                    malformed(format!("{what} names control point {i} of {cps}"))
                })?;
                if !w.is_finite() {
                    return Err(malformed(format!("{what} has a non-finite weight")));
                }
                if *w > 0.0 {
                    per_cp[cp].push((j, *w as f32));
                }
            }
        }
        if joints.is_empty() {
            return Ok(None);
        }
        if per_cp.is_empty() {
            per_cp = vec![Vec::new(); cps];
        }
        let mut widest = 0usize;
        let mut capped = 0usize;
        for list in per_cp.iter_mut() {
            list.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            if list.len() > MAX_INFLUENCES {
                list.truncate(MAX_INFLUENCES);
                capped += 1;
            }
            widest = widest.max(list.len());
        }
        if capped > 0 {
            self.warn(&format!("cap-{g}"), format!("{capped} vertex(es) had more than {MAX_INFLUENCES} influences; the smallest were dropped"));
        }
        // An FBX control point no cluster names stays where the mesh puts it;
        // a glTF skin has no such vertex (`pose::skin_in_place`, like
        // three.js, binds it to its first joint slot), so say so.
        let unweighted = per_cp.iter().filter(|l| l.is_empty()).count();
        if unweighted > 0 {
            self.warn(
                &format!("unweighted-{g}"),
                format!(
                    "{unweighted} control point(s) of geometry {:?} have no skin weight; they follow the skin's first joint",
                    self.name_of(g)
                ),
            );
        }
        let width = widest.div_ceil(4).max(1) * 4;
        let mut inf = Influences { offsets: Vec::with_capacity(cps + 1), joints: Vec::new(), weights: Vec::new(), width };
        inf.offsets.push(0);
        for list in &per_cp {
            for (j, w) in list {
                inf.joints.push(*j);
                inf.weights.push(*w);
            }
            inf.offsets.push(inf.joints.len() as u32);
        }
        let name = Some(self.name_of(skin)).filter(|n| !n.is_empty());
        self.scene.skins.push(Skin { name, joints, inverse_bind_matrices: ibms, skeleton: None });
        Ok(Some((self.scene.skins.len() - 1, inf)))
    }

    /// The first BindPose, model id → global matrix.
    fn bind_pose(&self) -> Result<HashMap<i64, Mat4>, MeshError> {
        let dom = self.dom;
        let mut out = HashMap::new();
        for &id in &self.order {
            if self.kind(id) != "Pose" || self.sub(id) != "BindPose" {
                continue;
            }
            let Some(rec) = self.rec(id) else { continue };
            for pn in dom.children_named(rec, "PoseNode") {
                let node = dom.child_prop(pn, "Node")?.and_then(|p| p.as_i64());
                let m = match dom.child_array(pn, "Matrix")? {
                    Some(a) => dom.f64s(&a, "PoseNode Matrix")?,
                    None => continue,
                };
                if let (Some(node), true) = (node, m.len() == 16 && m.iter().all(|v| v.is_finite())) {
                    let mut mm = [0.0; 16];
                    mm.copy_from_slice(&m);
                    out.entry(node).or_insert(mm);
                }
            }
            break;
        }
        Ok(out)
    }

    // ── blend shapes ────────────────────────────────────────────────────────

    fn blend_for(&mut self, g: i64, cps: usize) -> Result<Option<std::rc::Rc<Blend>>, MeshError> {
        if let Some(b) = self.blend_map.get(&g) {
            return Ok(b.clone());
        }
        let out = self.build_blend(g, cps)?.map(std::rc::Rc::new);
        self.blend_map.insert(g, out.clone());
        Ok(out)
    }

    fn build_blend(&mut self, g: i64, cps: usize) -> Result<Option<Blend>, MeshError> {
        let dom = self.dom;
        let mut blend = Blend { channels: Vec::new(), names: Vec::new(), defaults: Vec::new(), targets: Vec::new() };
        for bs in self.kids_of(g, "Deformer", Some("BlendShape")) {
            for ch in self.kids_of(bs, "Deformer", Some("BlendShapeChannel")) {
                let what = format!("blend shape channel {:?}", self.name_of(ch));
                let shapes = self.kids_of(ch, "Geometry", Some("Shape"));
                if shapes.is_empty() {
                    continue;
                }
                let full = match self.array(ch, "FullWeights")? {
                    Some(a) => dom.f64s(&a, &format!("{what} FullWeights"))?,
                    None => Vec::new(),
                };
                // Blender writes FullWeights as one weight per shape vertex
                // (the shape key's vertex group), not one per shape (ufbx
                // reads it the same way); everyone else, one full weight per
                // shape, in percent, defaulting to 100.
                let blender = self.creator.contains("Blender");
                let mut keyed: Vec<(f64, i64)> = shapes
                    .iter()
                    .enumerate()
                    .map(|(i, s)| (if blender { 100.0 } else { full.get(i).copied().unwrap_or(100.0) }, *s))
                    .collect();
                if keyed.iter().any(|(w, _)| !w.is_finite()) {
                    return Err(malformed(format!("{what} has a non-finite FullWeight")));
                }
                keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
                if keyed.len() > 1 {
                    // In-between shapes: each becomes a target of its own,
                    // driven piecewise-linearly by the channel's weight.
                    self.warn(
                        &format!("inbetween-{ch}"),
                        format!("{what} has {} in-between shapes; each became a morph target, weighted from the channel's weight", keyed.len()),
                    );
                }
                if blend.targets.len().saturating_add(keyed.len()) > MAX_MORPH_TARGETS {
                    return Err(unsupported(format!("geometry {:?} has more than {MAX_MORPH_TARGETS} blend shapes", self.name_of(g))));
                }
                let pm = self.p70(ch)?;
                let pct = match self.props(&pm, "Deformer").f64("DeformPercent") {
                    Some(v) => v,
                    None => dom.child_prop(self.rec(ch).unwrap_or(ROOT), "DeformPercent")?.and_then(|p| p.as_f64()).unwrap_or(0.0),
                };
                if !pct.is_finite() {
                    return Err(malformed(format!("{what} DeformPercent is not finite")));
                }
                let channel = BlendChannel {
                    id: ch,
                    full: keyed.iter().map(|(pct, _)| pct / 100.0).collect(),
                    first: blend.targets.len(),
                    default: pct / 100.0,
                };
                let shares = inbetween_weights(&channel.full, channel.default);
                let cname = self.name_of(ch);
                for (k, (pct, shape)) in keyed.iter().enumerate() {
                    let (positions, normals) = self.shape_offsets(*shape, cps, &what, blender.then_some(full.as_slice()))?;
                    // The full-weight shape keeps the channel's name; an
                    // in-between is named for its weight, in percent.
                    blend.names.push(if k + 1 == keyed.len() { cname.clone() } else { format!("{cname}@{pct}") });
                    blend.defaults.push(shares[k] as f32);
                    blend.targets.push(Target { positions, normals });
                }
                blend.channels.push(channel);
            }
        }
        if blend.targets.is_empty() {
            return Ok(None);
        }
        // The scene keeps target normals all-or-nothing per mesh.
        if blend.targets.iter().any(|t| t.normals.is_some()) {
            for t in blend.targets.iter_mut() {
                if t.normals.is_none() {
                    t.normals = Some(vec![[0.0; 3]; cps]);
                }
            }
        }
        Ok(Some(blend))
    }

    /// One shape's offsets (and normal offsets), dense over control points.
    /// `vertex_weights`: Blender's per-vertex FullWeights (percent), applied
    /// when there is one per offset.
    fn shape_offsets(
        &mut self,
        shape: i64,
        cps: usize,
        what: &str,
        vertex_weights: Option<&[f64]>,
    ) -> Result<Offsets, MeshError> {
        let dom = self.dom;
        let idx = match self.array(shape, "Indexes")? {
            Some(a) => dom.i32s(&a, &format!("{what} Indexes"))?,
            None => Vec::new(),
        };
        let dv = match self.array(shape, "Vertices")? {
            Some(a) => dom.f64s(&a, &format!("{what} Vertices"))?,
            None => Vec::new(),
        };
        if dv.len() != idx.len().saturating_mul(3) {
            return Err(malformed(format!("{what} has {} offsets for {} indexes", dv.len(), idx.len())));
        }
        let dn = match self.array(shape, "Normals")? {
            Some(a) => {
                let v = dom.f64s(&a, &format!("{what} Normals"))?;
                if v.len() == dv.len() {
                    Some(v)
                } else {
                    self.warn(&format!("shapenormals-{shape}"), format!("{what}'s normals do not match its offsets; they were dropped"));
                    None
                }
            }
            None => None,
        };
        let scale = vertex_weights.filter(|w| w.len() == idx.len() && w.iter().all(|x| x.is_finite()));
        let charge = cps.saturating_mul(6);
        if self.morph_values.saturating_add(charge) > MAX_MORPH_VALUES {
            return Err(unsupported("the blend shapes expand past the morph-target limit"));
        }
        self.morph_values += charge;
        let mut positions = vec![[0.0f32; 3]; cps];
        let mut normals = dn.as_ref().map(|_| vec![[0.0f32; 3]; cps]);
        for (k, i) in idx.iter().enumerate() {
            let cp = usize::try_from(*i)
                .ok()
                .filter(|cp| *cp < cps)
                .ok_or_else(|| malformed(format!("{what} names control point {i} of {cps}")))?;
            let f = scale.map_or(1.0, |w| w[k] / 100.0);
            let d = [dv[k * 3] * f, dv[k * 3 + 1] * f, dv[k * 3 + 2] * f];
            if !finite3(d) {
                return Err(malformed(format!("{what} has a non-finite offset")));
            }
            positions[cp] = [d[0] as f32, d[1] as f32, d[2] as f32];
            if let (Some(ns), Some(src)) = (normals.as_mut(), dn.as_ref()) {
                let n = [src[k * 3] * f, src[k * 3 + 1] * f, src[k * 3 + 2] * f];
                if finite3(n) {
                    ns[cp] = [n[0] as f32, n[1] as f32, n[2] as f32];
                }
            }
        }
        Ok((positions, normals))
    }

    // ── materials and textures ──────────────────────────────────────────────

    fn material_for(&mut self, id: i64) -> Result<Option<usize>, MeshError> {
        if let Some(m) = self.material_map.get(&id) {
            return Ok(*m);
        }
        let m = self.build_material(id)?;
        self.scene.materials.push(m);
        let i = self.scene.materials.len() - 1;
        self.material_map.insert(id, Some(i));
        Ok(Some(i))
    }

    fn build_material(&mut self, id: i64) -> Result<Material, MeshError> {
        let pm = self.p70(id)?;
        // The notes below quote the name, clipped (Ruling A45).
        let what = format!("material {:?}", quoted_name(&self.name_of(id)));
        let props = self.props(&pm, "Material");
        let color = |name: &str| props.vec3(name).filter(|v| finite3(*v));
        let space = self.colors;
        let decode = |v: [f64; 3]| v.map(|c| decode_color(c, space));
        let mut m = Material { name: Some(self.name_of(id)).filter(|n| !n.is_empty()), metallic: 0.0, ..Default::default() };
        let mut deferred: Vec<String> = Vec::new();
        let mut opacity_note: Option<String> = None;
        let flavour = if props.has("Maya|base_color") || props.has("Maya|metallic") {
            Flavour::Stingray
        } else if props.has("Maya|baseColor") {
            Flavour::Arnold
        } else if props.has("3dsMax|Parameters|base_metalness") {
            Flavour::MaxOpenPbr
        } else if props.has("3dsMax|Parameters|base_color") {
            Flavour::MaxPhysical
        } else if let Some(k) = ["3dsMax|main|basecolor", "3dsMax|main|baseColor"].into_iter().find(|k| props.has(k)) {
            Flavour::MaxPbr(k)
        } else {
            Flavour::Phong
        };
        let f = |name: &str, default: f64| props.f64(name).filter(|v| v.is_finite()).unwrap_or(default);
        // Colours of Maya's PBR shaders are scene-linear like the shaders
        // themselves; 3ds Max's, like every classic FBX colour, are read as
        // sRGB (see `fbx::srgb_encode`).
        let linear = |name: &str| color(name).map(|c| c.map(|v| v as f32));
        let max_color = |name: &str| props.vec4(name).filter(|c| c.iter().all(|v| v.is_finite())).map(|c| decode([c[0], c[1], c[2]]));
        let scale = |c: [f32; 3], k: f64| c.map(|v| v * k as f32);
        let (base, metallic, roughness, emissive): (Option<[f32; 3]>, f64, f64, [f32; 3]) = match flavour {
            Flavour::Stingray => (
                linear("Maya|base_color"),
                f("Maya|metallic", 0.0),
                f("Maya|roughness", 1.0),
                scale(linear("Maya|emissive").unwrap_or([0.0; 3]), f("Maya|emissive_intensity", 1.0)),
            ),
            // Arnold's aiStandardSurface and Autodesk Standard Surface.
            Flavour::Arnold => (
                linear("Maya|baseColor").map(|c| scale(c, f("Maya|base", 1.0))),
                f("Maya|metalness", 0.0),
                f("Maya|specularRoughness", 1.0),
                scale(linear("Maya|emissionColor").unwrap_or([0.0; 3]), f("Maya|emission", 0.0)),
            ),
            Flavour::MaxPhysical => {
                let r = f("3dsMax|Parameters|roughness", 1.0);
                let inv = props.i64("3dsMax|Parameters|roughness_inv").unwrap_or(0) != 0;
                (
                    max_color("3dsMax|Parameters|base_color").map(|c| scale(c, f("3dsMax|Parameters|base_weight", 1.0))),
                    f("3dsMax|Parameters|metalness", 0.0),
                    if inv { 1.0 - r } else { r },
                    scale(max_color("3dsMax|Parameters|emit_color").unwrap_or([0.0; 3]), f("3dsMax|Parameters|emission", 0.0)),
                )
            }
            Flavour::MaxOpenPbr => (
                max_color("3dsMax|Parameters|base_color").map(|c| scale(c, f("3dsMax|Parameters|base_weight", 1.0))),
                f("3dsMax|Parameters|base_metalness", 0.0),
                f("3dsMax|Parameters|specular_roughness", 1.0),
                scale(max_color("3dsMax|Parameters|emission_color").unwrap_or([0.0; 3]), f("3dsMax|Parameters|emission_weight", 0.0)),
            ),
            Flavour::MaxPbr(key) => {
                let r = f("3dsMax|main|roughness", 1.0);
                // The material's own switch, as Assimp reads it: 1 means the
                // roughness slot holds glossiness, 2 that it holds roughness.
                let roughness = match props.i64("3dsMax|main|useGlossiness") {
                    Some(1) => 1.0 - r,
                    Some(2) | None => r,
                    Some(other) => {
                        deferred.push(format!("{what} has useGlossiness {other}; its roughness was read as roughness"));
                        r
                    }
                };
                let base = props
                    .vec4(key)
                    .or_else(|| props.vec3(key).map(|v| [v[0], v[1], v[2], 1.0]))
                    .filter(|c| c.iter().all(|v| v.is_finite()))
                    .map(|c| decode([c[0], c[1], c[2]]));
                let emit = max_color("3dsMax|main|emit_color").or_else(|| max_color("3dsMax|main|emissionColor"));
                (base, f("3dsMax|main|metalness", 0.0), roughness, emit.unwrap_or([0.0; 3]))
            }
            Flavour::Phong => {
                // The legacy `Diffuse` is the computed DiffuseColor ×
                // DiffuseFactor; three.js prefers it too.
                let diffuse = color("Diffuse").map(decode).or_else(|| {
                    color("DiffuseColor").map(|c| scale(decode(c), f("DiffuseFactor", 1.0)))
                });
                // Blender's encoding: shininess = (10·(1 − roughness))².
                // Decoded for every Phong file: it maps the SDK's default
                // exponent (20) to a plausible 0.55, and Blender's own files
                // exactly.
                let shininess = props.f64("ShininessExponent").or_else(|| props.f64("Shininess")).unwrap_or(20.0);
                // ReflectionFactor is metallic only in Blender's encoding; for
                // any other writer it is the SDK's reflection weight (default
                // 1), and reading it as metal would turn every Maya material
                // to chrome.
                let metallic = if self.creator.contains("Blender") { f("ReflectionFactor", 0.0) } else { 0.0 };
                // The legacy `Emissive`, like `Diffuse`, is the computed colour
                // × factor: the factor applies once.
                let emissive = color("Emissive").map(decode).or_else(|| {
                    color("EmissiveColor").map(|c| scale(decode(c), f("EmissiveFactor", 1.0)))
                });
                (diffuse, metallic, 1.0 - shininess.max(0.0).sqrt() / 10.0, emissive.unwrap_or([0.0; 3]))
            }
        };
        if let Some(b) = base {
            m.base_color = [b[0], b[1], b[2], 1.0];
        }
        // Opacity: the explicit property when present (Blender and the SDK
        // both write it), else the SDK's TransparentColor × TransparencyFactor
        // — or Arnold's own opacity.
        let alpha = match (flavour, props.vec3("Maya|opacity")) {
            (Flavour::Arnold, Some(o)) => (o[0] + o[1] + o[2]) / 3.0,
            _ => match props.f64("Opacity") {
                Some(o) => o,
                None => {
                    // The FbxSurfacePhong template's defaults: factor 0,
                    // colour black (research-fbx.md).
                    let tf = props.f64("TransparencyFactor").unwrap_or(0.0);
                    let c = props.vec3("TransparentColor").unwrap_or([0.0; 3]);
                    1.0 - tf * (c[0] + c[1] + c[2]) / 3.0
                }
            },
        };
        let alpha = if alpha.is_finite() {
            alpha
        } else {
            opacity_note = Some(format!("{what} opacity is not finite; it was read as opaque"));
            1.0
        };
        let alpha = alpha.clamp(0.0, 1.0) as f32;
        m.base_color[3] = alpha;
        if alpha < 1.0 {
            m.alpha_mode = AlphaMode::Blend;
        }
        m.metallic = (metallic as f32).clamp(0.0, 1.0);
        m.roughness = (roughness as f32).clamp(0.0, 1.0);
        if emissive.iter().all(|v| v.is_finite() && *v >= 0.0) {
            m.emissive = emissive;
        }
        if let Some(b) = props.f64("BumpFactor").or_else(|| props.f64("3dsMax|main|bump_map_amt")).filter(|b| b.is_finite()) {
            m.normal_scale = b as f32;
        }
        // A map on a material's colour REPLACES the colour value: Stingray
        // when its `use_*_map` switch is on, 3ds Max when `*_map_on` is (the
        // default), Arnold/Standard Surface whenever one is connected, and
        // classic Lambert/Phong whenever one is connected. The stored colour
        // is then the unconnected value — Stingray files carry placeholders
        // like 0.01 there, Arnold ones black, Blender its socket default 0.8,
        // Maya its 0.5 grey — and only the factor still scales the map. That
        // is how the SDK, Maya, Blender and ufbx read classic Phong, and what
        // the file's author saw: Meshy's FBX (DiffuseColor 0.8 beside its
        // texture) and Meshy's GLB of the same model (baseColorFactor 1.0)
        // agree only this way. three.js, Unity and Assimp multiply the colour
        // in instead, and render such a file darker than its author did.
        // When a switch is OFF the map is not used at all.
        let on = |name: &str, default: bool| match props.get(name).and_then(|v| v.first()) {
            Some(p) => p.as_f64().is_some_and(|v| v > 0.5),
            None => default,
        };
        let maps = match flavour {
            Flavour::Stingray => MapUse {
                base: on("Maya|use_color_map", false).then_some(1.0),
                emissive: on("Maya|use_emissive_map", false).then(|| f("Maya|emissive_intensity", 1.0) as f32),
                base_used: on("Maya|use_color_map", true),
                emissive_used: on("Maya|use_emissive_map", true),
                normal_used: on("Maya|use_normal_map", true),
                ao_used: on("Maya|use_ao_map", true),
            },
            Flavour::Arnold => MapUse {
                base: Some(f("Maya|base", 1.0) as f32),
                emissive: Some(f("Maya|emission", 0.0) as f32),
                ..MapUse::ALL
            },
            Flavour::MaxPhysical => MapUse {
                base: Some(f("3dsMax|Parameters|base_weight", 1.0) as f32),
                emissive: Some(f("3dsMax|Parameters|emission", 0.0) as f32),
                base_used: on("3dsMax|Parameters|base_color_map_on", true),
                emissive_used: on("3dsMax|Parameters|emit_color_map_on", true),
                normal_used: on("3dsMax|Parameters|bump_map_on", true),
                ..MapUse::ALL
            },
            Flavour::MaxOpenPbr => MapUse {
                base: Some(f("3dsMax|Parameters|base_weight", 1.0) as f32),
                emissive: Some(f("3dsMax|Parameters|emission_weight", 0.0) as f32),
                base_used: on("3dsMax|Parameters|base_color_map_on", true),
                emissive_used: on("3dsMax|Parameters|emission_color_map_on", true),
                ..MapUse::ALL
            },
            Flavour::MaxPbr(_) => MapUse { base: Some(1.0), emissive: Some(1.0), ..MapUse::ALL },
            Flavour::Phong => MapUse {
                base: Some(f("DiffuseFactor", 1.0) as f32),
                emissive: Some(f("EmissiveFactor", 1.0) as f32),
                ..MapUse::ALL
            },
        };
        for d in deferred {
            self.warn("maxgloss", d);
        }
        if let Some(n) = opacity_note {
            self.warn(&format!("opacity-{id}"), n);
        }
        // Textures, by the material property they drive.
        let edges: Vec<Edge> = self.kids(id).to_vec();
        for (tex, prop) in edges {
            let Some(slot) = prop else { continue };
            let target = match slot.as_str() {
                "DiffuseColor" | "Diffuse" | "Maya|TEX_color_map" | "Maya|baseColor" | "3dsMax|Parameters|base_color_map"
                | "3dsMax|main|base_color_map" | "3dsMax|main|baseColorMap" => 0,
                "NormalMap" | "Maya|TEX_normal_map" | "Maya|normalCamera" | "3dsMax|main|norm_map" | "3dsMax|main|normalMap" => 1,
                "EmissiveColor" | "Emissive" | "Maya|TEX_emissive_map" | "Maya|emissionColor" | "3dsMax|Parameters|emit_color_map"
                | "3dsMax|main|emit_color_map" | "3dsMax|main|emissionMap" => 2,
                "Maya|TEX_ao_map" | "3dsMax|main|ao_map" | "3dsMax|main|ambientOcclusionMap" => 3,
                other => {
                    if matches!(self.kind(tex), "Texture" | "LayeredTexture") {
                        // Textures on layer properties aren't carried yet;
                        // the layer reads from its values.
                        let layer = flavour.layers().and_then(|n| n.texture_slots.iter().find(|(s, _)| *s == other));
                        let msg = match layer {
                            Some((_, layer)) => format!(
                                "textures on the material slot {other:?} drive the {} layer; FBX layer textures are not carried, so the layer is read from the slot's stored value only",
                                layer.as_str()
                            ),
                            None => format!("textures on the material slot {other:?} have no metallic-roughness equivalent and were not carried"),
                        };
                        self.warn(&format!("slot-{other}"), msg);
                    }
                    continue;
                }
            };
            let used = match target {
                0 => maps.base_used,
                1 => maps.normal_used,
                2 => maps.emissive_used,
                _ => maps.ao_used,
            };
            if !used {
                // Connected but switched off in the material: the DCC does
                // not sample it, and neither does the scene.
                continue;
            }
            let Some(r) = self.texture_ref(tex)? else { continue };
            let slot_ref = match target {
                0 => &mut m.base_color_texture,
                1 => &mut m.normal_texture,
                2 => &mut m.emissive_texture,
                _ => &mut m.occlusion_texture,
            };
            if slot_ref.is_none() {
                *slot_ref = Some(r);
            }
        }
        if let (Some(w), true) = (maps.base, m.base_color_texture.is_some()) {
            m.base_color = [w, w, w, m.base_color[3]];
        }
        if let (Some(w), true) = (maps.emissive, m.emissive_texture.is_some()) {
            m.emissive = [w; 3];
        }
        // The layers, once the base colour glTF tints transmission with is
        // final.
        if let Some(names) = flavour.layers() {
            let textured = m.base_color_texture.is_some();
            let notes = read_layers(names, self.props(&pm, "Material"), &what, self.colors, textured, &mut m);
            for (i, note) in notes.into_iter().enumerate() {
                self.warn(&format!("layers-{id}-{i}"), note);
            }
        }
        Ok(m)
    }

    fn texture_ref(&mut self, tex: i64) -> Result<Option<TextureRef>, MeshError> {
        let tex = if self.kind(tex) == "LayeredTexture" {
            self.warn("layered", "layered textures were read as their first layer".into());
            match self.kids_of(tex, "Texture", None).first() {
                Some(t) => *t,
                None => return Ok(None),
            }
        } else {
            tex
        };
        if self.kind(tex) != "Texture" {
            return Ok(None);
        }
        let Some(texture) = self.texture_for(tex)? else { return Ok(None) };
        let pm = self.p70(tex)?;
        let props = self.props(&pm, "Texture");
        let uv_set = props.string("UVSet").and_then(|n| self.uv_names.get(&n).copied()).unwrap_or(0);
        // three.js applies FBX Translation/Scaling as `S·uv + T` in V-up
        // space; the scene's transform is in V-down space.
        let t = props.vec3("Translation").filter(|v| finite3(*v)).unwrap_or([0.0; 3]);
        let s = props.vec3("Scaling").filter(|v| finite3(*v)).unwrap_or([1.0; 3]);
        let rot = props.vec3("Rotation").filter(|v| finite3(*v)).unwrap_or([0.0; 3]);
        if rot.iter().any(|v| *v != 0.0) {
            self.warn("texrot", "a texture's UV rotation was not carried (three.js ignores it too)".into());
        }
        let tr = UvTransform {
            offset: [t[0] as f32, (1.0 - s[1] - t[1]) as f32],
            rotation: 0.0,
            scale: [s[0] as f32, s[1] as f32],
        };
        Ok(Some(TextureRef { texture, uv_set, transform: (!tr.is_identity()).then_some(tr) }))
    }

    fn texture_for(&mut self, tex: i64) -> Result<Option<usize>, MeshError> {
        if let Some(t) = self.texture_map.get(&tex) {
            return Ok(*t);
        }
        let video = self.kids_of(tex, "Video", None).first().copied();
        let image = self.image_for(video, tex)?;
        let out = match image {
            None => None,
            Some(image) => {
                let pm = self.p70(tex)?;
                let props = self.props(&pm, "Texture");
                let wrap = |name: &str| if props.i64(name).unwrap_or(0) == 1 { Wrap::ClampToEdge } else { Wrap::Repeat };
                let sampler = Sampler { wrap_s: wrap("WrapModeU"), wrap_t: wrap("WrapModeV"), ..Default::default() };
                self.scene.textures.push(Texture { name: Some(self.name_of(tex)).filter(|n| !n.is_empty()), image, sampler });
                Some(self.scene.textures.len() - 1)
            }
        };
        self.texture_map.insert(tex, out);
        Ok(out)
    }

    fn image_for(&mut self, video: Option<i64>, tex: i64) -> Result<Option<usize>, MeshError> {
        let key = video.unwrap_or(tex);
        if let Some(i) = self.image_map.get(&key) {
            return Ok(*i);
        }
        let dom = self.dom;
        let s = |id: i64, name: &str| -> Result<Option<String>, MeshError> {
            match self.rec(id) {
                Some(r) => Ok(dom.child_prop(r, name)?.and_then(|p| p.as_str().map(|s| s.into_owned())).filter(|s| !s.is_empty())),
                None => Ok(None),
            }
        };
        let mut bytes: Option<Vec<u8>> = None;
        if let Some(v) = video {
            if let Some(r) = self.rec(v) {
                match dom.child_prop(r, "Content")? {
                    Some(Prop::Raw(b)) if !b.is_empty() => bytes = Some(b.to_vec()),
                    Some(Prop::Str(b)) if !b.is_empty() => {
                        bytes = Some(decode_base64(&b).ok_or_else(|| malformed("an embedded texture's base64 does not decode"))?)
                    }
                    _ => {}
                }
            }
        }
        let mut names: Vec<String> = Vec::new();
        for id in [video, Some(tex)].into_iter().flatten() {
            for field in ["RelativeFilename", "Filename", "FileName"] {
                if let Some(n) = s(id, field)? {
                    names.push(n);
                }
            }
        }
        if bytes.is_none() {
            // A referenced file: its relative path first, then its bare name
            // beside the model (where SDK tools extract `.fbm` folders' files).
            'find: for n in &names {
                let unified = n.replace('\\', "/");
                let base = unified.rsplit('/').next().unwrap_or("").to_string();
                for cand in [package::normalize_relative(&unified), package::normalize_relative(&base)].into_iter().flatten() {
                    if let Some(b) = self.sidecars.get(&cand) {
                        bytes = Some(b);
                        break 'find;
                    }
                }
            }
        }
        let shown = names.first().cloned().unwrap_or_else(|| self.name_of(tex));
        let shown: String = shown.chars().take(120).collect();
        let out = match bytes {
            None => {
                self.warn(&format!("missing-{key}"), format!("texture {shown:?} is neither embedded nor supplied; it was dropped"));
                None
            }
            Some(data) => match ImageFormat::sniff(&data) {
                None => {
                    self.warn(
                        &format!("format-{key}"),
                        format!("texture {shown:?} is not PNG, JPEG, WebP or KTX2 (FBX also carries TGA, BMP, TIFF, DDS); it was dropped"),
                    );
                    None
                }
                Some(format) => {
                    self.image_bytes = self.image_bytes.saturating_add(data.len());
                    if self.image_bytes > super::MAX_INPUT_BYTES {
                        return Err(unsupported(format!("the textures exceed {} bytes in total", super::MAX_INPUT_BYTES)));
                    }
                    let name = video.map(|v| self.name_of(v)).filter(|n| !n.is_empty());
                    self.scene.images.push(Image { name, format, data });
                    Some(self.scene.images.len() - 1)
                }
            },
        };
        self.image_map.insert(key, out);
        Ok(out)
    }

    // ── animation ───────────────────────────────────────────────────────────

    fn curve(&mut self, id: i64) -> Result<Option<Curve>, MeshError> {
        if let Some(c) = self.curve_cache.get(&id) {
            return Ok(c.clone());
        }
        let c = self.decode_curve(id)?;
        self.curve_cache.insert(id, c.clone());
        Ok(c)
    }

    /// An AnimationCurve: `KeyTime` (1/46186158000 s ticks) and `KeyValueFloat`
    /// as parallel arrays, with `KeyAttrFlags` and `KeyAttrDataFloat` run-length
    /// coded by `KeyAttrRefCount`. The SDK's FbxAnimCurve and FbxAnimCurveDef;
    /// ufbx v0.23.1 `ufbxi_read_animation_curve` and `ufbxi_key_flags`
    /// (https://github.com/ufbx/ufbx/blob/26a482ae66871d7de36eb722aa060bce95bce274/ufbx.c#L14260-L14535,
    /// #L14091-L14107).
    fn decode_curve(&mut self, id: i64) -> Result<Option<Curve>, MeshError> {
        let dom = self.dom;
        let what = "animation curve".to_string();
        let Some(ta) = self.array(id, "KeyTime")? else { return Ok(None) };
        let Some(va) = self.array(id, "KeyValueFloat")?.or(self.array(id, "KeyValueDouble")?) else { return Ok(None) };
        let ticks = dom.i64s(&ta, "KeyTime")?;
        let values = dom.f64s(&va, "KeyValueFloat")?;
        if ticks.len() != values.len() {
            return Err(malformed(format!("{what} has {} times and {} values", ticks.len(), values.len())));
        }
        if ticks.is_empty() {
            return Ok(None);
        }
        self.anim_keys = self.anim_keys.saturating_add(ticks.len());
        if self.anim_keys > MAX_ANIMATION_KEYS {
            return Err(unsupported(format!("the animation has more than {MAX_ANIMATION_KEYS} keys")));
        }
        if ticks.windows(2).any(|w| w[1] < w[0]) {
            return Err(malformed(format!("{what} key times go backwards")));
        }
        if values.iter().any(|v| !v.is_finite()) {
            return Err(malformed(format!("{what} has a non-finite value")));
        }
        let times: Vec<f64> = ticks.iter().map(|t| *t as f64 / self.tick_rate).collect();
        let n = times.len();
        let flags = match self.array(id, "KeyAttrFlags")? {
            Some(a) => dom.i32s(&a, "KeyAttrFlags")?,
            None => Vec::new(),
        };
        let data = match self.array(id, "KeyAttrDataFloat")? {
            Some(a) => dom.key_attr_floats(&a, "KeyAttrDataFloat")?,
            None => Vec::new(),
        };
        let refs = match self.array(id, "KeyAttrRefCount")? {
            Some(a) => dom.i32s(&a, "KeyAttrRefCount")?,
            None => Vec::new(),
        };
        if !flags.is_empty() && refs.len() != flags.len() {
            return Err(malformed(format!("{what} has {} attribute flags for {} runs", flags.len(), refs.len())));
        }
        // Which attribute run each key belongs to.
        let mut attr = vec![0usize; n];
        if !flags.is_empty() {
            let mut k = 0usize;
            for (run, count) in refs.iter().enumerate() {
                let count = usize::try_from(*count).map_err(|_| malformed(format!("{what} has a negative run length")))?;
                // Only the keys still unassigned: a declared count past them
                // covers nothing (ufbx consumes runs per key).
                let covered = count.min(n - k);
                attr[k..k + covered].fill(run);
                k += covered;
            }
            // Keys past the declared runs keep the last run's attributes.
            for a in attr.iter_mut().skip(k) {
                *a = flags.len() - 1;
            }
        }
        let flag = |k: usize| flags.get(attr[k]).copied().unwrap_or(0x4);
        let datum = |k: usize, j: usize| data.get(attr[k] * 4 + j).copied().unwrap_or(0.0) as f64;
        let packed = |k: usize| data.get(attr[k] * 4 + 2).map_or(0, |x| x.to_bits());
        // Each key's interpolation and (left, right) tangents, decoded as
        // ufbx does (`ufbxi_read_animation_curve`): a key's four attributes
        // are its right slope, the NEXT key's left slope, packed handle
        // weights and velocities, and the SDK leaves auto and TCB tangents
        // to be computed from the neighbouring keys.
        #[derive(Clone, Copy)]
        enum Interp {
            Constant(bool),
            Linear,
            Cubic,
        }
        struct Key {
            interp: Interp,
            left: f64,
            wl: f64,
            right: f64,
            wr: f64,
        }
        let mut keys: Vec<Key> = Vec::with_capacity(n);
        let (mut slope_left, mut weight_left) = (0.0f64, DEFAULT_WEIGHT);
        for i in 0..n {
            let f = flag(i);
            let (mut right, mut next_left) = (datum(i, 0), datum(i, 1));
            let (mut wr, mut next_wl) = (DEFAULT_WEIGHT, DEFAULT_WEIGHT);
            if f & (FLAG_WEIGHTED_RIGHT | FLAG_WEIGHTED_NEXT_LEFT) != 0 {
                // Two decimal fixed-point weights (units of 1/10000) packed
                // into the bits of the third attribute.
                let bits = packed(i);
                if f & FLAG_WEIGHTED_RIGHT != 0 {
                    wr = (bits & 0xffff) as f64 * 1e-4;
                }
                if f & FLAG_WEIGHTED_NEXT_LEFT != 0 {
                    next_wl = (bits >> 16) as f64 * 1e-4;
                }
            }
            let (t, v) = (times[i], values[i]);
            let prev = (i > 0 && t > times[i - 1]).then(|| (times[i - 1], values[i - 1]));
            let next = (i + 1 < n && times[i + 1] > t).then(|| (times[i + 1], values[i + 1]));
            let interp = if f & FLAG_CONSTANT != 0 {
                (wr, next_wl, right, next_left) = (DEFAULT_WEIGHT, DEFAULT_WEIGHT, 0.0, 0.0);
                Interp::Constant(f & FLAG_CONSTANT_NEXT != 0)
            } else if f & FLAG_CUBIC != 0 {
                if f & FLAG_TANGENT_TCB != 0 {
                    let sl = prev.map_or(0.0, |(tp, vp)| (v - vp) / (t - tp));
                    let sr = next.map_or(0.0, |(tn, vn)| (vn - v) / (tn - t));
                    let edge = prev.is_none() || next.is_none();
                    (slope_left, right) = tcb_slopes(datum(i, 0), datum(i, 1), datum(i, 2), sl, sr, edge);
                    (next_left, next_wl) = (0.0, DEFAULT_WEIGHT);
                } else if f & FLAG_TANGENT_USER == 0 {
                    // Auto tangents: the stored values are the auto bias.
                    (slope_left, right) = match (prev, next) {
                        (Some((tp, vp)), Some((tn, vn))) => {
                            let (ts, vs) = ([tp, t, tn], [vp, v, vn]);
                            if (slope_left + right).abs() <= 1e-4 {
                                let s = auto_tangent(ts, vs, weight_left, wr, right, f);
                                (s, s)
                            } else {
                                (
                                    auto_tangent(ts, vs, weight_left, wr, -slope_left, f),
                                    auto_tangent(ts, vs, weight_left, wr, right, f),
                                )
                            }
                        }
                        (Some((tp, vp)), None) => {
                            let s = auto_tangent_end(t - tp, v - vp, -slope_left, f);
                            (s, s)
                        }
                        (None, Some((tn, vn))) => {
                            let s = auto_tangent_end(tn - t, vn - v, right, f);
                            (s, s)
                        }
                        (None, None) => (0.0, 0.0),
                    };
                }
                // User tangents keep the stored slopes as they are.
                Interp::Cubic
            } else {
                // Linear, or a mode this reader does not know.
                let s = next.map_or(0.0, |(tn, vn)| (vn - v) / (tn - t));
                (wr, next_wl, right, next_left) = (DEFAULT_WEIGHT, DEFAULT_WEIGHT, s, s);
                Interp::Linear
            };
            keys.push(Key { interp, left: slope_left, wl: weight_left, right, wr });
            (slope_left, weight_left) = (next_left, next_wl);
        }
        let seg: Vec<Seg> = (0..n)
            .map(|k| match keys[k].interp {
                Interp::Constant(next) => Seg::Constant { next },
                Interp::Cubic if k + 1 < n => {
                    // Handles longer than their interval would let the
                    // curve's time run backwards.
                    let (right, wr) = (keys[k].right, keys[k].wr.clamp(0.0, 1.0));
                    let (next_left, wl) = (keys[k + 1].left, keys[k + 1].wl.clamp(0.0, 1.0));
                    if [right, next_left, wr, wl].iter().all(|x| x.is_finite()) {
                        Seg::Cubic { right, next_left, wr, wl }
                    } else {
                        Seg::Linear
                    }
                }
                Interp::Cubic | Interp::Linear => Seg::Linear,
            })
            .collect();
        let (lo, hi) = values.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), v| (l.min(*v), h.max(*v)));
        let tol = RESAMPLE_TOLERANCE * (hi - lo).max(1e-3);
        Ok(Some(Curve { times, values, seg, tol }))
    }

    fn animation(&mut self) -> Result<(), MeshError> {
        let dom = self.dom;
        let stacks: Vec<i64> = self.order.iter().copied().filter(|id| self.kind(*id) == "AnimationStack").collect();
        if stacks.is_empty() {
            return Ok(());
        }
        let has_curves = |r: &Self, stack: i64| {
            r.kids_of(stack, "AnimationLayer", None).iter().any(|l| !r.kids_of(*l, "AnimationCurveNode", None).is_empty())
        };
        let active = match dom.child(ROOT, "Documents").and_then(|d| dom.child(d, "Document")) {
            Some(doc) => read_p70(dom, doc)?.get("ActiveAnimStackName").and_then(|v| v.first()).and_then(|p| p.as_str()).map(|s| s.into_owned()),
            None => None,
        };
        let chosen = stacks
            .iter()
            .copied()
            .find(|s| active.as_deref() == Some(self.name_of(*s).as_str()) && has_curves(self, *s))
            .or_else(|| stacks.iter().copied().find(|s| has_curves(self, *s)));
        let Some(active) = chosen else { return Ok(()) };
        // Every stack is a clip: the active one first (what a one-clip
        // container keeps), then the rest in the file's order. FBX writers,
        // ours included, write one stack per clip.
        let order: Vec<i64> = std::iter::once(active)
            .chain(stacks.iter().copied().filter(|s| *s != active && has_curves(self, *s)))
            .collect();
        for stack in order {
            self.stack_animation(stack)?;
        }
        Ok(())
    }

    /// One animation stack as one clip.
    fn stack_animation(&mut self, stack: i64) -> Result<(), MeshError> {
        let layers = self.kids_of(stack, "AnimationLayer", None);
        let layers: Vec<i64> = layers.into_iter().filter(|l| !self.kids_of(*l, "AnimationCurveNode", None).is_empty()).collect();
        let Some(&layer) = layers.first() else { return Ok(()) };
        if layers.len() > 1 {
            self.warn("layers", format!("{} animation layers would blend; only the first was carried", layers.len()));
        }
        // Gather curve nodes by what they drive.
        let mut node_curves: BTreeMap<usize, [[Option<Curve>; 3]; 3]> = BTreeMap::new();
        let mut node_defaults: HashMap<(usize, usize), [f64; 3]> = HashMap::new();
        let mut channel_curves: HashMap<i64, Curve> = HashMap::new();
        let mut unmodelled: BTreeMap<String, usize> = BTreeMap::new();
        for cn in self.kids_of(layer, "AnimationCurveNode", None) {
            let target = self.ups(cn).iter().find(|(_, p)| p.is_some()).cloned();
            let Some((obj, Some(prop))) = target else { continue };
            let curves: Vec<(i64, String)> = self
                .kids(cn)
                .iter()
                .filter(|(c, p)| p.is_some() && self.kind(*c) == "AnimationCurve")
                .map(|(c, p)| (*c, p.clone().unwrap_or_default()))
                .collect();
            let path = match prop.as_str() {
                "Lcl Translation" => Some(0usize),
                "Lcl Rotation" => Some(1),
                // Assimp's exporter (6.0) names the scale property "Lcl Scale"
                // — a bug, but an unambiguous one (the curve node is "S", and
                // three.js, which keys on that name, plays it).
                "Lcl Scaling" | "Lcl Scale" => Some(2),
                _ => None,
            };
            match (self.kind(obj), path) {
                ("Model", Some(path)) => {
                    let Some(&node) = self.node_of.get(&obj) else { continue };
                    let pm = self.p70(cn)?;
                    let props = self.props(&pm, "AnimationCurveNode");
                    let entry = node_curves.entry(node).or_insert([[None, None, None], [None, None, None], [None, None, None]]);
                    let mut defaults = [f64::NAN; 3];
                    for (axis, label) in ["d|X", "d|Y", "d|Z"].iter().enumerate() {
                        defaults[axis] = props.f64(label).filter(|v| v.is_finite()).unwrap_or(f64::NAN);
                    }
                    for (c, label) in curves {
                        let axis = match label.as_str() {
                            "d|X" | "X" => 0,
                            "d|Y" | "Y" => 1,
                            "d|Z" | "Z" => 2,
                            _ => continue,
                        };
                        entry[path][axis] = self.curve(c)?;
                    }
                    node_defaults.insert((node, path), defaults);
                }
                ("Deformer", _) if prop == "DeformPercent" => {
                    if let Some((c, _)) = curves.into_iter().find(|(_, l)| l.contains("DeformPercent")) {
                        if let Some(curve) = self.curve(c)? {
                            channel_curves.insert(obj, curve);
                        }
                    }
                }
                (kind, _) => {
                    *unmodelled.entry(format!("{prop} of a {kind}")).or_insert(0) += 1;
                }
            }
        }
        if !unmodelled.is_empty() {
            let list: Vec<String> = unmodelled.iter().map(|(k, n)| format!("{k} ({n})")).collect();
            self.warn("unmodelled", format!("animated properties the scene does not model were not carried: {}", list.join(", ")));
        }
        let mut channels: Vec<Channel> = Vec::new();
        // Ancestors whose scale animates make inherit types 0/2 time-dependent.
        let scale_animated: HashSet<usize> = node_curves.iter().filter(|(_, c)| c[2].iter().any(|x| x.is_some())).map(|(n, _)| *n).collect();
        // Per model: it, or an ancestor, animates its scale. Remembered along
        // every walk, so a chain is walked once rather than once per node.
        let mut scaled_at_or_above: HashMap<i64, bool> = HashMap::new();
        for (node, curves) in &node_curves {
            let model = self.model_of_node[*node];
            let mut complex = self.complex.contains(&model);
            if !complex && self.xf.get(&model).is_some_and(|x| x.inherit != 1) {
                let mut walked = Vec::new();
                let mut cur = self.model_parent.get(&model).copied();
                let mut found = false;
                while let Some(p) = cur {
                    if let Some(&known) = scaled_at_or_above.get(&p) {
                        found = known;
                        break;
                    }
                    if self.node_of.get(&p).is_some_and(|n| scale_animated.contains(n)) {
                        scaled_at_or_above.insert(p, true);
                        found = true;
                        break;
                    }
                    walked.push(p);
                    if walked.len() > self.model_parent.len() {
                        break;
                    }
                    cur = self.model_parent.get(&p).copied();
                }
                for p in walked {
                    scaled_at_or_above.insert(p, found);
                }
                complex = found;
            }
            let defaults: [[f64; 3]; 3] = [0, 1, 2].map(|k| node_defaults.get(&(*node, k)).copied().unwrap_or([f64::NAN; 3]));
            if complex {
                channels.extend(self.complex_channels(*node, &node_curves, &node_defaults)?);
            } else {
                channels.extend(self.simple_channels(*node, curves, &defaults)?);
            }
        }
        // Blend-shape weights, per node holding a mesh that has them.
        let mut nodes_of_mesh: HashMap<usize, Vec<usize>> = HashMap::new();
        for (n, node) in self.scene.nodes.iter().enumerate() {
            if let Some(m) = node.mesh {
                nodes_of_mesh.entry(m).or_default().push(n);
            }
        }
        for (mi, chans) in self.mesh_channels.clone() {
            if !chans.iter().any(|c| channel_curves.contains_key(&c.id)) {
                continue;
            }
            let ntargets = self.scene.meshes[mi].weights.len();
            let curves: Vec<Option<&Curve>> = chans.iter().map(|c| channel_curves.get(&c.id)).collect();
            let present: Vec<&Curve> = curves.iter().flatten().copied().collect();
            let step = present.iter().all(|c| c.all_constant());
            let mut times = self.sample_times(&present, step, None, false)?;
            if !step {
                // In-between shares bend where the channel weight passes a
                // shape's full weight: key those crossings too.
                let mut extra = Vec::new();
                // Charged as each crossing is found, not after the search: the
                // search is bounded only by sample pairs x full weights, far
                // past the key budget, and every crossing costs a 48-step
                // bisection.
                let budget = MAX_ANIMATION_KEYS.saturating_sub(self.anim_keys);
                for (c, curve) in chans.iter().zip(&curves) {
                    let Some(curve) = curve.filter(|_| c.full.len() > 1) else { continue };
                    let w = |t: f64| curve.eval(t) / 100.0;
                    for pair in times.windows(2) {
                        let (a, b) = (pair[0], pair[1]);
                        let (wa, wb) = (w(a), w(b));
                        for &bp in &c.full {
                            if (wa - bp) * (wb - bp) < 0.0 {
                                if extra.len() >= budget {
                                    return Err(unsupported(format!("the resampled animation passes {MAX_ANIMATION_KEYS} keys")));
                                }
                                let (mut lo, mut hi) = (a, b);
                                for _ in 0..48 {
                                    let mid = 0.5 * (lo + hi);
                                    if (w(lo) - bp) * (w(mid) - bp) <= 0.0 {
                                        hi = mid;
                                    } else {
                                        lo = mid;
                                    }
                                }
                                extra.push(0.5 * (lo + hi));
                            }
                        }
                    }
                }
                if !extra.is_empty() {
                    self.anim_keys = self.anim_keys.saturating_add(extra.len());
                    if self.anim_keys > MAX_ANIMATION_KEYS {
                        return Err(unsupported(format!("the resampled animation passes {MAX_ANIMATION_KEYS} keys")));
                    }
                    times.extend(extra);
                    times.sort_by(|a, b| a.total_cmp(b));
                    times.dedup();
                }
            }
            // Every channel's weight at every time, once per node holding the
            // mesh: charged before anything is allocated, since nothing else
            // bounds times × channels (1,024 channels and one long cubic
            // curve asked for 651 MB).
            let holders = nodes_of_mesh.get(&mi).map_or(0, Vec::len).max(1);
            let need = times.len().checked_mul(ntargets).and_then(|v| v.checked_mul(holders)).unwrap_or(usize::MAX);
            if self.morph_values.saturating_add(need) > MAX_MORPH_VALUES {
                return Err(unsupported(format!("the blend-shape weight animation expands past {MAX_MORPH_VALUES} values")));
            }
            self.morph_values += need;
            let mut values = vec![0.0f32; times.len() * ntargets];
            for (i, t) in times.iter().enumerate() {
                for (c, curve) in chans.iter().zip(&curves) {
                    let w = curve.map_or(c.default, |k| k.at(*t, step) / 100.0);
                    for (k, share) in inbetween_weights(&c.full, w).into_iter().enumerate() {
                        if let Some(v) = values.get_mut(i * ntargets + c.first + k) {
                            *v = share as f32;
                        }
                    }
                }
            }
            for &n in nodes_of_mesh.get(&mi).map_or(&[][..], |v| v.as_slice()) {
                if let Some(ch) = finish_channel(n, AnimPath::Weights, step, &times, values.clone(), ntargets) {
                    channels.push(ch);
                }
            }
        }
        if !channels.is_empty() {
            let name = Some(self.name_of(stack)).filter(|n| !n.is_empty());
            // A take with pre-roll keys before 0; the scene's clock starts
            // there (glTF 2.0 §3.11), as the USD reader's does.
            if let Some(lo) = super::pose::start_at_zero(&mut channels) {
                self.warn(
                    &format!("preroll-{stack}"),
                    format!("animation stack {:?} starts at {lo}s; it was shifted to start at 0", name.as_deref().unwrap_or("")),
                );
            }
            self.scene.animations.push(Animation { name, channels });
        }
        Ok(())
    }

    /// Key times for a set of curves: the union of their keys, plus samples
    /// inside segments the scene cannot interpolate the same way. `euler`
    /// (when given) is the rotation's three curves, for Euler-specific
    /// subdivision.
    fn sample_times(
        &mut self,
        curves: &[&Curve],
        step: bool,
        euler: Option<&dyn Fn(f64) -> [f64; 3]>,
        arcs: bool,
    ) -> Result<Vec<f64>, MeshError> {
        let mut u: Vec<f64> = curves.iter().flat_map(|c| c.times.iter().copied()).collect();
        u.sort_by(|a, b| a.total_cmp(b));
        u.dedup();
        if step {
            return Ok(u);
        }
        let mut out = Vec::with_capacity(u.len());
        for w in u.windows(2) {
            let (a, b) = (w[0], w[1]);
            out.push(a);
            // Held values inside a linear channel: key them just after a
            // jump to the next key's value, and just before a jump at the
            // next key, so linear interpolation reproduces the hold.
            let eps = HOLD_EPSILON.min((b - a) / 4.0);
            let held = |next: bool| {
                curves.iter().any(|c| match c.seg_over(a, b) {
                    Some(Seg::Constant { next: n }) if n == next => {
                        if next {
                            c.eval(a) != c.eval_right(a)
                        } else {
                            c.eval(a) != c.eval(b)
                        }
                    }
                    _ => false,
                })
            };
            let (jump_after_a, hold_before_b) = (held(true), held(false));
            if jump_after_a {
                out.push(a + eps);
            }
            let mut n = 1usize;
            for c in curves {
                if let Some(Seg::Cubic { .. }) = c.seg_over(a, b) {
                    n = n.max(((b - a) * RESAMPLE_HZ).ceil() as usize);
                }
            }
            if let Some(e) = euler {
                let (ea, eb) = (e(a), e(b));
                let d: Vec<f64> = (0..3).map(|k| (eb[k] - ea[k]).abs()).collect();
                let moving = d.iter().filter(|x| **x > 1e-9).count();
                let max = d.iter().cloned().fold(0.0, f64::max);
                if arcs && moving >= 1 {
                    n = n.max((max / PIVOT_STEP_DEG).ceil() as usize);
                } else if moving >= 2 {
                    n = n.max((max / MULTI_AXIS_STEP_DEG).ceil() as usize);
                } else if moving == 1 {
                    n = n.max((max / SINGLE_AXIS_STEP_DEG).ceil() as usize);
                }
            }
            let n = n.min(MAX_SUBDIVISIONS);
            // Checked as the list grows, not after: a hostile curve can ask
            // for a thousand samples in each of millions of segments.
            if self.anim_keys.saturating_add(out.len()).saturating_add(n) > MAX_ANIMATION_KEYS {
                return Err(unsupported(format!("the resampled animation passes {MAX_ANIMATION_KEYS} keys")));
            }
            let cubic: Vec<&Curve> = curves.iter().copied().filter(|c| matches!(c.seg_over(a, b), Some(Seg::Cubic { .. }))).collect();
            let mut prev = a;
            for k in 1..=n {
                let x = if k == n { b } else { a + (b - a) * k as f64 / n as f64 };
                if !cubic.is_empty() {
                    refine(&cubic, prev, x, REFINE_DEPTH, &mut out);
                }
                if k < n && x > *out.last().unwrap_or(&a) {
                    out.push(x);
                }
                prev = x;
            }
            if hold_before_b {
                let h = b - eps;
                if h > *out.last().unwrap_or(&a) {
                    out.push(h);
                }
            }
            if self.anim_keys.saturating_add(out.len()) > MAX_ANIMATION_KEYS {
                return Err(unsupported(format!("the resampled animation passes {MAX_ANIMATION_KEYS} keys")));
            }
        }
        if let Some(last) = u.last() {
            out.push(*last);
        }
        self.anim_keys = self.anim_keys.saturating_add(out.len());
        if self.anim_keys > MAX_ANIMATION_KEYS {
            return Err(unsupported(format!("the resampled animation passes {MAX_ANIMATION_KEYS} keys")));
        }
        Ok(out)
    }

    /// Channels for a node whose IR transform is its FBX local's T, R, S
    /// directly: each path from its own curves.
    fn simple_channels(&mut self, node: usize, curves: &[[Option<Curve>; 3]; 3], defaults: &[[f64; 3]; 3]) -> Result<Vec<Channel>, MeshError> {
        let model = self.model_of_node[node];
        let Some(xf) = self.xf.get(&model).cloned() else { return Ok(Vec::new()) };
        let statics = [xf.t, xf.r, xf.s];
        let mut out = Vec::new();
        for path in 0..3 {
            let present: Vec<&Curve> = curves[path].iter().flatten().collect();
            if present.is_empty() {
                continue;
            }
            let step = present.iter().all(|c| c.all_constant());
            let value = |axis: usize, t: f64| -> f64 {
                match &curves[path][axis] {
                    Some(c) => c.at(t, step),
                    None => {
                        let d = defaults[path][axis];
                        if d.is_finite() {
                            d
                        } else {
                            statics[path][axis]
                        }
                    }
                }
            };
            let euler_fn = |t: f64| [value(0, t), value(1, t), value(2, t)];
            let times = if path == 1 {
                self.sample_times(&present, step, Some(&euler_fn), false)?
            } else {
                self.sample_times(&present, step, None, false)?
            };
            let mut values = Vec::with_capacity(times.len() * 4);
            let mut prev_q: Option<[f64; 4]> = None;
            for t in &times {
                let v = [value(0, *t), value(1, *t), value(2, *t)];
                match path {
                    1 => {
                        let mut q = xf.rotation(v);
                        if prev_q.is_some_and(|p| math::quat_dot(p, q) < 0.0) {
                            q = [-q[0], -q[1], -q[2], -q[3]];
                        }
                        prev_q = Some(q);
                        values.extend(q.iter().map(|x| *x as f32));
                    }
                    _ => values.extend(v.iter().map(|x| *x as f32)),
                }
            }
            let ap = [AnimPath::Translation, AnimPath::Rotation, AnimPath::Scale][path];
            if let Some(ch) = finish_channel(node, ap, step, &times, values, if path == 1 { 4 } else { 3 }) {
                out.push(ch);
            }
        }
        Ok(out)
    }

    /// Channels for a node whose IR local is derived from world matrices
    /// (pivots, or inherit types that differ from composition): every path,
    /// from every curve that can move it.
    fn complex_channels(
        &mut self,
        node: usize,
        all: &BTreeMap<usize, [[Option<Curve>; 3]; 3]>,
        defaults: &HashMap<(usize, usize), [f64; 3]>,
    ) -> Result<Vec<Channel>, MeshError> {
        let model = self.model_of_node[node];
        // The node and its ancestors, root first.
        let mut chain = vec![model];
        let mut cur = self.model_parent.get(&model).copied();
        while let Some(p) = cur {
            chain.push(p);
            if chain.len() > MAX_NODES {
                break;
            }
            cur = self.model_parent.get(&p).copied();
        }
        chain.reverse();
        let mut present: Vec<&Curve> = Vec::new();
        for m in &chain {
            if let Some(cs) = self.node_of.get(m).and_then(|n| all.get(n)) {
                present.extend(cs.iter().flatten().flatten());
            }
        }
        let step = present.iter().all(|c| c.all_constant());
        let rot = all.get(&node);
        let xf_self = self.xf.get(&model).cloned();
        let euler_fn = |t: f64| -> [f64; 3] {
            let base = xf_self.as_ref().map_or([0.0; 3], |x| x.r);
            let mut e = base;
            if let Some(cs) = rot {
                for (k, c) in cs[1].iter().enumerate() {
                    if let Some(c) = c {
                        e[k] = c.eval(t);
                    }
                }
            }
            e
        };
        let present_owned: Vec<Curve> = present.iter().map(|c| (*c).clone()).collect();
        let refs: Vec<&Curve> = present_owned.iter().collect();
        let times = self.sample_times(&refs, step, Some(&euler_fn), true)?;
        self.chain_work = self.chain_work.saturating_add(times.len().saturating_mul(chain.len()));
        if self.chain_work > MAX_CHAIN_WORK {
            return Err(unsupported(
                "the animation keys too many nodes whose transforms must be re-evaluated through deep inherit-type chains",
            ));
        }
        let eval_local = |r: &Self, m: i64, t: f64| -> Mat4 {
            let Some(xf) = r.xf.get(&m) else { return math::IDENTITY };
            let Some(n) = r.node_of.get(&m) else { return math::IDENTITY };
            let cs = all.get(n);
            let comp = |path: usize, stat: [f64; 3]| -> [f64; 3] {
                let mut v = stat;
                if let Some(cs) = cs {
                    for axis in 0..3 {
                        match &cs[path][axis] {
                            Some(c) => v[axis] = c.at(t, step),
                            None => {
                                if let Some(d) = defaults.get(&(*n, path)).map(|d| d[axis]).filter(|d| d.is_finite()) {
                                    if cs[path].iter().any(|x| x.is_some()) {
                                        v[axis] = d;
                                    }
                                }
                            }
                        }
                    }
                }
                v
            };
            xf.local(comp(0, xf.t), comp(1, xf.r), comp(2, xf.s))
        };
        let mut tv = Vec::with_capacity(times.len() * 3);
        let mut rv = Vec::with_capacity(times.len() * 4);
        let mut sv = Vec::with_capacity(times.len() * 3);
        let mut prev_q: Option<[f64; 4]> = None;
        let mut sheared = false;
        for t in &times {
            let mut world = math::IDENTITY;
            let mut parent_world = math::IDENTITY;
            let mut parent_local = math::IDENTITY;
            let mut local = math::IDENTITY;
            for m in &chain {
                local = eval_local(self, *m, *t);
                let inherit = self.xf.get(m).map_or(1, |x| x.inherit);
                parent_world = world;
                world = fbx_world(&parent_world, &parent_local, &local, inherit);
                parent_local = local;
            }
            let _ = local;
            let ir_local = match math::invert(&parent_world) {
                Some(inv) => math::mat_mul(&inv, &world),
                None => world,
            };
            sheared |= math::decompose(&ir_local).is_none();
            let (tr, mut q, sc) = trs_lossy(&ir_local);
            if prev_q.is_some_and(|p| math::quat_dot(p, q) < 0.0) {
                q = [-q[0], -q[1], -q[2], -q[3]];
            }
            prev_q = Some(q);
            tv.extend(tr.iter().map(|x| *x as f32));
            rv.extend(q.iter().map(|x| *x as f32));
            sv.extend(sc.iter().map(|x| *x as f32));
        }
        if sheared {
            self.warn("shear", "an animated node's transform shears at some keys; the shear was dropped".into());
        }
        let mut out = Vec::new();
        for (path, values, stride) in [(AnimPath::Translation, tv, 3), (AnimPath::Rotation, rv, 4), (AnimPath::Scale, sv, 3)] {
            if let Some(ch) = finish_channel(node, path, step, &times, values, stride) {
                out.push(ch);
            }
        }
        Ok(out)
    }

    // ── lights and cameras ──────────────────────────────────────────────────

    /// Places each model's first light and first camera (see the module
    /// doc), then turns the frames of the nodes that hold them, at rest and
    /// at every key, from FBX's to the scene's (`fbx::reframe` with each
    /// `fbx::Turn`'s inverse), so that every other node stays where it is.
    /// What this builds is bounded by the node budget: every light and
    /// camera a model places, and a model places one of each at most, and
    /// `models` charged each model against `MAX_NODES` before building any
    /// node; a child node made for one is charged against the same budget
    /// before it is made.
    fn lights_and_cameras(&mut self) {
        let joints = self.scene.joint_nodes();
        let mut turns: Vec<Option<Turn>> = vec![None; self.scene.nodes.len()];
        // `finish` adds a root for the axis and unit conversion after this:
        // a child made here must leave it room.
        let budget = MAX_NODES - usize::from(!is_identity(&self.correction));
        for ni in 0..self.model_of_node.len() {
            let m = self.model_of_node[ni];
            let attributes: Vec<(i64, bool)> = self
                .kids(m)
                .iter()
                .filter(|(c, p)| p.is_none() && self.kind(*c) == "NodeAttribute")
                .filter_map(|(c, _)| match self.sub(*c) {
                    "Light" => Some((*c, true)),
                    "Camera" => Some((*c, false)),
                    _ => None,
                })
                .collect();
            let light = attributes.iter().find(|a| a.1).map(|a| a.0);
            let camera = attributes.iter().find(|a| !a.1).map(|a| a.0);
            if attributes.len() > usize::from(light.is_some()) + usize::from(camera.is_some()) {
                let model = quoted_name(&self.name_of(m));
                let n = attributes.len();
                // Keyed by the model, so each such model is named, once.
                self.warn(&format!("extra-attributes {m}"), format!("model \"{model}\" has {n} light and camera attributes; only its first light and first camera were read"));
            }
            // What stays on the model's node: nothing beside its mesh or a
            // joint, and of a light and a camera the one its class names (a
            // Camera model's camera, else the light). The other goes on a
            // child node of its own, which carries the turn back.
            let crowded = self.scene.nodes[ni].mesh.is_some() || joints.get(ni).copied().unwrap_or(false);
            let keeps_camera = camera.is_some() && (light.is_none() || self.sub(m) == "Camera");
            for (attribute, is_light) in [(light, true), (camera, false)] {
                let Some(a) = attribute else { continue };
                let (kind, turn) = if is_light { ("light", Turn::LIGHT) } else { ("camera", Turn::CAMERA) };
                let on_node = !crowded && is_light != keeps_camera;
                if !on_node && self.scene.nodes.len() >= budget {
                    let label = self.attribute_label(a, m, kind);
                    self.warnings.push(format!(
                        "{label} is dropped: it shares its model with a mesh, a joint or a light, so it needs a node of its own, past the scene's {MAX_NODES}"
                    ));
                    continue;
                }
                let index = if is_light { self.light_for(a, m) } else { self.camera_for(a, m) };
                let Some(index) = index else { continue };
                let place = |n: &mut Node| {
                    if is_light {
                        n.light = Some(index);
                    } else {
                        n.camera = Some(index);
                    }
                };
                if on_node {
                    place(&mut self.scene.nodes[ni]);
                    turns[ni] = Some(turn.inverse());
                } else {
                    let name = self.scene.nodes[ni].name.as_ref().map(|n| format!("{n}_{kind}"));
                    let transform = Transform::Trs { translation: [0.0; 3], rotation: turn.inverse().quat(), scale: [1.0; 3] };
                    let mut child = Node { name, transform, ..Default::default() };
                    place(&mut child);
                    self.scene.nodes.push(child);
                    let c = self.scene.nodes.len() - 1;
                    self.scene.nodes[ni].children.push(c);
                    turns.push(None);
                }
            }
        }
        self.unplaced_lights_and_cameras();
        reframe(&mut self.scene, &turns);
    }

    /// A light or camera as a warning names it: by its attribute's name,
    /// else its model's, quoted and cut at [`MAX_QUOTED_NAME`] (Ruling A45).
    fn attribute_label(&self, a: i64, m: i64, kind: &str) -> String {
        let name = self.name_of(a);
        let name = if name.is_empty() { self.name_of(m) } else { name };
        format!("{kind} \"{}\"", quoted_name(&name))
    }

    /// The scene's index of light attribute `a`, which model `m` places:
    /// read once however many models place it, and `None`, with a warning
    /// naming it and why, when it is dropped.
    fn light_for(&mut self, a: i64, m: i64) -> Option<usize> {
        if let Some(&kept) = self.light_map.get(&a) {
            return kept;
        }
        let label = self.attribute_label(a, m, "light");
        let name = Some(self.name_of(a)).filter(|n| !n.is_empty());
        let read = match self.p70(a) {
            Ok(pm) => read_light(Props { own: &pm, tmpl: self.class_templates.get("FbxLight") }, name, self.unit, self.colors, self.light_units, &label),
            Err(e) => Err(format!("its properties do not read ({e})")),
        };
        let kept = match read {
            Ok((light, notes)) => {
                self.warnings.extend(notes);
                self.scene.lights.push(light);
                Some(self.scene.lights.len() - 1)
            }
            Err(why) => {
                self.warnings.push(format!("{label} is dropped: {why}"));
                None
            }
        };
        self.light_map.insert(a, kept);
        kept
    }

    /// [`Reader::light_for`] for a camera attribute.
    fn camera_for(&mut self, a: i64, m: i64) -> Option<usize> {
        if let Some(&kept) = self.camera_map.get(&a) {
            return kept;
        }
        let label = self.attribute_label(a, m, "camera");
        let name = Some(self.name_of(a)).filter(|n| !n.is_empty());
        let blender = blender_exporter(&self.creator);
        let read = match self.p70(a) {
            Ok(pm) => read_camera(Props { own: &pm, tmpl: self.class_templates.get("FbxCamera") }, name, self.unit, blender, &label),
            Err(e) => Err(format!("its properties do not read ({e})")),
        };
        let kept = match read {
            Ok((camera, notes)) => {
                self.warnings.extend(notes);
                self.scene.cameras.push(camera);
                Some(self.scene.cameras.len() - 1)
            }
            Err(why) => {
                self.warnings.push(format!("{label} is dropped: {why}"));
                None
            }
        };
        self.camera_map.insert(a, kept);
        kept
    }

    /// A warning for each light and camera attribute that no model holds:
    /// each is dropped. They are named one by one while the node budget
    /// lasts (`MAX_NODES`, as many as a scene can place), and one more
    /// warning counts any past it, as glTF's reader does, so a file of
    /// millions cannot make millions of warnings.
    fn unplaced_lights_and_cameras(&mut self) {
        let mut notes = Vec::new();
        let mut more = [0usize; 2];
        for &a in &self.order {
            let is_light = match (self.kind(a), self.sub(a)) {
                ("NodeAttribute", "Light") => true,
                ("NodeAttribute", "Camera") => false,
                _ => continue,
            };
            if self.ups(a).iter().any(|(p, prop)| prop.is_none() && self.kind(*p) == "Model") {
                continue;
            }
            let kind = if is_light { "light" } else { "camera" };
            if notes.len() < MAX_NODES {
                let name = self.name_of(a);
                let label = if name.is_empty() { format!("{kind} {a}") } else { format!("{kind} \"{}\"", quoted_name(&name)) };
                notes.push(format!("{label} is placed by no model, so it is dropped"));
            } else {
                more[usize::from(!is_light)] += 1;
            }
        }
        for (n, kind) in more.iter().zip(["light", "camera"]) {
            if *n > 0 {
                notes.push(format!("{n} more {kind}(s) are placed by no model, so they are dropped"));
            }
        }
        self.warnings.extend(notes);
    }

    fn finish(mut self) -> Result<SceneRead, MeshError> {
        if !is_identity(&self.correction) {
            // One root carrying the axis and unit conversion, so everything
            // below it — skins' inverse binds, animation keys — stays in the
            // file's own units and stays consistent.
            let children = std::mem::take(&mut self.scene.roots);
            self.scene.nodes.push(Node {
                name: Some("FbxAxisUnitConversion".into()),
                transform: Transform::from_matrix(self.correction),
                children,
                ..Default::default()
            });
            self.scene.roots = vec![self.scene.nodes.len() - 1];
        }
        if let Some(doc) = self.dom.child(ROOT, "Documents").and_then(|d| self.dom.child(d, "Document")) {
            let props = self.dom.props(doc)?;
            self.scene.name = props.get(1).and_then(|p| p.as_str()).map(|s| s.into_owned()).filter(|s| !s.is_empty());
        }
        if self.scene.drawable_count() == 0 && self.non_mesh_geometry > 0 {
            return Err(unsupported(
                "the file's only geometry is NURBS, patches or curves, which this converter does not tessellate",
            ));
        }
        self.scene.validate()?;
        Ok(SceneRead { scene: self.scene, warnings: self.warnings })
    }
}

/// Turn sampled values into a channel: f32 times, collapsed where two
/// samples land on one f32 instant, and for STEP channels without the keys
/// that repeat the previous value (they change nothing).
/// Push the points that split `(p, q)` until, for every curve, the chord of
/// each piece passes within the curve's tolerance of its midpoint.
fn refine(curves: &[&Curve], p: f64, q: f64, depth: u32, out: &mut Vec<f64>) {
    if depth == 0 {
        return;
    }
    let m = 0.5 * (p + q);
    if !(m > p && m < q) {
        return;
    }
    if curves.iter().all(|c| (c.eval(m) - 0.5 * (c.eval(p) + c.eval(q))).abs() <= c.tol) {
        return;
    }
    refine(curves, p, m, depth - 1, out);
    if out.last().is_none_or(|l| m > *l) {
        out.push(m);
    }
    refine(curves, m, q, depth - 1, out);
}

fn finish_channel(node: usize, path: AnimPath, step: bool, times: &[f64], values: Vec<f32>, stride: usize) -> Option<Channel> {
    if times.is_empty() || stride == 0 || values.len() != times.len() * stride {
        return None;
    }
    let mut ot: Vec<f32> = Vec::with_capacity(times.len());
    let mut ov: Vec<f32> = Vec::with_capacity(values.len());
    for (k, t) in times.iter().enumerate() {
        let t = *t as f32;
        let v = &values[k * stride..(k + 1) * stride];
        if ot.last().is_some_and(|l| *l >= t) {
            // Same f32 instant: the later sample wins.
            let start = ov.len() - stride;
            ov[start..].copy_from_slice(v);
            continue;
        }
        if step && !ot.is_empty() && ov[ov.len() - stride..] == *v {
            continue;
        }
        ot.push(t);
        ov.extend_from_slice(v);
    }
    Some(Channel {
        node,
        path,
        interpolation: if step { Interpolation::Step } else { Interpolation::Linear },
        times: ot,
        values: ov,
    })
}

#[cfg(test)]
mod tests {
    use super::super::package::NoSidecars;
    use super::*;
    use super::super::fixtures::sweeps::{layer_sweep, span};

    const SECOND: i64 = 46_186_158_000;

    /// Decode one curve from an ASCII document of the given version.
    /// `keys` are (seconds, value); `runs` are (flags, [four attributes as
    /// written], key count).
    fn curve(version: u32, keys: &[(f64, f64)], runs: &[(i32, [&str; 4], usize)]) -> Curve {
        let times: Vec<String> = keys.iter().map(|(t, _)| ((t * SECOND as f64).round() as i64).to_string()).collect();
        let values: Vec<String> = keys.iter().map(|(_, v)| v.to_string()).collect();
        let flags: Vec<String> = runs.iter().map(|r| r.0.to_string()).collect();
        let data: Vec<&str> = runs.iter().flat_map(|r| r.1).collect();
        let refs: Vec<String> = runs.iter().map(|r| r.2.to_string()).collect();
        let arr = |name: &str, v: &[String]| format!("\t\t{name}: *{} {{\n\t\t\ta: {}\n\t\t}}\n", v.len(), v.join(","));
        let src = format!(
            "; FBX project file\nFBXHeaderExtension:  {{\n\tFBXVersion: {version}\n}}\nObjects:  {{\n\tAnimationCurve: 7, \"AnimCurve::\", \"\" {{\n{}{}{}{}{}\t}}\n}}\n",
            arr("KeyTime", &times),
            arr("KeyValueFloat", &values),
            arr("KeyAttrFlags", &flags),
            arr("KeyAttrDataFloat", &data.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
            arr("KeyAttrRefCount", &refs),
        );
        let dom = super::super::fbx::parse(src.as_bytes()).unwrap_or_else(|e| panic!("{e}"));
        let side = NoSidecars;
        let mut r = Reader::new(&dom, &side);
        r.header().unwrap();
        r.index().unwrap();
        r.curve(7).unwrap().expect("a curve")
    }

    /// The bit pattern of a float, as the SDK's 7.2+ ASCII writer prints it.
    fn bits(x: f32) -> String {
        (x.to_bits() as i32).to_string()
    }

    const CUBIC_AUTO: i32 = FLAG_CUBIC | FLAG_CONSTANT_NEXT;

    #[test]
    fn auto_tangents_slope_to_the_one_neighbour_at_the_ends() {
        // Two auto keys, nothing stored: the ends take the line between
        // them, so the curve is that line (not a flat-ended smoothstep).
        let c = curve(7400, &[(0.0, 0.0), (1.0, 10.0)], &[(CUBIC_AUTO, ["0", "0", "0", "0"], 2)]);
        for t in [0.25, 0.5, 0.8] {
            assert!((c.eval(t) - 10.0 * t).abs() < 1e-9, "{t}: {}", c.eval(t));
        }
        // Clamped progressive: flat at both ends.
        let c = curve(7400, &[(0.0, 0.0), (1.0, 10.0)], &[(CUBIC_AUTO | FLAG_CLAMP_PROGRESSIVE, ["0", "0", "0", "0"], 2)]);
        assert!((c.eval(0.25) - 10.0 * (3.0 * 0.0625 - 2.0 * 0.015625)).abs() < 1e-9, "{}", c.eval(0.25));
    }

    #[test]
    fn auto_tangents_inside_a_curve_follow_the_sdk() {
        let keys = [(0.0, 0.0), (1.0, 10.0), (3.0, 0.0)];
        // Time-independent: the slope through the neighbours, (0-0)/3 = 0.
        let c = curve(7400, &keys, &[(CUBIC_AUTO | FLAG_TIME_INDEPENDENT, ["0", "0", "0", "0"], 3)]);
        let seg = c.seg[0];
        assert!(matches!(seg, Seg::Cubic { next_left, .. } if next_left.abs() < 1e-12), "{seg:?}");
        // Otherwise a blend with the one-sided slopes (10 and -5) weighted by
        // where the key sits: 0.5 * 0 + 0.5 * (10 * 2/3 + -5 * 1/3) = 2.5.
        let c = curve(7400, &keys, &[(CUBIC_AUTO, ["0", "0", "0", "0"], 3)]);
        let seg = c.seg[0];
        assert!(matches!(seg, Seg::Cubic { next_left, .. } if (next_left - 2.5).abs() < 1e-9), "{seg:?}");
        // Clamped progressive at a peak: flat.
        let c = curve(7400, &keys, &[(CUBIC_AUTO | FLAG_CLAMP_PROGRESSIVE, ["0", "0", "0", "0"], 3)]);
        let seg = c.seg[0];
        assert!(matches!(seg, Seg::Cubic { next_left, .. } if next_left == 0.0), "{seg:?}");
        // Clamped: flat where a neighbour holds the same value.
        let c = curve(7400, &[(0.0, 5.0), (1.0, 5.0), (2.0, 9.0)], &[(CUBIC_AUTO | FLAG_CLAMP, ["0", "0", "0", "0"], 3)]);
        assert!(matches!(c.seg[0], Seg::Cubic { next_left, .. } if next_left == 0.0), "{:?}", c.seg[0]);
    }

    #[test]
    fn user_tangents_and_weights_are_read_from_the_attributes() {
        // Slopes 0 and 0, handles weighted 0.6 (right) and 0.2 (next left):
        // a Bézier whose midpoint (value 5) falls at time 0.65.
        let packed = f32::from_bits((2000u32 << 16) | 6000);
        let weighted = FLAG_CUBIC | FLAG_TANGENT_USER | FLAG_WEIGHTED_RIGHT | FLAG_WEIGHTED_NEXT_LEFT;
        for version in [7100, 7500] {
            // Before 7.2 the ASCII writer prints the attributes as floats.
            let w = if version >= 7200 { bits(packed) } else { format!("{:e}", packed) };
            let c = curve(version, &[(0.0, 0.0), (1.0, 10.0)], &[(weighted, ["0", "0", w.as_str(), "0"], 2)]);
            assert!((c.eval(0.65) - 5.0).abs() < 1e-6, "{version}: {}", c.eval(0.65));
        }
        // 7.2+ ASCII: a stored slope of 20 is printed as its float's bits.
        let user = FLAG_CUBIC | FLAG_TANGENT_USER;
        let c = curve(7500, &[(0.0, 0.0), (1.0, 0.0)], &[(user, [bits(20.0).as_str(), bits(-20.0).as_str(), "0", "0"], 2)]);
        // Hermite from 0 to 0 with slopes 20 and -20 over one second: at
        // its middle, (1/8) * 20 + (1/8) * 20 = 5.
        assert!((c.eval(0.5) - 5.0).abs() < 1e-9, "{}", c.eval(0.5));
    }

    #[test]
    fn tcb_keys_take_kochanek_bartels_slopes() {
        // Tension, continuity, bias 0 at an interior key: the Catmull-Rom
        // slope, the mean of the one-sided ones ((10 - 0) and (10 - 10)).
        let tcb = FLAG_CUBIC | FLAG_TANGENT_TCB;
        let c = curve(7400, &[(0.0, 0.0), (1.0, 10.0), (2.0, 10.0)], &[(tcb, ["0", "0", "0", "0"], 3)]);
        assert!(matches!(c.seg[1], Seg::Cubic { right, .. } if (right - 5.0).abs() < 1e-9), "{:?}", c.seg[1]);
        // Full tension: flat. (From 7.2 ASCII prints the float's bits.)
        let one = bits(1.0);
        let c = curve(7400, &[(0.0, 0.0), (1.0, 10.0), (2.0, 10.0)], &[(tcb, [one.as_str(), "0", "0", "0"], 3)]);
        assert!(matches!(c.seg[1], Seg::Cubic { right, .. } if right == 0.0), "{:?}", c.seg[1]);
    }

    #[test]
    fn a_key_holding_the_next_value_shows_its_own_value_at_its_time() {
        let c = curve(7400, &[(0.0, 1.0), (1.0, 2.0), (2.0, 3.0)], &[(FLAG_CONSTANT | FLAG_CONSTANT_NEXT, ["0", "0", "0", "0"], 3)]);
        assert_eq!(c.eval(0.0), 1.0);
        assert_eq!(c.eval(0.5), 2.0);
        assert_eq!(c.eval_right(0.0), 2.0);
        assert_eq!(c.eval(1.0), 2.0);
        assert_eq!(c.eval_right(1.0), 3.0);
    }

    #[test]
    fn bezier_parameter_inverts_the_time_handles() {
        for (x1, x2) in [(1.0 / 3.0, 2.0 / 3.0), (0.0, 1.0), (1.0, 0.0), (0.6, 0.8), (0.0, 0.0), (1.0, 1.0)] {
            for k in 0..=20 {
                let u = k as f64 / 20.0;
                let s = bezier_param(x1, x2, u);
                let r = 1.0 - s;
                let x = 3.0 * r * r * s * x1 + 3.0 * r * s * s * x2 + s * s * s;
                assert!((x - u).abs() < 1e-9, "({x1}, {x2}) at {u}: s {s} gives {x}");
            }
        }
    }

    #[test]
    fn in_between_shares_are_piecewise_linear_in_the_channel_weight() {
        let full = [0.5, 1.0];
        let cases: [(f64, [f64; 2]); 6] =
            [(0.0, [0.0, 0.0]), (0.25, [0.5, 0.0]), (0.5, [1.0, 0.0]), (0.75, [0.5, 0.5]), (1.0, [0.0, 1.0]), (1.5, [-1.0, 2.0])];
        for (w, want) in cases {
            let got = inbetween_weights(&full, w);
            assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-12), "{w}: {got:?}");
        }
        // Below zero the first shape extrapolates; one shape is linear.
        assert_eq!(inbetween_weights(&full, -0.5), vec![-1.0, 0.0]);
        assert_eq!(inbetween_weights(&[1.0], 0.3), vec![0.3]);
    }

    /// 2026-10-12 review, finding 16: assigning keys to attribute runs looped
    /// over each run's declared count even after every key had its run, so a
    /// 2-key curve declaring a run of 2³¹−1 spun for seconds (a ~150 KB file
    /// of 10k such runs: about 2 h). ufbx consumes runs per key.
    #[test]
    fn a_huge_attribute_run_costs_only_the_keys_it_covers() {
        let started = std::time::Instant::now();
        let c = curve(7400, &[(0.0, 0.0), (1.0, 10.0)], &[(FLAG_CUBIC | FLAG_TANGENT_USER, ["0", "0", "0", "0"], i32::MAX as usize)]);
        assert!(started.elapsed().as_millis() < 500, "{:?}", started.elapsed());
        assert!((c.eval(1.0) - 10.0).abs() < 1e-9);
    }

    // ── material layers ──────────────────────────────────────────────────

    /// One `Properties70` line, typed as the exporter types it.
    fn p(name: &str, ty: &str, label: &str, flags: &str, v: &[f64]) -> String {
        let values: Vec<String> = v.iter().map(|x| x.to_string()).collect();
        format!("\t\t\tP: \"{name}\", \"{ty}\", \"{label}\", \"{flags}\",{}\n", values.join(","))
    }

    /// Maya's types (`maya_arnold_properties_7500_ascii.fbx`): float,
    /// Vector3D and bool.
    fn maya(name: &str, x: f64) -> String {
        p(name, "float", "", "", &[x])
    }
    fn maya3(name: &str, c: [f64; 3]) -> String {
        p(name, "Vector3D", "Vector", "", &c)
    }
    fn maya_bool(name: &str, on: bool) -> String {
        p(name, "bool", "", "", &[f64::from(u8::from(on))])
    }

    /// 3ds Max's (`max_physical_material_properties_7500_ascii.fbx`): Float,
    /// ColorAndAlpha and Bool.
    fn max(name: &str, x: f64) -> String {
        p(name, "Float", "", "A", &[x])
    }
    fn max4(name: &str, c: [f64; 4]) -> String {
        p(name, "ColorAndAlpha", "", "A", &c)
    }
    fn max_bool(name: &str, on: bool) -> String {
        p(name, "Bool", "", "A", &[f64::from(u8::from(on))])
    }

    /// 3ds Max's class ids for a Physical (`physical`) or OpenPBR material,
    /// as both exports write them (ufbx.c L22340-L22347 reads them).
    fn max_class(physical: bool) -> String {
        let (a, b) = if physical { (1030429932, -559038463) } else { (-246079949, 939201335) };
        format!("\t\t\tP: \"3dsMax|ClassIDa\", \"int\", \"Integer\", \"\",{a}\n\t\t\tP: \"3dsMax|ClassIDb\", \"int\", \"Integer\", \"\",{b}\n")
    }

    /// A triangle with one material holding `props` and, when `slot` is
    /// given, a texture (`t.png`) connected to that material property; read
    /// back as the material and the reader's warnings.
    fn layered_material(props: &str, slot: Option<&str>) -> (Material, Vec<String>) {
        let r = layered_scene(props, slot, &NoSidecars);
        (r.scene.materials[0].clone(), r.warnings)
    }

    /// `layered_material`'s document, read with `side`'s files.
    fn layered_scene(props: &str, slot: Option<&str>, side: &dyn Sidecars) -> SceneRead {
        let src = layered_doc(props, slot);
        let r = super::super::read_scene(src.as_bytes(), super::super::MeshFormat::Fbx, side).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.scene.materials.len(), 1, "{src}");
        r
    }

    /// `layered_material`'s document, as ASCII FBX text.
    fn layered_doc(props: &str, slot: Option<&str>) -> String {
        let texture = match slot {
            Some(_) => "\tTexture: 30, \"Texture::t\", \"\" {\n\t\tType: \"TextureVideoClip\"\n\t\tFileName: \"t.png\"\n\t\tRelativeFilename: \"t.png\"\n\t}\n",
            None => "",
        };
        let link = slot.map(|s| format!("\tC: \"OP\",30,20, \"{s}\"\n")).unwrap_or_default();
        format!(
            "; FBX 7.5.0 project file\nFBXHeaderExtension:  {{\n\tFBXVersion: 7500\n}}\nObjects:  {{\n\
             \tModel: 10, \"Model::m\", \"Mesh\" {{\n\t}}\n\
             \tGeometry: 99, \"Geometry::tri\", \"Mesh\" {{\n\t\tVertices: *9 {{\n\t\t\ta: 0,0,0,1,0,0,0,1,0\n\t\t}}\n\t\tPolygonVertexIndex: *3 {{\n\t\t\ta: 0,1,-3\n\t\t}}\n\
             \t\tLayerElementMaterial: 0 {{\n\t\t\tMappingInformationType: \"AllSame\"\n\t\t\tReferenceInformationType: \"IndexToDirect\"\n\t\t\tMaterials: *1 {{\n\t\t\t\ta: 0\n\t\t\t}}\n\t\t}}\n\
             \t\tLayer: 0 {{\n\t\t\tLayerElement:  {{\n\t\t\t\tType: \"LayerElementMaterial\"\n\t\t\t\tTypedIndex: 0\n\t\t\t}}\n\t\t}}\n\t}}\n\
             \tMaterial: 20, \"Material::mat\", \"\" {{\n\t\tShadingModel: \"unknown\"\n\t\tProperties70:  {{\n{props}\t\t}}\n\t}}\n\
             {texture}}}\nConnections:  {{\n\tC: \"OO\",10,0\n\tC: \"OO\",99,10\n\tC: \"OO\",20,10\n{link}}}\n"
        )
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1e-6 * b.abs().max(1.0)
    }

    /// Every warning in `got` is one of `want` (each given as fragments the
    /// warning must all contain), and each of `want` is warned exactly once.
    fn assert_warnings(got: &[String], want: &[&[&str]], ctx: &str) {
        for w in want {
            let n = got.iter().filter(|g| w.iter().all(|f| g.contains(f))).count();
            assert_eq!(n, 1, "{ctx}: {w:?} warned {n} times in {got:#?}");
        }
        for g in got {
            assert!(want.iter().any(|w| w.iter().all(|f| g.contains(f))), "{ctx}: unexpected warning {g:?} in {got:#?}");
        }
    }

    fn no_layers(m: &Material) -> bool {
        MaterialLayer::ALL.iter().all(|l| !m.has_layer(*l))
    }

    /// The export's lines L342-L386, all of them, from
    /// `data/maya_arnold_properties_7500_ascii.fbx` in ufbx v0.23.1,
    /// which `arnold_layers_read` cites.
    const ARNOLD_EXPORT: &str = "\t\t\tP: \"Maya\", \"Compound\", \"\", \"\"\n\
            \t\t\tP: \"Maya|TypeId\", \"int\", \"Integer\", \"\",1138001\n\
            \t\t\tP: \"Maya|outAlpha\", \"float\", \"\", \"\",0\n\
            \t\t\tP: \"Maya|normalCamera\", \"Vector3D\", \"Vector\", \"\",1,1,1\n\
            \t\t\tP: \"Maya|aiEnableMatte\", \"bool\", \"\", \"\",1\n\
            \t\t\tP: \"Maya|aiMatteColor\", \"Vector3D\", \"Vector\", \"\",0.649999976158142,0.660000026226044,0.670000016689301\n\
            \t\t\tP: \"Maya|aiMatteColorA\", \"float\", \"\", \"\",0.68\n\
            \t\t\tP: \"Maya|base\", \"float\", \"\", \"\",0.01\n\
            \t\t\tP: \"Maya|baseColor\", \"Vector3D\", \"Vector\", \"\",0.0199999995529652,0.0299999993294477,0.0399999991059303\n\
            \t\t\tP: \"Maya|diffuseRoughness\", \"float\", \"\", \"\",0.05\n\
            \t\t\tP: \"Maya|specular\", \"float\", \"\", \"\",0.07\n\
            \t\t\tP: \"Maya|specularColor\", \"Vector3D\", \"Vector\", \"\",0.0799999982118607,0.0900000035762787,0.100000001490116\n\
            \t\t\tP: \"Maya|specularRoughness\", \"float\", \"\", \"\",0.11\n\
            \t\t\tP: \"Maya|specularIOR\", \"float\", \"\", \"\",0.12\n\
            \t\t\tP: \"Maya|specularAnisotropy\", \"float\", \"\", \"\",0.13\n\
            \t\t\tP: \"Maya|specularRotation\", \"float\", \"\", \"\",0.14\n\
            \t\t\tP: \"Maya|metalness\", \"float\", \"\", \"\",0.06\n\
            \t\t\tP: \"Maya|transmission\", \"float\", \"\", \"\",0.15\n\
            \t\t\tP: \"Maya|transmissionColor\", \"Vector3D\", \"Vector\", \"\",0.159999996423721,0.170000001788139,0.180000007152557\n\
            \t\t\tP: \"Maya|transmissionDepth\", \"float\", \"\", \"\",0.19\n\
            \t\t\tP: \"Maya|transmissionScatter\", \"Vector3D\", \"Vector\", \"\",0.200000002980232,0.209999993443489,0.219999998807907\n\
            \t\t\tP: \"Maya|transmissionScatterAnisotropy\", \"float\", \"\", \"\",0.23\n\
            \t\t\tP: \"Maya|transmissionDispersion\", \"float\", \"\", \"\",0.24\n\
            \t\t\tP: \"Maya|transmissionExtraRoughness\", \"float\", \"\", \"\",0.25\n\
            \t\t\tP: \"Maya|transmitAovs\", \"bool\", \"\", \"\",1\n\
            \t\t\tP: \"Maya|subsurface\", \"float\", \"\", \"\",0.26\n\
            \t\t\tP: \"Maya|subsurfaceColor\", \"Vector3D\", \"Vector\", \"\",0.270000010728836,0.280000001192093,0.28999999165535\n\
            \t\t\tP: \"Maya|subsurfaceRadius\", \"Vector3D\", \"Vector\", \"\",0.300000011920929,0.310000002384186,0.319999992847443\n\
            \t\t\tP: \"Maya|subsurfaceScale\", \"float\", \"\", \"\",0.33\n\
            \t\t\tP: \"Maya|subsurfaceAnisotropy\", \"float\", \"\", \"\",0.34\n\
            \t\t\tP: \"Maya|subsurfaceType\", \"enum\", \"\", \"\",1\n\
            \t\t\tP: \"Maya|sheen\", \"float\", \"\", \"\",0.46\n\
            \t\t\tP: \"Maya|sheenColor\", \"Vector3D\", \"Vector\", \"\",0.469999998807907,0.479999989271164,0.490000009536743\n\
            \t\t\tP: \"Maya|sheenRoughness\", \"float\", \"\", \"\",0.5\n\
            \t\t\tP: \"Maya|thinWalled\", \"bool\", \"\", \"\",1\n\
            \t\t\tP: \"Maya|tangent\", \"Vector3D\", \"Vector\", \"\",0.600000023841858,0.610000014305115,0.620000004768372\n\
            \t\t\tP: \"Maya|coat\", \"float\", \"\", \"\",0.35\n\
            \t\t\tP: \"Maya|coatColor\", \"Vector3D\", \"Vector\", \"\",0.360000014305115,0.370000004768372,0.379999995231628\n\
            \t\t\tP: \"Maya|coatRoughness\", \"float\", \"\", \"\",0.39\n\
            \t\t\tP: \"Maya|coatIOR\", \"float\", \"\", \"\",0.4\n\
            \t\t\tP: \"Maya|coatAnisotropy\", \"float\", \"\", \"\",0.41\n\
            \t\t\tP: \"Maya|coatRotation\", \"float\", \"\", \"\",0.42\n\
            \t\t\tP: \"Maya|coatNormal\", \"Vector3D\", \"Vector\", \"\",0.430000007152557,0.439999997615814,0.449999988079071\n\
            \t\t\tP: \"Maya|thinFilmThickness\", \"float\", \"\", \"\",0.55\n\
            \t\t\tP: \"Maya|thinFilmIOR\", \"float\", \"\", \"\",0.56\n";

    /// Arnold's aiStandardSurface and Maya's Standard Surface read the glTF
    /// layers their properties map to, and warn about what glTF can't hold.
    ///
    /// The names and value types are Maya's own. The first case's lines are
    /// copied from a Maya export, `data/maya_arnold_properties_7500_ascii.fbx`
    /// in ufbx v0.23.1 (FBX SDK 2020.1;
    /// https://github.com/ufbx/ufbx/blob/v0.23.1/data/maya_arnold_properties_7500_ascii.fbx),
    /// which sets each property to a distinct value. Its standardSurface twin,
    /// `data/maya_osl_properties_7500_ascii.fbx`, writes the same names and
    /// binds each to its Standard Surface parameter in its BindingTable
    /// (`"Maya|coatRoughness"` → `coat_roughness`), and ufbx.c L19570-L19618
    /// reads them so. The meanings, units and defaults are the Standard
    /// Surface spec's, v1.0.1.1
    /// (https://github.com/Autodesk/standard-surface/blob/v1.0.1.1/index.html),
    /// and its reference `reference/standard_surface.osl` at the same tag.
    #[test]
    fn arnold_layers_read() {
        let export = ARNOLD_EXPORT;
        let (m, w) = layered_material(export, None);
        let ctx = "the Maya export";
        // coat → clearcoatFactor, coatRoughness → clearcoatRoughnessFactor:
        // both are roughnesses squared into the GGX alpha (SS L163, .osl
        // L219; KHR_materials_clearcoat: alpha = clearcoatRoughness^2).
        assert_eq!(m.clearcoat, Some(Clearcoat { factor: 0.35, roughness: 0.39, ..Default::default() }), "{ctx}");
        // transmission → transmissionFactor (SS L276, L295).
        assert_eq!(m.transmission, Some(Transmission { factor: 0.15, texture: None }), "{ctx}");
        // specular → specularFactor: both scale the whole dielectric lobe
        // (SS L246; KHR_materials_specular: f90 = specular).
        assert_eq!(m.specular, Some(Specular { factor: 0.07, ..Default::default() }), "{ctx}");
        // sheen × sheenColor → sheenColorFactor; sheenRoughness is the
        // sheen alpha, glTF's is its square root (MaterialX v1.39.5
        // standard_surface.mtlx L259-L262 against gltf_pbr.mtlx L258-L271).
        let s = m.sheen.clone().expect("sheen");
        assert!(close(s.color[0], 0.2162) && close(s.color[1], 0.2208) && close(s.color[2], 0.2254), "{ctx}: {s:?}");
        assert!(close(s.roughness, 0.70710677) && s.color_texture.is_none() && s.roughness_texture.is_none(), "{ctx}: {s:?}");
        // thinFilmThickness (nm) → the thickness glTF uses without a
        // texture, its maximum (KHR_materials_iridescence); thinFilmIOR 0.56
        // is below glTF's minimum of 1, so it reads as SS's default, 1.5.
        assert_eq!(
            m.iridescence,
            Some(Iridescence { factor: 1.0, ior: 1.5, thickness_max: 0.55, ..Default::default() }),
            "{ctx}"
        );
        // Neither an ior KHR_materials_ior can't hold (0.12), nor what glTF
        // has no slot for.
        assert_eq!((m.ior, m.volume.clone(), m.anisotropy.clone(), m.dispersion), (None, None, None, None), "{ctx}");
        assert_warnings(
            &w,
            &[
                &["Maya|coatColor linear (0.36, 0.37, 0.38)", "Maya|coatIOR 0.4", "Maya|coatAnisotropy 0.41"],
                // depth > 0: SS's tint is white, its colour an absorption (SS L282).
                &["tints transmitted light with the base colour, linear", "Standard Surface tints it linear (1, 1, 1)"],
                // glTF's transmission takes the base roughness (Ruling A24h).
                &["Maya|transmissionExtraRoughness 0.25"],
                &["Maya|specularColor linear (0.08, 0.09, 0.1)"],
                &["Maya|specularIOR 0.12"],
                &["Maya|specularAnisotropy 0.13"],
                // transmissionDispersion 0.24 goes unwarned: the surface is
                // thin-walled (SS L287), so it renders no dispersion.
                &["Maya|thinFilmIOR 0.56"],
            ],
            ctx,
        );

        // Maya's defaults read no layer and warn about nothing: SS's
        // defaults (SS L161-L333, .osl L155-L195) with Maya's own
        // specularIOR of 1.52 (data/maya_arnold_textures_7500_ascii.fbx),
        // which reads as unset (Ruling A24b).
        let defaults = [
            maya("Maya|base", 0.8),
            maya3("Maya|baseColor", [1.0; 3]),
            maya("Maya|specular", 1.0),
            maya3("Maya|specularColor", [1.0; 3]),
            maya("Maya|specularIOR", 1.52),
            maya("Maya|specularAnisotropy", 0.0),
            maya("Maya|transmission", 0.0),
            maya3("Maya|transmissionColor", [1.0; 3]),
            maya("Maya|transmissionDepth", 0.0),
            maya("Maya|transmissionDispersion", 0.0),
            maya("Maya|sheen", 0.0),
            maya3("Maya|sheenColor", [1.0; 3]),
            maya("Maya|sheenRoughness", 0.3),
            maya_bool("Maya|thinWalled", false),
            maya("Maya|coat", 0.0),
            maya3("Maya|coatColor", [1.0; 3]),
            maya("Maya|coatRoughness", 0.1),
            maya("Maya|coatIOR", 1.5),
            maya("Maya|coatAnisotropy", 0.0),
            maya("Maya|thinFilmThickness", 0.0),
            maya("Maya|thinFilmIOR", 1.5),
        ]
        .concat();
        let (m, w) = layered_material(&defaults, None);
        assert!(no_layers(&m) && w.is_empty(), "Maya's defaults: {m:?} {w:?}");

        // Layers glTF holds as they are: no warning.
        let held = [
            maya("Maya|base", 1.0),
            maya3("Maya|baseColor", [0.5; 3]),
            maya("Maya|coat", 1.0),
            maya("Maya|coatRoughness", 0.25),
            maya3("Maya|coatColor", [1.0; 3]),
            maya("Maya|coatIOR", 1.5),
            maya("Maya|coatAffectColor", 0.0),
            maya("Maya|coatAffectRoughness", 0.0),
            maya("Maya|transmission", 1.0),
            maya3("Maya|transmissionColor", [0.5; 3]),
            maya("Maya|transmissionDepth", 0.0),
            maya("Maya|transmissionExtraRoughness", 0.0),
            maya("Maya|transmissionDispersion", 20.0),
            maya_bool("Maya|thinWalled", true),
            maya("Maya|specularIOR", 1.45),
            maya("Maya|sheen", 1.0),
            maya3("Maya|sheenColor", [0.3, 0.2, 0.1]),
            maya("Maya|sheenRoughness", 0.36),
        ]
        .concat();
        let (m, w) = layered_material(&held, None);
        assert!(w.is_empty(), "held: {w:?}");
        assert_eq!(m.clearcoat, Some(Clearcoat { factor: 1.0, roughness: 0.25, ..Default::default() }));
        assert_eq!(m.transmission, Some(Transmission { factor: 1.0, texture: None }));
        assert_eq!(m.ior, Some(1.45));
        let s = m.sheen.clone().expect("sheen");
        assert!(close(s.color[0], 0.3) && close(s.color[1], 0.2) && close(s.color[2], 0.1) && close(s.roughness, 0.6), "{s:?}");
        assert!(m.specular.is_none() && m.iridescence.is_none(), "{m:?}");

        // Values outside their glTF field's range are left out with a
        // warning, never clamped (Ruling A16): a weight leaves its layer
        // out, a roughness reads as SS's default (sheen_roughness 0.3).
        let outside = [
            maya3("Maya|baseColor", [1.0; 3]),
            maya("Maya|coat", 1.5),
            maya("Maya|transmission", -0.5),
            maya("Maya|sheen", 1.0),
            maya("Maya|sheenRoughness", 2.0),
            maya("Maya|specular", 1.2),
            maya("Maya|specularIOR", 0.5),
            maya("Maya|thinFilmThickness", -10.0),
        ]
        .concat();
        let (m, w) = layered_material(&outside, None);
        assert!(m.clearcoat.is_none() && m.transmission.is_none() && m.specular.is_none(), "{m:?}");
        assert!(m.ior.is_none() && m.iridescence.is_none(), "{m:?}");
        let s = m.sheen.clone().expect("sheen");
        assert!(close(s.roughness, 0.5477226) && s.color == [1.0; 3], "{s:?}");
        assert_warnings(
            &w,
            &[
                &["Maya|coat 1.5"],
                &["Maya|transmission -0.5"],
                &["Maya|sheenRoughness 2"],
                &["Maya|specular 1.2"],
                &["Maya|specularIOR 0.5"],
                &["Maya|thinFilmThickness -10"],
            ],
            "outside",
        );
        // So is a non-finite one (MSVC prints NaN as 1.#IND, infinity as
        // 1.#INF): ignored with a warning, never handed to `validate`, which
        // would refuse the file.
        let non_finite = [
            maya3("Maya|baseColor", [1.0; 3]),
            maya("Maya|coat", 1.0),
            "\t\t\tP: \"Maya|coatRoughness\", \"float\", \"\", \"\",1.#IND00\n".to_string(),
            "\t\t\tP: \"Maya|specularIOR\", \"float\", \"\", \"\",1.#INF00\n".to_string(),
            maya("Maya|sheen", 1.0),
            "\t\t\tP: \"Maya|sheenColor\", \"Vector3D\", \"Vector\", \"\",1.#IND00,0,0\n".to_string(),
        ]
        .concat();
        let (m, w) = layered_material(&non_finite, None);
        assert_eq!(m.clearcoat, Some(Clearcoat { factor: 1.0, roughness: 0.1, ..Default::default() }));
        assert_eq!(m.ior, None);
        // An ignored colour reads as SS's default, white (L332).
        let s = m.sheen.clone().expect("sheen");
        assert!(s.color == [1.0; 3] && close(s.roughness, 0.5477226), "{s:?}");
        assert_warnings(&w, &[&["Maya|coatRoughness NaN"], &["Maya|specularIOR inf"], &["Maya|sheenColor linear (NaN, 0, 0)"]], "non-finite");
        // A sheen colour outside 0 to 1 leaves the sheen out, as glTF's
        // sheenColorFactor is 0 to 1 (KHR_materials_sheen's schema).
        let bright = [maya3("Maya|baseColor", [1.0; 3]), maya("Maya|sheen", 1.0), maya3("Maya|sheenColor", [2.0, 0.5, 0.5])].concat();
        let (m, w) = layered_material(&bright, None);
        assert!(m.sheen.is_none(), "{m:?}");
        assert_warnings(&w, &[&["Maya|sheenColor linear (2, 0.5, 0.5)", "the sheen layer was left out"]], "a bright sheen");
        // An ior of 0 is KHR_materials_ior's own special value, so it is
        // carried.
        let (m, w) = layered_material(&[maya3("Maya|baseColor", [1.0; 3]), maya("Maya|specularIOR", 0.0)].concat(), None);
        assert!(m.ior == Some(0.0) && w.is_empty(), "{m:?} {w:?}");

        // A solid (not thin-walled) glass reads thin-walled, saying so, and
        // names the absorption and dispersion it loses (Rulings A24c, A24a).
        // Its untinted transmission matches the white base colour, so no tint
        // warning.
        let cases: [(f64, f64, &[&str]); 2] = [
            (2.0, 30.0, &["thin-walled", "Maya|transmissionDepth 2", "Maya|transmissionDispersion 30"]),
            (0.0, 0.0, &["thin-walled"]),
        ];
        for (depth, dispersion, fragments) in cases {
            let solid = [
                maya3("Maya|baseColor", [1.0; 3]),
                maya("Maya|transmission", 1.0),
                maya3("Maya|transmissionColor", [1.0; 3]),
                maya("Maya|transmissionDepth", depth),
                maya("Maya|transmissionDispersion", dispersion),
                maya_bool("Maya|thinWalled", false),
            ]
            .concat();
            let (m, w) = layered_material(&solid, None);
            assert_eq!(m.transmission, Some(Transmission { factor: 1.0, texture: None }), "depth {depth}");
            assert!(m.volume.is_none() && m.dispersion.is_none(), "depth {depth}");
            assert_warnings(&w, &[fragments], &format!("solid, depth {depth}"));
            assert!(depth > 0.0 || (!w[0].contains("transmissionDepth") && !w[0].contains("Dispersion")), "{w:?}");
        }
        // An opaque surface shows no dispersion, so it says nothing of it.
        let opaque = [maya3("Maya|baseColor", [1.0; 3]), maya("Maya|transmission", 0.0), maya("Maya|transmissionDispersion", 30.0)].concat();
        let (m, w) = layered_material(&opaque, None);
        assert!(no_layers(&m) && w.is_empty(), "opaque: {m:?} {w:?}");
        // SS tints by the colour only at depth 0, whatever thin_walled says
        // (SS L282), so a thin sheet with a depth goes untinted where glTF
        // tints it with the base colour.
        let sheet = [
            maya3("Maya|baseColor", [0.5; 3]),
            maya("Maya|transmission", 1.0),
            maya3("Maya|transmissionColor", [0.5; 3]),
            maya("Maya|transmissionDepth", 1.0),
            maya_bool("Maya|thinWalled", true),
        ]
        .concat();
        let (_, w) = layered_material(&sheet, None);
        assert_warnings(&w, &[&["base colour, linear (0.5, 0.5, 0.5)", "Standard Surface tints it linear (1, 1, 1)"]], "a thin sheet");
        // A map on the base colour tints glTF's transmission too, whatever
        // the factor beside it.
        let files = vec![super::super::package::PackageFile { path: "t.png".into(), bytes: super::super::fixtures::quadrant_png() }];
        let side = super::super::package::MemorySidecars { files: &files };
        let mapped = [
            maya3("Maya|baseColor", [1.0; 3]),
            maya("Maya|transmission", 1.0),
            maya3("Maya|transmissionColor", [1.0; 3]),
            maya_bool("Maya|thinWalled", true),
        ]
        .concat();
        let r = layered_scene(&mapped, Some("Maya|baseColor"), &side);
        assert!(r.scene.materials[0].base_color_texture.is_some(), "{:?}", r.scene.materials[0]);
        assert_warnings(&r.warnings, &[&["base colour and its map, linear (1, 1, 1)", "Standard Surface tints it linear (1, 1, 1)"]], "a mapped base");

        // The coat's effect on the layers under it isn't glTF's (Ruling
        // A24g): a darker, more saturated base and a rougher one (SS
        // L142-L151, L168-L169, default 0).
        let affect = [
            maya3("Maya|baseColor", [1.0; 3]),
            maya("Maya|coat", 1.0),
            maya("Maya|coatAffectColor", 0.3),
            maya("Maya|coatAffectRoughness", 0.2),
        ]
        .concat();
        let (m, w) = layered_material(&affect, None);
        assert!(m.clearcoat.is_some(), "{m:?}");
        assert_warnings(&w, &[&["Maya|coatAffectColor 0.3", "Maya|coatAffectRoughness 0.2"]], "coat affect");

        // A texture on a layer property is named, and the layer reads
        // untextured, from its value (coatRoughness unset: SS's 0.1). The
        // warning says no more than that.
        let (m, w) = layered_material(&[maya3("Maya|baseColor", [1.0; 3]), maya("Maya|coat", 0.5)].concat(), Some("Maya|coat"));
        assert_eq!(m.clearcoat, Some(Clearcoat { factor: 0.5, roughness: 0.1, ..Default::default() }));
        assert_warnings(&w, &[&["\"Maya|coat\"", "clearcoat", "stored value"]], "a textured coat");
    }

    /// The export's lines L325-L369, all of them, from
    /// `data/max_physical_material_properties_7500_ascii.fbx` in ufbx v0.23.1,
    /// which `physical_layers_read` cites.
    const PHYSICAL_EXPORT: &str = "\t\t\tP: \"3dsMax|ClassIDa\", \"int\", \"Integer\", \"\",1030429932\n\
            \t\t\tP: \"3dsMax|ClassIDb\", \"int\", \"Integer\", \"\",-559038463\n\
            \t\t\tP: \"3dsMax|SuperClassID\", \"int\", \"Integer\", \"\",3072\n\
            \t\t\tP: \"3dsMax|Parameters\", \"Compound\", \"\", \"\"\n\
            \t\t\tP: \"3dsMax|Parameters|material_mode\", \"Integer\", \"\", \"A\",1\n\
            \t\t\tP: \"3dsMax|Parameters|base_weight\", \"Float\", \"\", \"A\",0.01\n\
            \t\t\tP: \"3dsMax|Parameters|base_color\", \"ColorAndAlpha\", \"\", \"A\",0.0199999995529652,0.0299999993294477,0.0399999991059303,0.0500000007450581\n\
            \t\t\tP: \"3dsMax|Parameters|reflectivity\", \"Float\", \"\", \"A\",1\n\
            \t\t\tP: \"3dsMax|Parameters|roughness\", \"Float\", \"\", \"A\",0.06\n\
            \t\t\tP: \"3dsMax|Parameters|roughness_inv\", \"Bool\", \"\", \"A\",0\n\
            \t\t\tP: \"3dsMax|Parameters|metalness\", \"Float\", \"\", \"A\",0.07\n\
            \t\t\tP: \"3dsMax|Parameters|refl_color\", \"ColorAndAlpha\", \"\", \"A\",1,1,1,1\n\
            \t\t\tP: \"3dsMax|Parameters|diff_roughness\", \"Float\", \"\", \"A\",0.37\n\
            \t\t\tP: \"3dsMax|Parameters|brdf_mode\", \"Bool\", \"\", \"A\",1\n\
            \t\t\tP: \"3dsMax|Parameters|brdf_low\", \"Float\", \"\", \"A\",0.05\n\
            \t\t\tP: \"3dsMax|Parameters|brdf_high\", \"Float\", \"\", \"A\",1\n\
            \t\t\tP: \"3dsMax|Parameters|brdf_curve\", \"Float\", \"\", \"A\",5\n\
            \t\t\tP: \"3dsMax|Parameters|anisotropy\", \"Float\", \"\", \"A\",0.32\n\
            \t\t\tP: \"3dsMax|Parameters|anisoangle\", \"Float\", \"\", \"A\",0.33\n\
            \t\t\tP: \"3dsMax|Parameters|aniso_mode\", \"Integer\", \"\", \"A\",0\n\
            \t\t\tP: \"3dsMax|Parameters|aniso_channel\", \"Integer\", \"\", \"A\",0\n\
            \t\t\tP: \"3dsMax|Parameters|transparency\", \"Float\", \"\", \"A\",0.09\n\
            \t\t\tP: \"3dsMax|Parameters|trans_color\", \"ColorAndAlpha\", \"\", \"A\",0.100000001490116,0.109999999403954,0.119999997317791,0.129999995231628\n\
            \t\t\tP: \"3dsMax|Parameters|trans_depth\", \"Float\", \"\", \"A\",0.059055\n\
            \t\t\tP: \"3dsMax|Parameters|trans_roughness\", \"Float\", \"\", \"A\",0.14\n\
            \t\t\tP: \"3dsMax|Parameters|trans_roughness_inv\", \"Bool\", \"\", \"A\",0\n\
            \t\t\tP: \"3dsMax|Parameters|trans_roughness_lock\", \"Bool\", \"\", \"A\",0\n\
            \t\t\tP: \"3dsMax|Parameters|trans_ior\", \"Float\", \"\", \"A\",0.8\n\
            \t\t\tP: \"3dsMax|Parameters|thin_walled\", \"Bool\", \"\", \"A\",0\n\
            \t\t\tP: \"3dsMax|Parameters|scattering\", \"Float\", \"\", \"A\",0.16\n\
            \t\t\tP: \"3dsMax|Parameters|sss_color\", \"ColorAndAlpha\", \"\", \"A\",0.170000001788139,0.180000007152557,0.189999997615814,0.200000002980232\n\
            \t\t\tP: \"3dsMax|Parameters|sss_depth\", \"Float\", \"\", \"A\",0.098425\n\
            \t\t\tP: \"3dsMax|Parameters|sss_scale\", \"Float\", \"\", \"A\",0.26\n\
            \t\t\tP: \"3dsMax|Parameters|sss_scatter_color\", \"ColorAndAlpha\", \"\", \"A\",0.209999993443489,0.219999998807907,0.230000004172325,0.239999994635582\n\
            \t\t\tP: \"3dsMax|Parameters|emission\", \"Float\", \"\", \"A\",0.27\n\
            \t\t\tP: \"3dsMax|Parameters|emit_color\", \"ColorAndAlpha\", \"\", \"A\",0.280000001192093,0.28999999165535,0.300000011920929,0.310000002384186\n\
            \t\t\tP: \"3dsMax|Parameters|emit_luminance\", \"Float\", \"\", \"A\",1500\n\
            \t\t\tP: \"3dsMax|Parameters|emit_kelvin\", \"Float\", \"\", \"A\",6500\n\
            \t\t\tP: \"3dsMax|Parameters|coating\", \"Float\", \"\", \"A\",0.38\n\
            \t\t\tP: \"3dsMax|Parameters|coat_color\", \"ColorAndAlpha\", \"\", \"A\",0.389999985694885,0.400000005960464,0.409999996423721,0.419999986886978\n\
            \t\t\tP: \"3dsMax|Parameters|coat_roughness\", \"Float\", \"\", \"A\",0.43\n\
            \t\t\tP: \"3dsMax|Parameters|coat_roughness_inv\", \"Bool\", \"\", \"A\",0\n\
            \t\t\tP: \"3dsMax|Parameters|coat_affect_color\", \"Float\", \"\", \"A\",0.45\n\
            \t\t\tP: \"3dsMax|Parameters|coat_affect_roughness\", \"Float\", \"\", \"A\",0.46\n\
            \t\t\tP: \"3dsMax|Parameters|coat_ior\", \"Float\", \"\", \"A\",0.44\n";

    /// 3ds Max's Physical Material reads the glTF layers its properties map
    /// to, and warns about what glTF can't hold.
    ///
    /// The first case's lines are copied from a 3ds Max export,
    /// `data/max_physical_material_properties_7500_ascii.fbx` in ufbx v0.23.1
    /// (FBX SDK 2019.2;
    /// https://github.com/ufbx/ufbx/blob/v0.23.1/data/max_physical_material_properties_7500_ascii.fbx),
    /// read by ufbx.c L19628-L19679 under the `3dsMax|Parameters|` prefix
    /// (L22340-L22343). The meanings and defaults are Autodesk's: the 3ds Max
    /// 2024 help, "Physical Material Parameters"
    /// (https://help.autodesk.com/cloudhelp/2024/ENU/3DSMax-Lighting-Shading/files/GUID-C1328905-7783-4917-AB86-FC3CC19E8972.htm),
    /// and the MAXScript reference's Physical_Material
    /// (https://help.autodesk.com/cloudhelp/2018/ENU/MAXScript-Help/files/GUID-57562F6A-A8A1-4A28-BAE1-0D4729411214.htm).
    #[test]
    fn physical_layers_read() {
        let export = PHYSICAL_EXPORT;
        let (m, w) = layered_material(export, None);
        let ctx = "the 3ds Max export";
        // coating → clearcoatFactor, coat_roughness → its roughness, on the
        // material's roughness scale (help: 0.0 "completely smooth and
        // mirror-like", 1.0 "extremely rough"), as the base layer reads it.
        assert_eq!(m.clearcoat, Some(Clearcoat { factor: 0.38, roughness: 0.43, ..Default::default() }), "{ctx}");
        // transparency → transmissionFactor (help: "how opaque or
        // see-through the object appears. Light will be refracted").
        assert_eq!(m.transmission, Some(Transmission { factor: 0.09, texture: None }), "{ctx}");
        // reflectivity 1 is glTF's default specular; trans_ior 0.8 is outside
        // KHR_materials_ior; Physical has no sheen or thin film.
        assert_eq!((m.ior, m.specular.clone(), m.sheen.clone(), m.iridescence.clone()), (None, None, None, None), "{ctx}");
        assert_eq!((m.volume.clone(), m.anisotropy.clone(), m.dispersion), (None, None, None), "{ctx}");
        assert_warnings(
            &w,
            &[
                // coat_affect_* other than their 0.5 (Ruling A24g).
                &[
                    "3dsMax|Parameters|coat_color sRGB (0.39, 0.4, 0.41)",
                    "3dsMax|Parameters|coat_ior 0.44",
                    "3dsMax|Parameters|coat_affect_color 0.45",
                    "3dsMax|Parameters|coat_affect_roughness 0.46",
                ],
                &["thin-walled", "3dsMax|Parameters|trans_depth 0.059055"],
                &["tints transmitted light with the base colour, linear", "3ds Max Physical tints it linear (1, 1, 1)"],
                // Unlocked from the roughness, 0.06 (Ruling A24h).
                &["3dsMax|Parameters|trans_roughness 0.14"],
                &["3dsMax|Parameters|trans_ior 0.8"],
                &["3dsMax|Parameters|anisotropy 0.32"],
            ],
            ctx,
        );

        // The MAXScript defaults read no layer and warn about nothing:
        // trans_ior's and coat_ior's 1.52 among them (Ruling A24b), and an
        // anisotropy of 1.0, "not stretched" (help).
        let defaults = [
            max_class(true),
            max("3dsMax|Parameters|base_weight", 1.0),
            max4("3dsMax|Parameters|base_color", [0.5, 0.5, 0.5, 1.0]),
            max("3dsMax|Parameters|reflectivity", 1.0),
            max4("3dsMax|Parameters|refl_color", [1.0; 4]),
            max("3dsMax|Parameters|anisotropy", 1.0),
            max("3dsMax|Parameters|anisoangle", 0.0),
            max("3dsMax|Parameters|transparency", 0.0),
            max4("3dsMax|Parameters|trans_color", [1.0; 4]),
            max("3dsMax|Parameters|trans_depth", 0.0),
            max_bool("3dsMax|Parameters|trans_roughness_lock", true),
            max("3dsMax|Parameters|trans_roughness", 0.0),
            max_bool("3dsMax|Parameters|trans_roughness_inv", false),
            max("3dsMax|Parameters|trans_ior", 1.52),
            max_bool("3dsMax|Parameters|thin_walled", false),
            max("3dsMax|Parameters|coating", 0.0),
            max4("3dsMax|Parameters|coat_color", [1.0; 4]),
            max("3dsMax|Parameters|coat_roughness", 0.0),
            max_bool("3dsMax|Parameters|coat_roughness_inv", false),
            max("3dsMax|Parameters|coat_affect_color", 0.5),
            max("3dsMax|Parameters|coat_affect_roughness", 0.5),
            max("3dsMax|Parameters|coat_ior", 1.52),
        ]
        .concat();
        let (m, w) = layered_material(&defaults, None);
        assert!(no_layers(&m) && w.is_empty(), "the MAXScript defaults: {m:?} {w:?}");

        // Layers glTF holds as they are. coat_roughness_inv makes the coat
        // roughness a glossiness (MAXScript: "Invert Coating Roughness";
        // ufbx.c L19678, L20117-L20121); reflectivity is the weight glTF's
        // specularFactor is (help: "the weight multiplied by the angular
        // function"). An unlocked transparency roughness equal to the
        // roughness, here through trans_roughness_inv (1 - 0.7), and the
        // coat_affect_* defaults say nothing.
        let held = [
            max_class(true),
            max4("3dsMax|Parameters|base_color", [1.0; 4]),
            max("3dsMax|Parameters|roughness", 0.3),
            max("3dsMax|Parameters|coating", 1.0),
            max("3dsMax|Parameters|coat_roughness", 0.8),
            max_bool("3dsMax|Parameters|coat_roughness_inv", true),
            max("3dsMax|Parameters|coat_ior", 1.52),
            max("3dsMax|Parameters|coat_affect_color", 0.5),
            max("3dsMax|Parameters|coat_affect_roughness", 0.5),
            max("3dsMax|Parameters|transparency", 1.0),
            max4("3dsMax|Parameters|trans_color", [1.0; 4]),
            max_bool("3dsMax|Parameters|trans_roughness_lock", false),
            max("3dsMax|Parameters|trans_roughness", 0.7),
            max_bool("3dsMax|Parameters|trans_roughness_inv", true),
            max_bool("3dsMax|Parameters|thin_walled", true),
            max("3dsMax|Parameters|trans_ior", 1.33),
            max("3dsMax|Parameters|reflectivity", 0.5),
        ]
        .concat();
        let (m, w) = layered_material(&held, None);
        assert!(w.is_empty(), "held: {w:?}");
        let c = m.clearcoat.clone().expect("clearcoat");
        assert!(c.factor == 1.0 && close(c.roughness, 0.2), "{c:?}");
        assert_eq!(m.transmission, Some(Transmission { factor: 1.0, texture: None }));
        assert_eq!(m.ior, Some(1.33));
        assert_eq!(m.specular, Some(Specular { factor: 0.5, ..Default::default() }));

        // A locked transparency roughness follows the roughness (MAXScript
        // L81-L94), whatever trans_roughness says.
        let locked = [
            max_class(true),
            max4("3dsMax|Parameters|base_color", [1.0; 4]),
            max("3dsMax|Parameters|roughness", 0.3),
            max("3dsMax|Parameters|transparency", 1.0),
            max_bool("3dsMax|Parameters|thin_walled", true),
            max_bool("3dsMax|Parameters|trans_roughness_lock", true),
            max("3dsMax|Parameters|trans_roughness", 0.9),
        ]
        .concat();
        let (m, w) = layered_material(&locked, None);
        assert!(m.transmission.is_some() && w.is_empty(), "locked: {m:?} {w:?}");

        // A thin-walled surface has no depth (MAXScript L100-L107), so its
        // colour tints it whatever trans_depth says, and a colour equal to
        // the base colour needs no warning.
        let sheet = [
            max_class(true),
            max4("3dsMax|Parameters|base_color", [0.5, 0.5, 0.5, 1.0]),
            max("3dsMax|Parameters|transparency", 1.0),
            max4("3dsMax|Parameters|trans_color", [0.5, 0.5, 0.5, 1.0]),
            max("3dsMax|Parameters|trans_depth", 1.0),
            max_bool("3dsMax|Parameters|thin_walled", true),
        ]
        .concat();
        let (m, w) = layered_material(&sheet, None);
        assert!(m.transmission.is_some() && w.is_empty(), "a thin sheet: {m:?} {w:?}");

        // A texture on a layer property is named; the layer reads from its
        // value (coat_roughness unset: the MAXScript default, 0).
        let coat = [max_class(true), max4("3dsMax|Parameters|base_color", [1.0; 4]), max("3dsMax|Parameters|coating", 0.5)].concat();
        let (m, w) = layered_material(&coat, Some("3dsMax|Parameters|coat_map"));
        assert_eq!(m.clearcoat, Some(Clearcoat { factor: 0.5, roughness: 0.0, ..Default::default() }));
        assert_warnings(&w, &[&["\"3dsMax|Parameters|coat_map\"", "clearcoat", "stored value"]], "a textured coat");
    }

    /// The export's lines L355-L396, all of them, from
    /// `data/max_openpbr_material_7700_ascii.fbx` in ufbx v0.23.1,
    /// which `openpbr_layers_read` cites.
    const OPENPBR_EXPORT: &str = "\t\t\tP: \"3dsMax|ClassIDa\", \"int\", \"Integer\", \"\",-246079949\n\
            \t\t\tP: \"3dsMax|ClassIDb\", \"int\", \"Integer\", \"\",939201335\n\
            \t\t\tP: \"3dsMax|SuperClassID\", \"int\", \"Integer\", \"\",3072\n\
            \t\t\tP: \"3dsMax|Parameters\", \"Compound\", \"\", \"\"\n\
            \t\t\tP: \"3dsMax|Parameters|notes\", \"KString\", \"\", \"A\", \"\"\n\
            \t\t\tP: \"3dsMax|Parameters|base_weight\", \"Float\", \"\", \"A\",0.01\n\
            \t\t\tP: \"3dsMax|Parameters|base_color\", \"ColorAndAlpha\", \"\", \"A\",0.0199999995529652,0.0299999993294477,0.0399999991059303,0.0500000007450581\n\
            \t\t\tP: \"3dsMax|Parameters|base_metalness\", \"Float\", \"\", \"A\",0.06\n\
            \t\t\tP: \"3dsMax|Parameters|base_diffuse_roughness\", \"Float\", \"\", \"A\",0.07\n\
            \t\t\tP: \"3dsMax|Parameters|specular_weight\", \"Float\", \"\", \"A\",0.08\n\
            \t\t\tP: \"3dsMax|Parameters|specular_color\", \"ColorAndAlpha\", \"\", \"A\",0.0900000035762787,0.100000001490116,0.109999999403954,0.119999997317791\n\
            \t\t\tP: \"3dsMax|Parameters|specular_roughness\", \"Float\", \"\", \"A\",0.14\n\
            \t\t\tP: \"3dsMax|Parameters|specular_roughness_anisotropy\", \"Float\", \"\", \"A\",0.15\n\
            \t\t\tP: \"3dsMax|Parameters|specular_ior\", \"Float\", \"\", \"A\",1.3\n\
            \t\t\tP: \"3dsMax|Parameters|transmission_weight\", \"Float\", \"\", \"A\",0.16\n\
            \t\t\tP: \"3dsMax|Parameters|transmission_color\", \"ColorAndAlpha\", \"\", \"A\",0.170000001788139,0.180000007152557,0.189999997615814,0.200000002980232\n\
            \t\t\tP: \"3dsMax|Parameters|transmission_depth\", \"Float\", \"\", \"A\",0.21\n\
            \t\t\tP: \"3dsMax|Parameters|transmission_scatter\", \"ColorAndAlpha\", \"\", \"A\",0.219999998807907,0.230000004172325,0.239999994635582,0.25\n\
            \t\t\tP: \"3dsMax|Parameters|transmission_scatter_anisotropy\", \"Float\", \"\", \"A\",0.26\n\
            \t\t\tP: \"3dsMax|Parameters|transmission_dispersion_scale\", \"Float\", \"\", \"A\",0.27\n\
            \t\t\tP: \"3dsMax|Parameters|transmission_dispersion_abbe_number\", \"Float\", \"\", \"A\",0.28\n\
            \t\t\tP: \"3dsMax|Parameters|subsurface_weight\", \"Float\", \"\", \"A\",0.29\n\
            \t\t\tP: \"3dsMax|Parameters|subsurface_color\", \"ColorAndAlpha\", \"\", \"A\",0.300000011920929,0.310000002384186,0.319999992847443,0.330000013113022\n\
            \t\t\tP: \"3dsMax|Parameters|subsurface_radius\", \"Float\", \"\", \"A\",0.39\n\
            \t\t\tP: \"3dsMax|Parameters|subsurface_radius_scale\", \"ColorAndAlpha\", \"\", \"A\",0.349999994039536,0.360000014305115,0.370000004768372,0.379999995231628\n\
            \t\t\tP: \"3dsMax|Parameters|subsurface_scatter_anisotropy\", \"Float\", \"\", \"A\",0.34\n\
            \t\t\tP: \"3dsMax|Parameters|coat_weight\", \"Float\", \"\", \"A\",0.4\n\
            \t\t\tP: \"3dsMax|Parameters|coat_color\", \"ColorAndAlpha\", \"\", \"A\",0.409999996423721,0.419999986886978,0.430000007152557,0.439999997615814\n\
            \t\t\tP: \"3dsMax|Parameters|coat_roughness\", \"Float\", \"\", \"A\",0.46\n\
            \t\t\tP: \"3dsMax|Parameters|coat_roughness_anisotropy\", \"Float\", \"\", \"A\",0.47\n\
            \t\t\tP: \"3dsMax|Parameters|coat_ior\", \"Float\", \"\", \"A\",4.5\n\
            \t\t\tP: \"3dsMax|Parameters|coat_darkening\", \"Float\", \"\", \"A\",0.48\n\
            \t\t\tP: \"3dsMax|Parameters|fuzz_weight\", \"Float\", \"\", \"A\",0.49\n\
            \t\t\tP: \"3dsMax|Parameters|fuzz_color\", \"ColorAndAlpha\", \"\", \"A\",0.5,0.509999990463257,0.519999980926514,0.529999971389771\n\
            \t\t\tP: \"3dsMax|Parameters|fuzz_roughness\", \"Float\", \"\", \"A\",0.54\n\
            \t\t\tP: \"3dsMax|Parameters|emission_weight\", \"Float\", \"\", \"A\",0.55\n\
            \t\t\tP: \"3dsMax|Parameters|emission_color\", \"ColorAndAlpha\", \"\", \"A\",0.560000002384186,0.569999992847443,0.579999983310699,0.589999973773956\n\
            \t\t\tP: \"3dsMax|Parameters|emission_luminance\", \"Float\", \"\", \"A\",6000\n\
            \t\t\tP: \"3dsMax|Parameters|thin_film_weight\", \"Float\", \"\", \"A\",0.61\n\
            \t\t\tP: \"3dsMax|Parameters|thin_film_thickness\", \"Float\", \"\", \"A\",0.62\n\
            \t\t\tP: \"3dsMax|Parameters|thin_film_ior\", \"Float\", \"\", \"A\",6.3\n\
            \t\t\tP: \"3dsMax|Parameters|geometry_thin_walled\", \"Bool\", \"\", \"A\",1\n";

    /// 3ds Max's OpenPBR material reads the glTF layers its properties map
    /// to, and warns about what glTF can't hold.
    ///
    /// The first case's lines are copied from a 3ds Max export,
    /// `data/max_openpbr_material_7700_ascii.fbx` in ufbx v0.23.1 (FBX SDK
    /// 2020.3.5;
    /// https://github.com/ufbx/ufbx/blob/v0.23.1/data/max_openpbr_material_7700_ascii.fbx),
    /// read by ufbx.c L19703-L19751 under the `3dsMax|Parameters|` prefix
    /// (L22344-L22347). The meanings, units and defaults are the OpenPBR
    /// Surface spec's, v1.1.1
    /// (https://github.com/AcademySoftwareFoundation/OpenPBR/blob/v1.1.1/index.html).
    #[test]
    fn openpbr_layers_read() {
        let export = OPENPBR_EXPORT;
        let (m, w) = layered_material(export, None);
        let ctx = "the 3ds Max export";
        // coat_weight → clearcoatFactor; coat_roughness r → alpha = r², as
        // glTF's (OpenPBR L398-L401, L421).
        assert_eq!(m.clearcoat, Some(Clearcoat { factor: 0.4, roughness: 0.46, ..Default::default() }), "{ctx}");
        // transmission_weight → transmissionFactor (OpenPBR L839).
        assert_eq!(m.transmission, Some(Transmission { factor: 0.16, texture: None }), "{ctx}");
        // specular_ior → ior (OpenPBR L518).
        assert_eq!(m.ior, Some(1.3), "{ctx}");
        // thin_film_weight → iridescenceFactor; thin_film_thickness is in
        // micrometres (OpenPBR L854, L882), glTF's in nanometres.
        let f = m.iridescence.clone().expect("iridescence");
        assert!(f.factor == 0.61 && f.ior == 6.3 && close(f.thickness_max, 620.0) && f.thickness_min == 100.0, "{ctx}: {f:?}");
        assert!(f.texture.is_none() && f.thickness_texture.is_none(), "{ctx}: {f:?}");
        assert_eq!((m.specular.clone(), m.sheen.clone(), m.volume.clone(), m.anisotropy.clone(), m.dispersion), (None, None, None, None, None), "{ctx}");
        assert_warnings(
            &w,
            &[
                &["3dsMax|Parameters|specular_weight 0.08"],
                &["3dsMax|Parameters|specular_color sRGB (0.09, 0.1, 0.11)"],
                &["3dsMax|Parameters|specular_roughness_anisotropy 0.15"],
                // Thin-walled: the colour tints the sheet (OpenPBR L1259).
                &["tints transmitted light with the base colour, linear", "where OpenPBR tints it linear"],
                // transmission_dispersion_scale 0.27 goes unwarned: the sheet
                // is thin-walled, whose refraction is un-deflected (L1259).
                // coat_darkening other than its 1 (Ruling A24g).
                &[
                    "3dsMax|Parameters|coat_color sRGB (0.41, 0.42, 0.43)",
                    "3dsMax|Parameters|coat_roughness_anisotropy 0.47",
                    "3dsMax|Parameters|coat_ior 4.5",
                    "3dsMax|Parameters|coat_darkening 0.48",
                ],
                &["3dsMax|Parameters|fuzz_weight 0.49"],
            ],
            ctx,
        );

        // OpenPBR's defaults (L514-L518, L839-L845, L881-L883, L936-L941,
        // L1121-L1123) read no layer and warn about nothing: coat_ior's 1.6
        // among them (Ruling A24b).
        let defaults = [
            max_class(false),
            max("3dsMax|Parameters|base_weight", 1.0),
            max4("3dsMax|Parameters|base_color", [0.8, 0.8, 0.8, 1.0]),
            max("3dsMax|Parameters|base_metalness", 0.0),
            max("3dsMax|Parameters|specular_weight", 1.0),
            max4("3dsMax|Parameters|specular_color", [1.0; 4]),
            max("3dsMax|Parameters|specular_roughness", 0.3),
            max("3dsMax|Parameters|specular_roughness_anisotropy", 0.0),
            max("3dsMax|Parameters|specular_ior", 1.5),
            max("3dsMax|Parameters|transmission_weight", 0.0),
            max4("3dsMax|Parameters|transmission_color", [1.0; 4]),
            max("3dsMax|Parameters|transmission_depth", 0.0),
            max("3dsMax|Parameters|transmission_dispersion_scale", 0.0),
            max("3dsMax|Parameters|transmission_dispersion_abbe_number", 20.0),
            max("3dsMax|Parameters|coat_weight", 0.0),
            max4("3dsMax|Parameters|coat_color", [1.0; 4]),
            max("3dsMax|Parameters|coat_roughness", 0.0),
            max("3dsMax|Parameters|coat_roughness_anisotropy", 0.0),
            max("3dsMax|Parameters|coat_ior", 1.6),
            max("3dsMax|Parameters|coat_darkening", 1.0),
            max("3dsMax|Parameters|fuzz_weight", 0.0),
            max("3dsMax|Parameters|thin_film_weight", 0.0),
            max("3dsMax|Parameters|thin_film_thickness", 0.5),
            max("3dsMax|Parameters|thin_film_ior", 1.4),
            max_bool("3dsMax|Parameters|geometry_thin_walled", false),
        ]
        .concat();
        let (m, w) = layered_material(&defaults, None);
        assert!(no_layers(&m) && w.is_empty(), "OpenPBR's defaults: {m:?} {w:?}");

        // Layers glTF holds as they are: no warning.
        let held = [
            max_class(false),
            max("3dsMax|Parameters|base_metalness", 0.0),
            max4("3dsMax|Parameters|base_color", [1.0; 4]),
            max("3dsMax|Parameters|coat_weight", 1.0),
            max("3dsMax|Parameters|coat_roughness", 0.2),
            max("3dsMax|Parameters|coat_ior", 1.6),
            max("3dsMax|Parameters|coat_darkening", 1.0),
            max("3dsMax|Parameters|thin_film_weight", 1.0),
            max("3dsMax|Parameters|thin_film_thickness", 0.25),
            max("3dsMax|Parameters|thin_film_ior", 1.33),
            max("3dsMax|Parameters|specular_ior", 2.4),
            max("3dsMax|Parameters|transmission_weight", 0.5),
            max4("3dsMax|Parameters|transmission_color", [1.0; 4]),
            max_bool("3dsMax|Parameters|geometry_thin_walled", true),
        ]
        .concat();
        let (m, w) = layered_material(&held, None);
        assert!(w.is_empty(), "held: {w:?}");
        assert_eq!(m.clearcoat, Some(Clearcoat { factor: 1.0, roughness: 0.2, ..Default::default() }));
        let f = m.iridescence.clone().expect("iridescence");
        assert!(f.factor == 1.0 && f.ior == 1.33 && close(f.thickness_max, 250.0), "{f:?}");
        assert_eq!(m.ior, Some(2.4));
        assert_eq!(m.transmission, Some(Transmission { factor: 0.5, texture: None }));

        // A thin-walled sheet's colour is its transmittance whatever its
        // depth (OpenPBR L1259), so a colour equal to the base colour, which
        // glTF tints with, needs no warning.
        let sheet = [
            max_class(false),
            max("3dsMax|Parameters|base_metalness", 0.0),
            max4("3dsMax|Parameters|base_color", [0.5, 0.5, 0.5, 1.0]),
            max("3dsMax|Parameters|transmission_weight", 1.0),
            max4("3dsMax|Parameters|transmission_color", [0.5, 0.5, 0.5, 1.0]),
            max("3dsMax|Parameters|transmission_depth", 1.0),
            max_bool("3dsMax|Parameters|geometry_thin_walled", true),
        ]
        .concat();
        let (m, w) = layered_material(&sheet, None);
        assert!(w.is_empty() && m.transmission.is_some(), "a thin sheet: {m:?} {w:?}");

        // A negative thickness reads as OpenPBR's default, 0.5 micrometres
        // (Ruling A16: thicknesses are 0 or more), and a texture on a layer
        // property is named while the layer reads from its values.
        let film = [
            max_class(false),
            max("3dsMax|Parameters|base_metalness", 0.0),
            max("3dsMax|Parameters|thin_film_weight", 1.0),
            max("3dsMax|Parameters|thin_film_thickness", -1.0),
        ]
        .concat();
        let (m, w) = layered_material(&film, Some("3dsMax|Parameters|thin_film_weight_map"));
        let f = m.iridescence.clone().expect("iridescence");
        assert!(f.factor == 1.0 && close(f.thickness_max, 500.0) && f.ior == 1.4, "{f:?}");
        assert_warnings(
            &w,
            &[&["3dsMax|Parameters|thin_film_thickness -1"], &["\"3dsMax|Parameters|thin_film_weight_map\"", "iridescence", "stored value"]],
            "a negative, textured film",
        );
        // A thickness whose nanometres overflow a 32-bit float leaves the
        // film out, rather than handing `validate` an infinity.
        let thick = [
            max_class(false),
            max("3dsMax|Parameters|base_metalness", 0.0),
            max("3dsMax|Parameters|thin_film_weight", 1.0),
            max("3dsMax|Parameters|thin_film_thickness", 1e36),
        ]
        .concat();
        let (m, w) = layered_material(&thick, None);
        assert!(m.iridescence.is_none(), "{m:?}");
        assert_warnings(&w, &[&["3dsMax|Parameters|thin_film_thickness", "too thick to hold"]], "a thick film");

        // A solid surface names the dispersion it loses with its refraction
        // (Rulings A24c, A24a).
        let solid = [
            max_class(false),
            max("3dsMax|Parameters|base_metalness", 0.0),
            max4("3dsMax|Parameters|base_color", [1.0; 4]),
            max("3dsMax|Parameters|transmission_weight", 1.0),
            max4("3dsMax|Parameters|transmission_color", [1.0; 4]),
            max_bool("3dsMax|Parameters|geometry_thin_walled", false),
            max("3dsMax|Parameters|transmission_dispersion_scale", 0.5),
        ]
        .concat();
        let (m, w) = layered_material(&solid, None);
        assert!(m.transmission.is_some() && m.dispersion.is_none(), "{m:?}");
        assert_warnings(&w, &[&["thin-walled", "3dsMax|Parameters|transmission_dispersion_scale 0.5"]], "a solid, dispersive glass");
    }

    /// What the FBX reader reads into the layers survives FBX → glTF: glTF
    /// keeps every material layer (its row of the capability table holds
    /// them all), so a GLB written from the read scene reports no material
    /// loss and reads back with the same layers.
    #[test]
    fn layers_read_from_fbx_survive_glb() {
        let props = [
            maya("Maya|base", 1.0),
            maya3("Maya|baseColor", [0.8; 3]),
            maya("Maya|coat", 1.0),
            maya("Maya|coatRoughness", 0.25),
            maya("Maya|transmission", 0.5),
            maya3("Maya|transmissionColor", [0.8; 3]),
            maya_bool("Maya|thinWalled", true),
            maya("Maya|specular", 0.5),
            maya("Maya|specularIOR", 1.45),
            maya("Maya|sheen", 1.0),
            maya3("Maya|sheenColor", [0.3, 0.2, 0.1]),
            maya("Maya|sheenRoughness", 0.36),
            maya("Maya|thinFilmThickness", 350.0),
            maya("Maya|thinFilmIOR", 1.33),
        ]
        .concat();
        let fbx = layered_scene(&props, None, &NoSidecars).scene;
        let m = &fbx.materials[0];
        let read = |m: &Material| MaterialLayer::ALL.iter().filter(|l| m.has_layer(**l)).count();
        assert_eq!(read(m), 6, "clearcoat, transmission, ior, specular, sheen and iridescence: {m:?}");
        let opts = super::super::WriteOptions::default();
        let glb = super::super::write_scene(&fbx, super::super::MeshFormat::Glb, opts).unwrap_or_else(|e| panic!("{e}"));
        assert!(glb.losses.iter().all(|l| l.kind != super::super::LossKind::Materials), "{:?}", glb.losses);
        let back = super::super::read_scene(&glb.package.files[0].bytes, super::super::MeshFormat::Glb, &NoSidecars).unwrap_or_else(|e| panic!("{e}"));
        let b = &back.scene.materials[0];
        assert_eq!(
            (&b.clearcoat, &b.transmission, &b.volume, b.ior, &b.specular, &b.sheen, &b.iridescence, &b.anisotropy, b.dispersion),
            (&m.clearcoat, &m.transmission, &m.volume, m.ior, &m.specular, &m.sheen, &m.iridescence, &m.anisotropy, m.dispersion)
        );
    }

    /// A layer note quotes its material's name, and an FBX can name a
    /// material at length: the note clips the name to 256 characters, marked
    /// "…" past that (Ruling A45), and quotes an ordinary one whole.
    #[test]
    fn a_layer_note_clips_a_long_material_name() {
        let long = "n".repeat(100_000);
        let props = [maya3("Maya|baseColor", [1.0; 3]), maya("Maya|coat", 5.0)].concat();
        let doc = layered_doc(&props, None).replace("\"Material::mat\"", &format!("\"Material::{long}\""));
        let r = super::super::read_scene(doc.as_bytes(), super::super::MeshFormat::Fbx, &NoSidecars).unwrap_or_else(|e| panic!("{e}"));
        let note = r.warnings.iter().find(|w| w.contains("Maya|coat 5")).unwrap_or_else(|| panic!("no coat note in {} warning(s)", r.warnings.len()));
        let n = note.chars().count();
        assert!(n < 400 && note.contains("nnn…"), "{n} characters: {}", note.chars().take(160).collect::<String>());
        let (_, warnings) = layered_material(&props, None);
        assert_warnings(&warnings, &[&["material \"mat\" has Maya|coat 5, outside 0 to 1"]], "an ordinary name");
    }

    /// `src`, an ASCII FBX, as the binary FBX of the same records: parsed by
    /// `fbx_dom::parse_ascii` and written by litegen's own DOM writer,
    /// `fbx_dom::write_records`, as a 7400 file (the layout that writer
    /// emits; the semantic reader never sees which). As binary FBX spells
    /// them, an object's `"Class::Name"` becomes `"Name\0\x01Class"`, and the
    /// arrays are written as the SDK writes them: `Vertices` and a Line's
    /// `Points` as doubles, `PolygonVertexIndex`, `PointsIndex` and
    /// `Materials` as 32-bit integers.
    fn binary_twin(src: &str) -> Vec<u8> {
        use super::super::fbx_dom::{parse_ascii, write_records, WProp, WRec, BINARY_MAGIC};
        fn record(dom: &Dom<'_>, r: RecId, object: bool) -> WRec {
            let name = dom.name(r);
            let props = dom.props(r).unwrap_or_else(|e| panic!("{name}: {e}"));
            let props = props
                .into_iter()
                .enumerate()
                .map(|(i, p)| match p {
                    Prop::Bool(b) => WProp::Byte(u8::from(b)),
                    Prop::Int(v) => i32::try_from(v).map_or(WProp::I64(v), WProp::I32),
                    Prop::Float(v) => WProp::F64(v),
                    Prop::Str(s) if object && i == 1 => {
                        let s = String::from_utf8_lossy(&s).into_owned();
                        let (class, object_name) = s.split_once("::").unwrap_or_else(|| panic!("{name} {s:?} has no class"));
                        WProp::Str([object_name.as_bytes(), b"\0\x01", class.as_bytes()].concat())
                    }
                    Prop::Str(s) => WProp::Str(s.into_owned()),
                    Prop::Raw(r) => WProp::Raw(r.to_vec()),
                    Prop::Array(a) if matches!(name, "Vertices" | "Points") => WProp::ArrF64(dom.f64s(&a, name).unwrap()),
                    Prop::Array(a) => WProp::ArrI32(dom.i32s(&a, name).unwrap_or_else(|e| panic!("{name}: {e}"))),
                })
                .collect();
            let children = dom.children(r).map(|c| record(dom, c, name == "Objects")).collect();
            // `WRec` names its records with `&'static str`s: a test's few
            // dozen record names are leaked.
            WRec { name: Box::leak(Box::<str>::from(name)), props, children, force_null: false }
        }
        let dom = parse_ascii(src.as_bytes()).unwrap_or_else(|e| panic!("{e}"));
        let top: Vec<WRec> = dom.children(ROOT).map(|r| record(&dom, r, false)).collect();
        let mut out = BINARY_MAGIC.to_vec();
        out.extend_from_slice(&[0x1A, 0x00]);
        out.extend_from_slice(&7400u32.to_le_bytes());
        write_records(&top, &mut out).unwrap();
        out
    }

    /// The layer properties of Arnold's Standard Surface, 3ds Max's Physical
    /// Material and OpenPBR, through the reader: the binary twins
    /// ([`binary_twin`]) of the three export documents these tests read
    /// (`ARNOLD_EXPORT`, `PHYSICAL_EXPORT` and `OPENPBR_EXPORT` in
    /// `layered_doc`) and of `arnold_layers_read`'s textured coat, each swept
    /// by [`layer_sweep`] with every bit of the material's `Properties70` among
    /// the single flips. Every reading is `Ok` or `Err`. No cut reaches a
    /// material: each record's end offset lies past it. The documents are
    /// 1.4–4.8 KB as they are, so unlike the other codecs' sweeps this one
    /// has no lean twin and no `_full` test (Ruling A40a).
    #[test]
    fn fbx_layer_inputs_survive_truncation_and_bit_flips() {
        let coat = [maya3("Maya|baseColor", [1.0; 3]), maya("Maya|coat", 0.5)].concat();
        let docs = [
            ("the Standard Surface export", layered_doc(ARNOLD_EXPORT, None)),
            ("the Physical Material export", layered_doc(PHYSICAL_EXPORT, None)),
            ("the OpenPBR export", layered_doc(OPENPBR_EXPORT, None)),
            ("a textured Standard Surface coat", layered_doc(&coat, Some("Maya|coat"))),
        ];
        let read = |bytes: &[u8]| super::super::read_scene(bytes, super::super::MeshFormat::Fbx, &NoSidecars);
        for (name, text) in docs {
            let file = binary_twin(&text);
            let ascii = read(text.as_bytes()).unwrap_or_else(|e| panic!("{name}, as text: {e}"));
            let whole = read(&file).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!((&whole.scene, &whole.warnings), (&ascii.scene, &ascii.warnings), "{name} reads as its text does");
            assert!(!no_layers(&whole.scene.materials[0]), "{name} itself carries layers");
            layer_sweep(name, &file, span(name, &file, "Properties70", "Connections"), |bytes| {
                let _ = read(bytes);
            });
        }
    }

    // ── Line geometry ───────────────────────────────────────────────────────

    /// A Geometry (id 99) of subclass `sub`, as the SDK's FbxLine stores a
    /// line: `Points`, three doubles each, and `PointsIndex`, whose negative
    /// entries `-(e + 1)` end a strip at point `e`.
    fn line_geometry(sub: &str, points: &[f64], index: &[i32]) -> String {
        let p: Vec<String> = points.iter().map(|v| v.to_string()).collect();
        let i: Vec<String> = index.iter().map(|v| v.to_string()).collect();
        format!(
            "\tGeometry: 99, \"Geometry::lines\", \"{sub}\" {{\n\t\tPoints: *{} {{\n\t\t\ta: {}\n\t\t}}\n\t\tPointsIndex: *{} {{\n\t\t\ta: {}\n\t\t}}\n\t}}\n",
            p.len(),
            p.join(","),
            i.len(),
            i.join(",")
        )
    }

    /// An ASCII FBX of one model (id 10, `model_props` its Properties70
    /// lines) under the root, with `objects` after it and `connections`
    /// after the model's own.
    fn line_doc(model_props: &str, objects: &str, connections: &str) -> String {
        format!(
            "; FBX 7.5.0 project file\nFBXHeaderExtension:  {{\n\tFBXVersion: 7500\n}}\nObjects:  {{\n\
             \tModel: 10, \"Model::m\", \"Mesh\" {{\n\t\tProperties70:  {{\n{model_props}\t\t}}\n\t}}\n\
             {objects}}}\nConnections:  {{\n\tC: \"OO\",10,0\n{connections}}}\n"
        )
    }

    /// `src` read as the text it is and as its binary twin, which must read
    /// alike (scene and warnings, or the same refusal).
    fn read_text_and_binary(src: &str) -> Result<SceneRead, MeshError> {
        let read = |bytes: &[u8]| super::super::read_scene(bytes, super::super::MeshFormat::Fbx, &NoSidecars);
        let (text, binary) = (read(src.as_bytes()), read(&binary_twin(src)));
        match (&text, &binary) {
            (Ok(t), Ok(b)) => assert_eq!((&t.scene, &t.warnings), (&b.scene, &b.warnings), "the binary twin reads as its text does"),
            (Err(t), Err(b)) => assert_eq!(t, b, "the binary twin is refused as its text is"),
            _ => panic!("text: {:?}; binary: {:?}", text.as_ref().err(), binary.as_ref().err()),
        }
        text
    }

    /// Each segment of the scene's one lines primitive, as its two positions.
    fn segments(scene: &Scene) -> Vec<[[f32; 3]; 2]> {
        let prims: Vec<&Primitive> = scene.meshes.iter().flat_map(|m| &m.primitives).collect();
        assert_eq!(prims.len(), 1, "one primitive");
        let p = prims[0];
        assert_eq!(p.topology, super::super::scene::Topology::Lines);
        p.indices.chunks(2).map(|s| [p.positions[s[0] as usize], p.positions[s[1] as usize]]).collect()
    }

    const LINE_CONNECTIONS: &str = "\tC: \"OO\",99,10\n";

    /// `Line` geometry reads as a lines primitive: `Points` and `PointsIndex`,
    /// a negative entry `-(e + 1)` ending a strip at point `e`, as
    /// `PolygonVertexIndex` ends a polygon. Two strips (0-1-2, then
    /// 3-4-5-6) give 2 + 3 segments and none joins them; a point in no strip
    /// is not carried; the model's geometric transform and first material
    /// apply as a mesh's do.
    #[test]
    fn fbx_line_geometry_reads() {
        use super::super::scene::Topology;
        let points = [0., 0., 0., 1., 0., 0., 1., 1., 0., 3., 0., 0., 4., 0., 0., 4., 1., 0., 3., 1., 0., 9., 9., 9.];
        let index = [0, 1, -3, 3, 4, 5, -7];
        let model = "\t\t\tP: \"GeometricTranslation\", \"Vector3D\", \"Vector\", \"\",0,10,0\n\
                     \t\t\tP: \"GeometricScaling\", \"Vector3D\", \"Vector\", \"\",2,1,1\n";
        let material = "\tMaterial: 20, \"Material::mat\", \"\" {\n\t\tShadingModel: \"lambert\"\n\t\tProperties70:  {\n\t\t}\n\t}\n";
        let objects = [line_geometry("Line", &points, &index), material.to_string()].concat();
        let src = line_doc(model, &objects, &format!("{LINE_CONNECTIONS}\tC: \"OO\",20,10\n"));

        // A Line-only file reads (the gate), as text and as binary.
        let read = read_text_and_binary(&src);
        assert!(read.is_ok(), "a file whose only geometry is a Line reads: {:?}", read.as_ref().err());
        let r = read.unwrap_or_else(|e| panic!("{e}"));
        let p = &r.scene.meshes[0].primitives[0];
        assert_eq!(p.topology, Topology::Lines);
        // The geometric transform: (2x, y + 10, z). Point 7 is in no strip.
        let want: Vec<[f32; 3]> = [[0., 10., 0.], [2., 10., 0.], [2., 11., 0.], [6., 10., 0.], [8., 10., 0.], [8., 11., 0.], [6., 11., 0.]].to_vec();
        assert_eq!(p.positions, want);
        // Five segments, and none from point 2 to point 3.
        assert_eq!(p.indices, vec![0, 1, 1, 2, 3, 4, 4, 5, 5, 6]);
        assert!(p.normals.is_none() && p.tangents.is_none() && p.colors.is_none() && p.uvs.is_empty() && p.skin.is_none());
        assert_eq!((r.scene.drawable_count(), r.scene.triangle_count()), (5, 0));
        // The model's first material binds, and its node draws the mesh.
        assert_eq!((p.material, r.scene.materials.len()), (Some(0), 1));
        assert_eq!(r.scene.nodes.iter().filter(|n| n.mesh == Some(0)).count(), 1);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);

        // The flat path holds triangles alone: a Line-only file is Empty there.
        assert_eq!(super::super::read(src.as_bytes(), super::super::MeshFormat::Fbx).err(), Some(MeshError::Empty));

        // A NurbsCurve beside it still warns, now saying nothing of lines,
        // and the Line still reads.
        let curve = "\tGeometry: 98, \"Geometry::curve\", \"NurbsCurve\" {\n\t}\n\tModel: 11, \"Model::c\", \"Mesh\" {\n\t}\n";
        let both = line_doc(model, &[line_geometry("Line", &points, &index), curve.to_string()].concat(), &format!("{LINE_CONNECTIONS}\tC: \"OO\",11,0\n\tC: \"OO\",98,11\n"));
        let r = read_text_and_binary(&both).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            r.warnings,
            vec!["\"NurbsCurve\" geometry (NURBS, patches and curves) is not carried: the scene models polygon meshes and line segments only"]
        );
        assert_eq!(r.scene.drawable_count(), 5);
        // A file of nothing else is still refused.
        let only = line_doc("", curve, "\tC: \"OO\",11,0\n\tC: \"OO\",98,11\n");
        assert_eq!(
            read_text_and_binary(&only).err(),
            Some(MeshError::Unsupported("fbx: the file's only geometry is NURBS, patches or curves, which this converter does not tessellate".into()))
        );
    }

    /// A strip of c points gives c - 1 segments, in the order written; a strip
    /// of one point gives none; and a trailing strip with no end stops where
    /// the array does (Ruling I10-R10: assimp v6.0.5's `ConvertLine` wraps it
    /// back to the first index, the FBX SDK always ends a strip).
    #[test]
    fn fbx_line_strips_end_where_the_file_ends_them() {
        let points = [0., 0., 0., 1., 0., 0., 2., 0., 0.];
        let segs = |index: &[i32]| segments(&read_text_and_binary(&line_doc("", &line_geometry("Line", &points, index), LINE_CONNECTIONS)).unwrap_or_else(|e| panic!("{index:?}: {e}")).scene);
        let (a, b) = ([[0., 0., 0.], [1., 0., 0.]], [[1., 0., 0.], [2., 0., 0.]]);
        // No negative entry: two segments, and no closing 2 to 0.
        assert_eq!(segs(&[0, 1, 2]), vec![a, b]);
        assert_eq!(segs(&[0, 1, -3]), vec![a, b]);
        // A strip of one point is nothing; the next strip stands alone.
        assert_eq!(segs(&[-1, 1, -3]), vec![b]);
        // Two strips of two points: nothing between them.
        assert_eq!(segs(&[0, -2, 1, -3]), vec![a, b]);
        // The direction written is kept.
        assert_eq!(segs(&[1, -1]), vec![[[1., 0., 0.], [0., 0., 0.]]]);
    }

    /// A Line is one geometry of its model, as a mesh is: the first connected
    /// is read and the rest are named as unread.
    #[test]
    fn fbx_a_line_counts_as_one_of_a_models_geometries() {
        use super::super::scene::Topology;
        let tri = "\tGeometry: 98, \"Geometry::tri\", \"Mesh\" {\n\t\tVertices: *9 {\n\t\t\ta: 0,0,0,1,0,0,0,1,0\n\t\t}\n\t\tPolygonVertexIndex: *3 {\n\t\t\ta: 0,1,-3\n\t\t}\n\t}\n";
        let objects = [line_geometry("Line", &[0., 0., 0., 1., 0., 0.], &[0, -2]), tri.to_string()].concat();
        for (first, kind, count) in [("99,10", Topology::Lines, 1), ("98,10", Topology::Triangles, 1)] {
            let other = if first == "99,10" { "98,10" } else { "99,10" };
            let r = read_text_and_binary(&line_doc("", &objects, &format!("\tC: \"OO\",{first}\n\tC: \"OO\",{other}\n"))).unwrap_or_else(|e| panic!("{e}"));
            let prims: Vec<&Primitive> = r.scene.meshes.iter().flat_map(|m| &m.primitives).collect();
            assert_eq!((prims.len(), prims[0].topology, r.scene.drawable_count()), (1, kind, count), "{first} first");
            assert_eq!(r.warnings, vec!["model \"m\" has 2 geometries; only the first was read"], "{first} first");
        }
    }

    /// What a Line cannot say is refused, as a polygon's is, or skipped with
    /// a name: an index out of range, a coordinate list that is not whole
    /// points, a segment on a point that is not finite; and a Line with no
    /// segment left draws nothing.
    #[test]
    fn fbx_line_geometry_refuses_what_it_cannot_index() {
        let doc = |points: &[f64], index: &[i32]| line_doc("", &line_geometry("Line", points, index), LINE_CONNECTIONS);
        let three = [0., 0., 0., 1., 0., 0., 2., 0., 0.];
        for index in [vec![0, 3], vec![0, -4], vec![0, i32::MIN]] {
            let err = read_text_and_binary(&doc(&three, &index)).err();
            assert!(matches!(&err, Some(MeshError::Malformed(m)) if m.contains("names point") && m.contains("of 3")), "{index:?}: {err:?}");
        }
        let err = read_text_and_binary(&doc(&[0., 0., 0., 1.], &[0, -1])).err();
        assert!(matches!(&err, Some(MeshError::Malformed(m)) if m.contains("4 point coordinates, not a multiple of 3")), "{err:?}");
        // Point 1 is infinite: the two segments on it go, with a note, and
        // the third stands (the points it names are kept, the others not).
        let off = [0., 0., 0., f64::INFINITY, 0., 0., 2., 0., 0., 3., 0., 0.];
        let r = read_text_and_binary(&doc(&off, &[0, 1, 2, -4])).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(segments(&r.scene), vec![[[2., 0., 0.], [3., 0., 0.]]]);
        assert_eq!(r.scene.meshes[0].primitives[0].positions.len(), 2);
        assert_eq!(r.warnings, vec!["geometry \"lines\" has 2 line segment(s) on a non-finite point; they were skipped"]);
        // Nothing left to draw.
        assert_eq!(read_text_and_binary(&doc(&off, &[0, -2])).err(), Some(MeshError::Empty));
        assert_eq!(read_text_and_binary(&doc(&three, &[-1])).err(), Some(MeshError::Empty));
    }

    /// A Line's segments (two indices each) and its points are charged to the
    /// scene's budgets before they are allocated, as a mesh's polygons are:
    /// what exactly fits is read, one over is `Unsupported`.
    #[test]
    fn fbx_line_geometry_is_charged_to_the_scene_budgets() {
        // The fourth point is named by no segment: it is no vertex of the
        // scene, so it is not charged.
        let src = line_doc("", &line_geometry("Line", &[0., 0., 0., 1., 0., 0., 1., 1., 0., 9., 9., 9.], &[0, 1, -3]), LINE_CONNECTIONS);
        let dom = super::super::fbx::parse(src.as_bytes()).unwrap_or_else(|e| panic!("{e}"));
        // 2 segments are 4 indices; 3 points named are 3 vertices. What it
        // builds, with the totals the reader holds after.
        let build = |indices_before: usize, vertices_before: usize| {
            let side = NoSidecars;
            let mut r = Reader::new(&dom, &side);
            r.header().unwrap();
            r.index().unwrap();
            r.total_indices = indices_before;
            r.total_vertices = vertices_before;
            let built = r.build_lines(99, &[], &math::IDENTITY).map(|m| m.is_some());
            (built, r.total_indices, r.total_vertices)
        };
        let (built, indices, vertices) = build(MAX_SCENE_INDICES - 4, MAX_SCENE_VERTICES - 3);
        assert_eq!(built, Ok(true));
        // What it builds is added to the totals, for the geometries after it.
        assert_eq!((indices, vertices), (MAX_SCENE_INDICES, MAX_SCENE_VERTICES), "met to the index and to the vertex");
        let (built, indices, vertices) = build(10, 20);
        assert_eq!((built, indices, vertices), (Ok(true), 14, 23), "4 indices (2 segments) and 3 vertices (the points named), not 4");
        let (err, indices, vertices) = build(MAX_SCENE_INDICES - 3, 0);
        assert!(matches!(&err, Err(MeshError::Unsupported(m)) if m.contains("indices")), "{err:?}");
        assert_eq!((indices, vertices), (MAX_SCENE_INDICES - 3, 0), "a refusal charges nothing");
        let (err, indices, vertices) = build(0, MAX_SCENE_VERTICES - 2);
        assert!(matches!(&err, Err(MeshError::Unsupported(m)) if m.contains("vertices")), "{err:?}");
        assert_eq!((indices, vertices), (0, MAX_SCENE_VERTICES - 2), "a refusal charges nothing");
    }

    /// The Line reader under truncation and bit flips: a lean FBX of two
    /// strips and a material, read as the binary twin and as the text, each
    /// cut at every length and flipped bit by bit across the Line records
    /// ([`layer_sweep`]). Every reading returns.
    #[test]
    fn fbx_line_inputs_survive_truncation_and_bit_flips() {
        let points = [0., 0., 0., 1., 0., 0., 1., 1., 0., 3., 0., 0., 4., 0., 0., 4., 1., 0., 3., 1., 0., 9., 9., 9.];
        let material = "\tMaterial: 20, \"Material::mat\", \"\" {\n\t\tShadingModel: \"lambert\"\n\t\tProperties70:  {\n\t\t}\n\t}\n";
        let objects = [line_geometry("Line", &points, &[0, 1, -3, 3, 4, 5, -7]), material.to_string()].concat();
        let text = line_doc("", &objects, &format!("{LINE_CONNECTIONS}\tC: \"OO\",20,10\n"));
        let read = |bytes: &[u8]| super::super::read_scene(bytes, super::super::MeshFormat::Fbx, &NoSidecars);
        let binary = binary_twin(&text);
        for (name, file, from) in [("a Line geometry as binary FBX", binary, "Points"), ("a Line geometry as ASCII FBX", text.clone().into_bytes(), "Points:")] {
            let whole = read(&file).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(whole.scene.drawable_count(), 5, "{name} reads whole");
            layer_sweep(name, &file, span(name, &file, from, "Connections"), |bytes| {
                let _ = read(bytes);
            });
        }
    }
}
