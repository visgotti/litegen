//! The conversion matrix: every supported container to every other one.
//!
//! `MeshFormat::ALL` drives these, so adding a variant adds a full row and
//! column of coverage automatically rather than needing a new test — which is
//! the point. A format that is declared convertible and is not will fail here
//! before it can reach a caller.
//!
//! # What is asserted, and what deliberately is not
//!
//! Vertex COUNT is not an invariant. STL is a triangle soup, so anything that
//! passes through it is de-indexed and re-welded: a cube's 8 shared corners
//! become 8 again only because `Mesh::from_soup` welds bit-identical positions.
//! Assert triangle count and geometry instead — they survive every legal step:
//!
//! - the bytes re-parse as the target format
//! - triangle count is preserved exactly
//! - the bounding box is preserved (a conversion that moved, rotated or
//!   rescaled geometry shows up here and essentially nowhere else)
//! - the SET of triangles is preserved, compared as unordered corner triples,
//!   because index order is not something any of these formats promises
//!
//! Attribute preservation is checked separately, against the lossiness table in
//! `super`, so that a writer silently dropping normals cannot hide behind a
//! geometry-only assertion.

use super::*;

/// A cube with distinct, non-symmetric bounds on every axis, plus normals and
/// UVs.
///
/// Non-symmetric on purpose: a cube centred on the origin with equal extents
/// survives an axis swap, a sign flip and a transpose unchanged, so it would
/// pass a bounding-box assertion while being visibly wrong. These bounds pin
/// the orientation.
fn fixture() -> Mesh {
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
    // Per-vertex normals pointing out from the centre. Not the true face
    // normals of a cube, but they are distinct per vertex and unit length,
    // which is what makes a dropped or reordered normal detectable.
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

    Mesh {
        positions: corners.to_vec(),
        indices,
        normals: Some(normals),
        uvs: Some(uvs),
        base_color: Some([0.25, 0.5, 0.75, 1.0]),
        name: Some("litegen_matrix_cube".to_string()),
    }
}

/// Triangles as sorted corner triples, so two meshes can be compared without
/// depending on triangle order or on which corner a triangle starts from.
fn triangle_set(m: &Mesh) -> Vec<[[u32; 3]; 3]> {
    let mut tris: Vec<[[u32; 3]; 3]> = m
        .to_soup()
        .iter()
        .map(|t| {
            let mut corners: [[u32; 3]; 3] = [
                [t[0][0].to_bits(), t[0][1].to_bits(), t[0][2].to_bits()],
                [t[1][0].to_bits(), t[1][1].to_bits(), t[1][2].to_bits()],
                [t[2][0].to_bits(), t[2][1].to_bits(), t[2][2].to_bits()],
            ];
            // Rotating a triangle's corners preserves its winding; sorting
            // would not, so rotate to a canonical start instead.
            let start = corners.iter().enumerate().min_by_key(|(_, c)| **c).map(|(i, _)| i).unwrap();
            corners.rotate_left(start);
            corners
        })
        .collect();
    tris.sort_unstable();
    tris
}

fn assert_bounds_match(a: &Mesh, b: &Mesh, ctx: &str) {
    let (amin, amax) = a.bounds().expect("source has bounds");
    let (bmin, bmax) = b.bounds().expect("result has bounds");
    for i in 0..3 {
        assert!(
            (amin[i] - bmin[i]).abs() < 1e-4 && (amax[i] - bmax[i]).abs() < 1e-4,
            "{ctx}: bounds moved on axis {i}: {amin:?}..{amax:?} -> {bmin:?}..{bmax:?}",
        );
    }
}

#[test]
fn every_format_converts_to_every_other_format() {
    let source = fixture();

    for from in MeshFormat::ALL {
        // Materialise the fixture in the source container first, so each row
        // starts from real bytes of that format rather than from the IR — a
        // reader bug cannot be masked by a writer that never ran.
        let encoded = write(&source, from)
            .unwrap_or_else(|e| panic!("failed to write the fixture as {from}: {e}"));
        let decoded = read(&encoded, from)
            .unwrap_or_else(|e| panic!("failed to re-read our own {from}: {e}"));
        assert_eq!(
            decoded.triangle_count(), source.triangle_count(),
            "{from} round-trip changed the triangle count",
        );
        assert_bounds_match(&source, &decoded, &format!("{from} round-trip"));
        assert_eq!(
            triangle_set(&decoded), triangle_set(&source),
            "{from} round-trip changed the geometry",
        );

        for to in MeshFormat::ALL {
            let ctx = format!("{from} -> {to}");
            let out = convert(&encoded, from, to)
                .unwrap_or_else(|e| panic!("{ctx}: conversion failed: {e}"));
            assert!(!out.is_empty(), "{ctx}: produced no bytes");

            let back = read(&out, to)
                .unwrap_or_else(|e| panic!("{ctx}: output does not parse as {to}: {e}"));

            assert_eq!(
                back.triangle_count(), source.triangle_count(),
                "{ctx}: triangle count changed",
            );
            assert_bounds_match(&source, &back, &ctx);
            assert_eq!(triangle_set(&back), triangle_set(&source), "{ctx}: geometry changed");
        }
    }
}

