//! Scene → binary FBX 7400.
//!
//! # What the file looks like, and why
//!
//! The layout is the one the SDK itself writes (Mixamo, 3ds Max and Maya
//! output, measured byte by byte in the research notes), because the readers
//! that matter are each strict about a different part of it:
//!
//! - **Assimp** refuses a file without `FBXHeaderExtension`, drops every
//!   animation when `AnimationLayer` lacks its NULL record, finds layer
//!   elements only through `Layer`/`LayerElement`, and ignores materials
//!   unless the geometry has a `LayerElementMaterial`.
//! - **three.js** (the dashboard's viewer) throws on a material without
//!   `ShadingModel`, a texture without `FileName` or a blend-shape channel
//!   without `FullWeights`; binds animation tracks to nodes BY NAME (so names
//!   are made unique in the form three.js compares them); walks rotation
//!   curves by the X curve's key index (so X, Y and Z are keyed at identical
//!   times); ignores interpolation flags (so STEP is emulated with a second
//!   key just before each step); mis-places a skinned mesh without a
//!   `BindPose`; and reads every colour as sRGB.
//! - **The SDK** (Maya, 3ds Max, Unity, Unreal) is believed to reject a file
//!   whose `FileId`, `CreationTime` and footer id disagree; the triple below
//!   is the one Blender and Assimp have shipped in every file for years.
//!
//! Content mapping: one `Model` per node (`LimbNode` for skin joints, `Null`
//! otherwise, `Mesh` when it holds geometry), one `Geometry` per mesh (one per
//! node when skinned or morphed, since FBX hangs deformers off the geometry),
//! primitives as per-polygon materials, Phong materials in Blender's
//! metallic-roughness encoding plus Stingray-style `Maya|…` factors, embedded
//! textures, and animation resampled at 30 fps plus every source key.

use super::fbx::{srgb_encode, unsupported, TICKS_PER_SECOND, WRITE_VERSION};
use super::fbx_dom::{write_records, WProp, WRec, BINARY_MAGIC};
use super::math::{self, EulerOrder, Mat4};
use super::scene::{AnimPath, Interpolation, Primitive, Scene, Transform, Wrap};
use super::{pose, MeshError};
use std::collections::{BTreeMap, HashMap, HashSet};

/// IDs start well above zero (the scene root) and stay far below 2^53, where
/// three.js, which reads them into JavaScript numbers, would start merging
/// neighbours. Sequential, so the same scene always gets the same IDs.
const FIRST_ID: i64 = 1_000_000;
const CREATOR: &str = "litegen mesh converter";
/// The FileId / CreationTime / footer-id triple Blender and Assimp write. The
/// two ids are hashes of the time string (verified against four SDK files);
/// a real date with these hashes would be inconsistent, so the date is fixed
/// too — which also keeps the output deterministic.
const FILE_ID: [u8; 16] = [0x28, 0xB3, 0x2A, 0xEB, 0xB6, 0x24, 0xCC, 0xC2, 0xBF, 0xC8, 0xB0, 0x2A, 0xA9, 0x2B, 0xFC, 0xF1];
const CREATION_TIME: &str = "1970-01-01 10:00:00:000";
const FOOTER_ID: [u8; 16] = [0xFA, 0xBC, 0xAB, 0x09, 0xD0, 0xC8, 0xD4, 0x66, 0xB1, 0x76, 0xFB, 0x83, 0x1C, 0xF7, 0x26, 0x7E];
const FOOTER_MAGIC: [u8; 16] = [0xF8, 0x5A, 0x8C, 0x6A, 0xDE, 0xF5, 0xD9, 0x7E, 0xEC, 0xE9, 0x0C, 0xE3, 0x75, 0x8F, 0x29, 0x0B];

/// Resampling rate for animation: FBX rotation curves interpolate Euler
/// angles componentwise (and three.js slerps between whatever keys it gets),
/// so the source's own interpolation has to be pinned down at a rate where
/// the two cannot drift apart visibly. 30 fps is Mixamo's and the SDK's
/// default `TimeMode`. Near gimbal lock no fixed rate is enough; see
/// [`refine_euler_times`].
const RESAMPLE_FPS: f32 = 30.0;
/// How far, in degrees, a rotation curve interpolated per Euler angle may
/// stray between two keys from the rotation the scene has there before
/// [`refine_euler_times`] keys the midpoint.
const EULER_TOLERANCE_DEG: f64 = 0.05;
/// The shortest segment [`refine_euler_times`] still splits, in seconds. At
/// an exact gimbal lock the Euler angles turn a corner, and no key spacing
/// straightens a corner.
const EULER_MIN_GAP: f32 = 1e-4;
/// Rounds of splitting; each halves every segment that still strays.
const EULER_MAX_ROUNDS: usize = 16;
/// How far before a STEP key the emulation key sits. Small enough that no
/// sensible sample time lands inside the ramp, large enough to survive f32
/// seconds on clips up to a couple of hours.
const STEP_EPSILON: f32 = 0.001;

/// `KeyAttrFlags`: hold the value until the next key.
const KEY_CONSTANT: i32 = 0x0000_0002;
/// `KeyAttrFlags`: interpolate linearly to the next key.
const KEY_LINEAR: i32 = 0x0000_0004;
/// `KeyAttrDataFloat[2]`: the SDK's default tangent weights (two u16 fixed
/// point 1/3 values packed into one float's bits).
const DEFAULT_KEY_WEIGHTS: u32 = 0x0D05_0D05;

/// Pose evaluations (sample times × (nodes + channels)) one animation may
/// cost to resample. A rig keyed every frame for minutes needs millions; the
/// cap is what keeps a scene with millions of keys on thousands of nodes
/// (possible from a hostile source file) from stalling the writer.
const MAX_RESAMPLE_WORK: usize = 200_000_000;

/// Max candidates compared when welding a vertex into a control point. Welding
/// is a courtesy to DCC users (a cube becomes 8 control points, not 24); a
/// pathological scene with millions of vertices at one position must not turn
/// it quadratic.
const MAX_WELD_CANDIDATES: usize = 16;

// ─── record helpers ─────────────────────────────────────────────────────────

fn ws(v: &str) -> WProp {
    WProp::Str(v.as_bytes().to_vec())
}

fn rec(name: &'static str, props: Vec<WProp>) -> WRec {
    WRec::with(name, props)
}

fn leaf_i32(name: &'static str, v: i32) -> WRec {
    rec(name, vec![WProp::I32(v)])
}

fn leaf_str(name: &'static str, v: &str) -> WRec {
    rec(name, vec![ws(v)])
}

/// `"Name\x00\x01Class"`, the binary spelling of an object's name.
fn obj_name(name: &str, class: &str) -> WProp {
    let mut b = name.as_bytes().to_vec();
    b.extend_from_slice(b"\x00\x01");
    b.extend_from_slice(class.as_bytes());
    WProp::Str(b)
}

/// One `P` record of a `Properties70` block.
fn p(name: &str, ty: &str, label: &str, flags: &str, values: Vec<WProp>) -> WRec {
    let mut props = vec![ws(name), ws(ty), ws(label), ws(flags)];
    props.extend(values);
    rec("P", props)
}

fn p_int(name: &str, ty: &str, v: i32) -> WRec {
    let label = if ty == "int" { "Integer" } else { "" };
    p(name, ty, label, "", vec![WProp::I32(v)])
}

fn p_num(name: &str, v: f64) -> WRec {
    p(name, "Number", "", "A", vec![WProp::F64(v)])
}

fn p_double(name: &str, v: f64) -> WRec {
    p(name, "double", "Number", "", vec![WProp::F64(v)])
}

fn p_vec3(name: &str, ty: &str, label: &str, flags: &str, v: [f64; 3]) -> WRec {
    p(name, ty, label, flags, v.iter().map(|x| WProp::F64(*x)).collect())
}

fn p_color(name: &str, v: [f64; 3]) -> WRec {
    p_vec3(name, "Color", "", "A", v)
}

fn p_str(name: &str, ty: &str, label: &str, v: &str) -> WRec {
    p(name, ty, label, "", vec![ws(v)])
}

fn p_ktime(name: &str, ticks: i64) -> WRec {
    p(name, "KTime", "Time", "", vec![WProp::I64(ticks)])
}

/// A `Properties70` block; an empty one still gets its NULL record, as the
/// SDK writes it.
fn props70(ps: Vec<WRec>) -> WRec {
    WRec { name: "Properties70", props: Vec::new(), force_null: ps.is_empty(), children: ps }
}

fn ticks(seconds: f32) -> i64 {
    (seconds as f64 * TICKS_PER_SECOND).round() as i64
}

fn degrees(v: [f64; 3]) -> [f64; 3] {
    [v[0].to_degrees(), v[1].to_degrees(), v[2].to_degrees()]
}

// ─── names ──────────────────────────────────────────────────────────────────

/// three.js's `PropertyBinding.sanitizeNodeName`: whitespace becomes `_`,
/// `[ ] . : /` are removed. Animation tracks are bound by this form, so this
/// is the form that has to be unique.
fn three_js_key(name: &str) -> String {
    name.chars()
        .filter_map(|c| {
            if c.is_whitespace() {
                Some('_')
            } else if "[].:/".contains(c) {
                None
            } else {
                Some(c)
            }
        })
        .collect()
}

/// Hands out object names that are unique under [`three_js_key`] and contain
/// no control characters (a NUL would be read as the name/class separator).
#[derive(Default)]
struct Names {
    used: HashSet<String>,
}

impl Names {
    fn unique(&mut self, raw: Option<&str>, fallback: &str) -> String {
        let clean: String = raw.unwrap_or("").chars().map(|c| if c.is_control() { '_' } else { c }).collect();
        let base = if clean.trim().is_empty() { fallback.to_string() } else { clean };
        let mut name = base.clone();
        let mut n = 1;
        loop {
            let key = three_js_key(&name);
            if !key.is_empty() && !self.used.contains(&key) {
                self.used.insert(key);
                return name;
            }
            name = format!("{base}_{n}");
            n += 1;
        }
    }
}

