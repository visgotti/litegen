# Lights and Cameras Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The scene holds punctual lights and cameras on nodes. glTF, FBX, Collada and USD read and write them with pinned unit and axis conversions, and the static containers report them lost.

**Architecture:** `Scene.lights` and `Scene.cameras` in glTF's units, with `Node.light`/`Node.camera` indices. Each codec converts at its boundary (FBX's +X camera and −Y light axes by fixed post-rotations), and litegen annotations make Collada and USD round trips exact.

**Tech Stack:** Rust (`mesh/`), the Node harness (new `camera` mode, `lit` mode from item 4, usdrecord `--camera`).

**Spec:** `docs/superpowers/specs/2026-11-04-mesh-lights-cameras-design.md`

**Status: done on 2026-11-07 by session litegen-fc, not committed.** The ledger,
`.superpowers/sdd/2026-11-04-mesh-lights-cameras/progress.md`, records every task's RED and
GREEN, every review and fix round, and the gate; it isn't committed, so the rulings are
listed in the "Rulings" section at the end of this plan, and code comments cite them by
ID. The build differs from the steps below in these places:

- **Task split.** Task 5 ran as 5a (Collada) and 5b (USD) (R9). Task 6 ran as 6a (the
  harness), 6b (the third-party imports and the docs) and 6c (the controller's release
  runs and full gate) (R20). A final whole-item review, one fix wave and its re-review
  followed.
