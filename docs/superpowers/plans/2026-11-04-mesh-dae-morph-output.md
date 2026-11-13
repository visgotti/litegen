# Collada Morph Output Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** With `dae_morphs: keep`, the Collada writer writes morph controllers (skin over morph, weight animation). The default stays byte-identical.

**Architecture:** A `WriteOptions.dae_morphs` switch flips `dae::capabilities(opts).morph_targets`. `dae::prepare` skips `bake_morph_targets` under `Keep`, and the writer emits `<morph>` controllers that the existing reader and SceneKit read.

**Tech Stack:** Rust (`mesh/dae.rs`, `mesh/xml.rs` via `quick_xml`), the convert handler, the harness (SceneKit, xmllint).

**Spec:** `docs/superpowers/specs/2026-11-04-mesh-dae-morph-output-design.md`

**Status: done on 2026-11-09 by session litegen-fc, not committed.** The ledger,
`.superpowers/sdd/2026-11-04-mesh-dae-morph-output/progress.md`, records each task's RED and
GREEN, every review and fix round, both negative controls, the rulings DM1–DM23 and the gate.
It isn't committed. Code comments cite the rulings by ID, and the SceneKit spike's findings
are in `docs/superpowers/research/research-scenekit-collada-morphs.md`.

The proof:
- **The spike (Task 0).** SceneKit (macOS 26.6.2) plays exactly what the writer writes:
  - NORMALIZED morphs with absolute targets, at their default weights;
  - the weight animation, on the MORPH_WEIGHT `<source>` id only;
  - a skin over the morph.
  Each picture is 0.00 % from a hand-made static control, and every comparison can fail.
- **Rust gate,** on tree 7e02650 with cargo 1.99.1 and clippy 0.1.99:
  - lib 2,986 passed, 0 failed, 18 ignored;
  - nine load-sensitive tests run alone, 9/9;
  - `--tests --no-fail-fast` with Postgres: 54 integration suites, 896 passed, 0 failed;
  - clippy `--all-targets --all-features -D warnings` clean.
- **Release harness,** on a `mesh_matrix` built from the same tree:
  - every fixture with `--dae-keep`, 1,260/1,260. 120 of the cases are `dae-keep`, and 7 of
    those hold a `<morph>`: three.js and assimp refused all 7 as documented, and SceneKit
    judged every frame;
  - three Khronos morph imports with `--dae-keep`, each 126/126: MorphPrimitivesTest, with
    several primitives; AnimatedMorphCube, with a weight animation; and MorphStressTest, whose
    animated frames in 5 cases are unjudged because SceneKit drops clips that share a node
    (DM21);
  - xmllint validates every `dae-keep` file against COLLADA 1.4.1.
- **Default unchanged.** `bake_is_todays_output` pins the Collada output of every fixture,
  with the macOS-only pins of DM2a. The matrix without `--dae-keep` is byte-identical.
- **Negative controls.** Negating one target's offsets, and writing RELATIVE with absolute
  targets, each fail `kept_morphs_round_trip_through_our_reader`.
- **SDKs and the converter package.** litegen-d0 regenerated the SDKs, and its package offers
  `keep`. The converter and converter-app CI jobs pass on the final mesh, with wasm at
  1,120 KiB of a 1,145 KiB budget.

The build differs from the steps below in these places:

- **Task split.** Task 4 ran as 4a (the harness and the docs) and 4b (the controller's runs and
  gate) (DM3). The final review led to one fix wave and two addenda (DM22, DM23).
  Implementers and reviewers ran on sonnet; the final review ran on fable.