// ─── geometry ───────────────────────────────────────────────────────────────

/// A geometry as written: its id and how each primitive vertex maps to a
/// control point, which skins and blend shapes need.
struct GeomOut {
    id: i64,
    /// One representative (primitive, vertex) per control point.
    cp_rep: Vec<(usize, usize)>,
    /// Blend shape channel ids, one per morph target.
    channels: Vec<i64>,
}

fn bits3(v: [f32; 3]) -> [u32; 3] {
    [v[0].to_bits(), v[1].to_bits(), v[2].to_bits()]
}

/// A vertex's non-zero influences, normalised, in a canonical order; what two
/// vertices must agree on to share a control point.
fn influences(p: &Primitive, v: usize) -> Vec<(u16, f32)> {
    let Some(sw) = &p.skin else { return Vec::new() };
    let (js, ws) = sw.vertex(v);
    let sum: f64 = ws.iter().map(|w| w.max(0.0) as f64).sum();
    if sum <= 0.0 || !sum.is_finite() {
        // `pose::skin_in_place` binds an all-zero vertex wholly to its first
        // listed joint; the file says so explicitly.
        return vec![(js[0], 1.0)];
    }
    let mut out: Vec<(u16, f32)> = js
        .iter()
        .zip(ws)
        .filter(|(_, w)| **w > 0.0)
        .map(|(j, w)| (*j, (*w as f64 / sum) as f32))
        .collect();
    // Two slots naming the same joint are one influence.
    out.sort_by_key(|(j, _)| *j);
    let mut merged: Vec<(u16, f32)> = Vec::with_capacity(out.len());
    for (j, w) in out {
        match merged.last_mut() {
            Some(last) if last.0 == j => last.1 += w,
            _ => merged.push((j, w)),
        }
    }
    merged
}

struct Writer<'s> {
    s: &'s Scene,
    embed: bool,
    next_id: i64,
    objects: Vec<WRec>,
    conns: Vec<WRec>,
    counts: BTreeMap<&'static str, usize>,
    names: Names,
    /// Per node: its Model id.
    model: Vec<i64>,
    /// Per node: the Model holding its geometry (itself, or a child when the
    /// node is also a joint).
    holder: Vec<i64>,
    /// Per node: the holder's name, for curve nodes.
    holder_name: Vec<String>,
    /// Per node with a mesh: the geometry it uses.
    geom_of_node: Vec<Option<usize>>,
    geoms: Vec<GeomOut>,
    /// Per geometry: the material ids its LayerElementMaterial indexes, in
    /// slot order; every model using it connects them in that order.
    geom_slots: Vec<Vec<i64>>,
    material_ids: Vec<Option<i64>>,
    default_material: Option<i64>,
    texture_ids: HashMap<(usize, u32), i64>,
    video_ids: HashMap<usize, (i64, String)>,
    video_names: HashSet<String>,
    /// BindPose entries: (model id, global matrix), first wins.
    bind_pose: Vec<(i64, Mat4)>,
}

impl<'s> Writer<'s> {
    fn id(&mut self) -> i64 {
        let i = self.next_id;
        self.next_id += 1;
        i
    }

    fn object(&mut self, kind: &'static str, r: WRec) {
        *self.counts.entry(kind).or_insert(0) += 1;
        self.objects.push(r);
    }

    fn oo(&mut self, child: i64, parent: i64) {
        self.conns.push(rec("C", vec![ws("OO"), WProp::I64(child), WProp::I64(parent)]));
    }

    fn op(&mut self, child: i64, parent: i64, prop: &str) {
        self.conns.push(rec("C", vec![ws("OP"), WProp::I64(child), WProp::I64(parent), ws(prop)]));
    }

    // ── models ──────────────────────────────────────────────────────────────

    fn models(&mut self, animated: &[[bool; 3]]) -> Result<(), MeshError> {
        let s = self.s;
        let joints = s.joint_nodes();
        let parents = s.parents();
        let n = s.nodes.len();
        self.model = (0..n).map(|_| self.id()).collect();
        self.holder = vec![0; n];
        self.holder_name = vec![String::new(); n];
        for (i, node) in s.nodes.iter().enumerate() {
            let Some((t, q, sc)) = node.transform.trs() else {
                return Err(unsupported(format!(
                    "node {i} ({:?}) has a sheared transform; FBX transforms are translation, rotation and scale",
                    node.name.as_deref().unwrap_or("")
                )));
            };
            let name = self.names.unique(node.name.as_deref(), &format!("node{i}"));
            let class = if joints[i] {
                "LimbNode"
            } else if node.mesh.is_some() {
                "Mesh"
            } else {
                "Null"
            };
            let id = self.model[i];
            let anim = animated.get(i).copied().unwrap_or([false; 3]);
            let r = model_record(id, &name, class, t, q, sc, anim);
            self.object("Model", r);
            if class != "Mesh" {
                self.node_attribute(id, &name, joints[i]);
            }
            self.holder[i] = id;
            self.holder_name[i] = name.clone();
            if joints[i] && node.mesh.is_some() {
                // A joint cannot also be a Mesh model; its geometry rides on
                // an untransformed child.
                let hid = self.id();
                let hname = self.names.unique(Some(&format!("{name}_mesh")), "mesh");
                let r = model_record(hid, &hname, "Mesh", [0.0; 3], math::QUAT_IDENTITY, [1.0; 3], [false; 3]);
                self.object("Model", r);
                self.oo(hid, id);
                self.holder[i] = hid;
                self.holder_name[i] = hname;
            }
        }
        // Hierarchy: parent links after every model exists.
        for (i, parent) in parents.iter().enumerate() {
            let parent = parent.map_or(0, |p| self.model[p]);
            let child = self.model[i];
            self.oo(child, parent);
        }
        Ok(())
    }

    fn node_attribute(&mut self, model: i64, name: &str, joint: bool) {
        let id = self.id();
        let (class, flags) = if joint { ("LimbNode", "Skeleton") } else { ("Null", "Null") };
        let r = rec("NodeAttribute", vec![WProp::I64(id), obj_name(name, "NodeAttribute"), ws(class)])
            .child(leaf_str("TypeFlags", flags));
        self.object("NodeAttribute", r);
        self.oo(id, model);
    }

    // ── geometry ────────────────────────────────────────────────────────────

