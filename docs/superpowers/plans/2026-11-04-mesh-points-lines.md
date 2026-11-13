# Points and Lines Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The scene holds point and line primitives. glTF, OBJ, PLY, USD and Collada read and write them, and every other container reports them lost by name.

**Architecture:** `Primitive.topology` (Triangles, Lines or Points, glTF's modes 0, 1 and 4) with the existing attributes. The posing path (`world_primitives`, `bake`, `degrade`, `merge_all`) carries every topology, and bounds count them. Only `flatten`/`ir::Mesh`, the metrics and `inspect`'s triangle counts take triangles alone. `prepare` strips what a target can't hold.

**Tech Stack:** Rust (`mesh/`), the Node render harness (three.js r183, usdrecord, SceneKit).

**Spec:** `docs/superpowers/specs/2026-11-04-mesh-points-lines-design.md`

**Status: done on 2026-11-08 by session litegen-fc, not committed.** The ledger,
`.superpowers/sdd/2026-11-04-mesh-points-lines/progress.md`, records each task's RED and GREEN,
every review and fix round, and the gate. It isn't committed, so the rulings are listed in the
"Rulings" section at the end of this plan, and code comments cite them by ID.

The proof:
- **Rust gate,** on tree fe679b6 with cargo 1.99.1 and clippy 0.1.99:
  - lib 2,764 passed, 0 failed;
  - the three load-sensitive tests alone. `fbx::tests::scale_animated_ancestors_are_found_once_per_chain`
    failed once at load average ~11, then passed 3/3 alone;
  - `--tests --no-fail-fast` with Postgres: 54 integration suites, 860 passed, 0 failed.
    The run's 56 targets also repeat the library's 2,764 and main.rs's none (3,624 in all);
  - clippy `--all-targets --all-features` clean.
- **Release harness,** on matrices built from tree 64bb3cc, whose item 10 code equals fe679b6's:
  - every fixture 1,140/1,140;
  - the `--import` set 501 of 570, with the other 69 refused by name as the expected scene is;
  - Khronos MeshPrimitiveModes 228/228;
  - Stanford's drill point cloud 45 plus 69 refused;
  - a usdcat-written `wire.usdc` 114/114;
  - textured points 45 plus 69 refused;
  - 0 failures anywhere, with every point or line comparison a renderer can't draw skipped
    with a citation.
- **SDKs:** litegen-d0 regenerated the SDK and landing copies of the two converter header
  descriptions.

The build differs from the steps below in these places:

- **Task split.** Task 6 ran as 6a (the harness), 6b (the `--import` sets and the docs) and
  6c (the controller's release runs and gate) (I10-R3). 6a had two follow-ups for dense
  clouds (I10-R20, R23). A final whole-item review (fable), one fix wave and its re-review
  followed. Implementers and reviewers ran on sonnet, because the Opus limit ended Task 1's
  first implementer (I10-R5).
- **Choices the spec left open:**
  - A Draco primitive whose mode isn't TRIANGLES or TRIANGLE_STRIP is refused by name.
    Item 2's arm was widened for this, with litegen-b2's agreement (I10-R6a).
  - An FBX `Line` strip with no end stops at the array's end. assimp wraps it back to the
    first index instead (I10-R10).
  - USD writes each line segment as its own 2-vertex linear curve, de-indexed (I10-R8).
    Primvar element counts follow the schema (I10-R7).
  - `widths` falls back to 1 cm when the scene has no usable bounds (I10-R9).
  - Every material-less primitive binds one shared `DefaultMaterial`, so triangle scenes
    stay byte-identical to the writer before item 10 (I10-R15).
  - USD reports a line's normals as lost, since no reader orients a ribbon by them (final
    review).
  - 3MF reports a node emptied by the strip once, not twice (I10-R24).
- **A bug the harness found (Task 6b, I10-R17).** OBJ's `p` takes no texture coordinate, so
  textured points drew wrong. Points now draw with an untextured copy of their material,
  `uvs` and `textures` are reported, and a self-contained OBJ gets the reader's white
  materials for each kind (I10-R12).
- **Carried by default, with no opt-in (I10-R2).** Points and lines are content, as layers,
  lights and cameras are, not an output policy.
- **Judges that differ from the spec's wording:**
  - A sparse cloud is judged by blob count plus the mean of blob centres (I10-R19).
  - A dense cloud is judged by four checks: the bounding box, the convex hull's centroid,
    a 1 px precision check and an 8 px occupancy check (I10-R21, R23).
  - A case where both sides refuse by name counts as a refusal (I10-R22).
  - Points and lines are hidden in the light and lit modes (I10-R18).
  - SceneKit's vertex-only-PLY picture keeps only point indices that name a vertex (I10-R25).
    Model I/O gives such a file 3n indices and draws the extra ones at the origin.
- **Known limits, documented** (3d-format-conversion.md §1 and §3–§8, the harness README):
  - three.js's USDLoader and RealityKit drop USD `Points` and `BasisCurves`, and SceneKit
    drops `Points`, so usdrecord is USD's judge.
  - FBX, 3MF and STL write neither points nor lines.
  - The dense judge sees coverage, not density: a hole under ~24 px, or every third point
    thinned out, passes.
  - A future fixture holding both lights or cameras and points or lines would mismatch in
    the camlight judges (I10-R18).
- **Parked minors,** left as they are:
  - `world_bounds`' signature and `MODE_*`'s placement;
  - `write_keyed`'s retry;
  - Collada's per-corner `Box<dyn Iterator>` and `read_geometry`'s nine parameters;
  - OBJ's colour-plan duplication and `SceneFacts`' counts;
  - RF1's Collada `t=None` leg and the usdzip command for `wire.usdc`;
  - an FBX trailing-strip warning;
  - two over-long doc lines (glb.rs:30, obj.rs).
- **Item 5's follow-ups (I10-R11)** are item 5's, not item 10's: unnamed cameras paired by
  rank, and VirtualCity's yfov-only cameras in the Collada oracle and usdrecord.

Files to commit (29). Five of them, marked †, also hold item 3's edits (item 3 is in
progress). Commit those with item 3, or check its status first.