- **Choices the plan left open:**
  - The weight channel targets the MORPH_WEIGHT `<source>` id (DM9), which SceneKit
    requires. A test reads the written XML, since the round trip can't catch it.
  - `prepare` under Keep names every target as written: `ncname`, or `target{i}` when
    unnamed (DM14).
  - On the request, a bad word is 400 `malformed_request`, as `fbx_light_units`' is. On the
    converter it is `param_invalid` (DM8).
  - Keep's resample budget charges what the writer writes, `primitives × targets × 3` per key
    (DM13). Its controllers are charged against `MAX_SCENE_VERTICES` before anything is
    written:
    - the target references (DM17);
    - every per-set skin controller's vertices (DM22);
    - its joint names and bind poses (DM23).
    Without that, a crafted upload could make Keep write gigabytes. Keep charges no copied
    geometry, because it bakes nothing per instance (DM11).
  - The harness variant is the opt-in `mesh_matrix --dae-keep` (DM15). Its judges:
    - xmllint, with the schema fetched, pinned and resolved offline (DM6, DM7);
    - CI installs xmllint for the test (DM16);
    - SceneKit's renderer drives the morpher's players (DM10);
    - a `<morph>` file needs every frame judged (DM18), apart from DM21's cited shared-clip
      case;
    - SceneKit's rest uv frame is judged against SceneKit's own picture of the plain file
      (DM20). Its uv rendering differs from three.js's on textured Khronos models whatever
      the morph form.
- **Known limits, documented** (dae.rs's module doc, 3d-format-conversion.md §7, the harness
  README):
  - three.js and assimp refuse a `keep` file: three.js first in
    `ColladaParser.parseAnimationChannel` when the file has a weight clip;
  - a morphed mesh with several primitives reads back as one mesh per primitive;
  - the byte pins run on macOS only (DM2a);
  - the schema pins are trust-on-first-use.
- **Open, for the user:**
  - DM4's follow-up: an opt-in that renames a Collada clip whose name is the id of a node, a
    geometry or an animation, because SceneKit registers no such clip;
  - pre-existing, not item 8's:
    - Bake's per-instance copies may repeat a skin controller's joint arrays uncharged;
    - the resample budget counts `resample_times` while the writers key `bake_times` (up to
      about 2× with STEP keys).
- **Parked minors,** left as they are:
  - `weight_animations` repeats the matrix sampler block;
  - `geometry_element`'s two positional bools;
  - `entry()` in `plan_morph_sets`;
  - the converter param is a `string` in OpenAPI, as `fbx_light_units` is;
  - no `output_packages` + keep test (the same `write_keyed`);
  - the `0.02`/`24` constants repeated in the harness. A one-off check found them identical
    to the page's `compareNear`;
  - the 16-float bind-shape matrix isn't charged (the joint term bounds it).

Files to commit (38). † marks a file that also holds edits from items 3, 5 or 10, or the
light-units option, all of them done. ‡ marks a file that also holds other sessions' work in
progress:
- litegen-4c's horizontal scaling, in `router.rs`, `poller.rs`, `api/handlers/mod.rs`,
  `ci.yml` and `docs/ci.md`;
- the provider audit's comment edits in `fal.rs`;
- another session's edits in the research README.
Check with those sessions first.

```
.github/workflows/ci.yml ‡
docs/ci.md ‡
docs/mcp/tools.md †
docs/superpowers/3d-format-conversion.md †
docs/superpowers/plans/2026-11-04-mesh-dae-morph-output.md
docs/superpowers/plans/2026-11-04-mesh-gaps-handoff.md †
docs/superpowers/research/README.md ‡
docs/superpowers/research/research-scenekit-collada-morphs.md
docs/superpowers/specs/2026-11-04-mesh-dae-morph-output-design.md
docs/superpowers/specs/2026-11-04-mesh-gaps-overview-design.md †
litegen-core/examples/mesh_matrix.rs †
litegen-core/src/api/handlers/convert.rs †
litegen-core/src/api/handlers/mod.rs † ‡
litegen-core/src/mesh/dae.rs †
litegen-core/src/mesh/mod.rs †
litegen-core/src/mesh/pose.rs †
litegen-core/src/mesh/scene_matrix_tests.rs †
litegen-core/src/providers/mod.rs †
litegen-core/src/providers/model3d/fal.rs † ‡
litegen-core/src/providers/model3d/meshy.rs †
litegen-core/src/providers/model3d/mock.rs †
litegen-core/src/providers/model3d/tripo.rs †
litegen-core/src/proxy/config_conformance.rs †
litegen-core/src/proxy/model3d_convert.rs †
litegen-core/src/proxy/model3d_deliver.rs †
litegen-core/src/proxy/poller.rs † ‡
litegen-core/src/proxy/router.rs † ‡
litegen-core/src/types/mod.rs †
litegen-core/tests/convert_api.rs †
litegen-core/tests/live_model3d.rs †
litegen-core/tests/model3d_api.rs †
litegen-core/tests/model3d_validation.rs †
models/mock.yaml
scripts/mesh-verify/README.md †
scripts/mesh-verify/dae-keep.mjs
scripts/mesh-verify/dae-keep.test.mjs
scripts/mesh-verify/scenekit/render.swift †
scripts/mesh-verify/verify.mjs †
```

