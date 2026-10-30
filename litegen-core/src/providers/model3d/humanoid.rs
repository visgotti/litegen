//! The skinned box humanoid `mock/rig-3d` returns (3D doc §3I).
//!
//! Seven joints in glTF conventions — +Y up, metres, facing +Z, its left side
//! at +X — each driving one box: a rig small enough to reason about in a test
//! and real enough for every reader, converter and animator to treat as one.
//! T-pose: the arms lie along ±X.

use crate::mesh::math::{self, Mat4};
use crate::mesh::scene::{Material, Node, Primitive, Scene, SceneMesh, Skin, SkinWeights, Transform};

/// The joint names `mock/rig-3d` gives its rig, in skin order:
/// hips, torso, head, arm_l, arm_r, leg_l, leg_r.
pub const MOCK_JOINTS: [&str; 7] = ["hips", "torso", "head", "arm_l", "arm_r", "leg_l", "leg_r"];

/// The same joints under their VRM 1.0 humanoid bone names, which
/// `joint_naming: "vrm"` asks for.
pub const VRM_JOINTS: [&str; 7] = ["hips", "spine", "head", "leftUpperArm", "rightUpperArm", "leftUpperLeg", "rightUpperLeg"];

/// (parent joint, world position of the joint, box min, box max), per joint in
/// skin order. Positions in metres; the boxes together stand 1.75 m tall.
const BONES: [(Option<usize>, [f64; 3], [f32; 3], [f32; 3]); 7] = [
    (None, [0.0, 0.95, 0.0], [-0.15, 0.85, -0.10], [0.15, 1.00, 0.10]),   // hips
    (Some(0), [0.0, 1.00, 0.0], [-0.17, 1.00, -0.10], [0.17, 1.45, 0.10]), // torso
    (Some(1), [0.0, 1.45, 0.0], [-0.11, 1.45, -0.11], [0.11, 1.75, 0.11]), // head
    (Some(1), [0.17, 1.40, 0.0], [0.17, 1.35, -0.05], [0.75, 1.43, 0.05]), // arm_l (+X)
    (Some(1), [-0.17, 1.40, 0.0], [-0.75, 1.35, -0.05], [-0.17, 1.43, 0.05]), // arm_r (-X)
    (Some(0), [0.09, 0.85, 0.0], [0.03, 0.0, -0.06], [0.15, 0.85, 0.06]),  // leg_l (+X)
    (Some(0), [-0.09, 0.85, 0.0], [-0.15, 0.0, -0.06], [-0.03, 0.85, 0.06]), // leg_r (-X)
];

