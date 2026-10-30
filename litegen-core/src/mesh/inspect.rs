//! What litegen's own reader finds in a mesh (PixelGenie 3D doc §3D).
//!
//! Every delivered mesh is described by what litegen measured in it, never by
//! what its vendor claims: bounds at rest, vertex and triangle counts, uv sets,
//! vertex colour, each material's texture maps, the skin (its joints, root,
//! humanoid map and bind pose) and every clip's length and root travel,
//! measured from its keyframes.

use super::math::Mat4;
use super::scene::{Scene, TextureSlot};
use super::{pose, MeshError};

/// A clip moves its root less than this (horizontally, start to end) to count
/// as in place.
pub const IN_PLACE_TOLERANCE_M: f32 = 0.01;

#[derive(Debug, Clone, PartialEq)]
pub struct SceneFacts {
    /// World-space bounds at rest (the bind pose for a skinned mesh).
    pub bounds: Option<([f32; 3], [f32; 3])>,
    pub vertex_count: usize,
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
    /// `base_color`, `metallic_roughness`, `normal`, `occlusion`, `emissive`.
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
    /// World-space displacement of the skin's root joint, first key to last.
    pub root_travel_m: [f32; 3],
}

impl ClipFacts {
    /// Whether the root ends where it started, horizontally.
    pub fn in_place(&self) -> bool {
        self.root_travel_m[0].hypot(self.root_travel_m[2]) < IN_PLACE_TOLERANCE_M
    }
}

fn slot_name(slot: TextureSlot) -> &'static str {
    match slot {
        TextureSlot::BaseColor => "base_color",
        TextureSlot::MetallicRoughness => "metallic_roughness",
        TextureSlot::Normal => "normal",
        TextureSlot::Occlusion => "occlusion",
        TextureSlot::Emissive => "emissive",
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

/// Centre of the bind-pose vertices whose strongest influence is skin joint
/// `joint_index`. At bind pose a skinned vertex sits where it is stored.
fn driven_centroid(scene: &Scene, skin: usize, joint_index: usize) -> Option<[f64; 3]> {
    let mut sum = [0.0f64; 3];
    let mut n = 0usize;
    for node in scene.nodes.iter().filter(|n| n.skin == Some(skin)) {
        let Some(mesh) = node.mesh else { continue };
        for prim in &scene.meshes[mesh].primitives {
            let Some(sw) = prim.skin.as_ref() else { continue };
            for (v, p) in prim.positions.iter().enumerate() {
                let (js, ws) = sw.vertex(v);
                let strongest = js.iter().zip(ws).max_by(|a, b| a.1.total_cmp(b.1)).map(|(j, _)| *j as usize);
                if strongest == Some(joint_index) {
                    for k in 0..3 {
                        sum[k] += f64::from(p[k]);
                    }
                    n += 1;
                }
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
    let flat = pose::flatten(scene)?;
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

    let root = scene.skins.first().and_then(|s| root_joint(scene, &s.joints, s.skeleton));
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
            ClipFacts { name: anim.name.clone(), duration_s: end - start, root_travel_m }
        })
        .collect();

    Ok(SceneFacts {
        bounds: flat.bounds(),
        vertex_count: scene.vertex_count(),
        triangle_count: scene.triangle_count(),
        uv_sets,
        vertex_colors,
        materials,
        skin,
        animations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::fixtures;

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
