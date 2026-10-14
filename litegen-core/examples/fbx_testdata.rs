//! Writes the source GLBs of the Assimp-generated vectors in
//! `mesh/testdata/fbx/`:
//!
//! - `fbx_testdata <fixture> out.glb`: the fixture with its roots under an
//!   identity node named `wrapper` (Assimp's FBX exporter turns a glTF
//!   scene's single root into the FBX scene root and drops its transform);
//! - `fbx_testdata skinned_small out.glb`: the `skinned` fixture with its
//!   tube decimated (every 4th ring and side), so Assimp's uncompressed FBX
//!   of it stays small, and with only its LINEAR channels (Assimp writes
//!   STEP as linear and CUBICSPLINE as garbage).
//!
//! `cargo run --example fbx_testdata -- textured out.glb`, then
//! `assimp export out.glb x.fbx -ffbx`; `testdata/fbx/README.md` lists the
//! exact commands for each vector.
use litegen::mesh::{self, fixtures, scene::{Node, Primitive}, MeshFormat, WriteOptions};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (which, out) = (&a[0], &a[1]);
    let mut s = if which == "skinned_small" { small_skinned() } else { fixtures::by_name(which).expect("fixture") };
    if which != "skinned_small" {
        let roots = std::mem::take(&mut s.roots);
        s.add_root(Node { name: Some("wrapper".into()), children: roots, ..Default::default() });
    }
    s.validate().unwrap();
    let w = mesh::write_scene(&s, MeshFormat::Glb, WriteOptions::default()).unwrap();
    std::fs::write(out, &w.package.files[0].bytes).unwrap();
}

fn small_skinned() -> mesh::Scene {
    let mut s = fixtures::skinned();
    let p = &s.meshes[0].primitives[0];
    let (sides, rings, step) = (16usize, 24usize, 4usize);
    let row = sides + 1;
    let keep_k: Vec<usize> = (0..=sides).step_by(step).collect();
    let keep_r: Vec<usize> = (0..=rings).step_by(step).collect();
    let mut q = Primitive { material: p.material, ..Default::default() };
    let (mut n, mut uv, mut js, mut ws) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let sw = p.skin.as_ref().unwrap();
    for &r in &keep_r {
        for &k in &keep_k {
            let v = r * row + k;
            q.positions.push(p.positions[v]);
            n.push(p.normals.as_ref().unwrap()[v]);
            uv.push(p.uvs[0][v]);
            let (j, w) = sw.vertex(v);
            js.extend_from_slice(j);
            ws.extend_from_slice(w);
        }
    }
    let nk = keep_k.len() as u32;
    for r in 0..keep_r.len() as u32 - 1 {
        for k in 0..nk - 1 {
            let a = r * nk + k;
            q.indices.extend_from_slice(&[a, a + nk, a + 1, a + 1, a + nk, a + nk + 1]);
        }
    }
    q.normals = Some(n);
    q.uvs = vec![uv];
    q.skin = Some(mesh::scene::SkinWeights { influences: 4, joints: js, weights: ws });
    s.meshes[0].primitives = vec![q];
    // Assimp 6.0 writes a glTF CUBICSPLINE channel as three FBX keys per key
    // (in-tangent, value, out-tangent) at one time, and a STEP channel as
    // linear keys; keep only the LINEAR channels, which it carries exactly.
    s.animations[0].channels.retain(|c| c.interpolation == mesh::scene::Interpolation::Linear);
    s
}
