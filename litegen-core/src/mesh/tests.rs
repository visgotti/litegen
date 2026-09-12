//! Tests for the contract itself — the IR and the `read`/`write`/`convert`
//! entry points. Per-format parsing lives in each codec's own test module, and
//! the cross-format guarantees live in `matrix_tests`.

use super::*;

fn tri() -> Mesh {
    Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        indices: vec![0, 1, 2],
        ..Default::default()
    }
}

#[test]
fn validate_rejects_a_partial_triangle() {
    let mut m = tri();
    m.indices.pop();
    assert!(matches!(m.validate(), Err(MeshError::Malformed(_))));
}

#[test]
fn validate_rejects_an_index_past_the_end() {
    let mut m = tri();
    m.indices[2] = 99;
    assert!(matches!(m.validate(), Err(MeshError::Malformed(_))));
}

#[test]
fn validate_rejects_attribute_arrays_that_do_not_match_the_positions() {
    let mut m = tri();
    m.normals = Some(vec![[0.0, 0.0, 1.0]; 2]);
    assert!(matches!(m.validate(), Err(MeshError::Malformed(_))));

    let mut m = tri();
    m.uvs = Some(vec![[0.0, 0.0]; 7]);
    assert!(matches!(m.validate(), Err(MeshError::Malformed(_))));
}

#[test]
fn bounds_are_none_for_an_empty_mesh_and_exact_otherwise() {
    assert_eq!(Mesh::default().bounds(), None);
    let (min, max) = tri().bounds().unwrap();
    assert_eq!(min, [0.0, 0.0, 0.0]);
    assert_eq!(max, [1.0, 1.0, 0.0]);
}

#[test]
fn soup_round_trips_an_indexed_mesh_exactly() {
    // The property STL depends on: de-index, then weld back to what we started
    // with. If this drifts, every conversion through STL silently duplicates
    // vertices.
    let m = Mesh {
        positions: vec![
            [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0],
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
        ..Default::default()
    };
    let welded = Mesh::from_soup(&m.to_soup());
    assert_eq!(welded.positions.len(), 4, "the shared corners must weld back");
    assert_eq!(welded.triangle_count(), 2);
    assert_eq!(welded.bounds(), m.bounds());
}

#[test]
fn welding_treats_negative_zero_as_zero() {
    // -0.0 == 0.0 but their bit patterns differ, so a bitwise weld key without
    // the sign fix leaves duplicate vertices behind — and OBJ and ASCII STL
    // both produce -0.0 readily.
    let soup = vec![
        [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        [[-0.0f32, -0.0, -0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
    ];
    let welded = Mesh::from_soup(&soup);
    assert_eq!(welded.positions.len(), 4, "got {:?}", welded.positions);
}

#[test]
fn facet_normal_is_unit_length_and_follows_the_winding() {
    let n = Mesh::facet_normal(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    assert!((n[2] - 1.0).abs() < 1e-6, "{n:?}");
    // Reversing the winding must reverse the normal, or STL exports render
    // inside-out in viewers that trust the stored normal.
    let r = Mesh::facet_normal(&[[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]]);
    assert!((r[2] + 1.0).abs() < 1e-6, "{r:?}");
}

#[test]
fn facet_normal_of_a_degenerate_triangle_is_zero_not_nan() {
    let n = Mesh::facet_normal(&[[1.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 1.0]]);
    assert_eq!(n, [0.0, 0.0, 0.0]);
    assert!(n.iter().all(|c| c.is_finite()));
}

#[test]
fn read_refuses_an_empty_mesh_rather_than_returning_one() {
    // A zero-triangle result is never a useful deliverable, and passing it on
    // turns a vendor failure into a file the caller has to diagnose.
    let empty_obj = b"# nothing here\no lonely\n";
    assert_eq!(read(empty_obj, MeshFormat::Obj), Err(MeshError::Empty));
}

#[test]
fn keeps_uvs_and_keeps_color_describe_the_lossiness_table() {
    assert!(MeshFormat::Glb.keeps_uvs() && MeshFormat::Glb.keeps_color());
    assert!(MeshFormat::Obj.keeps_uvs() && !MeshFormat::Obj.keeps_color());
    assert!(!MeshFormat::Ply.keeps_uvs() && MeshFormat::Ply.keeps_color());
    assert!(!MeshFormat::Stl.keeps_uvs() && !MeshFormat::Stl.keeps_color());
}

#[test]
fn all_lists_every_variant_exactly_once() {
    // `MeshFormat::ALL` drives the conversion matrix, so a variant missing from
    // it is a format with zero test coverage that still looks covered.
    let mut sorted = MeshFormat::ALL.to_vec();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), MeshFormat::ALL.len(), "ALL contains a duplicate");
    for f in MeshFormat::ALL {
        assert_eq!(MeshFormat::parse(f.as_str()), Some(f));
    }
}

#[test]
fn non_finite_positions_are_rejected_at_the_boundary() {
    // Surfaced by a fuzz probe during review: a GLB carrying NaN positions
    // parsed cleanly, and the failure only appeared later when a writer that
    // happened to guard refused it. NaN survives every structural check, makes
    // bounds() meaningless, and turns glTF's REQUIRED position min/max into
    // NaN — which three.js renders as an empty scene rather than reporting.
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let m = Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [bad, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        assert!(matches!(m.validate(), Err(MeshError::Malformed(_))), "{bad} was accepted");
        // And it must be refused on the way IN as well as on the way out, so
        // the error names the vendor's bytes rather than our writer.
        for f in MeshFormat::ALL {
            assert!(write(&m, f).is_err(), "{f} wrote a non-finite mesh");
        }
    }
}
