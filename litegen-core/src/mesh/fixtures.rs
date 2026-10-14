//! Deterministic scenes that exercise everything the converter claims to carry.
//!
//! These are the inputs of the conversion matrix and of the render verification
//! harness (`examples/mesh_matrix.rs`, `scripts/mesh-verify/`). Each one is built
//! to make a specific class of mistake VISIBLE, which is the whole point of a
//! fixture: a cube centred on the origin survives an axis swap, a texture that is
//! symmetric survives a V-flip, an animation keyed only at its endpoints survives
//! an interpolation bug. So:
//!
//! - [`textured`]: an asymmetric texture (four coloured quadrants and a diagonal
//!   stripe from one corner) on a box, a flat-coloured wedge under non-uniform
//!   scale, a vertex-coloured panel, a mirrored instance of the box, all under a
//!   rotated root. A flipped V, a swapped axis, a dropped transform, a lost mirror
//!   or a reversed winding each shows up as a different picture.
//! - [`skinned`]: a banded tube bound to a three-joint chain and bent by an
//!   animation with LINEAR, STEP and CUBICSPLINE channels whose keys are placed
//!   so that sampling BETWEEN them is where the differences are.
//! - [`morph`]: a grid with two morph targets (one with normal offsets) and an
//!   animation of their weights.
//! - [`cube`]: the flat, attribute-complete cube the original matrix used.
//!
//! Everything here is plain arithmetic: no randomness, no clock, no
//! environment. Two runs produce byte-identical files.

use super::math;
use super::scene::{
    AnimPath, Animation, Channel, Image, ImageFormat, Interpolation, Material, MorphTarget, Node,
    Primitive, Sampler, Scene, SceneMesh, Skin, SkinWeights, Texture, TextureRef, Transform, Wrap,
};

/// Every fixture by name, in a stable order.
pub fn all() -> Vec<(&'static str, Scene)> {
    vec![("textured", textured()), ("skinned", skinned()), ("morph", morph()), ("cube", cube())]
}

pub fn by_name(name: &str) -> Option<Scene> {
    all().into_iter().find(|(n, _)| *n == name).map(|(_, s)| s)
}

fn trs(t: [f64; 3], r: [f64; 4], s: [f64; 3]) -> Transform {
    Transform::Trs { translation: t, rotation: math::quat_normalize(r), scale: s }
}

fn axis_angle(axis: [f64; 3], degrees: f64) -> [f64; 4] {
    math::quat_from_axis_angle(axis, degrees.to_radians())
}

// ─── images ─────────────────────────────────────────────────────────────────

/// A minimal PNG encoder: 8-bit RGBA, no filtering, stored (uncompressed)
/// deflate blocks. Fixture images are a few KB, so compression would buy
/// nothing, and a stored stream is short enough to read by eye.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    let row = width as usize * 4;
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0); // filter type: None
        raw.extend_from_slice(&rgba[y * row..(y + 1) * row]);
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65_535).peekable();
    while let Some(c) = chunks.next() {
        z.push(if chunks.peek().is_none() { 1 } else { 0 });
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, deflate, no filter, no interlace
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// 64x64: red top-left, green top-right, blue bottom-left, yellow bottom-right,
/// with a white diagonal stripe running from the top-left corner towards the
/// centre and a black dot near the top edge. Every flip, transpose and rotation
/// of this image is a different picture.
pub fn quadrant_png() -> Vec<u8> {
    let (w, h) = (64u32, 64u32);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let left = x < w / 2;
            let top = y < h / 2;
            let mut c = match (top, left) {
                (true, true) => [220, 40, 40, 255],
                (true, false) => [40, 190, 60, 255],
                (false, true) => [50, 70, 220, 255],
                (false, false) => [235, 210, 40, 255],
            };
            if x == y && x < w / 2 || (x + 1 == y || x == y + 1) && x < w / 2 {
                c = [255, 255, 255, 255];
            }
            if (44..50).contains(&x) && (4..10).contains(&y) {
                c = [10, 10, 10, 255];
            }
            let i = ((y * w + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&c);
        }
    }
    encode_png(w, h, &px)
}

