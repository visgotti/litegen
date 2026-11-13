//! What litegen's own reader finds in a mesh (PixelGenie 3D doc §3D).
//!
//! Every delivered mesh is described by what litegen measured in it, never by
//! what its vendor claims: bounds at rest, vertex and triangle counts, uv sets,
//! vertex colour, each material's texture maps, the skin (its joints, root,
//! humanoid map and bind pose) and every clip's length and root travel,
//! measured from its keyframes.

use super::math::Mat4;
use super::scene::{Scene, TextureSlot, Topology};
use super::{pose, MeshError};

/// A clip moves its root less than this (horizontally, start to end) to count
/// as in place.
pub const IN_PLACE_TOLERANCE_M: f32 = 0.01;

#[derive(Debug, Clone, PartialEq)]
pub struct SceneFacts {
    /// World-space bounds at rest (the bind pose for a skinned mesh), of
    /// everything drawn: points and lines count ([`pose::world_bounds`]).
    pub bounds: Option<([f32; 3], [f32; 3])>,
    pub vertex_count: usize,
    /// Triangles only: lines and points are not counted.
    pub triangle_count: usize,
    /// Most texture-coordinate sets on any primitive.
    pub uv_sets: usize,
    pub vertex_colors: bool,
    pub materials: Vec<MaterialFacts>,
    pub skin: Option<SkinFacts>,
    pub animations: Vec<ClipFacts>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaterialFacts {
    pub name: Option<String>,
    /// `base_color`, `metallic_roughness`, `normal`, `occlusion`, `emissive`,
    /// and the layer maps `clearcoat`, `clearcoat_roughness`,
    /// `clearcoat_normal`, `transmission`, `volume_thickness`, `specular`,
    /// `specular_color`, `sheen_color`, `sheen_roughness`, `iridescence`,
    /// `iridescence_thickness`, `anisotropy`.
    pub maps: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindPose {
    /// Arms within 15° of horizontal.
    T,
    /// Arms 25°–55° below horizontal.
    A,
    /// Neither, or no humanoid arms to measure.
    Unknown,
}

impl BindPose {
    pub const fn as_str(self) -> &'static str {
        match self {
            BindPose::T => "t",
            BindPose::A => "a",
            BindPose::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkinFacts {
    /// Joint names in skin order (an unnamed joint is `""`).
    pub joints: Vec<String>,
    /// The joint every other joint of the skin descends from.
    pub root: Option<String>,
    pub bind_pose: BindPose,
    /// `(table, [(VRM bone, joint)])` when a known naming scheme matches.
    pub humanoid: Option<(&'static str, Vec<(String, String)>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClipFacts {
    pub name: Option<String>,
    pub duration_s: f32,
    /// World-space displacement of the motion joint ([`motion_joint`]),
    /// first key to last.
    pub root_travel_m: [f32; 3],
    /// The farthest the motion joint gets from where it starts,
    /// horizontally, at any time in the clip: a clip that goes out and comes
    /// back travels nowhere but is not in place.
    pub excursion_m: f32,
}

impl ClipFacts {
    /// Whether the motion joint stays where it starts, horizontally, for
    /// the whole clip.
    pub fn in_place(&self) -> bool {
        self.excursion_m < IN_PLACE_TOLERANCE_M
    }
}

/// Samples [`clip_times`] takes between keys, per second.
const MOTION_SAMPLE_HZ: f64 = 30.0;
/// Most grid samples [`clip_times`] adds, whatever the clip's length.
const MAX_MOTION_SAMPLES: usize = 4096;

/// Every key time of clip `anim`, plus a grid between (`MOTION_SAMPLE_HZ`,
/// at most `MAX_MOTION_SAMPLES`), sorted and distinct: where a joint's motion
/// is measured and held, so it follows its ancestors' curves, not just their
/// keys.
pub fn clip_times(scene: &Scene, anim: usize) -> Vec<f32> {
    let Some(a) = scene.animations.get(anim) else { return Vec::new() };
    let Some((lo, hi)) = a.time_range() else { return Vec::new() };
    let mut times: Vec<f32> = a.channels.iter().flat_map(|c| c.times.iter().copied()).collect();
    let span = f64::from(hi) - f64::from(lo);
    let steps = ((span * MOTION_SAMPLE_HZ).ceil() as usize).min(MAX_MOTION_SAMPLES);
    for i in 0..=steps {
        times.push((f64::from(lo) + span * (i as f64 / steps.max(1) as f64)) as f32);
    }
    times.sort_by(|a, b| a.total_cmp(b));
    times.dedup_by(|a, b| a.to_bits() == b.to_bits());
    times
}

/// The joint a clip's root motion is measured and held on: the humanoid
/// hips when the rig reads as a humanoid ("Usually only this bone moves",
/// VRM 1.0 humanoid.md; Mixamo's In Place holds `mixamorig:Hips`), else the
/// skin's topmost joint.
pub fn motion_joint(scene: &Scene) -> Option<usize> {
    let skin = scene.skins.first()?;
    let names: Vec<String> = skin.joints.iter().map(|&j| scene.nodes[j].name.clone().unwrap_or_default()).collect();
    let hips = super::humanoid::humanoid_map(&names).and_then(|(_, map)| {
        let joint = map.into_iter().find(|(bone, _)| bone == "hips")?.1;
        skin.joints.iter().copied().find(|&j| scene.nodes[j].name.as_deref() == Some(joint.as_str()))
    });
    hips.or_else(|| root_joint(scene, &skin.joints, skin.skeleton))
}

fn slot_name(slot: TextureSlot) -> &'static str {
    match slot {
        TextureSlot::BaseColor => "base_color",
        TextureSlot::MetallicRoughness => "metallic_roughness",
        TextureSlot::Normal => "normal",
        TextureSlot::Occlusion => "occlusion",
        TextureSlot::Emissive => "emissive",
        TextureSlot::Clearcoat => "clearcoat",
        TextureSlot::ClearcoatRoughness => "clearcoat_roughness",
        TextureSlot::ClearcoatNormal => "clearcoat_normal",
        TextureSlot::Transmission => "transmission",
        TextureSlot::VolumeThickness => "volume_thickness",
        TextureSlot::Specular => "specular",
        TextureSlot::SpecularColor => "specular_color",
        TextureSlot::SheenColor => "sheen_color",
        TextureSlot::SheenRoughness => "sheen_roughness",
        TextureSlot::Iridescence => "iridescence",
        TextureSlot::IridescenceThickness => "iridescence_thickness",
        TextureSlot::Anisotropy => "anisotropy",
    }
}

fn translation_of(m: &Mat4) -> [f64; 3] {
    [m[12], m[13], m[14]]
}

/// The joint of `joints` (node indices) none of the others is an ancestor of.
fn root_joint(scene: &Scene, joints: &[usize], skeleton: Option<usize>) -> Option<usize> {
    if let Some(s) = skeleton.filter(|s| joints.contains(s)) {
        return Some(s);
    }
    let parents = scene.parents();
    joints.iter().copied().find(|&j| {
        let mut p = parents[j];
        while let Some(n) = p {
            if joints.contains(&n) {
                return false;
            }
            p = parents[n];
        }
        true
    })
}

/// Degrees below horizontal of the left upper arm (or the right, when the left
/// has nothing to measure), from its joint to its child joint, or — for a bone
/// with no child — to the centre of the vertices it drives.
fn upper_arm_angle(scene: &Scene, skin: usize, humanoid: &[(String, String)]) -> Option<f64> {
    let world = scene.world_matrices();
    let joints = &scene.skins[skin].joints;
    let node_named = |name: &str| joints.iter().copied().find(|&j| scene.nodes[j].name.as_deref() == Some(name));
    for (upper, lower) in [("leftUpperArm", "leftLowerArm"), ("rightUpperArm", "rightLowerArm")] {
        let Some(arm) = humanoid.iter().find(|(b, _)| b == upper).and_then(|(_, j)| node_named(j)) else { continue };
        let start = translation_of(&world[arm]);
        let end = humanoid
            .iter()
            .find(|(b, _)| b == lower)
            .and_then(|(_, j)| node_named(j))
            .or_else(|| scene.nodes[arm].children.iter().copied().find(|c| joints.contains(c)))
            .map(|c| translation_of(&world[c]))
            .or_else(|| driven_centroid(scene, skin, joints.iter().position(|&j| j == arm)?));
        let Some(end) = end else { continue };
        let d = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
        let horizontal = d[0].hypot(d[2]);
        if horizontal == 0.0 && d[1] == 0.0 {
            continue;
        }
        return Some((-d[1]).atan2(horizontal).to_degrees());
    }
    None
}

/// Centre, where they are drawn at rest, of the vertices whose strongest
/// influence is skin joint `joint_index`: Σ wᵢ · world(jointᵢ) · IBMᵢ · v
/// (glTF 2.0 §3.7.3). The stored vertices are in bind space, which matches
/// the joints' world positions only when nothing sits above the joints — a
/// height or placement root tilted the angle by tens of degrees.
fn driven_centroid(scene: &Scene, skin: usize, joint_index: usize) -> Option<[f64; 3]> {
    let world = scene.world_matrices();
    let sk = &scene.skins[skin];
    let joint_mats: Vec<super::math::Mat4> =
        sk.joints.iter().zip(&sk.inverse_bind_matrices).map(|(&j, ibm)| super::math::mat_mul(&world[j], ibm)).collect();
    let mut sum = [0.0f64; 3];
    let mut n = 0usize;
    for node in scene.nodes.iter().filter(|n| n.skin == Some(skin)) {
        let Some(mesh) = node.mesh else { continue };
        for prim in &scene.meshes[mesh].primitives {
            let Some(sw) = prim.skin.as_ref() else { continue };
            for (v, p) in prim.positions.iter().enumerate() {
                let (js, ws) = sw.vertex(v);
                let strongest = js.iter().zip(ws).max_by(|a, b| a.1.total_cmp(b.1)).map(|(j, _)| *j as usize);
                if strongest != Some(joint_index) {
                    continue;
                }
                let local = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
                let mut drawn = [0.0f64; 3];
                for (j, w) in js.iter().zip(ws) {
                    let Some(m) = joint_mats.get(*j as usize) else { continue };
                    let q = super::math::transform_point_f64(m, local);
                    for k in 0..3 {
                        drawn[k] += f64::from(*w) * q[k];
                    }
                }
                for k in 0..3 {
                    sum[k] += drawn[k];
                }
                n += 1;
            }
        }
    }
    (n > 0).then(|| [sum[0] / n as f64, sum[1] / n as f64, sum[2] / n as f64])
}

fn classify(angle: Option<f64>) -> BindPose {
    match angle {
        Some(a) if a.abs() <= 15.0 => BindPose::T,
        Some(a) if (25.0..=55.0).contains(&a) => BindPose::A,
        _ => BindPose::Unknown,
    }
}

/// Measure `scene`.
pub fn inspect(scene: &Scene) -> Result<SceneFacts, MeshError> {
    let rest = pose::default_pose(scene);
    let bounds = match pose::world_bounds(scene, &rest) {
        // Positions are `f32`, so the `f64` bounds hold them exactly.
        Some((lo, hi)) => Some((lo.map(|v| v as f32), hi.map(|v| v as f32))),
        None => {
            // No positions; or a scene too big to pose, which is an error.
            pose::world_primitives(scene, &rest)?;
            None
        }
    };
    let mut uv_sets = 0;
    let mut vertex_colors = false;
    for prim in scene.meshes.iter().flat_map(|m| &m.primitives) {
        uv_sets = uv_sets.max(prim.uvs.len());
        vertex_colors |= prim.colors.is_some();
    }
    let materials = scene
        .materials
        .iter()
        .map(|m| MaterialFacts { name: m.name.clone(), maps: m.texture_refs().map(|(slot, _)| slot_name(slot)).collect() })
        .collect();

    let skin = scene.skins.first().map(|s| {
        let joints: Vec<String> = s.joints.iter().map(|&j| scene.nodes[j].name.clone().unwrap_or_default()).collect();
        let root = root_joint(scene, &s.joints, s.skeleton).and_then(|r| scene.nodes[r].name.clone());
        let humanoid = super::humanoid::humanoid_map(&joints);
        let bind_pose = classify(humanoid.as_ref().and_then(|(_, map)| upper_arm_angle(scene, 0, map)));
        SkinFacts { joints, root, bind_pose, humanoid }
    });

    let root = motion_joint(scene);
    let animations = scene
        .animations
        .iter()
        .enumerate()
        .map(|(i, anim)| {
            let (start, end) = anim.time_range().unwrap_or((0.0, 0.0));
            let root_travel_m = match root {
                Some(r) => {
                    let at = |t: f32| {
                        let world = super::scene::world_from_locals(scene, &pose::sample_pose(scene, i, t).locals());
                        translation_of(&world[r])
                    };
                    let (a, b) = (at(start), at(end));
                    [(b[0] - a[0]) as f32, (b[1] - a[1]) as f32, (b[2] - a[2]) as f32]
                }
                None => [0.0; 3],
            };
            // From t = 0, where a clip begins (glTF 2.0 §3.11: sampler inputs
            // are relative to the start of the animation): a clip keyed from
            // 0.5 s lasts to its last key, as FBX and Collada, which start
            // every take at 0, also say.
            let excursion_m = match root {
                Some(r) => {
                    let place = |t: f32| {
                        let world = super::scene::world_from_locals(scene, &pose::sample_pose(scene, i, t).locals());
                        translation_of(&world[r])
                    };
                    let from = place(start);
                    clip_times(scene, i)
                        .into_iter()
                        .map(|t| {
                            let p = place(t);
                            (p[0] - from[0]).hypot(p[2] - from[2]) as f32
                        })
                        .fold(0.0f32, f32::max)
                }
                None => 0.0,
            };
            ClipFacts { name: anim.name.clone(), duration_s: end - start.min(0.0), root_travel_m, excursion_m }
        })
        .collect();

    Ok(SceneFacts {
        bounds,
        vertex_count: scene.vertex_count(),
        triangle_count: scene.triangle_count(),
        uv_sets,
        vertex_colors,
        materials,
        skin,
        animations,
    })
}

/// One part of a segmented mesh: a mesh-bearing node.
#[derive(Debug, Clone, PartialEq)]
pub struct PartFacts {
    pub node: usize,
    pub name: Option<String>,
    /// Triangles only: lines and points are not counted.
    pub triangle_count: usize,
    /// World-space bounds at rest, of everything the part draws.
    pub bounds: Option<([f32; 3], [f32; 3])>,
}

/// Every mesh-bearing node, in node order, measured at rest.
pub fn parts(scene: &Scene) -> Result<Vec<PartFacts>, MeshError> {
    let mut out: Vec<PartFacts> = scene
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.mesh.is_some())
        .map(|(node, n)| PartFacts { node, name: n.name.clone(), triangle_count: 0, bounds: None })
        .collect();
    for wp in pose::world_primitives(scene, &pose::default_pose(scene))? {
        let Some(part) = out.iter_mut().find(|p| p.node == wp.node) else { continue };
        if wp.topology == Topology::Triangles {
            part.triangle_count += wp.indices.len() / 3;
        }
        for p in &wp.positions {
            let (lo, hi) = part.bounds.get_or_insert((*p, *p));
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::fixtures;

    #[test]
    fn each_mesh_node_is_a_part_with_its_own_triangles_and_world_bounds() {
        use super::super::scene::{Node, Transform};
        let mut scene = fixtures::cube();
        let mesh = scene.nodes.iter().find_map(|n| n.mesh).unwrap();
        let at = |x: f64, name: &str| Node {
            name: Some(name.into()),
            transform: Transform::Trs { translation: [x, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
            children: Vec::new(),
            mesh: Some(mesh),
            skin: None,
            weights: None,
            light: None,
            camera: None,
        };
        scene.nodes = vec![at(-5.0, "left"), at(5.0, "right")];
        scene.roots = vec![0, 1];
        let cube = scene.meshes[mesh].primitives.iter().map(|p| p.triangle_count()).sum::<usize>();
        let parts = parts(&scene).unwrap();
        assert_eq!(parts.iter().map(|p| (p.node, p.name.as_deref(), p.triangle_count)).collect::<Vec<_>>(), [(0, Some("left"), cube), (1, Some("right"), cube)]);
        let (l, r) = (parts[0].bounds.unwrap(), parts[1].bounds.unwrap());
        assert!(l.1[0] < 0.0 && r.0[0] > 0.0, "each part's own world bounds: {l:?} {r:?}");
    }

    /// The counts are of triangles and the bounds are of everything drawn: a
    /// loop and a grid beside the cube add no triangles, to the scene or to
    /// the part that draws them, and widen the bounds, the scene's and their
    /// own part's.
    #[test]
    fn a_loop_and_a_grid_add_no_triangles_but_widen_the_bounds() {
        use super::super::scene::Topology;
        let wire = fixtures::wire();
        let mut cube = wire.clone();
        for m in cube.meshes.iter_mut() {
            m.primitives.retain(|p| p.topology == Topology::Triangles);
        }
        let (with, without) = (inspect(&wire).unwrap(), inspect(&cube).unwrap());
        assert_eq!((with.triangle_count, without.triangle_count), (12, 12));
        assert_eq!(with.bounds, Some(([-1.0; 3], [1.0; 3])));
        assert_eq!(without.bounds, Some(([-0.5; 3], [0.5; 3])));

        let parts = parts(&wire).unwrap();
        let per_part: Vec<_> = parts.iter().map(|p| (p.name.as_deref(), p.triangle_count, p.bounds)).collect();
        assert_eq!(
            per_part,
            [
                (Some("wire_cube_node"), 12, Some(([-0.5; 3], [0.5; 3]))),
                (Some("wire_loop_node"), 0, Some(([-0.75, 1.0, -0.75], [0.75, 1.0, 0.75]))),
                (Some("wire_points_node"), 0, Some(([-1.0, -1.0, -1.0], [1.0, -1.0, 1.0]))),
            ]
        );
    }

    #[test]
    fn a_static_mesh_reports_its_geometry_and_materials() {
        let scene = fixtures::cube();
        let f = inspect(&scene).unwrap();
        assert_eq!(f.triangle_count, scene.triangle_count());
        assert_eq!(f.vertex_count, scene.vertex_count());
        let (lo, hi) = f.bounds.unwrap();
        assert!(hi.iter().zip(lo.iter()).all(|(h, l)| h > l), "{lo:?} {hi:?}");
        assert!(f.skin.is_none());
        assert!(f.animations.is_empty());
        assert_eq!(f.materials.len(), scene.materials.len());
    }

    #[test]
    fn a_textured_mesh_names_its_maps_and_uv_sets() {
        let f = inspect(&fixtures::textured()).unwrap();
        assert!(f.uv_sets >= 1);
        assert!(f.materials.iter().any(|m| m.maps.contains(&"base_color")), "{:?}", f.materials);
    }

    #[test]
    fn layer_maps_are_named_like_base_ones() {
        use super::super::scene::{Anisotropy, Clearcoat, Iridescence, Material, Sheen, Specular, TextureRef, Transmission, Volume};
        let mut scene = fixtures::textured();
        let t = || Some(TextureRef { texture: 0, uv_set: 0, transform: None });
        scene.materials[1] = Material {
            name: Some("layers".into()),
            clearcoat: Some(Clearcoat { texture: t(), roughness_texture: t(), normal_texture: t(), ..Default::default() }),
            transmission: Some(Transmission { texture: t(), ..Default::default() }),
            volume: Some(Volume { thickness_texture: t(), ..Default::default() }),
            specular: Some(Specular { texture: t(), color_texture: t(), ..Default::default() }),
            sheen: Some(Sheen { color_texture: t(), roughness_texture: t(), ..Default::default() }),
            iridescence: Some(Iridescence { texture: t(), thickness_texture: t(), ..Default::default() }),
            anisotropy: Some(Anisotropy { texture: t(), ..Default::default() }),
            ..Default::default()
        };
        let f = inspect(&scene).unwrap();
        let m = f.materials.iter().find(|m| m.name.as_deref() == Some("layers")).expect("the layered material");
        assert_eq!(
            m.maps,
            [
                "clearcoat",
                "clearcoat_roughness",
                "clearcoat_normal",
                "transmission",
                "volume_thickness",
                "specular",
                "specular_color",
                "sheen_color",
                "sheen_roughness",
                "iridescence",
                "iridescence_thickness",
                "anisotropy",
            ]
        );
    }

    #[test]
    fn a_skinned_mesh_reports_its_joints_root_and_clips_measured_from_keys() {
        let scene = fixtures::skinned();
        let f = inspect(&scene).unwrap();
        let skin = f.skin.expect("a skin");
        assert_eq!(skin.joints.len(), scene.skins[0].joints.len());
        assert_eq!(skin.root.as_deref(), scene.nodes[scene.skins[0].joints[0]].name.as_deref());
        assert!(skin.humanoid.is_none(), "a three-joint tube is no humanoid");
        assert_eq!(skin.bind_pose, BindPose::Unknown, "no humanoid map, no arms to measure");
        assert_eq!(f.animations.len(), scene.animations.len());
        for (clip, anim) in f.animations.iter().zip(&scene.animations) {
            let (a, b) = anim.time_range().unwrap();
            assert!((clip.duration_s - (b - a)).abs() < 1e-6);
            assert_eq!(clip.name, anim.name);
        }
    }

    /// Inspect a vendor's file without committing it (vendor outputs carry the
    /// vendor's licence): `LITEGEN_INSPECT_FILE=/path/x.glb cargo test --lib
    /// inspects_the_file_named_by_the_environment -- --ignored --nocapture`.
    #[test]
    #[ignore = "set LITEGEN_INSPECT_FILE to a mesh"]
    fn inspects_the_file_named_by_the_environment() {
        let path = std::env::var("LITEGEN_INSPECT_FILE").expect("LITEGEN_INSPECT_FILE");
        let bytes = std::fs::read(&path).unwrap();
        let format = super::super::MeshFormat::parse(path.rsplit('.').next().unwrap()).unwrap();
        let scene = super::super::read_scene(&bytes, format, &super::super::package::NoSidecars).unwrap().scene;
        let f = inspect(&scene).unwrap();
        println!("bounds {:?}\ntriangles {} vertices {} uv_sets {}", f.bounds, f.triangle_count, f.vertex_count, f.uv_sets);
        if let Some(s) = &f.skin {
            println!("root {:?} bind_pose {:?} joints {}\nhumanoid {:?}", s.root, s.bind_pose, s.joints.len(), s.humanoid.as_ref().map(|(t, m)| (t, m.len())));
        }
        for c in &f.animations {
            println!("clip {:?} {:.3}s travel {:?} in_place {}", c.name, c.duration_s, c.root_travel_m, c.in_place());
        }
    }

    #[test]
    fn duplicate_names_are_counted_not_merged() {
        let mut scene = fixtures::skinned();
        let j0 = scene.skins[0].joints[0];
        let j1 = scene.skins[0].joints[1];
        scene.nodes[j1].name = scene.nodes[j0].name.clone();
        let f = inspect(&scene).unwrap();
        assert_eq!(f.skin.unwrap().joints.len(), scene.skins[0].joints.len());
    }
}