```
docs/superpowers/3d-format-conversion.md †
docs/superpowers/plans/2026-11-04-mesh-gaps-handoff.md
docs/superpowers/plans/2026-11-04-mesh-points-lines.md
docs/superpowers/specs/2026-11-04-mesh-gaps-overview-design.md
litegen-core/src/api/handlers/convert.rs †
litegen-core/src/mesh/dae.rs
litegen-core/src/mesh/fbx.rs
litegen-core/src/mesh/fbx_read.rs
litegen-core/src/mesh/fbx_write.rs
litegen-core/src/mesh/fixtures.rs
litegen-core/src/mesh/glb.rs
litegen-core/src/mesh/inspect.rs
litegen-core/src/mesh/mod.rs †
litegen-core/src/mesh/normalize.rs †
litegen-core/src/mesh/obj.rs
litegen-core/src/mesh/ply.rs
litegen-core/src/mesh/pose.rs
litegen-core/src/mesh/scene.rs †
litegen-core/src/mesh/scene_matrix_tests.rs
litegen-core/src/mesh/testdata/usd/README.md
litegen-core/src/mesh/testdata/usd/wire.usdc
litegen-core/src/mesh/threemf.rs
litegen-core/src/mesh/usd.rs
litegen-core/src/mesh/usd_read.rs
litegen-core/src/mesh/usd_write.rs
scripts/mesh-verify/README.md
scripts/mesh-verify/page/render.js
scripts/mesh-verify/scenekit/render.swift
scripts/mesh-verify/verify.mjs
```

litegen-d0 lists the regenerated SDK and landing files with its own work.

## Global Constraints

- The overview's "Rules every plan carries" apply.
- Start after items 4 and 5 land, because all three edit `scene.rs`, `Capabilities` and `LossKind`.
- New wire strings, exact: `points`, `lines` (`LossKind::Points`, `LossKind::Lines`). The converter's `X-Litegen-Conversion-Losses` description and the SDK doc comments list them. Ping the frontend/SDK session with the two strings.
- The `Topology` default is `Triangles`, so every existing constructor and fixture keeps its meaning.
- USD `widths`: one constant, 0.25 % of the rest-bounds diagonal.

## Review Focus

