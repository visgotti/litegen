# Review Leftovers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the three open leftovers (joint names across a chained animate, glTF `skin.skeleton`, the fps-resampling key budget) and re-prove and close the three already fixed.

**Architecture:** A chained operation inherits its rig's joint naming through `SourceInput` and the recorded delivery plan. The glTF writer computes the joints' closest common root. `normalize` counts the keys a rekey would make before it allocates them.

**Tech Stack:** Rust (litegen-core: `mesh/`, `proxy/`, `api/handlers/`, `providers/model3d/mock.rs`), cargo tests.

**Spec:** `docs/superpowers/specs/2026-11-04-mesh-review-leftovers-design.md`

**Status: done on 2026-11-04 by session litegen-a3 [1a9a54], not committed.** Every task below
was carried out. The ledger, `.superpowers/sdd/2026-11-04-mesh-review-leftovers/progress.md`,
records each ruling (Rulings 1–24) with its reason and its cost. The build differs from the
steps in these places:

- **(b) joint names.**
  - Task 2's mock animates the mesh a mock rig job stored when it is chained by that job's task id.
  - Task 3's mock serves any mock row whose task is animate, and the operator row gets its own id (Ruling 6).
  - The naming is read from the rig row whatever provider rigged it; only `vendor_task_id` is provider-filtered (Ruling 12).
  - In lax mode, a dropped `joint_naming: "vendor"` means the rig's naming is inherited (Ruling 13).
  - One test beyond the plan: `a_rig_from_another_provider_still_passes_its_naming_on`.
- **(e) `skin.skeleton`.**
  - `closest_common_root` became a private binary-lifting `Ancestry` table, built once per write (Rulings 14 and 16). A path walk per skin was quadratic on a crafted deep chain.
  - A held skeleton that isn't an ancestor-or-self of every joint is replaced by the closest common root (Ruling 17). The spec says a held skeleton is "written unchanged", and that still holds for every legal value.
  - The matrix check also fails a skin that should name a skeleton but doesn't, and the FBX test reuses it.
- **(f) the budget.**
  - Beyond `rekeyed_size` and `fits_fps`, normalize.rs adds `animation_size`, `size_after_rekey`, `check_rekey_budget`, `RekeyLimit` and `resample_with_size(scene, &mut size, anim, fps) -> Result<bool, RekeyLimit>` (Rulings 11, 20 and 21). Together they keep delivery linear and decide the budget in one place.
  - A clip already on its grid counts as nothing to rekey.
  - `key_fps` is the requested fps only when no clip was declined.
  - `fits_fps`'s grid-cap test uses `resample`'s own formula.
  - Ruling 10's extra scene (250 nodes and one morph target) is there so that the key-term control can fail.
- **Docs.**
  - `3d-format-conversion.md` §3, the glTF bullet.
  - The 3D doc's §3B `joint_naming` bullet and its `mock/animate-3d` row.
  - The spec's glTF section number, §5.27 → §5.28.2.
  - The `mock/animate-3d` row text in `models/mock.yaml`.
- **Late fixes.**
  - A clippy `type_complexity` alias in `tests/animate_api.rs`.
  - One final fix wave of seven Minors, plus text-only residuals (Rulings 22–24). The final review found nothing Critical or Important.