    /// Write one geometry for `mesh`, optionally welded with `skinned`'s
    /// influences, and return its index in `self.geoms`.
    fn geometry(&mut self, mi: usize, skinned: bool, weights: &[f32]) -> Result<usize, MeshError> {
        let s = self.s;
        let mesh = &s.meshes[mi];
        let targets = mesh.target_count();
        // Control points: vertices welded where position, influences and
        // every morph offset agree bit for bit — so welding never changes
        // what any deformation produces.
        let mut cp_pos: Vec<[f32; 3]> = Vec::new();
        let mut cp_rep: Vec<(usize, usize)> = Vec::new();
        let mut buckets: HashMap<[u32; 3], Vec<u32>> = HashMap::new();
        let mut cp_of: Vec<Vec<u32>> = Vec::with_capacity(mesh.primitives.len());
        let same = |a: (usize, usize), b: (usize, usize)| -> bool {
            let (pa, pb) = (&mesh.primitives[a.0], &mesh.primitives[b.0]);
            if skinned && influences(pa, a.1) != influences(pb, b.1) {
                return false;
            }
            (0..targets).all(|t| {
                let (ta, tb) = (&pa.targets[t], &pb.targets[t]);
                bits3(ta.positions[a.1]) == bits3(tb.positions[b.1])
                    && match (&ta.normals, &tb.normals) {
                        (Some(x), Some(y)) => bits3(x[a.1]) == bits3(y[b.1]),
                        (None, None) => true,
                        _ => false,
                    }
            })
        };
        for (pi, prim) in mesh.primitives.iter().enumerate() {
            let mut map = Vec::with_capacity(prim.positions.len());
            for (v, pos) in prim.positions.iter().enumerate() {
                let bucket = buckets.entry(bits3(*pos)).or_default();
                let found = bucket.iter().take(MAX_WELD_CANDIDATES).find(|cp| same(cp_rep[**cp as usize], (pi, v))).copied();
                let cp = match found {
                    Some(cp) => cp,
                    None => {
                        let cp = u32::try_from(cp_pos.len()).map_err(|_| unsupported("more than 2^32 control points"))?;
                        cp_pos.push(*pos);
                        cp_rep.push((pi, v));
                        bucket.push(cp);
                        cp
                    }
                };
                map.push(cp);
            }
            cp_of.push(map);
        }
        if cp_pos.len() > i32::MAX as usize {
            return Err(unsupported("more control points than FBX's 32-bit polygon indices address"));
        }

        // Material slots: the materials the primitives use, in first-use
        // order; a primitive without one gets glTF's default material (white,
        // fully metallic and rough) rather than three.js's grey.
        let mut slots: Vec<Option<usize>> = Vec::new();
        let mut slot_of: Vec<i32> = Vec::with_capacity(mesh.primitives.len());
        for prim in &mesh.primitives {
            let k = match slots.iter().position(|m| *m == prim.material) {
                Some(k) => k,
                None => {
                    slots.push(prim.material);
                    slots.len() - 1
                }
            };
            slot_of.push(k as i32);
        }

        let any_normals = mesh.primitives.iter().any(|p| p.normals.is_some());
        let all_tangents = mesh.primitives.iter().all(|p| p.tangents.is_some()) && any_normals;
        let uv_sets = mesh.primitives.iter().map(|p| p.uvs.len()).max().unwrap_or(0);
        let any_colors = mesh.primitives.iter().any(|p| p.colors.is_some());

        let mut pvi: Vec<i32> = Vec::new();
        let mut normals: Vec<f64> = Vec::new();
        let mut tangents: Vec<f64> = Vec::new();
        let mut binormals: Vec<f64> = Vec::new();
        let mut uv_values: Vec<Vec<f64>> = vec![Vec::new(); uv_sets];
        let mut uv_index: Vec<Vec<i32>> = vec![Vec::new(); uv_sets];
        let mut uv_seen: Vec<HashMap<[u32; 2], i32>> = vec![HashMap::new(); uv_sets];
        let mut color_values: Vec<f64> = Vec::new();
        let mut color_index: Vec<i32> = Vec::new();
        let mut color_seen: HashMap<[u32; 4], i32> = HashMap::new();
        let mut materials: Vec<i32> = Vec::new();
        let mut edges: Vec<i32> = Vec::new();
        let mut edge_seen: HashSet<(u32, u32)> = HashSet::new();
        for (pi, prim) in mesh.primitives.iter().enumerate() {
            let derived = if any_normals && prim.normals.is_none() {
                Some(pose::derived_normals(&prim.positions, &prim.indices))
            } else {
                None
            };
            for tri in prim.indices.chunks_exact(3) {
                let cps = [cp_of[pi][tri[0] as usize], cp_of[pi][tri[1] as usize], cp_of[pi][tri[2] as usize]];
                let first = pvi.len();
                pvi.push(cps[0] as i32);
                pvi.push(cps[1] as i32);
                // The last corner of a polygon is stored as -(index + 1).
                pvi.push(-(cps[2] as i32) - 1);
                for k in 0..3 {
                    let (a, b) = (cps[k], cps[(k + 1) % 3]);
                    if edge_seen.insert((a.min(b), a.max(b))) {
                        edges.push((first + k) as i32);
                    }
                }
                materials.push(slot_of[pi]);
                for &v in tri {
                    let v = v as usize;
                    if any_normals {
                        let n = match (&prim.normals, &derived) {
                            (Some(ns), _) => ns[v],
                            (None, Some(d)) => d[v],
                            _ => [0.0, 0.0, 1.0],
                        };
                        normals.extend(n.iter().map(|x| *x as f64));
                        if all_tangents {
                            let t = prim.tangents.as_ref().map_or([1.0, 0.0, 0.0, 1.0], |t| t[v]);
                            tangents.extend(t[..3].iter().map(|x| *x as f64));
                            // FBX keeps handedness in the binormal: N × T, signed.
                            let w = if t[3] < 0.0 { -1.0 } else { 1.0 };
                            let b = [
                                (n[1] * t[2] - n[2] * t[1]) * w,
                                (n[2] * t[0] - n[0] * t[2]) * w,
                                (n[0] * t[1] - n[1] * t[0]) * w,
                            ];
                            binormals.extend(b.iter().map(|x| *x as f64));
                        }
                    }
                    for set in 0..uv_sets {
                        let uv = prim.uvs.get(set).map_or([0.0, 0.0], |s| s[v]);
                        let key = [uv[0].to_bits(), uv[1].to_bits()];
                        let next = (uv_values[set].len() / 2) as i32;
                        let idx = *uv_seen[set].entry(key).or_insert_with(|| {
                            // glTF's V runs down the image, FBX's up.
                            uv_values[set].extend_from_slice(&[uv[0] as f64, 1.0 - uv[1] as f64]);
                            next
                        });
                        uv_index[set].push(idx);
                    }
                    if any_colors {
                        let c = prim.colors.as_ref().map_or([1.0; 4], |c| c[v]);
                        let key = [c[0].to_bits(), c[1].to_bits(), c[2].to_bits(), c[3].to_bits()];
                        let next = (color_values.len() / 4) as i32;
                        let idx = *color_seen.entry(key).or_insert_with(|| {
                            color_values.extend_from_slice(&[srgb_encode(c[0]), srgb_encode(c[1]), srgb_encode(c[2]), c[3] as f64]);
                            next
                        });
                        color_index.push(idx);
                    }
                }
            }
        }

        let id = self.id();
        let name = mesh.name.clone().unwrap_or_default().replace(|c: char| c.is_control(), "_");
        let mut g = rec("Geometry", vec![WProp::I64(id), obj_name(&name, "Geometry"), ws("Mesh")])
            .child(rec("Vertices", vec![WProp::ArrF64(cp_pos.iter().flat_map(|p| p.iter().map(|x| *x as f64)).collect())]))
            .child(rec("PolygonVertexIndex", vec![WProp::ArrI32(pvi)]))
            .child(rec("Edges", vec![WProp::ArrI32(edges)]))
            .child(leaf_i32("GeometryVersion", 124));
        let element = |name: &'static str, index: i32, layer_name: &str, mapping: &str, reference: &str| -> WRec {
            rec(name, vec![WProp::I32(index)])
                .child(leaf_i32("Version", 101))
                .child(leaf_str("Name", layer_name))
                .child(leaf_str("MappingInformationType", mapping))
                .child(leaf_str("ReferenceInformationType", reference))
        };
        let mut layer0: Vec<&'static str> = Vec::new();
        if any_normals {
            g = g.child(element("LayerElementNormal", 0, "", "ByPolygonVertex", "Direct").child(rec("Normals", vec![WProp::ArrF64(normals)])));
            layer0.push("LayerElementNormal");
        }
        if all_tangents {
            g = g.child(element("LayerElementBinormal", 0, "", "ByPolygonVertex", "Direct").child(rec("Binormals", vec![WProp::ArrF64(binormals)])));
            g = g.child(element("LayerElementTangent", 0, "", "ByPolygonVertex", "Direct").child(rec("Tangents", vec![WProp::ArrF64(tangents)])));
            layer0.push("LayerElementBinormal");
            layer0.push("LayerElementTangent");
        }
        if any_colors {
            g = g.child(
                element("LayerElementColor", 0, "Col", "ByPolygonVertex", "IndexToDirect")
                    .child(rec("Colors", vec![WProp::ArrF64(color_values)]))
                    .child(rec("ColorIndex", vec![WProp::ArrI32(color_index)])),
            );
            layer0.push("LayerElementColor");
        }
        for set in 0..uv_sets {
            g = g.child(
                element("LayerElementUV", set as i32, &uv_set_name(set), "ByPolygonVertex", "IndexToDirect")
                    .child(rec("UV", vec![WProp::ArrF64(std::mem::take(&mut uv_values[set]))]))
                    .child(rec("UVIndex", vec![WProp::ArrI32(std::mem::take(&mut uv_index[set]))])),
            );
        }
        // Always written: Assimp ignores a model's materials without it.
        let single = slots.len() == 1;
        g = g.child(
            element("LayerElementMaterial", 0, "", if single { "AllSame" } else { "ByPolygon" }, "IndexToDirect")
                .child(rec("Materials", vec![WProp::ArrI32(if single { vec![0] } else { materials })])),
        );
        layer0.push("LayerElementMaterial");
        if uv_sets > 0 {
            layer0.push("LayerElementUV");
        }
        let layer_element = |ty: &str, index: i32| -> WRec {
            WRec::new("LayerElement").child(leaf_str("Type", ty)).child(leaf_i32("TypedIndex", index))
        };
        let mut l0 = rec("Layer", vec![WProp::I32(0)]).child(leaf_i32("Version", 100));
        for ty in layer0 {
            l0 = l0.child(layer_element(ty, 0));
        }
        g = g.child(l0);
        for set in 1..uv_sets {
            g = g.child(
                rec("Layer", vec![WProp::I32(set as i32)])
                    .child(leaf_i32("Version", 100))
                    .child(layer_element("LayerElementUV", set as i32)),
            );
        }
        self.object("Geometry", g);

        // Blend shapes: one channel per target, one Shape each.
        let mut channels = Vec::new();
        if targets > 0 {
            let bs = self.id();
            let r = rec("Deformer", vec![WProp::I64(bs), obj_name(&name, "Deformer"), ws("BlendShape")]).child(leaf_i32("Version", 100));
            self.object("Deformer", r);
            self.oo(bs, id);
            let mut channel_names = Names::default();
            for t in 0..targets {
                let cname = channel_names.unique(mesh.target_names.get(t).map(|s| s.as_str()), &format!("target{t}"));
                let percent = weights.get(t).copied().unwrap_or(0.0) as f64 * 100.0;
                let ch = self.id();
                let r = rec("Deformer", vec![WProp::I64(ch), obj_name(&cname, "SubDeformer"), ws("BlendShapeChannel")])
                    .child(leaf_i32("Version", 100))
                    // Both spellings: Assimp and OpenFBX read the child,
                    // ufbx the property.
                    .child(rec("DeformPercent", vec![WProp::F64(percent)]))
                    .child(rec("FullWeights", vec![WProp::ArrF64(vec![100.0])]))
                    .child(props70(vec![p("DeformPercent", "Number", "", "A", vec![WProp::F64(percent)])]));
                self.object("Deformer", r);
                self.oo(ch, bs);
                let has_normals = mesh.primitives.iter().any(|p| p.targets[t].normals.is_some());
                let mut idx = Vec::new();
                let mut dv = Vec::new();
                let mut dn = Vec::new();
                for (cp, &(pi, v)) in cp_rep.iter().enumerate() {
                    let tg = &mesh.primitives[pi].targets[t];
                    let d = tg.positions[v];
                    let n = tg.normals.as_ref().map_or([0.0; 3], |n| n[v]);
                    if d == [0.0; 3] && n == [0.0; 3] {
                        continue;
                    }
                    idx.push(cp as i32);
                    dv.extend(d.iter().map(|x| *x as f64));
                    dn.extend(n.iter().map(|x| *x as f64));
                }
                let sh = self.id();
                let mut r = rec("Geometry", vec![WProp::I64(sh), obj_name(&cname, "Geometry"), ws("Shape")])
                    .child(leaf_i32("Version", 100))
                    .child(rec("Indexes", vec![WProp::ArrI32(idx)]))
                    .child(rec("Vertices", vec![WProp::ArrF64(dv)]));
                if has_normals {
                    r = r.child(rec("Normals", vec![WProp::ArrF64(dn)]));
                }
                self.object("Geometry", r);
                self.oo(sh, ch);
                channels.push(ch);
            }
        }

