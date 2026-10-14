//! Evaluating a scene: animation sampling, morphing, skinning and baking.
//!
//! Every container that cannot hold a skeleton or an animation (OBJ, PLY, STL,
//! 3MF) receives the model *as a viewer would show it* with no clip playing:
//! morph targets at their default weights, skinned geometry deformed by its
//! joints' current transforms, every node's transform baked into the vertices.
//! That is one evaluation, done here, so four writers cannot each get a slightly
//! different answer.
//!
//! The same evaluator samples animation at arbitrary times, which the writers
//! that must resample (FBX stores Euler curves, Collada matrices) use to bake
//! keys, and which the tests use to compare a converted animation against its
//! source at times that fall BETWEEN keyframes — the only times where an
//! interpolation mistake is visible at all.

use super::ir::Mesh;
use super::math::{self, Mat4};
use super::scene::{
    AnimPath, Channel, Interpolation, Node, Primitive, Scene, SceneMesh, Transform, MAX_SCENE_INDICES,
    MAX_SCENE_VERTICES,
};
use super::MeshError;

/// Per-node local transforms and morph weights at one instant.
#[derive(Debug, Clone)]
pub struct Pose {
    pub trs: Vec<Option<([f64; 3], [f64; 4], [f64; 3])>>,
    /// Matrices for nodes whose transform has no TRS form (shear) and which no
    /// channel animates.
    pub matrices: Vec<Mat4>,
    /// Effective morph weights per node (empty when the node has no targets).
    pub weights: Vec<Vec<f32>>,
}

impl Pose {
    pub fn locals(&self) -> Vec<Mat4> {
        self.trs
            .iter()
            .zip(&self.matrices)
            .map(|(trs, m)| match trs {
                Some((t, r, s)) => math::compose(*t, *r, *s),
                None => *m,
            })
            .collect()
    }
}

/// The scene as it stands, no clip playing.
pub fn default_pose(scene: &Scene) -> Pose {
    let trs: Vec<_> = scene.nodes.iter().map(|n| n.transform.trs()).collect();
    let matrices: Vec<Mat4> = scene.nodes.iter().map(|n| n.transform.matrix()).collect();
    let weights = scene
        .nodes
        .iter()
        .map(|n| match (&n.weights, n.mesh) {
            (Some(w), _) => w.clone(),
            (None, Some(m)) => default_weights(&scene.meshes[m]),
            (None, None) => Vec::new(),
        })
        .collect();
    Pose { trs, matrices, weights }
}

fn default_weights(mesh: &SceneMesh) -> Vec<f32> {
    let n = mesh.target_count();
    if mesh.weights.len() == n {
        mesh.weights.clone()
    } else {
        vec![0.0; n]
    }
}

/// The pose at time `t` of animation `anim`, starting from the default pose so
/// that nodes and paths the clip does not touch keep their own values.
pub fn sample_pose(scene: &Scene, anim: usize, t: f32) -> Pose {
    let mut pose = default_pose(scene);
    let Some(a) = scene.animations.get(anim) else { return pose };
    for ch in &a.channels {
        let Some(node) = scene.nodes.get(ch.node) else { continue };
        let targets = node.mesh.map_or(0, |m| scene.meshes[m].target_count());
        let v = sample_channel(ch, t, ch.stride(targets));
        // A sheared node has no TRS for a channel to write into; glTF forbids
        // animating a `matrix` node outright, so this decomposes what it can
        // and leaves the rest alone.
        let base = pose.trs[ch.node].unwrap_or(([0.0; 3], math::QUAT_IDENTITY, [1.0; 3]));
        let (mut tr, mut rot, mut sc) = base;
        match ch.path {
            AnimPath::Translation => tr = [v[0] as f64, v[1] as f64, v[2] as f64],
            AnimPath::Rotation => {
                rot = math::quat_normalize([v[0] as f64, v[1] as f64, v[2] as f64, v[3] as f64])
            }
            AnimPath::Scale => sc = [v[0] as f64, v[1] as f64, v[2] as f64],
            AnimPath::Weights => {
                pose.weights[ch.node] = v;
                continue;
            }
        }
        pose.trs[ch.node] = Some((tr, rot, sc));
    }
    pose
}

/// One channel's value at `t`. Before the first key it holds the first value,
/// after the last the last: glTF's (and every DCC's) clamp behaviour.
pub fn sample_channel(ch: &Channel, t: f32, stride: usize) -> Vec<f32> {
    let n = ch.times.len();
    let cubic = ch.interpolation == Interpolation::CubicSpline;
    // Where the VALUE of key k lives: cubic keys are [in, value, out].
    let value_at = |k: usize| -> &[f32] {
        let base = if cubic { k * stride * 3 + stride } else { k * stride };
        &ch.values[base..base + stride]
    };
    if n == 0 || stride == 0 {
        return vec![0.0; stride];
    }
    if t <= ch.times[0] || n == 1 {
        return value_at(0).to_vec();
    }
    if t >= ch.times[n - 1] {
        return value_at(n - 1).to_vec();
    }
    // Last key at or before t.
    let k = match ch.times.binary_search_by(|x| x.partial_cmp(&t).unwrap_or(std::cmp::Ordering::Less)) {
        Ok(i) => return value_at(i).to_vec(),
        Err(i) => i - 1,
    };
    let (t0, t1) = (ch.times[k], ch.times[k + 1]);
    let dt = t1 - t0;
    let u = if dt > 0.0 { (t - t0) / dt } else { 0.0 };
    let a = value_at(k);
    let b = value_at(k + 1);
    let mut out = match ch.interpolation {
        Interpolation::Step => a.to_vec(),
        Interpolation::Linear if ch.path == AnimPath::Rotation => {
            let q = math::slerp(
                [a[0] as f64, a[1] as f64, a[2] as f64, a[3] as f64],
                [b[0] as f64, b[1] as f64, b[2] as f64, b[3] as f64],
                u as f64,
            );
            q.iter().map(|v| *v as f32).collect()
        }
        Interpolation::Linear => a.iter().zip(b).map(|(x, y)| x + (y - x) * u).collect(),
        Interpolation::CubicSpline => {
            // glTF Appendix C: tangents are scaled by the key interval.
            let out_tan = &ch.values[k * stride * 3 + 2 * stride..k * stride * 3 + 3 * stride];
            let in_tan = &ch.values[(k + 1) * stride * 3..(k + 1) * stride * 3 + stride];
            let (u2, u3) = (u * u, u * u * u);
            let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
            let h10 = u3 - 2.0 * u2 + u;
            let h01 = -2.0 * u3 + 3.0 * u2;
            let h11 = u3 - u2;
            (0..stride)
                .map(|i| h00 * a[i] + h10 * dt * out_tan[i] + h01 * b[i] + h11 * dt * in_tan[i])
                .collect()
        }
    };
    if ch.path == AnimPath::Rotation && ch.interpolation != Interpolation::Linear {
        let q = math::quat_normalize([out[0] as f64, out[1] as f64, out[2] as f64, out[3] as f64]);
        out = q.iter().map(|v| *v as f32).collect();
    }
    out
}