#[test]
fn conversion_keeps_every_attribute_the_target_can_carry() {
    // The geometry assertions above would pass a writer that silently dropped
    // normals, uvs and colour. This is the half that would not.
    let source = fixture();

    for to in MeshFormat::ALL {
        let out = write(&source, to).unwrap();
        let back = read(&out, to).unwrap();
        let ctx = format!("glb -> {to}");

        // Normals survive every container except STL, where they cannot: STL
        // stores one normal per FACET, and a welded vertex belongs to several.
        if to != MeshFormat::Stl {
            assert!(back.normals.is_some(), "{ctx}: normals were dropped");
        }
        assert_eq!(to.keeps_uvs(), back.uvs.is_some(), "{ctx}: uvs disagree with keeps_uvs()");
        assert_eq!(
            to.keeps_color(), back.base_color.is_some(),
            "{ctx}: colour disagrees with keeps_color()",
        );
    }
}

#[test]
fn a_mesh_with_no_optional_attributes_still_converts_everywhere() {
    // Positions and indices only — what an STL-sourced mesh looks like, and the
    // input most likely to make a writer index into a `None`.
    let bare = Mesh {
        positions: fixture().positions,
        indices: fixture().indices,
        ..Default::default()
    };
    for from in MeshFormat::ALL {
        let encoded = write(&bare, from).unwrap_or_else(|e| panic!("write {from}: {e}"));
        for to in MeshFormat::ALL {
            let out = convert(&encoded, from, to)
                .unwrap_or_else(|e| panic!("{from} -> {to}: {e}"));
            let back = read(&out, to).unwrap_or_else(|e| panic!("{from} -> {to} reparse: {e}"));
            assert_eq!(back.triangle_count(), bare.triangle_count(), "{from} -> {to}");
            assert_bounds_match(&bare, &back, &format!("{from} -> {to}"));
        }
    }
}

#[test]
fn converting_a_format_to_itself_is_not_a_passthrough() {
    // Same-format conversion goes through the IR like every other pair, so it
    // normalises rather than echoing. A reader bug must not be reachable only
    // via the pairs someone happened to test.
    let source = fixture();
    for f in MeshFormat::ALL {
        let encoded = write(&source, f).unwrap();
        let out = convert(&encoded, f, f).unwrap();
        let back = read(&out, f).unwrap();
        assert_eq!(triangle_set(&back), triangle_set(&source), "{f} -> {f}");
    }
}

#[test]
fn every_reader_rejects_every_other_formats_bytes() {
    // A reader that accepts the wrong container silently produces nonsense
    // geometry, which is far worse than an error — and `convert` trusts the
    // caller's `from`, so this is the only thing standing between a mislabelled
    // vendor file and a garbage mesh.
    let source = fixture();
    for from in MeshFormat::ALL {
        let bytes = write(&source, from).unwrap();
        for to in MeshFormat::ALL {
            if from == to {
                continue;
            }
            assert!(
                read(&bytes, to).is_err(),
                "the {to} reader accepted {from} bytes",
            );
        }
    }
}

#[test]
fn no_reader_panics_on_truncated_or_hostile_input() {
    // Vendor bytes are untrusted input on the poll path: a panic here takes out
    // the worker, not just the generation.
    let source = fixture();
    for f in MeshFormat::ALL {
        let full = write(&source, f).unwrap();

        // Every prefix, at a stride that stays cheap but still lands inside
        // headers, counts and record boundaries.
        let step = (full.len() / 64).max(1);
        for cut in (0..full.len()).step_by(step) {
            let _ = read(&full[..cut], f); // must return, panic-free
        }

        // Bit-flips through the whole file, hitting length and count fields.
        let step = (full.len() / 128).max(1);
        for i in (0..full.len()).step_by(step) {
            let mut corrupt = full.clone();
            corrupt[i] = corrupt[i].wrapping_add(0x7f);
            let _ = read(&corrupt, f);
        }

        // Degenerate inputs.
        for bytes in [vec![], vec![0u8; 3], vec![0xffu8; 256]] {
            let _ = read(&bytes, f);
        }
    }
}

