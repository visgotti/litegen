//! Tests for the contract itself — the IR and the `read`/`write`/`convert`
//! entry points. Per-format parsing lives in each codec's own test module, and
//! the cross-format guarantees live in `matrix_tests`.

use super::{*, scene::TextureRef};

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
    // A single-file OBJ carries colour as per-vertex colour (`v x y z r g b`).
    assert!(MeshFormat::Obj.keeps_uvs() && MeshFormat::Obj.keeps_color());
    assert!(MeshFormat::Ply.keeps_uvs() && MeshFormat::Ply.keeps_color());
    assert!(!MeshFormat::Stl.keeps_uvs() && !MeshFormat::Stl.keeps_color());
    // 3MF keeps texture coordinates only beside a texture, which the flat mesh
    // cannot carry; its colour survives as a base material.
    assert!(!MeshFormat::ThreeMf.keeps_uvs() && MeshFormat::ThreeMf.keeps_color());
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

/// `nodes` triangles, each translated by its own two-key LINEAR channel from
/// 0 s to `seconds`: the writers that resample (FBX, Collada, USD) key every
/// animated node at 30 fps across the whole span.
fn long_clip(nodes: usize, seconds: f32) -> Scene {
    use super::scene::{AnimPath, Animation, Channel, Interpolation, Node, Primitive, SceneMesh};
    let mut s = Scene::default();
    s.meshes.push(SceneMesh {
        primitives: vec![Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut channels = Vec::new();
    for n in 0..nodes {
        s.add_root(Node { name: Some(format!("n{n}")), mesh: Some(0), ..Default::default() });
        channels.push(Channel {
            node: n,
            path: AnimPath::Translation,
            interpolation: Interpolation::Linear,
            times: vec![0.0, seconds],
            values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        });
    }
    s.animations.push(Animation { name: Some("long".into()), channels });
    s
}

/// 2026-10-12 review, finding 3: each resampling writer multiplied a grid of
/// up to 100,001 times by every animated node with no budget — 21 nodes over
/// 3,400 s is 33.6M baked matrix values (269 MB of f64 before Collada's text),
/// and 1,000 such nodes asked for 35 GB. Refused up front, before any sampling.
#[test]
fn resampling_writers_refuse_animation_past_the_value_budget_before_sampling() {
    let big = long_clip(21, 3400.0);
    for to in [MeshFormat::Dae, MeshFormat::Fbx] {
        let started = std::time::Instant::now();
        let e = write_scene(&big, to, WriteOptions::default()).map(|_| ()).unwrap_err();
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("resample")), "{to}: {e:?}");
        assert!(started.elapsed().as_secs() < 5, "{to}: refused only after {:?}", started.elapsed());
    }
    // glTF and USD keep LINEAR keys as authored: 42 keys, nothing to budget.
    for to in [MeshFormat::Glb, MeshFormat::Usdz, MeshFormat::Usda, MeshFormat::Usdc] {
        assert!(write_scene(&big, to, WriteOptions::default()).is_ok(), "{to}");
    }
    // What USD does resample — cubic channels — is budgeted where it is.
    let mut cubic = long_clip(120, 3400.0);
    for ch in cubic.animations[0].channels.iter_mut() {
        ch.interpolation = super::scene::Interpolation::CubicSpline;
        // [in, value, out] per key: from the origin to x = 1.
        ch.values = vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    }
    for to in [MeshFormat::Usdz, MeshFormat::Usda, MeshFormat::Usdc] {
        let e = write_scene(&cubic, to, WriteOptions::default()).map(|_| ()).unwrap_err();
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("resample")), "{to}: {e:?}");
    }
    let small = long_clip(2, 10.0);
    for to in [MeshFormat::Dae, MeshFormat::Fbx, MeshFormat::Usdz, MeshFormat::Usda, MeshFormat::Usdc] {
        assert!(write_scene(&small, to, WriteOptions::default()).is_ok(), "{to}");
    }
}

/// One 99,999-vertex mesh with a morph target, drawn by `nodes` nodes with
/// distinct weights: valid (the mesh is stored once) at 0.1M vertices.
fn instanced_morph(nodes: usize) -> Scene {
    use super::scene::{MorphTarget, Node, Primitive, SceneMesh};
    let n = 99_999usize;
    let positions: Vec<[f32; 3]> = (0..n).map(|i| [(i % 1000) as f32, (i / 1000) as f32, 0.0]).collect();
    let indices: Vec<u32> = (0..n as u32 / 3 * 3).collect();
    let mut s = Scene::default();
    s.meshes.push(SceneMesh {
        primitives: vec![Primitive {
            targets: vec![MorphTarget { positions: vec![[0.0, 0.0, 1.0]; n], normals: None }],
            positions,
            indices,
            ..Default::default()
        }],
        weights: vec![0.0],
        ..Default::default()
    });
    for k in 0..nodes {
        s.add_root(Node { name: Some(format!("i{k}")), mesh: Some(0), weights: Some(vec![k as f32 / nodes as f32]), ..Default::default() });
    }
    s
}

/// 2026-10-12 review, finding 4: only `pose::world_primitives` checked the
/// merged-geometry ceiling before copying an instance. USD writes every
/// instance's mesh inline; FBX gives each morphed or skinned node its own
/// geometry; Collada and 3MF bake morphs per instance — so 1.2 MB of input
/// became a 114 MB USDZ, and 100k instances asked for 50–145 GB. Each is now
/// counted as the writer copies it, before anything is copied.
#[test]
fn writers_that_copy_instances_refuse_geometry_past_the_ceiling_before_copying() {
    let big = instanced_morph(45);
    big.validate().unwrap();
    for to in [MeshFormat::Usdz, MeshFormat::Usda, MeshFormat::Usdc, MeshFormat::Fbx, MeshFormat::Dae, MeshFormat::ThreeMf, MeshFormat::Obj, MeshFormat::Stl, MeshFormat::Ply] {
        let started = std::time::Instant::now();
        let e = write_scene(&big, to, WriteOptions::default()).map(|_| ()).unwrap_err();
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains(&super::scene::MAX_SCENE_VERTICES.to_string())), "{to}: {e:?}");
        assert!(started.elapsed().as_secs() < 5, "{to}: refused only after {:?}", started.elapsed());
    }
    // glTF references the one mesh from every node.
    assert!(write_scene(&big, MeshFormat::Glb, WriteOptions::default()).is_ok());
    // Under the ceiling, every container still writes it.
    let small = instanced_morph(3);
    for to in MeshFormat::ALL {
        assert!(write_scene(&small, to, WriteOptions::default()).is_ok(), "{to}");
    }
}

/// A triangle skinned to `joints` joints under a chain of `nulls` non-joint
/// nodes; the joints form a chain when `chain`, else all hang off the last
/// null.
fn deep_rig(nulls: usize, joints: usize, chain: bool) -> Scene {
    use super::scene::{Node, Primitive, SceneMesh, Skin, SkinWeights};
    let mut s = Scene::default();
    s.meshes.push(SceneMesh {
        primitives: vec![Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            skin: Some(SkinWeights { influences: 4, joints: vec![0; 12], weights: [1.0, 0.0, 0.0, 0.0].repeat(3) }),
            ..Default::default()
        }],
        ..Default::default()
    });
    let attach = |s: &mut Scene, parent: Option<usize>, node: Node| -> usize {
        s.nodes.push(node);
        let n = s.nodes.len() - 1;
        match parent {
            Some(p) => s.nodes[p].children.push(n),
            None => s.roots.push(n),
        }
        n
    };
    let mut top = None;
    for i in 0..nulls {
        top = Some(attach(&mut s, top, Node { name: Some(format!("null{i}")), ..Default::default() }));
    }
    let mut ids = Vec::new();
    let mut under = top;
    for j in 0..joints {
        let n = attach(&mut s, under, Node { name: Some(format!("j{j}")), ..Default::default() });
        if chain {
            under = Some(n);
        }
        ids.push(n);
    }
    s.skins.push(Skin { name: None, inverse_bind_matrices: vec![super::math::IDENTITY; ids.len()], joints: ids, skeleton: None });
    attach(&mut s, None, Node { name: Some("body".into()), mesh: Some(0), skin: Some(0), ..Default::default() });
    s
}

/// 2026-10-12 review, findings 15 and 17: the FBX writer listed every
/// non-joint ancestor of every joint in the bind pose before deduplicating,
/// testing joint membership by scanning the joint list — 2,000 joints under
/// 2,000 nulls reached 566 MB, and the scan made it quadratic in time too.
#[test]
fn the_fbx_bind_pose_lists_each_ancestor_once_without_scanning() {
    let rig = deep_rig(1000, 1000, false);
    let started = std::time::Instant::now();
    let w = write_scene(&rig, MeshFormat::Fbx, WriteOptions::default()).unwrap();
    assert!(started.elapsed().as_millis() < 2000, "{:?}", started.elapsed());
    let back = read_scene(&w.package.files[0].bytes, MeshFormat::Fbx, &NoSidecars).unwrap().scene;
    assert_eq!(back.skins[0].joints.len(), 1000);
}

/// Finding 15: UsdSkel names every joint by its path from the skeleton
/// root, so a chain's joint tokens grow with its depth squared (an 8k-joint
/// chain, 1 MB as GLB, became a 329 MB USDA). Refused past a budget.
#[test]
fn usd_joint_paths_are_budgeted() {
    let e = write_scene(&deep_rig(0, 3000, true), MeshFormat::Usda, WriteOptions::default()).map(|_| ()).unwrap_err();
    assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("joint paths")), "{e:?}");
    assert!(write_scene(&deep_rig(0, 100, true), MeshFormat::Usdz, WriteOptions::default()).is_ok());
}

/// 2026-10-12 review, finding 17: every writer's name uniquifier counted its
/// suffixes up from 1 for each repeat of a name (OBJ's over a list, so
/// cubic): 8,000 same-named nodes took 179 s to write as OBJ, and 100k
/// minutes as FBX, Collada or USDA. Delivery's `unique_node_names` scanned
/// every node per candidate too.
#[test]
fn same_named_nodes_are_named_apart_in_linear_time() {
    use super::scene::{Node, Primitive, SceneMesh};
    let mut s = Scene::default();
    s.meshes.push(SceneMesh {
        primitives: vec![Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        }],
        ..Default::default()
    });
    for _ in 0..2000 {
        s.add_root(Node { name: Some("part".into()), mesh: Some(0), ..Default::default() });
    }
    for to in [MeshFormat::Obj, MeshFormat::Fbx, MeshFormat::Dae, MeshFormat::Usda] {
        let started = std::time::Instant::now();
        write_scene(&s, to, WriteOptions::default()).unwrap();
        assert!(started.elapsed().as_millis() < 2000, "{to}: {:?}", started.elapsed());
    }
    let mut named = s.clone();
    let started = std::time::Instant::now();
    assert!(super::normalize::unique_node_names(&mut named));
    assert!(started.elapsed().as_millis() < 1000, "{:?}", started.elapsed());
    let distinct: std::collections::HashSet<&str> = named.nodes.iter().filter_map(|n| n.name.as_deref()).collect();
    assert_eq!(distinct.len(), 2000);
}