/// A primitive evaluated into world space: morphs applied, skin applied, node
/// transform baked, winding corrected for mirrors. What every static writer
/// consumes.
#[derive(Debug, Clone)]
pub struct WorldPrimitive {
    pub node: usize,
    pub mesh: usize,
    pub primitive: usize,
    pub name: Option<String>,
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub uvs: Vec<Vec<[f32; 2]>>,
    pub colors: Option<Vec<[f32; 4]>>,
    pub indices: Vec<u32>,
    pub material: Option<usize>,
}

/// Every drawn primitive in world space, in node traversal order.
pub fn world_primitives(scene: &Scene, pose: &Pose) -> Result<Vec<WorldPrimitive>, MeshError> {
    let locals = pose.locals();
    let world = super::scene::world_from_locals(scene, &locals);
    let mut out = Vec::new();
    let (mut total_vertices, mut total_indices) = (0usize, 0usize);
    for node_index in scene.topological_order() {
        let node: &Node = &scene.nodes[node_index];
        let Some(mi) = node.mesh else { continue };
        let mesh = &scene.meshes[mi];
        // Instancing is where a small file multiplies: one mesh reached from a
        // thousand nodes reads once and bakes a thousand times. The ceiling is
        // checked before the copies are made, because a Vec sized past it is
        // one the allocator may answer by aborting the process.
        total_vertices = total_vertices.saturating_add(mesh.primitives.iter().map(|p| p.positions.len()).sum());
        total_indices = total_indices.saturating_add(mesh.primitives.iter().map(|p| p.indices.len()).sum());
        if total_vertices > MAX_SCENE_VERTICES {
            return Err(MeshError::Unsupported(format!(
                "the merged mesh passes {MAX_SCENE_VERTICES} vertices at node {node_index}; \
                 the node graph instances more geometry than this converter will hold"
            )));
        }
        if total_indices > MAX_SCENE_INDICES {
            return Err(MeshError::Unsupported(format!(
                "the merged mesh passes {MAX_SCENE_INDICES} indices at node {node_index}"
            )));
        }
        let weights = pose.weights.get(node_index).cloned().unwrap_or_default();
        // Joint matrices, when skinned: world(joint) * inverseBind(joint). The
        // holding node's own transform is deliberately absent — glTF's rule.
        let joint_mats: Option<Vec<Mat4>> = node.skin.map(|s| {
            let skin = &scene.skins[s];
            skin.joints
                .iter()
                .zip(&skin.inverse_bind_matrices)
                .map(|(&j, ibm)| math::mat_mul(&world[j], ibm))
                .collect()
        });
        for (pi, prim) in mesh.primitives.iter().enumerate() {
            let (mut positions, mut normals) = morphed(prim, &weights);
            let mut indices = prim.indices.clone();
            match (&joint_mats, &prim.skin) {
                (Some(mats), Some(sw)) => {
                    skin_in_place(&mut positions, normals.as_mut(), sw, mats);
                    // A skinned primitive's mirroring is per vertex in theory;
                    // in practice it comes from the joints' common ancestry, and
                    // the first joint's sign is the one every vertex shares.
                    if mats.first().is_some_and(math::is_mirroring) {
                        flip_winding(&mut indices);
                    }
                }
                _ => {
                    let m = &world[node_index];
                    let nm = math::normal_matrix(m);
                    for p in positions.iter_mut() {
                        *p = math::transform_point(m, *p);
                    }
                    if let Some(ns) = normals.as_mut() {
                        for n in ns.iter_mut() {
                            *n = math::transform_normal(&nm, *n);
                        }
                    }
                    if math::is_mirroring(m) {
                        flip_winding(&mut indices);
                    }
                }
            }
            out.push(WorldPrimitive {
                node: node_index,
                mesh: mi,
                primitive: pi,
                name: mesh.name.clone().or_else(|| node.name.clone()),
                positions,
                normals,
                uvs: prim.uvs.clone(),
                colors: prim.colors.clone(),
                indices,
                material: prim.material,
            });
        }
    }
    Ok(out)
}

fn flip_winding(indices: &mut [u32]) {
    for t in indices.chunks_exact_mut(3) {
        t.swap(1, 2);
    }
}