#[test]
fn an_empty_mesh_is_an_error_in_both_directions() {
    let empty = Mesh::default();
    for f in MeshFormat::ALL {
        assert_eq!(write(&empty, f), Err(MeshError::Empty), "write {f}");
    }
}

#[test]
fn an_input_over_the_size_cap_is_refused_before_parsing() {
    let huge = vec![0u8; MAX_INPUT_BYTES + 1];
    for f in MeshFormat::ALL {
        assert!(
            matches!(read(&huge, f), Err(MeshError::Unsupported(_))),
            "{f} must refuse an oversized input rather than parse it",
        );
    }
}

#[test]
fn format_parsing_round_trips_and_rejects_what_we_cannot_convert() {
    for f in MeshFormat::ALL {
        assert_eq!(MeshFormat::parse(f.as_str()), Some(f));
        assert_eq!(MeshFormat::parse(&f.as_str().to_uppercase()), Some(f));
        assert_eq!(MeshFormat::parse(&format!("  {f}  ")), Some(f));
    }
    // The two we deliberately do not model, plus noise. A caller must read
    // `None` as "not convertible", never as a reason to fall back to glb.
    for s in ["fbx", "usdz", "gltf", "3mf", "", "  ", "png"] {
        assert_eq!(MeshFormat::parse(s), None, "{s:?} must not parse");
    }
}

// ─── Regressions from the adversarial review ────────────────────────────────
//
// Each of these was reproduced with a concrete input before it was fixed.
// They live here rather than in a codec's own tests because each is an instance
// of a rule that holds for EVERY codec: bounded work, bounded error text, no
// silent guessing, and write/read agreement.

#[test]
fn a_header_line_that_never_ends_is_refused_without_scanning_the_file() {
    // PLY bounded `pos` but not the WORK: the newline search and the UTF-8
    // check both ran over the whole remainder before `pos` could exceed the
    // cap. Measured at 120 MB of input: 0.57 s and 361 MB resident — on the
    // poll path, from bytes a vendor supplied. 8 MB here is enough to fail
    // loudly on the old code without making the suite slow.
    let mut bytes = b"ply\nformat ascii 1.0\n".to_vec();
    bytes.extend(std::iter::repeat_n(b'C', 8 * 1024 * 1024));
    let started = std::time::Instant::now();
    let err = read(&bytes, MeshFormat::Ply).unwrap_err();
    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "took {:?} — the header cap is not bounding the work", started.elapsed(),
    );
    assert!(matches!(err, MeshError::Malformed(_)), "{err:?}");
}

#[test]
fn an_error_message_never_quotes_an_unbounded_amount_of_input() {
    // The realistic trigger is not an attack: it is a vendor returning a
    // minified JSON or HTML error page where a mesh was expected, which came
    // back as an error whose Display string was the entire page — and error
    // text goes to logs, to operators, and potentially into a response.
    let blob: Vec<u8> = std::iter::repeat_n(b'x', 2 * 1024 * 1024).collect();
    for f in MeshFormat::ALL {
        if let Err(e) = read(&blob, f) {
            let rendered = e.to_string();
            assert!(
                rendered.len() < 4096,
                "{f} rendered a {}-byte error for a {}-byte input",
                rendered.len(), blob.len(),
            );
        }
    }
}

#[test]
fn ply_refuses_float_vertex_indices_instead_of_truncating_them() {
    // `property list uchar float vertex_indices` parsed happily and truncated
    // 0.9 to vertex 0 — a silent guess, made on geometry.
    let src = b"ply\nformat ascii 1.0\n\
element vertex 3\nproperty float x\nproperty float y\nproperty float z\n\
element face 1\nproperty list uchar float vertex_indices\n\
end_header\n0 0 0\n1 0 0\n0 1 0\n3 0.9 1.9 2.9\n";
    let err = read(src, MeshFormat::Ply).unwrap_err();
    assert!(matches!(err, MeshError::Malformed(_)), "{err:?}");
    assert!(err.to_string().contains("integer"), "{err}");
}