/// 16x64 vertical bands, so bending a tube that wraps it is visible.
pub fn bands_png() -> Vec<u8> {
    let (w, h) = (16u32, 64u32);
    let colors: [[u8; 4]; 4] = [[230, 90, 40, 255], [250, 240, 230, 255], [40, 120, 200, 255], [30, 30, 40, 255]];
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let mut c = colors[(y / 8 % 4) as usize];
            // A thin seam marker on one side shows which way the tube faces.
            if x < 2 {
                c = [200, 30, 200, 255];
            }
            let i = ((y * w + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&c);
        }
    }
    encode_png(w, h, &px)
}

// ─── geometry builders ──────────────────────────────────────────────────────

/// A unit box centred on the origin: 24 vertices so each face has its own
/// normals and the full texture.
fn box_primitive(material: Option<usize>) -> Primitive {
    // Each face: normal, then the four corners counter-clockwise as seen from
    // outside, starting at the corner that maps to UV (0, 1) — the image's
    // bottom-left in glTF's top-left-origin convention.
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        ([0.0, 0.0, 1.0], [[-0.5, -0.5, 0.5], [0.5, -0.5, 0.5], [0.5, 0.5, 0.5], [-0.5, 0.5, 0.5]]),
        ([0.0, 0.0, -1.0], [[0.5, -0.5, -0.5], [-0.5, -0.5, -0.5], [-0.5, 0.5, -0.5], [0.5, 0.5, -0.5]]),
        ([1.0, 0.0, 0.0], [[0.5, -0.5, 0.5], [0.5, -0.5, -0.5], [0.5, 0.5, -0.5], [0.5, 0.5, 0.5]]),
        ([-1.0, 0.0, 0.0], [[-0.5, -0.5, -0.5], [-0.5, -0.5, 0.5], [-0.5, 0.5, 0.5], [-0.5, 0.5, -0.5]]),
        ([0.0, 1.0, 0.0], [[-0.5, 0.5, 0.5], [0.5, 0.5, 0.5], [0.5, 0.5, -0.5], [-0.5, 0.5, -0.5]]),
        ([0.0, -1.0, 0.0], [[-0.5, -0.5, -0.5], [0.5, -0.5, -0.5], [0.5, -0.5, 0.5], [-0.5, -0.5, 0.5]]),
    ];
    let corner_uv: [[f32; 2]; 4] = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    let mut p = Primitive { material, ..Default::default() };
    let (mut normals, mut uvs) = (Vec::new(), Vec::new());
    for (n, corners) in faces {
        let base = p.positions.len() as u32;
        for (i, c) in corners.iter().enumerate() {
            p.positions.push(*c);
            normals.push(n);
            uvs.push(corner_uv[i]);
        }
        p.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    p.normals = Some(normals);
    p.uvs = vec![uvs];
    p
}

/// A triangular prism, flat-shaded, along Z.
fn wedge_primitive(material: Option<usize>) -> Primitive {
    let a = [-0.5f32, -0.5];
    let b = [0.5f32, -0.5];
    let c = [0.0f32, 0.5];
    let z0 = -0.5f32;
    let z1 = 0.5f32;
    let mut p = Primitive { material, ..Default::default() };
    let mut normals = Vec::new();
    let mut tri = |p: &mut Primitive, v: [[f32; 3]; 3]| {
        let n = super::ir::Mesh::facet_normal(&v);
        let base = p.positions.len() as u32;
        for x in v {
            p.positions.push(x);
            normals.push(n);
        }
        p.indices.extend_from_slice(&[base, base + 1, base + 2]);
    };
    let at = |xy: [f32; 2], z: f32| [xy[0], xy[1], z];
    // Caps.
    tri(&mut p, [at(a, z1), at(b, z1), at(c, z1)]);
    tri(&mut p, [at(a, z0), at(c, z0), at(b, z0)]);
    // Sides as two triangles each.
    for (u, v) in [(a, b), (b, c), (c, a)] {
        tri(&mut p, [at(u, z0), at(v, z0), at(v, z1)]);
        tri(&mut p, [at(u, z0), at(v, z1), at(u, z1)]);
    }
    p.normals = Some(normals);
    p
}

