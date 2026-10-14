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
//! - **Winding.** `orientation = "leftHanded"` reverses each face.
//! - **Skinning.** UsdSkel renders a skinned point as `skelWorld · J · bind⁻¹ ·
//!   geomBind · p`; with joints as nodes under the skeleton's node, that is the
//!   scene's `world(joint) · ibm · p` with `ibm = bind⁻¹ · geomBind`.
//! - **Time.** Time codes become seconds through `timeCodesPerSecond`.
//!
//! # What is not carried, and said so
//!
//! Cameras, lights, curves, points, non-mesh gprims (`Cube`, `Sphere`, ...),
//! point instancers, guide and proxy purposes, invisible prims, vertex
//! animation (time-sampled `points`), blend-shape inbetweens, and shading
//! networks other than `UsdPreviewSurface` (MaterialX, MDL): each is a warning
//! naming the prim. A texture channel layout glTF cannot express (roughness and
//! metallic in two different images, opacity from a different image than the
//! base colour) keeps the factor and drops the texture, with a warning, because
//! splitting or packing images would mean re-encoding them.

use std::collections::HashMap;

use super::math::{self, Mat4};
use super::scene::{
    AlphaMode, AnimPath, Animation, Channel, Image, ImageFormat, Interpolation, Material, MorphTarget, Node,
    Primitive, Sampler, Scene, SceneMesh, Skin, SkinWeights, Texture, TextureRef, Transform, UvTransform, Wrap,
    MAX_SCENE_INDICES, MAX_SCENE_VERTICES,
};
use super::usd_stage::{short, split_property, Attr, Stage, Value};
use super::MeshError;

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
/// path.
pub fn read_stage(stage: &Stage, fetch: &mut FetchBytes<'_>) -> Result<(Scene, Vec<String>), MeshError> {
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
        node_of: HashMap::new(),
        materials: HashMap::new(),
        images: HashMap::new(),
        textures: HashMap::new(),
        skeletons: Vec::new(),
        skinned: Vec::new(),
        morphs: Vec::new(),
        channels: Vec::new(),
        anim_names: Vec::new(),
        total_vertices: 0,
        total_indices: 0,
    };
    r.walk()?;
    r.bind_skeletons()?;
    r.finish_animation()?;
    r.up_axis_and_units()?;
    let Reader { mut scene, warnings, .. } = r;
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
    node_of: HashMap<usize, usize>,
    /// (material prim, doubleSided) → IR material and what it reads.
    materials: HashMap<(usize, bool), (usize, MaterialReads)>,
    images: HashMap<String, Option<usize>>,
    /// (image, wrapS, wrapT) → texture. `Wrap` is not `Hash`; its code is.
    textures: HashMap<(usize, u8, u8), usize>,
    skeletons: Vec<SkeletonInfo>,
    skinned: Vec<SkinnedMesh>,
    morphs: Vec<MorphMesh>,
    channels: Vec<Channel>,
    anim_names: Vec<String>,
    total_vertices: usize,
    total_indices: usize,
}

/// What a material reads from the mesh beyond positions and normals.
#[derive(Debug, Clone, Default)]
struct MaterialReads {
    /// Texture-coordinate primvar names, in uv-set order.
    uv_names: Vec<String>,
    /// A float3 primvar feeding the diffuse colour (vertex colours).
    color: Option<String>,
    /// A float primvar feeding opacity.
    opacity: Option<String>,
}

