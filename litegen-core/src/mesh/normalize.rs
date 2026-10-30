//! Bringing a vendor's mesh to the conventions every delivery meets
//! (PixelGenie 3D doc §2 rule 3, §3D).
//!
//! glTF 2.0's: +Y up, metres, right-handed, the asset's front facing +Z and its
//! right side at −X; textures a core glTF reader decodes (PNG, JPEG); node
//! names a map can key on. Each step reports whether it changed anything, so
//! the caller knows to deliver the vendor's own file beside the result.

use super::math::{self, Mat4};
use super::scene::{AnimPath, ImageFormat, Interpolation, Node, Scene, Transform};
use super::MeshError;

/// A signed coordinate axis, as orientations are declared (`"+x"`, `"-z"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl Axis {
    pub fn parse(s: &str) -> Option<Axis> {
        match s.trim().to_ascii_lowercase().as_str() {
            "+x" | "x" => Some(Axis::PosX),
            "-x" => Some(Axis::NegX),
            "+y" | "y" => Some(Axis::PosY),
            "-y" => Some(Axis::NegY),
            "+z" | "z" => Some(Axis::PosZ),
            "-z" => Some(Axis::NegZ),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Axis::PosX => "+x",
            Axis::NegX => "-x",
            Axis::PosY => "+y",
            Axis::NegY => "-y",
            Axis::PosZ => "+z",
            Axis::NegZ => "-z",
        }
    }

    fn vector(self) -> [f64; 3] {
        match self {
            Axis::PosX => [1.0, 0.0, 0.0],
            Axis::NegX => [-1.0, 0.0, 0.0],
            Axis::PosY => [0.0, 1.0, 0.0],
            Axis::NegY => [0.0, -1.0, 0.0],
            Axis::PosZ => [0.0, 0.0, 1.0],
            Axis::NegZ => [0.0, 0.0, -1.0],
        }
    }
}