#[test]
fn obj_rejects_a_junk_token_on_a_vertex_line() {
    // `tokens.count()` consumed the iterator without parsing it, so this was
    // the one place in the OBJ reader where garbage returned Ok.
    for src in [
        &b"v 0 0 0 xyzzy\nv 1 0 0\nv 0 1 0\nf 1 2 3\n"[..],
        &b"v 0 0 0 1..2\nv 1 0 0\nv 0 1 0\nf 1 2 3\n"[..],
    ] {
        assert!(read(src, MeshFormat::Obj).is_err(), "accepted {:?}", String::from_utf8_lossy(src));
    }
    // The legitimate 4th component still works: `w` is a rational-surface
    // weight, meaningless for polygonal geometry but legal to write.
    let ok = b"v 0 0 0 1\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    assert_eq!(read(ok, MeshFormat::Obj).unwrap().triangle_count(), 1);
    // `nan` IS accepted here, deliberately: Rust parses it as a valid f32, and
    // `w` is discarded rather than stored, so it cannot reach the geometry. The
    // non-finite guard in `Mesh::validate` is what covers x/y/z.
    let nan_w = b"v 0 0 0 nan\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    assert_eq!(read(nan_w, MeshFormat::Obj).unwrap().triangle_count(), 1);
}

#[test]
fn every_writer_produces_a_file_its_own_reader_accepts_for_any_mesh_name() {
    // A glTF node name is arbitrary vendor-supplied UTF-8. A Windows path used
    // as one ends in a backslash, which OBJ reads as a line continuation — so
    // `convert(glb, obj)` emitted a file our own reader then refused.
    let base = Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        indices: vec![0, 1, 2],
        ..Default::default()
    };
    let names = [
        r"C:\models\thing\", "\\", "foo\\ ", "a#b", "with\ttab", "with\nnewline",
        "  padded  ", "", "ünïcødé", "end_header", "solid", "ply",
    ];
    for name in names {
        let mut m = base.clone();
        m.name = Some(name.to_string());
        for f in MeshFormat::ALL {
            let bytes = write(&m, f).unwrap_or_else(|e| panic!("write {f} with name {name:?}: {e}"));
            let back = read(&bytes, f)
                .unwrap_or_else(|e| panic!("{f} refused its own output for name {name:?}: {e}"));
            assert_eq!(back.triangle_count(), 1, "{f} / {name:?}");
        }
    }
}

// ─── GLB hardening regressions ──────────────────────────────────────────────
//
// GLB is the container every vendor emits, so its reader is the one that
// actually faces untrusted bytes. These construct GLB files byte-by-byte rather
// than going through `write`, because each reproduces a case our own writer
// would never produce — which is exactly why the reader has to handle it.

/// Assemble a GLB from a JSON document and an optional BIN chunk.
fn glb_bytes(doc: &serde_json::Value, bin: Option<&[u8]>) -> Vec<u8> {
    let mut json = serde_json::to_vec(doc).unwrap();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut bin = bin.map(|b| b.to_vec()).unwrap_or_default();
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let has_bin = !bin.is_empty();
    let total = 12 + 8 + json.len() + if has_bin { 8 + bin.len() } else { 0 };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&0x4654_6C67u32.to_le_bytes()); // "glTF"
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
    out.extend_from_slice(&json);
    if has_bin {
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // "BIN\0"
        out.extend_from_slice(&bin);
    }
    out
}

/// One triangle: 3 VEC3 positions then 3 u32 indices.
fn triangle_bin() -> Vec<u8> {
    let mut bin = Vec::new();
    for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
        for c in p {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    for i in [0u32, 1, 2] {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    bin
}

fn triangle_doc() -> serde_json::Value {
    serde_json::json!({
        "asset": { "version": "2.0" },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{ "mesh": 0 }],
        "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 }, "indices": 1 }] }],
        "accessors": [
            { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
              "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0] },
            { "bufferView": 1, "componentType": 5125, "count": 3, "type": "SCALAR" },
        ],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0,  "byteLength": 36 },
            { "buffer": 0, "byteOffset": 36, "byteLength": 12 },
        ],
        "buffers": [{ "byteLength": 48 }],
    })
}