        // Material ids for the slots, created on first use.
        let mut slot_ids = Vec::with_capacity(slots.len());
        for m in &slots {
            slot_ids.push(match m {
                Some(m) => self.material(*m)?,
                None => self.default_material(),
            });
        }
        self.geoms.push(GeomOut { id, cp_rep, channels });
        let gi = self.geoms.len() - 1;
        self.geom_slots.push(slot_ids);
        Ok(gi)
    }
}

fn uv_set_name(set: usize) -> String {
    format!("UVSet{set}")
}

#[allow(clippy::too_many_arguments)]
fn model_record(id: i64, name: &str, class: &str, t: [f64; 3], q: [f64; 4], sc: [f64; 3], animated: [bool; 3]) -> WRec {
    let e = degrees(math::quat_to_euler(q, EulerOrder::Xyz));
    let flag = |a: bool| if a { "A+" } else { "A" };
    rec("Model", vec![WProp::I64(id), obj_name(name, "Model"), ws(class)])
        .child(leaf_i32("Version", 232))
        .child(props70(vec![
            // RotationActive: without it ufbx (Godot, Blender 4.5) ignores the
            // rotation order. InheritType 1 is ordinary matrix composition —
            // glTF's — where the template default (0) is not.
            p_int("RotationActive", "bool", 1),
            p_int("InheritType", "enum", 1),
            p_int("DefaultAttributeIndex", "int", 0),
            p_vec3("Lcl Translation", "Lcl Translation", "", flag(animated[0]), t),
            p_vec3("Lcl Rotation", "Lcl Rotation", "", flag(animated[1]), e),
            // three.js composes and then decomposes this, so exactly zero
            // would come back as unit scale (`math::nonsingular_scale`).
            p_vec3("Lcl Scaling", "Lcl Scaling", "", flag(animated[2]), math::nonsingular_scale(sc)),
        ]))
        .child(rec("Shading", vec![WProp::Byte(b'T')]))
        .child(leaf_str("Culling", "CullingOff"))
}