/// A direction through `m`'s linear part (no translation).
fn transform_vector(m: &Mat4, v: [f64; 3]) -> [f64; 3] {
    [
        m[0] * v[0] + m[4] * v[1] + m[8] * v[2],
        m[1] * v[0] + m[5] * v[1] + m[9] * v[2],
        m[2] * v[0] + m[6] * v[1] + m[10] * v[2],
    ]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Turn a scene whose vendor builds `up`-up, `front`-facing into +Y up,
/// +Z front, with one new root node, so skins and clips are untouched.
/// `Ok(false)` when it already is.
pub fn orient(scene: &mut Scene, up: Axis, front: Axis) -> Result<bool, MeshError> {
    let (u, f) = (up.vector(), front.vector());
    if u.iter().zip(&f).map(|(a, b)| a * b).sum::<f64>().abs() > 1e-9 {
        return Err(MeshError::Malformed(format!("up {} and front {} are not perpendicular", up.as_str(), front.as_str())));
    }
    if up == Axis::PosY && front == Axis::PosZ {
        return Ok(false);
    }
    // Rows map the vendor's frame onto glTF's: u → +Y, f → +Z, u×f → +X.
    let x = cross(u, f);
    let r: Mat4 = [
        x[0], u[0], f[0], 0.0,
        x[1], u[1], f[1], 0.0,
        x[2], u[2], f[2], 0.0,
        0.0, 0.0, 0.0, 1.0,
    ];
    let children = std::mem::take(&mut scene.roots);
    scene.add_root(Node {
        name: Some("litegen_orientation".into()),
        transform: Transform::from_matrix(r),
        children,
        ..Default::default()
    });
    Ok(true)
}

/// Give every node a distinct name by suffixing repeats `_1`, `_2`, …, so a
/// map from bone to joint name is unambiguous. Whether anything was renamed.
pub fn unique_node_names(scene: &mut Scene) -> bool {
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut changed = false;
    for i in 0..scene.nodes.len() {
        let Some(name) = scene.nodes[i].name.clone() else { continue };
        if taken.insert(name.clone()) {
            continue;
        }
        let mut n = 1;
        let fresh = loop {
            let candidate = format!("{name}_{n}");
            if !scene.nodes.iter().any(|node| node.name.as_deref() == Some(candidate.as_str())) && !taken.contains(&candidate) {
                break candidate;
            }
            n += 1;
        };
        taken.insert(fresh.clone());
        scene.nodes[i].name = Some(fresh);
        changed = true;
    }
    changed
}

/// A segmented mesh's parts (3D doc §3F) are its mesh-bearing nodes, each
/// named: the vendor's label, else `part_<n>` for the n-th part in node
/// order. Whether any was named.
pub fn name_parts(scene: &mut Scene) -> bool {
    let mut changed = false;
    for (n, node) in scene.nodes.iter_mut().filter(|node| node.mesh.is_some()).enumerate() {
        if node.name.as_deref().is_none_or(|s| s.trim().is_empty()) {
            node.name = Some(format!("part_{n}"));
            changed = true;
        }
    }
    changed
}

/// Rename nodes by `(from, to)`. Geometry never changes. Whether any did.
pub fn rename_nodes(scene: &mut Scene, renames: &[(String, String)]) -> bool {
    let mut changed = false;
    for node in &mut scene.nodes {
        if let Some((_, to)) = renames.iter().find(|(from, _)| node.name.as_deref() == Some(from.as_str())) {
            if node.name.as_deref() != Some(to.as_str()) {
                node.name = Some(to.clone());
                changed = true;
            }
        }
    }
    changed
}

/// Name the scene's clips `names`, in order. Whether any name changed.
pub fn name_animations(scene: &mut Scene, names: &[String]) -> bool {
    let mut changed = false;
    for (anim, name) in scene.animations.iter_mut().zip(names) {
        if anim.name.as_deref() != Some(name.as_str()) {
            anim.name = Some(name.clone());
            changed = true;
        }
    }
    changed
}

/// Hold the skin root's horizontal (world X and Z) position at its first key
/// for the whole of clip `anim`, keeping vertical movement — Mixamo's "In
/// Place". Done in world space, through the root's parent, so a scaled or
/// rotated armature (Meshy's 0.01-scale `Armature`) is handled. Whether keys changed.
pub fn remove_root_motion(scene: &mut Scene, anim: usize) -> bool {
    let Some(skin) = scene.skins.first() else { return false };
    let joints = skin.joints.clone();
    let parents = scene.parents();
    let Some(root) = joints.iter().copied().find(|&j| {
        let mut p = parents[j];
        while let Some(n) = p {
            if joints.contains(&n) {
                return false;
            }
            p = parents[n];
        }
        true
    }) else {
        return false;
    };
    let parent_world = parents[root].map(|p| scene.world_matrices()[p]).unwrap_or(math::IDENTITY);
    let Some(parent_inv) = math::invert(&parent_world) else { return false };
    let Some(ch) = scene.animations.get_mut(anim).and_then(|a| {
        a.channels.iter_mut().find(|c| c.node == root && c.path == AnimPath::Translation)
    }) else {
        return false;
    };
    let cubic = ch.interpolation == Interpolation::CubicSpline;
    let per_key = if cubic { 9 } else { 3 };
    let value_at = |vals: &[f32], k: usize| -> [f64; 3] {
        let o = k * per_key + if cubic { 3 } else { 0 };
        [f64::from(vals[o]), f64::from(vals[o + 1]), f64::from(vals[o + 2])]
    };
    let keys = ch.values.len() / per_key;
    if keys == 0 {
        return false;
    }
    let to_world = |l: [f64; 3]| math::transform_point_f64(&parent_world, l);
    let start = to_world(value_at(&ch.values, 0));
    let mut changed = false;
    for k in 0..keys {
        let w = to_world(value_at(&ch.values, k));
        let held = [start[0], w[1], start[2]];
        let local = math::transform_point_f64(&parent_inv, held);
        let o = k * per_key + if cubic { 3 } else { 0 };
        for i in 0..3 {
            let v = local[i] as f32;
            if (ch.values[o + i] - v).abs() > 1e-7 {
                ch.values[o + i] = v;
                changed = true;
            }
        }
        if cubic {
            // Tangents move nothing horizontally once the value is held.
            for t in [k * per_key, k * per_key + 6] {
                let tw = transform_vector(&parent_world, [f64::from(ch.values[t]), f64::from(ch.values[t + 1]), f64::from(ch.values[t + 2])]);
                let tl = transform_vector(&parent_inv, [0.0, tw[1], 0.0]);
                for i in 0..3 {
                    ch.values[t + i] = tl[i] as f32;
                }
            }
        }
    }
    changed
}

/// What [`transcode_textures`] did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TranscodeReport {
    pub changed: bool,
    /// Textures dropped because they could not be decoded (KTX2/Basis).
    pub dropped: Vec<String>,
}