- **Proof on the final tree.** This plan's gate ran combined with item 1's, on one frozen tree.
  - Gate (12:20–12:28 ET, freeze held by every peer): `cargo test --lib` with the 3 load-sensitive tests skipped, 2251/0, including 11 Postgres tests on a throwaway postgres:16-alpine; the 3 alone, 3/3; `--tests`, 54 suites, 3088/0; clippy `-D warnings`, clean. A mutant check (`for_request`'s inherit arm set to `None`) made `a_vrm_rig_named_by_the_vendor_stays_vrm` fail. The tree record, with every changed `src/mesh` file's sha256, is `gate/tree.txt` in the ledger folder.
  - Harness, from the gate's release binary: `--fixtures textured,skinned,morph,cube,metal` × every format, 570/570, with gltf-validator at 0 errors and 0 warnings over 115 runs, assimp 455/455, `usdchecker --arkit` 115/115, and every tool true. Assimp's skinned FBX through `--import` × every format, 114/114. litegen-5c's final `ply.rs` and litegen-05's last `src/mesh` edit were in. Item 11's (litegen-37) were not: only its Tasks 1–2 were.

To commit on the other laptop (other items edit several of the same files):
- `litegen-core/src/providers/mod.rs`: `SourceInput.joint_naming`. Item 1's `Model3dExtras.output_packages` is in the same file.
- `litegen-core/src/api/handlers/mod.rs`: `resolve_source_model`.
- `litegen-core/src/proxy/model3d_deliver.rs`: `for_request`, the rekey loop, `key_fps` and tests. Item 1 edits the same file.
- `litegen-core/src/providers/model3d/mock.rs`: the ANIMATE arm and tests. Item 1's textured mock is in the same file.
- `litegen-core/src/providers/model3d/{fal,tripo,meshy}.rs`: the `SourceInput` literals.
- `litegen-core/tests/animate_api.rs`.
- `litegen-core/src/mesh/glb.rs`: `Ancestry` and the skins loop. Item 1's image-name dedupe is in the same file.
- `litegen-core/src/mesh/matrix_tests.rs`.
- `litegen-core/src/mesh/tests.rs`: the FBX skeleton test. Item 11's uv-set test edit is in the same file.
- `litegen-core/src/mesh/normalize.rs`.
- `models/mock.yaml`: the `mock/animate-3d` row. Item 1's `mock/textured-3d` row is in the same file.
- `docs/superpowers/3d-format-conversion.md`: this item's lines are §3's glTF skeleton sentences.
- `docs/superpowers/plans/2026-10-30-litegen-3d-and-partial-edit-apis.md`: the §3B bullet and the mock row.
- `docs/superpowers/specs/2026-11-04-mesh-review-leftovers-design.md`.

## Global Constraints

- The rules in the overview's "Rules every plan carries" apply: `mesh/` stays standalone, no commits or staging here, one cargo at a time, RED before GREEN, restore by rewrite.
- No new dependency. No OpenAPI change. `SourceInput` and `DeliveryPlan` aren't in the OpenAPI.
- `MAX_RESAMPLE_INTERVALS` (100,000, `mesh/normalize.rs`), `scene::MAX_ANIMATION_KEYS` (20,000,000, `mesh/scene.rs`) and `pose::MAX_RESAMPLED_VALUES` (32,000,000, already `pub` in `mesh/pose.rs`) keep their values. No new constant.
- New note texts, exact:
  - keys: `clip {name} was not rekeyed at {fps} fps: it would hold {keys} keys, over the 20000000-key limit`;
  - values: `clip {name} was not rekeyed at {fps} fps: it would hold {values} values, over the 32000000-value limit`.

  Past the grid cap, keep today's `…: it is too long to key at that rate`. Keys are checked before values.
- Joint-naming inheritance is for `task: animate` only, and an explicit `joint_naming` (`vendor` included) always wins.

## Review Focus

1. A chained animate whose rig row has no recorded plan (a row from before plans were recorded) or one that doesn't parse: it inherits nothing, and nothing panics.
2. A chain across providers (a mock rig, then another provider's animate by data URI): the naming is still inherited, and renames that find nothing to rename are a no-op.
3. A rig whose joints were hoisted or re-parented by `glb::hoist_skinned_meshes` before writing: the common root comes from the written graph, not the input's.
4. Morph-weight channels in the budget: each key counts once against the key limit and target-count times against the value limit, and CUBICSPLINE keys in untouched clips count three times their stride.
5. A rig delivered with `joint_naming: vendor` passes nothing on (no `"joint_naming"` in the lineage record).

---

### Task 1: Re-prove and close a, c and d

**Files:** none changed. Ledger lines only.

- [ ] Run `cargo test --lib -- proxy::model3d_deliver::tests::options_that_cannot_be_applied_are_named_on_the_asset proxy::model3d_deliver::tests::names_stay_unique_after_joint_naming`. Expected: 2 passed.
- [ ] Run `grep -rn joint_renames litegen-core/src`. Expected: no output.
- [ ] Write a ledger line for each of a, c and d with the test name or grep and its result. Send litegen-a3 the three closures for the matrix page data.

### Task 2: The mock animates the rig it holds

**Files:** `litegen-core/src/providers/model3d/mock.rs` (the `ANIMATE_MODEL` arm, `animate_source`).

**Interfaces:**
- Produces: when `source.vendor_task_id` names a stored `RIG_MODEL` job, `mock/animate-3d` animates that job's `meshes[0]` GLB. Otherwise it animates `source.bytes`, as today.

- [ ] Write `mock_animate_by_task_id_animates_the_rigs_own_mesh` in mock.rs tests. Submit `mock/rig-3d` with `joint_naming: Some("unity")`, then `mock/animate-3d` with a `SourceInput` whose `vendor_task_id` is the rig's job id and whose `bytes` are the rig renamed to Unity names (`humanoid::renames_to("unity", …)` applied). Assert the animated GLB's skin joint names are `MOCK_JOINTS`. With `vendor_task_id: None`, assert they're the Unity names.
- [ ] Run it. Expected: FAIL (the chained case returns the Unity names).
- [ ] Implement: in the `ANIMATE_MODEL` arm, when the recorded delivery is `VendorTask`, build a `SourceInput` view over the rig job's stored GLB bytes and pass that to `animate_source`.
- [ ] Run the mock tests and `cargo test --test animate_api`. Expected: the new test passes. In `animate_api`, any test that relied on the mock animating renamed bytes now fails. List those, because they're the bug made visible, and Task 3 turns them green. Expected candidates: none today. Record what you see.

### Task 3: A chained operation inherits its rig's joint naming

**Files:**
- `litegen-core/src/providers/mod.rs` (`SourceInput`)
- `litegen-core/src/api/handlers/mod.rs` (`resolve_source_model`)
- `litegen-core/src/proxy/model3d_deliver.rs` (`DeliveryPlan::for_request`)
- every `SourceInput { … }` literal (find them with `grep -rn "SourceInput {" litegen-core/src litegen-core/tests`)
- `litegen-core/tests/animate_api.rs`

**Interfaces:**
- Produces: `SourceInput.joint_naming: Option<String>`. This is the scheme the source rig was delivered in, from the chained rig row's `metadata.delivery.joint_naming`, and `None` otherwise. It's recorded for any chained source.
- Produces: lineage record key `"joint_naming"`, only when it is `Some`.
- Produces: `DeliveryPlan::for_request` sets `joint_naming` by this rule:
  - `Some(n)` in the request → `(n != "vendor").then(n)`;
  - `None`, and `schema.task == ModelTask::Animate` → `extras.source.and_then(|s| s.joint_naming.clone())`;
  - otherwise → `None`.

- [ ] Write the tests in `tests/animate_api.rs`, all on the mock through the harness:
  - `an_animation_chained_from_a_named_rig_keeps_its_joint_names`: rig with `joint_naming: "unity"`, animate with `source_model: {type: generation, value: rig_id}`, both polled to `completed`. Assert:
    - the animated GLB's joint names (read with `mesh::read_scene`) `==` the rig GLB's, in order;
    - the mesh asset's `scene.skin.joints` and `scene.skin.humanoid` equal the rig's;
    - the row's `metadata.delivery.joint_naming == "unity"` and `metadata.source.joint_naming == "unity"`.
  - `a_chain_of_animations_keeps_the_rigs_names`: rig (unity), then A, then B chained from A. B's joints `==` the rig's.
  - `a_rig_delivered_with_vendor_names_passes_nothing_on`: rig without `joint_naming`, then animate. The joints `==` `MOCK_JOINTS`, `metadata.delivery` has no `joint_naming`, `metadata.source` has no `joint_naming`, and no manifest note mentions naming.
  - `a_vrm_rig_named_by_the_vendor_stays_vrm`: rig `joint_naming: "vrm"` (applied natively by the mock). The animation's joints are `VRM_JOINTS` and equal the rig's.
  - `the_poller_delivers_the_inherited_naming`: the unity chain, where only the background poller completes the animate row. The joints equal the rig's.
  - `a_chain_from_a_row_without_a_plan_inherits_nothing` (Review Focus 1): remove `metadata.delivery` from the rig row in the DB before animating. The animation is delivered with the mock's names, and the request succeeds.
  - `an_explicit_vendor_naming_is_not_overridden`: an operator animate row (a runtime model added through the admin API) that takes `joint_naming`, asked `vendor` on a unity rig. `metadata.delivery` has no `joint_naming`, and the joints are the mock's.
  - `only_animation_inherits_the_naming`: `mock/texture-3d` chained from the unity rig. `metadata.delivery` has no `joint_naming`, and no naming note is written.
- [ ] Run `cargo test --test animate_api`. Expected: FAIL on the unity, chain and poller cases (mock names).
- [ ] Implement the three interface items. In `resolve_source_model`, read `chain_from`'s `metadata["delivery"]["joint_naming"]` as a string.
- [ ] Run `cargo test --test animate_api --test model3d_chain --test model3d_api` and `cargo test --lib proxy::model3d_deliver`. Expected: pass.
- [ ] Negative control: in a scratch copy, remove the `else extras.source…` fallback and run `an_animation_chained_from_a_named_rig_keeps_its_joint_names`. Expected: FAIL with mock names. Restore by rewriting the file (`touch` it before rerunning), and record RED and GREEN in the ledger.

### Task 4: glTF writes the joints' closest common root

**Files:** `litegen-core/src/mesh/glb.rs` (the skins loop in the writer), `litegen-core/src/mesh/tests.rs`, `litegen-core/src/mesh/matrix_tests.rs`.

**Interfaces:**
- Produces: private `fn closest_common_root(scene: &Scene, joints: &[usize]) -> Option<usize>` in glb.rs. It returns the deepest node that is an ancestor-or-self of every joint, or `None` when they share none.

- [ ] Write in glb.rs tests:
  - `glb_writes_the_closest_common_root_when_the_scene_names_no_skeleton`, with three cases: the `skinned` fixture with `skeleton = None` gives the root joint's index; joints starting at two sibling joints under a non-joint `Armature` give `Armature`; two separate root trees give no `skeleton` key.
  - `a_skeleton_the_scene_names_is_written_unchanged`.
- [ ] Write `fbx_skins_reach_glb_with_a_skeleton` in tests.rs: `testdata/fbx/skinned_small_assimp_7500.fbx` → GLB JSON `skins[*].skeleton` is present and an ancestor-or-self of every joint.
- [ ] Add a check to `matrix_tests` that every written glTF skin's `skeleton`, when present, is an ancestor-or-self of each of its joints.
- [ ] Run `cargo test --lib -- mesh::glb mesh::tests::fbx_skins_reach_glb_with_a_skeleton`. Expected: FAIL (no `skeleton` key).
- [ ] Implement `closest_common_root` and use it in the skins loop when `s.skeleton` is `None`. The same loop serves `write_gltf_scene`.
- [ ] Run `cargo test --lib mesh::`. Expected: pass.
- [ ] Negative control: return a child of the common root. The matrix check fails. Restore by rewrite.
- [ ] Run the harness on `--fixtures skinned --formats glb,gltf,fbx` with the validator. Expected: 0 errors, 0 warnings, and verdicts unchanged.

### Task 5: The resampling budget (keys and values)

**Files:** `litegen-core/src/mesh/normalize.rs` (`fits_fps`, `resample`, new `rekeyed_size` and `RekeyedSize`), `litegen-core/src/proxy/model3d_deliver.rs` (the rekey notes).

**Interfaces:**
- Produces: `pub struct RekeyedSize { pub keys: usize, pub values: usize }` and `pub fn rekeyed_size(scene: &Scene, anim: usize, fps: u32) -> Option<RekeyedSize>` (`None` past the grid cap).
  - `keys` = the clip's channel count × its new key count, plus every other clip's existing keys.
  - `values` = Σ keys × stride, where stride is the component count: 3 for translation and scale, 4 for rotation, the target count for weights. CUBICSPLINE keys in untouched clips count ×3. The rekeyed clip becomes linear.
- Produces: `fits_fps` = `rekeyed_size(..)` is `Some(s)` with `s.keys <= scene::MAX_ANIMATION_KEYS && s.values <= pose::MAX_RESAMPLED_VALUES`. A clip with no time range, or `fps == 0`, still fits.

- [ ] Write `rekeyed_size_counts_every_channel`. 3,000 nodes, each with a 2-key translation from 0 to 3,000 s, at 30 fps (90,000 intervals) gives `keys == 270_003_000` and `values == 810_009_000`. With a second clip of 500 translation keys, `keys == 270_003_500`. The same scene from 0 to 3,600 s (108,000 intervals) gives `None`.
- [ ] Write `rekeying_past_the_key_budget_is_declined_without_allocating`: on the 3,000-channel scene, `fits_fps` is `false`, `resample` is `false`, and the scene equals its clone from before. It's over both limits, and keys are checked first.
- [ ] Write `rekeying_past_the_value_budget_is_declined`: a 3-vertex mesh with 1,024 morph targets and one 2-key weights channel from 0 to 2,000 s, at 30 fps. That's 60,001 keys and 61,441,024 values, so `fits_fps` is `false`, `resample` is `false`, and the scene is unchanged.
- [ ] Write `rekeying_within_the_budget_still_rekeys`: 100 channels over 10 s at 30 fps, every channel 301 keys.
- [ ] Write `the_notes_name_the_budget_that_failed` in model3d_deliver tests, with `fps: Some(30)`. The 3,000-channel GLB gives `fatal` `None` and a note containing `over the 20000000-key limit`. The 1,024-target GLB gives `over the 32000000-value limit`. The existing `u32::MAX` case keeps `too long to key at that rate`.
- [ ] Run them. Expected: FAIL (`rekeyed_size` is undefined; once it's stubbed, `fits_fps` is `true`).
- [ ] Implement. Delivery chooses the note from `rekeyed_size`.
- [ ] Run `cargo test --lib -- mesh::normalize proxy::model3d_deliver`. Expected: pass.
- [ ] Negative controls, one per term: (a) drop the key term from `fits_fps`, and the key test fails at its `fits_fps` assertion; (b) drop the value term, and the value test fails. Restore by rewriting each time.

### Task 6: Gate and docs

**Files:**
- `docs/superpowers/3d-format-conversion.md` §3 (the glTF writer bullet: "a skin the scene names no skeleton for gets its joints' closest common root")
- the 3D doc's §3B joint-naming paragraph (`docs/superpowers/plans/2026-10-30-litegen-3d-and-partial-edit-apis.md` holds it; add "a chained animation keeps its rig's joint names")
- `docs/superpowers/specs/2026-11-04-mesh-gaps-overview-design.md` (mark item 12 closed)

- [ ] Run the full gate from the overview.
- [ ] Write the ledger: each task's RED→GREEN with test names, each negative control's result, and the gate's counts.