impl Writer<'_> {
    fn default_material(&mut self) -> i64 {
        if let Some(id) = self.default_material {
            return id;
        }
        let m = super::scene::Material { name: Some("default".into()), ..Default::default() };
        let id = self.material_record(&m, &[]);
        self.default_material = Some(id);
        id
    }

    fn material(&mut self, mi: usize) -> Result<i64, MeshError> {
        if let Some(Some(id)) = self.material_ids.get(mi) {
            return Ok(*id);
        }
        let m = self.s.materials[mi].clone();
        let mut slots: Vec<(&'static str, i64)> = Vec::new();
        for (r, slot) in [
            (&m.base_color_texture, "DiffuseColor"),
            (&m.normal_texture, "NormalMap"),
            (&m.emissive_texture, "EmissiveColor"),
        ] {
            if let Some(r) = r {
                if let Some(tid) = self.texture(r)? {
                    slots.push((slot, tid));
                }
            }
        }
        let id = self.material_record(&m, &slots);
        if let Some(slot) = self.material_ids.get_mut(mi) {
            *slot = Some(id);
        }
        Ok(id)
    }

    /// A Phong material in Blender's encoding of metallic-roughness —
    /// `ShininessExponent = (10·(1 − roughness))²`, `ReflectionFactor =
    /// metallic`, which Assimp, Unity and ufbx decode — plus the same factors
    /// as Stingray `Maya|…` properties, which Assimp reads as real PBR and
    /// which give this module's reader an exact round trip. Colours are sRGB
    /// (see `fbx::srgb_encode`), except the Stingray ones, which are linear
    /// like the shader they belong to.
    ///
    /// Under a map the colour is written white and only a factor scales the
    /// map: readers disagree about a colour beside a map (the SDK, Maya and
    /// Blender let the map replace it; three.js and Unity multiply it in), and
    /// white means the same to both. `fbx::prepare` applies the same rule.
    fn material_record(&mut self, m: &super::scene::Material, textures: &[(&'static str, i64)]) -> i64 {
        let id = self.id();
        let name = m.name.clone().unwrap_or_default().replace(|c: char| c.is_control(), "_");
        let alpha = (m.base_color[3] as f64).clamp(0.0, 1.0);
        let base = if m.base_color_texture.is_some() { [1.0; 3] } else { [m.base_color[0], m.base_color[1], m.base_color[2]] };
        let rgb = base.map(srgb_encode);
        // Emission above 1 rides in the factor, as KHR_materials_emissive_strength
        // does; under a map, all of it does (prepare has made it grey).
        let peak = m.emissive.iter().cloned().fold(0.0f32, f32::max);
        let (e_lin, e_factor) = if m.emissive_texture.is_some() {
            ([1.0; 3], peak as f64)
        } else if peak > 1.0 {
            (m.emissive.map(|c| c / peak), peak as f64)
        } else {
            (m.emissive, 1.0)
        };
        let e_srgb = [srgb_encode(e_lin[0]), srgb_encode(e_lin[1]), srgb_encode(e_lin[2])];
        let roughness = (m.roughness as f64).clamp(0.0, 1.0);
        let shininess = (10.0 * (1.0 - roughness)).powi(2);
        let mut ps = vec![
            p_str("ShadingModel", "KString", "", "Phong"),
            p_color("EmissiveColor", e_srgb),
            p_num("EmissiveFactor", e_factor),
            p_color("AmbientColor", [0.0; 3]),
            p_color("DiffuseColor", rgb),
            p_num("DiffuseFactor", 1.0),
            // Transparency both ways: SDK readers use TransparentColor ×
            // TransparencyFactor, three.js and Assimp prefer Opacity.
            p_color("TransparentColor", [1.0; 3]),
            p_num("TransparencyFactor", 1.0 - alpha),
            p_num("Opacity", alpha),
            p_color("SpecularColor", [1.0; 3]),
            p_num("SpecularFactor", 0.25),
            p_num("ShininessExponent", shininess),
            p_num("Shininess", shininess),
            p_num("ReflectionFactor", m.metallic as f64),
        ];
        if m.normal_texture.is_some() {
            ps.push(p_vec3("NormalMap", "Vector3D", "Vector", "", [0.0; 3]));
            ps.push(p_double("BumpFactor", m.normal_scale as f64));
        }
        ps.push(p_vec3("Maya|base_color", "Vector3D", "Vector", "", base.map(|c| c as f64)));
        ps.push(p_double("Maya|metallic", m.metallic as f64));
        ps.push(p_double("Maya|roughness", m.roughness as f64));
        ps.push(p_vec3("Maya|emissive", "Vector3D", "Vector", "", e_lin.map(|c| c as f64)));
        ps.push(p_double("Maya|emissive_intensity", e_factor));
        let r = rec("Material", vec![WProp::I64(id), obj_name(&name, "Material"), ws("")])
            .child(leaf_i32("Version", 102))
            .child(leaf_str("ShadingModel", "Phong"))
            .child(leaf_i32("MultiLayer", 0))
            .child(props70(ps));
        self.object("Material", r);
        for (slot, tid) in textures {
            self.op(*tid, id, slot);
        }
        id
    }

    /// The Texture object for a reference, sharing one per (texture, uv set).
    /// `None` when the image cannot travel in FBX. UV transforms never reach
    /// here: `fbx::capabilities` declares none, so `pose::degrade` has baked
    /// them into the texture coordinates (FBX's Translation/Scaling cannot
    /// express a rotation, and three.js ignores its Rotation).
    fn texture(&mut self, r: &super::scene::TextureRef) -> Result<Option<i64>, MeshError> {
        let s = self.s;
        let Some(tex) = s.textures.get(r.texture) else { return Ok(None) };
        let Some(img) = s.images.get(tex.image) else { return Ok(None) };
        if !img.format.is_universal() {
            return Ok(None);
        }
        let key = (r.texture, r.uv_set);
        if let Some(id) = self.texture_ids.get(&key) {
            return Ok(Some(*id));
        }
        let (video, file) = self.video(tex.image, tex.name.as_deref());
        let id = self.id();
        let name = tex.name.clone().unwrap_or_else(|| file.clone()).replace(|c: char| c.is_control(), "_");
        let mut ps = vec![p_str("UVSet", "KString", "", &uv_set_name(r.uv_set as usize)), p_int("UseMaterial", "bool", 1)];
        if tex.sampler.wrap_s == Wrap::ClampToEdge {
            ps.push(p_int("WrapModeU", "enum", 1));
        }
        if tex.sampler.wrap_t == Wrap::ClampToEdge {
            ps.push(p_int("WrapModeV", "enum", 1));
        }
        let vname = s_name_bytes(&name, "Video");
        let r = rec("Texture", vec![WProp::I64(id), obj_name(&name, "Texture"), ws("")])
            .child(leaf_str("Type", "TextureVideoClip"))
            .child(leaf_i32("Version", 202))
            .child(rec("TextureName", vec![obj_name(&name, "Texture")]))
            .child(props70(ps))
            .child(rec("Media", vec![WProp::Str(vname)]))
            .child(leaf_str("FileName", &file))
            .child(leaf_str("RelativeFilename", &file))
            .child(rec("ModelUVTranslation", vec![WProp::F64(0.0), WProp::F64(0.0)]))
            .child(rec("ModelUVScaling", vec![WProp::F64(1.0), WProp::F64(1.0)]))
            .child(leaf_str("Texture_Alpha_Source", "None"))
            .child(rec("Cropping", vec![WProp::I32(0), WProp::I32(0), WProp::I32(0), WProp::I32(0)]));
        self.object("Texture", r);
        self.oo(video, id);
        self.texture_ids.insert(key, id);
        Ok(Some(id))
    }

    /// The Video (embedded image) for an image, one per image. Its file name
    /// carries the real extension because three.js picks the decoder from it,
    /// and is unique because three.js keys embedded blobs by it.
    fn video(&mut self, image: usize, texture_name: Option<&str>) -> (i64, String) {
        if let Some(v) = self.video_ids.get(&image) {
            return v.clone();
        }
        let img = &self.s.images[image];
        let stem = super::package::safe_file_stem(img.name.as_deref().or(texture_name).unwrap_or(""), &format!("image_{image}"));
        let ext = img.format.extension();
        let mut file = format!("{stem}.{ext}");
        let mut n = 1;
        while self.video_names.contains(&file) {
            file = format!("{stem}_{n}.{ext}");
            n += 1;
        }
        self.video_names.insert(file.clone());
        let id = self.id();
        let name = img.name.clone().unwrap_or_else(|| stem.clone()).replace(|c: char| c.is_control(), "_");
        let mut r = rec("Video", vec![WProp::I64(id), obj_name(&name, "Video"), ws("Clip")])
            .child(leaf_str("Type", "Clip"))
            .child(props70(vec![p_str("Path", "KString", "XRefUrl", &file), p_str("RelPath", "KString", "XRefUrl", &file)]))
            .child(leaf_i32("UseMipMap", 0))
            .child(leaf_str("Filename", &file))
            .child(leaf_str("RelativeFilename", &file));
        if self.embed {
            r = r.child(rec("Content", vec![WProp::Raw(img.data.clone())]));
        }
        self.object("Video", r);
        self.video_ids.insert(image, (id, file.clone()));
        (id, file)
    }
}

fn s_name_bytes(name: &str, class: &str) -> Vec<u8> {
    let mut b = name.as_bytes().to_vec();
    b.extend_from_slice(b"\x00\x01");
    b.extend_from_slice(class.as_bytes());
    b
}

// ─── skins, animation, document ─────────────────────────────────────────────

impl Writer<'_> {
    /// Skin + clusters for node `ni`'s geometry `gi`, and its BindPose entries.
    fn skin(&mut self, ni: usize, gi: usize) -> Result<(), MeshError> {
        let s = self.s;
        let Some(si) = s.nodes[ni].skin else { return Ok(()) };
        let Some(mi) = s.nodes[ni].mesh else { return Ok(()) };
        let skin = &s.skins[si];
        let world = s.world_matrices();
        let mesh = &s.meshes[mi];
        // M: the mesh model's global matrix at bind time. Skinned meshes are
        // hoisted to identity roots, so this is the identity in practice; the
        // general form costs nothing.
        let m = world[ni];
        let skin_id = self.id();
        let name = self.holder_name[ni].clone();
        let r = rec("Deformer", vec![WProp::I64(skin_id), obj_name(&name, "Deformer"), ws("Skin")])
            .child(leaf_i32("Version", 101))
            .child(rec("Link_DeformAcuracy", vec![WProp::F64(50.0)]))
            .child(leaf_str("SkinningType", "Linear"));
        self.object("Deformer", r);
        let geom_id = self.geoms[gi].id;
        self.oo(skin_id, geom_id);
        // Per joint: the control points it moves and by how much.
        let joints = skin.joints.len();
        let mut per_joint: Vec<(Vec<i32>, Vec<f64>)> = vec![(Vec::new(), Vec::new()); joints];
        for (cp, &(pi, v)) in self.geoms[gi].cp_rep.iter().enumerate() {
            for (j, w) in influences(&mesh.primitives[pi], v) {
                if let Some(slot) = per_joint.get_mut(j as usize) {
                    slot.0.push(cp as i32);
                    slot.1.push(w as f64);
                }
            }
        }
        self.bind_pose.push((self.holder[ni], m));
        for (ji, (idx, wts)) in per_joint.into_iter().enumerate() {
            let joint = skin.joints[ji];
            let ibm = skin.inverse_bind_matrices[ji];
            // Transform = the inverse bind matrix (mesh space → bone space);
            // TransformLink = the bone's world matrix at bind time.
            let link = math::invert(&ibm).map(|inv| math::mat_mul(&m, &inv)).unwrap_or(world[joint]);
            let cid = self.id();
            let jname = self.s.nodes[joint].name.clone().unwrap_or_default().replace(|c: char| c.is_control(), "_");
            let mut r = rec("Deformer", vec![WProp::I64(cid), obj_name(&jname, "SubDeformer"), ws("Cluster")])
                .child(leaf_i32("Version", 100))
                .child(rec("UserData", vec![ws(""), ws("")]));
            if !idx.is_empty() {
                r = r.child(rec("Indexes", vec![WProp::ArrI32(idx)])).child(rec("Weights", vec![WProp::ArrF64(wts)]));
            }
            r = r
                .child(rec("Transform", vec![WProp::ArrF64(ibm.to_vec())]))
                .child(rec("TransformLink", vec![WProp::ArrF64(link.to_vec())]));
            self.object("Deformer", r);
            self.oo(cid, skin_id);
            let jm = self.model[joint];
            self.oo(jm, cid);
            self.bind_pose.push((jm, link));
        }
        // Maya also lists every non-joint ancestor of a joint.
        let parents = s.parents();
        for &j in &skin.joints {
            let mut cur = parents[j];
            let mut steps = 0;
            while let Some(a) = cur {
                steps += 1;
                if steps > s.nodes.len() {
                    break;
                }
                if !skin.joints.contains(&a) {
                    self.bind_pose.push((self.model[a], world[a]));
                }
                cur = parents[a];
            }
        }
        Ok(())
    }

    fn bind_pose_object(&mut self) {
        if self.bind_pose.is_empty() {
            return;
        }
        let mut seen = HashSet::new();
        let entries: Vec<(i64, Mat4)> = self.bind_pose.iter().filter(|(id, _)| seen.insert(*id)).cloned().collect();
        let id = self.id();
        let mut r = rec("Pose", vec![WProp::I64(id), obj_name("BindPose", "Pose"), ws("BindPose")])
            .child(leaf_str("Type", "BindPose"))
            .child(leaf_i32("Version", 100))
            .child(leaf_i32("NbPoseNodes", entries.len() as i32));
        for (model, m) in entries {
            r = r.child(
                WRec::new("PoseNode")
                    .child(rec("Node", vec![WProp::I64(model)]))
                    .child(rec("Matrix", vec![WProp::ArrF64(m.to_vec())])),
            );
        }
        self.object("Pose", r);
    }

    /// One AnimationStack per clip.
    fn animations(&mut self) -> Result<Vec<(String, i64, i64)>, MeshError> {
        let s = self.s;
        let mut stacks = Vec::new();
        let mut stack_names = Names::default();
        for (ai, anim) in s.animations.iter().enumerate() {
            let Some((lo, hi)) = anim.time_range() else { continue };
            // Every source key, 30 fps between, and for STEP channels a key
            // just before each step, so readers that ignore the constant flag
            // (three.js) still see a step and not a ramp.
            let mut times = pose::resample_times(s, ai, RESAMPLE_FPS);
            for ch in &anim.channels {
                if ch.interpolation == Interpolation::Step {
                    for k in 1..ch.times.len() {
                        let t = ch.times[k] - STEP_EPSILON;
                        if t > ch.times[k - 1] {
                            times.push(t);
                        }
                    }
                }
            }
            times.sort_by(|a, b| a.total_cmp(b));
            times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
            let per_time = s.nodes.len() + anim.channels.len();
            let too_many = |times: &[f32]| times.len() > super::scene::MAX_ANIMATION_KEYS || times.len().saturating_mul(per_time) > MAX_RESAMPLE_WORK;
            if too_many(&times) {
                return Err(unsupported(format!(
                    "animation {ai} would resample to {} keys on {} nodes, more than this converter evaluates",
                    times.len(),
                    s.nodes.len()
                )));
            }
            // What each node animates, and whether every channel of a path
            // steps (only then is the curve constant-interpolated).
            let mut paths: BTreeMap<usize, [Option<bool>; 4]> = BTreeMap::new();
            for ch in &anim.channels {
                let slot = match ch.path {
                    AnimPath::Translation => 0,
                    AnimPath::Rotation => 1,
                    AnimPath::Scale => 2,
                    AnimPath::Weights => 3,
                };
                let step = ch.interpolation == Interpolation::Step;
                let e = paths.entry(ch.node).or_insert([None; 4]);
                e[slot] = Some(e[slot].map_or(step, |prev| prev && step));
            }
            let interpolated: Vec<usize> = paths.iter().filter(|(_, p)| p[1] == Some(false)).map(|(n, _)| *n).collect();
            let budget = (MAX_RESAMPLE_WORK / per_time.max(1)).min(super::scene::MAX_ANIMATION_KEYS);
            refine_euler_times(s, ai, &interpolated, &mut times, budget);
            // Sample once per time; keep only the animated values.
            let mut tracks: BTreeMap<(usize, usize), Vec<f64>> = BTreeMap::new();
            let mut prev_euler: HashMap<usize, [f64; 3]> = HashMap::new();
            for &t in &times {
                let pose = pose::sample_pose(s, ai, t);
                for (&node, slots) in &paths {
                    let (tr, rot, sc) = pose.trs[node].unwrap_or(([0.0; 3], math::QUAT_IDENTITY, [1.0; 3]));
                    if slots[0].is_some() {
                        tracks.entry((node, 0)).or_default().extend(tr);
                    }
                    if slots[1].is_some() {
                        let e = math::quat_to_euler(rot, EulerOrder::Xyz);
                        let e = match prev_euler.get(&node) {
                            Some(prev) => math::euler_unwrap(e, *prev, EulerOrder::Xyz),
                            None => e,
                        };
                        prev_euler.insert(node, e);
                        tracks.entry((node, 1)).or_default().extend(degrees(e));
                    }
                    if slots[2].is_some() {
                        tracks.entry((node, 2)).or_default().extend(sc);
                    }
                    if slots[3].is_some() {
                        let w = pose.weights.get(node).cloned().unwrap_or_default();
                        tracks.entry((node, 3)).or_default().extend(w.iter().map(|x| *x as f64 * 100.0));
                    }
                }
            }
            let name = stack_names.unique(anim.name.as_deref(), &format!("Take{ai}"));
            // The take starts at 0, where playback does, and the curves hold
            // the first pose until the first key (`pose::resample_times`).
            let (t0, t1) = (ticks(lo.min(0.0)), ticks(hi));
            let stack = self.id();
            let r = rec("AnimationStack", vec![WProp::I64(stack), obj_name(&name, "AnimStack"), ws("")]).child(props70(vec![
                p_ktime("LocalStart", t0),
                p_ktime("LocalStop", t1),
                p_ktime("ReferenceStart", t0),
                p_ktime("ReferenceStop", t1),
            ]));
            self.object("AnimationStack", r);
            let layer = self.id();
            let r = WRec { force_null: true, ..rec("AnimationLayer", vec![WProp::I64(layer), obj_name("BaseLayer", "AnimLayer"), ws("")]) };
            self.object("AnimationLayer", r);
            self.oo(layer, stack);
            let key_times: Vec<i64> = times.iter().map(|t| ticks(*t)).collect();
            for ((node, slot), values) in tracks {
                let step = paths.get(&node).and_then(|p| p[slot]).unwrap_or(false);
                if slot < 3 {
                    let (cn_name, prop) = [("T", "Lcl Translation"), ("R", "Lcl Rotation"), ("S", "Lcl Scaling")][slot];
                    let first = [values[0], values[1], values[2]];
                    let cn = self.curve_node(cn_name, &[("d|X", first[0]), ("d|Y", first[1]), ("d|Z", first[2])]);
                    self.oo(cn, layer);
                    let model = self.model[node];
                    self.op(cn, model, prop);
                    for (axis, label) in ["d|X", "d|Y", "d|Z"].iter().enumerate() {
                        let v: Vec<f32> = values.iter().skip(axis).step_by(3).map(|x| *x as f32).collect();
                        let c = self.curve(&key_times, v, step);
                        self.op(c, cn, label);
                    }
                } else {
                    let Some(gi) = self.geom_of_node.get(node).copied().flatten() else { continue };
                    let channels = self.geoms[gi].channels.clone();
                    let n = channels.len();
                    if n == 0 || values.len() != n * times.len() {
                        continue;
                    }
                    for (k, ch) in channels.iter().enumerate() {
                        let v: Vec<f32> = values.iter().skip(k).step_by(n).map(|x| *x as f32).collect();
                        let cn = self.curve_node("DeformPercent", &[("d|DeformPercent", v[0] as f64)]);
                        self.oo(cn, layer);
                        self.op(cn, *ch, "DeformPercent");
                        let c = self.curve(&key_times, v, step);
                        self.op(c, cn, "d|DeformPercent");
                    }
                }
            }
            stacks.push((name, t0, t1));
        }
        Ok(stacks)
    }

    fn curve_node(&mut self, name: &str, defaults: &[(&str, f64)]) -> i64 {
        let id = self.id();
        let ps = defaults.iter().map(|(k, v)| p(k, "Number", "", "A", vec![WProp::F64(*v)])).collect();
        let r = rec("AnimationCurveNode", vec![WProp::I64(id), obj_name(name, "AnimCurveNode"), ws("")]).child(props70(ps));
        self.object("AnimationCurveNode", r);
        id
    }

    fn curve(&mut self, times: &[i64], values: Vec<f32>, step: bool) -> i64 {
        let id = self.id();
        let flags = if step { KEY_CONSTANT } else { KEY_LINEAR };
        let r = rec("AnimationCurve", vec![WProp::I64(id), obj_name("", "AnimCurve"), ws("")])
            .child(rec("Default", vec![WProp::F64(values.first().copied().unwrap_or(0.0) as f64)]))
            .child(leaf_i32("KeyVer", 4009))
            .child(rec("KeyTime", vec![WProp::ArrI64(times.to_vec())]))
            .child(rec("KeyValueFloat", vec![WProp::ArrF32(values)]))
            // One attribute run covering every key.
            .child(rec("KeyAttrFlags", vec![WProp::ArrI32(vec![flags])]))
            .child(rec("KeyAttrDataFloat", vec![WProp::ArrF32(vec![0.0, 0.0, f32::from_bits(DEFAULT_KEY_WEIGHTS), 0.0])]))
            .child(rec("KeyAttrRefCount", vec![WProp::ArrI32(vec![times.len() as i32])]));
        self.object("AnimationCurve", r);
        id
    }
}

/// Add key times until every interpolated rotation curve stays within
/// [`EULER_TOLERANCE_DEG`] of the scene's rotation between its keys.
///
/// FBX readers that evaluate curves (the SDK and so Maya, 3ds Max, Unity and
/// Unreal; Blender; this module's reader) interpolate each Euler angle on its
/// own, while the scene's rotation follows the arc between quaternions. The
/// two agree at every key and part between them, slightly almost everywhere,
/// by degrees near gimbal lock — the middle axis near ±90°, where the outer
/// two angles swing fast — however fine the fixed resampling rate.
/// CesiumMan's right leg, keyed at 24 fps and resampled at 30, was 2.5° off
/// at t = 1.178, which three.js never showed: it turns every key into a
/// quaternion and slerps. Checked at a quarter, half and three quarters of
/// each segment, against the Euler values exactly as the tracks spell them
/// (sequentially unwrapped), keying the midpoint of each segment that strays
/// and repeating until none does or [`EULER_MAX_ROUNDS`] or `budget` (sample
/// times this animation may have) runs out.
fn refine_euler_times(s: &Scene, ai: usize, nodes: &[usize], times: &mut Vec<f32>, budget: usize) {
    if nodes.is_empty() || times.len() < 2 {
        return;
    }
    let rotations = |t: f32| -> Vec<[f64; 4]> {
        let pose = pose::sample_pose(s, ai, t);
        nodes.iter().map(|&n| pose.trs[n].map_or(math::QUAT_IDENTITY, |x| x.1)).collect()
    };
    let angle = |a: [f64; 4], b: [f64; 4]| 2.0 * math::quat_dot(a, b).abs().min(1.0).acos().to_degrees();
    let mut at_key: HashMap<u32, Vec<[f64; 4]>> = HashMap::new();
    // Segments that passed, by their end times and a hash of the Euler
    // values at both ends: a new key can change how later keys unwrap, and a
    // segment spelled differently is checked again.
    let mut settled: HashMap<(u32, u32), u64> = HashMap::new();
    let spelling = |ea: &[[f64; 3]], eb: &[[f64; 3]]| {
        ea.iter().chain(eb).flatten().fold(0xcbf2_9ce4_8422_2325u64, |h, v| (h ^ v.to_bits()).wrapping_mul(0x0100_0000_01b3))
    };
    for _ in 0..EULER_MAX_ROUNDS {
        let mut chain: Vec<Vec<[f64; 3]>> = Vec::with_capacity(times.len());
        for (k, t) in times.iter().enumerate() {
            let rots = at_key.entry(t.to_bits()).or_insert_with(|| rotations(*t));
            let row: Vec<[f64; 3]> = rots
                .iter()
                .enumerate()
                .map(|(i, q)| {
                    let e = math::quat_to_euler(*q, EulerOrder::Xyz);
                    match k.checked_sub(1) {
                        Some(prev) => math::euler_unwrap(e, chain[prev][i], EulerOrder::Xyz),
                        None => e,
                    }
                })
                .collect();
            chain.push(row);
        }
        let mut extra = Vec::new();
        for k in 0..times.len() - 1 {
            let (a, b) = (times[k], times[k + 1]);
            let spelt = spelling(&chain[k], &chain[k + 1]);
            if b - a < EULER_MIN_GAP || settled.get(&(a.to_bits(), b.to_bits())) == Some(&spelt) {
                continue;
            }
            let strays = [0.25f32, 0.5, 0.75].iter().any(|&u| {
                let t = a + (b - a) * u;
                let f = ((t - a) / (b - a)) as f64;
                let want = rotations(t);
                (0..nodes.len()).any(|i| {
                    let (ea, eb) = (chain[k][i], chain[k + 1][i]);
                    let e = [ea[0] + (eb[0] - ea[0]) * f, ea[1] + (eb[1] - ea[1]) * f, ea[2] + (eb[2] - ea[2]) * f];
                    angle(math::euler_to_quat(e, EulerOrder::Xyz), want[i]) > EULER_TOLERANCE_DEG
                })
            });
            if strays {
                extra.push(a + (b - a) * 0.5);
            } else {
                settled.insert((a.to_bits(), b.to_bits()), spelt);
            }
        }
        if extra.is_empty() || times.len() + extra.len() > budget {
            return;
        }
        times.extend(extra);
        times.sort_by(|a, b| a.total_cmp(b));
        times.dedup();
    }
}

impl Writer<'_> {
    fn connect_geometry(&mut self, ni: usize, gi: usize) {
        let holder = self.holder[ni];
        let gid = self.geoms[gi].id;
        // Geometry first: three.js finds a blend shape's model as the
        // geometry's FIRST parent.
        self.oo(gid, holder);
        let slots = self.geom_slots[gi].clone();
        for m in slots {
            self.oo(m, holder);
        }
    }
}

