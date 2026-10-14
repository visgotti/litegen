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
//! materials, Phong/Lambert materials (plus Stingray, 3ds Max Physical and 3ds
//! Max PBR properties when present), embedded or sidecar textures, skin clusters,
//! blend shapes (an in-between shape becomes a target of its own, weighted
//! piecewise-linearly from its channel's weight), and the active animation
//! stack's transform and blend-shape curves. Warned about and left out:
//! cameras and lights (as empty nodes), NURBS and line geometry, other
//! animation stacks and layers, animated properties the scene does not
//! model, texture slots with no metallic-roughness equivalent. Refused:
//! indices out of range, arrays that disagree with their mapping, a cyclic
//! hierarchy — anything that would otherwise become a plausible but wrong
//! model.
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

use super::fbx::{malformed, srgb_decode, unsupported, TICKS_PER_SECOND, TICKS_PER_SECOND_2019};
use super::fbx_dom::{split_object_name, ArrayRef, Dom, Prop, RecId, ROOT};
use super::math::{self, EulerOrder, Mat4};
use super::package::{self, Sidecars};
use super::scene::{
    AlphaMode, AnimPath, Animation, Channel, Image, ImageFormat, Interpolation, Material, MorphTarget, Node,
    Primitive, Sampler, Scene, SceneMesh, Skin, SkinWeights, Texture, TextureRef, Transform, UvTransform, Wrap,
    MAX_ANIMATION_KEYS, MAX_JOINTS, MAX_MORPH_TARGETS, MAX_NODES, MAX_SCENE_INDICES, MAX_SCENE_VERTICES,
};
use super::{MeshError, SceneRead};
use std::collections::{BTreeMap, HashMap, HashSet};

