# UV Sets in USD and Collada Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** USDA and Collada write up to eight UV sets. The USD reader numbers sets by name. `degrade` keeps the sets textures use, so USDZ (two sets) and the one-set containers keep textures that sit on later sets.

**Architecture:** A name-based set list in `usd_read.rs`, a texture-aware set selection in `pose::degrade` (a per-material remap of `TextureRef.uv_set`), and `uv_sets` raised to 8 for USDA and Collada with the writers' per-set loops.

**Tech Stack:** Rust (`mesh/usd*.rs`, `mesh/dae.rs`, `mesh/pose.rs`), the Node harness (a set selector in the `uv` mode; `--arkit` on USDZ only).

**Spec:** `docs/superpowers/specs/2026-11-04-mesh-uv-sets-usd-dae-design.md`

**Status: done on 2026-11-04 by session litegen-b2, finishing litegen-37's work, not
committed.** litegen-37 built Tasks 1–4 and dispatched the final review, then ended.
litegen-b2 (item 4's session) re-ran the final review, ran its one fix wave, and ran the full
gate and the closing harness. The ledger, `.superpowers/sdd/2026-11-04-mesh-uv-sets-usd-dae/progress.md`,
records every ruling with its cost if wrong. The build differs from the steps below in
these places:

- **Order.** The tasks ran T1, T2, T3b, T3, T4 (P1), because Task 3's Collada test asserts
  Task 3b's order.
- **`degrade`'s set order counts only the slots the target keeps** (P3a; the spec was
  amended). The permutation renumbers every binding (P3d), so a map that's dropped later
  still reads its own coordinates.
  - The Collada test expects [3, 0, 1, 2] (P3b; spec amended).
  - Textureless targets keep their first k sets (P3c).
  - Collada and FBX reorder after their last binding drop (P3f).
  - A degenerate material is never remapped (P5, P5a).
- **3MF is in scope** (P4; spec amended). It keeps the base colour's set through
  `order_sampled_sets_first`, which moved from Task 3b to Task 2.
- **The USD reader** makes one material copy per distinct set layout (T1a). It keeps
  today's handling of a name a material reads that the mesh lacks (T1b).
- **Tests beyond the plan:**
  - `a_dropped_map_keeps_its_own_coordinates`;
  - `a_huge_uv_set_index_is_renumbered_at_once`;
  - `a_dropped_image_never_takes_set_zero`;
  - the hostile-input sweeps `uv_set_inputs_survive_truncation_and_bit_flips` (P7);
  - from the fix wave, `collada_puts_an_emissive_only_set_first` and
    `fbx_puts_an_emissive_or_normal_set_first`, which pin the per-codec slot lists.
- **The harness** ran every fixture (P9a), not the plan's three, and fetched MultiUVTest
  into the ledger's `import/` folder (P11).
  - A BlendShape USD's uv sets get a cited skip (T4a).
  - `scripts/mesh-verify/scenekit/render.swift` changed too (a texture-channel binding),
    though the plan's file list doesn't name it.
  - Since the fix wave, a SceneKit comparison error fails its case.