litegen-d0 lists the regenerated SDK, landing and converter-package files with its own work.

## Global Constraints

- The overview's "Rules every plan carries" apply.
- `DaeMorphs::Bake` is the default. Output under it is byte-identical to today's: record the sha256 of every fixture's Collada output before Task 1.
- D6 is settled: `dae_morphs: "bake" | "keep"` (default `bake`), as a converter query parameter (only with `to=dae`) and as a 3D request field, recorded in the `DeliveryPlan`. Keep-nothing with `keep` is 400 `option_needs_keeping`. Ping the frontend/SDK session first with the shape.

## Review Focus

1. A mesh whose primitives disagree in vertex count across targets. `validate` already refuses this, but the writer must use per-primitive geometries, one `<geometry>` per primitive, as today.
2. A mesh instanced on two nodes with different `weights`. One controller per mesh, plus per-node weights, isn't expressible in Collada, because `<instance_controller>` has no weight override. Write one controller per node and weight set, and test that the two instances read back with their own weights.
3. A morph target with no normal offsets beside one with them: the target geometries must still carry normals (base normals for the first).
4. Weight animation with STEP or CUBICSPLINE: resample by the same rules as matrix channels, with the key at 0.
5. A scene with morph targets on an unskinned mesh under an animated parent: the morph controller's `<instance_controller>` sits on the node, and the transform animation keeps working.

---

### Task 0: Spike: what SceneKit plays

**Files:** throwaway, under the ledger folder: `.superpowers/sdd/<date>-mesh-dae-morph-output/spike/`. Nothing in `src/`.

- [ ] Hand-write three Collada files from the `morph` fixture's numbers. With `scripts/mesh-verify/scenekit/render.swift` (built by the harness), render each at rest and at three sample times:
  - (a) a NORMALIZED morph with absolute targets and NORMAL inside `<vertices>`, static weights;
  - (b) (a) plus weight animation;
  - (c) a skin over (b).
- [ ] Do the same for (a) as RELATIVE with offset targets.
- [ ] Record per file: whether SceneKit shows the target at its default weight, whether the weight animation plays, and whether skin over morph deforms. Put the pictures in the spike folder.
- [ ] Apply the spec's decision table. Record the branch taken in the ledger and the `dae.rs` doc. If SceneKit reads no morph at all, stop here and tell the user.

### Task 1: The switch, and Bake unchanged

**Files:** `litegen-core/src/mesh/mod.rs` (`WriteOptions.dae_morphs`, `pub enum DaeMorphs { Bake, Keep }`, `#[default] Bake`), `litegen-core/src/mesh/dae.rs` (`capabilities`, `prepare`, `extra_losses`).

**Interfaces:**
- Produces: `pub enum DaeMorphs { #[default] Bake, Keep }` and `WriteOptions { dae_morphs: DaeMorphs, .. }`.