/// Positions and normals with morph targets applied at `weights`.
pub fn morphed(prim: &Primitive, weights: &[f32]) -> (Vec<[f32; 3]>, Option<Vec<[f32; 3]>>) {
    let mut positions = prim.positions.clone();
    let mut normals = prim.normals.clone();
    for (k, target) in prim.targets.iter().enumerate() {
        let w = weights.get(k).copied().unwrap_or(0.0);
        if w == 0.0 {
            continue;
        }
        for (p, d) in positions.iter_mut().zip(&target.positions) {
            for a in 0..3 {
                p[a] += w * d[a];
            }
        }
        if let (Some(ns), Some(dn)) = (normals.as_mut(), &target.normals) {
            for (n, d) in ns.iter_mut().zip(dn) {
                for a in 0..3 {
                    n[a] += w * d[a];
                }
            }
        }
    }
    if prim.targets.iter().any(|t| t.normals.is_some()) {
        if let Some(ns) = normals.as_mut() {
            for n in ns.iter_mut() {
                let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                if l > 0.0 && l.is_finite() {
                    *n = [n[0] / l, n[1] / l, n[2] / l];
                }
            }
        }
    }
    (positions, normals)
}

/// Linear-blend skinning in place. Weights are normalised per vertex, as
/// three.js's loaders do on import; an all-zero vertex is bound wholly to its
/// first listed joint, which is three.js's behaviour too.
pub fn skin_in_place(
    positions: &mut [[f32; 3]],
    normals: Option<&mut Vec<[f32; 3]>>,
    sw: &super::scene::SkinWeights,
    joint_mats: &[Mat4],
) {
    let blend = |v: usize| -> Mat4 {
        let (js, ws) = sw.vertex(v);
        let sum: f32 = ws.iter().sum();
        let mut m = [0.0f64; 16];
        if sum <= 0.0 || !sum.is_finite() {
            return joint_mats.get(js[0] as usize).copied().unwrap_or(math::IDENTITY);
        }
        for (j, w) in js.iter().zip(ws) {
            if *w == 0.0 {
                continue;
            }
            let Some(jm) = joint_mats.get(*j as usize) else { continue };
            let w = (*w / sum) as f64;
            for (o, x) in m.iter_mut().zip(jm) {
                *o += w * x;
            }
        }
        m
    };
    let mut normals = normals;
    for v in 0..positions.len() {
        let m = blend(v);
        positions[v] = math::transform_point(&m, positions[v]);
        if let Some(ns) = normals.as_deref_mut() {
            ns[v] = math::transform_normal(&math::normal_matrix(&m), ns[v]);
        }
    }
}

/// The scene posed and baked: one root node per drawn primitive, geometry in
/// world space, no skins, no morph targets, no animation. Materials, textures
/// and images are kept as they are. This is the scene a static container
/// receives.
pub fn bake(scene: &Scene, pose: &Pose) -> Result<Scene, MeshError> {
    let prims = world_primitives(scene, pose)?;
    let mut out = Scene {
        name: scene.name.clone(),
        materials: scene.materials.clone(),
        textures: scene.textures.clone(),
        images: scene.images.clone(),
        ..Default::default()
    };
    for wp in prims {
        out.meshes.push(SceneMesh {
            name: wp.name.clone(),
            primitives: vec![Primitive {
                positions: wp.positions,
                normals: wp.normals,
                tangents: None,
                uvs: wp.uvs,
                colors: wp.colors,
                skin: None,
                indices: wp.indices,
                material: wp.material,
                targets: Vec::new(),
            }],
            weights: Vec::new(),
            target_names: Vec::new(),
        });
        let mesh = out.meshes.len() - 1;
        out.add_root(Node {
            name: scene.nodes[wp.node].name.clone(),
            transform: Transform::default(),
            mesh: Some(mesh),
            ..Default::default()
        });
    }
    Ok(out)
}

/// The default-pose bake, which is what "as a viewer shows it" means.
pub fn bake_default(scene: &Scene) -> Result<Scene, MeshError> {
    bake(scene, &default_pose(scene))
}

/// The whole scene as ONE world-space triangle mesh: the flat view.
///
/// Semantics are those the GLB reader has always had, so every caller of the
/// flat API sees the same mesh it did before the scene existed:
///
/// - primitives are merged in node traversal order;
/// - when some primitives carry normals and others do not, the gaps are filled
///   with area-weighted normals derived from the geometry, keeping every normal
///   the file actually shipped;
/// - when some carry UV set 0 and others do not, the gaps are (0, 0): UVs are
///   inert without a texture, and dropping real ones over a sibling's absence
///   would lose data for nothing;
/// - the base colour is the first one a primitive's material actually sets;
/// - the name is the first non-empty mesh name.
pub fn flatten(scene: &Scene) -> Result<Mesh, MeshError> {
    let prims = world_primitives(scene, &default_pose(scene))?;
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let any_normals = prims.iter().any(|p| p.normals.is_some());
    let any_uvs = prims.iter().any(|p| !p.uvs.is_empty());
    let mut base_color = None;
    let mut name = None;
    for wp in &prims {
        let base = u32::try_from(positions.len())
            .map_err(|_| MeshError::Unsupported("merged mesh exceeds 2^32 vertices".into()))?;
        if any_normals {
            match &wp.normals {
                Some(n) => normals.extend_from_slice(n),
                None => normals.extend(derived_normals(&wp.positions, &wp.indices)),
            }
        }
        if any_uvs {
            match wp.uvs.first() {
                Some(set) => uvs.extend_from_slice(set),
                None => uvs.extend(std::iter::repeat_n([0.0f32; 2], wp.positions.len())),
            }
        }
        positions.extend_from_slice(&wp.positions);
        indices.extend(wp.indices.iter().map(|i| i + base));
        if base_color.is_none() {
            base_color = wp.material.and_then(|m| scene.materials.get(m)).and_then(|m| {
                // White is glTF's default and therefore indistinguishable from
                // "no colour set"; the flat view has always reported the
                // first colour a file actually chose.
                (m.base_color != [1.0; 4]).then_some(m.base_color)
            });
        }
        if name.is_none() {
            name = scene.meshes[wp.mesh].name.clone().filter(|s| !s.is_empty());
        }
    }
    Ok(Mesh {
        positions,
        indices,
        normals: any_normals.then_some(normals),
        uvs: any_uvs.then_some(uvs),
        base_color,
        name,
    })
}