/// 2026-10-12 review, finding 17: `Scene::validate` checked for cycles by
/// walking every node up to its root — quadratic in a deep chain, and it
/// runs two to four times per conversion (a 99,999-deep chain took 23–38 s
/// to convert).
#[test]
fn a_deep_chain_validates_and_round_trips_in_linear_time() {
    use super::scene::{Node, Primitive, SceneMesh};
    let mut s = Scene::default();
    s.meshes.push(SceneMesh {
        primitives: vec![Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        }],
        ..Default::default()
    });
    let depth = 50_000;
    for i in 0..depth {
        s.nodes.push(Node { name: Some(format!("n{i}")), children: if i + 1 < depth { vec![i + 1] } else { vec![] }, mesh: (i + 1 == depth).then_some(0), ..Default::default() });
    }
    s.roots = vec![0];
    let started = std::time::Instant::now();
    s.validate().unwrap();
    assert!(started.elapsed().as_millis() < 1000, "validate: {:?}", started.elapsed());
    let started = std::time::Instant::now();
    let glb = write_scene(&s, MeshFormat::Glb, WriteOptions::default()).unwrap();
    let back = read_scene(&glb.package.files[0].bytes, MeshFormat::Glb, &NoSidecars).unwrap().scene;
    assert_eq!(back.nodes.len(), depth);
    assert!(started.elapsed().as_secs() < 5, "glb round trip: {:?}", started.elapsed());
    // A cycle is still found.
    let mut cyclic = s.clone();
    cyclic.nodes[depth - 1].children = vec![0];
    assert!(cyclic.validate().is_err());
}