- [ ] Write `bake_is_todays_output`, which compares against the recorded hashes.
- [ ] Add the field and wire `capabilities` and `extra_losses`. `prepare` calls `bake_morph_targets` only under `Bake`.
- [ ] Run `cargo test --lib mesh::`. Expected: pass (Bake is unchanged; Keep isn't written yet).

### Task 2: Writing morph controllers

**Files:** `litegen-core/src/mesh/dae.rs` (the writer's geometry, controller and animation sections, and tests), `litegen-core/src/mesh/scene_matrix_tests.rs` (the `dae:keep` write mode).

**Interfaces:**
- Consumes: `DaeMorphs::Keep`.

- [ ] Write:
  - `kept_morphs_round_trip_through_our_reader`;
  - `kept_morphs_under_a_skin_round_trip` (or, per Task 0's branch, the documented bake and loss);
  - `kept_morph_normals_sit_in_vertices`;
  - `a_multi_primitive_morph_comes_back_as_meshes`;
  - one test per Review Focus item: `instances_with_their_own_weights_get_their_own_controllers`, `targets_without_normal_offsets_still_carry_normals`, `step_and_cubic_weight_animation_is_resampled`, `a_morphed_mesh_under_an_animated_parent_keeps_both`;
  - the `dae:keep` mode in `scene_matrix_tests`.
- [ ] Run `cargo test --lib -- mesh::dae mesh::scene_matrix_tests`. Expected: FAIL (Keep writes no controller; targets are lost).
- [ ] Implement per the spec's form: the method Task 0 settled, with `POSITION` and `NORMAL` inside `<vertices>` for morphed geometries only. Unmorphed geometries keep today's layout.
- [ ] Run the same command. Expected: pass.
- [ ] Negative controls: (a) negate one target's offsets; (b) write RELATIVE with absolute targets. Each makes `kept_morphs_round_trip_through_our_reader` fail. Restore by rewriting.

### Task 3: The option on the converter and on generations

**Files:** `litegen-core/src/api/handlers/convert.rs` (`PARAMS`, `parse_query`, the OpenAPI param), `litegen-core/src/types/mod.rs` (the request field `dae_morphs`), the 3D request validation and the keep-nothing refusal (`api/handlers/mod.rs`), `litegen-core/src/proxy/model3d_deliver.rs` (`DeliveryPlan.dae_morphs`, used when writing Collada), the convert tests, `litegen-core/tests/model3d_api.rs`, and `docs/mcp/tools.md` (regenerated).

- [ ] Ping the frontend/SDK session with the shape.
- [ ] Write `dae_morphs_keep_writes_controllers`, `dae_morphs_only_for_dae`, and the generation tests from the spec (Collada with `<morph>` beside an unchanged GLB, the default byte-identical, the keep-nothing refusal, the poller path).
- [ ] Run the convert tests. Expected: FAIL (an unknown parameter).
- [ ] Implement, then regenerate `docs/mcp/tools.md` (`cargo run --example mcp_tools_doc > ../docs/mcp/tools.md`).
- [ ] Run the convert, openapi and mcp tests. Expected: pass.

### Task 4: Oracles, docs and gate

**Files:** `litegen-core/examples/mesh_matrix.rs` (emit `dae:keep` outputs), `scripts/mesh-verify/verify.mjs` (expected-refusal handling for three.js and assimp on `dae:keep`; an xmllint pass), and the docs listed in the spec.

- [ ] Implement the harness variant. three.js and assimp load failures on `dae:keep` are recorded as refusals citing their source lines. A successful load fails the case.
- [ ] Run `cargo run --release --example mesh_matrix -- <dir> --formats glb,dae,fbx,usdz` and `node verify.mjs <dir> <report>`. Expected: SceneKit passes every `dae:keep` case at rest and at every sample time, xmllint reports 0 errors, and three.js and assimp are recorded as refusals.
- [ ] Confirm the harness agrees with Task 0's branch: SceneKit plays what the spike said it plays. Record any difference in the ledger before merging.
- [ ] Update the docs. Run the overview's full gate. Write the ledger.