/// Area-weighted vertex normals.
pub fn derived_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut acc = vec![[0.0f64; 3]; positions.len()];
    for t in indices.chunks_exact(3) {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let (Some(pa), Some(pb), Some(pc)) = (positions.get(a), positions.get(b), positions.get(c)) else {
            continue;
        };
        let u = [(pb[0] - pa[0]) as f64, (pb[1] - pa[1]) as f64, (pb[2] - pa[2]) as f64];
        let w = [(pc[0] - pa[0]) as f64, (pc[1] - pa[1]) as f64, (pc[2] - pa[2]) as f64];
        let n = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
        for v in [a, b, c] {
            for k in 0..3 {
                acc[v][k] += n[k];
            }
        }
    }
    acc.into_iter()
        .map(|n| {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len > 0.0 && len.is_finite() {
                [(n[0] / len) as f32, (n[1] / len) as f32, (n[2] / len) as f32]
            } else {
                [0.0, 0.0, 1.0]
            }
        })
        .collect()
}

/// The flat mesh as a one-node scene, so flat-API callers reach every writer.
pub fn from_flat(mesh: &Mesh) -> Scene {
    let mut scene = Scene::default();
    let material = mesh.base_color.map(|c| {
        scene.materials.push(super::scene::Material {
            base_color: c,
            // Fully rough, non-metallic: the flat mesh carries a colour and
            // nothing else, and these are the values that show it as itself.
            metallic: 0.0,
            roughness: 1.0,
            ..Default::default()
        });
        0
    });
    scene.meshes.push(SceneMesh {
        name: mesh.name.clone(),
        primitives: vec![Primitive {
            positions: mesh.positions.clone(),
            normals: mesh.normals.clone(),
            uvs: mesh.uvs.clone().into_iter().collect(),
            indices: mesh.indices.clone(),
            material,
            ..Default::default()
        }],
        ..Default::default()
    });
    scene.add_root(Node { name: mesh.name.clone(), mesh: Some(0), ..Default::default() });
    scene
}

/// Zero scales as a millionth (`math::nonsingular_scale`), on every node and,
/// with `keys`, in every scale key: what writers whose output readers
/// decompose (Collada, FBX) encode, so `prepare` says exactly that.
pub fn nonsingular_scales(scene: &mut Scene, keys: bool) {
    for n in scene.nodes.iter_mut() {
        if let Transform::Trs { scale, .. } = &mut n.transform {
            *scale = math::nonsingular_scale(*scale);
        }
    }
    if keys {
        for ch in scene.animations.iter_mut().flat_map(|a| a.channels.iter_mut()).filter(|c| c.path == AnimPath::Scale) {
            for v in ch.values.chunks_exact_mut(3) {
                let s = math::nonsingular_scale([v[0] as f64, v[1] as f64, v[2] as f64]);
                for k in 0..3 {
                    v[k] = s[k] as f32;
                }
            }
        }
    }
}

/// Remove transform channels that never move their node from its default:
/// every key equal to the node's own translation, rotation or scale. They
/// change no pose, and they are not harmless everywhere. Baked Collada
/// matrices come back as all three channels for every animated node, and
/// SceneKit bakes a USD transform with several animated ops into one matrix
/// sampled at about 120 Hz, which rounds off a step the scene keeps sharp
/// (measured: Khronos InterpolationTest's step scale through Collada). A clip
/// that moves nothing at all keeps one channel, so it keeps its name and
/// length.
pub fn drop_constant_channels(scene: &mut Scene) {
    let Scene { nodes, animations, .. } = scene;
    let close = |a: f32, b: f64| (a as f64 - b).abs() <= 1e-6 * b.abs().max(1.0);
    for a in animations.iter_mut() {
        let still = |ch: &Channel| -> bool {
            let Some((t, r, s)) = nodes.get(ch.node).and_then(|n| n.transform.trs()) else { return false };
            match ch.path {
                AnimPath::Translation => ch.values.chunks_exact(3).all(|v| (0..3).all(|k| close(v[k], t[k]))),
                AnimPath::Scale => ch.values.chunks_exact(3).all(|v| (0..3).all(|k| close(v[k], s[k]))),
                AnimPath::Rotation => ch.values.chunks_exact(4).all(|v| {
                    let q = [v[0] as f64, v[1] as f64, v[2] as f64, v[3] as f64];
                    math::quat_dot(math::quat_normalize(q), r).abs() >= 1.0 - 1e-12
                }),
                AnimPath::Weights => false,
            }
        };
        if a.channels.iter().all(still) {
            a.channels.truncate(1);
        } else {
            a.channels.retain(|ch| !still(ch));
        }
    }
}