struct Built {
    objects: Vec<WRec>,
    conns: Vec<WRec>,
    counts: BTreeMap<&'static str, usize>,
    stacks: Vec<(String, i64, i64)>,
    has_joints: bool,
    has_nulls: bool,
}

fn build(scene: &Scene, embed: bool) -> Result<Built, MeshError> {
    let mut w = Writer {
        s: scene,
        embed,
        next_id: FIRST_ID,
        objects: Vec::new(),
        conns: Vec::new(),
        counts: BTreeMap::new(),
        names: Names::default(),
        model: Vec::new(),
        holder: Vec::new(),
        holder_name: Vec::new(),
        geom_of_node: vec![None; scene.nodes.len()],
        geoms: Vec::new(),
        geom_slots: Vec::new(),
        material_ids: vec![None; scene.materials.len()],
        default_material: None,
        texture_ids: HashMap::new(),
        video_ids: HashMap::new(),
        video_names: HashSet::new(),
        bind_pose: Vec::new(),
    };
    // Which TRS paths each node animates, for the "A+" flags.
    let mut animated = vec![[false; 3]; scene.nodes.len()];
    for a in &scene.animations {
        for c in &a.channels {
            match c.path {
                AnimPath::Translation => animated[c.node][0] = true,
                AnimPath::Rotation => animated[c.node][1] = true,
                AnimPath::Scale => animated[c.node][2] = true,
                AnimPath::Weights => {}
            }
        }
    }
    w.models(&animated)?;
    // Geometry per mesh, or per node when the node deforms it (a skin or
    // blend shapes hang off the geometry in FBX).
    let mut shared: HashMap<usize, usize> = HashMap::new();
    for ni in 0..scene.nodes.len() {
        let node = &scene.nodes[ni];
        let Some(mi) = node.mesh else { continue };
        let mesh = &scene.meshes[mi];
        let own = node.skin.is_some() || mesh.target_count() > 0;
        let gi = if own {
            let weights = node.weights.clone().unwrap_or_else(|| {
                if mesh.weights.len() == mesh.target_count() {
                    mesh.weights.clone()
                } else {
                    vec![0.0; mesh.target_count()]
                }
            });
            w.geometry(mi, node.skin.is_some(), &weights)?
        } else if let Some(gi) = shared.get(&mi) {
            *gi
        } else {
            let gi = w.geometry(mi, false, &[])?;
            shared.insert(mi, gi);
            gi
        };
        w.geom_of_node[ni] = Some(gi);
        w.connect_geometry(ni, gi);
        w.skin(ni, gi)?;
    }
    w.bind_pose_object();
    let stacks = w.animations()?;
    let joints = scene.joint_nodes();
    let has_joints = joints.iter().any(|j| *j);
    let has_nulls = scene.nodes.iter().zip(&joints).any(|(n, j)| !j && n.mesh.is_none());
    let Writer { objects, conns, counts, .. } = w;
    Ok(Built { objects, conns, counts, stacks, has_joints, has_nulls })
}

