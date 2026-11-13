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
//! - [`multiuv`]: a quad with four uv sets, each a different affine map of its
//!   corners, whose base colour samples set 3 and whose occlusion samples set 1
//!   (two images). A swapped, shifted or dropped set, or a swapped binding, is
//!   a different picture. [`multiuv_shared`] is two such quads sharing the one
//!   material.
//! - [`layered`]: one box per group of glTF PBR layers (clearcoat;
//!   transmission, volume, ior and dispersion; sheen; specular and ior;
//!   iridescence; anisotropy), several of them textured, so a container that
//!   drops or misreads one layer shows it on that box alone.
//! - [`staged`]: a static ground and cube faced by a directional, a point and
//!   a spot light and by a perspective and an orthographic camera, all on
//!   nodes under a parent a clip turns, so a light or camera on the wrong
//!   axis, or one its parent's motion does not reach, is a different picture
//!   through the file's own camera and lights.
//!
//! - [`wire`]: a unit cube, a square line loop with a colour at each corner
//!   under a node a clip moves, and a 3 x 3 grid of coloured points, so that
//!   a container which drops points or lines, or reads one as the other, or
//!   as triangles, or moves the loop with the wrong parent, is a different
//!   picture and a different count.
//!
//! - [`clips`]: the skinned leg of [`skinned`] and a prop on a plain node,
//!   with three named clips of different lengths ("walk" 1 s, "run" 0.6 s,
//!   "wave" 2 s), two of which also move the prop, so a container that keeps
//!   one clip, a clip's skeleton but not its rigid motion, or the clips in
//!   the wrong order or at the wrong length is a different picture and a
//!   different list.
//!
//! - [`unlit`]: [`textured`] with its textured box (and the box's mirrored
//!   instance) unlit, beside the flat wedge and the vertex-coloured panel,
//!   which stay lit as the control, so a container that draws the unlit box
//!   lit, or loses its texture, is a different picture, and a render under a
//!   second light changes the control and not the box.
//!
//! Everything here is plain arithmetic: no randomness, no clock, no
//! environment. Two runs produce byte-identical files.
//!
//! Ruling IDs ("Ruling A40a") are mesh-gaps item 4's rulings, listed with
//! their costs in the status block of
//! `docs/superpowers/plans/2026-11-04-mesh-material-layers.md`.

use super::math;
use super::scene::{
    AnimPath, Animation, Anisotropy, Channel, Clearcoat, Image, ImageFormat, Interpolation, Iridescence, Material,
    MorphTarget, Node, Primitive, Sampler, Scene, SceneMesh, Sheen, Skin, SkinWeights, Specular, Texture, TextureRef,
    Transform, Transmission, Volume, Wrap,
};
use super::scene::{Camera, Light, LightKind, Projection};
use super::scene::Topology;