/// Key times to resample an animation at: every source key time, plus evenly
/// spaced samples at `fps` so that the interpolation a target applies between
/// keys (linear Euler, linear matrix) cannot drift far from the source's
/// (slerp, cubic). Capped so a long clip cannot become millions of keys.
///
/// A clip whose first key comes after 0 is also keyed AT 0, holding its first
/// pose: playback starts at 0, and glTF holds the first key until it comes.
/// Without that key, readers that start a clip at its first key (SceneKit's
/// Collada and USD importers) play its opening early: CesiumMan, whose keys
/// begin at 1/24 s, drew 1/24 s ahead at the start.
pub fn resample_times(scene: &Scene, anim: usize, fps: f32) -> Vec<f32> {
    let Some(a) = scene.animations.get(anim) else { return Vec::new() };
    let Some((lo, hi)) = a.time_range() else { return Vec::new() };
    let mut times: Vec<f32> = a.channels.iter().flat_map(|c| c.times.iter().copied()).collect();
    if lo > 0.0 {
        times.push(0.0);
    }
    if hi > lo && fps > 0.0 {
        let steps = (((hi - lo) * fps).ceil() as usize).min(100_000);
        for i in 0..=steps {
            times.push(lo + (hi - lo) * (i as f32 / steps.max(1) as f32));
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Merge keys closer than a microsecond: they are the same instant written
    // through two different float paths, and two keys at one time are a
    // validation error in every target.
    times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    times
}

/// All drawn geometry merged into ONE primitive in world space, the way a
/// single-mesh container holds it.
///
/// Attributes follow the flat view's rules (`flatten`): normals are derived
/// where a primitive lacks them, UV set 0 is zero-filled, and the merged
/// primitive takes the first primitive's material. Vertex colours, which the
/// flat view never carried, are white-filled for primitives that lack them.
pub fn merge_all(scene: &Scene) -> Result<Scene, MeshError> {
    let prims = world_primitives(scene, &default_pose(scene))?;
    let any_normals = prims.iter().any(|p| p.normals.is_some());
    let any_uvs = prims.iter().any(|p| !p.uvs.is_empty());
    let any_colors = prims.iter().any(|p| p.colors.is_some());
    let mut out = Primitive::default();
    let (mut normals, mut uvs, mut colors) = (Vec::new(), Vec::new(), Vec::new());
    for wp in &prims {
        let base = out.positions.len() as u32;
        if any_normals {
            match &wp.normals {
                Some(n) => normals.extend_from_slice(n),
                None => normals.extend(derived_normals(&wp.positions, &wp.indices)),
            }
        }
        if any_uvs {
            match wp.uvs.first() {
                Some(set) => uvs.extend_from_slice(set),
                None => uvs.extend(std::iter::repeat_n([0.0f32; 2], wp.positions.len())),
            }
        }
        if any_colors {
            match &wp.colors {
                Some(c) => colors.extend_from_slice(c),
                None => colors.extend(std::iter::repeat_n([1.0f32; 4], wp.positions.len())),
            }
        }
        out.positions.extend_from_slice(&wp.positions);
        out.indices.extend(wp.indices.iter().map(|i| i + base));
        if out.material.is_none() {
            out.material = wp.material;
        }
    }
    out.normals = any_normals.then_some(normals);
    if any_uvs {
        out.uvs = vec![uvs];
    }
    out.colors = any_colors.then_some(colors);
    let mut s = Scene {
        name: scene.name.clone(),
        materials: scene.materials.clone(),
        textures: scene.textures.clone(),
        images: scene.images.clone(),
        ..Default::default()
    };
    let name = prims.iter().find_map(|wp| scene.meshes[wp.mesh].name.clone());
    s.meshes.push(SceneMesh { name: name.clone(), primitives: vec![out], ..Default::default() });
    s.add_root(Node { name, mesh: Some(0), ..Default::default() });
    Ok(s)
}

/// The scene reduced to what a container with `caps` can hold.
///
/// This is the lossiness table as code: every writer is handed (or behaves as
/// if handed) exactly this, and the render verification harness renders it as
/// the EXPECTED picture for a conversion into such a container. A writer that
/// keeps more than this would be inventing data the target cannot honestly
/// carry; one that keeps less is losing data it should not.
pub fn degrade(scene: &Scene, caps: &super::scene::Capabilities) -> Result<Scene, MeshError> {
    use super::scene::MaterialSupport;
    let mut s = if !caps.skinning || !caps.morph_targets || !caps.hierarchy {
        bake_default(scene)?
    } else {
        scene.clone()
    };
    if !caps.animation {
        s.animations.clear();
    }
    if caps.materials == MaterialSupport::VertexColor && caps.vertex_colors && !caps.multiple_meshes {
        // Per-primitive colours must be baked before the merge keeps only one
        // material, or every part takes the first part's colour.
        let materials = s.materials.clone();
        let any_color = s.meshes.iter().flat_map(|m| &m.primitives).any(|p| {
            p.colors.is_some() || p.material.and_then(|m| materials.get(m)).is_some_and(|m| m.base_color != [1.0; 4])
        });
        if any_color {
            for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
                let base = p.material.and_then(|m| materials.get(m)).map_or([1.0; 4], |m| m.base_color);
                let cs = p.colors.get_or_insert_with(|| vec![[1.0; 4]; p.positions.len()]);
                for c in cs.iter_mut() {
                    for k in 0..4 {
                        c[k] *= base[k];
                    }
                }
                p.material = None;
            }
            s.materials.clear();
        }
    }
    if !caps.multiple_meshes {
        s = merge_all(&s)?;
    }
    for mesh in s.meshes.iter_mut() {
        for p in mesh.primitives.iter_mut() {
            if !caps.normals {
                p.normals = None;
                for t in p.targets.iter_mut() {
                    t.normals = None;
                }
            }
            p.uvs.truncate(caps.uv_sets);
            if !caps.vertex_colors {
                p.colors = None;
            }
            if !caps.textures || p.uvs.is_empty() {
                p.tangents = None;
            }
        }
    }
    let uv_sets = caps.uv_sets as u32;
    let keep = |r: &Option<super::scene::TextureRef>| -> Option<super::scene::TextureRef> {
        r.as_ref().filter(|r| caps.textures && r.uv_set < uv_sets).cloned()
    };
    match caps.materials {
        MaterialSupport::None => {
            s.materials.clear();
            for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
                p.material = None;
            }
        }
        MaterialSupport::VertexColor => {
            // The base colour becomes vertex colour, multiplied into whatever
            // vertex colour the primitive already had. Textures cannot follow:
            // sampling an image per vertex would need an image decoder, and a
            // texture's detail lives between vertices anyway.
            let materials = std::mem::take(&mut s.materials);
            for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
                let base = p.material.and_then(|m| materials.get(m)).map(|m| m.base_color);
                p.material = None;
                if !caps.vertex_colors {
                    p.colors = None;
                    continue;
                }
                match (p.colors.as_mut(), base) {
                    (Some(cs), Some(b)) => {
                        for c in cs.iter_mut() {
                            for k in 0..4 {
                                c[k] *= b[k];
                            }
                        }
                    }
                    (None, Some(b)) if b != [1.0; 4] => p.colors = Some(vec![b; p.positions.len()]),
                    _ => {}
                }
            }
        }
        MaterialSupport::ColorOnly => {
            for m in s.materials.iter_mut() {
                *m = super::scene::Material {
                    name: m.name.clone(),
                    base_color: m.base_color,
                    base_color_texture: keep(&m.base_color_texture),
                    metallic: 0.0,
                    roughness: 1.0,
                    double_sided: m.double_sided,
                    ..Default::default()
                };
            }
        }
        MaterialSupport::Phong => {
            for m in s.materials.iter_mut() {
                m.base_color_texture = keep(&m.base_color_texture);
                m.normal_texture = keep(&m.normal_texture);
                m.emissive_texture = keep(&m.emissive_texture);
                m.metallic_roughness_texture = None;
                m.occlusion_texture = None;
            }
        }
        MaterialSupport::Pbr => {
            for m in s.materials.iter_mut() {
                m.base_color_texture = keep(&m.base_color_texture);
                m.metallic_roughness_texture = keep(&m.metallic_roughness_texture);
                m.normal_texture = keep(&m.normal_texture);
                m.occlusion_texture = keep(&m.occlusion_texture);
                m.emissive_texture = keep(&m.emissive_texture);
            }
        }
    }
    if caps.textures && !caps.texture_transforms {
        bake_texture_transforms(&mut s);
    }
    if !caps.texture_wrap {
        for t in s.textures.iter_mut() {
            t.sampler.wrap_s = super::scene::Wrap::Repeat;
            t.sampler.wrap_t = super::scene::Wrap::Repeat;
        }
    }
    if !caps.texture_filters {
        for t in s.textures.iter_mut() {
            t.sampler.mag_filter = None;
            t.sampler.min_filter = None;
            t.sampler.mipmaps = true;
            t.sampler.mipmap_filter = super::scene::Filter::Linear;
        }
    }
    prune_unused_textures(&mut s);
    s.validate()?;
    Ok(s)
}