- **`tests.rs`** (item 12's file) changed one test, with its owner's agreement:
  `a_texture_on_a_dropped_uv_set_is_reported_as_a_lost_texture` now puts an emissive
  texture on set 1.
- **The final review** found no correctness defect. Its one fix wave corrected the doc's
  SceneKit and `--arkit` claims (I1) and five Minors (M2–M6). Its Minor M1 (the USD reader
  re-reads sets per GeomSubset) and the ledger's Rulings F2 and D1–D7 are follow-ups for
  the user.

Proof, on tree ba778e7 (the final tree):

- **Full gate:** lib 2266 passed and 0 failed (the three load-sensitive tests skipped);
  those three alone 3/3; `--tests` 54 suites, 3104 passed, 0 failed; clippy clean.
- **Every-fixture harness:** 798/798, with every tool present and 0 native failures. Of
  those, 617 cases carry uv sets, and every set is judged by three.js or SceneKit or
  skipped with a cited source line. There are 0 verdict changes against Task 4's run.
- **MultiUVTest `--import`:** 228/228, with 0 native failures.

To commit on the other laptop:

- `litegen-core/src/mesh/`:
  - `dae.rs`, `fbx.rs`, `fixtures.rs`, `pose.rs`, `scene.rs`, `scene_matrix_tests.rs`,
    `threemf.rs`, `usd.rs`, `usd_read.rs` and `usd_write.rs`;
  - `mod.rs` (item 11's lines are the uv-set rows; other items edit the same file);
  - `tests.rs` (one test; item 12's file);
- the untracked `litegen-core/src/mesh/testdata/usd/multiuv.usdc`, and
  `testdata/usd/README.md`. The USD tests won't build without `multiuv.usdc`;
- `scripts/mesh-verify/`: `verify.mjs`, `page/render.js`, `scenekit/render.swift` and
  `README.md`;
- `docs/superpowers/3d-format-conversion.md` (item 11's lines are in §3–§8; other items
  edit the same file);
- this plan and its spec, both untracked.

## Global Constraints

- The overview's "Rules every plan carries" apply.
- Caps: USDZ 2, USDA (and USDC when item 6 lands) 8, Collada 8.
- The USD reader's set order: `st`, `st1`…`st7` present, in suffix order with gaps closed, then other material-read primvars in first-reference order, up to 8.
- `degrade`'s order: the sets sampled through the slots the target keeps, in first-use order, then the rest by index. The slot order is base colour, metallic-roughness, normal, occlusion, emissive, then layer slots if item 4 has landed, restricted to the target's material model. A target that keeps no textures keeps the first k. (Amended from the build: rulings P3a and P3c.)

## Review Focus

1. A primitive with no material and more sets than the target holds: keep the first k, as today.
2. A texture transform on a remapped set (`pose::bake_texture_transforms`): it bakes into the set's new index, not its old one.
3. USD `primvars:st2:indices` (indexed primvars) in third-party input: values and indices stay paired after the renumbering.
4. Two materials on one mesh whose sampled sets differ: each primitive is remapped by its own material, and they don't interfere.
5. A material whose only texture is on set 5, converted to a 2-set USDZ: the texture moves to set 0, and nothing is reported lost except the extra sets.

---

### Task 1: Fixtures and the USD reader's numbering

**Files:** `litegen-core/src/mesh/fixtures.rs` (`multiuv()`, `multiuv_shared()`, registered in `all()`), `litegen-core/src/mesh/usd_read.rs` (`default_uv_names`, the `uv_names` assembly, `TexRead`'s index).

**Interfaces:**
- Produces: `fn uv_set_names(&self, p: usize, material_reads: &[String]) -> Vec<String>` in usd_read.rs. It replaces the ad hoc assembly.

- [ ] Write `usd_sets_are_numbered_by_name`, `a_gap_in_the_suffixes_closes`, `uvmap_still_reads`, `a_material_reading_only_st1_keeps_both` and `indexed_extra_primvars_stay_paired` (Review Focus 3). Use hand-written USDA in the tests.
- [ ] Run `cargo test --lib mesh::usd`. Expected: FAIL (the first two give two sets, or the wrong indices).
- [ ] Implement.
- [ ] Run `cargo test --lib -- mesh::usd mesh::usda mesh::usdc` and the existing USD third-party tests (`testdata/usd/`). Expected: pass, and every existing USD test is unchanged.

### Task 2: `degrade` keeps sampled sets

**Files:** `litegen-core/src/mesh/pose.rs` (`degrade`'s uv-set reduction, the remap, `bake_texture_transforms` interplay), `litegen-core/src/mesh/scene.rs` (`losses`: the "texture binding(s) on uv set N or later" count uses the remapped indices), `litegen-core/src/mesh/threemf.rs` (`prepare` and `losses` reorder through `order_sampled_sets_first`; ruling P4).

**Interfaces:**
- Produces: `pub fn sampled_set_order(m: &Material) -> Vec<u32>` in pose.rs (first-use order of a material's sets).
- Produces: `pub fn order_sampled_sets_first(…)` in pose.rs, moved here from Task 3b (ruling P4). It takes the target's kept-slot list (ruling P3a), which also restricts `sampled_set_order`. The ledger has the final signatures.

- [ ] Write:
  - `degrade_keeps_sampled_sets` (GLB base colour on `TEXCOORD_1` → OBJ package keeps the texture);
  - `shared_materials_share_the_remap`;
  - `a_material_sampling_three_sets_loses_one_in_usdz`;
  - `a_materialless_primitive_keeps_its_first_sets` (Review Focus 1);
  - `a_transform_on_a_remapped_set_bakes_into_its_new_index` (Review Focus 2);
  - `two_materials_on_one_mesh_remap_independently` (Review Focus 4);
  - `a_lone_texture_on_set_five_moves_to_set_zero` (Review Focus 5);
  - `a_phong_target_keeps_the_set_of_a_map_it_keeps` (ruling P3a);
  - `threemf_keeps_the_base_colour_set` (ruling P4).
- [ ] Run `cargo test --lib -- mesh::pose mesh::scene mesh::scene_matrix_tests`. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test --lib mesh::`. Expected: pass. Fixture outputs whose textures sit on set 0 are byte-identical to before. Check with the matrix hashes taken before this task.
- [ ] Negative control: make `degrade` keep the first k sets. `degrade_keeps_sampled_sets` fails. Restore by rewriting.

### Task 3: USDA and Collada write eight sets

**Files:** `litegen-core/src/mesh/usd.rs` (`capabilities`: `uv_sets` 2 for USDZ and 8 otherwise; doc), `litegen-core/src/mesh/usd_write.rs` (drop `take(2)`; cap at the capability), `litegen-core/src/mesh/dae.rs` (`UV_SETS = 8`; doc), `litegen-core/src/mesh/testdata/usd/multiuv.usdc` with its README line (written by `usdcat` from the test's USDA).

- [ ] Write `usda_writes_and_reads_every_uv_set`, `usdc_reads_every_uv_set`, `usdz_keeps_the_two_sets_its_textures_use`, `dae_writes_and_reads_every_uv_set` and `nine_sets_still_report_one_lost`.
- [ ] Run `cargo test --lib -- mesh::usd mesh::dae`. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test --lib mesh::`. Expected: pass.
- [ ] Negative control: write set 3's data into set 2 in the Collada writer. The Collada test fails. Restore by rewriting.

### Task 3b: Collada and FBX put the sampled sets first

**Files:** `litegen-core/src/mesh/dae.rs` (`prepare`), `litegen-core/src/mesh/fbx.rs` (`prepare`).

**Interfaces:**
- Consumes: `pose::order_sampled_sets_first` from Task 2. `dae::prepare` passes Collada's kept slots (base colour, emissive) and `fbx::prepare` passes FBX's (base colour, normal, emissive).

- [ ] Record the hashes of every fixture's FBX and DAE output before the change.
- [ ] Write `fbx_and_dae_put_the_base_colour_set_first` (unit: the written sets and bindings; harness: three.js draws the texture), and `a_scene_textured_on_set_zero_is_unchanged` against the hashes.
- [ ] Run `cargo test --lib -- mesh::dae mesh::fbx`. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test --lib mesh::`. Expected: pass.

### Task 4: Harness, docs and gate

**Files:** `scripts/mesh-verify/page/render.js` (the `uv` mode's set selector), `scripts/mesh-verify/verify.mjs` (per-set cases; cited skips for sets ≥ 2 in three.js `ColladaLoader`/`USDLoader`; `usdchecker --arkit` on USDZ only, plain `usdchecker` on USDA and USDC), `scripts/mesh-verify/README.md`, and the docs listed in the spec.

- [ ] Implement the set selector and the checker split.
- [ ] Run `cargo run --release --example mesh_matrix -- <dir> --fixtures multiuv,textured,skinned` and `node verify.mjs <dir> <report>`. Expected: 0 failures. usdrecord draws the `st3` texture. USDZ draws both textures in three.js, SceneKit and usdrecord. The checkers report 0 errors.
- [ ] Run `--import` with Khronos `MultiUVTest` (local only). Expected: it passes as before.
- [ ] Update the docs. Run the overview's full gate. Write the ledger.