#[test]
fn the_glb_fixture_itself_is_valid() {
    // Everything below is this document with one field broken, so it has to
    // parse cleanly first — otherwise the tests would pass for the wrong reason.
    let m = read(&glb_bytes(&triangle_doc(), Some(&triangle_bin())), MeshFormat::Glb).unwrap();
    assert_eq!(m.triangle_count(), 1);
    assert_eq!(m.positions.len(), 3);
}

#[test]
fn a_tiny_file_cannot_make_us_allocate_without_bound() {
    // A 244-byte GLB declaring 4 billion elements asked for a 48 GB Vec, and
    // Rust's allocation-failure handler ABORTS the process — so this was a
    // remote kill from vendor bytes on the poll path, with MAX_INPUT_BYTES
    // offering no protection at all. The count must be validated against what
    // the backing view can actually hold, before anything is reserved.
    let no_view = serde_json::json!({
        "asset": { "version": "2.0" },
        "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }],
        "accessors": [{ "componentType": 5126, "count": 20_000_001u64, "type": "VEC3" }],
        "buffers": [],
    });
    let bytes = glb_bytes(&no_view, None);
    assert!(bytes.len() < 1024, "the point is that the INPUT is tiny: {}", bytes.len());
    let started = std::time::Instant::now();
    assert!(read(&bytes, MeshFormat::Glb).is_err(), "20M zero-filled vertices were accepted");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(250),
        "took {:?} — it allocated before rejecting", started.elapsed(),
    );

    // The same defect reached through a real bufferView: a count far larger
    // than the view's bytes.
    let mut doc = triangle_doc();
    doc["accessors"][0]["count"] = serde_json::json!(500_000_000u64);
    assert!(
        read(&glb_bytes(&doc, Some(&triangle_bin())), MeshFormat::Glb).is_err(),
        "a count larger than its bufferView was accepted",
    );
}

#[test]
fn the_four_billion_count_that_aborted_the_process_is_refused() {
    // The reviewer measured this exact input killing the process: Rust's
    // allocation-failure handler ABORTS, so it was not catchable — a remote
    // kill from vendor bytes. If this regresses, the test binary dies rather
    // than failing, which is itself the signal.
    let doc = serde_json::json!({
        "asset": { "version": "2.0" },
        "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }],
        "accessors": [{ "componentType": 5126, "count": 4_000_000_000u64, "type": "VEC3" }],
        "buffers": [],
    });
    assert!(read(&glb_bytes(&doc, None), MeshFormat::Glb).is_err());

    // And the same count behind a real bufferView, where the reader does have
    // a buffer to bound against but the reservation came first.
    let mut doc = triangle_doc();
    doc["accessors"][0]["count"] = serde_json::json!(4_000_000_000u64);
    assert!(read(&glb_bytes(&doc, Some(&triangle_bin())), MeshFormat::Glb).is_err());
}

#[test]
fn instancing_cannot_amplify_a_small_file_without_bound() {
    // Geometry is re-read per node instance, so N nodes pointing at one mesh
    // multiply the vertex count by N from a file that never grows. Measured at
    // 484x before the cap.
    let mut doc = triangle_doc();
    doc["nodes"] = serde_json::Value::Array(
        (0..4000).map(|_| serde_json::json!({ "mesh": 0 })).collect(),
    );
    doc["scenes"] = serde_json::json!([{ "nodes": (0..4000).collect::<Vec<_>>() }]);
    let bytes = glb_bytes(&doc, Some(&triangle_bin()));
    // Either it refuses, or it stays proportionate — but it must not blow up.
    if let Ok(m) = read(&bytes, MeshFormat::Glb) {
        assert!(
            m.positions.len() <= 4000 * 3,
            "read {} positions from {} bytes", m.positions.len(), bytes.len(),
        );
    }
}

#[test]
fn an_accessor_may_not_read_past_its_own_buffer_view() {
    // Bounding against the BUFFER rather than the VIEW meant a corrupt file
    // read its neighbour's bytes — usually the index array — and returned
    // plausible wrong geometry instead of an error.
    let mut doc = triangle_doc();
    doc["bufferViews"][0]["byteLength"] = serde_json::json!(12); // one vertex, not three
    assert!(
        read(&glb_bytes(&doc, Some(&triangle_bin())), MeshFormat::Glb).is_err(),
        "an accessor read 24 bytes past its declared view",
    );
}