/// Move every `KHR_texture_transform` into the texture coordinates it
/// transforms, for containers that cannot express one.
///
/// Sampling `T(uv)` from untransformed coordinates and sampling `uv` through
/// the transform are the same picture, so this is lossless whenever a UV set
/// feeds textures under one transform — what every exporter produces. When a
/// material applies two DIFFERENT transforms to one set, only the first in
/// priority order (base colour, emissive, normal, metallic-roughness,
/// occlusion) can be baked, and the others are left untransformed: a loss the
/// caller sees through `scene::losses`.
pub fn bake_texture_transforms(s: &mut Scene) {
    use super::scene::UvTransform;
    let materials = s.materials.clone();
    for mesh in s.meshes.iter_mut() {
        for p in mesh.primitives.iter_mut() {
            let Some(m) = p.material.and_then(|m| materials.get(m)) else { continue };
            // The first reference to a UV set CLAIMS it, transformed or not.
            // An untransformed base-colour texture on set 0 must keep set 0
            // untransformed even when the emissive map applies an offset to
            // it — baking the emissive offset would shift the base colour.
            let mut claimed: Vec<(u32, Option<UvTransform>)> = Vec::new();
            let order = [
                &m.base_color_texture,
                &m.emissive_texture,
                &m.normal_texture,
                &m.metallic_roughness_texture,
                &m.occlusion_texture,
            ];
            for r in order.into_iter().flatten() {
                if claimed.iter().any(|(set, _)| *set == r.uv_set) {
                    continue;
                }
                claimed.push((r.uv_set, r.transform));
                let Some(t) = r.transform else { continue };
                if let Some(set) = p.uvs.get_mut(r.uv_set as usize) {
                    for uv in set.iter_mut() {
                        *uv = t.apply(*uv);
                    }
                }
            }
        }
    }
    for m in s.materials.iter_mut() {
        for r in [
            &mut m.base_color_texture,
            &mut m.metallic_roughness_texture,
            &mut m.normal_texture,
            &mut m.occlusion_texture,
            &mut m.emissive_texture,
        ]
        .into_iter()
        .flatten()
        {
            r.transform = None;
        }
    }
}