/// Key every channel of animation `anim` at `fps`: evenly spaced from the
/// clip's start, with its end kept, each value sampled from the source curve.
/// Linear and step channels keep their interpolation; a cubic spline becomes
/// linear through its samples. Whether anything changed: a clip already keyed
/// so is left alone.
pub fn resample(scene: &mut Scene, anim: usize, fps: u32) -> bool {
    let Some(a) = scene.animations.get(anim) else { return false };
    let Some((lo, hi)) = a.time_range() else { return false };
    if fps == 0 || hi <= lo {
        return false;
    }
    let step = 1.0 / fps as f64;
    let intervals = (((hi - lo) as f64) / step).floor() as usize;
    let mut times: Vec<f32> = (0..=intervals).map(|k| (lo as f64 + k as f64 * step) as f32).collect();
    if times.last().is_some_and(|t| (hi - t).abs() > 1e-4) {
        times.push(hi);
    }
    let on_grid = a.channels.iter().all(|c| {
        c.times.len() == times.len() && c.times.iter().zip(&times).all(|(x, y)| (x - y).abs() < 1e-4)
    });
    if on_grid {
        return false;
    }
    let targets: Vec<usize> = a
        .channels
        .iter()
        .map(|c| scene.nodes.get(c.node).and_then(|n| n.mesh).map_or(0, |m| scene.meshes[m].target_count()))
        .collect();
    let a = &mut scene.animations[anim];
    for (ch, targets) in a.channels.iter_mut().zip(targets) {
        let stride = ch.stride(targets);
        let values: Vec<f32> = times.iter().flat_map(|&t| super::pose::sample_channel(ch, t, stride)).collect();
        ch.times = times.clone();
        ch.values = values;
        if ch.interpolation == Interpolation::CubicSpline {
            ch.interpolation = Interpolation::Linear;
        }
    }
    true
}

/// Remove clips that do not move: no keys, or every key at one instant (a
/// pose, like the bind-pose clip Meshy ships in its rigged file). How many
/// went.
pub fn drop_static_animations(scene: &mut Scene) -> usize {
    let before = scene.animations.len();
    scene.animations.retain(|a| a.time_range().is_some_and(|(lo, hi)| hi - lo > 1e-6));
    before - scene.animations.len()
}