// ─── document ───────────────────────────────────────────────────────────────

fn header_extension() -> WRec {
    let scene_info = rec("SceneInfo", vec![obj_name("GlobalInfo", "SceneInfo"), ws("UserData")])
        .child(leaf_str("Type", "UserData"))
        .child(leaf_i32("Version", 100))
        .child(
            WRec::new("MetaData")
                .child(leaf_i32("Version", 100))
                .child(leaf_str("Title", ""))
                .child(leaf_str("Subject", ""))
                .child(leaf_str("Author", ""))
                .child(leaf_str("Keywords", ""))
                .child(leaf_str("Revision", ""))
                .child(leaf_str("Comment", "")),
        )
        .child(props70(vec![
            p_str("DocumentUrl", "KString", "Url", "model.fbx"),
            p_str("SrcDocumentUrl", "KString", "Url", "model.fbx"),
            p("Original", "Compound", "", "", vec![]),
            p_str("Original|ApplicationVendor", "KString", "", "litegen"),
            p_str("Original|ApplicationName", "KString", "", CREATOR),
            p_str("Original|ApplicationVersion", "KString", "", "1"),
            p_str("Original|DateTime_GMT", "DateTime", "", "01/01/1970 10:00:00.000"),
            p_str("Original|FileName", "KString", "", "model.fbx"),
            p("LastSaved", "Compound", "", "", vec![]),
            p_str("LastSaved|ApplicationVendor", "KString", "", "litegen"),
            p_str("LastSaved|ApplicationName", "KString", "", CREATOR),
            p_str("LastSaved|ApplicationVersion", "KString", "", "1"),
            p_str("LastSaved|DateTime_GMT", "DateTime", "", "01/01/1970 10:00:00.000"),
        ]));
    WRec::new("FBXHeaderExtension")
        .child(leaf_i32("FBXHeaderVersion", 1003))
        .child(leaf_i32("FBXVersion", WRITE_VERSION as i32))
        .child(leaf_i32("EncryptionType", 0))
        .child(
            WRec::new("CreationTimeStamp")
                .child(leaf_i32("Version", 1000))
                .child(leaf_i32("Year", 1970))
                .child(leaf_i32("Month", 1))
                .child(leaf_i32("Day", 1))
                .child(leaf_i32("Hour", 10))
                .child(leaf_i32("Minute", 0))
                .child(leaf_i32("Second", 0))
                .child(leaf_i32("Millisecond", 0)),
        )
        .child(leaf_str("Creator", CREATOR))
        .child(scene_info)
}

/// Y-up, right-handed, +Z front, metres declared as 100 cm per unit: glTF's
/// frame, spelled as FBX's "Maya Y-up" preset.
fn global_settings(span: (i64, i64)) -> WRec {
    WRec::new("GlobalSettings").child(leaf_i32("Version", 1000)).child(props70(vec![
        p_int("UpAxis", "int", 1),
        p_int("UpAxisSign", "int", 1),
        p_int("FrontAxis", "int", 2),
        p_int("FrontAxisSign", "int", 1),
        p_int("CoordAxis", "int", 0),
        p_int("CoordAxisSign", "int", 1),
        p_int("OriginalUpAxis", "int", 1),
        p_int("OriginalUpAxisSign", "int", 1),
        p_double("UnitScaleFactor", 100.0),
        p_double("OriginalUnitScaleFactor", 100.0),
        p_vec3("AmbientColor", "ColorRGB", "Color", "", [0.0; 3]),
        p_str("DefaultCamera", "KString", "", "Producer Perspective"),
        p_int("TimeMode", "enum", 6),
        p_int("TimeProtocol", "enum", 2),
        p_int("SnapOnFrameMode", "enum", 0),
        p_ktime("TimeSpanStart", span.0),
        p_ktime("TimeSpanStop", span.1),
        p_double("CustomFrameRate", -1.0),
        p("TimeMarker", "Compound", "", "", vec![]),
        p_int("CurrentTimeMarker", "int", -1),
    ]))
}

/// A `PropertyTemplate`: the defaults an object of this class falls back to
/// for any property it does not write itself.
fn template(class: &str, ps: Vec<WRec>) -> WRec {
    rec("PropertyTemplate", vec![ws(class)]).child(props70(ps))
}

