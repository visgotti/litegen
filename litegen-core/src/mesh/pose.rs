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
    AnimPath, Channel, Interpolation, Node, Primitive, Scene, SceneMesh, TextureSlot, Topology, Transform,
    MAX_SCENE_INDICES, MAX_SCENE_VERTICES,
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
    // `!(t > first)` also catches NaN, which would otherwise search past the
    // last key.
    if !(t > ch.times[0]) || n == 1 {
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
    /// What `indices` draws, as the source primitive's.
    pub topology: Topology,
}

/// Every drawn primitive in world space, in node traversal order, whatever it
/// draws: triangles, segments or points. A mirror reverses a triangle's
/// winding and leaves a segment's ends and a point as they are.
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
                    if mats.first().is_some_and(math::is_mirroring) && prim.topology == Topology::Triangles {
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
                    if math::is_mirroring(m) && prim.topology == Topology::Triangles {
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
                topology: prim.topology,
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
                topology: wp.topology,
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

/// The axis-aligned bounds, as `(min, max)`, of every position `scene` has in
/// `pose`: triangles, segments and points alike, so a point cloud has bounds
/// and a model's loose points and lines are inside them. Positions are
/// counted whether or not an index names them, as the flat view's
/// ([`Mesh::bounds`]) are. `None` when the scene draws no positions, and
/// when it cannot be posed within the scene caps ([`world_primitives`] says
/// why).
pub fn world_bounds(scene: &Scene, pose: &Pose) -> Option<([f64; 3], [f64; 3])> {
    let mut bounds: Option<([f32; 3], [f32; 3])> = None;
    for wp in world_primitives(scene, pose).ok()? {
        for p in &wp.positions {
            let (lo, hi) = bounds.get_or_insert((*p, *p));
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    bounds.map(|(lo, hi)| (lo.map(f64::from), hi.map(f64::from)))
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
///
/// The flat view holds triangles only: a scene's lines and points are left
/// out of it, positions and all, and a scene of nothing else gives an empty
/// mesh.
pub fn flatten(scene: &Scene) -> Result<Mesh, MeshError> {
    let mut prims = world_primitives(scene, &default_pose(scene))?;
    prims.retain(|wp| wp.topology == Topology::Triangles);
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

/// Refuse a scene whose geometry, counted as a writer will copy it, passes the
/// merged-geometry ceiling — before the writer copies anything. `copied(node)`
/// says whether the writer gives that mesh-bearing node its own copy of its
/// mesh; every mesh some other node draws is counted once, as it is written
/// once. One mesh reached from many nodes reads once and can be written many
/// times, which is how 1.2 MB of input became a 114 MB USDZ.
pub fn check_copied_geometry(scene: &Scene, copied: impl Fn(usize) -> bool) -> Result<(), MeshError> {
    let size = |mi: usize| {
        scene.meshes.get(mi).map_or((0, 0), |m| {
            m.primitives.iter().fold((0usize, 0usize), |(v, i), p| (v + p.positions.len(), i + p.indices.len()))
        })
    };
    let mut shared = vec![false; scene.meshes.len()];
    let (mut vertices, mut indices) = (0usize, 0usize);
    for (n, node) in scene.nodes.iter().enumerate() {
        let Some(mi) = node.mesh else { continue };
        if copied(n) {
            let (v, i) = size(mi);
            vertices = vertices.saturating_add(v);
            indices = indices.saturating_add(i);
        } else if let Some(s) = shared.get_mut(mi).filter(|s| !**s) {
            *s = true;
            let (v, i) = size(mi);
            vertices = vertices.saturating_add(v);
            indices = indices.saturating_add(i);
        }
        if vertices > MAX_SCENE_VERTICES {
            return Err(MeshError::Unsupported(format!(
                "written out, the meshes pass {MAX_SCENE_VERTICES} vertices at node {n}; \
                 the node graph instances more geometry than this converter will hold"
            )));
        }
        if indices > MAX_SCENE_INDICES {
            return Err(MeshError::Unsupported(format!("written out, the meshes pass {MAX_SCENE_INDICES} indices at node {n}")));
        }
    }
    Ok(())
}

/// Start a clip at 0 when a key comes before it: the scene keeps glTF's
/// clock, which has no negative time (glTF 2.0 §3.11; the validator's
/// ACCESSOR_ANIMATION_INPUT_NEGATIVE). Every channel moves together, so they
/// stay in step; keys the shift rounds together in f32 keep the first.
/// Returns the clip's original start when it moved.
pub fn start_at_zero(channels: &mut [Channel]) -> Option<f32> {
    let lo = channels.iter().filter_map(|c| c.times.first().copied()).fold(f32::INFINITY, f32::min);
    if !(lo < 0.0) {
        return None;
    }
    for c in channels.iter_mut() {
        let per = c.values.len() / c.times.len().max(1);
        let (mut times, mut values) = (Vec::with_capacity(c.times.len()), Vec::with_capacity(c.values.len()));
        for (k, t) in c.times.iter().enumerate() {
            let t = t - lo;
            if times.last().is_some_and(|l: &f32| *l >= t) {
                continue;
            }
            times.push(t);
            values.extend_from_slice(&c.values[k * per..(k + 1) * per]);
        }
        (c.times, c.values) = (times, values);
    }
    Some(lo)
}

/// What [`bake_shear`] changes: each mesh baked out of a sheared run of
/// static transforms, each light and camera moved out of one, and each
/// sheared node whose nearest TRS something is still drawn through.
#[derive(Debug, Default)]
pub struct ShearPlan {
    /// (mesh node, the animated ancestor its baked copy hangs from, the
    /// static transforms below that ancestor, composed).
    pub baked: Vec<(usize, Option<usize>, Mat4)>,
    /// The same for each node that places a light or a camera under a
    /// sheared run: [`bake_shear`] moves them onto a new node hung from that
    /// ancestor, framed as the run leaves them (Ruling R19).
    pub lights_and_cameras: Vec<(usize, Option<usize>, Mat4)>,
    pub approximated: Vec<usize>,
}

fn sheared_local(n: &Node) -> bool {
    matches!(n.transform, Transform::Matrix(m) if math::decompose(&m).is_none())
}

/// Where [`bake_shear`] must act. A node's world transform may be sheared
/// freely (a rotated child of a non-uniformly scaled parent); only a LOCAL
/// matrix must decompose (glTF 2.0 §5.25: "When matrix is defined, it MUST be
/// decomposable to TRS properties"; FBX has no other form).
pub fn shear_plan(s: &Scene) -> ShearPlan {
    if !s.nodes.iter().any(sheared_local) {
        return ShearPlan::default();
    }
    let n = s.nodes.len();
    let mut animated = vec![false; n];
    for c in s.animations.iter().flat_map(|a| &a.channels) {
        if c.path != AnimPath::Weights {
            if let Some(a) = animated.get_mut(c.node) {
                *a = true;
            }
        }
    }
    // Per node: its nearest animated ancestor-or-self, the static locals
    // composed from just below it down to this node, and whether any is
    // sheared. A mesh whose run holds a shear is baked by exactly that run,
    // and hangs from the ancestor, so the clip still moves it.
    let mut run: Vec<(Option<usize>, Mat4, bool)> = vec![(None, math::IDENTITY, false); n];
    let parents = s.parents();
    let (mut baked, mut lights_and_cameras) = (Vec::new(), Vec::new());
    for i in s.topological_order() {
        run[i] = if animated[i] {
            (Some(i), math::IDENTITY, false)
        } else {
            let (anchor, above, shear) = parents[i].map_or((None, math::IDENTITY, false), |p| run[p]);
            (anchor, math::mat_mul(&above, &s.nodes[i].transform.matrix()), shear || sheared_local(&s.nodes[i]))
        };
        let node = &s.nodes[i];
        if node.mesh.is_some() && node.skin.is_none() && run[i].2 {
            baked.push((i, run[i].0, run[i].1));
        }
        if (node.light.is_some() || node.camera.is_some()) && run[i].2 {
            lights_and_cameras.push((i, run[i].0, run[i].1));
        }
    }
    // A sheared node matters while something is still drawn through it: an
    // unbaked rigid mesh, a joint, a light or a camera, or a baked mesh's
    // anchor below it. A light or camera moved out of its run keeps its eye
    // and the way it faces, but not the shear of its frame (its up is
    // squared to its view), so the shear it hung from still counts.
    let joints = s.joint_nodes();
    let mut matters: Vec<bool> = (0..n)
        .map(|i| {
            let node = &s.nodes[i];
            joints[i] || (node.mesh.is_some() && node.skin.is_none() && !run[i].2) || node.light.is_some() || node.camera.is_some()
        })
        .collect();
    for (_, anchor, _) in &baked {
        if let Some(a) = anchor {
            matters[*a] = true;
        }
    }
    for i in s.topological_order().into_iter().rev() {
        if let Some(p) = parents[i] {
            if matters[i] {
                matters[p] = true;
            }
        }
    }
    let approximated = (0..n).filter(|&i| sheared_local(&s.nodes[i]) && matters[i]).collect();
    ShearPlan { baked, lights_and_cameras, approximated }
}

/// The frame a light or a camera keeps of a sheared placement `m` (Ruling
/// R19): its eye, which is `m`'s translation; the way it faces, `m`'s −Z
/// normalised; and the up nearest `m`'s +Y that is square to that (or, when
/// +Y runs along the view, the up that makes the right nearest `m`'s +X).
/// The scale is 1: glTF's camera ignores its node's scale, and a light's
/// direction is a direction. `None` when `m` has no way to face (its Z axis
/// scaled to nothing).
fn view_frame(m: &Mat4) -> Option<Transform> {
    let col = |c: usize| [m[c * 4], m[c * 4 + 1], m[c * 4 + 2]];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let unit = |v: [f64; 3]| {
        let l = dot(v, v).sqrt();
        (l > 1e-12 && l.is_finite()).then(|| [v[0] / l, v[1] / l, v[2] / l])
    };
    let z = unit(col(2))?;
    let square = |v: [f64; 3]| {
        let d = dot(v, z);
        unit([v[0] - d * z[0], v[1] - d * z[1], v[2] - d * z[2]])
    };
    let y = square(col(1)).or_else(|| square(col(0)).map(|x| cross(z, x)))?;
    let x = cross(y, z);
    let r = [x[0], x[1], x[2], y[0], y[1], y[2], z[0], z[1], z[2]];
    Some(Transform::Trs { translation: [m[12], m[13], m[14]], rotation: math::mat3_to_quat(&r), scale: [1.0; 3] })
}

/// Make every local transform decomposable to TRS, for the writers that can
/// only say TRS (glTF, FBX). A mesh hung from a sheared run of static
/// transforms is baked through that run — positions, normals, tangents,
/// morph deltas, winding under a mirror — onto a new node with no transform
/// under the run's animated ancestor (or at the root), which draws it
/// exactly where it was at every time. A skinned mesh is drawn by its joints
/// and is left alone. A light or a camera hung from such a run moves onto a
/// new node under the same ancestor (or at the root), framed by
/// [`view_frame`], so it keeps its eye and the way it faces at every time,
/// with its up squared to its view (Ruling R19). Each new node, a baked
/// mesh's or a light's and camera's, takes its old node's name, so tests and
/// readers that bind by name (three.js) find it there. The old node of a
/// light or camera, which now holds neither, is renamed `<name>_shear`, so
/// that a writer or reader that keeps names unique does not rename the
/// light's or camera's instead; an old node that only held a mesh keeps its
/// name. Each keeps its children and whatever else it held. Every sheared
/// matrix left is replaced by its nearest TRS ([`math::decompose_lossy`]);
/// [`shear_plan`] says which of those still move something drawn.
pub fn bake_shear(s: &mut Scene) {
    let plan = shear_plan(s);
    // The nodes whose light or camera moves, each renamed once the meshes
    // are baked (below), so a baked mesh still takes the name it had.
    let mut moved = Vec::new();
    for &(n, anchor, rel) in &plan.lights_and_cameras {
        let Some(transform) = view_frame(&rel) else { continue };
        let node = Node {
            name: s.nodes[n].name.clone(),
            transform,
            light: s.nodes[n].light.take(),
            camera: s.nodes[n].camera.take(),
            ..Default::default()
        };
        match anchor {
            Some(a) => {
                s.nodes.push(node);
                let new = s.nodes.len() - 1;
                s.nodes[a].children.push(new);
            }
            None => {
                s.add_root(node);
            }
        }
        moved.push(n);
    }
    for &(n, anchor, rel) in &plan.baked {
        let Some(mi) = s.nodes[n].mesh else { continue };
        let nm = math::normal_matrix(&rel);
        let lin = math::upper3(&rel);
        let mirror = math::is_mirroring(&rel);
        let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let mut mesh = s.meshes[mi].clone();
        for p in mesh.primitives.iter_mut() {
            for q in p.positions.iter_mut() {
                *q = math::transform_point(&rel, *q);
            }
            // A target's normal delta is added to the unit base normal and
            // the sum renormalised: scaled by how much the base stretches, it
            // still sums to the transformed direction.
            let stretch: Vec<f32> = match &p.normals {
                Some(ns) => ns
                    .iter()
                    .map(|v| Some(len(math::transform_dir3(&nm, *v))).filter(|l| *l > 0.0 && l.is_finite()).unwrap_or(1.0))
                    .collect(),
                None => Vec::new(),
            };
            for tgt in p.targets.iter_mut() {
                for q in tgt.positions.iter_mut() {
                    *q = math::transform_dir3(&lin, *q);
                }
                if let Some(ns) = tgt.normals.as_mut() {
                    for (v, d) in ns.iter_mut().enumerate() {
                        let k = stretch.get(v).copied().unwrap_or(1.0);
                        let t = math::transform_dir3(&nm, *d);
                        *d = [t[0] / k, t[1] / k, t[2] / k];
                    }
                }
            }
            if let Some(ns) = p.normals.as_mut() {
                for v in ns.iter_mut() {
                    *v = math::transform_normal(&nm, *v);
                }
            }
            if let Some(ts) = p.tangents.as_mut() {
                for v in ts.iter_mut() {
                    let d = math::transform_dir3(&lin, [v[0], v[1], v[2]]);
                    let l = len(d);
                    // The bitangent is cross(normal, tangent) * w; a mirror
                    // reverses that cross product.
                    if l > 0.0 && l.is_finite() {
                        *v = [d[0] / l, d[1] / l, d[2] / l, if mirror { -v[3] } else { v[3] }];
                    }
                }
            }
            // Only a triangle has a winding to reverse: a segment's ends and a
            // point's place in its list are the same under a mirror.
            if mirror && p.topology == Topology::Triangles {
                for tri in p.indices.chunks_exact_mut(3) {
                    tri.swap(1, 2);
                }
            }
        }
        s.meshes.push(mesh);
        let node = Node {
            name: s.nodes[n].name.clone(),
            mesh: Some(s.meshes.len() - 1),
            weights: s.nodes[n].weights.take(),
            ..Default::default()
        };
        s.nodes[n].mesh = None;
        let new = match anchor {
            Some(a) => {
                s.nodes.push(node);
                let new = s.nodes.len() - 1;
                s.nodes[a].children.push(new);
                new
            }
            None => s.add_root(node),
        };
        for c in s.animations.iter_mut().flat_map(|a| a.channels.iter_mut()) {
            if c.node == n && c.path == AnimPath::Weights {
                c.node = new;
            }
        }
    }
    // The name is the moved light's or camera's alone: the writers and
    // readers that keep names unique (FBX's `Names::unique`, GLTFLoader's
    // `createUniqueName`) would otherwise rename one of the two, and a
    // reader that binds by name could take the wrong one (Ruling R19).
    for n in moved {
        if let Some(name) = s.nodes[n].name.as_mut() {
            name.push_str("_shear");
        }
    }
    for node in s.nodes.iter_mut().filter(|n| sheared_local(n)) {
        if let Transform::Matrix(m) = node.transform {
            let (translation, rotation, scale) = math::decompose_lossy(&m);
            node.transform = Transform::Trs { translation, rotation, scale };
        }
    }
}

/// What [`bake_shear`] costs a container that can only say TRS (`target`).
pub fn shear_losses(s: &Scene, target: &str) -> Vec<super::scene::Loss> {
    use super::scene::{Loss, LossKind};
    let plan = shear_plan(s);
    let name = |i: usize| s.nodes[i].name.clone().unwrap_or_else(|| format!("node {i}"));
    let mut out = Vec::new();
    if !plan.baked.is_empty() {
        out.push(Loss::new(
            LossKind::Hierarchy,
            format!("the sheared placement of {} mesh node(s) (first: {:?})", plan.baked.len(), name(plan.baked[0].0)),
            format!("{target} transforms are translation, rotation and scale; each mesh is baked in place onto a new node"),
        ));
    }
    if !plan.approximated.is_empty() {
        out.push(Loss::new(
            LossKind::Geometry,
            format!("the shear of {} node(s) (first: {:?})", plan.approximated.len(), name(plan.approximated[0])),
            format!("{target} transforms are translation, rotation and scale; the nearest one moves what hangs from them"),
        ));
    }
    out
}

/// Values a writer that resamples animation (FBX, Collada, USD) may key, over
/// all clips: sample times × what each time holds, counted as Collada bakes
/// it (a 4×4 matrix per animated node) plus one weight per morph target. That
/// is 256 MB as Collada's f64 matrices, and keeps every FBX the writer makes
/// under our own reader's key cap (≤ 9 curves per 16 values). A minute of a
/// 100-joint rig at 30 fps is 2.9M.
pub const MAX_RESAMPLED_VALUES: usize = 32_000_000;

/// What resampling `scene` for a writer would cost, refused before any
/// sampling when it passes [`MAX_RESAMPLED_VALUES`]: the grid
/// ([`resample_times`]) times the nodes and morph targets each clip keys. The
/// grid alone is capped; across thousands of animated nodes it is not.
///
/// `weight_values(mesh)` is what one weight channel of a node drawing `mesh`
/// costs at each sampled time, in values written: its target count where a
/// writer keys one weight per target (FBX, Collada baking its morphs), more
/// where it spreads each over several arrays (Collada keeping them writes a
/// `<animation>` for each primitive and target, of three arrays). The error
/// counts the weights, and names the values a key as well where they differ.
pub fn check_resample_budget(scene: &Scene, fps: f32, weight_values: impl Fn(&SceneMesh) -> usize) -> Result<(), MeshError> {
    let mut total: usize = 0;
    for (ai, a) in scene.animations.iter().enumerate() {
        let nodes: std::collections::BTreeSet<usize> =
            a.channels.iter().filter(|c| c.path != AnimPath::Weights).map(|c| c.node).collect();
        let drawn: Vec<&SceneMesh> = a
            .channels
            .iter()
            .filter(|c| c.path == AnimPath::Weights)
            .filter_map(|c| scene.nodes.get(c.node).and_then(|n| n.mesh))
            .map(|m| &scene.meshes[m])
            .collect();
        let weights: usize = drawn.iter().map(|m| m.target_count()).sum();
        let values = drawn.iter().fold(0usize, |sum, m| sum.saturating_add(weight_values(m)));
        let width = nodes.len().saturating_mul(16).saturating_add(values);
        total = total.saturating_add(resample_times(scene, ai, fps).len().saturating_mul(width));
        if total > MAX_RESAMPLED_VALUES {
            // A writer that spreads each weight over several values (Collada
            // keeping its morphs) says so: the weights alone would not show
            // what crossed the cap.
            let written = if values == weights { String::new() } else { format!(", written as {values} values a key") };
            return Err(MeshError::Unsupported(format!(
                "the animation would resample to more than {MAX_RESAMPLED_VALUES} keyed values (clip {ai}: {} nodes, {weights} morph weights{written})",
                nodes.len()
            )));
        }
    }
    Ok(())
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
    // Each time, and whether it is exact: a source key (or the hold at 0) is,
    // a grid time is not.
    let mut times: Vec<(f32, bool)> = a.channels.iter().flat_map(|c| c.times.iter().map(|&t| (t, true))).collect();
    if lo > 0.0 {
        times.push((0.0, true));
    }
    if hi > lo && fps > 0.0 {
        // In f64: keys at ±2e38 s span more than f32 holds, and `inf × 0` made
        // the grid's first time NaN.
        let (lo64, span) = (f64::from(lo), f64::from(hi) - f64::from(lo));
        let steps = ((span * f64::from(fps)).ceil() as usize).min(100_000);
        for i in 0..=steps {
            times.push(((lo64 + span * (i as f64 / steps.max(1) as f64)) as f32, false));
        }
    }
    times.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    // Merge keys closer than a microsecond: they are the same instant written
    // through two different float paths, and two keys at one time are a
    // validation error in every target. A source key wins over the grid time
    // beside it: STEP sampled one ULP before its key returns the previous
    // value, so keeping the grid time fired every such step a frame late.
    let mut merged: Vec<(f32, bool)> = Vec::with_capacity(times.len());
    for (t, exact) in times {
        match merged.last_mut() {
            Some(last) if (t - last.0).abs() < 1e-6 => {
                if exact && !last.1 {
                    *last = (t, true);
                }
            }
            _ => merged.push((t, exact)),
        }
    }
    merged.into_iter().map(|(t, _)| t).collect()
}

/// All drawn geometry merged into ONE primitive in world space, the way a
/// single-mesh container holds it: one per topology, for a scene that has
/// lines or points beside its triangles. Primitives of different topologies
/// are never merged, so the merged mesh has at most three primitives, in the
/// order triangles, lines, points, each indexing its own vertices.
///
/// Attributes follow the flat view's rules (`flatten`): normals are derived
/// where a primitive lacks them (and are `+Z` where it draws no triangles to
/// derive them from), UV set 0 is zero-filled, and the merged primitive takes
/// the first primitive's material. Vertex colours, which the flat view never
/// carried, are white-filled for primitives that lack them.
pub fn merge_all(scene: &Scene) -> Result<Scene, MeshError> {
    let prims = world_primitives(scene, &default_pose(scene))?;
    let mut merged: Vec<Primitive> = [Topology::Triangles, Topology::Lines, Topology::Points]
        .into_iter()
        .filter_map(|topology| {
            let group: Vec<&WorldPrimitive> = prims.iter().filter(|wp| wp.topology == topology).collect();
            (!group.is_empty()).then(|| merge_group(topology, &group))
        })
        .collect();
    if merged.is_empty() {
        merged.push(Primitive::default());
    }
    let mut s = Scene {
        name: scene.name.clone(),
        materials: scene.materials.clone(),
        textures: scene.textures.clone(),
        images: scene.images.clone(),
        ..Default::default()
    };
    let name = prims.iter().find_map(|wp| scene.meshes[wp.mesh].name.clone());
    s.meshes.push(SceneMesh { name: name.clone(), primitives: merged, ..Default::default() });
    s.add_root(Node { name, mesh: Some(0), ..Default::default() });
    Ok(s)
}

/// `group`, which all draw `topology`, as one primitive ([`merge_all`]).
fn merge_group(topology: Topology, group: &[&WorldPrimitive]) -> Primitive {
    let any_normals = group.iter().any(|p| p.normals.is_some());
    let any_uvs = group.iter().any(|p| !p.uvs.is_empty());
    let any_colors = group.iter().any(|p| p.colors.is_some());
    let mut out = Primitive { topology, ..Default::default() };
    let (mut normals, mut uvs, mut colors) = (Vec::new(), Vec::new(), Vec::new());
    for wp in group {
        let base = out.positions.len() as u32;
        if any_normals {
            match &wp.normals {
                Some(n) => normals.extend_from_slice(n),
                None if topology == Topology::Triangles => normals.extend(derived_normals(&wp.positions, &wp.indices)),
                None => normals.extend(std::iter::repeat_n([0.0f32, 0.0, 1.0], wp.positions.len())),
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
    out
}

/// The texture slots a container with `caps` keeps under its material model:
/// all five base slots for PBR; base colour, normal and emissive for Phong,
/// which folds metallic-roughness into its factors and drops occlusion; base
/// colour alone for colour-only; none where no texture survives
/// (`caps.textures` false, or vertex colour or no materials at all). Under
/// PBR and Phong, the models that keep layers, the slots of each layer
/// `caps.layers` keeps follow, after emissive. [`degrade`] and
/// `scene::losses` order a material's uv sets by these slots' bindings alone
/// ([`UvSetPlan`]): a map the container drops uses no set in its output.
pub fn kept_texture_slots(caps: &super::scene::Capabilities) -> Vec<TextureSlot> {
    use super::scene::{MaterialLayer, MaterialSupport};
    use super::scene::TextureSlot::{BaseColor, Emissive, MetallicRoughness, Normal, Occlusion};
    if !caps.textures {
        return Vec::new();
    }
    let base: &[TextureSlot] = match caps.materials {
        MaterialSupport::Pbr => &[BaseColor, MetallicRoughness, Normal, Occlusion, Emissive],
        MaterialSupport::Phong => &[BaseColor, Normal, Emissive],
        MaterialSupport::ColorOnly => &[BaseColor],
        MaterialSupport::VertexColor | MaterialSupport::None => &[],
    };
    let mut slots = base.to_vec();
    if matches!(caps.materials, MaterialSupport::Pbr | MaterialSupport::Phong) {
        for layer in MaterialLayer::ALL.into_iter().filter(|&l| caps.layers.keeps(l)) {
            slots.extend_from_slice(layer.texture_slots());
        }
    }
    slots
}

/// The distinct uv sets that a material's bindings in `slots` sample, in
/// first-use order over its slots as `Material::texture_refs` lists them
/// (base colour, metallic-roughness, normal, occlusion, emissive, then each
/// layer's), whatever order `slots` comes in. A container that holds fewer
/// sets than a primitive has keeps these first ([`UvSetPlan`]).
pub fn sampled_set_order(m: &super::scene::Material, slots: &[TextureSlot]) -> Vec<u32> {
    let mut order: Vec<u32> = Vec::new();
    for (slot, r) in m.texture_refs() {
        if slots.contains(&slot) && !order.contains(&r.uv_set) {
            order.push(r.uv_set);
        }
    }
    order
}

/// Where the uv sets of the primitives drawing each material go. [`degrade`],
/// [`order_sampled_sets_first`], `scene::losses` and `threemf::losses` all
/// decide through this one plan, so the sets a writer keeps and the bindings
/// a report names as lost cannot disagree.
///
/// A plan is made for a list of texture slots, the ones its container keeps
/// ([`kept_texture_slots`]), and that list decides only the order: a planned
/// material's primitives take the sets its bindings in those slots sample
/// first ([`sampled_set_order`]), then every other set in index order. The
/// permutation then renumbers every binding of the material, in every slot
/// (`Material::texture_refs_mut`), so none is left pointing at another set's
/// coordinates. A map the container drops keeps reading its own until it is
/// dropped, and a later plan with a longer slot list (`degrade` after
/// Collada's reorder) still finds each map on its own data. Every primitive
/// drawing one material is reordered alike, because the bindings belong to
/// the material. The rest keep scene order: a primitive without a material,
/// and every primitive of a material that is not planned.
#[derive(Debug)]
pub struct UvSetPlan {
    /// Per material: the sets its primitives take first, or `None` for scene
    /// order.
    orders: Vec<Option<Vec<u32>>>,
    /// The slots whose bindings set the order, and that
    /// `planned_binding_sets` reports.
    slots: Vec<TextureSlot>,
}

impl UvSetPlan {
    /// The plan for a container that keeps `keep` sets and the texture slots
    /// in `slots`. A material is planned when one of its bindings in those
    /// slots samples a set at index `keep` or later, which cutting each
    /// primitive to its first `keep` sets would drop with its texture.
    /// `keep = 0` plans every material with a binding in `slots`, as
    /// [`order_sampled_sets_first`] does; no slots plan nothing.
    ///
    /// A material with any binding, in any slot, on a set that some
    /// primitive drawing it lacks keeps scene order, whatever `keep` and
    /// `slots` are. glTF calls such a scene invalid (a `texCoord` must name an
    /// attribute the primitive has), but `Scene::validate` lets it through.
    /// It keeps the first sets, and a kept map past them is reported lost.
    pub fn new(scene: &Scene, keep: usize, slots: &[TextureSlot]) -> UvSetPlan {
        UvSetPlan::of(&scene.materials, &scene.meshes, keep, slots)
    }

    /// [`UvSetPlan::new`] for `meshes` drawn with `materials`.
    fn of(materials: &[super::scene::Material], meshes: &[SceneMesh], keep: usize, slots: &[TextureSlot]) -> UvSetPlan {
        let mut kept: Vec<TextureSlot> = Vec::new();
        for &slot in slots {
            if !kept.contains(&slot) {
                kept.push(slot);
            }
        }
        let mut orders: Vec<Option<Vec<u32>>> = materials
            .iter()
            .map(|m| Some(sampled_set_order(m, &kept)).filter(|order| order.iter().any(|&set| set as usize >= keep)))
            .collect();
        // Degenerate materials: any binding, in any slot, on a set that a
        // primitive drawing the material lacks.
        for p in meshes.iter().flat_map(|m| &m.primitives) {
            let Some(mi) = p.material else { continue };
            let Some(entry) = orders.get_mut(mi) else { continue };
            if entry.is_some() && materials[mi].texture_refs().any(|(_, r)| r.uv_set as usize >= p.uvs.len()) {
                *entry = None;
            }
        }
        UvSetPlan { orders, slots: kept }
    }

    /// The plan for a container with `caps`: `caps.uv_sets` sets, and the
    /// slots its material model keeps ([`kept_texture_slots`]). A container
    /// that keeps no textures gets no plan, so its primitives keep their
    /// first sets.
    pub fn for_caps(scene: &Scene, caps: &super::scene::Capabilities) -> UvSetPlan {
        UvSetPlan::new(scene, caps.uv_sets, &kept_texture_slots(caps))
    }

    /// The index that set `set` of a primitive drawing `material` has under
    /// the plan, for any set. A set in the order takes its place there, and
    /// every other set moves up by the number of ordered sets above it (the
    /// order holds distinct sets). That is linear in the order's length
    /// whatever `set` is, and a hostile file can bind a map to set 4294967295.
    pub fn set_index(&self, material: usize, set: u32) -> u32 {
        let Some(Some(order)) = self.orders.get(material) else { return set };
        match order.iter().position(|&s| s == set) {
            Some(i) => i as u32,
            None => {
                // At most u32::MAX: only sets above `set` count, and at most
                // u32::MAX - set of them fit.
                let above = order.iter().filter(|&&s| s > set).count() as u64;
                u32::try_from(u64::from(set) + above).unwrap_or(u32::MAX)
            }
        }
    }

    /// For every binding in the plan's slots, the index its set has under the
    /// plan. These are the maps the container keeps, so `scene::losses` counts
    /// a binding lost when this index is past the sets the container holds.
    pub fn planned_binding_sets<'a>(&'a self, scene: &'a Scene) -> impl Iterator<Item = u32> + 'a {
        scene.materials.iter().enumerate().flat_map(move |(i, m)| {
            m.texture_refs().filter(move |(slot, _)| self.slots.contains(slot)).map(move |(_, r)| self.set_index(i, r.uv_set))
        })
    }

    /// Reorder every planned primitive's uv sets, and renumber every binding
    /// of its material, in every slot, to match.
    pub fn apply(&self, scene: &mut Scene) {
        for p in scene.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
            let Some(order) = p.material.and_then(|m| self.orders.get(m)).and_then(Option::as_ref) else { continue };
            let mut sets: Vec<Option<Vec<[f32; 2]>>> = std::mem::take(&mut p.uvs).into_iter().map(Some).collect();
            let rest: Vec<usize> = (0..sets.len()).filter(|&i| !order.contains(&(i as u32))).collect();
            p.uvs = order
                .iter()
                .map(|&s| s as usize)
                .chain(rest)
                .filter_map(|i| sets.get_mut(i).and_then(Option::take))
                .collect();
        }
        for (i, m) in scene.materials.iter_mut().enumerate() {
            for (_, r) in m.texture_refs_mut() {
                r.uv_set = self.set_index(i, r.uv_set);
            }
        }
    }
}

/// Each primitive's uv sets reordered so that the ones its material's
/// bindings in `slots` sample come first ([`sampled_set_order`]), every
/// binding renumbered to match, and nothing dropped. `slots` are the texture
/// slots the container keeps: base colour for 3MF; base colour and emissive
/// for Collada; base colour, normal and emissive for FBX. They decide only
/// the order, so a map in any other slot keeps reading its own coordinates
/// until the caller drops it.
///
/// This is for containers whose readers draw from the first set whatever the
/// file binds: 3MF holds only that one, and three.js's Collada and FBX
/// loaders sample every texture from it. A scene whose textures already sit
/// on its first sets, in that order, is unchanged. So is a material with any
/// binding, in any slot, on a set that some primitive drawing it lacks: its
/// primitives keep scene order ([`UvSetPlan::new`]).
pub fn order_sampled_sets_first(scene: &mut Scene, slots: &[TextureSlot]) {
    UvSetPlan::new(scene, 0, slots).apply(scene);
}

/// The scene reduced to what a container with `caps` can hold.
///
/// This is the lossiness table as code: every writer is handed (or behaves as
/// if handed) exactly this, and the render verification harness renders it as
/// the EXPECTED picture for a conversion into such a container. A writer that
/// keeps more than this would be inventing data the target cannot honestly
/// carry; one that keeps less is losing data it should not.
///
/// A material keeps the layers `caps.layers` keeps, under PBR and Phong;
/// any other goes whole, its factors and its maps.
///
/// The lights and cameras the container lacks go, with every node's
/// reference to one ([`drop_lights_and_cameras`]), and so do the points and
/// lines it lacks, with the meshes they leave empty ([`strip_topologies`]).
/// Those it holds are posed and merged as triangles are: baked into world
/// space with the rest, and merged one primitive per topology.
///
/// A primitive with more uv sets than `caps.uv_sets` keeps the ones its
/// material's textures sample, counting only the maps the container keeps
/// ([`kept_texture_slots`]): all five base maps for PBR; base colour, normal
/// and emissive for Phong; base colour for colour-only; then the maps of the
/// layers it keeps. When one of those maps samples a set at index
/// `caps.uv_sets` or later, every primitive drawing the material takes those
/// maps' sets first, in first-use order (base colour, metallic-roughness,
/// normal, occlusion, emissive, then each layer's), then its other sets by
/// index, and every binding of the material follows, in every slot
/// ([`UvSetPlan`]). Then the first `caps.uv_sets` are kept, so a kept map is
/// dropped only when the material's kept maps sample more distinct sets than
/// that. Every primitive keeps its first sets in a container that keeps no
/// textures. So does a primitive without a material, and every primitive of
/// a material that has any binding, in any slot, on a set one of them lacks.
pub fn degrade(scene: &Scene, caps: &super::scene::Capabilities) -> Result<Scene, MeshError> {
    reduce(scene, None, caps)
}

/// [`degrade`] of `scene` drawn with `materials` in place of its own, one
/// for one. A container's `prepare` restates the materials as the container
/// holds them before the reduction, so the uv sets planned here follow the
/// maps it keeps; this way it copies the materials alone for that, and the
/// scene is copied once, here, as [`degrade`] copies it. Only the codecs'
/// `prepare` calls it, so it isn't part of the library's public surface.
pub(super) fn degrade_with_materials(
    scene: &Scene,
    materials: Vec<super::scene::Material>,
    caps: &super::scene::Capabilities,
) -> Result<Scene, MeshError> {
    debug_assert_eq!(materials.len(), scene.materials.len(), "one material for each of the scene's, in order");
    reduce(scene, Some(materials), caps)
}

/// [`degrade`], and [`degrade_with_materials`] when `materials` is given.
fn reduce(scene: &Scene, materials: Option<Vec<super::scene::Material>>, caps: &super::scene::Capabilities) -> Result<Scene, MeshError> {
    use super::scene::{MaterialLayer, MaterialSupport};
    // Decided from the bindings the material model below keeps, before it
    // drops the others, and from the scene `scene::losses` reports on.
    let uv_plan = UvSetPlan::of(materials.as_deref().unwrap_or(&scene.materials), &scene.meshes, caps.uv_sets, &kept_texture_slots(caps));
    let mut s = if !caps.skinning || !caps.morph_targets || !caps.hierarchy {
        bake_default(scene)?
    } else {
        scene.clone()
    };
    if let Some(m) = materials {
        s.materials = m;
    }
    // The sampled sets first, so the cut to `caps.uv_sets` below keeps them:
    // before the merge, which keeps each primitive's first set, and before
    // `bake_base_colors` takes the materials the plan follows.
    uv_plan.apply(&mut s);
    if !caps.animation {
        s.animations.clear();
    }
    drop_lights_and_cameras(&mut s, caps);
    // Before the merge and the colour baking below, which judge what is left.
    strip_topologies(&mut s, caps);
    // Ruling B28 (mesh-gaps item 2): a container that keeps UVs but not
    // texture transforms bakes each transform into its coordinates whether or
    // not it keeps the textures, or gltfpack's quantized UVs (their scale in
    // `KHR_texture_transform`) come out 16× too small in a textureless
    // single-file obj, ply, dae or usda. Where the container keeps no
    // textures, the bake runs here, while the bindings and their transforms
    // still exist: the material model below drops them with the textures
    // (`bake_base_colors` takes the materials whole for a vertex-colour
    // container, and `keep` drops every map for the others). Where it keeps
    // textures, the bake runs after the material model, on the bindings that
    // survive it, as before. Either way `bake_texture_transforms` picks one
    // transform a set where they conflict. `scene::losses` names that
    // conflict only where the container keeps textures (its texture losses
    // sit under `caps.textures`), so a textureless container's goes unnamed.
    if !caps.textures && caps.uv_sets > 0 && !caps.texture_transforms {
        bake_texture_transforms(&mut s);
    }
    if caps.materials == MaterialSupport::VertexColor && caps.vertex_colors && !caps.multiple_meshes {
        // Per-primitive colours must be baked before the merge keeps only one
        // material, or every part takes the first part's colour; and once one
        // part has colours, every part needs them to merge.
        bake_base_colors(&mut s, true);
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
            // The base colour becomes vertex colour. Textures cannot follow:
            // sampling an image per vertex would need an image decoder, and a
            // texture's detail lives between vertices anyway.
            if caps.vertex_colors {
                bake_base_colors(&mut s, false);
            } else {
                s.materials.clear();
                for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
                    p.material = None;
                    p.colors = None;
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
            // The metallic-roughness map goes, but its factors are its
            // multipliers (glTF 2.0 §3.9.2: metalness in B, roughness in G):
            // its mean is folded in. One that does not decode leaves the
            // material non-metallic rather than at glTF's default of fully
            // metallic.
            let by_image = mr_image_means(&s);
            let means: Vec<Option<[f32; 4]>> = s
                .materials
                .iter()
                .map(|m| {
                    let t = m.metallic_roughness_texture.as_ref()?;
                    let image = s.textures.get(t.texture)?.image;
                    by_image.get(&image).copied().flatten()
                })
                .collect();
            for (m, mean) in s.materials.iter_mut().zip(means) {
                if m.metallic_roughness_texture.take().is_some() {
                    match mean {
                        Some(c) => {
                            m.metallic *= c[2];
                            m.roughness *= c[1];
                        }
                        None => m.metallic = 0.0,
                    }
                }
                m.occlusion_texture = None;
            }
        }
        // Every slot stays, filtered below like the other models' slots.
        MaterialSupport::Pbr => {}
    }
    // What the material model leaves, through the layers, textures and uv
    // sets the container keeps: a layer it lacks goes whole, and each
    // remaining binding, base or layer, keeps its map only where textures
    // survive, on a set the container holds.
    for m in s.materials.iter_mut() {
        for layer in MaterialLayer::ALL.into_iter().filter(|&l| !caps.layers.keeps(l)) {
            m.drop_layer(layer);
        }
        for (_, r) in m.texture_slots_mut() {
            *r = keep(&*r);
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

/// The scene without the lights, or the cameras, that a container with
/// `caps` lacks, and with no node referring to one. A node that held only a
/// light or a camera is left an empty node, as the rest of the reduction
/// treats one: kept where the hierarchy is, gone where [`bake`] flattens it.
pub fn drop_lights_and_cameras(s: &mut Scene, caps: &super::scene::Capabilities) {
    if !caps.lights {
        s.lights.clear();
        for n in s.nodes.iter_mut() {
            n.light = None;
        }
    }
    if !caps.cameras {
        s.cameras.clear();
        for n in s.nodes.iter_mut() {
            n.camera = None;
        }
    }
}

/// The scene without the point and line primitives a container with `caps`
/// cannot hold ([`Capabilities::holds`](super::scene::Capabilities::holds)),
/// and what depended on them: a mesh left with no primitive goes, a node that
/// drew only that mesh draws nothing (no mesh, so no skin or morph weights
/// either, and no clip of weights), and the meshes after it are renumbered.
/// The node itself stays, with its transform, children and animation, as any
/// node that draws nothing does. Triangles are never removed.
pub fn strip_topologies(scene: &mut Scene, caps: &super::scene::Capabilities) {
    if scene.meshes.iter().flat_map(|m| &m.primitives).all(|p| caps.holds(p.topology)) {
        return;
    }
    // The meshes this empties: one that was empty already is not ours to drop.
    let mut emptied = vec![false; scene.meshes.len()];
    for (mesh, emptied) in scene.meshes.iter_mut().zip(emptied.iter_mut()) {
        let before = mesh.primitives.len();
        mesh.primitives.retain(|p| caps.holds(p.topology));
        *emptied = before > 0 && mesh.primitives.is_empty();
    }
    let mut renumbered = vec![None; scene.meshes.len()];
    let mut kept = Vec::with_capacity(scene.meshes.len());
    for (i, mesh) in std::mem::take(&mut scene.meshes).into_iter().enumerate() {
        if !emptied[i] {
            renumbered[i] = Some(kept.len());
            kept.push(mesh);
        }
    }
    scene.meshes = kept;
    for node in scene.nodes.iter_mut() {
        if let Some(mesh) = node.mesh {
            node.mesh = renumbered.get(mesh).copied().flatten();
            if node.mesh.is_none() {
                node.skin = None;
                node.weights = None;
            }
        }
    }
    let nodes = &scene.nodes;
    let mut emptied_clips = Vec::with_capacity(scene.animations.len());
    for anim in scene.animations.iter_mut() {
        let before = anim.channels.len();
        anim.channels.retain(|c| c.path != AnimPath::Weights || nodes.get(c.node).is_some_and(|n| n.mesh.is_some()));
        emptied_clips.push(before > 0 && anim.channels.is_empty());
    }
    let mut clips = emptied_clips.into_iter();
    scene.animations.retain(|_| !clips.next().unwrap_or(false));
}

/// A writer's side of [`strip_topologies`]: `prepare` removed every point and
/// line primitive the container cannot hold, so the writer is never given
/// one. A debug build panics when it is, naming `writer` and the topology; a
/// release build writes what it is given.
pub fn debug_assert_holds(writer: &str, scene: &Scene, caps: &super::scene::Capabilities) {
    for p in scene.meshes.iter().flat_map(|m| &m.primitives) {
        let topology = match p.topology {
            Topology::Triangles => "triangles",
            Topology::Lines => "lines",
            Topology::Points => "points",
        };
        debug_assert!(caps.holds(p.topology), "{writer} was given a {topology} primitive; prepare strips what it cannot hold");
    }
}

/// Move every `KHR_texture_transform` into the texture coordinates it
/// transforms, for containers that cannot express one.
///
/// Sampling `T(uv)` from untransformed coordinates and sampling `uv` through
/// the transform are the same picture, so this is lossless whenever a UV set
/// feeds textures under one transform — what every exporter produces. When a
/// material applies two DIFFERENT transforms to one set, only the first in
/// priority order (base colour, emissive, normal, metallic-roughness,
/// occlusion, then the layers' maps in `Material::texture_refs` order) can be
/// baked, and the others are left untransformed: a loss the caller sees
/// through `scene::losses`.
pub fn bake_texture_transforms(s: &mut Scene) {
    use super::scene::UvTransform;
    // The base maps claim first, in this order; every other slot follows.
    const FIRST: [TextureSlot; 5] =
        [TextureSlot::BaseColor, TextureSlot::Emissive, TextureSlot::Normal, TextureSlot::MetallicRoughness, TextureSlot::Occlusion];
    let materials = s.materials.clone();
    for mesh in s.meshes.iter_mut() {
        for p in mesh.primitives.iter_mut() {
            let Some(m) = p.material.and_then(|m| materials.get(m)) else { continue };
            // The first reference to a UV set CLAIMS it, transformed or not.
            // An untransformed base-colour texture on set 0 must keep set 0
            // untransformed even when the emissive map applies an offset to
            // it — baking the emissive offset would shift the base colour.
            let mut claimed: Vec<(u32, Option<UvTransform>)> = Vec::new();
            let mut order: Vec<(TextureSlot, &super::scene::TextureRef)> = m.texture_refs().collect();
            order.sort_by_key(|(slot, _)| FIRST.iter().position(|s| s == slot).unwrap_or(FIRST.len()));
            for (_, r) in order {
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
        for (_, r) in m.texture_refs_mut() {
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
        for (_, r) in m.texture_slots_mut() {
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

/// Each primitive's material base colour multiplied into its vertex colours,
/// or taken as them by a primitive with none, and the materials dropped: what
/// a container whose only material is vertex colour holds of them. With
/// `every`, once any primitive is coloured every one is (white where it was
/// untinted), as a merge into one mesh needs; otherwise an untinted primitive
/// without colours stays without.
fn bake_base_colors(s: &mut Scene, every: bool) {
    let materials = std::mem::take(&mut s.materials);
    let base_of = |p: &super::scene::Primitive| p.material.and_then(|m| materials.get(m)).map_or([1.0; 4], |m| m.base_color);
    let all = every && s.meshes.iter().flat_map(|m| &m.primitives).any(|p| p.colors.is_some() || base_of(p) != [1.0; 4]);
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
        let b = base_of(p);
        p.material = None;
        match p.colors.as_mut() {
            Some(cs) => {
                for c in cs.iter_mut() {
                    for k in 0..4 {
                        c[k] *= b[k];
                    }
                }
            }
            None if all || b != [1.0; 4] => p.colors = Some(vec![b; p.positions.len()]),
            None => {}
        }
    }
}

/// The channel means of every image a material's metallic-roughness map
/// reads (`normalize::channel_means`), each decoded once however many
/// materials share it.
fn mr_image_means(s: &Scene) -> std::collections::HashMap<usize, Option<[f32; 4]>> {
    let mut out = std::collections::HashMap::new();
    for m in &s.materials {
        let Some(image) = m.metallic_roughness_texture.as_ref().and_then(|t| s.textures.get(t.texture)).map(|t| t.image) else {
            continue;
        };
        out.entry(image).or_insert_with(|| s.images.get(image).and_then(|img| super::normalize::channel_means(&img.data, img.format)));
    }
    out
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
    fn a_layer_texture_transform_is_baked() {
        // The set's one map is the sheen colour's, shifted and stretched: the
        // transform moves into the coordinates, and the binding keeps none.
        use super::super::scene::Sheen;
        let mut p = tri();
        p.uvs = vec![vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]];
        p.material = Some(0);
        let mut s = scene_with(p, Transform::default());
        let t = UvTransform { offset: [0.5, 0.25], rotation: 0.0, scale: [2.0, 1.0] };
        let map = |transform| Some(TextureRef { texture: 0, uv_set: 0, transform });
        s.materials.push(Material { sheen: Some(Sheen { color: [1.0; 3], color_texture: map(Some(t)), ..Default::default() }), ..Default::default() });
        bake_texture_transforms(&mut s);
        assert_eq!(s.meshes[0].primitives[0].uvs, vec![vec![[0.5, 0.25], [2.5, 0.25], [0.5, 1.25]]]);
        assert_eq!(s.materials[0].sheen.as_ref().map(|sh| sh.color_texture.clone()), Some(map(None)));
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

    fn stepped_clip() -> (Scene, Vec<f32>) {
        let mut s = scene_with(tri(), Transform::Trs { translation: [0.0; 3], rotation: math::QUAT_IDENTITY, scale: [1.0; 3] });
        s.nodes[0].name = Some("stepper".into());
        let times: Vec<f32> = (0..300).map(|k| k as f32 / 30.0).collect();
        s.animations.push(Animation {
            name: Some("steps".into()),
            channels: vec![Channel {
                node: 0,
                path: AnimPath::Translation,
                interpolation: Interpolation::Step,
                values: (0..300).flat_map(|k| [k as f32, 0.0, 0.0]).collect(),
                times: times.clone(),
            }],
        });
        (s, times)
    }

    /// 2026-10-12 review, finding 18: `dedup_by` keeps the earlier of two
    /// times under a microsecond apart, so a grid time one ULP before a source
    /// key replaced the key itself.
    #[test]
    fn resampling_keeps_every_source_key_time_exactly() {
        let (s, times) = stepped_clip();
        let grid = resample_times(&s, 0, 30.0);
        let lost: Vec<f32> = times.iter().copied().filter(|t| !grid.contains(t)).collect();
        assert!(lost.is_empty(), "{} of 300 keys replaced by a grid time beside them, e.g. {:?}", lost.len(), &lost[..lost.len().min(4)]);
        assert!(grid.windows(2).all(|w| w[1] - w[0] >= 1e-6), "no two keys at one instant");
    }

    /// The symptom: STEP sampled just before its key returns the previous
    /// value, so in the containers that resample (FBX, Collada) a step fired
    /// a frame late. Halfway between keys k and k+1 the clip holds key k.
    #[test]
    fn a_step_lands_on_its_key_in_every_animated_container() {
        let (s, times) = stepped_clip();
        use super::super::MeshFormat;
        for to in [MeshFormat::Fbx, MeshFormat::Dae, MeshFormat::Usdz, MeshFormat::Usdc, MeshFormat::Glb] {
            let w = super::super::write_scene(&s, to, super::super::WriteOptions::default()).unwrap();
            let back = super::super::read_scene(&w.package.files[0].bytes, to, &super::super::package::NoSidecars).unwrap().scene;
            let late: Vec<usize> = (0..times.len() - 1)
                .filter(|&k| {
                    let t = (times[k] + times[k + 1]) / 2.0;
                    let x = world_primitives(&back, &sample_pose(&back, 0, t)).unwrap()[0].positions[0][0];
                    (x - k as f32).abs() > 1e-3
                })
                .collect();
            assert!(late.is_empty(), "{to}: {} of 299 steps not held at their key, e.g. key {:?}", late.len(), &late[..late.len().min(4)]);
        }
    }

    /// 2026-10-12 review, finding 7: keys at ±2e38 s span more than f32 can
    /// hold, so the grid's first time was `inf × 0` = NaN, and sampling at NaN
    /// indexed past the last key: glb→fbx and glb→dae panicked, which on the
    /// poll path threw away every file the vendor returned.
    #[test]
    fn key_times_spanning_past_f32_never_panic_a_writer() {
        let mut s = scene_with(tri(), Transform::Trs { translation: [0.0; 3], rotation: math::QUAT_IDENTITY, scale: [1.0; 3] });
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let ch = Channel {
            node: 0,
            path: AnimPath::Rotation,
            interpolation: Interpolation::Linear,
            times: vec![-2e38, 0.0, 2e38],
            values: vec![0.0, 0.0, 0.0, 1.0, 0.0, h, 0.0, h, 0.0, 1.0, 0.0, 0.0],
        };
        assert_eq!(sample_channel(&ch, f32::NAN, 4), [0.0, 0.0, 0.0, 1.0], "NaN holds the first key");
        s.animations.push(Animation { name: Some("wide".into()), channels: vec![ch] });
        let grid = resample_times(&s, 0, 30.0);
        assert!(grid.iter().all(|t| t.is_finite()) && grid.windows(2).all(|w| w[1] > w[0]), "{:?}", &grid[..grid.len().min(4)]);
        for to in super::super::MeshFormat::ALL {
            let _ = super::super::write_scene(&s, to, super::super::WriteOptions::default());
        }
    }

    /// N1 (2026-11-01): folding a dropped metallic-roughness map decoded its
    /// image once per material that used it. It is decoded once per image.
    #[test]
    fn a_metallic_roughness_map_shared_by_materials_is_decoded_once() {
        let mut s = super::super::fixtures::textured();
        s.textures.push(super::super::scene::Texture { name: None, image: 0, sampler: Default::default() });
        let mr = s.textures.len() - 1;
        for m in s.materials.iter_mut() {
            m.metallic_roughness_texture = Some(super::super::scene::TextureRef { texture: mr, uv_set: 0, transform: None });
        }
        assert!(s.materials.len() >= 3);
        let means = mr_image_means(&s);
        assert_eq!(means.len(), 1, "one image, decoded once");
        assert!(means[&0].is_some());
    }

    // ─── the uv sets a container keeps: the ones textures sample ────────────

    use super::super::package::MemorySidecars;
    use super::super::scene::{Loss, LossKind, Material, TextureRef, TextureSlot, UvTransform};
    use super::super::{fixtures, read_scene, write_scene, MeshFormat, WriteOptions};

    /// A unit quad with `sets` distinct uv sets: set `i` maps the corners to
    /// `((u + i) / 8, (v + 7 − i) / 8)`, exact in every text format.
    fn quad_with_sets(sets: usize, material: Option<usize>) -> Primitive {
        let corners: [([f32; 2], [f32; 2]); 4] =
            [([0.0, 0.0], [0.0, 1.0]), ([1.0, 0.0], [1.0, 1.0]), ([1.0, 1.0], [1.0, 0.0]), ([0.0, 1.0], [0.0, 0.0])];
        Primitive {
            positions: corners.iter().map(|(p, _)| [p[0], p[1], 0.0]).collect(),
            normals: Some(vec![[0.0, 0.0, 1.0]; 4]),
            uvs: (0..sets)
                .map(|i| corners.iter().map(|(_, [u, v])| [(u + i as f32) / 8.0, (v + 7.0 - i as f32) / 8.0]).collect())
                .collect(),
            indices: vec![0, 1, 2, 0, 2, 3],
            material,
            ..Default::default()
        }
    }

    /// `prims` in one mesh on one node, over `materials`, with
    /// `fixtures::multiuv`'s two PNG textures to bind.
    fn uv_scene(materials: Vec<Material>, prims: Vec<Primitive>) -> Scene {
        let base = fixtures::multiuv();
        let mut s = Scene { images: base.images, textures: base.textures, materials, ..Default::default() };
        s.meshes.push(SceneMesh { primitives: prims, ..Default::default() });
        s.add_root(Node { mesh: Some(0), ..Default::default() });
        s
    }

    fn on(texture: usize, uv_set: u32) -> Option<TextureRef> {
        Some(TextureRef { texture, uv_set, transform: None })
    }

    /// Written as `fmt`, then read back through the package's own files.
    fn through(s: &Scene, fmt: MeshFormat, opts: WriteOptions) -> (Scene, Vec<Loss>) {
        let w = write_scene(s, fmt, opts).unwrap_or_else(|e| panic!("write {fmt}: {e}"));
        let side = MemorySidecars { files: &w.package.files };
        let back = read_scene(&w.package.files[0].bytes, fmt, &side).unwrap_or_else(|e| panic!("read {fmt}: {e}")).scene;
        (back, w.losses)
    }

    /// A set's distinct coordinates, sorted: the same whatever vertex order or
    /// welding a reader chooses, and different for every set `quad_with_sets`
    /// and `fixtures::multiuv` make.
    fn values(set: &[[f32; 2]]) -> Vec<[i64; 2]> {
        let mut v: Vec<[i64; 2]> = set.iter().map(|uv| uv.map(|x| (f64::from(x) * 65536.0).round() as i64)).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    fn slot_set(r: &Option<TextureRef>) -> Option<u32> {
        r.as_ref().map(|r| r.uv_set)
    }

    #[test]
    fn degrade_keeps_sampled_sets() {
        // A GLB whose base colour samples TEXCOORD_1, into an OBJ package (one
        // set). Cut to its first set, the primitive kept set 0, which nothing
        // samples, and the texture went with set 1.
        let material = Material { base_color_texture: on(0, 1), metallic: 0.0, ..Default::default() };
        let s = uv_scene(vec![material], vec![quad_with_sets(2, Some(0))]);
        let package = WriteOptions { self_contained: false, ..Default::default() };
        let (glb, _) = through(&s, MeshFormat::Glb, package);
        assert_eq!(slot_set(&glb.materials[0].base_color_texture), Some(1), "the GLB samples TEXCOORD_1");
        let glb_set1 = values(&glb.meshes[0].primitives[0].uvs[1]);
        let (obj, losses) = through(&glb, MeshFormat::Obj, package);
        assert!(!losses.iter().any(|l| l.kind == LossKind::Textures), "{losses:?}");
        let p = &obj.meshes[0].primitives[0];
        let m = p.material.and_then(|m| obj.materials.get(m)).expect("the OBJ's material");
        assert_eq!(slot_set(&m.base_color_texture), Some(0), "the texture survives, on the OBJ's one set");
        assert_eq!(p.uvs.len(), 1);
        assert_eq!(values(&p.uvs[0]), glb_set1, "which is the GLB's set 1");
    }

    #[test]
    fn a_phong_target_keeps_the_set_of_a_map_it_keeps() {
        // An OBJ package (Phong, one set) keeps base-colour, normal and
        // emissive maps; it folds metallic-roughness into factors and drops
        // occlusion. With the emissive map on set 0 and occlusion (or
        // metallic-roughness) on set 1, set 0 is the one to keep: putting the
        // dropped map's set first would cost the emissive texture.
        let package = WriteOptions { self_contained: false, ..Default::default() };
        for dropped in [TextureSlot::Occlusion, TextureSlot::MetallicRoughness] {
            let ctx = format!("{} on set 1", dropped.as_str());
            let mut material = Material { emissive_texture: on(0, 0), emissive: [1.0; 3], metallic: 0.0, ..Default::default() };
            match dropped {
                TextureSlot::Occlusion => material.occlusion_texture = on(1, 1),
                _ => material.metallic_roughness_texture = on(1, 1),
            }
            let s = uv_scene(vec![material], vec![quad_with_sets(2, Some(0))]);
            let source = &s.meshes[0].primitives[0].uvs;
            let (obj, losses) = through(&s, MeshFormat::Obj, package);
            assert!(!losses.iter().any(|l| l.kind == LossKind::Textures), "{ctx}: {losses:?}");
            let p = &obj.meshes[0].primitives[0];
            let m = p.material.and_then(|m| obj.materials.get(m)).expect("the OBJ's material");
            assert_eq!(slot_set(&m.emissive_texture), Some(0), "{ctx}: the emissive texture survives, on the OBJ's one set");
            assert_eq!(p.uvs.len(), 1, "{ctx}");
            assert_eq!(values(&p.uvs[0]), values(&source[0]), "{ctx}: which is the source's set 0");
        }
    }

    #[test]
    fn shared_materials_share_the_remap() {
        // The remap belongs to the material: both primitives drawing it keep
        // the same two sets, the fixture's 3 (base colour) and 1 (occlusion).
        let s = fixtures::multiuv_shared();
        let source = &s.meshes[0].primitives[0].uvs;
        let (back, _) = through(&s, MeshFormat::Usdz, WriteOptions::default());
        let prims: Vec<&Primitive> = back.meshes.iter().flat_map(|m| &m.primitives).collect();
        assert_eq!(prims.len(), 2);
        for (i, p) in prims.iter().enumerate() {
            let m = &back.materials[p.material.expect("a material")];
            assert_eq!((slot_set(&m.base_color_texture), slot_set(&m.occlusion_texture)), (Some(0), Some(1)), "primitive {i}");
            assert_eq!(p.uvs.len(), 2, "primitive {i}");
            assert_eq!(values(&p.uvs[0]), values(&source[3]), "primitive {i}: set 0 is the fixture's set 3");
            assert_eq!(values(&p.uvs[1]), values(&source[1]), "primitive {i}: set 1 is the fixture's set 1");
        }
    }

    #[test]
    fn a_material_sampling_three_sets_loses_one_in_usdz() {
        // USDZ holds two sets and the material samples three: the first two in
        // first-use order (base colour, ..., occlusion, emissive) stay, and the
        // emissive map goes with the third. The second case puts them on sets
        // out of index order, which keeping sets 0 and 1 gets wrong.
        for (base, occlusion, emissive) in [(0u32, 1u32, 2u32), (2, 0, 1)] {
            let ctx = format!("base colour on {base}, occlusion on {occlusion}, emissive on {emissive}");
            let material = Material {
                base_color_texture: on(0, base),
                occlusion_texture: on(1, occlusion),
                emissive_texture: on(0, emissive),
                emissive: [1.0; 3],
                metallic: 0.0,
                ..Default::default()
            };
            let s = uv_scene(vec![material], vec![quad_with_sets(4, Some(0))]);
            let source = &s.meshes[0].primitives[0].uvs;
            let (back, losses) = through(&s, MeshFormat::Usdz, WriteOptions::default());
            let p = &back.meshes[0].primitives[0];
            let m = &back.materials[p.material.expect("a material")];
            assert_eq!(slot_set(&m.base_color_texture), Some(0), "{ctx}");
            assert_eq!(slot_set(&m.occlusion_texture), Some(1), "{ctx}");
            assert!(m.emissive_texture.is_none(), "{ctx}: the emissive map samples the third set");
            assert_eq!(p.uvs.len(), 2, "{ctx}");
            assert_eq!(values(&p.uvs[0]), values(&source[base as usize]), "{ctx}: set 0");
            assert_eq!(values(&p.uvs[1]), values(&source[occlusion as usize]), "{ctx}: set 1");
            let lost: Vec<&Loss> = losses.iter().filter(|l| l.kind == LossKind::Textures).collect();
            assert_eq!(lost.len(), 1, "{ctx}: {losses:?}");
            assert_eq!(lost[0].what, "1 texture binding(s) on uv set 2 or later", "{ctx}");
        }
    }

    #[test]
    fn a_materialless_primitive_keeps_its_first_sets() {
        // Nothing samples its sets, so it keeps the first two, as before, even
        // beside a primitive whose material is remapped in the same mesh.
        let material = Material { base_color_texture: on(0, 3), metallic: 0.0, ..Default::default() };
        let s = uv_scene(vec![material], vec![quad_with_sets(4, None), quad_with_sets(4, Some(0))]);
        let source = &s.meshes[0].primitives[0].uvs;
        let d = degrade(&s, &MeshFormat::Usdz.capabilities(WriteOptions::default())).unwrap();
        let (plain, textured) = (&d.meshes[0].primitives[0], &d.meshes[0].primitives[1]);
        assert_eq!(plain.material, None);
        assert_eq!(plain.uvs, source[..2].to_vec(), "the primitive without a material keeps sets 0 and 1");
        assert_eq!(textured.uvs, vec![source[3].clone(), source[0].clone()], "its neighbour keeps its sampled set 3 first");
        assert_eq!(slot_set(&d.materials[0].base_color_texture), Some(0));
    }

    #[test]
    fn a_transform_on_a_remapped_set_bakes_into_its_new_index() {
        // USDZ bakes texture transforms into the coordinates. The base colour's
        // set 2 moves to index 0, and its transform is baked there; the
        // untransformed occlusion set 0 moves to index 1 as it is.
        let t = UvTransform { offset: [0.5, 0.25], rotation: 0.0, scale: [2.0, 0.5] };
        let material = Material {
            base_color_texture: Some(TextureRef { texture: 0, uv_set: 2, transform: Some(t) }),
            occlusion_texture: on(1, 0),
            metallic: 0.0,
            ..Default::default()
        };
        let s = uv_scene(vec![material], vec![quad_with_sets(3, Some(0))]);
        let source = &s.meshes[0].primitives[0].uvs;
        let caps = MeshFormat::Usdz.capabilities(WriteOptions::default());
        assert!(caps.textures && !caps.texture_transforms);
        let d = degrade(&s, &caps).unwrap();
        let baked: Vec<[f32; 2]> = source[2].iter().map(|uv| t.apply(*uv)).collect();
        assert_eq!(d.meshes[0].primitives[0].uvs, vec![baked, source[0].clone()]);
        assert_eq!(d.materials[0].base_color_texture, Some(TextureRef { texture: 0, uv_set: 0, transform: None }));
        assert_eq!(d.materials[0].occlusion_texture, Some(TextureRef { texture: 1, uv_set: 1, transform: None }));
    }

    #[test]
    fn two_materials_on_one_mesh_remap_independently() {
        // Each primitive follows its own material's order: A samples set 3,
        // B sets 2 (base colour) and 1 (occlusion), on one mesh.
        let a = Material { base_color_texture: on(0, 3), metallic: 0.0, ..Default::default() };
        let b = Material { base_color_texture: on(0, 2), occlusion_texture: on(1, 1), metallic: 0.0, ..Default::default() };
        let s = uv_scene(vec![a, b], vec![quad_with_sets(4, Some(0)), quad_with_sets(4, Some(1))]);
        let source = &s.meshes[0].primitives[0].uvs;
        let d = degrade(&s, &MeshFormat::Usdz.capabilities(WriteOptions::default())).unwrap();
        assert_eq!(d.meshes[0].primitives[0].uvs, vec![source[3].clone(), source[0].clone()], "A: its set 3, then set 0");
        assert_eq!(d.meshes[0].primitives[1].uvs, vec![source[2].clone(), source[1].clone()], "B: its sets 2 and 1");
        assert_eq!(slot_set(&d.materials[0].base_color_texture), Some(0));
        assert_eq!((slot_set(&d.materials[1].base_color_texture), slot_set(&d.materials[1].occlusion_texture)), (Some(0), Some(1)));
        // And the USDZ carries both, each primitive its own Mesh.
        let (back, losses) = through(&s, MeshFormat::Usdz, WriteOptions::default());
        assert!(!losses.iter().any(|l| l.kind == LossKind::Textures), "{losses:?}");
        let prims: Vec<&Primitive> = back.meshes.iter().flat_map(|m| &m.primitives).collect();
        assert_eq!(prims.len(), 2);
        for p in prims {
            let m = &back.materials[p.material.expect("a material")];
            let want = if m.occlusion_texture.is_some() { [2, 1] } else { [3, 0] };
            assert_eq!(slot_set(&m.base_color_texture), Some(0));
            assert_eq!([values(&p.uvs[0]), values(&p.uvs[1])], want.map(|i| values(&source[i])), "the sets of the material on {want:?}");
        }
    }

    #[test]
    fn a_lone_texture_on_set_five_moves_to_set_zero() {
        // Its one texture on set 5 of six, into USDZ's two: set 5 becomes set
        // 0, and only the four sets nothing samples are reported lost.
        let material = Material { base_color_texture: on(0, 5), metallic: 0.0, ..Default::default() };
        let s = uv_scene(vec![material], vec![quad_with_sets(6, Some(0))]);
        let source = &s.meshes[0].primitives[0].uvs;
        let w = write_scene(&s, MeshFormat::Usdz, WriteOptions::default()).unwrap();
        let reported: Vec<(LossKind, &str)> = w.losses.iter().map(|l| (l.kind, l.what.as_str())).collect();
        assert_eq!(reported, vec![(LossKind::Uvs, "4 of 6 uv set(s)")], "{:?}", w.losses);
        let d = MeshFormat::Usdz.prepare(&s, WriteOptions::default()).unwrap();
        assert_eq!(d.meshes[0].primitives[0].uvs, vec![source[5].clone(), source[0].clone()]);
        assert_eq!(slot_set(&d.materials[0].base_color_texture), Some(0));
    }

    #[test]
    fn ordering_sampled_sets_first_drops_nothing() {
        // The always-reorder form 3MF uses (and Collada and FBX will): every
        // set kept, the sampled ones first in first-use order, the rest by
        // index; the bindings follow. With every slot kept, multiuv's base
        // colour (set 3) and occlusion (set 1) lead.
        let every = [
            TextureSlot::BaseColor,
            TextureSlot::MetallicRoughness,
            TextureSlot::Normal,
            TextureSlot::Occlusion,
            TextureSlot::Emissive,
        ];
        let mut s = fixtures::multiuv();
        let source = s.meshes[0].primitives[0].uvs.clone();
        order_sampled_sets_first(&mut s, &every);
        assert_eq!(s.meshes[0].primitives[0].uvs, [3, 1, 0, 2].map(|i| source[i].clone()).to_vec());
        assert_eq!((slot_set(&s.materials[0].base_color_texture), slot_set(&s.materials[0].occlusion_texture)), (Some(0), Some(1)));
        let once = s.clone();
        order_sampled_sets_first(&mut s, &every);
        assert_eq!(s, once, "a second pass changes nothing");
        // The slot list decides the order: with base colour alone (3MF's
        // list), the occlusion map's set is not put first.
        let mut t = fixtures::multiuv();
        order_sampled_sets_first(&mut t, &[TextureSlot::BaseColor]);
        assert_eq!(t.meshes[0].primitives[0].uvs, [3, 0, 1, 2].map(|i| source[i].clone()).to_vec());
        assert_eq!(slot_set(&t.materials[0].base_color_texture), Some(0));
    }

    #[test]
    fn a_dropped_map_keeps_its_own_coordinates() {
        // The slot list decides only the order; the permutation renumbers
        // every binding. 3MF orders by base colour alone, and the occlusion
        // map it drops later still reads the fixture's set 1, not whichever
        // set moved into index 1.
        let mut s = fixtures::multiuv();
        let source = s.meshes[0].primitives[0].uvs.clone();
        order_sampled_sets_first(&mut s, &[TextureSlot::BaseColor]);
        let p = &s.meshes[0].primitives[0];
        let reads = |r: &Option<TextureRef>| r.as_ref().and_then(|r| p.uvs.get(r.uv_set as usize)).cloned();
        assert_eq!(reads(&s.materials[0].base_color_texture), Some(source[3].clone()), "base colour");
        assert_eq!(reads(&s.materials[0].occlusion_texture), Some(source[1].clone()), "occlusion");
        // In an intermediate scene too: after Collada's reorder (base colour
        // and emissive), a normal map still reads its own set, as the Phong
        // plan `degrade` makes next needs.
        let material = Material { base_color_texture: on(0, 2), normal_texture: on(1, 1), metallic: 0.0, ..Default::default() };
        let mut c = uv_scene(vec![material], vec![quad_with_sets(4, Some(0))]);
        let source = c.meshes[0].primitives[0].uvs.clone();
        order_sampled_sets_first(&mut c, &[TextureSlot::BaseColor, TextureSlot::Emissive]);
        let p = &c.meshes[0].primitives[0];
        let reads = |r: &Option<TextureRef>| r.as_ref().and_then(|r| p.uvs.get(r.uv_set as usize)).cloned();
        assert_eq!(p.uvs, [2, 0, 1, 3].map(|i| source[i].clone()).to_vec());
        assert_eq!(reads(&c.materials[0].base_color_texture), Some(source[2].clone()), "base colour");
        assert_eq!(reads(&c.materials[0].normal_texture), Some(source[1].clone()), "normal map");
    }

    #[test]
    fn a_huge_uv_set_index_is_renumbered_at_once() {
        // The glTF reader takes any u32 texCoord, so a hostile file can bind
        // a map to set 4294967295. Renumbering it must not walk every index
        // below it.
        let hostile = || Material {
            base_color_texture: on(0, 0),
            normal_texture: on(0, 1),
            occlusion_texture: on(1, u32::MAX),
            metallic: 0.0,
            ..Default::default()
        };
        let package = WriteOptions { self_contained: false, ..Default::default() };
        // b) On a material no primitive draws, which nothing makes
        //    degenerate: 3MF's reorder plans it, and so does the OBJ cut (its
        //    normal map is on set 1). Every binding is renumbered, and the
        //    occlusion map stays on its own set.
        let unused = uv_scene(vec![Material::default(), hostile()], vec![quad_with_sets(2, Some(0))]);
        let mut ordered = unused.clone();
        order_sampled_sets_first(&mut ordered, &[TextureSlot::BaseColor]);
        assert_eq!(slot_set(&ordered.materials[1].occlusion_texture), Some(u32::MAX));
        assert_eq!(ordered, unused, "base colour on 0 and normal on 1 are already first");
        for fmt in [MeshFormat::ThreeMf, MeshFormat::Obj, MeshFormat::Dae] {
            write_scene(&unused, fmt, package).unwrap_or_else(|e| panic!("{fmt}: {e}"));
        }
        // a) On a primitive with two sets, which lacks set 4294967295: the
        //    material is degenerate, so nothing is planned for it, for any slot
        //    list, and the output is the first-k cut's.
        let drawn = uv_scene(vec![hostile()], vec![quad_with_sets(2, Some(0))]);
        let colour_only: &[TextureSlot] = &[TextureSlot::BaseColor];
        let phong = kept_texture_slots(&MeshFormat::Obj.capabilities(package));
        for slots in [colour_only, phong.as_slice()] {
            assert_eq!(UvSetPlan::new(&drawn, 0, slots).orders, vec![None], "{slots:?}");
        }
        let mut ordered = drawn.clone();
        order_sampled_sets_first(&mut ordered, colour_only);
        assert_eq!(ordered, drawn);
        for fmt in [MeshFormat::ThreeMf, MeshFormat::Obj, MeshFormat::Dae] {
            write_scene(&drawn, fmt, package).unwrap_or_else(|e| panic!("{fmt}: {e}"));
        }
    }

    #[test]
    fn a_material_sampling_a_set_its_primitive_lacks_keeps_the_first_sets() {
        // A material on set 3 drawn by a primitive with four sets and one with
        // two: a scene glTF rejects and `Scene::validate` allows. It is not
        // remapped for either, by `degrade`, `scene::losses` or the reorder:
        // both keep their first sets, and the binding is dropped and reported.
        let material = Material { base_color_texture: on(0, 3), metallic: 0.0, ..Default::default() };
        let s = uv_scene(vec![material], vec![quad_with_sets(4, Some(0)), quad_with_sets(2, Some(0))]);
        s.validate().unwrap();
        let source = &s.meshes[0].primitives[0].uvs;
        let caps = MeshFormat::Usdz.capabilities(WriteOptions::default());
        let d = degrade(&s, &caps).unwrap();
        assert_eq!(d.meshes[0].primitives[0].uvs, source[..2].to_vec());
        assert!(d.materials[0].base_color_texture.is_none());
        let reported = super::super::scene::losses(&s, &caps, "usdz");
        assert!(reported.iter().any(|l| l.what == "1 texture binding(s) on uv set 2 or later"), "{reported:?}");
        let mut ordered = s.clone();
        order_sampled_sets_first(&mut ordered, &kept_texture_slots(&caps));
        assert_eq!(ordered, s);
    }

    // ─── material layers ────────────────────────────────────────────────────

    #[test]
    fn pruning_remaps_a_layer_texture() {
        // Texture 0 is used by nothing and goes; texture 1, used by the
        // clearcoat map alone, and texture 2, the base colour's, move down
        // one, each still reaching its own image.
        use super::super::scene::{Clearcoat, Image, ImageFormat, Texture};
        let mut s = scene_with(tri(), Transform::default());
        for (i, name) in ["unused", "coat", "base"].into_iter().enumerate() {
            s.images.push(Image { name: Some(name.into()), format: ImageFormat::Png, data: vec![i as u8] });
            s.textures.push(Texture { name: None, image: i, sampler: Default::default() });
        }
        s.materials.push(Material { base_color_texture: on(2, 0), clearcoat: Some(Clearcoat { factor: 1.0, texture: on(1, 0), ..Default::default() }), ..Default::default() });
        prune_unused_textures(&mut s);
        let image = |r: &Option<TextureRef>| r.as_ref().and_then(|r| s.textures.get(r.texture)).and_then(|t| s.images.get(t.image)).and_then(|i| i.name.clone());
        let m = &s.materials[0];
        let coat = m.clearcoat.as_ref().map(|c| &c.texture).expect("the clearcoat layer");
        assert_eq!((coat.as_ref().map(|r| r.texture), image(coat)), (Some(0), Some("coat".into())));
        assert_eq!((m.base_color_texture.as_ref().map(|r| r.texture), image(&m.base_color_texture)), (Some(1), Some("base".into())));
        assert_eq!(s.textures.len(), 2);
    }

    #[test]
    fn degrade_reduces_layer_textures() {
        // Into two sets, keeping clearcoat but not sheen: the base colour's set
        // 2 comes first, then the clearcoat map's set 0, renumbered with it;
        // the clearcoat roughness map's set 1 goes, and the map with it; the
        // sheen goes whole.
        use super::super::scene::{Capabilities, Clearcoat, LayerSupport, Sheen};
        let material = Material {
            base_color_texture: on(0, 2),
            clearcoat: Some(Clearcoat { factor: 1.0, texture: on(1, 0), roughness_texture: on(1, 1), ..Default::default() }),
            sheen: Some(Sheen { color: [1.0; 3], color_texture: on(1, 0), ..Default::default() }),
            metallic: 0.0,
            ..Default::default()
        };
        let s = uv_scene(vec![material], vec![quad_with_sets(3, Some(0))]);
        let source = &s.meshes[0].primitives[0].uvs;
        let caps = Capabilities { uv_sets: 2, layers: LayerSupport { clearcoat: true, ..LayerSupport::none() }, ..Capabilities::full() };
        let d = degrade(&s, &caps).unwrap();
        assert_eq!(d.meshes[0].primitives[0].uvs, vec![source[2].clone(), source[0].clone()]);
        let m = &d.materials[0];
        assert_eq!(slot_set(&m.base_color_texture), Some(0));
        let coat = m.clearcoat.as_ref().expect("the clearcoat layer is kept");
        assert_eq!((slot_set(&coat.texture), slot_set(&coat.roughness_texture)), (Some(1), None));
        assert!(m.sheen.is_none(), "the container has no sheen layer");
        // Where no texture survives, the layer stays, untextured.
        let bare = degrade(&s, &Capabilities { textures: false, ..caps }).unwrap();
        assert_eq!(bare.materials[0].clearcoat, Some(Clearcoat { factor: 1.0, ..Default::default() }));
        assert!(bare.textures.is_empty() && bare.images.is_empty());
    }

    /// `degrade` drops the lights and cameras a container lacks, each kind
    /// by its own flag, with every node's reference to one. A node that held
    /// one is then an empty node: kept where the hierarchy is, baked away
    /// where it is not.
    #[test]
    fn degrade_drops_the_lights_and_cameras_a_container_lacks() {
        use super::super::scene::{Camera, Capabilities, Light, LightKind, Projection};
        let mut s = scene_with(tri(), Transform::default());
        s.nodes[0].name = Some("drawn".into());
        s.lights.push(Light { name: Some("lamp".into()), kind: LightKind::Point, color: [1.0; 3], intensity: 20.0, range: None });
        s.cameras.push(Camera { name: Some("eye".into()), projection: Projection::Perspective { yfov: 0.8, aspect: None, znear: 0.125, zfar: None } });
        s.add_root(Node { name: Some("lamp_node".into()), light: Some(0), ..Default::default() });
        s.add_root(Node { name: Some("eye_node".into()), camera: Some(0), ..Default::default() });
        let full = Capabilities::full();
        assert_eq!(degrade(&s, &full).unwrap(), s, "a container that keeps both keeps them where they are");
        let unlit = degrade(&s, &Capabilities { lights: false, ..full }).unwrap();
        assert!(unlit.lights.is_empty() && unlit.nodes.iter().all(|n| n.light.is_none()), "{unlit:?}");
        assert_eq!((unlit.nodes.len(), unlit.cameras.len(), unlit.nodes[2].camera), (3, 1, Some(0)), "the lamp's node stays, empty");
        let blind = degrade(&s, &Capabilities { cameras: false, ..full }).unwrap();
        assert!(blind.cameras.is_empty() && blind.nodes.iter().all(|n| n.camera.is_none()), "{blind:?}");
        assert_eq!((blind.nodes.len(), blind.lights.len(), blind.nodes[1].light), (3, 1, Some(0)), "the camera's node stays, empty");
        let flat = degrade(&s, &Capabilities { hierarchy: false, lights: false, cameras: false, ..full }).unwrap();
        let names: Vec<Option<&str>> = flat.nodes.iter().map(|n| n.name.as_deref()).collect();
        assert_eq!((names, flat.lights.len(), flat.cameras.len()), (vec![Some("drawn")], 0, 0), "baked, only what draws stays");
    }

    /// glTF's row keeps everything the scene holds, so its `prepare` returns
    /// the scene unchanged: its light and camera, a texture no material
    /// uses, and the tangents of a primitive without uvs. `degrade` would
    /// drop the texture and the tangents, so `prepare` must not run it.
    #[test]
    fn gltf_prepare_keeps_what_degrade_would_drop() {
        use super::super::scene::{Camera, Image, ImageFormat, Light, LightKind, Projection, Texture};
        use super::super::{MeshFormat, WriteOptions};
        let mut s = scene_with(Primitive { tangents: Some(vec![[1.0, 0.0, 0.0, 1.0]; 3]), ..tri() }, Transform::default());
        s.images.push(Image { name: None, format: ImageFormat::Png, data: super::super::fixtures::encode_png(1, 1, &[255; 4]) });
        s.textures.push(Texture { name: None, image: 0, sampler: Default::default() });
        s.lights.push(Light { name: None, kind: LightKind::Directional, color: [1.0; 3], intensity: 3.0, range: None });
        s.cameras.push(Camera { name: None, projection: Projection::Orthographic { xmag: 1.0, ymag: 1.0, znear: 0.0, zfar: 10.0 } });
        (s.nodes[0].light, s.nodes[0].camera) = (Some(0), Some(0));
        let opts = WriteOptions::default();
        for fmt in [MeshFormat::Glb, MeshFormat::Gltf] {
            assert_eq!(fmt.prepare(&s, opts).unwrap(), s, "{fmt}");
            assert_ne!(degrade(&s, &fmt.capabilities(opts)).unwrap(), s, "{fmt}: degrade is no identity here, so prepare must not run it");
        }
    }

    /// Ruling R19 (Review Focus 2): a light or camera hung from a sheared
    /// run of static transforms keeps its eye and the way it faces through
    /// [`bake_shear`], at rest and while the run's animated ancestor turns.
    /// It moves onto a new node with its old node's name, hung from that
    /// ancestor and framed as the run leaves it, with its up squared to the
    /// way it faces; the old node, renamed `eye_node_shear` so that the name
    /// stays the light's and camera's alone, keeps its children. A camera
    /// on an animated node under a shear turns by its own keys, so no node
    /// can keep it: the shear is approximated, and the loss names it.
    #[test]
    fn a_light_or_camera_under_a_shear_keeps_its_eye_and_forward() {
        use super::super::fixtures::lightcam;
        use super::super::scene::{Animation, Camera, Light, LightKind, Projection};
        let child = |s: &mut Scene, parent: usize, node: Node| -> usize {
            s.nodes.push(node);
            let i = s.nodes.len() - 1;
            s.nodes[parent].children.push(i);
            i
        };
        let camera = || Camera { name: Some("eye".into()), projection: Projection::Perspective { yfov: 0.8, aspect: None, znear: 0.1, zfar: None } };
        let quarter = math::quat_from_axis_angle([0.0, 1.0, 0.0], std::f64::consts::FRAC_PI_2);
        let turn = |node: usize| Animation {
            name: None,
            channels: vec![Channel {
                node,
                path: AnimPath::Rotation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: math::QUAT_IDENTITY.iter().chain(quarter.iter()).map(|v| *v as f32).collect(),
            }],
        };
        // Its Z axis leans half a unit toward +X, which no TRS holds.
        let shear = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.5, 0.0, 1.0, 0.0, 1.0, 2.0, 3.0, 1.0];

        let mut s = scene_with(tri(), Transform::default());
        s.lights.push(Light { name: Some("lamp".into()), kind: LightKind::Spot { inner_cone: 0.2, outer_cone: 0.6 }, color: [1.0; 3], intensity: 10.0, range: None });
        s.cameras.push(camera());
        let rig = s.add_root(Node { name: Some("rig".into()), ..Default::default() });
        let lean = child(&mut s, rig, Node { name: Some("lean".into()), transform: Transform::Matrix(shear), ..Default::default() });
        let aim = math::quat_mul(math::quat_from_axis_angle([0.0, 1.0, 0.0], 30f64.to_radians()), math::quat_from_axis_angle([1.0, 0.0, 0.0], (-20f64).to_radians()));
        let placed = Transform::Trs { translation: [0.5, 1.0, 4.0], rotation: aim, scale: [1.0; 3] };
        let eye = child(&mut s, lean, Node { name: Some("eye_node".into()), transform: placed, light: Some(0), camera: Some(0), ..Default::default() });
        child(&mut s, eye, Node { name: Some("tail".into()), ..Default::default() });
        s.animations.push(turn(rig));
        let times = [None, Some(0.0), Some(0.37), Some(1.0)];
        let before: Vec<_> = times.iter().map(|t| lightcam::frames(&s, *t)).collect();
        bake_shear(&mut s);
        let near = |a: [f64; 3], b: [f64; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-9);
        for (t, (want_lights, want_cameras)) in times.iter().zip(&before) {
            let (lights, cameras) = lightcam::frames(&s, *t);
            assert_eq!((lights.len(), cameras.len()), (1, 1), "{t:?}");
            let (c, w) = (&cameras[0], &want_cameras[0]);
            assert!(near(c.eye, w.eye) && near(c.forward, w.forward), "{t:?}: the camera {c:?} against {w:?}");
            let (l, w) = (&lights[0], &want_lights[0]);
            assert!(near(l.position, w.position) && near(l.direction, w.direction), "{t:?}: the light {l:?} against {w:?}");
            let squared = c.forward[0] * c.up[0] + c.forward[1] * c.up[1] + c.forward[2] * c.up[2];
            assert!(squared.abs() < 1e-12, "{t:?}: the up is square to the view: {squared}");
        }
        let holder = s.nodes.iter().position(|n| n.camera.is_some()).unwrap();
        assert_eq!((s.nodes[holder].name.as_deref(), s.nodes[holder].light), (Some("eye_node"), Some(0)), "the new node has the old one's name, and the light too");
        assert!(s.nodes[rig].children.contains(&holder), "hung from the animated rig");
        let old = &s.nodes[eye];
        assert!(old.light.is_none() && old.camera.is_none() && old.name.as_deref() == Some("eye_node_shear") && old.children.len() == 1, "{old:?}");
        assert!(s.nodes.iter().all(|n| math::decompose(&n.transform.matrix()).is_some()), "every local is a TRS");

        let mut a = scene_with(tri(), Transform::default());
        a.cameras.push(camera());
        let lean = a.add_root(Node { name: Some("lean".into()), transform: Transform::Matrix(shear), ..Default::default() });
        let eye = child(&mut a, lean, Node { name: Some("eye_node".into()), camera: Some(0), ..Default::default() });
        a.animations.push(turn(eye));
        let lost = shear_losses(&a, "glb");
        assert!(lost.iter().any(|l| l.what.contains("the shear of 1 node(s)") && l.what.contains("\"lean\"")), "{lost:?}");
    }

    // ─── points and lines ───────────────────────────────────────────────────

    /// Every primitive's topology, in mesh order.
    fn drawn(s: &Scene) -> Vec<Topology> {
        s.meshes.iter().flat_map(|m| &m.primitives).map(|p| p.topology).collect()
    }

    /// [`tri`]'s three vertices, drawn as a triangle, as two segments and as
    /// three points.
    fn three_ways() -> [Primitive; 3] {
        let at = |topology: Topology, indices: &[u32]| Primitive { topology, indices: indices.to_vec(), ..tri() };
        [at(Topology::Triangles, &[0, 1, 2]), at(Topology::Lines, &[0, 1, 1, 2]), at(Topology::Points, &[0, 1, 2])]
    }

    /// Each of [`three_ways`] on a node of its own at the origin, all three
    /// children of one parent that a clip moves and turns.
    fn parent_of_three() -> Scene {
        let mut s = Scene::default();
        for prim in three_ways() {
            s.meshes.push(SceneMesh { primitives: vec![prim], ..Default::default() });
        }
        s.nodes.push(Node { name: Some("parent".into()), children: vec![1, 2, 3], ..Default::default() });
        for mesh in 0..3 {
            s.nodes.push(Node { mesh: Some(mesh), ..Default::default() });
        }
        s.roots = vec![0];
        let quarter = math::quat_from_axis_angle([0.0, 0.0, 1.0], std::f64::consts::FRAC_PI_2);
        let turn: Vec<f32> = math::QUAT_IDENTITY.iter().chain(quarter.iter()).map(|v| *v as f32).collect();
        s.animations.push(Animation {
            name: None,
            channels: vec![
                Channel { node: 0, path: AnimPath::Translation, interpolation: Interpolation::Linear, times: vec![0.0, 1.0], values: vec![0.0, 0.0, 0.0, 2.0, 0.0, 1.0] },
                Channel { node: 0, path: AnimPath::Rotation, interpolation: Interpolation::Linear, times: vec![0.0, 1.0], values: turn },
            ],
        });
        s
    }

    /// An animated parent moves a segment's and a point's vertices exactly as
    /// it moves a triangle's, and keeps what each draws: a mirror reverses
    /// a triangle's winding and leaves a segment's ends and a point alone.
    #[test]
    fn world_primitives_pose_lines_and_points() {
        let s = parent_of_three();
        let want = [Topology::Triangles, Topology::Lines, Topology::Points];
        let indices = [vec![0, 1, 2], vec![0, 1, 1, 2], vec![0, 1, 2]];
        let mut moved = Vec::new();
        for t in [0.0, 0.4, 1.0] {
            let wp = world_primitives(&s, &sample_pose(&s, 0, t)).unwrap();
            assert_eq!(wp.iter().map(|w| w.topology).collect::<Vec<_>>(), want, "t = {t}");
            assert_eq!(wp.iter().map(|w| w.indices.clone()).collect::<Vec<_>>(), indices, "t = {t}");
            assert_eq!(wp[1].positions, wp[0].positions, "t = {t}: the segments' vertices");
            assert_eq!(wp[2].positions, wp[0].positions, "t = {t}: the points");
            moved.push(wp[0].positions.clone());
        }
        assert!(moved[0] != moved[1] && moved[1] != moved[2], "the clip does move them: {moved:?}");
        let (turned, near) = ([[2.0, 0.0, 1.0], [2.0, 1.0, 1.0], [1.0, 0.0, 1.0]], |a: f32, b: f32| (a - b).abs() < 1e-5);
        assert!(
            moved[2].iter().zip(turned).all(|(got, want)| (0..3).all(|k| near(got[k], want[k]))),
            "a quarter turn about Z, then (2, 0, 1): {:?}",
            moved[2]
        );

        let mut mirrored = s.clone();
        mirrored.nodes[0].transform = Transform::Trs { translation: [0.0; 3], rotation: math::QUAT_IDENTITY, scale: [-1.0, 1.0, 1.0] };
        let wp = world_primitives(&mirrored, &default_pose(&mirrored)).unwrap();
        assert_eq!(wp.iter().map(|w| w.indices.clone()).collect::<Vec<_>>(), [vec![0, 2, 1], vec![0, 1, 1, 2], vec![0, 1, 2]]);
    }

    /// `bake` and `degrade` carry every topology, with the posing: where the
    /// hierarchy goes, each primitive stands in world space under a root of
    /// its own; where the meshes merge, one primitive per topology; and what
    /// the container lacks goes (`strip_topologies`).
    #[test]
    fn bake_and_degrade_carry_every_topology() {
        use super::super::fixtures::wire;
        use super::super::scene::Capabilities;
        let s = wire();
        let full = Capabilities::full();
        let all = [Topology::Triangles, Topology::Lines, Topology::Points];
        assert_eq!(degrade(&s, &full).unwrap(), s, "a container that holds all three changes nothing");

        let baked = bake(&s, &sample_pose(&s, 0, 1.0)).unwrap();
        assert_eq!(drawn(&baked), all);
        let lines = &baked.meshes[1].primitives[0];
        assert_eq!(lines.indices, [0, 1, 1, 2, 2, 3, 3, 0], "the loop's segments, as the source has them");
        assert!((lines.positions[0][1] - 1.25).abs() < 1e-6, "posed at t = 1: {:?}", lines.positions);

        let flat = degrade(&s, &Capabilities { hierarchy: false, ..full }).unwrap();
        assert_eq!(drawn(&flat), all);
        assert_eq!(flat.nodes.iter().filter(|n| n.mesh.is_some()).count(), 3);
        assert_eq!(flat.meshes[1].primitives[0].positions, s.meshes[1].primitives[0].positions, "at rest the loop stands where it was");

        let one = degrade(&s, &Capabilities { multiple_meshes: false, ..full }).unwrap();
        assert_eq!((one.meshes.len(), drawn(&one)), (1, all.to_vec()), "one primitive of each topology");
        assert_eq!(one.drawable_count(), s.drawable_count());

        let no_points = Capabilities { points: false, ..full };
        assert_eq!(drawn(&degrade(&s, &no_points).unwrap()), [Topology::Triangles, Topology::Lines]);
        let no_points_or_lines = Capabilities { points: false, lines: false, ..full };
        assert_eq!(drawn(&degrade(&s, &no_points_or_lines).unwrap()), [Topology::Triangles]);
        let one_without = degrade(&s, &Capabilities { multiple_meshes: false, points: false, ..full }).unwrap();
        assert_eq!((one_without.meshes.len(), drawn(&one_without)), (1, vec![Topology::Triangles, Topology::Lines]));
    }

    /// Primitives of one topology merge, and no others: at most one merged
    /// primitive per topology, in the order triangles, lines, points, each
    /// indexing its own vertices, whatever the order they came in.
    #[test]
    fn merge_all_keeps_topologies_apart() {
        let kinds = [Topology::Triangles, Topology::Lines, Topology::Points, Topology::Triangles, Topology::Lines, Topology::Points];
        let ways = three_ways();
        let mut s = Scene::default();
        for (i, kind) in kinds.into_iter().enumerate() {
            let mut p = ways.iter().find(|p| p.topology == kind).unwrap().clone();
            for v in p.positions.iter_mut() {
                v[0] += 10.0 * i as f32;
            }
            // The first segments alone have normals, to which the second's
            // are filled in.
            if i == 1 {
                p.normals = Some(vec![[0.0, 1.0, 0.0]; 3]);
            }
            s.meshes.push(SceneMesh { primitives: vec![p], ..Default::default() });
            let mesh = s.meshes.len() - 1;
            s.add_root(Node { mesh: Some(mesh), ..Default::default() });
        }
        let merged = merge_all(&s).unwrap();
        merged.validate().unwrap();
        assert_eq!(merged.meshes.len(), 1);
        let prims = &merged.meshes[0].primitives;
        assert_eq!(prims.iter().map(|p| p.topology).collect::<Vec<_>>(), [Topology::Triangles, Topology::Lines, Topology::Points]);
        assert!(prims.iter().all(|p| p.positions.len() == 6), "two sources of three vertices each");
        assert_eq!(prims[0].indices, [0, 1, 2, 3, 4, 5]);
        assert_eq!(prims[1].indices, [0, 1, 1, 2, 3, 4, 4, 5]);
        assert_eq!(prims[2].indices, [0, 1, 2, 3, 4, 5]);
        let xs = |p: &Primitive| p.positions.iter().map(|v| v[0]).collect::<Vec<_>>();
        assert_eq!(xs(&prims[0]), [0.0, 1.0, 0.0, 30.0, 31.0, 30.0]);
        assert_eq!(xs(&prims[1]), [10.0, 11.0, 10.0, 40.0, 41.0, 40.0]);
        assert_eq!(xs(&prims[2]), [20.0, 21.0, 20.0, 50.0, 51.0, 50.0]);
        let normals = prims[1].normals.as_ref().expect("the segments that had normals keep them");
        assert_eq!((normals[0], normals[5]), ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]), "and the others are filled in");
        assert!(prims[0].normals.is_none() && prims[2].normals.is_none());
        assert_eq!(merged.drawable_count(), 2 + 4 + 6);
    }

    /// The bounds of every position drawn, of every topology: the points
    /// below the cube and the loop above it widen what the cube alone gives,
    /// at rest and while the clip moves the loop; a scene of points has
    /// bounds; a scene that draws no positions has none.
    #[test]
    fn world_bounds_count_points_and_lines() {
        use super::super::fixtures::wire;
        let s = wire();
        assert_eq!(world_bounds(&s, &default_pose(&s)), Some(([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0])), "the grid's metre and the loop's");
        let mut cube = s.clone();
        for m in cube.meshes.iter_mut() {
            m.primitives.retain(|p| p.topology == Topology::Triangles);
        }
        assert_eq!(world_bounds(&cube, &default_pose(&cube)), Some(([-0.5; 3], [0.5; 3])), "the cube alone");

        // At t = 1 the loop is 0.5 along x and 0.25 up, turned an eighth, so
        // its corners reach 0.75 * sqrt(2) along x and z.
        let (lo, hi) = world_bounds(&s, &sample_pose(&s, 0, 1.0)).expect("bounds");
        let reach = 0.75 * 2f64.sqrt();
        let want_lo = [-1.0, -1.0, -reach];
        let want_hi = [0.5 + reach, 1.25, reach];
        for k in 0..3 {
            assert!((lo[k] - want_lo[k]).abs() < 1e-5 && (hi[k] - want_hi[k]).abs() < 1e-5, "axis {k}: {lo:?} {hi:?}");
        }

        let points = Primitive { positions: vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]], indices: vec![0, 1], topology: Topology::Points, ..Default::default() };
        let cloud = scene_with(points, Transform::default());
        assert_eq!(world_bounds(&cloud, &default_pose(&cloud)), Some(([1.0, 2.0, 3.0], [4.0, 5.0, 6.0])));
        let nothing = Scene::default();
        assert_eq!(world_bounds(&nothing, &default_pose(&nothing)), None);
    }

    /// The flat view holds triangles: a loop and a grid beside the cube
    /// leave it as the cube alone gives it (the STL path, the flat API, and
    /// every caller of `flatten`).
    #[test]
    fn flatten_takes_triangles_only() {
        use super::super::fixtures::wire;
        let s = wire();
        let mut cube = s.clone();
        for m in cube.meshes.iter_mut() {
            m.primitives.retain(|p| p.topology == Topology::Triangles);
        }
        let flat = flatten(&s).unwrap();
        assert_eq!((flat.triangle_count(), flat.positions.len()), (12, 24));
        assert_eq!(flat, flatten(&cube).unwrap());
        // Of points alone there is no flat view to speak of.
        let points = Primitive { positions: vec![[0.0; 3]; 2], indices: vec![0, 1], topology: Topology::Points, ..Default::default() };
        let cloud = flatten(&scene_with(points, Transform::default())).unwrap();
        assert_eq!((cloud.positions.len(), cloud.triangle_count()), (0, 0));
    }

    /// A container without points, or without lines, loses the primitives of
    /// that topology: the primitive, the mesh it leaves empty and the
    /// node's hold on it (a skin, morph weights and their clip go with
    /// it, as any mesh a node no longer draws), and nothing else.
    #[test]
    fn strip_topologies_drops_what_a_container_lacks() {
        use super::super::fixtures::wire;
        use super::super::scene::Capabilities;
        let s = wire();
        let full = Capabilities::full();
        let stripped = |caps: &Capabilities| -> Scene {
            let mut c = s.clone();
            strip_topologies(&mut c, caps);
            c.validate().unwrap();
            c
        };
        assert_eq!(stripped(&full), s, "a container that holds both loses nothing");

        let no_points = stripped(&Capabilities { points: false, ..full });
        assert_eq!(drawn(&no_points), [Topology::Triangles, Topology::Lines]);
        assert_eq!(no_points.nodes.iter().map(|n| n.mesh).collect::<Vec<_>>(), [Some(0), Some(1), None], "the grid's node draws nothing");
        assert_eq!(no_points.animations, s.animations);

        // The lines go, and the points' mesh moves up to take their place.
        let no_lines = stripped(&Capabilities { lines: false, ..full });
        assert_eq!(drawn(&no_lines), [Topology::Triangles, Topology::Points]);
        assert_eq!(no_lines.nodes.iter().map(|n| n.mesh).collect::<Vec<_>>(), [Some(0), None, Some(1)]);
        assert_eq!(no_lines.meshes[1].name.as_deref(), Some("wire_points"));
        assert_eq!(no_lines.animations, s.animations, "the loop's node still moves, though it draws nothing");

        let neither = stripped(&Capabilities { points: false, lines: false, ..full });
        assert_eq!(drawn(&neither), [Topology::Triangles]);
        assert_eq!(neither.nodes.iter().map(|n| n.mesh).collect::<Vec<_>>(), [Some(0), None, None]);
        assert_eq!((neither.materials.len(), neither.nodes.len(), neither.roots.len()), (1, 3, 3));

        // A mesh of triangles and points keeps its triangles, and its place.
        let mut mixed = Scene::default();
        let target = |n: usize| MorphTarget { positions: vec![[0.0, 0.0, 1.0]; n], normals: None };
        let (mut a, mut b) = (three_ways()[0].clone(), three_ways()[2].clone());
        (a.targets, b.targets) = (vec![target(3)], vec![target(3)]);
        mixed.meshes.push(SceneMesh { primitives: vec![a, b.clone()], weights: vec![0.5], target_names: vec!["lift".into()], ..Default::default() });
        mixed.meshes.push(SceneMesh { primitives: vec![b], weights: vec![0.25], target_names: vec!["lift".into()], ..Default::default() });
        mixed.add_root(Node { mesh: Some(0), weights: Some(vec![0.5]), ..Default::default() });
        mixed.add_root(Node { mesh: Some(1), weights: Some(vec![0.25]), ..Default::default() });
        let wave = |node: usize| Channel { node, path: AnimPath::Weights, interpolation: Interpolation::Linear, times: vec![0.0, 1.0], values: vec![0.0, 1.0] };
        mixed.animations.push(Animation { name: None, channels: vec![wave(0), wave(1)] });
        mixed.validate().unwrap();
        strip_topologies(&mut mixed, &Capabilities { points: false, ..full });
        mixed.validate().unwrap();
        assert_eq!(drawn(&mixed), [Topology::Triangles]);
        assert_eq!(mixed.nodes.iter().map(|n| (n.mesh, n.weights.clone())).collect::<Vec<_>>(), [(Some(0), Some(vec![0.5])), (None, None)]);
        assert_eq!(mixed.animations[0].channels, [wave(0)], "the weights of a mesh no node draws any more are not animated");
    }

    /// glTF's row keeps everything the scene holds, points and lines among it,
    /// so its `prepare` changes none of it: the segments and the points stay,
    /// and so do a texture no material uses and the tangents of a primitive
    /// without uvs, which `degrade` would drop and so must not run.
    #[test]
    fn gltf_prepare_keeps_points_and_lines_and_what_degrade_would_drop() {
        use super::super::scene::{Image, ImageFormat, Light, LightKind};
        use super::super::{MeshFormat, WriteOptions};
        let mut s = scene_with(Primitive { tangents: Some(vec![[1.0, 0.0, 0.0, 1.0]; 3]), ..tri() }, Transform::default());
        s.images.push(Image { name: None, format: ImageFormat::Png, data: super::super::fixtures::encode_png(1, 1, &[255; 4]) });
        s.textures.push(super::super::scene::Texture { name: None, image: 0, sampler: Default::default() });
        s.lights.push(Light { name: None, kind: LightKind::Directional, color: [1.0; 3], intensity: 3.0, range: None });
        s.nodes[0].light = Some(0);
        for prim in &three_ways()[1..] {
            s.meshes.push(SceneMesh { primitives: vec![prim.clone()], ..Default::default() });
            let mesh = s.meshes.len() - 1;
            s.add_root(Node { mesh: Some(mesh), ..Default::default() });
        }
        let opts = WriteOptions::default();
        for fmt in [MeshFormat::Glb, MeshFormat::Gltf] {
            assert_eq!(fmt.prepare(&s, opts).unwrap(), s, "{fmt}");
            assert_ne!(degrade(&s, &fmt.capabilities(opts)).unwrap(), s, "{fmt}: degrade is no identity here, so prepare must not run it");
        }
    }

    /// A mirrored, sheared parent (which glTF's `prepare` bakes into its
    /// children's vertices) reverses a triangle's winding and leaves what a
    /// segment and a point list say alone: each segment keeps its two ends
    /// and each point its place in the list.
    #[test]
    fn bake_shear_flips_triangles_and_leaves_lines_and_points_alone() {
        // Mirrored in X (determinant -1), and its Z axis leans toward -X: no TRS holds it.
        let mirrored_shear = [-1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.5, 0.0, 1.0, 0.0, 1.0, 2.0, 3.0, 1.0];
        assert!(math::decompose(&mirrored_shear).is_none() && math::is_mirroring(&mirrored_shear));
        let corners = tri().positions;
        let prim = |topology: Topology, indices: &[u32]| Primitive { positions: corners.clone(), indices: indices.to_vec(), topology, ..tri() };
        let mut s = Scene::default();
        s.meshes.push(SceneMesh { primitives: vec![prim(Topology::Triangles, &[0, 1, 2, 2, 1, 0])], ..Default::default() });
        s.meshes.push(SceneMesh { primitives: vec![prim(Topology::Lines, &[0, 1, 1, 2, 2, 0])], ..Default::default() });
        s.meshes.push(SceneMesh { primitives: vec![prim(Topology::Points, &[2, 0, 1])], ..Default::default() });
        let lean = s.add_root(Node { name: Some("lean".into()), transform: Transform::Matrix(mirrored_shear), children: vec![1, 2, 3], ..Default::default() });
        assert_eq!(lean, 0);
        for mesh in 0..3 {
            s.nodes.push(Node { mesh: Some(mesh), ..Default::default() });
        }
        bake_shear(&mut s);
        let mut baked: Vec<(Topology, Vec<u32>)> =
            s.nodes.iter().filter_map(|n| n.mesh).flat_map(|m| &s.meshes[m].primitives).map(|p| (p.topology, p.indices.clone())).collect();
        baked.sort_by_key(|(topology, _)| *topology as u8);
        assert_eq!(
            baked,
            [(Topology::Triangles, vec![0, 2, 1, 2, 0, 1]), (Topology::Lines, vec![0, 1, 1, 2, 2, 0]), (Topology::Points, vec![2, 0, 1])]
        );
    }

    /// Ruling B28 (mesh-gaps item 2's final wave): a texture transform is
    /// baked into the UVs of every container that keeps UVs but not texture
    /// transforms, whether or not it keeps textures. gltfpack quantizes UVs
    /// and carries their scale (about 16×) in `KHR_texture_transform`, so
    /// before this the textureless single-file obj, ply, dae and usda spanned
    /// 0–0.0625 where the texture-keeping usdz spanned 0–1, and one
    /// generation's usda and usdz disagreed 16×. Each textureless output's
    /// coordinates are the usdz's, within f32.
    #[test]
    fn texture_transforms_are_baked_into_the_uvs_of_textureless_containers() {
        use super::super::package::NoSidecars;
        let glb = include_bytes!("testdata/gltf/gltfpack/textured.cc.glb");
        let scene = read_scene(glb, MeshFormat::Glb, &NoSidecars).unwrap_or_else(|e| panic!("textured.cc.glb: {e}")).scene;
        assert!(scene.materials.iter().any(|m| m.texture_refs().any(|(_, r)| r.transform.is_some())), "the input carries a texture transform");
        // The distinct coordinates of every primitive's first set, sorted:
        // the containers merge and order primitives their own ways, and the
        // flat ones bake each instance of a mesh into a copy of its own.
        let uvs = |s: &Scene| -> Vec<[f32; 2]> {
            let mut v: Vec<[f32; 2]> = s.meshes.iter().flat_map(|m| &m.primitives).flat_map(|p| p.uvs.first().cloned().unwrap_or_default()).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v.dedup();
            v
        };
        let plain = WriteOptions::default();
        let one_file = WriteOptions { self_contained: true, ..WriteOptions::default() };
        let want = uvs(&MeshFormat::Usdz.prepare(&scene, plain).unwrap());
        let raw = uvs(&scene);
        assert!(want.iter().zip(&raw).any(|(w, r)| (w[0] - r[0]).abs() > 0.1 || (w[1] - r[1]).abs() > 0.1), "the transform moves the coordinates");
        for (fmt, opts) in [(MeshFormat::Obj, one_file), (MeshFormat::Ply, plain), (MeshFormat::Dae, one_file), (MeshFormat::Usda, one_file), (MeshFormat::Usdc, one_file)] {
            let caps = fmt.capabilities(opts);
            assert!(!caps.textures && caps.uv_sets > 0 && !caps.texture_transforms, "{fmt}: keeps UVs, not textures or their transforms");
            let got = uvs(&fmt.prepare(&scene, opts).unwrap_or_else(|e| panic!("{fmt}: {e}")));
            assert_eq!(got.len(), want.len(), "{fmt}: as many coordinates as the usdz");
            for (i, (g, w)) in got.iter().zip(&want).enumerate() {
                assert!((g[0] - w[0]).abs() <= 1e-6 && (g[1] - w[1]).abs() <= 1e-6, "{fmt}: uv {i} is {g:?}, the usdz's {w:?}");
            }
        }
    }

    /// B28's other half: a container that keeps texture transforms is not
    /// baked. glTF keeps them, so its coordinates and the binding's transform
    /// come out as they went in; and the condition is the capability, not
    /// the container, so a textureless one that kept transforms would not
    /// be baked either.
    #[test]
    fn a_container_that_keeps_texture_transforms_is_not_baked() {
        use super::super::package::NoSidecars;
        use super::super::scene::Capabilities;
        let glb = include_bytes!("testdata/gltf/gltfpack/textured.cc.glb");
        let scene = read_scene(glb, MeshFormat::Glb, &NoSidecars).unwrap_or_else(|e| panic!("textured.cc.glb: {e}")).scene;
        let uvs = |s: &Scene| -> Vec<Vec<[f32; 2]>> { s.meshes.iter().flat_map(|m| &m.primitives).map(|p| p.uvs.first().cloned().unwrap_or_default()).collect() };
        let transforms = |s: &Scene| -> Vec<UvTransform> { s.materials.iter().flat_map(|m| m.texture_refs().filter_map(|(_, r)| r.transform).collect::<Vec<_>>()).collect() };
        assert!(!transforms(&scene).is_empty(), "the input carries a texture transform");
        let caps = MeshFormat::Gltf.capabilities(WriteOptions::default());
        assert!(caps.textures && caps.texture_transforms);
        let g = MeshFormat::Gltf.prepare(&scene, WriteOptions::default()).unwrap();
        assert_eq!(uvs(&g), uvs(&scene), "glTF: the coordinates as they went in");
        assert_eq!(transforms(&g), transforms(&scene), "glTF: the transforms as they went in");
        let kept = Capabilities { textures: false, texture_transforms: true, ..Capabilities::full() };
        let d = degrade(&scene, &kept).unwrap();
        assert_eq!(uvs(&d), uvs(&scene), "no textures, but transforms kept: not baked");
    }
}