/// Re-encode every WebP image as PNG: core glTF readers decode only PNG and
/// JPEG, and a delivered file must need no extension to read. A KTX2 image
/// (Basis Universal, which nothing in-process decodes) is dropped from the
/// materials that use it and reported; the vendor's file, delivered beside,
/// keeps it.
pub fn transcode_textures(scene: &mut Scene) -> TranscodeReport {
    let mut report = TranscodeReport::default();
    let mut dead_images = Vec::new();
    for (i, img) in scene.images.iter_mut().enumerate() {
        match img.format {
            ImageFormat::Webp => match image::load_from_memory_with_format(&img.data, image::ImageFormat::WebP) {
                Ok(decoded) => {
                    let mut png = Vec::new();
                    if decoded.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_ok() {
                        img.data = png;
                        img.format = ImageFormat::Png;
                        report.changed = true;
                    } else {
                        dead_images.push(i);
                    }
                }
                Err(_) => dead_images.push(i),
            },
            ImageFormat::Ktx2 => dead_images.push(i),
            ImageFormat::Png | ImageFormat::Jpeg => {}
        }
    }
    if dead_images.is_empty() {
        return report;
    }
    let dead_textures: Vec<usize> =
        scene.textures.iter().enumerate().filter(|(_, t)| dead_images.contains(&t.image)).map(|(i, _)| i).collect();
    for m in &mut scene.materials {
        for slot in [
            &mut m.base_color_texture,
            &mut m.metallic_roughness_texture,
            &mut m.normal_texture,
            &mut m.occlusion_texture,
            &mut m.emissive_texture,
        ] {
            if slot.as_ref().is_some_and(|t| dead_textures.contains(&t.texture)) {
                *slot = None;
                report.changed = true;
            }
        }
    }
    for i in dead_images {
        report.dropped.push(scene.images[i].name.clone().unwrap_or_else(|| format!("image {i}")));
    }
    super::pose::prune_unused_textures(scene);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::fixtures;
    use super::super::scene::{Animation, AnimPath, Channel, Interpolation, Node, Primitive, Scene, SceneMesh};

    /// One triangle near +X, so where +X ends up is visible.
    fn marker() -> Scene {
        let mut s = Scene::default();
        s.meshes.push(SceneMesh {
            primitives: vec![Primitive {
                positions: vec![[1.0, 0.0, 0.0], [1.0, 0.1, 0.0], [1.0, 0.0, 0.1]],
                indices: vec![0, 1, 2],
                ..Default::default()
            }],
            ..Default::default()
        });
        s.nodes.push(Node { name: Some("marker".into()), mesh: Some(0), ..Default::default() });
        s.roots = vec![0];
        s
    }

    fn centroid(scene: &Scene) -> [f32; 3] {
        let m = super::super::pose::flatten(scene).unwrap();
        let (lo, hi) = m.bounds().unwrap();
        [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0]
    }

    #[test]
    fn a_mesh_facing_x_is_turned_to_face_z() {
        let mut s = marker();
        assert!(orient(&mut s, Axis::PosY, Axis::PosX).unwrap());
        let c = centroid(&s);
        assert!(c[2] > 0.9 && c[0].abs() < 0.11, "what faced +X now faces +Z: {c:?}");
    }

    #[test]
    fn a_z_up_mesh_is_stood_up() {
        let mut s = marker();
        // The marker's +X is its front; the vendor's up was +Z.
        assert!(orient(&mut s, Axis::PosZ, Axis::PosX).unwrap());
        let mut probe = marker();
        probe.meshes[0].primitives[0].positions = vec![[0.0, 0.0, 1.0], [0.0, 0.1, 1.0], [0.1, 0.0, 1.0]];
        orient(&mut probe, Axis::PosZ, Axis::PosX).unwrap();
        assert!(centroid(&probe)[1] > 0.9, "what was up (+Z) is now up (+Y)");
        assert!(centroid(&s)[2] > 0.9, "and the front faces +Z");
    }

    #[test]
    fn a_mesh_already_in_conventions_is_left_alone() {
        let mut s = marker();
        let before = s.clone();
        assert!(!orient(&mut s, Axis::PosY, Axis::PosZ).unwrap());
        assert_eq!(s, before);
        assert!(orient(&mut s, Axis::PosY, Axis::PosY).is_err(), "up and front cannot be one axis");
    }

    #[test]
    fn duplicate_node_names_get_numeric_suffixes() {
        let mut s = marker();
        s.nodes.push(Node { name: Some("marker".into()), ..Default::default() });
        s.nodes.push(Node { name: Some("marker".into()), ..Default::default() });
        s.roots = vec![0, 1, 2];
        assert!(unique_node_names(&mut s));
        let names: Vec<_> = s.nodes.iter().map(|n| n.name.clone().unwrap()).collect();
        assert_eq!(names, ["marker", "marker_1", "marker_2"]);
        assert!(!unique_node_names(&mut s), "already unique");
    }

    #[test]
    fn joints_and_clips_are_renamed_without_touching_geometry() {
        let mut s = fixtures::skinned();
        let before = super::super::pose::flatten(&s).unwrap();
        let j0 = s.skins[0].joints[0];
        let old = s.nodes[j0].name.clone().unwrap();
        assert!(rename_nodes(&mut s, &[(old, "hips".into())]));
        assert_eq!(s.nodes[j0].name.as_deref(), Some("hips"));
        assert_eq!(super::super::pose::flatten(&s).unwrap(), before);
        let names: Vec<String> = (0..s.animations.len()).map(|i| format!("clip{i}")).collect();
        assert!(name_animations(&mut s, &names));
        assert_eq!(s.animations[0].name.as_deref(), Some("clip0"));
    }

    #[test]
    fn root_motion_is_removed_horizontally_and_kept_vertically() {
        let mut s = fixtures::skinned();
        let root = s.skins[0].joints[0];
        let (t0, t1) = (0.0f32, 1.0f32);
        s.animations = vec![Animation {
            name: Some("walk".into()),
            channels: vec![Channel {
                node: root,
                path: AnimPath::Translation,
                interpolation: Interpolation::Linear,
                times: vec![t0, t1],
                values: vec![0.0, 0.0, 0.0, 0.3, 0.2, 1.0],
            }],
        }];
        let before = super::super::inspect::inspect(&s).unwrap().animations[0].root_travel_m;
        assert!(before[2] > 0.9);
        assert!(remove_root_motion(&mut s, 0));
        let after = super::super::inspect::inspect(&s).unwrap().animations[0].root_travel_m;
        assert!(after[0].abs() < 1e-5 && after[2].abs() < 1e-5, "{after:?}");
        assert!((after[1] - before[1]).abs() < 1e-5, "vertical movement is kept: {after:?} vs {before:?}");
    }

    #[test]
    fn a_clip_is_rekeyed_at_the_asked_rate_and_left_alone_when_it_already_is() {
        let mut s = marker();
        s.animations.push(Animation {
            name: Some("slide".into()),
            channels: vec![Channel {
                node: 0,
                path: AnimPath::Translation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![0.0, 0.0, 0.0, 0.0, 0.0, 2.0],
            }],
        });
        assert!(resample(&mut s, 0, 4));
        let ch = &s.animations[0].channels[0];
        assert_eq!(ch.times, [0.0, 0.25, 0.5, 0.75, 1.0]);
        assert!((ch.values[3 * 2 + 2] - 1.0).abs() < 1e-6, "halfway, halfway there: {:?}", ch.values);
        assert!(!resample(&mut s, 0, 4), "already keyed at 4 fps");
    }

    #[test]
    fn static_clips_are_told_apart_from_motion() {
        let mut s = marker();
        let still = |name: &str, times: Vec<f32>| Animation {
            name: Some(name.into()),
            channels: vec![Channel { node: 0, path: AnimPath::Translation, interpolation: Interpolation::Linear, values: vec![0.0; 3 * times.len()], times }],
        };
        s.animations = vec![still("pose", vec![0.0]), still("walk", vec![0.0, 1.0])];
        assert_eq!(drop_static_animations(&mut s), 1);
        assert_eq!(s.animations.iter().map(|a| a.name.clone().unwrap()).collect::<Vec<_>>(), ["walk"]);
    }

    #[test]
    fn webp_textures_become_png_and_nothing_needs_an_extension() {
        let mut s = fixtures::textured();
        let png = s.images[0].data.clone();
        let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .encode(img.as_bytes(), img.width(), img.height(), img.color().into())
            .unwrap();
        s.images[0].format = super::super::scene::ImageFormat::Webp;
        s.images[0].data = webp;
        let report = transcode_textures(&mut s);
        assert!(report.changed);
        assert_eq!(s.images[0].format, super::super::scene::ImageFormat::Png);
        assert!(s.images[0].data.starts_with(&[0x89, b'P', b'N', b'G']));
        let glb = super::super::glb::write_scene(&s).unwrap();
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json = std::str::from_utf8(&glb[20..20 + json_len]).unwrap();
        assert!(!json.contains("EXT_texture_webp") && !json.contains("extensionsRequired"), "{json}");
    }

    #[test]
    fn unnamed_parts_are_named_by_their_place_and_labels_are_kept() {
        let mut scene = super::super::fixtures::cube();
        let mesh = scene.nodes.iter().find_map(|n| n.mesh).unwrap();
        let plain = |name: Option<&str>| super::super::scene::Node {
            name: name.map(String::from),
            transform: super::super::scene::Transform::Trs { translation: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
            children: Vec::new(),
            mesh: Some(mesh),
            skin: None,
            weights: None,
        };
        scene.nodes = vec![plain(Some("visor")), plain(None), plain(Some("")), plain(None)];
        scene.nodes[3].mesh = None;
        scene.roots = vec![0, 1, 2, 3];
        assert!(name_parts(&mut scene));
        let names: Vec<Option<&str>> = scene.nodes.iter().map(|n| n.name.as_deref()).collect();
        assert_eq!(names, [Some("visor"), Some("part_1"), Some("part_2"), None], "a node without a mesh is no part");
        assert!(!name_parts(&mut scene), "nothing left to name");
    }
}