/// An `n`x`n` grid in the XY plane facing +Z, with vertex colours and UVs.
fn grid_primitive(n: usize, size: f32, material: Option<usize>, colored: bool) -> Primitive {
    let mut p = Primitive { material, ..Default::default() };
    let (mut normals, mut uvs, mut colors) = (Vec::new(), Vec::new(), Vec::new());
    for j in 0..=n {
        for i in 0..=n {
            let u = i as f32 / n as f32;
            let v = j as f32 / n as f32;
            p.positions.push([(u - 0.5) * size, (v - 0.5) * size, 0.0]);
            normals.push([0.0, 0.0, 1.0]);
            uvs.push([u, 1.0 - v]);
            // Linear colours: red rises along x, blue falls, green rises up y.
            colors.push([u, v, 1.0 - u, 1.0]);
        }
    }
    let row = (n + 1) as u32;
    for j in 0..n as u32 {
        for i in 0..n as u32 {
            let a = j * row + i;
            p.indices.extend_from_slice(&[a, a + 1, a + row + 1, a, a + row + 1, a + row]);
        }
    }
    p.normals = Some(normals);
    p.uvs = vec![uvs];
    if colored {
        p.colors = Some(colors);
    }
    p
}

// ─── fixtures ───────────────────────────────────────────────────────────────

/// Static: textures, materials, vertex colours, hierarchy, non-uniform scale,
/// instancing and a mirror.
pub fn textured() -> Scene {
    let mut s = Scene { name: Some("textured".into()), ..Default::default() };
    s.images.push(Image { name: Some("quadrants".into()), format: ImageFormat::Png, data: quadrant_png() });
    s.textures.push(Texture {
        name: Some("quadrants".into()),
        image: 0,
        sampler: Sampler { wrap_s: Wrap::Repeat, wrap_t: Wrap::Repeat, ..Default::default() },
    });
    s.materials.push(Material {
        name: Some("box_mat".into()),
        base_color_texture: Some(TextureRef { texture: 0, uv_set: 0, transform: None }),
        metallic: 0.0,
        roughness: 0.6,
        ..Default::default()
    });
    s.materials.push(Material {
        name: Some("wedge_mat".into()),
        base_color: [0.9, 0.3, 0.1, 1.0],
        metallic: 0.0,
        roughness: 0.8,
        ..Default::default()
    });
    s.materials.push(Material {
        name: Some("panel_mat".into()),
        metallic: 0.0,
        roughness: 1.0,
        double_sided: true,
        ..Default::default()
    });
    s.meshes.push(SceneMesh { name: Some("box".into()), primitives: vec![box_primitive(Some(0))], ..Default::default() });
    s.meshes.push(SceneMesh { name: Some("wedge".into()), primitives: vec![wedge_primitive(Some(1))], ..Default::default() });
    s.meshes.push(SceneMesh {
        name: Some("panel".into()),
        primitives: vec![grid_primitive(4, 1.6, Some(2), true)],
        ..Default::default()
    });

    // Nodes: root, then its four children.
    s.nodes.push(Node {
        name: Some("assembly".into()),
        transform: trs([0.0, 0.1, 0.0], axis_angle([0.0, 1.0, 0.0], 20.0), [1.0; 3]),
        children: vec![1, 2, 3, 4],
        ..Default::default()
    });
    s.nodes.push(Node {
        name: Some("box_node".into()),
        transform: trs([-1.2, 0.0, 0.0], axis_angle([1.0, 0.0, 0.0], 30.0), [1.0; 3]),
        mesh: Some(0),
        ..Default::default()
    });
    s.nodes.push(Node {
        name: Some("wedge_node".into()),
        transform: trs([1.2, 0.0, 0.0], axis_angle([0.0, 0.0, 1.0], -15.0), [1.0, 1.5, 0.75]),
        mesh: Some(1),
        ..Default::default()
    });
    s.nodes.push(Node {
        name: Some("panel_node".into()),
        transform: trs([0.0, 1.25, -0.4], math::QUAT_IDENTITY, [1.0; 3]),
        mesh: Some(2),
        ..Default::default()
    });
    s.nodes.push(Node {
        name: Some("mirrored_box".into()),
        transform: trs([0.0, -1.3, 0.2], axis_angle([0.0, 1.0, 0.0], 35.0), [-1.0, 0.8, 1.0]),
        mesh: Some(0),
        ..Default::default()
    });
    s.roots = vec![0];
    s
}