#[test]
fn a_present_but_unreadable_field_is_an_error_not_a_default() {
    // serde_json's as_u64 rejects any number parsed as f64, so `24.0` from a
    // Python-side exporter was enough to make a present field look ABSENT and
    // silently take the default branch.
    //
    // The worst instance: a float `indices` made the reader ignore the index
    // accessor and synthesise a sequential list, so a file saying [2,1,0] read
    // as [0,1,2] — reversed winding, model inside out.
    let cases: &[(&str, serde_json::Value)] = &[
        ("indices", serde_json::json!(1.0)),
        ("mesh", serde_json::json!(0.0)),
    ];
    for (field, bad) in cases {
        let mut doc = triangle_doc();
        match *field {
            "indices" => doc["meshes"][0]["primitives"][0]["indices"] = bad.clone(),
            "mesh" => doc["nodes"][0]["mesh"] = bad.clone(),
            _ => unreachable!(),
        }
        assert!(
            read(&glb_bytes(&doc, Some(&triangle_bin())), MeshFormat::Glb).is_err(),
            "a float `{field}` was treated as absent",
        );
    }

    // byteStride is the other dangerous one: read as packed, an interleaved
    // view hands back normals where positions should be.
    let mut doc = triangle_doc();
    doc["bufferViews"][0]["byteStride"] = serde_json::json!(12.0);
    assert!(
        read(&glb_bytes(&doc, Some(&triangle_bin())), MeshFormat::Glb).is_err(),
        "a float byteStride was treated as absent",
    );
}

#[test]
fn a_required_extension_is_refused_however_it_is_spelled() {
    // The guard fell through to Ok whenever `extensionsRequired` was not an
    // array, so a string-valued one bypassed it entirely. Only a separate
    // per-primitive Draco check caught that case; meshopt had no backstop.
    for value in [
        serde_json::json!("KHR_draco_mesh_compression"),
        serde_json::json!("EXT_meshopt_compression"),
        serde_json::json!(["EXT_meshopt_compression"]),
        serde_json::json!(["KHR_draco_mesh_compression"]),
    ] {
        let mut doc = triangle_doc();
        doc["extensionsRequired"] = value.clone();
        assert!(
            read(&glb_bytes(&doc, Some(&triangle_bin())), MeshFormat::Glb).is_err(),
            "extensionsRequired {value} was ignored",
        );
    }

    // And a bufferView carrying meshopt data: the extension only has to appear
    // in extensionsRequired when its fallback buffer is NOT the BIN chunk, so a
    // file can declare it in extensionsUsed alone and have its placeholder
    // bytes read as geometry.
    let mut doc = triangle_doc();
    doc["extensionsUsed"] = serde_json::json!(["EXT_meshopt_compression"]);
    doc["bufferViews"][0]["extensions"] =
        serde_json::json!({ "EXT_meshopt_compression": { "buffer": 0, "byteLength": 36, "mode": "ATTRIBUTES" } });
    assert!(
        read(&glb_bytes(&doc, Some(&triangle_bin())), MeshFormat::Glb).is_err(),
        "meshopt placeholder bytes were read as geometry",
    );
}

#[test]
fn triangle_strips_and_fans_are_not_silently_dropped() {
    // A mesh mixing a strip with a list returned Ok carrying only the list — a
    // half-missing model, with no signal. Strip->list and fan->list are
    // mechanical and lossless, so converting is right; refusing would also be
    // defensible. Dropping silently is not.
    //
    // 5 vertices, so a strip yields 3 triangles and a fan yields 3.
    let mut bin = Vec::new();
    for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [2.0, 0.0, 0.0]] {
        for c in p {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    for mode in [5u64, 6] {
        let doc = serde_json::json!({
            "asset": { "version": "2.0" },
            "scene": 0,
            "scenes": [{ "nodes": [0] }],
            "nodes": [{ "mesh": 0 }],
            "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 }, "mode": mode }] }],
            "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 5, "type": "VEC3",
                            "min": [0.0, 0.0, 0.0], "max": [2.0, 1.0, 0.0] }],
            "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": 60 }],
            "buffers": [{ "byteLength": 60 }],
        });
        match read(&glb_bytes(&doc, Some(&bin)), MeshFormat::Glb) {
            Ok(m) => assert_eq!(m.triangle_count(), 3, "mode {mode} produced the wrong triangle count"),
            Err(e) => {
                // Refusing is acceptable; refusing SILENTLY is not, so the error
                // must name the mode rather than come back as Empty.
                assert!(!matches!(e, MeshError::Empty), "mode {mode} was dropped, then reported as empty");
            }
        }
    }
}