- **Mappings that differ from the spec's first word:**
  - Collada `falloff_angle` is the **full cone** (R23, revising R4): the COLLADA 1.4.1
    `<spot>` entry is silent, and SceneKit (the spec's Collada oracle, measured) and Blender
    both mean the full cone.
  - The FBX judge and numeric oracle use assimp's `assjson` export, because assimp 6.0's
    glTF2 exporter writes no cameras or lights (R11). The committed assimp-written FBX
    carries lights only (assimp writes no FBX cameras) (R10).
  - A light's `range` has no Collada or USD form; it rides in litegen's annotation and
    writes report it as a `lights` loss (R27).
  - A spot wider than a hemisphere becomes a point light with a warning everywhere (R32).
  - No re-hang for cameras under scaled ancestors: three.js r183 already excludes a
    camera's world scale from its view matrix (R33).
- **Added beyond the steps:** hostile-input sweeps for every new reader path, incl. the
  annotation-free Collada and USD paths (the overview's bar, line 7); placed-only reading
  with named warnings (R13); FBX light colours follow `FbxColorSpace` (R14); FBX
  `AmbientColor` and Collada `<ambient>` warned (R15); `pose::bake_shear` re-places lights
  and cameras under shear (R19); USD never refuses a stage over a light or camera, and
  failed prims keep their key charge (R21, R22); native camera and lights-only passes in
  the harness (R24); unnamed nodes paired by depth-first rank (R25, amended 2026-11-08); Blender-exported FBX and USD in
  the import set (R28).
- **Known limits, documented** (3d-format-conversion.md §8 and §9, the harness README):
  Blender reads FBX and USD light intensities in its own units, so by default lights
  differ between litegen and Blender, by about 54× (FBX point and spot) or 683× (FBX sun),
  and 683× (USD point and spot) or 2732× (USD sun). Since 2026-11-08 the opt-in
  `fbx_light_units` / `usd_light_units` = `blender` writes and reads Blender's units,
  confirmed in Blender 5.2.0 to f32 (the light-units plan,
  `docs/superpowers/plans/2026-11-08-light-units-option.md`). No external judge sees FBX or Collada orthographic
  magnifications, a USD spot's cone, or a USD light's intensity.

**Follow-ups, 2026-11-08 (litegen-fc):** six harness judge failures found by item 2's control run on Khronos's plain samples were fixed in `scripts/mesh-verify/` only (A–F in the ledger: depth-first pairing of unnamed nodes, R25 amended; assimp's lone-`<yfov>` skip; usdrecord's 16:9 expected picture; SceneKit's clip names and USD texture scale as cited skips; repeated material names), plus stale citations. The plain set passes 798/798 (was 761), every fixture 1,140/1,140, the camera set 114/114 each, every ratio unchanged. SceneKit can't play a litegen Collada clip named like a node; the writer fix is item 8's, as an opt-in (handoff, Interactions). The follow-ups add these to commit: `scripts/mesh-verify/{camlight-oracle.mjs, camlight-oracle.test.mjs, layer-extensions.mjs, layer-extensions.test.mjs, scenekit-limits.mjs (new), page/camera-view.js (new), page/node-rank.js (new), page/render.js, scenekit/render.swift, verify.mjs, README.md}` and `docs/superpowers/3d-format-conversion.md` (§8).

Files to commit (34; four also hold item 2's edits, marked †):

```
docs/mcp/tools.md
docs/superpowers/3d-format-conversion.md
docs/superpowers/plans/2026-11-04-mesh-gaps-handoff.md
docs/superpowers/plans/2026-11-04-mesh-lights-cameras.md
docs/superpowers/specs/2026-11-04-mesh-gaps-overview-design.md
litegen-core/src/api/handlers/convert.rs
litegen-core/src/mesh/dae.rs
litegen-core/src/mesh/fbx.rs
litegen-core/src/mesh/fbx_read.rs
litegen-core/src/mesh/fbx_write.rs
litegen-core/src/mesh/fixtures.rs
litegen-core/src/mesh/glb.rs †
litegen-core/src/mesh/inspect.rs
litegen-core/src/mesh/mod.rs †
litegen-core/src/mesh/normalize.rs
litegen-core/src/mesh/pose.rs †
litegen-core/src/mesh/scene.rs
litegen-core/src/mesh/scene_matrix_tests.rs
litegen-core/src/mesh/testdata/dae/README.md
litegen-core/src/mesh/testdata/dae/assimp/staged_assimp.dae
litegen-core/src/mesh/testdata/fbx/README.md
litegen-core/src/mesh/testdata/fbx/staged_assimp.fbx
litegen-core/src/mesh/usd.rs
litegen-core/src/mesh/usd_read.rs
litegen-core/src/mesh/usd_write.rs
litegen-core/src/providers/mod.rs
litegen-core/src/proxy/model3d_deliver.rs †
litegen-core/src/types/mod.rs
scripts/mesh-verify/README.md
scripts/mesh-verify/camlight-oracle.mjs
scripts/mesh-verify/camlight-oracle.test.mjs
scripts/mesh-verify/page/render.js
scripts/mesh-verify/scenekit/render.swift
scripts/mesh-verify/verify.mjs
```

**The proof** (the ledger holds each command and its output):

- Every test the spec names was seen failing on its assertion, then passing; each
  negative control (the FBX camera turn, the Collada `falloff_angle` rule) failed its
  tests and the file was restored by rewriting.
- Full gate on tree f84419f8: `cargo test --lib` (the three load-sensitive skipped) 2,507
  passed, 2 failed, both item 2's (`providers::config_conformance` reads item 2's new
  `providers/model3d/mock_data/` directory as a file); the three load-sensitive tests alone
  3/3; `cargo test --tests` with Postgres, 55 integration suites, 859 passed, 0 failed;
  `cargo clippy --all-targets --all-features -D warnings` clean. `mesh::isolation_tests`
  pass.
- Release harness: every fixture 1,026/1,026, the camera and lights oracle 2,737 checks
  with no failure; the `--import` set (Khronos Cameras, DirectionalLight,
  LightsPunctualLamp, PlaysetLightTest, PointLightIntensityTest, and Blender-exported FBX
  and USD of `staged`) 798/798, native camera and lights-only checks at 0.00 %;
  LightVisibility is refused by name (it requires `KHR_node_visibility`).
- Hostile-input sweeps on every new reader path, incl. the annotation-free Collada and USD
  paths; slowest case 3.4 ms in debug.


## Global Constraints

- The overview's "Rules every plan carries" apply.
- Start after item 4 (it provides the harness's `lit` mode and `Capabilities.layers`) and before item 10.
- Units are glTF's: candela or lux, half-angle cones in radians, `yfov` in radians, and −Z forward.
- New wire strings, exact: `lights`, `cameras`. Ping the frontend/SDK session with them.
- `KHR_lights_punctual` goes in `extensionsUsed` only.

## Review Focus

1. A light or camera on a node that is also a joint or holds a skinned mesh: hoisting skinned meshes (`glb::hoist_skinned_meshes`) and USD's joint folding mustn't move or lose it.
2. A camera under a non-uniformly scaled or sheared parent: glTF's cameras ignore scale per the spec. The world eye and forward must still match after `pose::bake_shear`.
3. An FBX camera with `ApertureMode` horizontal-only, or with `FieldOfViewX`/`FieldOfViewY` set: `yfov` comes from the right pair.
4. A spot with `inner_cone == outer_cone` (hard edge): glTF requires inner < outer, so the reader nudges it and warns. That's tested.
5. Two cameras with one name (three.js binds by name): unique names after `unique_node_names`, and both survive.
6. A light or camera that breaks a bound in a third-party file (cones out of order, perspective `znear` 0, a non-finite intensity): the reader drops it with a warning naming the bound, the file still reads, and the node stays.

---

### Task 1: Name glTF cameras in the warnings

**Files:** `litegen-core/src/mesh/glb.rs` (reader).

- [ ] Write `glb_names_cameras_it_drops`: a glTF with `cameras: [{…}]` and `node.camera: 0`. `read_scene`'s warnings contain "1 camera(s)".
- [ ] Run `cargo test --lib mesh::glb::tests::glb_names_cameras_it_drops`. Expected: FAIL (no warning).
- [ ] Implement the warning.
- [ ] Run `cargo test --lib mesh::glb`. Expected: pass. Record the fix in the ledger, because it can ship on its own.

### Task 2: Scene types, validation and losses

**Files:** `litegen-core/src/mesh/scene.rs` (`Light`, `LightKind`, `Camera`, `Projection`, `Scene.lights/cameras`, `Node.light/camera`, `validate`, `Capabilities.lights/cameras`, `LossKind::Lights/Cameras`, `losses`), `litegen-core/src/mesh/mod.rs` (capability rows), `litegen-core/src/mesh/pose.rs` (`degrade` drops them; empty nodes prune), `litegen-core/src/mesh/fixtures.rs` (`staged()`).

**Interfaces:**
- Produces: the types in the spec's code block, verbatim.

- [ ] Write `light_and_camera_values_are_validated` (one assertion per bound in the spec, including orthographic `znear == 0` accepted and perspective `znear == 0` refused) and `losses_name_lights_and_cameras`.
- [ ] Run `cargo test --lib mesh::scene`. Expected: FAIL (undefined).
- [ ] Implement. Every writer reports both as lost until its own task.
- [ ] Run `cargo test --lib mesh::`. Expected: pass.

### Task 3: glTF

**Files:** `litegen-core/src/mesh/glb.rs` (`KHR_lights_punctual` and `cameras`, read and write; `CARRIED_EXTENSIONS`).

- [ ] Write the glTF round trip of `staged()`, a `lights_are_never_required` assertion, Review Focus 1 and 4 as `a_light_on_a_skinned_node_survives_hoisting` and `a_hard_edged_spot_is_nudged_and_warned`, and Review Focus 6 as `a_bad_light_or_camera_is_dropped_not_refused` (glTF; repeat in Tasks 4 and 5 for FBX, Collada and USD).
- [ ] Run `cargo test --lib mesh::glb`. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test --lib -- mesh::glb mesh::scene_matrix_tests`. Expected: pass.

### Task 4: FBX

**Files:** `litegen-core/src/mesh/fbx_read.rs` (`NodeAttribute` Light/Camera to scene values, with the axis corrections), `fbx_write.rs` (`NodeAttribute` objects and connections, with the inverse corrections), and `testdata/fbx/staged_*.fbx` with its README line.

- [ ] Write the FBX round trip of `staged()`, `fbx_cameras_look_along_plus_x_on_the_wire`, `fbx_lights_point_along_minus_y_on_the_wire`, `fbx_aperture_modes_choose_the_right_fov` (Review Focus 3), and a test reading the committed third-party-written FBX.
- [ ] Run `cargo test --lib mesh::fbx`. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test --lib -- mesh::fbx mesh::scene_matrix_tests`. Expected: pass.
- [ ] Negative control: drop the camera post-rotation. The axis test fails. Restore by rewriting.

### Task 5: Collada and USD

**Files:** `litegen-core/src/mesh/dae.rs` (`library_lights`, `library_cameras`, `instance_light`/`instance_camera`, the litegen `<extra>`), `usd_read.rs` and `usd_write.rs` (`DistantLight`, `SphereLight` + `ShapingAPI`, `Camera`, customData), `usd.rs` (capabilities, `prepare` keeps them).

- [ ] Write:
  - the round trips of `staged()` with and without the annotations;
  - `usd_camera_fov_matches_the_aperture_formula`;
  - `collada_falloff_angle_follows_the_spec` (full or half as the COLLADA 1.4.1 `<spot>` entry says, quoted in the test's doc comment);
  - `a_camera_under_shear_keeps_its_eye_and_forward` (Review Focus 2);
  - `two_cameras_with_one_name_both_survive` (Review Focus 5);
  - a test reading a SceneKit- or assimp-written Collada with a camera and lights.
- [ ] Run `cargo test --lib -- mesh::dae mesh::usd`. Expected: FAIL.
- [ ] Implement, choosing the USD intensity mapping from the research note and citing it.
- [ ] Run `cargo test --lib mesh::`. Expected: pass.
- [ ] Negative control: invert the `falloff_angle` rule (full ↔ half). The Collada test fails. Restore by rewriting.

### Task 6: Harness, third-party files, docs and gate

**Files:**
- `scripts/mesh-verify/page/render.js` (`camera` mode; lights-only `lit`)
- `scripts/mesh-verify/verify.mjs` (the per-format judges: GLTFLoader for glTF, SceneKit for Collada and USD, usdrecord `--camera` for USD, assimp's glTF conversion for FBX; cited skips for three.js `FBXLoader` and `ColladaLoader`)
- `scripts/mesh-verify/scenekit/render.swift` (render through the file's camera)
- a new `scripts/mesh-verify/camlight-oracle.mjs` (the numeric oracle on assimp's glTF conversion)
- the docs listed in the spec

- [ ] Implement the `camera` mode, SceneKit's file-camera render and the numeric oracle. Calibrate: GLB → GLB is 0.00 %, and the oracle on the expected GLB itself reports no difference.
- [ ] Negative control: drop the FBX camera post-rotation. The numeric oracle's forward check fails by 90°. Restore by rewriting.
- [ ] Run `cargo run --release --example mesh_matrix -- <dir> --fixtures staged,textured,skinned` and `node verify.mjs <dir> <report>`. Expected: 0 failures, with skips cited.
- [ ] Run `--import` with the Khronos `Cameras` and `LightsPunctualLamp` models (local only). Expected: the native `camera` and `lit` checks pass.
- [ ] Update the docs. Send the loss strings to the frontend/SDK session.
- [ ] Run the overview's full gate. Write the ledger.

---

## Rulings (item 5's build, 2026-11-06, session litegen-fc)

The ledger (`.superpowers/sdd/2026-11-04-mesh-lights-cameras/progress.md`) records each with
its evidence; it isn't committed, so they are listed here, one line each. Code comments
cite them by ID.

- **R1:** Task 1's glTF camera warning shipped as the spec says; Task 3 re-pointed
  `glb_names_cameras_it_drops` at a camera the reader still drops (one breaking a bound).
  Cost if wrong: one test body.
- **R2:** Task 2 could edit every `Node {`/`Scene {` literal in `mesh/` mechanically, and
  gave `Light` and `Camera` a `check` returning the bound broken as text, which `validate`
  and every reader use. Cost if wrong: a wider Task 2 diff.
- **R3:** USD adds a case for Review Focus 1: a light and a camera on a joint survive USD's
  joint folding. Cost if wrong: one more test.
- **R4 (revised by R23):** Collada `falloff_angle` was first ruled the half-angle, on the
  OpenGL `GL_SPOT_CUTOFF` reading of its 180° default. Superseded: see R23.
- **R5:** a hard-edged spot (`inner == outer`) is softened (`inner = outer × (1 − 1e-4)`) and
  warned, not dropped; every other broken bound drops the light or camera with a warning,
  and the node stays. Cost if wrong: such a spot's edge softens by 0.01 %.
- **R6:** USD intensity is photometric per the UsdLux schema: `SphereLight` (treatAsPoint,
  radius 0, normalize) = 4 × candela, `DistantLight` (normalize, angle 0) = lux; the exact
  glTF values ride in `customData.litegen`. Cost if wrong: Storm and SceneKit render our
  USD lights brighter or dimmer than glTF's (Storm draws them 4× brighter, a cited skip).
- **R7:** glTF's interim camera warning counted the top-level `cameras` array. Cost if
  wrong: none now (Task 3 replaced it).
- **R8:** `check` adds the glTF schema's own minimums (intensity ≥ 0, range > 0, aspect > 0,
  colour in [0, 1]); readers normalise an over-range colour into the intensity first. Cost
  if wrong: a third-party light with a negative intensity is dropped, not kept.
- **R9:** Task 5 ran as 5a (Collada) and 5b (USD). Cost if wrong: one more review cycle.
- **R10:** the committed third-party FBX is assimp-written (assimp 6.0 writes FBX lights but
  no cameras) and is checked under assimp's documented conventions; cameras get their
  independent check from assimp's importer in the harness oracle. Cost if wrong: no
  third-party FBX camera file pins our camera reader.
- **R11:** the FBX and Collada oracle and camera judge use assimp's `assjson` export,
  because assimp 6.0's glTF2 exporter writes no cameras or lights. Cost if wrong: none.
- **R12:** Task 3 rewrote Task 2's glTF `prepare` test in place as
  `gltf_prepare_keeps_what_degrade_would_drop`. Cost if wrong: none.
- **R13:** readers keep only lights and cameras a node places; an unplaced one is dropped
  with a warning naming it; a directional light never carries a `range`. Cost if wrong: an
  unplaced glTF camera isn't carried (it's named).
- **R14:** FBX light colours follow `FbxColorSpace`, as material colours do. Cost if wrong:
  in the default sRGB mode Blender reads light colours slightly light, the trade-off
  material colours already make.
- **R15:** FBX `AmbientColor` (non-black) and Collada `<ambient>` are warned (the spec's
  non-goal said "Warned" and no task owned it). Cost if wrong: one more warning.
- **R16:** the stale colour-space doc comments were fixed with R14. Cost if wrong: none.
- **R17:** Collada light colours are linear (Collada stores colour × intensity, a linear
  scale; sRGB decoding explodes past 1). Cost if wrong: files from a writer that meant
  sRGB read slightly off.
- **R18:** Collada `quadratic_attenuation` stays 1 (inverse square); three.js's 1 m cutoff
  is a cited deviation. Cost if wrong: Collada lights look dim in the three.js viewer.
- **R19 (and R19a):** `pose::bake_shear` moves a light or camera out of a sheared static
  run onto a new node with its old node's name and the run's orthonormal frame, so glTF
  and FBX keep its eye and forward. Cost if wrong: one more node per sheared light or
  camera.
- **R20:** Task 6 ran as 6a (harness), 6b (imports and docs) and 6c (the controller's
  release runs and full gate). Cost if wrong: one more review cycle.
- **R21:** a USD light or camera prim whose transform can't be read is dropped with a
  warning; the stage still reads. Cost if wrong: such a stage reads without that light.
- **R22:** a failed USD light or camera keeps its animation-key charge, so failed prims
  can't multiply the densify work. Cost if wrong: a stage whose hostile light used up the
  budget refuses a later animated Xform.
- **R23 (revises R4):** Collada `falloff_angle` is the **full cone** in degrees
  (2·deg(outer)). The COLLADA 1.4.1 `<spot>` entry (quoted in dae.rs) is silent; the spec's
  Collada oracle, SceneKit (measured in Task 6a), and Blender both mean the full cone; three.js
  `ColladaLoader` (half-angle) and assimp (its inner cone) are cited deviations. Cost if
  wrong: three.js shows our Collada spots twice as wide, and files from half-angle writers
  read half as wide.
- **R24:** the harness gained native `camera` and `lit_lights` passes for glTF originals
  (GLTFLoader on both sides, cameras picked by name), because the spec runs the native
  check "in camera and lit mode". Cost if wrong: more harness code to maintain.
- **R25:** the harness identifies camera and light nodes by index where they have no name
  (GLTFLoader's associations, the oracle's `node i`, assimp's `nodes[i]`), so unnamed
  cameras are judged, not failed. Cost if wrong: none beyond the code.
  *Amended 2026-11-08 (follow-up A):* an unnamed node is paired by its depth-first rank
  (pre-order, roots and children in file order), the order litegen's writers number nodes
  in, not by its array index. The index held only for a file already in that order, and
  Khronos's Duck and VirtualCity failed under it (`scripts/mesh-verify/page/node-rank.js`).
- **R26:** Collada light colours stay linear (R17): assimp and Blender write them linear,
  and colour × intensity only makes sense linearly. SceneKit decodes them as sRGB
  (measured); the harness cites that. Cost if wrong: Apple renderers draw our Collada
  lights darker and more saturated.
- **R27:** glTF `range` has no native Collada or USD form; it rides only in litegen's
  annotation, and writes report it as a `lights` loss so other readers aren't misled. Cost
  if wrong: one more loss line for lights with a range.
- **R28:** the Blender-exported FBX and USD the spec asks for "if available" came from a
  local Blender 5.2.0 bundle, used read-only and headless, kept local. Cost if wrong: time.
- **R29:** Task 6b's doc findings were fixed in the final fix wave, after the harness
  follow-up's rerun and with R27's code, because the import summary and the range docs
  depend on both. Cost if wrong: none.
- **R30:** R28's Blender run, meant to be read-only, wrote 10 Python bytecode caches inside
  the local Blender bundle; they are left for the user to decide,
  and later runs disable bytecode writing. Cost if wrong: 10 cache files in that bundle.
- **R31 and R31a (superseded by R33):** a planned re-hang of cameras under scaled
  ancestors for glTF output, dropped before it was built.
- **R32:** a spot wider than a hemisphere becomes a point light with a warning in every
  reader, not dropped. Cost if wrong: it also lights the half-space behind it.
- **R33 (supersedes R31, R31a):** no re-hang for cameras under scaled ancestors: three.js
  r183, which the dashboard and harness pin, already excludes a camera's world scale from its
  view matrix (`Camera.js` L116-L126, "to be glTF conform"), so the harm the final review
  feared doesn't occur there; the docs say so. Cost if wrong: an engine that applies node
  scale to cameras shows a scaled far plane; the re-hang can be added later.
- **R34:** the sweeps' "under a second a case in release" rests on their debug maxima
  (1–4 ms; debug is slower than release); no separate release test build. Cost if wrong:
  none in practice.
- **R35:** the build's ledger is kept (the repo keeps ledgers as evidence), and nothing is
  committed on this laptop, so the skill's delete-the-workspace and finish-the-branch steps
  were skipped. Cost if wrong: about 6.5 GB of git-ignored reports stay in the ledger.

The implementers' own rulings (FBX: an unset aspect written as 16:9 and an unset far plane
as 1000 m, both reported as losses; `range` through `EnableFarAttenuation` +
`FarAttenuationEnd`; `OrthoZoom` per ufbx. Collada: the `falloff_exponent` ↔ inner-cone rule
and the orthographic rule. USD: the softness and aperture rules) are documented in each
codec's module doc with their citations.