fn definitions(counts: &BTreeMap<&'static str, usize>, has_joints: bool, has_nulls: bool) -> WRec {
    let total: usize = counts.values().sum::<usize>() + 1;
    let mut d = WRec::new("Definitions").child(leaf_i32("Version", 100)).child(leaf_i32("Count", total as i32));
    d = d.child(rec("ObjectType", vec![ws("GlobalSettings")]).child(leaf_i32("Count", 1)));
    let order = [
        "Model",
        "NodeAttribute",
        "Geometry",
        "Material",
        "Texture",
        "Video",
        "Deformer",
        "Pose",
        "AnimationStack",
        "AnimationLayer",
        "AnimationCurveNode",
        "AnimationCurve",
    ];
    for kind in order {
        let Some(&n) = counts.get(kind) else { continue };
        let mut o = rec("ObjectType", vec![ws(kind)]).child(leaf_i32("Count", n as i32));
        match kind {
            "Model" => {
                o = o.child(template(
                    "FbxNode",
                    vec![
                        p_int("QuaternionInterpolate", "enum", 0),
                        p_vec3("RotationOffset", "Vector3D", "Vector", "", [0.0; 3]),
                        p_vec3("RotationPivot", "Vector3D", "Vector", "", [0.0; 3]),
                        p_vec3("ScalingOffset", "Vector3D", "Vector", "", [0.0; 3]),
                        p_vec3("ScalingPivot", "Vector3D", "Vector", "", [0.0; 3]),
                        p_int("RotationOrder", "enum", 0),
                        p_vec3("PreRotation", "Vector3D", "Vector", "", [0.0; 3]),
                        p_vec3("PostRotation", "Vector3D", "Vector", "", [0.0; 3]),
                        p_int("RotationActive", "bool", 0),
                        p_int("InheritType", "enum", 0),
                        p_vec3("GeometricTranslation", "Vector3D", "Vector", "", [0.0; 3]),
                        p_vec3("GeometricRotation", "Vector3D", "Vector", "", [0.0; 3]),
                        p_vec3("GeometricScaling", "Vector3D", "Vector", "", [1.0; 3]),
                        p_vec3("Lcl Translation", "Lcl Translation", "", "A", [0.0; 3]),
                        p_vec3("Lcl Rotation", "Lcl Rotation", "", "A", [0.0; 3]),
                        p_vec3("Lcl Scaling", "Lcl Scaling", "", "A", [1.0; 3]),
                        p("Visibility", "Visibility", "", "A", vec![WProp::F64(1.0)]),
                        p("Visibility Inheritance", "Visibility Inheritance", "", "", vec![WProp::I32(1)]),
                        p_int("DefaultAttributeIndex", "int", -1),
                    ],
                ))
            }
            "NodeAttribute" => {
                if has_joints {
                    o = o.child(template("FbxSkeleton", vec![p_double("Size", 100.0), p_double("LimbLength", 1.0)]));
                }
                if has_nulls {
                    o = o.child(template("FbxNull", vec![p_double("Size", 100.0)]));
                }
            }
            "Geometry" => {
                o = o.child(template(
                    "FbxMesh",
                    vec![
                        p_vec3("Color", "ColorRGB", "Color", "", [0.8; 3]),
                        p_vec3("BBoxMin", "Vector3D", "Vector", "", [0.0; 3]),
                        p_vec3("BBoxMax", "Vector3D", "Vector", "", [0.0; 3]),
                        p_int("Primary Visibility", "bool", 1),
                        p_int("Casts Shadows", "bool", 1),
                        p_int("Receive Shadows", "bool", 1),
                    ],
                ))
            }
            "Material" => {
                o = o.child(template(
                    "FbxSurfacePhong",
                    vec![
                        p_str("ShadingModel", "KString", "", "Phong"),
                        p_int("MultiLayer", "bool", 0),
                        p_color("EmissiveColor", [0.0; 3]),
                        p_num("EmissiveFactor", 1.0),
                        p_color("AmbientColor", [0.2; 3]),
                        p_num("AmbientFactor", 1.0),
                        p_color("DiffuseColor", [0.8; 3]),
                        p_num("DiffuseFactor", 1.0),
                        p_vec3("Bump", "Vector3D", "Vector", "", [0.0; 3]),
                        p_vec3("NormalMap", "Vector3D", "Vector", "", [0.0; 3]),
                        p_double("BumpFactor", 1.0),
                        p_color("TransparentColor", [0.0; 3]),
                        p_num("TransparencyFactor", 0.0),
                        p_color("SpecularColor", [0.2; 3]),
                        p_num("SpecularFactor", 1.0),
                        p_num("ShininessExponent", 20.0),
                        p_color("ReflectionColor", [0.0; 3]),
                        p_num("ReflectionFactor", 1.0),
                    ],
                ))
            }
            "Texture" => {
                o = o.child(template(
                    "FbxFileTexture",
                    vec![
                        p_int("TextureTypeUse", "enum", 0),
                        p("Texture alpha", "Number", "", "A", vec![WProp::F64(1.0)]),
                        p_int("CurrentMappingType", "enum", 0),
                        p_int("WrapModeU", "enum", 0),
                        p_int("WrapModeV", "enum", 0),
                        p_int("UVSwap", "bool", 0),
                        p_int("PremultiplyAlpha", "bool", 1),
                        p_vec3("Translation", "Vector", "", "A", [0.0; 3]),
                        p_vec3("Rotation", "Vector", "", "A", [0.0; 3]),
                        p_vec3("Scaling", "Vector", "", "A", [1.0; 3]),
                        p_int("CurrentTextureBlendMode", "enum", 1),
                        p_str("UVSet", "KString", "", "default"),
                        p_int("UseMaterial", "bool", 0),
                        p_int("UseMipMap", "bool", 0),
                    ],
                ))
            }
            "Video" => {
                o = o.child(template(
                    "FbxVideo",
                    vec![p_str("Path", "KString", "XRefUrl", ""), p_str("RelPath", "KString", "XRefUrl", ""), p_int("UseMipMap", "bool", 0)],
                ))
            }
            "AnimationStack" => {
                o = o.child(template(
                    "FbxAnimStack",
                    vec![
                        p_str("Description", "KString", "", ""),
                        p_ktime("LocalStart", 0),
                        p_ktime("LocalStop", 0),
                        p_ktime("ReferenceStart", 0),
                        p_ktime("ReferenceStop", 0),
                    ],
                ))
            }
            "AnimationLayer" => {
                o = o.child(template(
                    "FbxAnimLayer",
                    vec![
                        p_num("Weight", 100.0),
                        p_int("Mute", "bool", 0),
                        p_int("Solo", "bool", 0),
                        p_int("Lock", "bool", 0),
                        p_int("BlendMode", "enum", 0),
                    ],
                ))
            }
            "AnimationCurveNode" => o = o.child(template("FbxAnimCurveNode", vec![p("d", "Compound", "", "", vec![])])),
            _ => {}
        }
        d = d.child(o);
    }
    d
}

fn documents(active_stack: &str) -> WRec {
    WRec::new("Documents").child(leaf_i32("Count", 1)).child(
        rec("Document", vec![WProp::I64(FIRST_ID - 1), ws("Scene"), ws("Scene")])
            .child(props70(vec![
                p("SourceObject", "object", "", "", vec![]),
                p_str("ActiveAnimStackName", "KString", "", active_stack),
            ]))
            .child(rec("RootNode", vec![WProp::I64(0)])),
    )
}

fn takes(stacks: &[(String, i64, i64)]) -> WRec {
    let mut t = WRec::new("Takes").child(leaf_str("Current", stacks.first().map_or("", |s| s.0.as_str())));
    for (name, t0, t1) in stacks {
        let file = format!("{}.tak", super::package::safe_file_stem(name, "take"));
        t = t.child(
            rec("Take", vec![ws(name)])
                .child(leaf_str("FileName", &file))
                .child(rec("LocalTime", vec![WProp::I64(*t0), WProp::I64(*t1)]))
                .child(rec("ReferenceTime", vec![WProp::I64(*t0), WProp::I64(*t1)])),
        );
    }
    // `Takes` is never childless here when there are stacks; with none it is
    // the SDK's lone `Current: ""`.
    t
}

fn serialize(b: Built) -> Result<Vec<u8>, MeshError> {
    let span = b.stacks.iter().fold(None, |acc: Option<(i64, i64)>, (_, t0, t1)| {
        Some(acc.map_or((*t0, *t1), |(a, z)| (a.min(*t0), z.max(*t1))))
    });
    let active = b.stacks.first().map_or(String::new(), |s| s.0.clone());
    let mut objects = WRec::new("Objects");
    objects.children = b.objects;
    objects.force_null = true;
    let mut connections = WRec::new("Connections");
    connections.children = b.conns;
    connections.force_null = true;
    let top = vec![
        header_extension(),
        rec("FileId", vec![WProp::Raw(FILE_ID.to_vec())]),
        leaf_str("CreationTime", CREATION_TIME),
        leaf_str("Creator", CREATOR),
        global_settings(span.unwrap_or((0, 0))),
        documents(&active),
        WRec { force_null: true, ..WRec::new("References") },
        definitions(&b.counts, b.has_joints, b.has_nulls),
        objects,
        connections,
        takes(&b.stacks),
    ];
    let mut out = BINARY_MAGIC.to_vec();
    out.extend_from_slice(&[0x1A, 0x00]);
    out.extend_from_slice(&WRITE_VERSION.to_le_bytes());
    write_records(&top, &mut out)?;
    // Footer, laid out as the SDK does: id, 1–16 bytes of zero padding to a
    // 16-byte boundary, four zero bytes, the version, 120 zero bytes, magic.
    out.extend_from_slice(&FOOTER_ID);
    let pad = 16 - out.len() % 16;
    out.extend(std::iter::repeat_n(0u8, pad));
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&WRITE_VERSION.to_le_bytes());
    out.extend(std::iter::repeat_n(0u8, 120));
    out.extend_from_slice(&FOOTER_MAGIC);
    Ok(out)
}

/// Write a scene as binary FBX 7400.
pub(super) fn write(scene: &Scene) -> Result<Vec<u8>, MeshError> {
    write_with(scene, true)
}

fn write_with(scene: &Scene, embed: bool) -> Result<Vec<u8>, MeshError> {
    scene.validate()?;
    if scene.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    for (i, n) in scene.nodes.iter().enumerate() {
        if matches!(n.transform, Transform::Matrix(_)) && n.transform.trs().is_none() {
            return Err(unsupported(format!(
                "node {i} ({:?}) has a sheared transform; FBX transforms are translation, rotation and scale",
                n.name.as_deref().unwrap_or("")
            )));
        }
    }
    let hoisted = super::glb::hoist_skinned_meshes(scene);
    serialize(build(&hoisted, embed)?)
}

#[cfg(test)]
pub(super) mod tests_support {
    use super::*;

    /// The scene written without embedded image bytes, as a DCC writes a
    /// file that references its textures.
    pub(in super::super) fn strip_video_content(scene: &Scene) -> Result<Vec<u8>, MeshError> {
        let p = super::super::fbx::prepare(scene, super::super::WriteOptions::default())?;
        write_with(&p, false)
    }

    /// A one-triangle ASCII FBX for hostile-input tests.
    pub(in super::super) fn ascii_cube() -> String {
        r#"; FBX 7.4.0 project file
FBXHeaderExtension:  {
	FBXHeaderVersion: 1003
	FBXVersion: 7400
}
GlobalSettings:  {
	Version: 1000
	Properties70:  {
		P: "UnitScaleFactor", "double", "Number", "",100
	}
}
Objects:  {
	Model: 10, "Model::tri", "Mesh" {
		Version: 232
		Properties70:  {
			P: "Lcl Translation", "Lcl Translation", "", "A",1,2,3
		}
	}
	Geometry: 20, "Geometry::tri", "Mesh" {
		Vertices: *9 {
			a: 0,0,0,1,0,0,0,1,0
		}
		PolygonVertexIndex: *3 {
			a: 0,1,-3
		}
		LayerElementNormal: 0 {
			Version: 101
			Name: ""
			MappingInformationType: "ByPolygonVertex"
			ReferenceInformationType: "Direct"
			Normals: *9 {
				a: 0,0,1,0,0,1,0,0,1
			}
		}
		LayerElementUV: 0 {
			Version: 101
			Name: "map1"
			MappingInformationType: "ByPolygonVertex"
			ReferenceInformationType: "IndexToDirect"
			UV: *6 {
				a: 0,0,1,0,0,1
			}
			UVIndex: *3 {
				a: 0,1,2
			}
		}
	}
}
Connections:  {
	C: "OO",10,0
	C: "OO",20,10
}
"#
        .to_string()
    }
}
