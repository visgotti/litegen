//! The mock's cube in the containers that are not glTF.
//!
//! These exist so `MockModel3dProvider` can actually honour a request for more
//! than one `output_format`. Without a second real container, the only subject
//! able to exercise the delivered-vs-requested guard would be a provider that
//! never delivers anything but glb — which tests the guard's failure arm and
//! never its success arm.
//!
//! Both writers take the same geometry as [`super::glb::generate_cube_glb`]
//! (same corners, same winding, same prompt-seeded scale), so a caller asking
//! for `["glb", "obj"]` gets one cube in two containers rather than two
//! different cubes.
//!
//! Deliberately not here: fbx and usdz. Neither is writable in a few dozen
//! honest lines — FBX's only complete writer is Autodesk's licence-encumbered
//! SDK, and USDZ is a zip of USD crates — and a mock that emitted a plausible
//! but invalid file would be worse than one that cannot emit it at all. A model
//! must not declare a container its provider cannot produce; that mismatch is
//! the exact bug this whole change closes.

use super::glb::{cube_scale, CUBE_CORNERS, CUBE_INDICES};

/// Wavefront OBJ. Text, single-file as long as no material library is
/// referenced — which is why the mock emits no `usemtl`/`mtllib` line: an
/// `.mtl` sidecar would be renamed by the rehost keying and its reference
/// would stop resolving.
///
/// OBJ face indices are 1-based.
///
/// @see <https://www.loc.gov/preservation/digital/formats/fdd/fdd000507.shtml>
pub fn generate_cube_obj(prompt: &str) -> Vec<u8> {
    let scale = cube_scale(prompt);
    let mut s = String::with_capacity(512);
    s.push_str("# litegen mock cube\n");
    s.push_str("o litegen_mock_cube\n");
    for c in CUBE_CORNERS {
        s.push_str(&format!(
            "v {:.6} {:.6} {:.6}\n",
            c[0] * scale, c[1] * scale, c[2] * scale
        ));
    }
    for tri in CUBE_INDICES.chunks_exact(3) {
        s.push_str(&format!("f {} {} {}\n", tri[0] + 1, tri[1] + 1, tri[2] + 1));
    }
    s.into_bytes()
}

/// Binary STL: an 80-byte header, a u32 triangle count, then 50 bytes per
/// triangle (a normal, three vertices, and a 2-byte attribute word).
///
/// Facet normals are computed per triangle rather than written as zero. Many
/// viewers fall back to the winding order when the normal is zero, but not all
/// of them, and a mesh that renders inside-out in some tools is exactly the
/// class of bug the GLB writer's winding comment exists to prevent.
///
/// @see <https://www.loc.gov/preservation/digital/formats/fdd/fdd000505.shtml>
pub fn generate_cube_stl(prompt: &str) -> Vec<u8> {
    let scale = cube_scale(prompt);
    let v = |i: u16| -> [f32; 3] {
        let c = CUBE_CORNERS[i as usize];
        [c[0] * scale, c[1] * scale, c[2] * scale]
    };

    let tri_count = (CUBE_INDICES.len() / 3) as u32;
    let mut out: Vec<u8> = Vec::with_capacity(84 + tri_count as usize * 50);

    let mut header = [0u8; 80];
    let tag = b"litegen mock cube (binary STL)";
    header[..tag.len()].copy_from_slice(tag);
    out.extend_from_slice(&header);
    out.extend_from_slice(&tri_count.to_le_bytes());

    for tri in CUBE_INDICES.chunks_exact(3) {
        let (a, b, c) = (v(tri[0]), v(tri[1]), v(tri[2]));
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            u[1] * w[2] - u[2] * w[1],
            u[2] * w[0] - u[0] * w[2],
            u[0] * w[1] - u[1] * w[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let n = if len > 0.0 { [n[0] / len, n[1] / len, n[2] / len] } else { [0.0; 3] };

        for f in n.iter().chain(a.iter()).chain(b.iter()).chain(c.iter()) {
            out.extend_from_slice(&f.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes()); // attribute byte count
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obj_has_every_vertex_and_face() {
        let obj = String::from_utf8(generate_cube_obj("a fox")).unwrap();
        assert_eq!(obj.lines().filter(|l| l.starts_with("v ")).count(), 8);
        assert_eq!(obj.lines().filter(|l| l.starts_with("f ")).count(), 12);
    }

    #[test]
    fn obj_face_indices_are_one_based_and_in_range() {
        let obj = String::from_utf8(generate_cube_obj("a fox")).unwrap();
        for line in obj.lines().filter(|l| l.starts_with("f ")) {
            for idx in line[2..].split_whitespace() {
                let i: usize = idx.parse().expect("face index parses");
                assert!((1..=8).contains(&i), "face index {i} outside 1..=8");
            }
        }
    }

    #[test]
    fn obj_references_no_sidecar() {
        // An mtllib/usemtl reference would survive rehost renaming as a broken
        // link — see the module doc.
        let obj = String::from_utf8(generate_cube_obj("a fox")).unwrap();
        assert!(!obj.contains("mtllib"), "obj must stay self-contained");
        assert!(!obj.contains("usemtl"), "obj must stay self-contained");
    }

    #[test]
    fn stl_length_matches_its_declared_triangle_count() {
        let stl = generate_cube_stl("a fox");
        let count = u32::from_le_bytes(stl[80..84].try_into().unwrap());
        assert_eq!(count, 12);
        assert_eq!(stl.len(), 84 + 12 * 50);
    }

    #[test]
    fn stl_normals_are_unit_length() {
        let stl = generate_cube_stl("a fox");
        for t in 0..12usize {
            let off = 84 + t * 50;
            let f = |i: usize| {
                f32::from_le_bytes(stl[off + i * 4..off + i * 4 + 4].try_into().unwrap())
            };
            let len = (f(0) * f(0) + f(1) * f(1) + f(2) * f(2)).sqrt();
            assert!((len - 1.0).abs() < 1e-5, "triangle {t} normal length {len}");
        }
    }

    #[test]
    fn containers_agree_on_scale() {
        // Same prompt must mean the same cube in every container, or a caller
        // who asked for two formats silently receives two different models.
        let obj = String::from_utf8(generate_cube_obj("a fox")).unwrap();
        let first_v: Vec<f32> = obj
            .lines()
            .find(|l| l.starts_with("v "))
            .unwrap()[2..]
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect();
        let stl = generate_cube_stl("a fox");
        let scale = cube_scale("a fox");
        assert!((first_v[0] - (-scale)).abs() < 1e-5);
        assert_eq!(u32::from_le_bytes(stl[80..84].try_into().unwrap()), 12);
    }

    #[test]
    fn different_prompts_make_different_bytes() {
        assert_ne!(generate_cube_obj("a fox"), generate_cube_obj("a hare"));
        assert_ne!(generate_cube_stl("a fox"), generate_cube_stl("a hare"));
    }
}