/// UV sets read per geometry; the scene's own ceiling (see `glb.rs`).
const MAX_UV_SETS: usize = 8;
/// Influences kept per vertex. Vendors ship 8–10 (Meshy); 32 is past anything
/// a skinning shader evaluates, and glTF's reader stops there too.
const MAX_INFLUENCES: usize = 32;
/// Morph-target floats the scene may hold (targets × vertices × 3), the
/// decoded-value ceiling `glb.rs` applies.
const MAX_MORPH_VALUES: usize = 48_000_000;
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
    let p_r_inv = math::transpose(&p_r);
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
    if let Some(trs) = math::decompose(m) {
        return trs;
    }
    let col = |c: usize| [m[c * 4], m[c * 4 + 1], m[c * 4 + 2]];
    let len = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let norm = |v: [f64; 3]| {
        let l = len(v);
        if l > 1e-300 {
            [v[0] / l, v[1] / l, v[2] / l]
        } else {
            [0.0; 3]
        }
    };
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (c0, c1) = (col(0), col(1));
    let x = norm(c0);
    let y = norm([c1[0] - dot(c1, x) * x[0], c1[1] - dot(c1, x) * x[1], c1[2] - dot(c1, x) * x[2]]);
    // The right-handed frame; a mirror goes into the third scale.
    let z = [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    let mut s = [len(c0), len(c1), len(col(2))];
    if math::is_mirroring(m) {
        s[2] = -s[2];
    }
    let r = [x[0], x[1], x[2], y[0], y[1], y[2], z[0], z[1], z[2]];
    ([m[12], m[13], m[14]], math::mat3_to_quat(&r), s)
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
    warnings: Vec<String>,
    warned: HashSet<String>,
    objects: HashMap<i64, Obj>,
    order: Vec<i64>,
    children: HashMap<i64, Vec<Edge>>,
    parents: HashMap<i64, Vec<Edge>>,
    templates: HashMap<String, PMap<'a>>,
    creator: String,
    tick_rate: f64,
    correction: Mat4,
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
pub(super) fn read(dom: &Dom<'_>, sidecars: &dyn Sidecars) -> Result<SceneRead, MeshError> {
    let mut r = Reader::new(dom, sidecars);
    r.header()?;
    r.index()?;
    r.globals()?;
    r.models()?;
    r.geometry()?;
    r.animation()?;
    r.finish()
}

impl<'d, 'a> Reader<'d, 'a> {
    fn new(dom: &'d Dom<'a>, sidecars: &'d dyn Sidecars) -> Self {
        Reader {
            dom,
            sidecars,
            warnings: Vec::new(),
            warned: HashSet::new(),
            objects: HashMap::new(),
            order: Vec::new(),
            children: HashMap::new(),
            parents: HashMap::new(),
            templates: HashMap::new(),
            creator: String::new(),
            tick_rate: TICKS_PER_SECOND,
            correction: math::IDENTITY,
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
        let m = axis_unit_matrix(&self.props(&gs, "GlobalSettings"))?;
        self.correction = m;
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
        let mut cameras = 0usize;
        let mut lights = 0usize;
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
                let p_s = math::mat_mul(&math::transpose(&p_r), &p_rs);
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
            match self.sub(m) {
                "Camera" | "CameraSwitcher" | "CameraStereo" => cameras += 1,
                "Light" => lights += 1,
                _ => {}
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
        if cameras + lights > 0 {
            self.warn("camlight", format!("{cameras} camera(s) and {lights} light(s) were read as empty nodes; the scene has neither"));
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
        // ignored and Lcl Rotation is XYZ.
        let active = props.i64("RotationActive").unwrap_or(0) != 0 && props.i64("RotationSpaceForLimitOnly").unwrap_or(0) == 0;
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
            let mut meshes: Vec<i64> = Vec::new();
            for g in geoms {
                let sub = self.sub(g).to_string();
                match sub.as_str() {
                    "Mesh" => meshes.push(g),
                    "Shape" => {}
                    other => {
                        self.non_mesh_geometry += 1;
                        let what = format!("{other:?} geometry (NURBS, curves and lines)");
                        self.warn("nonmesh", format!("{what} is not carried: the scene models polygon meshes only"));
                    }
                }
            }
            let Some(&g) = meshes.first() else { continue };
            if meshes.len() > 1 {
                self.warn("multigeo", format!("model {:?} has {} geometries; only the first was read", self.name_of(m), meshes.len()));
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
        let values = dom.f64s(&arr, what)?;
        if values.len() % stride != 0 {
            return Err(malformed(format!("{what} has {} values, not a multiple of {stride}", values.len())));
        }
        if values.iter().any(|v| !v.is_finite()) {
            return Err(malformed(format!("{what} has a non-finite value")));
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
        let built = self.build_mesh(g, materials, geo)?;
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

    fn build_mesh(&mut self, g: i64, materials: &[i64], geo: &Mat4) -> Result<Option<(SceneMesh, Vec<BlendChannel>)>, MeshError> {
        let dom = self.dom;
        let gname = self.name_of(g);
        let what = format!("geometry {gname:?}");
        let Some(rec) = self.rec(g) else { return Ok(None) };
        let Some(va) = dom.child_array(rec, "Vertices")? else { return Ok(None) };
        let Some(pa) = dom.child_array(rec, "PolygonVertexIndex")? else { return Ok(None) };
        let verts = dom.f64s(&va, &format!("{what} Vertices"))?;
        if verts.len() % 3 != 0 {
            return Err(malformed(format!("{what} has {} vertex coordinates, not a multiple of 3", verts.len())));
        }
        if verts.iter().any(|v| !v.is_finite()) {
            return Err(malformed(format!("{what} has a non-finite vertex")));
        }
        let cps = verts.len() / 3;
        let pvi = dom.i32s(&pa, &format!("{what} PolygonVertexIndex"))?;
        // Polygons: the last corner of each is stored as -(index + 1).
        let mut polys: Vec<(usize, usize)> = Vec::new();
        let mut corner_cp: Vec<u32> = Vec::with_capacity(pvi.len());
        let mut start = 0usize;
        let mut triangles = 0usize;
        let mut degenerate = 0usize;
        for (i, &raw) in pvi.iter().enumerate() {
            let (cp, end) = if raw < 0 { (!raw, true) } else { (raw, false) };
            if cp as usize >= cps {
                return Err(malformed(format!("{what} polygon corner {i} names control point {cp} of {cps}")));
            }
            corner_cp.push(cp as u32);
            if end {
                let n = i + 1 - start;
                if n >= 3 {
                    triangles += n - 2;
                } else {
                    degenerate += 1;
                }
                polys.push((start, n));
                start = i + 1;
            }
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
            // A fan from the first corner keeps the polygon's winding.
            for i in 1..n - 1 {
                if mirror {
                    b.indices.extend_from_slice(&[vids[0], vids[i + 1], vids[i]]);
                } else {
                    b.indices.extend_from_slice(&[vids[0], vids[i], vids[i + 1]]);
                }
            }
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
        let what = format!("material {:?}", self.name_of(id));
        let props = self.props(&pm, "Material");
        let color = |name: &str| props.vec3(name).filter(|v| finite3(*v));
        let decode = |v: [f64; 3]| [srgb_decode(v[0]), srgb_decode(v[1]), srgb_decode(v[2])];
        let mut m = Material { name: Some(self.name_of(id)).filter(|n| !n.is_empty()), metallic: 0.0, ..Default::default() };
        let mut deferred: Vec<String> = Vec::new();
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
                let emissive = color("Emissive").or_else(|| color("EmissiveColor")).map(decode).unwrap_or([0.0; 3]);
                (diffuse, metallic, 1.0 - shininess.max(0.0).sqrt() / 10.0, scale(emissive, f("EmissiveFactor", 1.0)))
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
                    let tf = props.f64("TransparencyFactor").unwrap_or(0.0);
                    let c = props.vec3("TransparentColor").unwrap_or([1.0; 3]);
                    1.0 - tf * (c[0] + c[1] + c[2]) / 3.0
                }
            },
        };
        if !alpha.is_finite() {
            return Err(malformed(format!("{what} opacity is not finite")));
        }
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
                        self.warn(
                            &format!("slot-{other}"),
                            format!("textures on the material slot {other:?} have no metallic-roughness equivalent and were not carried"),
                        );
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
                for _ in 0..count {
                    if k < n {
                        attr[k] = run;
                        k += 1;
                    }
                }
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
        let Some(stack) = chosen else { return Ok(()) };
        let others: Vec<String> = stacks.iter().copied().filter(|s| *s != stack && has_curves(self, *s)).map(|s| self.name_of(s)).collect();
        if !others.is_empty() {
            self.warn("stacks", format!("{} more animation stack(s) [{}] were not carried; only {:?} was", others.len(), others.join(", "), self.name_of(stack)));
        }
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
        for (node, curves) in &node_curves {
            let model = self.model_of_node[*node];
            let mut complex = self.complex.contains(&model);
            if !complex && self.xf.get(&model).is_some_and(|x| x.inherit != 1) {
                let mut cur = self.model_parent.get(&model).copied();
                while let Some(p) = cur {
                    if self.node_of.get(&p).is_some_and(|n| scale_animated.contains(n)) {
                        complex = true;
                        break;
                    }
                    cur = self.model_parent.get(&p).copied();
                }
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
                for (c, curve) in chans.iter().zip(&curves) {
                    let Some(curve) = curve.filter(|_| c.full.len() > 1) else { continue };
                    let w = |t: f64| curve.eval(t) / 100.0;
                    for pair in times.windows(2) {
                        let (a, b) = (pair[0], pair[1]);
                        let (wa, wb) = (w(a), w(b));
                        for &bp in &c.full {
                            if (wa - bp) * (wb - bp) < 0.0 {
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
            self.scene.animations.push(Animation { name: Some(self.name_of(stack)).filter(|n| !n.is_empty()), channels });
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
        if self.scene.triangle_count() == 0 && self.non_mesh_geometry > 0 {
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
}