1. A primitive with morph targets or a skin on lines or points: the pose evaluator must move them (`pose::sample_pose` and the writers' baking) exactly as it moves triangles.
2. A material with a texture on a points primitive with UVs: kept by glTF and USD, and the harness's albedo mode draws it.
3. `pose::degrade` to a hierarchy-less container (OBJ): line and point positions are baked to world space like triangles.
4. An OBJ `l` statement with `v/vt` pairs: the texture coordinates are read, not mistaken for positions.
5. A PLY with both `face` and unreferenced vertices: those vertices aren't turned into points (only a face-less file is a point cloud).

---

### Task 1: The scene model

**Files:** `litegen-core/src/mesh/scene.rs` (`Topology`, `Primitive.topology`, `validate`, `drawable_count`, `Capabilities.points/lines`, `LossKind::Points/Lines`, `losses`), `litegen-core/src/mesh/mod.rs` (the `Empty` check uses `drawable_count`; every format's capabilities; `prepare` strips), `litegen-core/src/mesh/pose.rs` (`world_primitives`, `bake`, `degrade` and `merge_all` carry every topology; `flatten` and the metrics take triangles only; new `world_bounds`), `litegen-core/src/mesh/normalize.rs` (`place` uses `world_bounds`), `litegen-core/src/mesh/inspect.rs` (manifest bounds use `world_bounds`).

**Interfaces:**
- Produces: `pub enum Topology { #[default] Triangles, Lines, Points }` and `Primitive.topology: Topology`.
- Produces: `Scene::drawable_count() -> usize`.
- Produces: `Capabilities { points: bool, lines: bool, .. }`. `full()` is true for both and `none()` false.
- Produces: `pose::strip_topologies(scene: &mut Scene, caps: &Capabilities)`, which `prepare` calls.
- Produces: `pub fn world_bounds(scene: &Scene, pose: &Pose) -> Option<([f64; 3], [f64; 3])>` in pose.rs, covering every topology.

- [ ] Write `a_lines_primitive_needs_pairs`, `points_take_any_count`, `a_scene_of_points_is_not_empty`, `metrics_ignore_points_and_lines`, `world_primitives_pose_lines_and_points`, `merge_all_keeps_topologies_apart`, `world_bounds_count_points_and_lines`, `a_point_cloud_is_placed`, and the two loss tests (`losses` names `points` and `lines` by count for a target without them).
- [ ] Add `fixtures::wire()` per the spec, and register it in `fixtures::all()`.
- [ ] Run `cargo test --lib -- mesh::scene mesh::pose`. Expected: FAIL (the types are undefined).
- [ ] Implement. Writers stay unchanged in this task, and `prepare` strips both topologies for every format.
- [ ] Run `cargo test --lib mesh::`. Expected: pass. Every format reports both losses for `wire()`.

### Task 2: glTF

**Files:** `litegen-core/src/mesh/glb.rs` (the reader's mode match, the writer's `mode`).

- [ ] Write `glb_reads_points_lines_loops_and_strips`, the glTF write/read round trip of `wire()`, `a_line_strip_of_a_billion_vertices_is_charged_before_expanding`, and the truncation and bit-flip sweeps on a points-and-lines GLB.
- [ ] Run them. Expected: FAIL (skipped with the warning).
- [ ] Implement. glTF's capabilities set `points` and `lines` true.
- [ ] Run `cargo test --lib -- mesh::glb mesh::scene_matrix_tests`. Expected: pass.

### Task 3: OBJ and PLY

**Files:** `litegen-core/src/mesh/obj.rs`, `litegen-core/src/mesh/ply.rs` (new `ply::prepare` and `ply::losses`, called from `MeshFormat::prepare` and `write_scene` as `threemf::prepare` and `threemf::losses` are), plus their capability rows in `mod.rs`.

- [ ] Write `obj_reads_p_and_l`, `obj_l_with_texture_coordinates` (Review Focus 4), `ply_without_faces_is_a_point_cloud`, `ply_edges_are_lines`, `ply_keeps_points_only_alone`, `unreferenced_vertices_beside_faces_are_not_points` (Review Focus 5), each writer's round trip of `wire()`, and the sweeps.
- [ ] Run them. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test --lib -- mesh::obj mesh::ply mesh::scene_matrix_tests`. Expected: pass.

### Task 4: USD and Collada

**Files:** `litegen-core/src/mesh/usd_read.rs`, `usd_write.rs`, `usd.rs` (capabilities, `prepare`), `dae.rs` (reader, writer, capabilities).

- [ ] Write `usda_points_and_linear_curves_read`, `dae_lines_and_linestrips_read`, the USD and Collada round trips of `wire()` (DAE reports `points`), `usd_widths_follow_the_rule`, and the sweeps on both readers' new paths.
- [ ] Run them. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test --lib -- mesh::usd mesh::usda mesh::usdc mesh::dae mesh::scene_matrix_tests`. Expected: pass.

### Task 5: FBX lines read, and the containers that drop both

**Files:** `litegen-core/src/mesh/fbx_read.rs` (line geometry), `litegen-core/src/mesh/threemf.rs`, `stl.rs` and `fbx_write.rs` (debug assertions).

- [ ] Write `fbx_line_geometry_reads` (a hand-built FBX through the test DOM helpers), `writers_never_see_a_topology_they_cannot_hold` (with `#[should_panic]` in a debug build for each assertion), and `points_to_stl_is_named`.
- [ ] Run them. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test --lib mesh::`. Expected: pass.

### Task 6: Harness, Review Focus 1–3, docs and gate

**Files:** `scripts/mesh-verify/page/render.js` (Points and LineSegments; PLY geometry without an index drawn as points), `scripts/mesh-verify/verify.mjs` (the usdrecord bbox and centroid comparison for points and lines; cited skips), and the docs listed in the spec.

- [ ] Write `skinned_and_morphed_lines_pose_like_triangles` (Review Focus 1), `textured_points_keep_their_texture` (Review Focus 2) and `obj_bakes_lines_to_world_space` (Review Focus 3) in `scene_matrix_tests`.
- [ ] Run `cargo test --lib mesh::`. Expected: pass after any fixes.
- [ ] Run the harness: `cargo run --release --example mesh_matrix -- <dir> --fixtures wire,cube,textured` and `node verify.mjs <dir> <report>`. Expected: 0 failures, and every skip cites a loader source line.
- [ ] Run the `--import` set from the spec. Expected: the native checks pass.
- [ ] Negative control: OBJ `l` indices off by one. The OBJ cases fail. Restore by rewriting.
- [ ] Update the docs. Send the loss strings to the frontend/SDK session. Run the overview's full gate. Write the ledger.

## Rulings (item 10's build, 2026-11-08, session litegen-fc)

The ledger (`.superpowers/sdd/2026-11-04-mesh-points-lines/progress.md`) records each ruling
with its evidence. It isn't committed, so they are listed here, one line each. Code comments
cite them by ID.

- **I10-R1:** In Task 1, glTF's `prepare` runs `degrade` only when its capabilities still
  differ from `full()` with points, lines, lights and cameras forced true. It strips the
  topologies separately, as item 5 did for lights and cameras. Task 2 restored
  `caps == full()`.
- **I10-R2:** Points and lines are carried by default wherever a container holds them,
  with no opt-in. They are content, not an output policy.
- **I10-R3:** Task 6 ran as 6a (the harness), 6b (imports and docs) and 6c (the
  controller's runs and gate).
- **I10-R4:** Task 1 made three edits in threemf.rs ahead of Task 5, so that `wire()` could
  join `all()`. A codec's losses are judged on the scene less the topologies it drops.
- **I10-R5:** Implementers and reviewers run on sonnet while Opus is limited (until
  2026-11-12).
- **I10-R6a** (supersedes R6): Item 2's Draco arm refuses every mode except TRIANGLES and
  TRIANGLE_STRIP as malformed, with the mode named, with litegen-b2's agreement.
- **I10-R7:** USD primvar element counts: one per point on `Points`, and one per curve for
  `uniform` on curves.
- **I10-R8:** USD lines are written de-indexed, one 2-vertex linear curve per segment.
- **I10-R9:** `widths` falls back to 0.01 (1 cm) when the bounds are missing, zero or not
  finite.
- **I10-R10:** An FBX `Line` strip with no end stops at the array's end, with no wrap to the
  first index (assimp wraps).
- **I10-R11:** Item 5's camera follow-ups (unnamed cameras paired by index; VirtualCity's
  yfov-only cameras) stay item 5's, after item 10's Task 6.
- **I10-R12:** A self-contained OBJ's `prepare` gets the reader's white materials for the
  `faces`, `lines` and `points` usemtl names (prepare equals the read-back). Also:
  - PLY's shared budget gets a test;
  - the matrix's PLY filter derives from `ply`'s content rule;
  - OBJ gets tests for `l v//vn` and `p v/vt`.
- **I10-R13:** In the shared tree, a mutant is restored by reversing only its lines with
  Edit, then touch and diff. A whole-file restore once erased a peer's edit.
- **I10-R14:** USD and Collada fix-round minors: test names, the linestrip budget test, a
  Collada twin with normals, UVs and colours on lines, and `deindex_scene` skipping
  de-indexed primitives.
- **I10-R15:** `prepare` binds one shared `DefaultMaterial` for every material-less
  primitive, so triangle scenes stay byte-identical. In a mixed scene, the material counts
  of prepare and the read-back differ in the known way.
- **I10-R16:** Collada's points-only expectation becomes the spec's named refusal.
- **I10-R17:** OBJ points carry no UV. Textured points bind an untextured copy of their
  material, and `obj::losses` names `uvs` and `textures`.
- **I10-R18:** Points and lines are hidden in the light and lit harness modes. This is a
  latent hazard for a fixture that also holds lights or cameras.
- **I10-R19:** A sparse cloud's centroid is the mean of blob centres, with a blob-count
  check. That is stricter than the spec's "centroid of covered pixels".
- **I10-R20:** The drill's failures were harness calibration. Dense clouds get their own
  judge, and assimp's face-less-PLY refusal is a cited skip.
- **I10-R21:** A dense cloud's centroid is the convex hull's, since it doesn't depend on
  dot size.
- **I10-R22:** A case where both sides refuse by name counts as a refusal. It is pinned to
  the case's format word.
- **I10-R23:** The dense judge adds a precision check (every drawn pixel within 1 px of the
  expected covered set) and an 8 px occupancy check.
- **I10-R24:** The fix wave may wire `threemf::losses_after_strip` at mod.rs:712. The
  OBJLoader citation is :406-425.
- **I10-R25:** SceneKit's picture of a vertex-only PLY keeps only point indices that name a
  vertex. Model I/O gives 3n indices and draws the rest at the origin. No dilation.