/// Drop textures and images nothing references any more, remapping indices.
pub fn prune_unused_textures(s: &mut Scene) {
    let mut used_tex = vec![false; s.textures.len()];
    for m in &s.materials {
        for (_, r) in m.texture_refs() {
            if let Some(u) = used_tex.get_mut(r.texture) {
                *u = true;
            }
        }
    }
    let mut tex_map = vec![None; s.textures.len()];
    let mut textures = Vec::new();
    for (i, t) in s.textures.iter().enumerate() {
        if used_tex[i] {
            tex_map[i] = Some(textures.len());
            textures.push(t.clone());
        }
    }
    let mut used_img = vec![false; s.images.len()];
    for t in &textures {
        if let Some(u) = used_img.get_mut(t.image) {
            *u = true;
        }
    }
    let mut img_map = vec![None; s.images.len()];
    let mut images = Vec::new();
    for (i, im) in s.images.iter().enumerate() {
        if used_img[i] {
            img_map[i] = Some(images.len());
            images.push(im.clone());
        }
    }
    for t in textures.iter_mut() {
        t.image = img_map[t.image].unwrap_or(0);
    }
    for m in s.materials.iter_mut() {
        for r in [
            &mut m.base_color_texture,
            &mut m.metallic_roughness_texture,
            &mut m.normal_texture,
            &mut m.occlusion_texture,
            &mut m.emissive_texture,
        ] {
            if let Some(tr) = r {
                match tex_map[tr.texture] {
                    Some(n) => tr.texture = n,
                    None => *r = None,
                }
            }
        }
    }
    s.textures = textures;
    s.images = images;
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::scene::{Animation, MorphTarget, Skin, SkinWeights};

    fn tri() -> Primitive {
        Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        }
    }

    fn scene_with(prim: Primitive, transform: Transform) -> Scene {
        let mut s = Scene { meshes: vec![SceneMesh { primitives: vec![prim], ..Default::default() }], ..Default::default() };
        s.add_root(Node { mesh: Some(0), transform, ..Default::default() });
        s
    }

    #[test]
    fn baking_applies_the_node_transform() {
        let s = scene_with(
            tri(),
            Transform::Trs { translation: [10.0, 0.0, 0.0], rotation: math::QUAT_IDENTITY, scale: [2.0; 3] },
        );
        let m = flatten(&s).unwrap();
        assert_eq!(m.positions, vec![[10.0, 0.0, 0.0], [12.0, 0.0, 0.0], [10.0, 2.0, 0.0]]);
    }

    #[test]
    fn a_mirror_flips_the_winding_so_the_face_still_points_out() {
        let s = scene_with(
            tri(),
            Transform::Trs { translation: [0.0; 3], rotation: math::QUAT_IDENTITY, scale: [-1.0, 1.0, 1.0] },
        );
        let m = flatten(&s).unwrap();
        // Original normal +Z; mirrored in X it would point -Z without the flip.
        let tri = [m.positions[m.indices[0] as usize], m.positions[m.indices[1] as usize], m.positions[m.indices[2] as usize]];
        assert!(Mesh::facet_normal(&tri)[2] > 0.0, "{tri:?}");
    }

    #[test]
    fn morph_targets_apply_at_their_default_weight() {
        let mut p = tri();
        p.targets.push(MorphTarget { positions: vec![[0.0, 0.0, 1.0]; 3], normals: None });
        let mut s = scene_with(p, Transform::default());
        s.meshes[0].weights = vec![0.5];
        let m = flatten(&s).unwrap();
        assert!(m.positions.iter().all(|p| (p[2] - 0.5).abs() < 1e-6), "{:?}", m.positions);
    }

    #[test]
    fn skinning_uses_joint_world_times_inverse_bind_and_ignores_the_mesh_node() {
        // Joint at x=5 bound where it stands (inverse bind = translate(-5)),
        // then moved to x=7 in the pose: every vertex follows by +2. The mesh
        // node's own +100 translation must be ignored.
        let mut p = tri();
        p.skin = Some(SkinWeights { influences: 4, joints: vec![0; 12], weights: [1.0, 0.0, 0.0, 0.0].repeat(3) });
        let mut s = Scene { meshes: vec![SceneMesh { primitives: vec![p], ..Default::default() }], ..Default::default() };
        let joint = s.add_root(Node {
            transform: Transform::Trs { translation: [7.0, 0.0, 0.0], rotation: math::QUAT_IDENTITY, scale: [1.0; 3] },
            ..Default::default()
        });
        s.skins.push(Skin { joints: vec![joint], inverse_bind_matrices: vec![math::translation([-5.0, 0.0, 0.0])], ..Default::default() });
        s.add_root(Node {
            mesh: Some(0),
            skin: Some(0),
            transform: Transform::Trs { translation: [100.0, 0.0, 0.0], rotation: math::QUAT_IDENTITY, scale: [1.0; 3] },
            ..Default::default()
        });
        s.validate().unwrap();
        let m = flatten(&s).unwrap();
        assert_eq!(m.positions, vec![[2.0, 0.0, 0.0], [3.0, 0.0, 0.0], [2.0, 1.0, 0.0]]);
    }

    fn channel(path: AnimPath, interp: Interpolation, times: Vec<f32>, values: Vec<f32>) -> Channel {
        Channel { node: 0, path, interpolation: interp, times, values }
    }

    #[test]
    fn linear_translation_interpolates_and_clamps() {
        let ch = channel(AnimPath::Translation, Interpolation::Linear, vec![1.0, 3.0], vec![0.0, 0.0, 0.0, 4.0, 0.0, 0.0]);
        assert_eq!(sample_channel(&ch, 0.0, 3), vec![0.0, 0.0, 0.0]);
        assert_eq!(sample_channel(&ch, 2.0, 3), vec![2.0, 0.0, 0.0]);
        assert_eq!(sample_channel(&ch, 9.0, 3), vec![4.0, 0.0, 0.0]);
    }

    #[test]
    fn step_holds_the_previous_key() {
        let ch = channel(AnimPath::Scale, Interpolation::Step, vec![0.0, 1.0], vec![1.0, 1.0, 1.0, 2.0, 2.0, 2.0]);
        assert_eq!(sample_channel(&ch, 0.99, 3), vec![1.0; 3]);
        assert_eq!(sample_channel(&ch, 1.0, 3), vec![2.0; 3]);
    }

    #[test]
    fn rotation_slerps_rather_than_lerping_components() {
        let a = math::quat_from_axis_angle([0.0, 0.0, 1.0], 0.0);
        let b = math::quat_from_axis_angle([0.0, 0.0, 1.0], 2.0);
        let vals: Vec<f32> = a.iter().chain(b.iter()).map(|v| *v as f32).collect();
        let ch = channel(AnimPath::Rotation, Interpolation::Linear, vec![0.0, 1.0], vals);
        let q = sample_channel(&ch, 0.5, 4);
        let want = math::quat_from_axis_angle([0.0, 0.0, 1.0], 1.0);
        let d = math::quat_dot([q[0] as f64, q[1] as f64, q[2] as f64, q[3] as f64], want).abs();
        assert!(d > 1.0 - 1e-6, "{q:?}");
    }

    #[test]
    fn cubic_spline_passes_through_keys_and_uses_scaled_tangents() {
        // One scalar-ish channel (translation x), keys at 0 and 2 with values
        // 0 and 1, zero tangents: a smoothstep. At the midpoint, 0.5 exactly.
        let mut values = Vec::new();
        for v in [0.0f32, 1.0] {
            values.extend_from_slice(&[0.0, 0.0, 0.0]); // in-tangent
            values.extend_from_slice(&[v, 0.0, 0.0]); // value
            values.extend_from_slice(&[0.0, 0.0, 0.0]); // out-tangent
        }
        let ch = channel(AnimPath::Translation, Interpolation::CubicSpline, vec![0.0, 2.0], values);
        assert!((sample_channel(&ch, 1.0, 3)[0] - 0.5).abs() < 1e-6);
        assert!((sample_channel(&ch, 0.5, 3)[0] - 0.15625).abs() < 1e-6);
        assert_eq!(sample_channel(&ch, 2.0, 3)[0], 1.0);
    }

    #[test]
    fn sample_pose_leaves_unanimated_paths_alone() {
        let mut s = scene_with(
            tri(),
            Transform::Trs { translation: [0.0; 3], rotation: math::QUAT_IDENTITY, scale: [3.0; 3] },
        );
        s.animations.push(Animation {
            name: None,
            channels: vec![channel(AnimPath::Translation, Interpolation::Linear, vec![0.0, 1.0], vec![0.0, 0.0, 0.0, 2.0, 0.0, 0.0])],
        });
        let pose = sample_pose(&s, 0, 0.5);
        let (t, _, sc) = pose.trs[0].unwrap();
        assert_eq!(t, [1.0, 0.0, 0.0]);
        assert_eq!(sc, [3.0; 3], "scale is not animated and must keep the node's value");
    }

    #[test]
    fn resample_includes_source_keys_and_regular_samples() {
        let mut s = scene_with(tri(), Transform::default());
        s.animations.push(Animation {
            name: None,
            channels: vec![channel(AnimPath::Translation, Interpolation::Linear, vec![0.0, 0.33, 1.0], vec![0.0; 9])],
        });
        let t = resample_times(&s, 0, 10.0);
        assert!(t.contains(&0.33));
        assert_eq!(t.first(), Some(&0.0));
        assert_eq!(t.last(), Some(&1.0));
        assert!(t.windows(2).all(|w| w[1] > w[0]));
        assert!(t.len() >= 11);
    }

    #[test]
    fn an_untransformed_texture_keeps_its_uv_set_when_another_map_transforms_it() {
        use super::super::scene::{Material, TextureRef, UvTransform};
        let mut p = tri();
        p.uvs = vec![vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]];
        p.material = Some(0);
        let mut s = scene_with(p, Transform::default());
        s.materials.push(Material {
            base_color_texture: Some(TextureRef { texture: 0, uv_set: 0, transform: None }),
            emissive_texture: Some(TextureRef {
                texture: 0,
                uv_set: 0,
                transform: Some(UvTransform { offset: [0.5, 0.0], rotation: 0.0, scale: [1.0, 1.0] }),
            }),
            ..Default::default()
        });
        let before = s.meshes[0].primitives[0].uvs.clone();
        bake_texture_transforms(&mut s);
        assert_eq!(s.meshes[0].primitives[0].uvs, before, "the base colour claimed set 0 untransformed");
        assert!(s.materials[0].emissive_texture.as_ref().unwrap().transform.is_none());
    }

    #[test]
    fn channels_that_never_move_their_node_are_dropped() {
        use super::super::scene::{Animation, Channel};
        let at = |t: [f32; 3]| Transform::Trs { translation: [t[0] as f64, t[1] as f64, t[2] as f64], rotation: math::QUAT_IDENTITY, scale: [1.0; 3] };
        let mut s = scene_with(tri(), at([1.0, 2.0, 3.0]));
        let ch = |path, values: Vec<f32>| Channel { node: 0, path, interpolation: Interpolation::Linear, times: vec![0.0, 1.0], values };
        s.animations.push(Animation {
            name: Some("clip".into()),
            channels: vec![
                ch(AnimPath::Translation, vec![1.0, 2.0, 3.0, 1.0, 2.0, 3.0]), // where it stands: dropped
                ch(AnimPath::Rotation, vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0]), // identity, either sign: dropped
                ch(AnimPath::Scale, vec![2.0, 2.0, 2.0, 2.0, 2.0, 2.0]), // constant but elsewhere: kept
            ],
        });
        let mut t = s.clone();
        drop_constant_channels(&mut t);
        let paths: Vec<AnimPath> = t.animations[0].channels.iter().map(|c| c.path).collect();
        assert_eq!(paths, vec![AnimPath::Scale]);
        // Nothing moves at all: the clip keeps one channel, and its length.
        s.animations[0].channels.truncate(2);
        drop_constant_channels(&mut s);
        assert_eq!(s.animations[0].channels.len(), 1);
        assert_eq!(s.animations[0].time_range(), Some((0.0, 1.0)));
    }

    #[test]
    fn flat_round_trip_through_a_scene_is_exact() {
        let mesh = Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            normals: Some(vec![[0.0, 0.0, 1.0]; 3]),
            uvs: Some(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
            base_color: Some([0.2, 0.4, 0.6, 1.0]),
            name: Some("tri".into()),
        };
        assert_eq!(flatten(&from_flat(&mesh)).unwrap(), mesh);
    }
}