fn warn_push(w: &mut Vec<String>, msg: String) {
    if w.len() < 500 && !w.contains(&msg) {
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
        // Which prims hold something drawn (prims are stored parents first, so
        // one reverse pass sees every child before its parent). A Scope or
        // untyped prim with nothing drawn under it — the usual "Materials"
        // scope — becomes no node, rather than an empty one.
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
                    | "Camera" | "DistantLight" | "SphereLight" | "RectLight" | "DiskLight" | "CylinderLight"
                    | "DomeLight" | "DomeLight_1" | "GeometryLight" | "PortalLight" | "PluginLight" | "RenderSettings"
                    | "RenderProduct" | "RenderVar" | "RenderPass" | "PhysicsScene"
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
                "Camera" | "DistantLight" | "SphereLight" | "RectLight" | "DiskLight" | "CylinderLight" | "DomeLight"
                | "DomeLight_1" | "GeometryLight" | "PortalLight" | "PluginLight" => {
                    self.warn(format!("{ty} {} is not carried (the scene holds no cameras or lights)", short(self.path(p))));
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
            let node = self.make_node(p, parent)?;
            match ty {
                "Mesh" => self.read_mesh(p, node)?,
                "Skeleton" => self.read_skeleton(p, Some(node))?,
                "Points" | "BasisCurves" | "NurbsCurves" | "HermiteCurves" | "NurbsPatch" | "Cube" | "Sphere"
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
        let dense = densify(&times, self.tcps);
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

    // ── meshes ──────────────────────────────────────────────────────────────

    fn primvar(&mut self, p: usize, attr_name: &str, width: usize, default_interp: Interp, counts: (usize, usize, usize)) -> Option<Primvar> {
        let st = self.st;
        let a = st.attr(p, attr_name)?;
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
        let indices = match st.attr(p, &format!("{attr_name}:indices")).and_then(|a| a.rest().cloned()) {
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
        let double_sided = st.attr(p, "doubleSided").and_then(|a| a.rest().and_then(Value::as_bool)).unwrap_or(false);

        // Normals: primvars:normals wins over normals.
        let normals = if st.attr(p, "primvars:normals").is_some() {
            self.primvar(p, "primvars:normals", 3, Interp::Constant, topo)
        } else {
            self.primvar(p, "normals", 3, Interp::Vertex, topo)
        };
        let has_skin = st.attr(p, "primvars:skel:jointIndices").is_some() && st.attr(p, "primvars:skel:jointWeights").is_some();

        let mut mesh = SceneMesh { name: Some(self.display_name(p)), ..Default::default() };
        let mut point_maps: Vec<Vec<u32>> = Vec::new();
        let mut warned_degenerate = false;
        for (binding, faces) in groups {
            let (material, reads) = match binding {
                Some(m) => {
                    let (mi, reads) = self.material(m, double_sided)?;
                    (Some(mi), reads)
                }
                None => {
                    // An unbound gprim draws with its displayColor, or Hydra's
                    // fallback grey when it has none.
                    let has_color = st.attr(p, "primvars:displayColor").is_some();
                    let mi = self.fallback_material(double_sided, has_color);
                    let reads = if has_color {
                        MaterialReads { color: Some("displayColor".into()), opacity: Some("displayOpacity".into()), ..Default::default() }
                    } else {
                        MaterialReads::default()
                    };
                    (Some(mi), reads)
                }
            };
            // Texture coordinates: the sets the material reads, else `st`.
            let mut uv_names = reads.uv_names.clone();
            if uv_names.is_empty() {
                uv_names = self.default_uv_names(p);
            }
            uv_names.truncate(MAX_UV_SETS);
            let mut uv_pvs: Vec<Option<Primvar>> = Vec::new();
            for n in &uv_names {
                let pv = self.primvar(p, &format!("primvars:{n}"), 2, Interp::Constant, topo);
                if pv.is_none() && !reads.uv_names.is_empty() {
                    self.warn(format!("mesh {} has no texture coordinates {n:?} for its material; (0, 0) is used", short(&path)));
                }
                uv_pvs.push(pv);
            }
            while uv_pvs.last().is_some_and(|u| u.is_none()) && reads.uv_names.is_empty() {
                uv_pvs.pop();
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
            let single_group = faces.len() == n_faces;

            // Triangles (fan), with the corner (face, point, face-vertex) of each.
            let mut corners: Vec<(usize, usize, usize)> = Vec::new();
            let mut tri_count = 0usize;
            for &f in &faces {
                if hole[f] {
                    continue;
                }
                let n = counts[f] as usize;
                if n < 3 {
                    if !warned_degenerate {
                        warned_degenerate = true;
                        self.warn(format!("mesh {} has faces with fewer than 3 vertices; they were skipped", short(&path)));
                    }
                    continue;
                }
                tri_count = tri_count.saturating_add(n - 2);
                if self.total_indices.saturating_add(tri_count.saturating_mul(3)) > MAX_SCENE_INDICES {
                    return Err(unsupported(format!("the stage has more than {MAX_SCENE_INDICES} triangle indices")));
                }
                let s = face_start[f];
                let corner = |k: usize| -> (usize, usize, usize) {
                    let k = if left { (n - k) % n } else { k };
                    (f, fv_indices[s + k] as usize, s + k)
                };
                for k in 1..n - 1 {
                    corners.push(corner(0));
                    corners.push(corner(k));
                    corners.push(corner(k + 1));
                }
            }
            if corners.is_empty() {
                continue;
            }

            // IR vertices: the points themselves when every attribute is per
            // point (exact round trip of our own files), else one per distinct
            // combination of the attribute values each corner uses.
            let mut prim = Primitive { material, ..Default::default() };
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
            let ji: Vec<i64> = st.attr(p, "primvars:skel:jointIndices").and_then(|a| a.rest().and_then(Value::ints).map(|v| v.to_vec())).unwrap_or_default();
            let wa = st.attr(p, "primvars:skel:jointWeights");
            let jw: Vec<f32> = wa.as_ref().and_then(|a| a.rest().and_then(Value::floats).map(|v| v.into_owned())).unwrap_or_default();
            let ja = st.attr(p, "primvars:skel:jointIndices");
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

    fn default_uv_names(&self, p: usize) -> Vec<String> {
        let st = self.st;
        let mut out = Vec::new();
        if st.attr(p, "primvars:st").is_some() {
            out.push("st".to_string());
        } else {
            // The first texture-coordinate primvar, by role.
            for n in st.prop_names(p) {
                let Some(name) = n.strip_prefix("primvars:") else { continue };
                if name.contains(':') {
                    continue;
                }
                if st.attr(p, &n).is_some_and(|a| a.type_name.starts_with("texCoord2")) {
                    out.push(name.to_string());
                    break;
                }
            }
        }
        if !out.is_empty() && st.attr(p, "primvars:st1").is_some() && out[0] != "st1" {
            out.push("st1".to_string());
        }
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
        // Hydra draws an unbound gprim with its displayColor (or grey), lit
        // as a plain diffuse surface — UsdPreviewSurface's defaults.
        let grey = 0.18;
        let m = Material {
            name: Some("displayColor".into()),
            base_color: if from_primvar { [1.0; 4] } else { [grey, grey, grey, 1.0] },
            metallic: 0.0,
            roughness: 0.5,
            double_sided,
            ..Default::default()
        };
        if let Some(i) = self.scene.materials.iter().position(|x| *x == m) {
            return i;
        }
        self.scene.materials.push(m);
        self.scene.materials.len() - 1
    }

    fn material(&mut self, m: usize, double_sided: bool) -> Result<(usize, MaterialReads), MeshError> {
        if let Some((i, reads)) = self.materials.get(&(m, double_sided)) {
            return Ok((*i, reads.clone()));
        }
        let (mut mat, reads) = self.read_material(m)?;
        mat.double_sided = double_sided;
        let i = self.scene.materials.len();
        self.scene.materials.push(mat);
        self.materials.insert((m, double_sided), (i, reads.clone()));
        Ok((i, reads))
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
        // Diffuse colour and opacity.
        let mut opacity_connected = false;
        match self.resolve(s, "inputs:diffuseColor") {
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
                    reads.color = self.token(t, "inputs:varname");
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
                            && self.texture_file(t) == self.texture_file_of_base(s);
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
                        reads.opacity = self.token(t, "inputs:varname");
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
                // Separate or differently packed images cannot become glTF's
                // one packed texture without re-encoding them.
                for (what, r) in [("metallic", &metal), ("roughness", &rough)] {
                    if let Resolved::Output(t, ch) = r {
                        let scale = self.float4(*t, "inputs:scale", [1.0; 4]);
                        let idx = match ch.as_str() { "r" => 0, "g" => 1, "b" => 2, "a" => 3, _ => 0 };
                        let v = if what == "metallic" { &mut mat.metallic } else { &mut mat.roughness };
                        // A NaN stays NaN, which validation refuses.
                        *v = scale[idx].clamp(0.0, 1.0);
                        self.warn(format!(
                            "material {} reads {what} from channel {ch:?} of its own texture; glTF packs metallic in B and roughness in G of one image, so the texture is not carried",
                            short(&mpath)
                        ));
                    }
                }
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
        // Emission.
        match self.resolve(s, "inputs:emissiveColor") {
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
        if let Resolved::Value(v, _) = self.resolve(s, "inputs:clearcoat") {
            if v.as_f64().is_some_and(|c| c > 0.0) {
                self.warn(format!("material {} has clearcoat, which is not carried", short(&mpath)));
            }
        }
        reads.uv_names = uv_names;
        Ok((mat, reads))
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

    fn texture_file_of_base(&self, surface: usize) -> Option<String> {
        match self.resolve(surface, "inputs:diffuseColor") {
            Resolved::Output(t, _) => self.texture_file(t),
            _ => None,
        }
    }

    /// A UsdUVTexture as a texture reference: its image, wrap modes, uv set
    /// and transform, plus its scale (the glTF factor).
    fn texture(&mut self, t: usize, uv_names: &mut Vec<String>, color: bool) -> Result<Option<TexRead>, MeshError> {
        let tpath = self.path(t).to_string();
        let Some(file) = self.texture_file(t) else {
            self.warn(format!("texture {} names no file inside the package; dropped", short(&tpath)));
            return Ok(None);
        };
        let space = self.token(t, "inputs:sourceColorSpace").unwrap_or_else(|| "auto".into());
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
        let ws = self.token(t, "inputs:wrapS");
        let wt = self.token(t, "inputs:wrapT");
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
                        varname = self.token(pr, "inputs:varname");
                    }
                }
                Some(id) if id.starts_with("UsdPrimvarReader") => varname = self.token(r, "inputs:varname"),
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
        let parent_of: Vec<Option<usize>> = joints
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
            .collect();
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
            self.animate_joints(a, &joints, &joint_nodes)?;
            let name = self.display_name(a);
            if !self.anim_names.contains(&name) {
                self.anim_names.push(name);
            }
        }
        self.skeletons.push(SkeletonInfo { prim: p, joint_nodes, joint_paths: joints, bind, anim, rooted });
        Ok(())
    }

    fn animate_joints(&mut self, a: usize, joints: &[String], joint_nodes: &[usize]) -> Result<(), MeshError> {
        let st = self.st;
        let apath = self.path(a).to_string();
        let anim_joints: Vec<String> = st.attr(a, "joints").and_then(|x| x.rest().and_then(Value::strs).map(|v| v.to_vec())).unwrap_or_default();
        let map: Vec<Option<usize>> = anim_joints.iter().map(|j| joints.iter().position(|s| s == j)).collect();
        if map.iter().any(Option::is_none) {
            self.warn(format!("animation {} names joints its skeleton lacks; they are ignored", short(&apath)));
        }
        let n = anim_joints.len();
        for (attr, path, width) in [("translations", AnimPath::Translation, 3), ("rotations", AnimPath::Rotation, 4), ("scales", AnimPath::Scale, 3)] {
            let Some(at) = st.attr(a, attr) else { continue };
            let expect_type = match attr {
                "translations" => "float3[]",
                "rotations" => "quatf[]",
                _ => "half3[]",
            };
            if !at.type_name.is_empty() && at.type_name != expect_type {
                // UsdSkel reads these attributes only with their schema types:
                // a float3[] scales is ignored by every USD runtime.
                self.warn(format!("{attr} of {} is {}, not {expect_type}; USD ignores it, and so does this reader", short(&apath), at.type_name));
                continue;
            }
            let shaped = |v: &Value| -> Option<Vec<f32>> {
                let f = v.floats()?.into_owned();
                (f.len() == n * width).then_some(f)
            };
            // Rest values: the animation's own, which override restTransforms
            // for the joints it lists.
            if let Some(v) = at.rest().and_then(shaped) {
                for (k, slot) in map.iter().enumerate() {
                    let Some(j) = slot else { continue };
                    let node = joint_nodes[*j];
                    let (mut t, mut r, mut s) = self.scene.nodes[node].transform.trs().unwrap_or(([0.0; 3], math::QUAT_IDENTITY, [1.0; 3]));
                    let x = &v[k * width..(k + 1) * width];
                    match path {
                        AnimPath::Translation => t = [x[0] as f64, x[1] as f64, x[2] as f64],
                        AnimPath::Rotation => r = math::quat_normalize([x[0] as f64, x[1] as f64, x[2] as f64, x[3] as f64]),
                        _ => s = [x[0] as f64, x[1] as f64, x[2] as f64],
                    }
                    self.scene.nodes[node].transform = Transform::Trs { translation: t, rotation: r, scale: s };
                }
            } else if at.rest().is_some() {
                self.warn(format!("{attr} of {} does not hold one value per joint; ignored", short(&apath)));
                continue;
            }
            if !at.animated() {
                continue;
            }
            let mut per_joint: Vec<(Vec<f32>, Vec<f32>)> = vec![(Vec::new(), Vec::new()); n];
            for (t, v) in &at.samples {
                let Some(vals) = shaped(v) else {
                    self.warn(format!("a sample of {attr} on {} does not hold one value per joint; skipped", short(&apath)));
                    continue;
                };
                let secs = self.seconds(*t);
                for (k, slot) in per_joint.iter_mut().enumerate() {
                    slot.0.push(secs);
                    slot.1.extend_from_slice(&vals[k * width..(k + 1) * width]);
                }
            }
            for (k, (times, values)) in per_joint.into_iter().enumerate() {
                let Some(j) = map[k] else { continue };
                let node = joint_nodes[j];
                // A joint that holds still at its rest value is not animated.
                let rest_vals: Vec<f32> = match (self.scene.nodes[node].transform.trs(), path) {
                    (Some((t, _, _)), AnimPath::Translation) => t.iter().map(|v| *v as f32).collect(),
                    (Some((_, r, _)), AnimPath::Rotation) => r.iter().map(|v| *v as f32).collect(),
                    (Some((_, _, s)), _) => s.iter().map(|v| *v as f32).collect(),
                    _ => Vec::new(),
                };
                let still = values.chunks_exact(width).all(|c| {
                    c.iter().zip(&rest_vals).all(|(a, b)| (a - b).abs() <= 1e-6 * b.abs().max(1.0))
                        || (path == AnimPath::Rotation && rest_vals.len() == 4 && c.iter().zip(&rest_vals).all(|(a, b)| (a + b).abs() <= 1e-6))
                });
                if still {
                    continue;
                }
                let mut values = values;
                if path == AnimPath::Rotation {
                    for q in values.chunks_exact_mut(4) {
                        let n = math::quat_normalize([q[0] as f64, q[1] as f64, q[2] as f64, q[3] as f64]);
                        for (o, v) in q.iter_mut().zip(n) {
                            *o = v as f32;
                        }
                    }
                }
                self.push_channel(Channel { node, path, interpolation: Interpolation::Linear, times, values });
            }
        }
        Ok(())
    }

    /// The skeleton a prim is bound to: `skel:skeleton` on it or its nearest
    /// ancestor that has one.
    fn bound_skeleton(&self, p: usize) -> Option<usize> {
        let st = self.st;
        let mut cur = Some(p);
        while let Some(c) = cur {
            if let Some(t) = st.rel(c, "skel:skeleton").first() {
                let k = st.find(t)?;
                return self.skeletons.iter().position(|s| s.prim == k);
            }
            cur = st.prims[c].parent;
        }
        None
    }

    fn bind_skeletons(&mut self) -> Result<(), MeshError> {
        let st = self.st;
        // Skins, keyed by skeleton and geomBindTransform.
        let mut skins: HashMap<(usize, [u64; 16]), usize> = HashMap::new();
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
            let local: Option<Vec<String>> = st.attr(sm.prim, "skel:joints").and_then(|a| a.rest().and_then(Value::strs).map(|v| v.to_vec()));
            let remap: Vec<usize> = match &local {
                Some(l) => {
                    let mut out = Vec::with_capacity(l.len());
                    for j in l {
                        match self.skeletons[k].joint_paths.iter().position(|s| s == j) {
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
            let g: Mat4 = match st.attr(sm.prim, "primvars:skel:geomBindTransform").and_then(|a| a.rest().and_then(Value::as_tuple).map(|t| t.to_vec())) {
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
                    let info = &self.skeletons[k];
                    let mut ibms = Vec::with_capacity(info.bind.len());
                    for b in &info.bind {
                        let inv = math::invert(b).ok_or_else(|| malformed(format!("skeleton {} has a singular bind transform", short(self.path(info.prim)))))?;
                        ibms.push(math::mat_mul(&inv, &g));
                    }
                    let roots: Vec<usize> = info
                        .joint_nodes
                        .iter()
                        .copied()
                        .filter(|j| !info.joint_nodes.iter().any(|o| self.scene.nodes[*o].children.contains(j)))
                        .collect();
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
        let morphs = std::mem::take(&mut self.morphs);
        for mm in morphs {
            let Some(k) = self.bound_skeleton(mm.prim) else { continue };
            if !self.skeletons[k].rooted {
                continue;
            }
            let Some(a) = self.skeletons[k].anim else { continue };
            let names: Vec<String> = st.attr(a, "blendShapes").and_then(|x| x.rest().and_then(Value::strs).map(|v| v.to_vec())).unwrap_or_default();
            let Some(wattr) = st.attr(a, "blendShapeWeights") else { continue };
            let map: Vec<Option<usize>> = mm.names.iter().map(|n| names.iter().position(|o| o == n)).collect();
            let pick = |v: &Value| -> Option<Vec<f32>> {
                let f = v.floats()?.into_owned();
                (f.len() == names.len()).then(|| map.iter().map(|m| m.map_or(0.0, |i| f[i])).collect())
            };
            if let Some(w) = wattr.rest().and_then(pick) {
                if w.iter().any(|x| *x != 0.0) {
                    self.scene.meshes[mm.mesh].weights = w;
                }
            }
            if wattr.animated() {
                let mut times = Vec::new();
                let mut values = Vec::new();
                for (t, v) in &wattr.samples {
                    if let Some(w) = pick(v) {
                        times.push(self.seconds(*t));
                        values.extend(w);
                    }
                }
                self.push_channel(Channel { node: mm.node, path: AnimPath::Weights, interpolation: Interpolation::Linear, times, values });
            }
        }
        Ok(())
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
        let lo = channels.iter().filter_map(|c| c.times.first().copied()).fold(f32::INFINITY, f32::min);
        if lo < 0.0 {
            self.warn(format!("the animation starts at {lo}s; it was shifted to start at 0"));
            for c in channels.iter_mut() {
                for t in c.times.iter_mut() {
                    *t -= lo;
                }
            }
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
        let mpu = match st.meta("metersPerUnit").and_then(Value::as_f64) {
            Some(m) if m.is_finite() && m > 0.0 => m,
            Some(_) => {
                self.warn("metersPerUnit is not a positive number; metres are assumed".into());
                1.0
            }
            None => {
                self.warn("the stage does not declare metersPerUnit; USD's fallback of centimetres was applied".into());
                0.01
            }
        };
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

struct TexRead {
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
        let Some(v) = a.rest() else { continue };
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
        None => a.rest().cloned(),
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
fn densify(times: &[f64], tcps: f64) -> Vec<f64> {
    let mut out = Vec::new();
    let step = tcps / RESAMPLE_FPS;
    for w in times.windows(2) {
        out.push(w[0]);
        let n = ((w[1] - w[0]) / step).ceil().clamp(1.0, 10_000.0) as usize;
        for i in 1..n {
            out.push(w[0] + (w[1] - w[0]) * i as f64 / n as f64);
        }
    }
    if let Some(l) = times.last() {
        out.push(*l);
    }
    out
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
    use super::*;

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
}