/// Every fixture by name, in a stable order.
pub fn all() -> Vec<(&'static str, Scene)> {
    vec![
        ("textured", textured()),
        ("skinned", skinned()),
        ("morph", morph()),
        ("cube", cube()),
        ("metal", metal()),
        ("multiuv", multiuv()),
        ("multiuv_shared", multiuv_shared()),
        ("layered", layered()),
        ("staged", staged()),
        ("wire", wire()),
        ("clips", clips()),
        ("unlit", unlit()),
    ]
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

/// Per-vertex tangents from a primitive's positions, normals and uv set 0, as
/// glTF's `TANGENT`: xyz is the direction in which u grows, made orthogonal
/// to the vertex normal, and w is the handedness (glTF 2.0 §3.7.2.1:
/// "bitangent = cross(normal, tangent.xyz) * tangent.w"). glTF stores v
/// growing DOWN the image (top-left origin) while its normal textures are +Y
/// up, so the bitangent points the way v DECREASES: w is +1 where
/// `cross(normal, tangent)` points against the stored v's growth and -1 where
/// it points along it. three.js says the same of MikkTSpace's sign, which it
/// negates "for normal map conventions in some formats, including glTF"
/// (scripts/mesh-verify/node_modules/three/examples/jsm/utils/BufferGeometryUtils.js:34-35),
/// and GLTFLoader flips `normalScale.y` when it has no TANGENT to go by
/// (GLTFLoader.js:3501-3505). Each triangle's dP/du and dP/dv are solved
/// from its edges and uv deltas and summed into its vertices (Lengyel's
/// method, the one MikkTSpace refines), so a vertex shared by faces averages
/// them. Pinned by `car_paint_tangents_follow_gltf`.
fn uv_tangents(p: &Primitive) -> Vec<[f32; 4]> {
    let n = p.positions.len();
    let normals = p.normals.as_ref().expect("tangents need normals");
    let uvs = &p.uvs[0];
    let mut tan = vec![[0.0f32; 3]; n];
    let mut bit = vec![[0.0f32; 3]; n];
    for tri in p.indices.chunks_exact(3) {
        let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let (p0, p1, p2) = (p.positions[a], p.positions[b], p.positions[c]);
        let (t0, t1, t2) = (uvs[a], uvs[b], uvs[c]);
        let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
        let (du1, dv1) = (t1[0] - t0[0], t1[1] - t0[1]);
        let (du2, dv2) = (t2[0] - t0[0], t2[1] - t0[1]);
        let det = du1 * dv2 - du2 * dv1;
        if det.abs() < 1e-12 {
            continue;
        }
        let r = 1.0 / det;
        for k in 0..3 {
            let du = (e1[k] * dv2 - e2[k] * dv1) * r;
            let dv = (e2[k] * du1 - e1[k] * du2) * r;
            for v in [a, b, c] {
                tan[v][k] += du;
                bit[v][k] += dv;
            }
        }
    }
    (0..n)
        .map(|v| {
            let nrm = normals[v];
            let t = tan[v];
            // Gram-Schmidt against the normal, then normalise.
            let d = nrm[0] * t[0] + nrm[1] * t[1] + nrm[2] * t[2];
            let o = [t[0] - nrm[0] * d, t[1] - nrm[1] * d, t[2] - nrm[2] * d];
            let len = (o[0] * o[0] + o[1] * o[1] + o[2] * o[2]).sqrt().max(1e-20);
            let t = [o[0] / len, o[1] / len, o[2] / len];
            let cross = [nrm[1] * t[2] - nrm[2] * t[1], nrm[2] * t[0] - nrm[0] * t[2], nrm[0] * t[1] - nrm[1] * t[0]];
            // `bit` is dP/dv for the stored v; glTF's bitangent runs the
            // other way.
            let bt = bit[v];
            let w = if cross[0] * bt[0] + cross[1] * bt[1] + cross[2] * bt[2] < 0.0 { 1.0 } else { -1.0 };
            [t[0], t[1], t[2], w]
        })
        .collect()
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
pub(super) fn grid_primitive(n: usize, size: f32, material: Option<usize>, colored: bool) -> Primitive {
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

/// Static, metallic: a gold metal, a brushed one, and a part whose metal and
/// roughness come from a map (roughness in G, metalness in B; glTF 2.0
/// §3.9.2) beside a base-colour map. Every other fixture is a dielectric,
/// which is how a container that keeps, folds or drops metal went untested
/// until 2026-11 (2026-10-12 review #22, #31).
pub fn metal() -> Scene {
    let mut s = Scene { name: Some("metal".into()), ..Default::default() };
    s.images.push(Image { name: Some("quadrants".into()), format: ImageFormat::Png, data: quadrant_png() });
    s.textures.push(Texture { name: Some("quadrants".into()), image: 0, sampler: Sampler::default() });
    s.materials.push(Material {
        name: Some("gold".into()),
        base_color: [1.0, 0.766, 0.336, 1.0],
        metallic: 1.0,
        roughness: 0.3,
        ..Default::default()
    });
    s.materials.push(Material {
        name: Some("brushed".into()),
        base_color: [0.6, 0.6, 0.65, 1.0],
        metallic: 0.8,
        roughness: 0.55,
        ..Default::default()
    });
    s.materials.push(Material {
        name: Some("mapped".into()),
        base_color_texture: Some(TextureRef { texture: 0, uv_set: 0, transform: None }),
        metallic_roughness_texture: Some(TextureRef { texture: 0, uv_set: 0, transform: None }),
        metallic: 1.0,
        roughness: 1.0,
        ..Default::default()
    });
    s.meshes.push(SceneMesh { name: Some("ingot".into()), primitives: vec![box_primitive(Some(0))], ..Default::default() });
    s.meshes.push(SceneMesh { name: Some("blade".into()), primitives: vec![wedge_primitive(Some(1))], ..Default::default() });
    s.meshes.push(SceneMesh { name: Some("plate".into()), primitives: vec![grid_primitive(4, 1.6, Some(2), false)], ..Default::default() });
    s.nodes.push(Node { name: Some("metals".into()), children: vec![1, 2, 3], ..Default::default() });
    s.nodes.push(Node {
        name: Some("ingot_node".into()),
        transform: trs([-1.2, 0.0, 0.0], axis_angle([0.0, 1.0, 0.0], 25.0), [1.0; 3]),
        mesh: Some(0),
        ..Default::default()
    });
    s.nodes.push(Node {
        name: Some("blade_node".into()),
        transform: trs([1.2, 0.0, 0.0], axis_angle([1.0, 0.0, 0.0], -20.0), [1.0; 3]),
        mesh: Some(1),
        ..Default::default()
    });
    s.nodes.push(Node {
        name: Some("plate_node".into()),
        transform: trs([0.0, 1.25, -0.4], math::QUAT_IDENTITY, [1.0; 3]),
        mesh: Some(2),
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

/// The skinned leg of [`skinned`] and a prop, with three named clips of
/// different lengths: "walk" (1 s), "run" (0.6 s) and "wave" (2 s). Each clip
/// moves the leg's joints with keys of its own, and ends at its own length.
/// "walk" and "run" also move the prop, a box on a plain node (rigid motion,
/// which USD can switch between clips only for the first one); "wave" moves
/// nothing but the skeleton, and scales its hip by 1.3, a value a half float
/// holds only approximately, so a clip after the first shows whether its
/// scales are rounded the way the file keeps them. Every channel is LINEAR,
/// every key time is on the clip's own grid and every clip's last key is the
/// clip's end.
pub fn clips() -> Scene {
    let mut s = skinned();
    s.name = Some("clips".into());
    s.animations.clear();
    // The prop is first in every list, node 0, mesh 0 and material 0, a root
    // beside the leg's. A USD file writes a plain node ahead of a skeleton and
    // reads the nodes back in that order, and Collada and 3MF number meshes
    // and materials by the node that first uses them, so the prop leads in all
    // of them and each comes back list for list.
    s.materials.insert(
        0,
        Material {
            name: Some("prop_mat".into()),
            base_color: [0.9, 0.35, 0.2, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            ..Default::default()
        },
    );
    for m in &mut s.meshes {
        for p in &mut m.primitives {
            if let Some(k) = &mut p.material {
                *k += 1;
            }
        }
    }
    s.meshes.insert(0, SceneMesh { name: Some("prop".into()), primitives: vec![box_primitive(Some(0))], ..Default::default() });
    for n in &mut s.nodes {
        for c in &mut n.children {
            *c += 1;
        }
        if let Some(m) = &mut n.mesh {
            *m += 1;
        }
    }
    for r in &mut s.roots {
        *r += 1;
    }
    for skin in &mut s.skins {
        for j in &mut skin.joints {
            *j += 1;
        }
        if let Some(k) = &mut skin.skeleton {
            *k += 1;
        }
    }
    s.nodes.insert(
        0,
        Node {
            name: Some("prop_node".into()),
            transform: trs([1.2, 0.5, 0.0], math::QUAT_IDENTITY, [0.4; 3]),
            mesh: Some(0),
            ..Default::default()
        },
    );
    s.roots.insert(0, 0);

    let turn = |node: usize, axis: [f64; 3], keys: &[(f32, f64)]| Channel {
        node,
        path: AnimPath::Rotation,
        interpolation: Interpolation::Linear,
        times: keys.iter().map(|k| k.0).collect(),
        values: keys.iter().flat_map(|k| axis_angle(axis, k.1).map(|v| v as f32)).collect(),
    };
    let slide = |node: usize, path: AnimPath, keys: &[(f32, [f32; 3])]| Channel {
        node,
        path,
        interpolation: Interpolation::Linear,
        times: keys.iter().map(|k| k.0).collect(),
        values: keys.iter().flat_map(|k| k.1).collect(),
    };
    let (prop, hip, knee, ankle) = (0, 1, 2, 3);
    s.animations.push(Animation {
        name: Some("walk".into()),
        channels: vec![
            turn(knee, [0.0, 0.0, 1.0], &[(0.0, 0.0), (0.5, 45.0), (1.0, 0.0)]),
            turn(ankle, [1.0, 0.0, 0.0], &[(0.0, 0.0), (0.3, 30.0), (1.0, 0.0)]),
            slide(prop, AnimPath::Translation, &[(0.0, [1.2, 0.5, 0.0]), (1.0, [1.2, 1.3, 0.0])]),
        ],
    });
    s.animations.push(Animation {
        name: Some("run".into()),
        channels: vec![
            turn(knee, [0.0, 0.0, 1.0], &[(0.0, 0.0), (0.3, 70.0), (0.6, 0.0)]),
            turn(ankle, [1.0, 0.0, 0.0], &[(0.0, 0.0), (0.6, 50.0)]),
            turn(prop, [0.0, 1.0, 0.0], &[(0.0, 0.0), (0.6, 90.0)]),
        ],
    });
    s.animations.push(Animation {
        name: Some("wave".into()),
        channels: vec![
            turn(hip, [0.0, 0.0, 1.0], &[(0.0, 0.0), (0.5, 20.0), (1.0, 0.0), (1.5, -20.0), (2.0, 0.0)]),
            turn(ankle, [1.0, 0.0, 0.0], &[(0.0, 0.0), (2.0, 60.0)]),
            slide(hip, AnimPath::Scale, &[(0.0, [1.0; 3]), (2.0, [1.3; 3])]),
        ],
    });
    s
}

/// [`textured`] with its box unlit (`KHR_materials_unlit`): the textured box,
/// and its mirrored instance, show their base colour texture whatever the
/// light, beside the flat wedge and the vertex-coloured panel, which stay lit
/// and are the control a render with a different light must change. The box's
/// metallic 0 and roughness of 0.6 are what a lit box would have, which an
/// unlit material has no use for.
pub fn unlit() -> Scene {
    let mut s = textured();
    s.name = Some("unlit".into());
    s.materials[0].name = Some("box_unlit".into());
    s.materials[0].unlit = true;
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

/// A unit quad facing +Z, centred on `centre`, with four uv sets. Each set
/// is a different affine map of the corners' uv `(u, v)` (V down), and no two
/// sets share a value, so a set read as another, or shifted by a vertex, does
/// not compare equal:
///
/// - set 0: `(u, v)`, the identity;
/// - set 1: `(0.25 + 0.5u, 0.125 + 0.5v)`, a half-size window off centre;
/// - set 2: `(0.125 + 0.75v, 0.875 − 0.75u)`, turned a quarter and scaled;
/// - set 3: `(0.9375 − 0.875u, 0.0625 + 0.875v)`, mirrored in u and scaled.
///
/// Every value is a multiple of 1/16, so `1 − v` (USD's flip) is exact.
fn multiuv_quad(material: Option<usize>, centre: [f32; 3]) -> Primitive {
    let corners: [([f32; 2], [f32; 2]); 4] =
        [([-0.5, -0.5], [0.0, 1.0]), ([0.5, -0.5], [1.0, 1.0]), ([0.5, 0.5], [1.0, 0.0]), ([-0.5, 0.5], [0.0, 0.0])];
    let maps: [fn([f32; 2]) -> [f32; 2]; 4] = [
        |[u, v]| [u, v],
        |[u, v]| [0.25 + 0.5 * u, 0.125 + 0.5 * v],
        |[u, v]| [0.125 + 0.75 * v, 0.875 - 0.75 * u],
        |[u, v]| [0.9375 - 0.875 * u, 0.0625 + 0.875 * v],
    ];
    let mut p = Primitive { material, ..Default::default() };
    p.positions = corners.iter().map(|(xy, _)| [centre[0] + xy[0], centre[1] + xy[1], centre[2]]).collect();
    p.normals = Some(vec![[0.0, 0.0, 1.0]; 4]);
    p.uvs = maps.iter().map(|f| corners.iter().map(|(_, uv)| f(*uv)).collect()).collect();
    p.indices = vec![0, 1, 2, 0, 2, 3];
    p
}

/// The images, textures and material [`multiuv`] and [`multiuv_shared`]
/// share: the quadrants as base colour on set 3, the bands as occlusion on
/// set 1. Sets 0 and 2 are sampled by nothing.
fn multiuv_scene(name: &str) -> Scene {
    let mut s = Scene { name: Some(name.into()), ..Default::default() };
    s.images.push(Image { name: Some("quadrants".into()), format: ImageFormat::Png, data: quadrant_png() });
    s.images.push(Image { name: Some("bands".into()), format: ImageFormat::Png, data: bands_png() });
    s.textures.push(Texture { name: Some("quadrants".into()), image: 0, sampler: Sampler::default() });
    s.textures.push(Texture { name: Some("bands".into()), image: 1, sampler: Sampler::default() });
    s.materials.push(Material {
        name: Some("multiuv_mat".into()),
        base_color_texture: Some(TextureRef { texture: 0, uv_set: 3, transform: None }),
        occlusion_texture: Some(TextureRef { texture: 1, uv_set: 1, transform: None }),
        metallic: 0.0,
        roughness: 0.5,
        ..Default::default()
    });
    s
}

/// Static: one quad with four uv sets (see [`multiuv_quad`]), its base colour
/// on set 3 and its occlusion on set 1.
pub fn multiuv() -> Scene {
    let mut s = multiuv_scene("multiuv");
    s.meshes.push(SceneMesh { name: Some("quad".into()), primitives: vec![multiuv_quad(Some(0), [0.0; 3])], ..Default::default() });
    s.nodes.push(Node { name: Some("quad_node".into()), mesh: Some(0), ..Default::default() });
    s.roots = vec![0];
    s
}

/// Static: two primitives share [`multiuv`]'s material, one quad in each of two
/// meshes side by side, so a remap that differs between primitives of one
/// material shows.
pub fn multiuv_shared() -> Scene {
    let mut s = multiuv_scene("multiuv_shared");
    s.meshes.push(SceneMesh { name: Some("left".into()), primitives: vec![multiuv_quad(Some(0), [-0.6, 0.0, 0.0])], ..Default::default() });
    s.meshes.push(SceneMesh { name: Some("right".into()), primitives: vec![multiuv_quad(Some(0), [0.6, 0.0, 0.0])], ..Default::default() });
    s.nodes.push(Node { name: Some("pair".into()), children: vec![1, 2], ..Default::default() });
    s.nodes.push(Node { name: Some("left_quad".into()), mesh: Some(0), ..Default::default() });
    s.nodes.push(Node { name: Some("right_quad".into()), mesh: Some(1), ..Default::default() });
    s.roots = vec![0];
    s
}

/// Static: seven boxes in two rows, each with its own material over glTF's
/// PBR layer extensions, every layer on at least one box, its maps on uv set
/// 0:
///
/// - `car_paint`: clearcoat, with its factor, roughness and normal maps. Its
///   primitive carries tangents: a clearcoat normal map needs a tangent
///   space, and with no base normal map to derive one from, a glTF primitive
///   must provide `TANGENT` (glTF-Validator `MESH_PRIMITIVE_NO_TANGENT_SPACE`,
///   an error; scripts/mesh-verify/node_modules/gltf-validator/ISSUES.md:114);
/// - `glass`: transmission over a volume with a thickness map, attenuation,
///   ior and dispersion;
/// - `velvet`: sheen with a colour map;
/// - `plastic`: specular with a colour map, and ior;
/// - `soap_film`: iridescence with a thickness map;
/// - `brushed_steel`: anisotropy, untextured;
/// - `lacquer`: a black base under an untextured clearcoat (factor and
///   roughness) with an ior of 1.45: the one material three.js's USDLoader
///   can show lit, since it reads `inputs:clearcoat`, `inputs:clearcoatRoughness`
///   and `inputs:ior` as values alone and decodes a base colour as sRGB, which
///   agrees with linear only at 0 and 1. Black, so the coat is all there is
///   to see.
///
/// The maps reuse the quadrant and band images; no base colour is textured,
/// so a container that drops every layer keeps no image.
pub fn layered() -> Scene {
    let mut s = Scene { name: Some("layered".into()), ..Default::default() };
    s.images.push(Image { name: Some("quadrants".into()), format: ImageFormat::Png, data: quadrant_png() });
    s.images.push(Image { name: Some("bands".into()), format: ImageFormat::Png, data: bands_png() });
    s.textures.push(Texture { name: Some("quadrants".into()), image: 0, sampler: Sampler::default() });
    s.textures.push(Texture { name: Some("bands".into()), image: 1, sampler: Sampler::default() });
    let map = |texture: usize| Some(TextureRef { texture, uv_set: 0, transform: None });
    let materials = [
        Material {
            name: Some("car_paint".into()),
            base_color: [0.55, 0.04, 0.05, 1.0],
            metallic: 0.3,
            roughness: 0.5,
            clearcoat: Some(Clearcoat {
                factor: 1.0,
                texture: map(0),
                roughness: 0.1,
                roughness_texture: map(1),
                normal_texture: map(0),
                normal_scale: 0.5,
            }),
            ..Default::default()
        },
        Material {
            name: Some("glass".into()),
            base_color: [0.95, 0.97, 1.0, 1.0],
            metallic: 0.0,
            roughness: 0.05,
            transmission: Some(Transmission { factor: 1.0, texture: None }),
            volume: Some(Volume { thickness: 0.4, thickness_texture: map(1), attenuation_distance: 1.5, attenuation_color: [0.9, 0.55, 0.2] }),
            ior: Some(1.52),
            dispersion: Some(0.3),
            ..Default::default()
        },
        Material {
            name: Some("velvet".into()),
            base_color: [0.3, 0.05, 0.4, 1.0],
            metallic: 0.0,
            roughness: 0.9,
            sheen: Some(Sheen { color: [0.9, 0.6, 0.8], color_texture: map(0), roughness: 0.4, roughness_texture: None }),
            ..Default::default()
        },
        Material {
            name: Some("plastic".into()),
            base_color: [0.1, 0.4, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.3,
            specular: Some(Specular { factor: 0.6, texture: None, color: [1.0, 0.85, 0.7], color_texture: map(1) }),
            ior: Some(1.6),
            ..Default::default()
        },
        Material {
            name: Some("soap_film".into()),
            base_color: [0.05, 0.05, 0.06, 1.0],
            metallic: 0.0,
            roughness: 0.2,
            iridescence: Some(Iridescence {
                factor: 1.0,
                texture: None,
                ior: 1.33,
                thickness_min: 200.0,
                thickness_max: 500.0,
                thickness_texture: map(0),
            }),
            ..Default::default()
        },
        Material {
            name: Some("brushed_steel".into()),
            base_color: [0.8, 0.8, 0.85, 1.0],
            metallic: 1.0,
            roughness: 0.35,
            anisotropy: Some(Anisotropy { strength: 0.7, rotation: 0.6, texture: None }),
            ..Default::default()
        },
        Material {
            name: Some("lacquer".into()),
            // Black, not white: a coat over white moves a pixel by under
            // 8/255 (measured, three.js: the coat dims the base by its
            // Fresnel and adds about as much back), where over black the coat
            // is the whole picture.
            base_color: [0.0, 0.0, 0.0, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            clearcoat: Some(Clearcoat { factor: 1.0, texture: None, roughness: 0.2, roughness_texture: None, normal_texture: None, normal_scale: 1.0 }),
            ior: Some(1.45),
            ..Default::default()
        },
    ];
    s.nodes.push(Node { name: Some("layers".into()), ..Default::default() });
    for (i, m) in materials.into_iter().enumerate() {
        let name = m.name.clone().unwrap_or_default();
        s.materials.push(m);
        let mut primitive = box_primitive(Some(i));
        if name == "car_paint" {
            primitive.tangents = Some(uv_tangents(&primitive));
        }
        s.meshes.push(SceneMesh { name: Some(name.clone()), primitives: vec![primitive], ..Default::default() });
        let (x, y) = ((i % 4) as f64 * 1.6 - 2.4, if i < 4 { 0.8 } else { -0.8 });
        s.nodes.push(Node {
            name: Some(format!("{name}_node")),
            transform: trs([x, y, 0.0], axis_angle([0.0, 1.0, 0.0], 30.0), [1.0; 3]),
            mesh: Some(i),
            ..Default::default()
        });
        s.nodes[0].children.push(i + 1);
    }
    s.roots = vec![0];
    s
}

/// The rotation that turns −Z, the way lights shine and cameras look, from
/// `eye` towards `target` with +Y kept up: a tilt about X, then a turn about
/// +Y.
fn aim(eye: [f64; 3], target: [f64; 3]) -> [f64; 4] {
    let d = [target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]];
    let turn = math::quat_from_axis_angle([0.0, 1.0, 0.0], (-d[0]).atan2(-d[2]));
    let tilt = math::quat_from_axis_angle([1.0, 0.0, 0.0], d[1].atan2(d[0].hypot(d[2])));
    math::quat_mul(turn, tilt)
}

/// Lights and cameras on the move around static geometry: a 4 m ground quad
/// and a cube on it at the root, and, on nodes under `rig`, which a 2 s clip
/// turns a quarter about the cube's vertical axis, a directional light, a
/// point light with a range, a spot (cones of 0.2 and 0.6 rad), a
/// perspective camera (yfov 0.8 rad, aspect 1.5) and an orthographic one.
/// The perspective camera is the first. Both cameras and the directional
/// and spot lights face the cube's centre, which stands on the rig's axis,
/// so the clip keeps them facing it: a render through the file's first
/// camera, lit by the file's own lights, frames the lit cube at every time.
/// Every node, light and camera has a name of its own, and every value but
/// the spec's cones, field of view and aspect is exact in binary.
pub fn staged() -> Scene {
    let mut s = Scene { name: Some("staged".into()), ..Default::default() };
    s.materials.push(Material {
        name: Some("ground_mat".into()),
        base_color: [0.5, 0.5, 0.5, 1.0],
        metallic: 0.0,
        roughness: 1.0,
        ..Default::default()
    });
    s.materials.push(Material {
        name: Some("cube_mat".into()),
        base_color: [0.75, 0.25, 0.125, 1.0],
        metallic: 0.0,
        roughness: 0.5,
        ..Default::default()
    });
    // The ground in the XZ plane, facing +Y.
    let ground = Primitive {
        positions: vec![[-2.0, 0.0, 2.0], [2.0, 0.0, 2.0], [2.0, 0.0, -2.0], [-2.0, 0.0, -2.0]],
        normals: Some(vec![[0.0, 1.0, 0.0]; 4]),
        indices: vec![0, 1, 2, 0, 2, 3],
        material: Some(0),
        ..Default::default()
    };
    s.meshes.push(SceneMesh { name: Some("ground".into()), primitives: vec![ground], ..Default::default() });
    s.meshes.push(SceneMesh { name: Some("cube".into()), primitives: vec![box_primitive(Some(1))], ..Default::default() });
    s.lights = vec![
        Light { name: Some("sun".into()), kind: LightKind::Directional, color: [1.0, 0.875, 0.75], intensity: 3.0, range: None },
        Light { name: Some("bulb".into()), kind: LightKind::Point, color: [1.0, 0.75, 0.5], intensity: 20.0, range: Some(10.0) },
        Light {
            name: Some("spot".into()),
            kind: LightKind::Spot { inner_cone: 0.2, outer_cone: 0.6 },
            color: [0.5, 0.75, 1.0],
            intensity: 40.0,
            range: None,
        },
    ];
    s.cameras = vec![
        Camera {
            name: Some("persp_camera".into()),
            projection: Projection::Perspective { yfov: 0.8, aspect: Some(1.5), znear: 0.125, zfar: Some(64.0) },
        },
        Camera {
            name: Some("ortho_camera".into()),
            projection: Projection::Orthographic { xmag: 3.0, ymag: 2.0, znear: 0.0625, zfar: 20.0 },
        },
    ];

    s.nodes.push(Node { name: Some("ground_node".into()), mesh: Some(0), ..Default::default() });
    s.nodes.push(Node {
        name: Some("cube_node".into()),
        transform: trs([0.5, 0.5, -0.25], axis_angle([0.0, 1.0, 0.0], 30.0), [1.0; 3]),
        mesh: Some(1),
        ..Default::default()
    });
    // The rig stands under the cube's centre, which is (0, 0.5, 0) in its
    // space.
    s.nodes.push(Node {
        name: Some("rig".into()),
        transform: trs([0.5, 0.0, -0.25], math::QUAT_IDENTITY, [1.0; 3]),
        children: vec![3, 4, 5, 6, 7],
        ..Default::default()
    });
    let facing = |eye: [f64; 3]| trs(eye, aim(eye, [0.0, 0.5, 0.0]), [1.0; 3]);
    s.nodes.push(Node { name: Some("persp_camera_node".into()), transform: facing([0.0, 1.5, 4.0]), camera: Some(0), ..Default::default() });
    s.nodes.push(Node { name: Some("ortho_camera_node".into()), transform: facing([-3.0, 2.5, 3.0]), camera: Some(1), ..Default::default() });
    s.nodes.push(Node { name: Some("sun_node".into()), transform: facing([2.0, 4.0, 3.0]), light: Some(0), ..Default::default() });
    s.nodes.push(Node {
        name: Some("bulb_node".into()),
        transform: trs([-1.5, 2.0, 1.0], math::QUAT_IDENTITY, [1.0; 3]),
        light: Some(1),
        ..Default::default()
    });
    s.nodes.push(Node { name: Some("spot_node".into()), transform: facing([1.5, 2.5, 1.5]), light: Some(2), ..Default::default() });
    s.roots = vec![0, 1, 2];

    // A quarter turn about +Y over 2 s.
    let mut turn = Vec::new();
    for deg in [0.0, 45.0, 90.0] {
        turn.extend(axis_angle([0.0, 1.0, 0.0], deg).iter().map(|v| *v as f32));
    }
    s.animations.push(Animation {
        name: Some("Orbit".into()),
        channels: vec![Channel { node: 2, path: AnimPath::Rotation, interpolation: Interpolation::Linear, times: vec![0.0, 1.0, 2.0], values: turn }],
    });
    s
}

/// Points and lines beside triangles: a unit cube, a square line loop and a
/// 3 x 3 grid of points. The loop (four segments as a line list, 1.5 m a
/// side, 1 m above the cube) hangs from a node a 2 s clip carries, turning it
/// a quarter about +Y while it travels, so that the loop's bounds change
/// along the way. The grid lies 1 m below the cube and stays put. Every loop
/// vertex and every point has a colour of its own, and neither has a
/// material or normals; the cube has its own material and no colours. Every
/// node, mesh and material has a name of its own, and every value is exact in
/// binary.
pub fn wire() -> Scene {
    let mut s = Scene { name: Some("wire".into()), ..Default::default() };
    s.materials.push(Material {
        name: Some("wire_cube_mat".into()),
        base_color: [0.75, 0.5, 0.25, 1.0],
        metallic: 0.0,
        roughness: 0.5,
        ..Default::default()
    });
    let ring = Primitive {
        positions: vec![[-0.75, 1.0, -0.75], [0.75, 1.0, -0.75], [0.75, 1.0, 0.75], [-0.75, 1.0, 0.75]],
        colors: Some(vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0], [1.0, 1.0, 0.0, 1.0]]),
        indices: vec![0, 1, 1, 2, 2, 3, 3, 0],
        topology: Topology::Lines,
        ..Default::default()
    };
    // Row `z`, column `x`: the position steps a metre, the colour steps by
    // halves in red and green, with blue half on.
    let cell = |z: u32, x: u32| (z * 3 + x) as usize;
    let mut grid = Primitive { topology: Topology::Points, ..Default::default() };
    grid.positions = vec![[0.0; 3]; 9];
    let mut colors = vec![[0.0f32; 4]; 9];
    for z in 0..3u32 {
        for x in 0..3u32 {
            grid.positions[cell(z, x)] = [x as f32 - 1.0, -1.0, z as f32 - 1.0];
            colors[cell(z, x)] = [x as f32 / 2.0, z as f32 / 2.0, 0.5, 1.0];
        }
    }
    grid.colors = Some(colors);
    grid.indices = (0..9).collect();
    s.meshes.push(SceneMesh { name: Some("wire_cube".into()), primitives: vec![box_primitive(Some(0))], ..Default::default() });
    s.meshes.push(SceneMesh { name: Some("wire_loop".into()), primitives: vec![ring], ..Default::default() });
    s.meshes.push(SceneMesh { name: Some("wire_points".into()), primitives: vec![grid], ..Default::default() });
    s.nodes.push(Node { name: Some("wire_cube_node".into()), mesh: Some(0), ..Default::default() });
    s.nodes.push(Node { name: Some("wire_loop_node".into()), mesh: Some(1), ..Default::default() });
    s.nodes.push(Node { name: Some("wire_points_node".into()), mesh: Some(2), ..Default::default() });
    s.roots = vec![0, 1, 2];

    let times = vec![0.0, 1.0, 2.0];
    let mut turn = Vec::new();
    for deg in [0.0, 45.0, 90.0] {
        turn.extend(axis_angle([0.0, 1.0, 0.0], deg).iter().map(|v| *v as f32));
    }
    s.animations.push(Animation {
        name: Some("Swing".into()),
        channels: vec![
            Channel {
                node: 1,
                path: AnimPath::Translation,
                interpolation: Interpolation::Linear,
                times: times.clone(),
                values: vec![0.0, 0.0, 0.0, 0.5, 0.25, 0.0, 0.5, 0.5, -0.5],
            },
            Channel { node: 1, path: AnimPath::Rotation, interpolation: Interpolation::Linear, times, values: turn },
        ],
    });
    s
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

/// The hostile-input sweep each reader path of mesh-gaps item 4 gets, and the
/// lean `layered()` those sweeps read (Rulings A40, A40a, A40d): the one copy
/// that the layer sweeps of glb.rs, usd.rs, obj.rs, dae.rs and fbx_read.rs
/// share. Test-only: this file is compiled into the library for the harness
/// example, which has no use for it.
#[cfg(test)]
pub(crate) mod sweeps {
    use super::*;

    /// The hostile-input sweep each reader path of mesh-gaps item 4 gets (the
    /// overview's bar): `file` cut at every length, with each bit of its
    /// first 4 KiB and of `layers` flipped in turn, and with 2–8 bits flipped
    /// at random in 10,000 cases seeded with `FLIP_SEED`. `read` must return
    /// every time, never panic. The count of readings and the slowest one are
    /// printed (`--nocapture`), for the release run's bar of a second a case.
    /// usd.rs's `uv_set_inputs_survive_truncation_and_bit_flips` is the
    /// pattern.
    pub(crate) fn layer_sweep(name: &str, file: &[u8], layers: std::ops::Range<usize>, read: impl Fn(&[u8])) {
        const FLIP_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
        let started = std::time::Instant::now();
        let (mut readings, mut slowest) = (0usize, (std::time::Duration::ZERO, String::new()));
        let mut survives = |bytes: &[u8], case: &dyn Fn() -> String| {
            let at = std::time::Instant::now();
            let returned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read(bytes))).is_ok();
            let took = at.elapsed();
            assert!(returned, "{name}: {}", case());
            readings += 1;
            if took > slowest.0 {
                slowest = (took, case());
            }
        };
        for cut in 0..file.len() {
            survives(&file[..cut], &|| format!("cut to {cut} bytes"));
        }
        let single: Vec<usize> = (0..file.len().min(4096)).chain(layers.filter(|i| (4096..file.len()).contains(i))).collect();
        let mut flipped = file.to_vec();
        for &byte in &single {
            for bit in 0..8 {
                flipped[byte] ^= 1 << bit;
                survives(&flipped, &|| format!("bit {} flipped", byte * 8 + bit));
                flipped[byte] ^= 1 << bit;
            }
        }
        // xorshift64 (Marsaglia, 2003): std only, and the same cases every
        // run, seeded again for each file.
        let mut state = FLIP_SEED;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let bits = file.len() as u64 * 8;
        for case in 0..10_000 {
            let flips = 2 + next() % 7;
            let mut chosen: Vec<u64> = Vec::new();
            while (chosen.len() as u64) < flips {
                let bit = next() % bits;
                if !chosen.contains(&bit) {
                    chosen.push(bit);
                }
            }
            let mut flipped = file.to_vec();
            for &bit in &chosen {
                flipped[(bit / 8) as usize] ^= 1 << (bit % 8);
            }
            survives(&flipped, &|| format!("case {case}: bits {chosen:?} flipped"));
        }
        eprintln!(
            "{name}: {} bytes; {readings} readings: {} cuts, {} single-bit flips, 10000 seeded cases; slowest {:?} ({}); all {:?}",
            file.len(),
            file.len(),
            single.len() * 8,
            slowest.0,
            slowest.1,
            started.elapsed()
        );
    }

    /// The bytes of `file` from its first `from` to its first `to`: the span
    /// a [`layer_sweep`] flips every bit of. A missing marker panics, and so
    /// does an empty span, so a sweep never runs over nothing unnoticed.
    pub(crate) fn span(name: &str, file: &[u8], from: &str, to: &str) -> std::ops::Range<usize> {
        let at = |key: &str| file.windows(key.len()).position(|w| w == key.as_bytes()).unwrap_or_else(|| panic!("{name} has no {key}"));
        let (start, end) = (at(from), at(to));
        assert!(start < end, "{name}: {from} at {start} is not before {to} at {end}");
        start..end
    }

    /// [`layered`] as item 4's sweeps read it (Ruling A40a): the
    /// materials named in `keep`, each with its textures and every layer
    /// value unchanged, on its mesh's first triangle, with the two images
    /// 1×1 PNGs. The layer paths read a file's materials, which stay
    /// layered's own. What goes is the boxes' geometry and the images'
    /// pixels: the paths that parse those are not item 4's, and have sweeps
    /// of their own. A sweep reads its file tens of thousands of times, and a
    /// whole debug reading of full-size layered costs about 1 to 2 ms, which
    /// made each full-size sweep take up to two minutes (the `_full` tests).
    pub(crate) fn lean_layered(keep: &[&str]) -> Scene {
        let full = layered();
        let pixel = encode_png(1, 1, &[128, 128, 255, 255]);
        let mut s = Scene {
            name: full.name.clone(),
            images: full.images.iter().map(|im| Image { data: pixel.clone(), ..im.clone() }).collect(),
            textures: full.textures.clone(),
            nodes: vec![Node { children: Vec::new(), ..full.nodes[0].clone() }],
            roots: vec![0],
            ..Default::default()
        };
        for (i, m) in full.materials.iter().enumerate() {
            if !keep.contains(&m.name.as_deref().unwrap_or_default()) {
                continue;
            }
            let mut p = full.meshes[i].primitives[0].clone();
            assert!(p.material == Some(i) && p.skin.is_none() && p.targets.is_empty(), "layered's mesh {i} draws material {i}, unskinned");
            let corners: Vec<usize> = p.indices[..3].iter().map(|&c| c as usize).collect();
            p.positions = corners.iter().map(|&c| p.positions[c]).collect();
            p.normals = p.normals.as_ref().map(|n| corners.iter().map(|&c| n[c]).collect());
            p.tangents = p.tangents.as_ref().map(|t| corners.iter().map(|&c| t[c]).collect());
            p.colors = p.colors.as_ref().map(|k| corners.iter().map(|&c| k[c]).collect());
            p.uvs = p.uvs.iter().map(|set| corners.iter().map(|&c| set[c]).collect()).collect();
            p.indices = vec![0, 1, 2];
            p.material = Some(s.materials.len());
            s.materials.push(m.clone());
            s.meshes.push(SceneMesh { primitives: vec![p], ..full.meshes[i].clone() });
            s.nodes.push(Node { mesh: Some(s.meshes.len() - 1), ..full.nodes[i + 1].clone() });
            let node = s.nodes.len() - 1;
            s.nodes[0].children.push(node);
        }
        assert_eq!(s.materials.len(), keep.len(), "every name in {keep:?} is one of layered's materials");
        s
    }
}

/// Where a scene's lights and cameras stand, and the one comparison of two
/// scenes' that the round trips of mesh-gaps item 5 share: glTF's in glb.rs,
/// FBX's, Collada's and USD's, and `scene_matrix_tests`. Test-only, as
/// [`sweeps`] is.
#[cfg(test)]
pub(crate) mod lightcam {
    use super::*;

    /// A placed light in world space: where its node stands in one pose.
    #[derive(Debug, Clone)]
    pub(crate) struct LightFrame {
        /// The name of the node that places it, which the comparison matches by.
        pub(crate) node: String,
        /// The node's world origin.
        pub(crate) position: [f64; 3],
        /// The node's world −Z, normalised: the way the light shines.
        pub(crate) direction: [f64; 3],
        /// Its colour, intensity, range and cones.
        pub(crate) light: Light,
    }

    /// A placed camera in world space: where its node stands in one pose.
    #[derive(Debug, Clone)]
    pub(crate) struct CameraFrame {
        /// The name of the node that places it, which the comparison matches by.
        pub(crate) node: String,
        /// The node's world origin.
        pub(crate) eye: [f64; 3],
        /// The node's world −Z, normalised: the way the camera looks.
        pub(crate) forward: [f64; 3],
        /// The node's world +Y, normalised.
        pub(crate) up: [f64; 3],
        /// Its projection.
        pub(crate) camera: Camera,
    }

    fn unit(v: [f64; 3]) -> [f64; 3] {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if l > 0.0 { [v[0] / l, v[1] / l, v[2] / l] } else { v }
    }

    /// Every light and camera a node of `scene` places, in world space at
    /// `time` of its first clip, or at rest for `None` or a scene with no
    /// clip; each list sorted by node name. The axes are normalised, so a
    /// node's scale does not count, as glTF's camera rule has it. A node that
    /// places one must have a name, of its own among the nodes placing that
    /// kind: the comparison matches by it.
    pub(crate) fn frames(scene: &Scene, time: Option<f32>) -> (Vec<LightFrame>, Vec<CameraFrame>) {
        let pose = match time {
            Some(t) if !scene.animations.is_empty() => super::super::pose::sample_pose(scene, 0, t),
            _ => super::super::pose::default_pose(scene),
        };
        let world = super::super::scene::world_from_locals(scene, &pose.locals());
        let (mut lights, mut cameras) = (Vec::new(), Vec::new());
        for (i, n) in scene.nodes.iter().enumerate().filter(|(_, n)| n.light.is_some() || n.camera.is_some()) {
            let node = n.name.clone().unwrap_or_else(|| panic!("node {i} places a light or camera and has no name to match it by"));
            let m = &world[i];
            let origin = [m[12], m[13], m[14]];
            let minus_z = unit([-m[8], -m[9], -m[10]]);
            if let Some(l) = n.light {
                lights.push(LightFrame { node: node.clone(), position: origin, direction: minus_z, light: scene.lights[l].clone() });
            }
            if let Some(c) = n.camera {
                cameras.push(CameraFrame { node, eye: origin, forward: minus_z, up: unit([m[4], m[5], m[6]]), camera: scene.cameras[c].clone() });
            }
        }
        lights.sort_by(|a, b| a.node.cmp(&b.node));
        cameras.sort_by(|a, b| a.node.cmp(&b.node));
        let twice = |names: Vec<&String>| names.windows(2).find(|w| w[0] == w[1]).map(|w| w[0].clone());
        if let Some(name) = twice(lights.iter().map(|f| &f.node).collect()).or_else(|| twice(cameras.iter().map(|f| &f.node).collect())) {
            panic!("two nodes named {name:?} place a light or two place a camera; the comparison matches by name");
        }
        (lights, cameras)
    }

    /// One value against the expected one, within `tolerance`; the failure
    /// names it.
    fn close(at: &str, what: &str, want: f64, got: f64, tolerance: f64) {
        assert!((want - got).abs() <= tolerance, "{at}: {what} {got} against {want}, past {tolerance}");
    }

    fn close3(at: &str, what: &str, want: [f64; 3], got: [f64; 3], tolerance: f64) {
        let near = (0..3).all(|k| (want[k] - got[k]).abs() <= tolerance);
        assert!(near, "{at}: {what} {got:?} against {want:?}, past {tolerance}");
    }

    fn close_some(at: &str, what: &str, want: Option<f32>, got: Option<f32>, tolerance: f64) {
        match (want, got) {
            (None, None) => {}
            (Some(w), Some(g)) => close(at, what, w.into(), g.into(), tolerance),
            _ => panic!("{at}: {what} {got:?} against {want:?}"),
        }
    }

    /// `got`'s lights and cameras against `want`'s, at rest and at every
    /// [`sample_times`] time of `want`'s first clip, matched by the name of
    /// the node that places each ([`frames`]): the same nodes place them, and
    /// each light's world position and direction (its node's −Z), colour,
    /// intensity, range and cones, and each camera's world eye, forward (−Z)
    /// and up (+Y) and its projection, are within `tolerance` of `want`'s.
    /// The failure names the time, the node and the value that differs.
    pub(crate) fn assert_same_lights_and_cameras(want: &Scene, got: &Scene, tolerance: f64, ctx: &str) {
        for time in std::iter::once(None).chain(sample_times(want).into_iter().map(Some)) {
            let when = time.map_or("at rest".to_string(), |t| format!("at {t} s"));
            let ((want_lights, want_cameras), (got_lights, got_cameras)) = (frames(want, time), frames(got, time));
            let lit: Vec<&str> = got_lights.iter().map(|f| f.node.as_str()).collect();
            let expected: Vec<&str> = want_lights.iter().map(|f| f.node.as_str()).collect();
            assert_eq!(lit, expected, "{ctx} {when}: the nodes that place lights");
            let seeing: Vec<&str> = got_cameras.iter().map(|f| f.node.as_str()).collect();
            let expected: Vec<&str> = want_cameras.iter().map(|f| f.node.as_str()).collect();
            assert_eq!(seeing, expected, "{ctx} {when}: the nodes that place cameras");
            for (w, g) in want_lights.iter().zip(&got_lights) {
                let at = format!("{ctx} {when}: the light on {:?}", w.node);
                close3(&at, "position", w.position, g.position, tolerance);
                close3(&at, "direction", w.direction, g.direction, tolerance);
                close3(&at, "colour", w.light.color.map(f64::from), g.light.color.map(f64::from), tolerance);
                close(&at, "intensity", w.light.intensity.into(), g.light.intensity.into(), tolerance);
                close_some(&at, "range", w.light.range, g.light.range, tolerance);
                match (w.light.kind, g.light.kind) {
                    (LightKind::Spot { inner_cone: wi, outer_cone: wo }, LightKind::Spot { inner_cone: gi, outer_cone: go }) => {
                        close(&at, "inner cone", wi.into(), gi.into(), tolerance);
                        close(&at, "outer cone", wo.into(), go.into(), tolerance);
                    }
                    (wk, gk) => assert!(wk == gk, "{at}: kind {gk:?} against {wk:?}"),
                }
            }
            for (w, g) in want_cameras.iter().zip(&got_cameras) {
                let at = format!("{ctx} {when}: the camera on {:?}", w.node);
                close3(&at, "eye", w.eye, g.eye, tolerance);
                close3(&at, "forward", w.forward, g.forward, tolerance);
                close3(&at, "up", w.up, g.up, tolerance);
                match (w.camera.projection, g.camera.projection) {
                    (
                        Projection::Perspective { yfov: wy, aspect: wa, znear: wn, zfar: wf },
                        Projection::Perspective { yfov: gy, aspect: ga, znear: gn, zfar: gf },
                    ) => {
                        close(&at, "yfov", wy.into(), gy.into(), tolerance);
                        close_some(&at, "aspect ratio", wa, ga, tolerance);
                        close(&at, "znear", wn.into(), gn.into(), tolerance);
                        close_some(&at, "zfar", wf, gf, tolerance);
                    }
                    (
                        Projection::Orthographic { xmag: wx, ymag: wy, znear: wn, zfar: wf },
                        Projection::Orthographic { xmag: gx, ymag: gy, znear: gn, zfar: gf },
                    ) => {
                        close(&at, "xmag", wx.into(), gx.into(), tolerance);
                        close(&at, "ymag", wy.into(), gy.into(), tolerance);
                        close(&at, "znear", wn.into(), gn.into(), tolerance);
                        close(&at, "zfar", wf.into(), gf.into(), tolerance);
                    }
                    (wp, gp) => panic!("{at}: projection {gp:?} against {wp:?}"),
                }
            }
        }
    }
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

    /// The car paint's written tangents follow glTF: unit, orthogonal to the
    /// normal, along the growth of u, and with w such that
    /// `cross(normal, tangent) * w` points the way v DECREASES (glTF's v
    /// runs down the image; its normal maps are +Y up). On the box's +Z
    /// face, whose corners map to (0,1), (1,1), (1,0), (0,0), that is
    /// (1, 0, 0, +1): u grows along +X and v decreases along +Y.
    #[test]
    fn car_paint_tangents_follow_gltf() {
        let s = layered();
        let mesh = s.meshes.iter().find(|m| m.name.as_deref() == Some("car_paint")).unwrap();
        let p = &mesh.primitives[0];
        let tangents = p.tangents.as_ref().expect("car_paint carries tangents");
        let normals = p.normals.as_ref().unwrap();
        let uvs = &p.uvs[0];
        assert_eq!(tangents.len(), p.positions.len());
        // The +Z face is the first: corner 0 at uv (0, 1), corner 1 at (1, 1),
        // corner 3 at (0, 0).
        assert_eq!(normals[0], [0.0, 0.0, 1.0]);
        assert_eq!((uvs[0], uvs[1], uvs[3]), ([0.0, 1.0], [1.0, 1.0], [0.0, 0.0]));
        assert_eq!(tangents[0], [1.0, 0.0, 0.0, 1.0], "the +Z face's tangent");
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        for face in 0..6 {
            let c = face * 4;
            for v in c..c + 4 {
                let t = tangents[v];
                let n = normals[v];
                let xyz = [t[0], t[1], t[2]];
                assert!((dot(xyz, xyz) - 1.0).abs() < 1e-5, "face {face}: unit");
                assert!(dot(xyz, n).abs() < 1e-5, "face {face}: orthogonal to the normal");
                assert!(t[3] == 1.0 || t[3] == -1.0, "face {face}: w is a sign");
                // u grows from corner 0 to corner 1; v decreases from corner 0
                // (v = 1) to corner 3 (v = 0).
                let du = sub(p.positions[c + 1], p.positions[c]);
                let dv_down = sub(p.positions[c + 3], p.positions[c]);
                assert!(dot(xyz, du) > 0.0, "face {face}: the tangent runs with u");
                let cross = [n[1] * xyz[2] - n[2] * xyz[1], n[2] * xyz[0] - n[0] * xyz[2], n[0] * xyz[1] - n[1] * xyz[0]];
                let bitangent = [cross[0] * t[3], cross[1] * t[3], cross[2] * t[3]];
                assert!(dot(bitangent, dv_down) > 0.0, "face {face}: cross(N, T) * w points the way v decreases, as glTF's bitangent does");
            }
        }
    }

    #[test]
    fn fixtures_are_deterministic() {
        assert_eq!(all(), all());
    }

    /// A sweep's span is found or the sweep stops: a missing marker, or one
    /// that comes after its end, panics rather than sweeping nothing.
    #[test]
    fn a_sweep_span_is_found_and_not_empty() {
        let file = b"<a>x</a>";
        assert_eq!(sweeps::span("doc", file, "<a>", "</a>"), 0..4);
        for (from, to) in [("<b>", "</a>"), ("<a>", "</b>"), ("</a>", "<a>")] {
            assert!(std::panic::catch_unwind(|| sweeps::span("doc", file, from, to)).is_err(), "{from} .. {to}");
        }
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
    fn multiuv_sets_tell_every_swap_and_shift_apart() {
        // What the uv-set tests lean on: no value of one set appears in
        // another, and no two corners of a set agree, so a set read as
        // another, or shifted by a vertex, never compares equal.
        for s in [multiuv(), multiuv_shared()] {
            for p in s.meshes.iter().flat_map(|m| &m.primitives) {
                assert_eq!(p.uvs.len(), 4);
                for (a, sa) in p.uvs.iter().enumerate() {
                    for (i, x) in sa.iter().enumerate() {
                        assert!(sa[i + 1..].iter().all(|y| y != x), "set {a} repeats {x:?}");
                    }
                    for (b, sb) in p.uvs.iter().enumerate().filter(|(b, _)| *b != a) {
                        assert!(sa.iter().all(|x| !sb.contains(x)), "sets {a} and {b} share a value");
                    }
                }
            }
            let m = &s.materials[0];
            let (base, occlusion) = (m.base_color_texture.as_ref().unwrap(), m.occlusion_texture.as_ref().unwrap());
            assert_eq!((base.uv_set, occlusion.uv_set), (3, 1));
            let image = |t: usize| &s.images[s.textures[t].image].data;
            assert_ne!(image(base.texture), image(occlusion.texture), "a swapped binding shows");
        }
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

    /// What the points and lines tests lean on: one primitive of each
    /// topology, in the order triangles, lines, points; the loop's four
    /// segments and the grid's nine points, each vertex its own colour; a
    /// clip that moves the loop alone; and a name of its own for every node,
    /// mesh and material.
    #[test]
    fn wire_holds_a_cube_a_coloured_loop_and_a_coloured_grid() {
        let s = wire();
        s.validate().unwrap();
        let prims: Vec<&Primitive> = s.meshes.iter().flat_map(|m| &m.primitives).collect();
        let topologies: Vec<Topology> = prims.iter().map(|p| p.topology).collect();
        assert_eq!(topologies, [Topology::Triangles, Topology::Lines, Topology::Points]);
        let (cube, ring, grid) = (prims[0], prims[1], prims[2]);
        assert_eq!(cube.triangle_count(), 12);
        assert_eq!((ring.positions.len(), ring.indices.as_slice()), (4, &[0, 1, 1, 2, 2, 3, 3, 0][..]), "four segments, as a line list");
        assert_eq!((grid.positions.len(), grid.indices.clone()), (9, (0..9).collect::<Vec<u32>>()), "nine points, as a point list");
        for p in [ring, grid] {
            let colors = p.colors.as_ref().expect("coloured");
            assert_eq!(colors.len(), p.positions.len());
            assert!(colors.iter().enumerate().all(|(i, c)| colors[i + 1..].iter().all(|d| d != c)), "no two vertices share a colour: {colors:?}");
            assert!(p.material.is_none() && p.normals.is_none() && p.uvs.is_empty());
        }
        assert!(cube.colors.is_none() && cube.material == Some(0));
        assert_eq!(s.nodes.iter().filter(|n| n.mesh.is_some()).count(), 3);

        let mut names: Vec<&str> = s.nodes.iter().filter_map(|n| n.name.as_deref()).collect();
        names.extend(s.meshes.iter().filter_map(|m| m.name.as_deref()));
        names.extend(s.materials.iter().filter_map(|m| m.name.as_deref()));
        let total = s.nodes.len() + s.meshes.len() + s.materials.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "{names:?}");

        // The clip moves the loop's node and no other, and does move it.
        let moved: Vec<Option<&str>> =
            s.animations[0].channels.iter().map(|c| s.nodes[c.node].name.as_deref()).collect();
        assert!(!moved.is_empty() && moved.iter().all(|n| *n == Some("wire_loop_node")), "{moved:?}");
        assert_eq!(s.nodes[s.nodes.iter().position(|n| n.mesh == Some(1)).unwrap()].name.as_deref(), Some("wire_loop_node"));
        let centre_at = |t: f32| {
            let wp = super::super::pose::world_primitives(&s, &super::super::pose::sample_pose(&s, 0, t)).unwrap();
            let ring = wp.iter().find(|w| w.mesh == 1).expect("the loop is drawn");
            (0..3).map(|k| ring.positions.iter().map(|p| f64::from(p[k])).sum::<f64>() / 4.0).collect::<Vec<f64>>()
        };
        let (first, last) = (centre_at(0.0), centre_at(2.0));
        assert!((0..3).map(|k| (last[k] - first[k]).abs()).fold(0.0, f64::max) > 0.25, "{first:?} to {last:?}");
    }

    /// What a render through `staged`'s first camera, lit by its own lights,
    /// leans on: that camera is the perspective one; both cameras and the
    /// directional and spot lights face the cube's centre at rest and at
    /// every sample time; the clip moves them; and every node, light and
    /// camera has a name of its own.
    #[test]
    fn staged_faces_the_cube_from_lights_and_cameras_that_move() {
        let s = staged();
        assert!(matches!(s.cameras[0].projection, Projection::Perspective { .. }), "the first camera is the perspective one");
        let mut names: Vec<&str> = s.nodes.iter().filter_map(|n| n.name.as_deref()).collect();
        names.extend(s.lights.iter().filter_map(|l| l.name.as_deref()));
        names.extend(s.cameras.iter().filter_map(|c| c.name.as_deref()));
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), s.nodes.len() + s.lights.len() + s.cameras.len(), "{names:?}");

        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let cube = s.nodes.iter().position(|n| n.name.as_deref() == Some("cube_node")).unwrap();
        let mut eyes = Vec::new();
        let poses = std::iter::once(super::super::pose::default_pose(&s))
            .chain(sample_times(&s).into_iter().map(|t| super::super::pose::sample_pose(&s, 0, t)));
        for (k, pose) in poses.enumerate() {
            let world = super::super::scene::world_from_locals(&s, &pose.locals());
            let centre = math::transform_point_f64(&world[cube], [0.0; 3]);
            for (i, n) in s.nodes.iter().enumerate() {
                if !(n.camera.is_some() || n.light.is_some_and(|l| s.lights[l].kind != LightKind::Point)) {
                    continue;
                }
                let m = world[i];
                let eye = [m[12], m[13], m[14]];
                let (forward, to) = ([-m[8], -m[9], -m[10]], [centre[0] - eye[0], centre[1] - eye[1], centre[2] - eye[2]]);
                let cos = dot(forward, to) / (dot(forward, forward) * dot(to, to)).sqrt();
                assert!(cos > 1.0 - 1e-6, "{:?}, pose {k}: {}° off the cube's centre", n.name, cos.min(1.0).acos().to_degrees());
                if n.camera == Some(0) {
                    eyes.push(eye);
                }
            }
        }
        let (first, last) = (eyes[0], eyes[eyes.len() - 1]);
        let moved = [last[0] - first[0], last[1] - first[1], last[2] - first[2]];
        assert!(dot(moved, moved).sqrt() > 1.0, "the clip moves the first camera, by {moved:?}");
    }

    /// The round trips' light and camera comparison is only worth something
    /// if it fails when it should: `staged` against itself passes, and a cone
    /// 2e-5 wider, a light turned a milliradian, a clip whose last key moved
    /// (so only the later sample times differ), a camera gone and a light of
    /// another kind each fail, naming the value.
    #[test]
    fn the_light_and_camera_comparison_notices_what_moves() {
        let s = staged();
        let failure = |got: &Scene| -> String {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lightcam::assert_same_lights_and_cameras(&s, got, 1e-5, "case")));
            r.err().map(|e| e.downcast_ref::<String>().cloned().unwrap_or_default()).unwrap_or_default()
        };
        assert_eq!(failure(&s), "", "staged against itself");
        let node = |name: &str| s.nodes.iter().position(|n| n.name.as_deref() == Some(name)).unwrap();
        let mut wide = s.clone();
        wide.lights[2].kind = LightKind::Spot { inner_cone: 0.2, outer_cone: 0.60002 };
        let mut turned = s.clone();
        if let Transform::Trs { rotation, .. } = &mut turned.nodes[node("spot_node")].transform {
            *rotation = math::quat_mul(*rotation, axis_angle([1.0, 0.0, 0.0], 0.001f64.to_degrees()));
        }
        let mut late = s.clone();
        late.animations[0].channels[0].values[8..12].copy_from_slice(&axis_angle([0.0, 1.0, 0.0], 91.0).map(|v| v as f32));
        let mut blind = s.clone();
        blind.nodes[node("ortho_camera_node")].camera = None;
        blind.cameras.truncate(1);
        let mut other = s.clone();
        other.lights[0].kind = LightKind::Point;
        for (what, got, names) in [
            ("a wider cone", &wide, r#"case at rest: the light on "spot_node": outer cone "#),
            ("a turned light", &turned, r#"case at rest: the light on "spot_node": direction"#),
            ("a later key", &late, r#"case at 1.16 s: the light on "bulb_node": position"#),
            ("a camera gone", &blind, "case at rest: the nodes that place cameras"),
            ("another kind", &other, r#"case at rest: the light on "sun_node": kind Point against Directional"#),
        ] {
            let message = failure(got);
            assert!(message.contains(names), "{what}: {message:?}");
        }
    }
}