/// 2026-10-12 review, finding 17: the glTF writer shared an animation input
/// accessor between channels by scanning every earlier one — quadratic in the
/// channels whose key times differ.
#[test]
fn many_distinct_key_time_arrays_write_in_linear_time() {
    use super::scene::{AnimPath, Animation, Channel, Interpolation, Node, Primitive, SceneMesh};
    let mut s = Scene::default();
    s.meshes.push(SceneMesh {
        primitives: vec![Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut channels = Vec::new();
    for i in 0..30_000 {
        let n = s.add_root(Node { name: Some(format!("n{i}")), mesh: (i == 0).then_some(0), ..Default::default() });
        channels.push(Channel {
            node: n,
            path: AnimPath::Translation,
            interpolation: Interpolation::Linear,
            times: vec![0.0, 1.0 + i as f32 * 1e-3],
            values: vec![0.0; 6],
        });
    }
    s.animations.push(Animation { name: Some("many".into()), channels });
    let started = std::time::Instant::now();
    write_scene(&s, MeshFormat::Glb, WriteOptions::default()).unwrap();
    assert!(started.elapsed().as_millis() < 2000, "{:?}", started.elapsed());
}

/// Each drawn triangle's signed area in the XY plane, at rest.
fn signed_areas_xy(s: &Scene) -> Vec<f32> {
    let mut out = Vec::new();
    for w in super::pose::world_primitives(s, &super::pose::default_pose(s)).unwrap() {
        for t in w.indices.chunks_exact(3) {
            let (a, b, c) = (w.positions[t[0] as usize], w.positions[t[1] as usize], w.positions[t[2] as usize]);
            out.push(((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) / 2.0);
        }
    }
    out
}

/// 2026-10-12 review, finding 20: OBJ, PLY, USD and FBX fanned every
/// polygon from its first corner, so the concave pentagon
/// (0,0)(4,0)(4,2)(2,0.5)(0,2), area 5, read as triangles totalling 7 with
/// one facing backwards; Collada ear-clipped but counted a repeated corner
/// as inside every ear, so a C-shape with one filled its notch. Every reader
/// now ear-clips (ufbx, three.js's FBX and USD loaders and Collada's spec
/// all describe the polygon, not a fan of it).
#[test]
fn concave_polygons_keep_their_shape_in_every_reader() {
    let pentagon = [[0.0, 0.0], [4.0, 0.0], [4.0, 2.0], [2.0, 0.5], [0.0, 2.0]];
    let c_shape = [[0.0, 0.0], [3.0, 0.0], [3.0, 1.0], [1.0, 1.0], [1.0, 1.0], [1.0, 2.0], [3.0, 2.0], [3.0, 3.0], [0.0, 3.0]];
    for (poly, area) in [(&pentagon[..], 5.0f32), (&c_shape[..], 7.0)] {
        let n = poly.len();
        let xyz: Vec<[f32; 3]> = poly.iter().map(|p| [p[0], p[1], 0.0]).collect();
        let list = |sep: &str| xyz.iter().map(|p| format!("{}{sep}{}{sep}{}", p[0], p[1], p[2])).collect::<Vec<_>>();
        let idx = |base: usize| (0..n).map(|i| (i + base).to_string()).collect::<Vec<_>>();
        let obj = format!("{}f {}\n", list(" ").iter().map(|v| format!("v {v}\n")).collect::<String>(), idx(1).join(" "));
        let ply = format!(
            "ply\nformat ascii 1.0\nelement vertex {n}\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n{}{n} {}\n",
            list(" ").iter().map(|v| format!("{v}\n")).collect::<String>(),
            idx(0).join(" ")
        );
        let usda = format!(
            "#usda 1.0\n(\n    defaultPrim = \"M\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n)\ndef Mesh \"M\" {{\n    int[] faceVertexCounts = [{n}]\n    int[] faceVertexIndices = [{}]\n    point3f[] points = [{}]\n}}\n",
            idx(0).join(", "),
            list(", ").iter().map(|v| format!("({v})")).collect::<Vec<_>>().join(", ")
        );
        let mut pvi = idx(0);
        pvi[n - 1] = format!("-{n}");
        let fbx = format!(
            "; FBX 7.4.0 project file\nFBXHeaderExtension:  {{\n\tFBXVersion: 7400\n}}\nGlobalSettings:  {{\n\tProperties70:  {{\n\t\tP: \"UnitScaleFactor\", \"double\", \"Number\", \"\",100\n\t}}\n}}\nObjects:  {{\n\tModel: 10, \"Model::m\", \"Mesh\" {{\n\t}}\n\tGeometry: 99, \"Geometry::p\", \"Mesh\" {{\n\t\tVertices: *{} {{\n\t\t\ta: {}\n\t\t}}\n\t\tPolygonVertexIndex: *{n} {{\n\t\t\ta: {}\n\t\t}}\n\t}}\n}}\nConnections:  {{\n\tC: \"OO\",10,0\n\tC: \"OO\",99,10\n}}\n",
            3 * n,
            list(",").join(","),
            pvi.join(",")
        );
        let pos = list(" ").join(" ");
        let dae = format!(
            r##"<?xml version="1.0" encoding="utf-8"?><COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1"><asset><created>x</created><modified>x</modified></asset><library_geometries><geometry id="g"><mesh><source id="g-p"><float_array id="g-pa" count="{}">{pos}</float_array><technique_common><accessor source="#g-pa" count="{n}" stride="3"><param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/></accessor></technique_common></source><vertices id="g-v"><input semantic="POSITION" source="#g-p"/></vertices><polylist count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><vcount>{n}</vcount><p>{}</p></polylist></mesh></geometry></library_geometries><library_visual_scenes><visual_scene id="vs"><node id="n"><instance_geometry url="#g"/></node></visual_scene></library_visual_scenes><scene><instance_visual_scene url="#vs"/></scene></COLLADA>"##,
            3 * n,
            idx(0).join(" ")
        );
        for (format, bytes) in [(MeshFormat::Obj, obj), (MeshFormat::Ply, ply), (MeshFormat::Usda, usda), (MeshFormat::Fbx, fbx), (MeshFormat::Dae, dae)] {
            let s = read_scene(bytes.as_bytes(), format, &NoSidecars).unwrap_or_else(|e| panic!("{format}: {e}")).scene;
            let areas = signed_areas_xy(&s);
            let total: f32 = areas.iter().sum();
            assert!(areas.iter().all(|a| *a >= -1e-6), "{format} ({n}-gon): a triangle faces backwards: {areas:?}");
            assert!((total - area).abs() < 1e-4, "{format} ({n}-gon): area {total}, not {area}: {areas:?}");
        }
    }
}

/// 2026-10-12 review, finding 22: a container without metallic-roughness
/// maps (FBX, Collada, OBJ's MTL) dropped the map and kept glTF's factors,
/// which are the map's multipliers — the defaults 1 and 1 then read as fully
/// metallic and fully rough (Meshy's GLB to FBX: metallic 1, where the map's
/// mean is 0.29 and Meshy's own FBX says 0). The map's mean is folded in:
/// glTF 2.0 §3.9.2 takes metalness from B and roughness from G.
#[test]
fn a_dropped_metallic_roughness_map_folds_into_its_factors() {
    use super::scene::{Image, ImageFormat, Material, Texture, TextureRef};
    let mut s = super::fixtures::cube();
    // A uniform map: metalness (B) 0, roughness (G) 128/255.
    let mr = super::fixtures::encode_png(2, 2, &[0, 128, 0, 255].repeat(4));
    s.images.push(Image { name: Some("mr".into()), format: ImageFormat::Png, data: mr });
    s.textures.push(Texture { name: None, image: s.images.len() - 1, sampler: Default::default() });
    let mr_tex = s.textures.len() - 1;
    s.materials = vec![Material {
        metallic: 1.0,
        roughness: 1.0,
        metallic_roughness_texture: Some(TextureRef { texture: mr_tex, uv_set: 0, transform: None }),
        ..Default::default()
    }];
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
        p.material = Some(0);
        if p.uvs.is_empty() {
            p.uvs = vec![vec![[0.0, 0.0]; p.positions.len()]];
        }
    }
    let prepared = MeshFormat::Fbx.prepare(&s, WriteOptions::default()).unwrap();
    let m = &prepared.materials[0];
    assert!(m.metallic_roughness_texture.is_none());
    assert!(m.metallic.abs() < 1e-6, "metallic {} (the map says 0)", m.metallic);
    assert!((m.roughness - 128.0 / 255.0).abs() < 1e-3, "roughness {}", m.roughness);
    let fbx = write_scene(&s, MeshFormat::Fbx, WriteOptions::default()).unwrap();
    let back = read_scene(&fbx.package.files[0].bytes, MeshFormat::Fbx, &NoSidecars).unwrap().scene;
    assert!(back.materials[0].metallic.abs() < 1e-3, "{:?}", back.materials[0]);
}

/// 2026-10-12 review, finding 27: what the glTF writer must never emit, and
/// shapes it must keep. glTF 2.0: animation input "time[0] >= 0"; a mesh's
/// `primitives` has at least one item; "When matrix is defined, it MUST be
/// decomposable to TRS properties"; and a skinned node's own transform is
/// ignored only for its skinned primitives.
#[test]
fn the_gltf_writer_emits_only_valid_documents_and_keeps_rigid_parts_in_place() {
    use super::scene::{Node, SceneMesh, Transform};
    // Negative key times (an FBX take with pre-roll) are shifted to start at 0.
    let mut s = super::fixtures::skinned();
    for ch in s.animations[0].channels.iter_mut() {
        for t in ch.times.iter_mut() {
            *t -= 1.0;
        }
    }
    let doc = super::glb::json_chunk(&write_scene(&s, MeshFormat::Glb, WriteOptions::default()).unwrap().package.files[0].bytes);
    for sampler in doc["animations"][0]["samplers"].as_array().unwrap() {
        let input = sampler["input"].as_u64().unwrap() as usize;
        assert!(doc["accessors"][input]["min"][0].as_f64().unwrap() >= 0.0, "{}", doc["accessors"][input]);
    }

    // A mesh left with no primitives is not written as `primitives: []`.
    let mut s = super::fixtures::cube();
    s.meshes.push(SceneMesh::default());
    s.add_root(Node { name: Some("empty".into()), ..Default::default() });
    let doc = super::glb::json_chunk(&write_scene(&s, MeshFormat::Glb, WriteOptions::default()).unwrap().package.files[0].bytes);
    assert!(doc["meshes"].as_array().unwrap().iter().all(|m| !m["primitives"].as_array().unwrap().is_empty()), "{}", doc["meshes"]);

    // A sheared node is written so that it decomposes, and its geometry kept.
    let mut s = super::fixtures::cube();
    let root = s.roots[0];
    s.nodes[root].transform = Transform::Matrix([1.0, 0.0, 0.0, 0.0, 0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    let before = super::pose::flatten(&s).unwrap().bounds().unwrap();
    for to in [MeshFormat::Glb, MeshFormat::Fbx] {
        let w = write_scene(&s, to, WriteOptions::default()).unwrap_or_else(|e| panic!("{to}: {e}"));
        let back = read_scene(&w.package.files[0].bytes, to, &NoSidecars).unwrap().scene;
        let after = super::pose::flatten(&back).unwrap().bounds().unwrap();
        for k in 0..3 {
            assert!((after.0[k] - before.0[k]).abs() < 1e-4 && (after.1[k] - before.1[k]).abs() < 1e-4, "{to}: {before:?} → {after:?}");
        }
        if to == MeshFormat::Glb {
            let doc = super::glb::json_chunk(&w.package.files[0].bytes);
            for n in doc["nodes"].as_array().unwrap() {
                if let Some(m) = n["matrix"].as_array() {
                    let m: Vec<f64> = m.iter().map(|v| v.as_f64().unwrap()).collect();
                    assert!(super::math::decompose(&m.try_into().unwrap()).is_some(), "a non-TRS matrix was written");
                }
            }
        }
    }

    // A rigid primitive on a skinned node keeps its node's transform.
    let mut s = super::fixtures::skinned();
    let holder = s.nodes.iter().position(|n| n.skin.is_some()).unwrap();
    s.nodes[holder].transform = Transform::Trs { translation: [5.0, 0.0, 0.0], rotation: super::math::QUAT_IDENTITY, scale: [1.0; 3] };
    let mesh = s.nodes[holder].mesh.unwrap();
    let mut rigid = s.meshes[mesh].primitives[0].clone();
    rigid.skin = None;
    rigid.targets.clear();
    let rigid_min_x = rigid.positions.iter().map(|p| p[0]).fold(f32::MAX, f32::min) + 5.0;
    s.meshes[mesh].primitives.push(rigid);
    s.meshes[mesh].weights.clear();
    s.meshes[mesh].target_names.clear();
    for p in s.meshes[mesh].primitives.iter_mut() {
        p.targets.clear();
    }
    s.animations.clear();
    let back = read_scene(&write_scene(&s, MeshFormat::Glb, WriteOptions::default()).unwrap().package.files[0].bytes, MeshFormat::Glb, &NoSidecars).unwrap().scene;
    let rigid_back = super::pose::world_primitives(&back, &super::pose::default_pose(&back))
        .unwrap()
        .into_iter()
        .map(|w| w.positions.iter().map(|p| p[0]).fold(f32::MAX, f32::min))
        .fold(f32::MIN, f32::max);
    assert!((rigid_back - rigid_min_x).abs() < 1e-4, "the rigid part lost its node's +5: {rigid_back} vs {rigid_min_x}");
}

/// Leftover e of the 2026-10-30 review: FBX names no skeleton root, so an
/// FBX's skins reached glTF without a `skeleton`. Each now names its joints'
/// closest common root, which the matrix's §5.28.2 check holds to: in the
/// written graph it is each joint or an ancestor of it. The FBX is Assimp's,
/// not ours.
#[test]
fn fbx_skins_reach_glb_with_a_skeleton() {
    let fbx = include_bytes!("testdata/fbx/skinned_small_assimp_7500.fbx");
    let scene = read_scene(fbx, MeshFormat::Fbx, &NoSidecars).unwrap().scene;
    assert!(!scene.skins.is_empty() && scene.skins.iter().all(|s| s.skeleton.is_none()), "the FBX reader names no skeleton");
    let doc = super::glb::json_chunk(&write_scene(&scene, MeshFormat::Glb, WriteOptions::default()).unwrap().package.files[0].bytes);
    assert_eq!(super::matrix_tests::skeletons_checked(&doc, "Assimp's FBX -> glb"), scene.skins.len(), "a skin named no skeleton");
}

/// Each drawn primitive's world bounds at `pose`, sorted, so two scenes that
/// draw the same parts compare equal whatever their node order.
fn part_bounds(s: &Scene, pose: &super::pose::Pose) -> Vec<[f32; 6]> {
    let mut out: Vec<[f32; 6]> = super::pose::world_primitives(s, pose)
        .unwrap()
        .iter()
        .map(|w| {
            let mut b = [f32::MAX, f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MIN];
            for p in &w.positions {
                for k in 0..3 {
                    b[k] = b[k].min(p[k]);
                    b[k + 3] = b[k + 3].max(p[k]);
                }
            }
            b
        })
        .collect();
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out
}

fn assert_same_parts(want: &[[f32; 6]], got: &[[f32; 6]], what: &str) {
    assert_eq!(want.len(), got.len(), "{what}: {want:?} vs {got:?}");
    for (a, b) in want.iter().zip(got) {
        assert!(a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4), "{what}: {want:?} vs {got:?}");
    }
}

const SHEAR: [f64; 16] = [1.0, 0.0, 0.0, 0.0, 0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// 2026-10-12 review, finding 27: a mesh baked out of a sheared transform
/// hangs from the nearest animated ancestor, so the clip still moves it.
#[test]
fn a_sheared_mesh_under_an_animated_parent_still_follows_it() {
    use super::scene::{AnimPath, Animation, Channel, Interpolation, Node, Transform};
    let mut s = super::fixtures::cube();
    let cube = s.roots[0];
    s.nodes[cube].transform = Transform::Matrix(SHEAR);
    s.roots.clear();
    let mover = s.add_root(Node { name: Some("mover".into()), children: vec![cube], ..Default::default() });
    s.animations = vec![Animation {
        name: Some("move".into()),
        channels: vec![Channel {
            node: mover,
            path: AnimPath::Translation,
            interpolation: Interpolation::Linear,
            times: vec![0.0, 1.0],
            values: vec![0.0, 0.0, 0.0, 3.0, 0.0, 0.0],
        }],
    }];
    for to in [MeshFormat::Glb, MeshFormat::Fbx] {
        let back = read_scene(&write_scene(&s, to, WriteOptions::default()).unwrap().package.files[0].bytes, to, &NoSidecars).unwrap().scene;
        for t in [0.0, 0.5, 1.0] {
            let want = part_bounds(&s, &super::pose::sample_pose(&s, 0, t));
            let got = part_bounds(&back, &super::pose::sample_pose(&back, 0, t));
            assert_same_parts(&want, &got, &format!("{to} at {t}s"));
        }
    }
}

/// 2026-10-12 review, finding 27: baking a mirrored shear into a morphed mesh
/// keeps its shape, its shading (normals and their morph deltas) and its
/// front faces, and says what it did.
#[test]
fn a_mirrored_sheared_morph_mesh_keeps_its_shape_shading_and_front_faces() {
    use super::scene::Transform;
    let mut s = super::fixtures::morph();
    s.nodes[0].transform = Transform::Matrix([-1.0, 0.0, 0.0, 0.0, 0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 1.5, 0.0, 0.0, 0.0, 0.0, 1.0]);
    s.meshes[0].weights = vec![1.0, 0.0];
    s.animations.clear();
    let want = super::pose::flatten(&s).unwrap();
    let w = write_scene(&s, MeshFormat::Glb, WriteOptions::default()).unwrap();
    assert!(
        w.losses.iter().any(|l| l.kind == LossKind::Hierarchy && l.what.contains("sheared placement")),
        "{:?}",
        w.losses
    );
    let back = read_scene(&w.package.files[0].bytes, MeshFormat::Glb, &NoSidecars).unwrap().scene;
    let got = super::pose::flatten(&back).unwrap();
    assert_eq!(got.positions.len(), want.positions.len());
    for (a, b) in got.positions.iter().zip(&want.positions) {
        assert!((0..3).all(|k| (a[k] - b[k]).abs() < 1e-4), "{a:?} vs {b:?}");
    }
    for (a, b) in got.normals.as_ref().unwrap().iter().zip(want.normals.as_ref().unwrap()) {
        let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        assert!(dot > 1.0 - 1e-5, "{a:?} vs {b:?}");
    }
    assert_eq!(got.indices, want.indices, "the winding (front faces) changed");
}

/// 2026-10-12 review, finding 27: tangents baked through a mirrored shear
/// follow the surface, and their handedness (w) flips with the mirror, since
/// the bitangent is cross(normal, tangent) * w (glTF 2.0 §3.7.2.1).
#[test]
fn tangents_baked_through_a_mirror_flip_their_handedness() {
    use super::scene::Transform;
    let mut s = super::fixtures::cube();
    let root = s.roots[0];
    let mi = s.nodes[root].mesh.unwrap();
    let n = s.meshes[mi].primitives[0].positions.len();
    s.meshes[mi].primitives[0].tangents = Some(vec![[0.0, 1.0, 0.0, 1.0]; n]);
    let m = [-1.0, 0.0, 0.0, 0.0, 0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    s.nodes[root].transform = Transform::Matrix(m);
    super::pose::bake_shear(&mut s);
    let holder = s.nodes.iter().position(|n| n.mesh.is_some()).unwrap();
    assert!(super::math::decompose(&s.nodes[holder].transform.matrix()).is_some());
    let ts = s.meshes[s.nodes[holder].mesh.unwrap()].primitives[0].tangents.clone().unwrap();
    // M·(0,1,0) = (0.5, 1, 0), normalised.
    let l = (0.25f32 + 1.0).sqrt();
    for t in &ts {
        assert!((t[0] - 0.5 / l).abs() < 1e-6 && (t[1] - 1.0 / l).abs() < 1e-6 && t[2].abs() < 1e-6, "{t:?}");
        assert_eq!(t[3], -1.0, "the mirror reverses the bitangent");
    }
}

/// 2026-10-12 review, finding 27: a sheared node something is still drawn
/// through (here a joint) is approximated, and the loss names it.
#[test]
fn an_approximated_shear_is_reported() {
    use super::scene::Transform;
    let mut s = super::fixtures::skinned();
    s.animations.clear();
    let joint = s.skins[0].joints[1];
    let mut m = s.nodes[joint].transform.matrix();
    m[4] += 0.3;
    s.nodes[joint].transform = Transform::Matrix(m);
    for to in [MeshFormat::Glb, MeshFormat::Fbx] {
        let w = write_scene(&s, to, WriteOptions::default()).unwrap();
        assert!(w.losses.iter().any(|l| l.kind == LossKind::Geometry && l.what.contains("shear")), "{to}: {:?}", w.losses);
    }
}

/// 2026-10-12 review, finding 27: an FBX take keyed before 0 (pre-roll) is
/// read starting at 0, with a warning, as the USD reader reads one.
#[test]
fn an_fbx_take_with_preroll_is_read_starting_at_zero() {
    let mut s = super::fixtures::skinned();
    for ch in s.animations[0].channels.iter_mut() {
        for t in ch.times.iter_mut() {
            *t -= 1.0;
        }
    }
    let fbx = write_scene(&s, MeshFormat::Fbx, WriteOptions::default()).unwrap();
    let r = read_scene(&fbx.package.files[0].bytes, MeshFormat::Fbx, &NoSidecars).unwrap();
    let lo = r.scene.animations[0].channels.iter().filter_map(|c| c.times.first()).fold(f32::INFINITY, |m, t| m.min(*t));
    assert!(lo.abs() < 1e-6, "starts at {lo}");
    assert!(r.warnings.iter().any(|w| w.contains("shifted to start at 0")), "{:?}", r.warnings);
}

/// 2026-10-12 review, finding 27: when a mesh mixing skinned and rigid
/// primitives is split, both halves keep its morph-weight animation.
#[test]
fn both_halves_of_a_split_mesh_keep_its_weight_animation() {
    use super::scene::{AnimPath, Channel, Interpolation, MorphTarget};
    let mut s = super::fixtures::skinned();
    let holder = s.nodes.iter().position(|n| n.skin.is_some()).unwrap();
    let mi = s.nodes[holder].mesh.unwrap();
    let mut rigid = s.meshes[mi].primitives[0].clone();
    rigid.skin = None;
    s.meshes[mi].primitives.push(rigid);
    for p in s.meshes[mi].primitives.iter_mut() {
        p.targets = vec![MorphTarget { positions: vec![[0.0, 0.25, 0.0]; p.positions.len()], normals: None }];
    }
    s.meshes[mi].weights = vec![0.0];
    s.meshes[mi].target_names = vec!["lift".into()];
    s.animations[0].channels.push(Channel {
        node: holder,
        path: AnimPath::Weights,
        interpolation: Interpolation::Linear,
        times: vec![0.0, 1.0],
        values: vec![0.0, 1.0],
    });
    let back = read_scene(&write_scene(&s, MeshFormat::Glb, WriteOptions::default()).unwrap().package.files[0].bytes, MeshFormat::Glb, &NoSidecars).unwrap().scene;
    let want = part_bounds(&s, &super::pose::sample_pose(&s, 0, 1.0));
    let got = part_bounds(&back, &super::pose::sample_pose(&back, 0, 1.0));
    assert_same_parts(&want, &got, "at 1s");
}

/// The value of FBX `GlobalSettings` property `name` (its first value).
fn fbx_global_int(bytes: &[u8], name: &str) -> Option<i64> {
    use super::fbx_dom::ROOT;
    let dom = super::fbx::parse(bytes).unwrap();
    let gs = dom.child(ROOT, "GlobalSettings")?;
    let p70 = dom.child(gs, "Properties70")?;
    let found = dom.children_named(p70, "P").find_map(|p| {
        let props = dom.props(p).ok()?;
        (props.first()?.as_str()?.as_ref() == name).then(|| props.get(4)?.as_i64()).flatten()
    });
    found
}

/// a3 #4 (2026-10-31 delivery review): the delivery's `fps` reaches every
/// container that resamples. FBX keys at it and declares it (`TimeMode`, the
/// SDK's `FbxTime::EMode`: 11 is 24 fps; research-fbx.md); Collada keys at it;
/// USD keys at it in its 30-per-second time codes (three.js's USDA reader
/// assumes 30, usd_write.rs) and says `framesPerSecond`. Each used to add its
/// own 30 fps grid and declare 30.
#[test]
fn the_fps_option_keys_every_resampling_container_at_that_rate() {
    let mut s = super::fixtures::skinned();
    super::normalize::resample(&mut s, 0, 24);
    let opts = WriteOptions { fps: Some(24), ..Default::default() };
    for to in [MeshFormat::Fbx, MeshFormat::Dae, MeshFormat::Usdz, MeshFormat::Usda, MeshFormat::Usdc] {
        let w = write_scene(&s, to, opts).unwrap_or_else(|e| panic!("{to}: {e}"));
        let back = read_scene(&w.package.files[0].bytes, to, &NoSidecars).unwrap().scene;
        let mut keys = 0;
        for ch in &back.animations[0].channels {
            for t in &ch.times {
                // Or the key FBX (1 ms) and Collada (0.1 ms) add just before a
                // STEP key, so a reader that interpolates linearly still steps.
                let on = |t: f64| ((t * 24.0) - (t * 24.0).round()).abs() < 1e-4;
                let t = f64::from(*t);
                assert!(on(t) || on(t + 1e-3) || on(t + 1e-4), "{to}: a key at {t} s is off the 24 fps grid");
                keys += 1;
            }
        }
        assert!(keys > 0, "{to}");
        if to == MeshFormat::Fbx {
            assert_eq!(fbx_global_int(&w.package.files[0].bytes, "TimeMode"), Some(11));
        }
        if to == MeshFormat::Usda {
            assert!(String::from_utf8_lossy(&w.package.files[0].bytes).contains("framesPerSecond = 24"));
        }
    }
    // Unset, every container keeps its 30.
    let w = write_scene(&s, MeshFormat::Fbx, WriteOptions::default()).unwrap();
    assert_eq!(fbx_global_int(&w.package.files[0].bytes, "TimeMode"), Some(6));
}

/// A cube skinned to a chain of joints named `names`, parents first.
fn named_rig(names: &[&str]) -> Scene {
    use super::scene::{Node, Skin, SkinWeights, Transform};
    let mut s = super::fixtures::cube();
    let holder = s.roots[0];
    let first = s.nodes.len();
    for (i, n) in names.iter().enumerate() {
        s.nodes.push(Node {
            name: Some(n.to_string()),
            transform: Transform::Trs { translation: [0.0, 0.1, 0.0], rotation: super::math::QUAT_IDENTITY, scale: [1.0; 3] },
            ..Default::default()
        });
        if i > 0 {
            s.nodes[first + i - 1].children.push(first + i);
        }
    }
    s.roots.push(first);
    let joints: Vec<usize> = (first..first + names.len()).collect();
    let world = s.world_matrices();
    let inverse_bind_matrices = joints.iter().map(|&j| super::math::invert(&world[j]).unwrap()).collect();
    s.skins.push(Skin { name: None, joints, inverse_bind_matrices, skeleton: Some(first) });
    let mi = s.nodes[holder].mesh.unwrap();
    for p in s.meshes[mi].primitives.iter_mut() {
        let n = p.positions.len();
        p.skin = Some(SkinWeights { influences: 4, joints: vec![0; n * 4], weights: (0..n).flat_map(|_| [1.0, 0.0, 0.0, 0.0]).collect() });
    }
    s.nodes[holder].skin = Some(0);
    s
}

/// a3 #5 (2026-10-31 delivery review): Mixamo's `mixamorig:` joint names
/// survive the containers that cannot spell `:` in a name. USD keeps them in
/// the Skeleton's `jointNames` (UsdSkel: "If authored, provides a unique name
/// per joint"; the reader names joints from it), and the humanoid map reads
/// Collada's `mixamorig_Hips` as Mixamo's. Both lost the map.
#[test]
fn mixamo_joint_names_keep_their_humanoid_map_through_usd_and_collada() {
    let names = [
        "mixamorig:Hips", "mixamorig:Spine", "mixamorig:Spine1", "mixamorig:Spine2", "mixamorig:Neck", "mixamorig:Head",
        "mixamorig:LeftShoulder", "mixamorig:LeftArm", "mixamorig:LeftForeArm", "mixamorig:LeftHand",
        "mixamorig:RightShoulder", "mixamorig:RightArm", "mixamorig:RightForeArm", "mixamorig:RightHand",
        "mixamorig:LeftUpLeg", "mixamorig:LeftLeg", "mixamorig:LeftFoot", "mixamorig:LeftToeBase",
        "mixamorig:RightUpLeg", "mixamorig:RightLeg", "mixamorig:RightFoot", "mixamorig:RightToeBase",
    ];
    let s = named_rig(&names);
    for to in [MeshFormat::Usdz, MeshFormat::Usda, MeshFormat::Usdc, MeshFormat::Dae] {
        let w = write_scene(&s, to, WriteOptions::default()).unwrap_or_else(|e| panic!("{to}: {e}"));
        let back = read_scene(&w.package.files[0].bytes, to, &NoSidecars).unwrap().scene;
        let joints: Vec<String> = back.skins[0].joints.iter().filter_map(|&j| back.nodes[j].name.clone()).collect();
        assert_eq!(super::humanoid::humanoid_map(&joints).map(|m| m.0), Some("mixamo"), "{to}: {joints:?}");
        if to != MeshFormat::Dae {
            assert!(joints.iter().any(|j| j == "mixamorig:Hips"), "{to}: {joints:?}");
        }
    }
}

/// 2026-10-12 review, findings 28.3–28.5: what OBJ and Collada cannot draw
/// of a material's or a vertex's alpha is said, and OBJ's `d` is the alpha
/// the material is drawn with (glTF ignores it when OPAQUE). An OPAQUE
/// material with alpha 0.6 wrote `d 0.6`, which reads back translucent.
#[test]
fn alpha_obj_and_collada_cannot_draw_is_reported() {
    use super::scene::AlphaMode;
    let mut s = super::fixtures::cube();
    s.materials[0].base_color[3] = 0.6;
    let w = write_scene(&s, MeshFormat::Obj, WriteOptions { self_contained: false, ..Default::default() }).unwrap();
    let mtl = w.package.files.iter().find(|f| f.path.ends_with(".mtl")).expect("an mtl");
    assert!(!String::from_utf8_lossy(&mtl.bytes).lines().any(|l| l.starts_with("d ")), "{}", String::from_utf8_lossy(&mtl.bytes));

    let kinds = |s: &Scene, to: MeshFormat, self_contained: bool| -> Vec<String> {
        write_scene(s, to, WriteOptions { self_contained, ..Default::default() }).unwrap().losses.iter().map(|l| l.to_string()).collect()
    };
    let mut s = super::fixtures::cube();
    let n = s.meshes[0].primitives[0].positions.len();
    s.meshes[0].primitives[0].colors = Some(vec![[1.0, 0.0, 0.0, 0.25]; n]);
    for to in [MeshFormat::Obj, MeshFormat::Dae] {
        let l = kinds(&s, to, true);
        assert!(l.iter().any(|x| x.contains("vertex colour alpha")), "{to}: {l:?}");
    }
    let mut s = super::fixtures::cube();
    s.materials[0].alpha_mode = AlphaMode::Mask;
    s.materials[0].double_sided = true;
    s.materials[0].unlit = true;
    let l = kinds(&s, MeshFormat::Obj, false);
    for what in ["alpha cutoff", "double-sided", "unlit"] {
        assert!(l.iter().any(|x| x.contains(what)), "{what}: {l:?}");
    }
    let mut s = super::fixtures::cube();
    s.materials[0].alpha_mode = AlphaMode::Blend;
    s.materials[0].base_color[3] = 0.5;
    let l = kinds(&s, MeshFormat::Obj, true);
    assert!(l.iter().any(|x| x.contains("translucency")), "a single-file OBJ bakes materials into RGB: {l:?}");
}

/// 2026-10-12 review, findings 29.9–29.12: losses name what is lost, once,
/// and only that. Default filtering — linear, trilinear minification, three.js
/// r170 GLTFLoader.js:3230-3231's fallbacks — loses nothing (usdz reported
/// it twice); 3MF keeps NEAREST in its filter attribute; a single-file USDA,
/// which drops the texture, keeps the vertex colours it said it dropped; an
/// OBJ package does not ship the image it says it dropped.
#[test]
fn losses_say_what_is_lost_and_only_that() {
    use super::scene::{Filter, Image, ImageFormat, Sampler, Wrap};
    let count = |s: &Scene, to: MeshFormat, self_contained: bool, kind: LossKind| {
        write_scene(s, to, WriteOptions { self_contained, ..Default::default() }).unwrap().losses.iter().filter(|l| l.kind == kind).count()
    };
    let mut s = super::fixtures::textured();
    let trilinear = Sampler { wrap_s: Wrap::Repeat, wrap_t: Wrap::Repeat, mag_filter: Some(Filter::Linear), min_filter: Some(Filter::Linear), mipmaps: true, mipmap_filter: Filter::Linear };
    s.textures.iter_mut().for_each(|t| t.sampler = trilinear.clone());
    for to in [MeshFormat::Usdz, MeshFormat::Fbx, MeshFormat::Obj] {
        assert_eq!(count(&s, to, true, LossKind::TextureSampling), 0, "{to}: default filtering");
    }
    s.textures[0].sampler.mag_filter = Some(Filter::Nearest);
    assert_eq!(count(&s, MeshFormat::Usdz, true, LossKind::TextureSampling), 1, "nearest, once");

    let mut s = super::fixtures::textured();
    let nearest = Sampler { mag_filter: Some(Filter::Nearest), min_filter: Some(Filter::Nearest), mipmaps: false, ..Default::default() };
    s.textures.iter_mut().for_each(|t| t.sampler = nearest.clone());
    assert_eq!(count(&s, MeshFormat::ThreeMf, true, LossKind::TextureSampling), 0, "3MF says nearest");

    let mut s = super::fixtures::textured();
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
        p.colors = Some(vec![[0.5, 0.5, 0.5, 1.0]; p.positions.len()]);
    }
    let w = write_scene(&s, MeshFormat::Usda, WriteOptions::default()).unwrap();
    assert!(!w.losses.iter().any(|l| l.what.contains("vertex colours on")), "{:?}", w.losses);

    let mut s = super::fixtures::textured();
    s.images[0] = Image { name: None, format: ImageFormat::Webp, data: b"RIFF\0\0\0\0WEBPVP8 ".to_vec() };
    let w = write_scene(&s, MeshFormat::Obj, WriteOptions { self_contained: false, ..Default::default() }).unwrap();
    assert!(w.losses.iter().any(|l| l.kind == LossKind::ImageFormats), "{:?}", w.losses);
    assert!(!w.package.files.iter().any(|f| f.path.ends_with(".webp")), "{:?}", w.package.files.iter().map(|f| &f.path).collect::<Vec<_>>());
}

/// 2026-10-12 review, finding 29.8: emission brighter than 1 (glTF's
/// emissive strength) survives MTL's `Ke` both ways: values up to 1 are
/// sRGB as `Kd`'s are, and above 1 the curve continues straight, where both
/// directions clamped to 1.
#[test]
fn hdr_emission_survives_mtl() {
    let mut s = super::fixtures::textured();
    s.materials[1].emissive = [4.0, 2.0, 0.5];
    let w = write_scene(&s, MeshFormat::Obj, WriteOptions { self_contained: false, ..Default::default() }).unwrap();
    let obj = w.package.files.iter().find(|f| f.path.ends_with(".obj")).unwrap();
    let back = read_scene(&obj.bytes, MeshFormat::Obj, &super::package::MemorySidecars { files: &w.package.files }).unwrap().scene;
    let e = back.materials.iter().map(|m| m.emissive).find(|e| e[0] > 0.0).expect("an emissive material");
    assert!((e[0] - 4.0).abs() < 1e-3 && (e[1] - 2.0).abs() < 1e-3 && (e[2] - 0.5).abs() < 1e-3, "{e:?}");
}

/// Final review, finding 1 (2026-11-01): a root litegen adds itself — the
/// converter's units and up axis, delivery's orientation and placement — is
/// not the input's hierarchy, so flattening it loses nothing. Every slicer
/// request (`to=stl&units=mm`) used to report `hierarchy` lost. A group the
/// input had still is.
#[test]
fn litegen_s_own_roots_are_not_a_lost_hierarchy() {
    use super::normalize::{self, Axis};
    use super::scene::{Node, Transform};
    let hierarchy = |s: &Scene, to: MeshFormat| {
        write_scene(s, to, WriteOptions::default()).unwrap().losses.iter().any(|l| l.kind == LossKind::Hierarchy)
    };
    let mut framed = super::fixtures::cube();
    normalize::express_frame(&mut framed, 0.001, true);
    let mut oriented = super::fixtures::cube();
    normalize::orient(&mut oriented, Axis::PosZ, Axis::NegY).unwrap();
    let mut placed = super::fixtures::cube();
    normalize::place(&mut placed, true, true, Some(1.5)).unwrap();
    for (what, s) in [("units", &framed), ("orientation", &oriented), ("placement", &placed)] {
        for to in [MeshFormat::Stl, MeshFormat::Obj, MeshFormat::Ply] {
            assert!(!hierarchy(s, to), "{what} → {to}");
        }
    }
    let mut grouped = super::fixtures::cube();
    let children = std::mem::take(&mut grouped.roots);
    grouped.add_root(Node { name: Some("Group".into()), transform: Transform::default(), children, ..Default::default() });
    assert!(hierarchy(&grouped, MeshFormat::Stl), "the input's own group");
}

/// Final review, finding 4 (2026-11-01): a GLB holds only meshes a node
/// draws. Baking a shear and splitting a skinned mesh left their originals
/// behind, written in full (validator UNUSED_OBJECT, the geometry twice).
#[test]
fn a_glb_holds_only_meshes_a_node_draws() {
    use super::scene::Transform;
    let mut s = super::fixtures::cube();
    let root = s.roots[0];
    s.nodes[root].transform = Transform::Matrix(SHEAR);
    let doc = super::glb::json_chunk(&write_scene(&s, MeshFormat::Glb, WriteOptions::default()).unwrap().package.files[0].bytes);
    let drawn: std::collections::HashSet<u64> = doc["nodes"].as_array().unwrap().iter().filter_map(|n| n["mesh"].as_u64()).collect();
    let meshes = doc["meshes"].as_array().unwrap().len() as u64;
    assert!((0..meshes).all(|m| drawn.contains(&m)), "{meshes} meshes, drawn {drawn:?}");
}

/// Final review, finding 3 (2026-11-01): a rigid primitive of a skinned mesh
/// under a sheared parent keeps its place. `prepare` approximated the shear
/// before the writer split the mesh, so the part moved, unreported.
#[test]
fn a_rigid_part_of_a_skinned_mesh_under_a_shear_keeps_its_place() {
    use super::scene::{Node, Transform};
    let mut s = super::fixtures::skinned();
    s.animations.clear();
    let holder = s.nodes.iter().position(|n| n.skin.is_some()).unwrap();
    let mi = s.nodes[holder].mesh.unwrap();
    let mut rigid = s.meshes[mi].primitives[0].clone();
    rigid.skin = None;
    s.meshes[mi].primitives.push(rigid);
    s.roots.retain(|&r| r != holder);
    s.add_root(Node { name: Some("sheared".into()), transform: Transform::Matrix(SHEAR), children: vec![holder], ..Default::default() });
    let want = part_bounds(&s, &super::pose::default_pose(&s));
    for to in [MeshFormat::Glb, MeshFormat::Fbx] {
        let back = read_scene(&write_scene(&s, to, WriteOptions::default()).unwrap().package.files[0].bytes, to, &NoSidecars).unwrap().scene;
        assert_same_parts(&want, &part_bounds(&back, &super::pose::default_pose(&back)), &format!("{to}"));
    }
}

/// Final review, finding 6 (2026-11-01): UsdSkel's `jointNames` is "a unique
/// name per joint"; two joints named alike got one name each time.
#[test]
fn usd_joint_names_are_unique() {
    let s = named_rig(&["Bone", "Bone", "Tip"]);
    let w = write_scene(&s, MeshFormat::Usda, WriteOptions::default()).unwrap();
    let text = String::from_utf8(w.package.files[0].bytes.clone()).unwrap();
    for line in text.lines().filter(|l| l.contains("jointNames")) {
        let names: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
        let unique: std::collections::HashSet<&&str> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "{line}");
    }
}

/// 2026-10-12 review, finding 27: the singular-matrix test used an absolute
/// determinant threshold, so a tiny but well-formed scale (1e-5, 1e-5, 1e-3)
/// counted as singular and its normals were transformed by the plain matrix.
#[test]
fn a_small_but_regular_scale_gets_the_true_normal_matrix() {
    let n = super::math::transform_normal(&super::math::normal_matrix(&super::math::scaling([1e-5, 1e-5, 1e-3])), [std::f32::consts::FRAC_1_SQRT_2, 0.0, std::f32::consts::FRAC_1_SQRT_2]);
    assert!((n[0] - 0.99995).abs() < 1e-4 && (n[2] - 0.0099995).abs() < 1e-4, "{n:?}");
}

/// A textured, normal-mapped grid: one material, one PNG, normals, UVs and
/// (if `colored`) vertex colours, `n` quads a side.
fn textured_grid(n: usize, colored: bool) -> Scene {
    let mut s = fixtures::textured();
    s.meshes.truncate(1);
    s.meshes[0].primitives = vec![fixtures::grid_primitive(n, 1.0, Some(0), colored)];
    s.nodes.truncate(2);
    s.nodes[0].children = vec![1];
    s
}

fn write_then_read(scene: &Scene, format: MeshFormat) -> Result<Scene, MeshError> {
    let written = write_scene(scene, format, WriteOptions::default())?;
    let package = written.package;
    let sidecars = package::MemorySidecars { files: package.sidecars() };
    let bytes = &package.primary().bytes;
    match format {
        // Past the input cap these go to their readers directly: what is
        // under test is the readers' own expansion budgets.
        MeshFormat::Fbx if bytes.len() > MAX_INPUT_BYTES => {
            super::fbx::read_scene_with(bytes, &sidecars, FbxColorSpace::default(), LightUnits::default()).map(|r| r.scene)
        }
        MeshFormat::ThreeMf if bytes.len() > MAX_INPUT_BYTES => super::threemf::read_scene(bytes, &sidecars).map(|r| r.scene),
        _ if bytes.len() > MAX_INPUT_BYTES => Err(MeshError::Unsupported(format!("{} bytes, over the input cap", bytes.len()))),
        _ => read_scene(bytes, format, &sidecars).map(|r| r.scene),
    }
}

/// 2026-10-12 review #30, at full size: the compressed containers read back
/// what litegen writes at the scene caps, past their expansion budgets'
/// reach. A regular grid at the index cap (1414² quads, 12M indices, 2M
/// vertices) with vertex colours, and a dense mesh at both caps (4M vertices,
/// 12M indices, every attribute distinct). The text and uncompressed
/// containers outgrow the 128 MB input cap first, which is that cap working.
/// Minutes in a debug build, so run it released:
/// `cargo test --release --lib -- --ignored at_the_scene_caps --nocapture`.
#[test]
#[ignore = "release-mode stress test; see the doc comment"]
fn every_container_reads_back_what_it_writes_at_the_scene_caps() {
    let n = 1414;
    assert!(6 * n * n <= scene::MAX_SCENE_INDICES && (n + 1) * (n + 1) <= scene::MAX_SCENE_VERTICES);
    let dense = dense_at_the_caps_ratio(1990, 1004);
    let (v, c) = (dense.meshes[0].primitives[0].positions.len(), dense.meshes[0].primitives[0].indices.len());
    assert!(v <= scene::MAX_SCENE_VERTICES && c <= scene::MAX_SCENE_INDICES, "{v} vertices, {c} indices");
    let mut refused = Vec::new();
    for (kind, scene) in [("grid", textured_grid(n, true)), ("dense", dense)] {
    for format in [MeshFormat::Fbx, MeshFormat::ThreeMf] {
        let started = std::time::Instant::now();
        match write_then_read(&scene, format) {
            Ok(back) => {
                let tris: usize = back.meshes.iter().flat_map(|m| &m.primitives).map(|p| p.indices.len() / 3).sum();
                eprintln!("{kind} {format:?}: {tris} triangles back in {:.1?}", started.elapsed());
            }
            Err(e) => {
                eprintln!("{kind} {format:?}: REFUSED in {:.1?}: {e}", started.elapsed());
                refused.push(format!("{kind} {format:?}: {e}"));
            }
        }
    }
    }
    assert!(refused.is_empty(), "{refused:#?}");
}

/// Three kinds of dense mesh, `n` quads a side: smooth with every normal,
/// tangent, UV and colour distinct (the worst case for a writer that stores
/// each value once); flat-shaded and unwelded, as many generators deliver;
/// and a perfectly regular grid, which compresses best of all.
fn dense_meshes(n: usize) -> Vec<(&'static str, Scene)> {
    fn normalize(v: [f32; 3]) -> [f32; 3] {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / l, v[1] / l, v[2] / l]
    }
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let mut smooth = textured_grid(n, true);
    {
        let p = &mut smooth.meshes[0].primitives[0];
        let count = p.positions.len();
        let wobble = |i: usize, k: u32| ((i as u32).wrapping_mul(2654435761).wrapping_add(k * 40503) % 1000) as f32 / 1000.0;
        p.normals = Some((0..count).map(|i| normalize([wobble(i, 1) - 0.5, wobble(i, 2) - 0.5, 1.0])).collect());
        p.tangents = Some((0..count).map(|i| {
            let t = normalize([1.0, wobble(i, 3) - 0.5, 0.0]);
            [t[0], t[1], t[2], if i % 2 == 0 { 1.0 } else { -1.0 }]
        }).collect());
        p.uvs = vec![(0..count).map(|i| [wobble(i, 4), wobble(i, 5)]).collect()];
        p.colors = Some((0..count).map(|i| [wobble(i, 6), wobble(i, 7), wobble(i, 8), 1.0]).collect());
    }
    let mut flat = textured_grid(n, false);
    {
        let p = &mut flat.meshes[0].primitives[0];
        let (pos, uv) = (p.positions.clone(), p.uvs[0].clone());
        let mut q = scene::Primitive { material: p.material, ..Default::default() };
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        for t in p.indices.chunks_exact(3) {
            let [a, b, c] = [pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]];
            let lift = |v: [f32; 3], k: u32| [v[0], v[1], v[2] + ((t[0] + k) % 7) as f32 * 0.01];
            let tri = [lift(a, 0), lift(b, 1), lift(c, 2)];
            let nrm = normalize(cross(sub(tri[1], tri[0]), sub(tri[2], tri[0])));
            for (k, v) in tri.iter().enumerate() {
                q.indices.push(q.positions.len() as u32);
                q.positions.push(*v);
                normals.push(nrm);
                uvs.push(uv[t[k] as usize]);
            }
        }
        q.normals = Some(normals);
        q.uvs = vec![uvs];
        *p = q;
    }
    let regular = textured_grid(n, true);
    vec![("smooth", smooth), ("flat", flat), ("regular", regular)]
}

/// A dense mesh at the scene caps' own ratio of vertices to corners (4M to
/// 12M, one to three), every normal, tangent, UV (two sets) and colour
/// distinct: `rows` rows of `n` quads, each row with vertices of its own.
fn dense_at_the_caps_ratio(rows: usize, n: usize) -> Scene {
    let mut s = textured_grid(1, true);
    let wobble = |i: usize, k: u32| ((i as u32).wrapping_mul(2654435761).wrapping_add(k * 40503) % 1000) as f32 / 1000.0;
    let unit = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let mut p = scene::Primitive { material: Some(0), ..Default::default() };
    let (mut normals, mut tangents, mut uv0, mut uv1, mut colors) = (vec![], vec![], vec![], vec![], vec![]);
    for r in 0..rows {
        let base = p.positions.len() as u32;
        for i in 0..=n {
            for edge in 0..2 {
                let k = p.positions.len();
                p.positions.push([i as f32, (r * 2 + edge) as f32, wobble(k, 9)]);
                normals.push(unit([wobble(k, 1) - 0.5, wobble(k, 2) - 0.5, 1.0]));
                let t = unit([1.0, wobble(k, 3) - 0.5, 0.0]);
                tangents.push([t[0], t[1], t[2], if k % 2 == 0 { 1.0 } else { -1.0 }]);
                uv0.push([wobble(k, 4), wobble(k, 5)]);
                uv1.push([wobble(k, 10), wobble(k, 11)]);
                colors.push([wobble(k, 6), wobble(k, 7), wobble(k, 8), 1.0]);
            }
        }
        for i in 0..n as u32 {
            let a = base + 2 * i;
            p.indices.extend_from_slice(&[a, a + 2, a + 3, a, a + 3, a + 1]);
        }
    }
    p.normals = Some(normals);
    p.tangents = Some(tangents);
    p.uvs = vec![uv0, uv1];
    p.colors = Some(colors);
    s.meshes[0].primitives = vec![p];
    s
}

/// 2026-10-12 review #30: our own reader refused our own textured FBX above
/// ~1.92M triangles, and our own textured 3MF above ~4M, on the budgets that
/// bound what their compressed arrays and entries may expand to. What a
/// container expands to grows linearly with a mesh of one shape, so measured
/// on a small one it gives the largest mesh of that shape the scene caps
/// admit. For each kind of dense mesh, litegen reads back what it writes.
#[test]
fn our_own_fbx_and_3mf_of_any_scene_within_the_caps_fit_their_expansion_budgets() {
    let mut kinds = dense_meshes(120);
    kinds.push(("caps ratio", dense_at_the_caps_ratio(12, 150)));
    for (kind, scene) in kinds {
        let p = &scene.meshes[0].primitives[0];
        let (vertices, corners) = (p.positions.len(), p.indices.len());
        // The largest mesh of this shape the caps admit: whichever of the
        // vertex and index caps it meets first.
        let scale = (scene::MAX_SCENE_INDICES as f64 / corners as f64).min(scene::MAX_SCENE_VERTICES as f64 / vertices as f64);

        let fbx = write_scene(&scene, MeshFormat::Fbx, WriteOptions::default()).unwrap().package.files.remove(0).bytes;
        let dom = super::fbx::parse(&fbx).unwrap();
        super::fbx_read::read(&dom, &NoSidecars, FbxColorSpace::default(), LightUnits::default()).unwrap();
        let fbx_at_caps = dom.decoded_bytes() as f64 * scale;
        assert!(
            fbx_at_caps <= super::fbx_dom::MAX_DECODED_BYTES as f64,
            "{kind}: our FBX at the caps decodes to {:.0} MB, past the reader's {} MB",
            fbx_at_caps / 1e6,
            super::fbx_dom::MAX_DECODED_BYTES / 1_000_000
        );

        let tmf = write_scene(&scene, MeshFormat::ThreeMf, WriteOptions::default()).unwrap().package.files.remove(0).bytes;
        let archive = super::zip::Archive::parse(&tmf).unwrap();
        let inflated: usize = archive.entries.iter().map(|e| archive.read(e).unwrap().len()).sum();
        // XML grows a little faster than linearly: its indices gain digits,
        // from five in this mesh to eight at the caps. 30% covers a triangle
        // line's six or seven of them.
        let tmf_at_caps = inflated as f64 * scale * 1.3;
        assert!(
            tmf_at_caps <= super::zip::MAX_TOTAL_UNCOMPRESSED as f64,
            "{kind}: our 3MF at the caps inflates to {:.0} MB, past the reader's {} MB",
            tmf_at_caps / 1e6,
            super::zip::MAX_TOTAL_UNCOMPRESSED / 1_000_000
        );
    }
}

/// 2026-10-12 review #29.6: a texture read through a uv set the target
/// cannot hold is dropped with it, and was reported only as a lost uv set.
#[test]
fn a_texture_on_a_dropped_uv_set_is_reported_as_a_lost_texture() {
    let mut s = fixtures::textured();
    let p = &mut s.meshes[0].primitives[0];
    let second = p.uvs[0].iter().map(|uv| [uv[1], uv[0]]).collect();
    p.uvs.push(second);
    // The base colour stays on set 0, so the emissive map's set is the one OBJ drops.
    s.materials[0].emissive_texture = Some(TextureRef { texture: 0, uv_set: 1, transform: None });
    // As a package, with its .mtl and images: textures, one uv set.
    let w = write_scene(&s, MeshFormat::Obj, WriteOptions { self_contained: false, ..Default::default() }).unwrap();
    let kinds: Vec<LossKind> = w.losses.iter().map(|l| l.kind).collect();
    assert!(kinds.contains(&LossKind::Uvs), "{:?}", w.losses);
    assert!(
        w.losses.iter().any(|l| l.kind == LossKind::Textures && l.what.contains("uv set")),
        "{:?}",
        w.losses
    );
}

// ─── KTX2 textures (mesh-gaps item 3, Task 4) ───────────────────────────────

/// CarConcept's `Dot_C` (CC BY 4.0, `testdata/ktx2/README.md`): ETC1S with
/// BasisLZ, 128 x 128, tagged sRGB.
const DOT_C: &[u8] = include_bytes!("testdata/ktx2/Dot_C.ktx2");
/// CarConcept's `Rib_N` (CC BY 4.0): UASTC with Zstandard, 32 x 32, tagged
/// linear.
const RIB_N: &[u8] = include_bytes!("testdata/ktx2/Rib_N.ktx2");
/// Our own UASTC file with every mode and some alpha, 62 x 59.
const UASTC_ALL_MODES: &[u8] = include_bytes!("testdata/ktx2/uastc_all_modes.ktx2");
/// FNV-1a (64 bit) of each file's reference RGBA, the Basis Universal
/// transcoder's output (`node` over `zstd -dc testdata/ktx2/<name>.rgba.zst`).
/// `ktx2.rs` proves its decoder byte-equal to that reference; this file cannot
/// inflate Zstandard (only `ktx2.rs` may name `ruzstd`), so a hash stands for it.
const DOT_C_RGBA: u64 = 0x012a_5fe3_dace_a13c;
const RIB_N_RGBA: u64 = 0xa6fc_7ff7_6f11_94a5;

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// The `textured` fixture with its base-colour image replaced by `ktx2`.
fn ktx2_textured(ktx2: &[u8]) -> Scene {
    let mut s = fixtures::textured();
    s.images[0].format = scene::ImageFormat::Ktx2;
    s.images[0].data = ktx2.to_vec();
    s
}

/// A GLB that requires `KHR_texture_basisu`, as gltfpack's `-tc` (ETC1S) and
/// `-tu` (UASTC) write one, made by our own writer from a corpus KTX2 (Ruling
/// K17: gltfpack is external code, so it is not built).
fn basisu_glb(ktx2: &[u8]) -> Vec<u8> {
    glb::write_scene(&ktx2_textured(ktx2)).expect("the textured fixture writes as a GLB")
}

/// The JSON chunk of a GLB.
fn glb_json(glb: &[u8]) -> &str {
    let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    std::str::from_utf8(&glb[20..20 + len]).unwrap()
}

/// Writes the committed GLB of `testdata/gltf/ktx2/`.
#[test]
#[ignore = "set KTX2_GLB_OUT to a folder; writes the committed GLB of testdata/gltf/ktx2/"]
fn write_the_ktx2_glb() {
    let dir = std::env::var("KTX2_GLB_OUT").expect("KTX2_GLB_OUT names a folder");
    std::fs::write(format!("{dir}/textured.basisu.glb"), basisu_glb(DOT_C)).unwrap();
}

/// Writes the files that `scripts/blender-light-units/probe.py` opens in real
/// Blender: `staged` as a GLB (the reference, since Blender's glTF import
/// defines the `energy` that `LightUnits::Blender` aims for) and, with both
/// units set to `Blender`, as an FBX, a USDA and a USDZ.
#[test]
#[ignore = "set LIGHT_UNITS_OUT to a folder; writes staged.glb and staged-blender.{fbx,usda,usdz} for scripts/blender-light-units/probe.py"]
fn write_the_light_units_probe_files() {
    let dir = std::env::var("LIGHT_UNITS_OUT").expect("LIGHT_UNITS_OUT names a folder");
    let scene = fixtures::staged();
    let blender = WriteOptions { fbx_light_units: LightUnits::Blender, usd_light_units: LightUnits::Blender, ..Default::default() };
    for (name, to, opts) in [
        ("staged.glb", MeshFormat::Glb, WriteOptions::default()),
        ("staged-blender.fbx", MeshFormat::Fbx, blender),
        ("staged-blender.usda", MeshFormat::Usda, blender),
        ("staged-blender.usdz", MeshFormat::Usdz, blender),
    ] {
        let written = write_scene(&scene, to, opts).unwrap();
        let paths: Vec<&str> = written.package.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths.len(), 1, "{name} is one file, got {paths:?}");
        std::fs::write(format!("{dir}/{name}"), &written.package.primary().bytes).unwrap();
    }
}

/// Reads every `.fbx`, `.usda` and `.usdz` of a folder, once with
/// `LightUnits::Blender` and once with `Photometric`, and prints one JSON line
/// per light (`file`, `units`, `light`, `name`, `kind`, `unit`, `intensity`),
/// and one per reader warning or error. The files are Blender's own exports
/// of the probe's glTF-imported scene, or litegen's `staged-blender.*`.
#[test]
#[ignore = "set LIGHT_UNITS_READ to a folder of .fbx, .usda and .usdz files; prints one JSON line per light, read in Blender and in photometric units (run with --nocapture)"]
fn read_the_light_units_probe_files() {
    let dir = std::env::var("LIGHT_UNITS_READ").expect("LIGHT_UNITS_READ names a folder");
    let mut names: Vec<String> =
        std::fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    let mut errors = 0;
    for name in names {
        let from = match name.rsplit('.').next().unwrap() {
            "fbx" => MeshFormat::Fbx,
            "usda" => MeshFormat::Usda,
            "usdz" => MeshFormat::Usdz,
            _ => continue,
        };
        let bytes = std::fs::read(format!("{dir}/{name}")).unwrap();
        for units in [LightUnits::Blender, LightUnits::Photometric] {
            let opts = ReadOptions { fbx_light_units: units, usd_light_units: units, ..Default::default() };
            let units = units.as_str();
            let read = match read_scene_with(&bytes, from, &NoSidecars, opts) {
                Ok(read) => read,
                Err(e) => {
                    errors += 1;
                    println!("{{\"file\":{name:?},\"units\":{units:?},\"error\":{:?}}}", e.to_string());
                    continue;
                }
            };
            for (i, light) in read.scene.lights.iter().enumerate() {
                let (kind, unit) = match light.kind {
                    scene::LightKind::Directional => ("directional", "lux"),
                    scene::LightKind::Point => ("point", "cd"),
                    scene::LightKind::Spot { .. } => ("spot", "cd"),
                };
                println!(
                    "{{\"file\":{name:?},\"units\":{units:?},\"light\":{i},\"name\":{:?},\"kind\":{kind:?},\"unit\":{unit:?},\"intensity\":{}}}",
                    light.name.as_deref().unwrap_or(""),
                    light.intensity
                );
            }
            for warning in &read.warnings {
                println!("{{\"file\":{name:?},\"units\":{units:?},\"warning\":{warning:?}}}");
            }
        }
    }
    assert_eq!(errors, 0, "{errors} reads failed; see the JSON lines");
}

/// The committed GLB (`testdata/gltf/ktx2/`, whose README says how it was
/// made) is the `textured` fixture carrying `Dot_C.ktx2`, byte for byte, as a
/// `KHR_texture_basisu` texture the file requires: the vendor's file that the
/// converter and delivery tests of `tests/` serve, and its pixels are
/// `Dot_C`'s reference.
#[test]
fn the_committed_ktx2_glb_carries_the_corpus_texture() {
    let committed: &[u8] = include_bytes!("testdata/gltf/ktx2/textured.basisu.glb");
    assert!(glb_json(committed).contains("\"extensionsRequired\":[\"KHR_texture_basisu\"]"), "{}", glb_json(committed));
    let read = read_scene(committed, MeshFormat::Glb, &NoSidecars).unwrap();
    assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    let mut scene = read.scene;
    assert_eq!(scene.images.len(), 1);
    assert_eq!((scene.images[0].format, scene.images[0].data.as_slice()), (scene::ImageFormat::Ktx2, DOT_C));
    assert_eq!(scene.triangle_count(), fixtures::textured().triangle_count(), "the fixture's geometry");
    assert!(base_colour_image(&scene).data == DOT_C, "the base colour samples it");
    normalize::transcode_textures(&mut scene);
    let (_, _, _, rgba) = normalize::png_rgba8(&base_colour_image(&scene).data);
    assert_eq!(fnv1a(&rgba), DOT_C_RGBA);
}

/// `ktx2`, which the `textured` fixture's box carries, as the PNG that
/// `transcode_textures` makes of it.
fn png_of(ktx2: &[u8]) -> Vec<u8> {
    let mut s = ktx2_textured(ktx2);
    let report = normalize::transcode_textures(&mut s);
    assert!(report.dropped.is_empty() && !s.images.is_empty(), "the texture was left out: {report:?}");
    assert_eq!(s.images[0].format, scene::ImageFormat::Png);
    s.images.remove(0).data
}

/// The image a scene's first base-colour map samples.
fn base_colour_image(s: &Scene) -> &scene::Image {
    let t = s.materials.iter().find_map(|m| m.base_color_texture.as_ref()).expect("a base-colour map survives");
    &s.images[s.textures[t.texture].image]
}

/// A KTX2 texture is PNG in every container that holds a texture, whatever
/// KTX2 it was (ETC1S or UASTC, with or without Zstandard), with the pixels
/// of the reference transcoder, and nothing is reported lost.
#[test]
fn ktx2_textures_survive_every_container() {
    for (what, file, side, want) in [("ETC1S", DOT_C, 128, DOT_C_RGBA), ("UASTC with Zstandard", RIB_N, 32, RIB_N_RGBA)] {
        let glb = basisu_glb(file);
        assert!(glb_json(&glb).contains("\"extensionsRequired\":[\"KHR_texture_basisu\"]"), "{what}: {}", glb_json(&glb));
        let mut scene = read_scene(&glb, MeshFormat::Glb, &NoSidecars).unwrap().scene;
        assert_eq!(scene.images[0].format, scene::ImageFormat::Ktx2, "{what}");
        let report = normalize::transcode_textures(&mut scene);
        assert!(report.changed && report.dropped.is_empty() && report.over_budget.is_empty(), "{what}: {report:?}");
        let mut holders = Vec::new();
        for fmt in MeshFormat::ALL {
            for self_contained in [true, false] {
                let opts = WriteOptions { self_contained, ..Default::default() };
                if !fmt.capabilities(opts).textures {
                    continue;
                }
                let w = write_scene(&scene, fmt, opts).unwrap_or_else(|e| panic!("{what} to {fmt}: {e}"));
                let kinds: Vec<LossKind> = w.losses.iter().map(|l| l.kind).collect();
                assert!(!kinds.contains(&LossKind::ImageFormats) && !kinds.contains(&LossKind::Textures), "{what} to {fmt}: {:?}", w.losses);
                let files = w.package.files;
                let back = read_scene(&files[0].bytes, fmt, &package::MemorySidecars { files: &files }).unwrap().scene;
                let image = base_colour_image(&back);
                assert_eq!(image.format, scene::ImageFormat::Png, "{what} to {fmt}");
                let (width, height, _, rgba) = normalize::png_rgba8(&image.data);
                assert_eq!((width, height, fnv1a(&rgba)), (side, side, want), "{what} to {fmt}: the reference's pixels");
                holders.push(format!("{fmt}{}", if self_contained { "" } else { " package" }));
            }
        }
        assert!(holders.len() >= 8, "{what}: the containers that hold a texture: {holders:?}");
    }
}

/// Two textures of one KTX2 image (a different sampler each) are one PNG
/// that both sample, decoded once: the scene's budget counts the image's
/// bytes once, so room for one PNG is room enough.
#[test]
fn one_ktx2_image_two_textures_transcode_once() {
    use scene::{Sampler, Texture, Wrap};
    let mut s = ktx2_textured(DOT_C);
    s.textures.push(Texture {
        name: Some("clamped".into()),
        image: 0,
        sampler: Sampler { wrap_s: Wrap::ClampToEdge, wrap_t: Wrap::ClampToEdge, ..Default::default() },
    });
    s.materials[1].emissive_texture = Some(TextureRef { texture: 1, uv_set: 0, transform: None });
    let lone = png_of(DOT_C).len();
    // One PNG fits beside this much, and two would not.
    s.images.push(scene::Image {
        name: Some("stored".into()),
        format: scene::ImageFormat::Png,
        data: stored_image(&PNG_MAGIC, normalize::TEXTURE_BUDGET - lone),
    });
    s.textures.push(Texture { name: None, image: 1, sampler: Default::default() });
    s.materials[2].emissive_texture = Some(TextureRef { texture: 2, uv_set: 0, transform: None });
    let report = normalize::transcode_textures(&mut s);
    assert!(report.changed && report.dropped.is_empty() && report.over_budget.is_empty(), "{report:?}");
    assert_eq!(s.images.len(), 2, "no image was added for the second texture");
    assert_eq!((s.textures[0].image, s.textures[1].image), (0, 0), "both textures sample the one image");
    assert_eq!(s.images[0].format, scene::ImageFormat::Png);
    let (_, _, _, rgba) = normalize::png_rgba8(&s.images[0].data);
    assert_eq!(fnv1a(&rgba), DOT_C_RGBA);
    assert_eq!(s.images[0].data.len(), lone, "the PNG a lone texture gets");
    assert!(s.materials[1].emissive_texture.is_some() && s.materials[0].base_color_texture.is_some(), "both bindings stay");
    s.validate().unwrap();

    // And written, one image serves both.
    let mut s = ktx2_textured(DOT_C);
    s.textures.push(Texture { name: Some("clamped".into()), image: 0, sampler: Default::default() });
    s.materials[1].emissive_texture = Some(TextureRef { texture: 1, uv_set: 0, transform: None });
    normalize::transcode_textures(&mut s);
    let out = glb::write_scene(&s).unwrap();
    let json = glb_json(&out);
    assert!(json.matches("\"mimeType\":\"image/png\"").count() == 1 && !json.contains("KHR_texture_basisu"), "{json}");
}

/// What a PNG file starts with: all the readers sniff, so a stand-in carries it.
const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// A stand-in for an image already in a scene (a PNG or JPEG nothing here
/// decodes), `len` bytes: it only counts toward the texture budget.
fn stored_image(magic: &[u8], len: usize) -> Vec<u8> {
    let mut data = magic.to_vec();
    data.resize(len, 0);
    data
}

/// The DFD's colour tags (here: a map tagged sRGB) say how a renderer should
/// read the colours, and nothing here converts by them: a metallic-roughness,
/// normal or occlusion map comes out as the stored values, whatever it is
/// tagged. (Review Focus 4.)
#[test]
fn material_maps_tagged_srgb_keep_their_bytes() {
    // The basic block's fourth word: colour model, primaries, transfer, flags.
    let tagged = |ktx2: &[u8], transfer: u8| {
        let dfd = u32::from_le_bytes(ktx2[48..52].try_into().unwrap()) as usize;
        let mut file = ktx2.to_vec();
        file[dfd + 14] = transfer;
        file
    };
    let transfer = |ktx2: &[u8]| ktx2[u32::from_le_bytes(ktx2[48..52].try_into().unwrap()) as usize + 14];
    const LINEAR: u8 = 1;
    const SRGB: u8 = 2;
    assert_eq!((transfer(DOT_C), transfer(RIB_N)), (SRGB, LINEAR), "the corpus is tagged as its README says");
    let mut kept: Vec<(String, Vec<u8>)> = Vec::new();
    for (what, ktx2, want) in [
        ("ETC1S tagged sRGB", tagged(DOT_C, SRGB), DOT_C_RGBA),
        ("ETC1S tagged linear", tagged(DOT_C, LINEAR), DOT_C_RGBA),
        ("UASTC tagged sRGB", tagged(RIB_N, SRGB), RIB_N_RGBA),
        ("UASTC tagged linear", tagged(RIB_N, LINEAR), RIB_N_RGBA),
    ] {
        let mut s = ktx2_textured(&ktx2);
        // The one image is every data map of the wedge.
        for (_, slot) in s.materials[1].texture_slots_mut().filter(|(slot, _)| {
            matches!(slot, scene::TextureSlot::MetallicRoughness | scene::TextureSlot::Normal | scene::TextureSlot::Occlusion)
        }) {
            *slot = Some(TextureRef { texture: 0, uv_set: 0, transform: None });
        }
        let report = normalize::transcode_textures(&mut s);
        assert!(report.dropped.is_empty(), "{what}: {report:?}");
        let wedge = &s.materials[1];
        assert!(wedge.metallic_roughness_texture.is_some() && wedge.normal_texture.is_some() && wedge.occlusion_texture.is_some(), "{what}");
        let (_, _, _, rgba) = normalize::png_rgba8(&s.images[0].data);
        assert_eq!(fnv1a(&rgba), want, "{what}: the stored values, no colour conversion");
        kept.push((what.to_string(), s.images[0].data.clone()));
    }
    assert_eq!(kept[0].1, kept[1].1, "one payload, two tags: the same PNG");
    assert_eq!(kept[2].1, kept[3].1, "one payload, two tags: the same PNG");
}

/// A scene holds at most [`normalize::TEXTURE_BUDGET`] (64 MiB) of encoded
/// images once transcoded, PNG and JPEG alike: KTX2 images are transcoded one
/// at a time in image order, and from the first that would pass it, the rest
/// are left out with a note that says why. The written GLB reads back.
#[test]
fn the_texture_budget_drops_the_rest_and_says_so() {
    use scene::{Image, ImageFormat, Texture};
    let (dot, rib) = (png_of(DOT_C).len(), png_of(RIB_N).len());
    assert!(dot > rib, "a 128 x 128 PNG is larger than a 32 x 32 one: {dot} {rib}");
    for (stored_format, magic) in [(ImageFormat::Png, &PNG_MAGIC[..]), (ImageFormat::Jpeg, &[0xFF, 0xD8, 0xFF][..])] {
        // Images: a stored one, then three KTX2 files in this order. The
        // stored one leaves room for the first KTX2 file's PNG, and for the
        // third's too (it is smaller than the second's), but not for the
        // second's.
        let mut s = fixtures::textured();
        s.images = vec![
            Image { name: Some("stored".into()), format: stored_format, data: stored_image(magic, normalize::TEXTURE_BUDGET - dot - rib) },
            Image { name: Some("first".into()), format: ImageFormat::Ktx2, data: DOT_C.to_vec() },
            Image { name: Some("second".into()), format: ImageFormat::Ktx2, data: DOT_C.to_vec() },
            Image { name: Some("third".into()), format: ImageFormat::Ktx2, data: RIB_N.to_vec() },
        ];
        s.textures = (0..4).map(|image| Texture { name: None, image, sampler: Default::default() }).collect();
        let bind = |texture| Some(TextureRef { texture, uv_set: 0, transform: None });
        s.materials[0].base_color_texture = bind(1);
        s.materials[1].base_color_texture = bind(2);
        s.materials[2].base_color_texture = bind(3);
        s.materials[2].emissive_texture = bind(0);
        let report = normalize::transcode_textures(&mut s);
        assert!(report.changed && report.dropped.is_empty(), "{report:?}");
        assert_eq!(report.over_budget, ["second (KTX2)", "third (KTX2)"], "transcoding stops at the first that would pass the budget");
        assert_eq!(
            report.notes(),
            [
                "texture second (KTX2) was left out: transcoding it would pass the 64 MiB texture budget",
                "texture third (KTX2) was left out: transcoding it would pass the 64 MiB texture budget",
            ]
        );
        assert_eq!(normalize::TEXTURE_BUDGET, 64 << 20);
        let names: Vec<_> = s.images.iter().map(|i| (i.name.as_deref().unwrap(), i.format)).collect();
        assert_eq!(names, [("stored", stored_format), ("first", ImageFormat::Png)], "the others are gone: {names:?}");
        assert!(s.materials[0].base_color_texture.is_some() && s.materials[2].emissive_texture.is_some(), "the textures that stayed are bound");
        assert!(s.materials[1].base_color_texture.is_none() && s.materials[2].base_color_texture.is_none(), "the others are unbound");
        let total: usize = s.images.iter().map(|i| i.data.len()).sum();
        assert!(total <= normalize::TEXTURE_BUDGET, "{total}");
        s.validate().unwrap();
        if stored_format == ImageFormat::Png {
            let glb = glb::write_scene(&s).unwrap();
            let back = read_scene(&glb, MeshFormat::Glb, &NoSidecars).unwrap().scene;
            assert_eq!(back.images.len(), 2);
            assert_eq!(base_colour_image(&back).format, ImageFormat::Png);
        }
    }
}

/// A KTX2 file litegen cannot decode is left out as it always was, and the
/// note now says why, in the decoder's words.
#[test]
fn a_ktx2_that_does_not_decode_says_why() {
    let mut cut = ktx2_textured(&DOT_C[..DOT_C.len() - 200]);
    let report = normalize::transcode_textures(&mut cut);
    assert!(report.changed && report.over_budget.is_empty(), "{report:?}");
    assert_eq!(report.dropped.len(), 1, "{report:?}");
    assert_eq!(report.dropped[0].texture, "quadrants (KTX2)");
    let reason = &report.dropped[0].reason;
    assert!(!reason.is_empty() && !reason.starts_with("KTX2 texture"), "the decoder's reason, without its own prefix: {reason}");
    assert_eq!(report.notes(), [format!("texture quadrants (KTX2) could not be decoded and was left out: {reason}")]);
    assert!(cut.images.is_empty() && cut.textures.is_empty(), "nothing of it is kept");
    assert!(cut.materials[0].base_color_texture.is_none());
}

/// A PNG keeps what the texture has: an opaque one is RGB, one with alpha is
/// RGBA, and both read back as the decoder's pixels.
#[test]
fn a_transcoded_png_has_an_alpha_channel_only_if_the_texture_does() {
    for (what, file, alpha) in [("opaque", DOT_C, false), ("translucent", UASTC_ALL_MODES, true)] {
        let (width, height, has_alpha, rgba) = normalize::png_rgba8(&png_of(file));
        let decoded = ktx2::decode_rgba8(file).unwrap();
        assert_eq!((width, height, has_alpha), (decoded.width, decoded.height, alpha), "{what}");
        assert_eq!(rgba, decoded.pixels, "{what}");
        assert_eq!(rgba.chunks_exact(4).any(|px| px[3] != 255), alpha, "{what}: the fixture has alpha exactly when it should");
    }
}