/// A banded tube on a three-joint chain, bent by LINEAR, STEP and CUBICSPLINE
/// channels.
pub fn skinned() -> Scene {
    let mut s = Scene { name: Some("skinned".into()), ..Default::default() };
    s.images.push(Image { name: Some("bands".into()), format: ImageFormat::Png, data: bands_png() });
    s.textures.push(Texture { name: Some("bands".into()), image: 0, sampler: Sampler::default() });
    s.materials.push(Material {
        name: Some("tube_mat".into()),
        base_color_texture: Some(TextureRef { texture: 0, uv_set: 0, transform: None }),
        metallic: 0.0,
        roughness: 0.7,
        ..Default::default()
    });

    // Tube: 16 sides, 24 rings over a height of 3, open ends.
    let (sides, rings, height, radius) = (16usize, 24usize, 3.0f32, 0.3f32);
    let mut p = Primitive { material: Some(0), ..Default::default() };
    let (mut normals, mut uvs, mut joints, mut weights) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for r in 0..=rings {
        let y = height * r as f32 / rings as f32;
        for k in 0..=sides {
            let a = std::f32::consts::TAU * k as f32 / sides as f32;
            let (sn, cs) = a.sin_cos();
            p.positions.push([radius * cs, y, radius * sn]);
            normals.push([cs, 0.0, sn]);
            uvs.push([k as f32 / sides as f32, 1.0 - y / height]);
            // Smooth two-bone blend around each joint at y = 1 and y = 2.
            let (j0, j1, w1) = if y <= 1.0 {
                (0u16, 1u16, ((y - 0.6) / 0.8).clamp(0.0, 1.0))
            } else {
                (1u16, 2u16, ((y - 1.6) / 0.8).clamp(0.0, 1.0))
            };
            joints.extend_from_slice(&[j0, j1, 0, 0]);
            weights.extend_from_slice(&[1.0 - w1, w1, 0.0, 0.0]);
        }
    }
    let row = (sides + 1) as u32;
    for r in 0..rings as u32 {
        for k in 0..sides as u32 {
            let a = r * row + k;
            // Counter-clockwise seen from outside the tube.
            p.indices.extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    p.normals = Some(normals);
    p.uvs = vec![uvs];
    p.skin = Some(SkinWeights { influences: 4, joints, weights });
    s.meshes.push(SceneMesh { name: Some("tube".into()), primitives: vec![p], ..Default::default() });

    // Joints: hip at the origin, knee at y=1, ankle at y=2 (local offsets 1).
    s.nodes.push(Node { name: Some("hip".into()), transform: trs([0.0; 3], math::QUAT_IDENTITY, [1.0; 3]), children: vec![1], ..Default::default() });
    s.nodes.push(Node { name: Some("knee".into()), transform: trs([0.0, 1.0, 0.0], math::QUAT_IDENTITY, [1.0; 3]), children: vec![2], ..Default::default() });
    s.nodes.push(Node { name: Some("ankle".into()), transform: trs([0.0, 1.0, 0.0], math::QUAT_IDENTITY, [1.0; 3]), ..Default::default() });
    s.nodes.push(Node { name: Some("tube_node".into()), mesh: Some(0), skin: Some(0), ..Default::default() });
    s.roots = vec![0, 3];
    s.skins.push(Skin {
        name: Some("leg".into()),
        joints: vec![0, 1, 2],
        inverse_bind_matrices: vec![
            math::IDENTITY,
            math::translation([0.0, -1.0, 0.0]),
            math::translation([0.0, -2.0, 0.0]),
        ],
        skeleton: Some(0),
    });

    let q = |axis: [f64; 3], deg: f64| -> Vec<f32> { axis_angle(axis, deg).iter().map(|v| *v as f32).collect() };
    let mut knee = Vec::new();
    for deg in [0.0, 55.0, -20.0, 0.0] {
        knee.extend(q([0.0, 0.0, 1.0], deg));
    }
    let mut ankle = Vec::new();
    for deg in [0.0, 40.0, 70.0, 0.0] {
        ankle.extend(q([1.0, 0.0, 0.0], deg));
    }
    // Cubic hip scale: [in-tangent, value, out-tangent] per key.
    let mut hip_scale = Vec::new();
    for (value, tangent) in [(1.0f32, 0.0f32), (1.25, 0.4), (1.0, 0.0)] {
        hip_scale.extend_from_slice(&[tangent; 3]);
        hip_scale.extend_from_slice(&[value; 3]);
        hip_scale.extend_from_slice(&[tangent; 3]);
    }
    s.animations.push(Animation {
        name: Some("Bend".into()),
        channels: vec![
            Channel { node: 1, path: AnimPath::Rotation, interpolation: Interpolation::Linear, times: vec![0.0, 0.7, 1.4, 2.0], values: knee },
            Channel { node: 2, path: AnimPath::Rotation, interpolation: Interpolation::Linear, times: vec![0.0, 0.5, 1.1, 2.0], values: ankle },
            Channel {
                node: 0,
                path: AnimPath::Translation,
                interpolation: Interpolation::Step,
                times: vec![0.0, 1.0],
                values: vec![0.0, 0.0, 0.0, 0.4, 0.0, 0.0],
            },
            Channel { node: 0, path: AnimPath::Scale, interpolation: Interpolation::CubicSpline, times: vec![0.0, 1.0, 2.0], values: hip_scale },
        ],
    });
    s
}

/// A grid with two morph targets — one moving positions only, one with normal
/// offsets too — and an animation of their weights.
pub fn morph() -> Scene {
    let mut s = Scene { name: Some("morph".into()), ..Default::default() };
    s.materials.push(Material {
        name: Some("morph_mat".into()),
        base_color: [0.2, 0.55, 0.85, 1.0],
        metallic: 0.0,
        roughness: 0.5,
        double_sided: true,
        ..Default::default()
    });
    let mut p = grid_primitive(10, 2.0, Some(0), false);
    let n = p.positions.len();
    let mut bulge = Vec::with_capacity(n);
    let mut bulge_n = Vec::with_capacity(n);
    let mut twist = Vec::with_capacity(n);
    for v in &p.positions {
        let r2 = v[0] * v[0] + v[1] * v[1];
        let h = 0.8 * (-r2 * 2.0).exp();
        bulge.push([0.0, 0.0, h]);
        // Offset that tilts the normal outward from the bump.
        bulge_n.push([v[0] * h * 0.8, v[1] * h * 0.8, 0.0]);
        twist.push([-v[1] * 0.35 * v[0], 0.0, 0.0]);
    }
    p.targets.push(MorphTarget { positions: bulge, normals: Some(bulge_n) });
    p.targets.push(MorphTarget { positions: twist, normals: Some(vec![[0.0; 3]; n]) });
    s.meshes.push(SceneMesh {
        name: Some("sheet".into()),
        primitives: vec![p],
        weights: vec![0.25, 0.0],
        target_names: vec!["bulge".into(), "twist".into()],
    });
    s.nodes.push(Node {
        name: Some("sheet_node".into()),
        transform: trs([0.0, 0.0, 0.0], axis_angle([1.0, 0.0, 0.0], -35.0), [1.0; 3]),
        mesh: Some(0),
        ..Default::default()
    });
    s.roots = vec![0];
    s.animations.push(Animation {
        name: Some("Breathe".into()),
        channels: vec![Channel {
            node: 0,
            path: AnimPath::Weights,
            interpolation: Interpolation::Linear,
            times: vec![0.0, 0.8, 2.0],
            values: vec![0.0, 0.0, 1.0, 0.6, 0.2, 1.0],
        }],
    });
    s
}

/// The original matrix fixture: a cube with distinct, non-symmetric bounds,
/// per-vertex normals and UVs and a flat colour, as one flat mesh.
pub fn cube() -> Scene {
    #[rustfmt::skip]
    let corners: [[f32; 3]; 8] = [
        [-1.0, -2.0, -3.0], [4.0, -2.0, -3.0], [4.0, 5.0, -3.0], [-1.0, 5.0, -3.0],
        [-1.0, -2.0,  6.0], [4.0, -2.0,  6.0], [4.0, 5.0,  6.0], [-1.0, 5.0,  6.0],
    ];
    #[rustfmt::skip]
    let indices: Vec<u32> = vec![
        0, 2, 1,  0, 3, 2,
        4, 5, 6,  4, 6, 7,
        0, 5, 4,  0, 1, 5,
        3, 6, 2,  3, 7, 6,
        0, 7, 3,  0, 4, 7,
        1, 6, 5,  1, 2, 6,
    ];
    let centre = [1.5f32, 1.5, 1.5];
    let normals: Vec<[f32; 3]> = corners
        .iter()
        .map(|c| {
            let v = [c[0] - centre[0], c[1] - centre[1], c[2] - centre[2]];
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / l, v[1] / l, v[2] / l]
        })
        .collect();
    let uvs: Vec<[f32; 2]> = (0..8).map(|i| [i as f32 / 8.0, 1.0 - i as f32 / 8.0]).collect();
    super::pose::from_flat(&super::ir::Mesh {
        positions: corners.to_vec(),
        indices,
        normals: Some(normals),
        uvs: Some(uvs),
        base_color: Some([0.25, 0.5, 0.75, 1.0]),
        name: Some("litegen_matrix_cube".to_string()),
    })
}

/// Times at which to compare an animated fixture: deliberately NOT key times,
/// so that interpolation is what gets compared, plus both ends.
pub fn sample_times(scene: &Scene) -> Vec<f32> {
    match scene.animations.first().and_then(|a| a.time_range()) {
        None => Vec::new(),
        Some((lo, hi)) => {
            let span = hi - lo;
            [0.0, 0.17, 0.36, 0.58, 0.83, 1.0].iter().map(|f| lo + span * f).collect()
        }
    }
}

/// World-space bounds of a scene over its default pose and every sample time
/// of its first animation: what a fixed camera has to frame.
pub fn bounds_over_animation(scene: &Scene) -> Option<([f32; 3], [f32; 3])> {
    let mut poses = vec![super::pose::default_pose(scene)];
    if !scene.animations.is_empty() {
        if let Some((lo, hi)) = scene.animations[0].time_range() {
            for i in 0..=40 {
                poses.push(super::pose::sample_pose(scene, 0, lo + (hi - lo) * i as f32 / 40.0));
            }
        }
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for pose in &poses {
        for wp in super::pose::world_primitives(scene, pose).ok()? {
            for p in &wp.positions {
                for a in 0..3 {
                    lo[a] = lo[a].min(p[a]);
                    hi[a] = hi[a].max(p[a]);
                }
            }
        }
    }
    (lo[0] <= hi[0]).then_some((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixture_is_a_valid_scene() {
        for (name, s) in all() {
            s.validate().unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(s.triangle_count() > 0, "{name}");
            assert!(bounds_over_animation(&s).is_some(), "{name}");
        }
    }

    #[test]
    fn fixtures_are_deterministic() {
        assert_eq!(all(), all());
    }

    #[test]
    fn the_png_encoder_writes_a_decodable_file() {
        let png = quadrant_png();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(ImageFormat::sniff(&png), Some(ImageFormat::Png));
        // IHDR width/height big-endian at bytes 16..24.
        assert_eq!(&png[16..24], &[0, 0, 0, 64, 0, 0, 0, 64]);
    }

    #[test]
    fn skinned_fixture_actually_moves_between_keys() {
        let s = skinned();
        let a = super::super::pose::world_primitives(&s, &super::super::pose::sample_pose(&s, 0, 0.0)).unwrap();
        let b = super::super::pose::world_primitives(&s, &super::super::pose::sample_pose(&s, 0, 0.9)).unwrap();
        let top_a = a[0].positions.last().unwrap();
        let top_b = b[0].positions.last().unwrap();
        let d = (0..3).map(|i| (top_a[i] - top_b[i]).abs()).fold(0.0, f32::max);
        assert!(d > 0.3, "the bend should move the tip by a visible amount, moved {d}");
    }
}