fn add_box(prim: &mut Primitive, joint: u16, lo: [f32; 3], hi: [f32; 3]) {
    // Six faces, four corners each, wound counter-clockwise seen from outside.
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        ([1.0, 0.0, 0.0], [[hi[0], lo[1], lo[2]], [hi[0], hi[1], lo[2]], [hi[0], hi[1], hi[2]], [hi[0], lo[1], hi[2]]]),
        ([-1.0, 0.0, 0.0], [[lo[0], lo[1], hi[2]], [lo[0], hi[1], hi[2]], [lo[0], hi[1], lo[2]], [lo[0], lo[1], lo[2]]]),
        ([0.0, 1.0, 0.0], [[lo[0], hi[1], lo[2]], [lo[0], hi[1], hi[2]], [hi[0], hi[1], hi[2]], [hi[0], hi[1], lo[2]]]),
        ([0.0, -1.0, 0.0], [[lo[0], lo[1], hi[2]], [lo[0], lo[1], lo[2]], [hi[0], lo[1], lo[2]], [hi[0], lo[1], hi[2]]]),
        ([0.0, 0.0, 1.0], [[lo[0], lo[1], hi[2]], [hi[0], lo[1], hi[2]], [hi[0], hi[1], hi[2]], [lo[0], hi[1], hi[2]]]),
        ([0.0, 0.0, -1.0], [[hi[0], lo[1], lo[2]], [lo[0], lo[1], lo[2]], [lo[0], hi[1], lo[2]], [hi[0], hi[1], lo[2]]]),
    ];
    let normals = prim.normals.get_or_insert_with(Vec::new);
    let skin = prim.skin.get_or_insert_with(|| SkinWeights { influences: 4, ..Default::default() });
    for (n, corners) in faces {
        let base = prim.positions.len() as u32;
        for c in corners {
            prim.positions.push(c);
            normals.push(n);
            skin.joints.extend_from_slice(&[joint, 0, 0, 0]);
            skin.weights.extend_from_slice(&[1.0, 0.0, 0.0, 0.0]);
        }
        prim.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// The humanoid as a scene, its joints named `names` (skin order as in
/// [`MOCK_JOINTS`]).
pub fn rigged_humanoid(names: &[&str; 7]) -> Scene {
    let mut s = Scene { name: Some("mock-humanoid".into()), ..Default::default() };
    s.materials.push(Material { name: Some("body".into()), base_color: [0.8, 0.6, 0.4, 1.0], metallic: 0.0, roughness: 0.8, ..Default::default() });
    let mut prim = Primitive { material: Some(0), ..Default::default() };
    for (j, (_, _, lo, hi)) in BONES.iter().enumerate() {
        add_box(&mut prim, j as u16, *lo, *hi);
    }
    // A nose on the front of the head, so which way the figure faces can be
    // checked: +Z, the glTF front.
    add_box(&mut prim, 2, [-0.02, 1.58, 0.11], [0.02, 1.62, 0.16]);
    s.meshes.push(SceneMesh { name: Some("body".into()), primitives: vec![prim], ..Default::default() });

    // Node 0 holds the skinned mesh; joints are nodes 1..=7.
    s.nodes.push(Node { name: Some("body".into()), mesh: Some(0), skin: Some(0), ..Default::default() });
    for (j, (parent, world, _, _)) in BONES.iter().enumerate() {
        let local = match parent {
            Some(p) => {
                let pw = BONES[*p].1;
                [world[0] - pw[0], world[1] - pw[1], world[2] - pw[2]]
            }
            None => *world,
        };
        s.nodes.push(Node {
            name: Some(names[j].to_string()),
            transform: Transform::Trs { translation: local, rotation: math::QUAT_IDENTITY, scale: [1.0; 3] },
            ..Default::default()
        });
        if let Some(p) = parent {
            s.nodes[1 + p].children.push(1 + j);
        }
    }
    s.roots = vec![0, 1];
    let inverse_bind: Vec<Mat4> = BONES.iter().map(|(_, w, _, _)| math::translation([-w[0], -w[1], -w[2]])).collect();
    s.skins.push(Skin { name: Some("humanoid".into()), joints: (1..=7).collect(), inverse_bind_matrices: inverse_bind, skeleton: Some(1) });
    s
}

/// The humanoid turned a quarter left, to face +X, as a vendor that builds
/// +X-forward delivers it: what `mock/turned-3d` emits.
pub fn turned_humanoid_glb() -> Vec<u8> {
    let mut s = rigged_humanoid(&MOCK_JOINTS);
    let children = std::mem::take(&mut s.roots);
    let h = std::f64::consts::FRAC_1_SQRT_2;
    s.add_root(Node {
        name: Some("vendor_root".into()),
        // +90° about +Y: +Z (the face) turns to +X.
        transform: Transform::Trs { translation: [0.0; 3], rotation: [0.0, h, 0.0, h], scale: [1.0; 3] },
        children,
        ..Default::default()
    });
    crate::mesh::glb::write_scene(&s).expect("the turned humanoid is a valid scene")
}

/// [`rigged_humanoid`] as a GLB.
pub fn rigged_humanoid_glb(names: &[&str; 7]) -> Vec<u8> {
    crate::mesh::glb::write_scene(&rigged_humanoid(names)).expect("the mock humanoid is a valid scene")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mock_humanoid_reads_back_as_one_skin_over_its_seven_joints() {
        let bytes = rigged_humanoid_glb(&MOCK_JOINTS);
        let read = crate::mesh::read_scene(&bytes, crate::mesh::MeshFormat::Glb, &crate::mesh::package::NoSidecars).unwrap();
        let scene = read.scene;
        assert_eq!(scene.skins.len(), 1);
        let names: Vec<&str> = scene.skins[0].joints.iter().map(|&j| scene.nodes[j].name.as_deref().unwrap_or("")).collect();
        assert_eq!(names, ["hips", "torso", "head", "arm_l", "arm_r", "leg_l", "leg_r"]);
        assert!(scene.triangle_count() >= 7 * 12, "a box per bone");
        let mesh = crate::mesh::pose::flatten(&scene).unwrap();
        let (lo, hi) = mesh.bounds().unwrap();
        assert!(lo[1].abs() < 1e-4 && (hi[1] - 1.75).abs() < 1e-3, "stands on y = 0, 1.75 m tall: {lo:?} {hi:?}");
        // glTF: the subject faces +Z and its left is +X.
        let arm_l = scene.nodes.iter().position(|n| n.name.as_deref() == Some("arm_l")).unwrap();
        let world = scene.world_matrices();
        assert!(world[arm_l][12] > 0.0, "the left arm is on +X");
    }

    #[test]
    fn renamed_joints_keep_the_skeleton() {
        let bytes = rigged_humanoid_glb(&VRM_JOINTS);
        let scene = crate::mesh::read_scene(&bytes, crate::mesh::MeshFormat::Glb, &crate::mesh::package::NoSidecars).unwrap().scene;
        let names: Vec<&str> = scene.skins[0].joints.iter().map(|&j| scene.nodes[j].name.as_deref().unwrap_or("")).collect();
        assert_eq!(names, ["hips", "spine", "head", "leftUpperArm", "rightUpperArm", "leftUpperLeg", "rightUpperLeg"]);
    }
}
