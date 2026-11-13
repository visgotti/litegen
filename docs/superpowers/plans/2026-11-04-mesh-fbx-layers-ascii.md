# FBX Layers and ASCII Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The FBX reader blends every animation layer of a stack into its clip, by ufbx's rules. On request (`fbx_encoding=ascii`, D5), the writer emits ASCII FBX 7.4 from the same record tree.

**Architecture:** Layers are evaluated per node and channel at the union of layer key times plus the 30 fps grid, combined in stack order by each layer's weight, blend mode and accumulation modes (ufbx v0.23.1's reconstruction), and stored as linear quaternion and TRS keys. ASCII is a second serialiser of `fbx_dom`'s `WRec` tree.

**Tech Stack:** Rust (`mesh/fbx_read.rs`, `fbx_dom.rs`, `fbx.rs`, `mod.rs`), an offline C program over ufbx v0.23.1 for the reference values, and the Node harness.

**Spec:** `docs/superpowers/specs/2026-11-04-mesh-fbx-layers-ascii-design.md`

## Global Constraints

- The overview's "Rules every plan carries" apply.
- **Every "Negative control" step below is void** (2026-11-09: the user banned mutation
  testing in the global `CLAUDE.md`). Prove each test red before the change and green after.
  For a test of behaviour that already exists, record which regression it would catch.
- The layer rules are ufbx v0.23.1's, exactly as the spec lists them (`ufbx.c` L22221–22251 and L25711–25817), copied into `fbx_read.rs`'s module doc with those citations.
- Where the SDK docs differ (base-layer weight or mode, pure-Override weight, `Mute`/`Solo`/`BlendModeBypass`), litegen follows ufbx and warns, naming the layer and property.
- A one-layer stack reads byte-identically to today. Hash every existing FBX test file's read scene before Task 1.
- The layer blend's times come from `pose::resample_times` (100,000-step cap). Its keys are charged to `fbx_read.rs`'s existing `MAX_ANIMATION_KEYS` accounting before allocation.
- D5 is settled: ASCII is built as an opt-in, `fbx_encoding: "binary" | "ascii"` (default `binary`). It's a converter query parameter (only with `to=fbx`) and a 3D request field, recorded in the `DeliveryPlan`. Keep-nothing with `ascii` is 400 `option_needs_keeping`. Ping the frontend/SDK session with the shape before Task 4.

## Review Focus

1. A layer whose curve node targets a property the base layer doesn't animate: in override mode at weight < 100, the static value is blended with the layer by weight (or not, per the cited rule). Pinned by a test.
2. Rotation with pre- or post-rotation and rotation order on the node: layers blend the `Lcl Rotation` values before the transform stack applies, never the composed matrix.
3. A stack where every layer is muted: ufbx doesn't read `Mute`, so the layers still blend as ufbx blends them, and the warning names the muted layers. The clip still exists.
4. Layered blend-shape `DeformPercent` with in-between shapes: the blended weight drives the in-between interpolation exactly as an unlayered weight does.
5. Very different key densities across layers (a 120 fps mocap base under a 3-key override): the union times plus the grid keep the dense layer's keys exactly.

---

### Task 1: Layer evaluation

**Files:** `litegen-core/src/mesh/fbx_read.rs` (`stack_animation`, a new `blend_layers`, and the layer properties), and `docs/superpowers/research/fbx/scripts/ufbx_eval.c` (new, offline: evaluates a file's stacks with ufbx v0.23.1 at given times and prints world TRS per node).

**Interfaces:**
- Produces: `struct LayerProps { weight: Curve | f64, mute: bool, solo: bool, blend: BlendMode, rotation: RotationAccumulation, scale: ScaleAccumulation }` (private).
- Produces: `fn blend_layers(&mut self, stack: i64, layers: &[i64]) -> Result<(), MeshError>` (private), called by `stack_animation` when more than one layer contributes.

- [ ] Write the hand-built stack tests from the spec: `two_additive_layers_add`, `an_override_layer_replaces`, `override_passthrough_lerps_by_weight`, `rotation_by_layer_composes_quaternions`, `scale_composes_and_adds_by_mode`, `mute_solo_and_bypass_are_warned_not_applied`, `a_base_layer_weight_is_warned_and_ignored`, `an_animated_layer_weight_is_evaluated_per_key`, `blend_shape_weights_blend_by_layer`. Add Review Focus 1–5 as `an_unanimated_base_property_blends_by_ufbx_rule`, `layers_blend_before_the_transform_stack`, `all_muted_layers_still_blend_as_ufbx_does` (ufbx ignores `Mute`; the warning names it), `layered_deform_percent_drives_inbetweens` and `dense_and_sparse_layers_keep_every_dense_key`.
- [ ] Write `a_single_layer_stack_is_byte_identical` (against the recorded hashes) and `layer_resampling_respects_the_budget`.
- [ ] Run `cargo test --lib mesh::fbx`. Expected: FAIL (only the first layer is read).
- [ ] Implement.
- [ ] Run `cargo test --lib -- mesh::fbx mesh::scene_matrix_tests`. Expected: pass.

### Task 2: Reference files against ufbx

**Files:** `litegen-core/src/mesh/testdata/fbx/layers/` (the ufbx corpus files the spec names, committed only if ufbx's `data/` licence allows, else read from a local path given by an env var in an `#[ignore]`d test that the ledger runs), and the committed expected values as a `const` table in `fbx_read.rs` tests.

- [ ] Take ufbx's own expectations from `test/test_animation.h` (L1224–1265 for the `maya_anim_layers_*` files; L2788 and L2794 for `maya_anim_layer_anim_7700_ascii.fbx`: −1.0 / 22.5° at frame 6, 0.0 / 45° at frame 12).
- [ ] Build `ufbx_eval.c` once (`cc -O2 ufbx_eval.c ufbx.c -lm`), and record 30 more sample times per file. Commit only the numbers, with the file names and the ufbx commit.
- [ ] Write `layered_files_match_ufbx`: at ufbx's own expectations, matched as its tests compare them, and within 1.1e-3 of model size at the extra times. Don't use frame 18 of the orders file (it equals the export pose).
- [ ] Run it. Expected: pass. If it doesn't, fix the rules, not the tolerance.
- [ ] Negative control: treat passthrough as additive. `override_passthrough_lerps_by_weight` and the `_over` files fail. Restore by rewriting.

### Task 3: ASCII serialiser

**Files:** `litegen-core/src/mesh/fbx_dom.rs` (`write_records_ascii`), `litegen-core/src/mesh/fbx_write.rs` (`write` takes the encoding), `litegen-core/src/mesh/fbx.rs` (`write_scene` signature), `litegen-core/src/mesh/mod.rs` (`WriteOptions.fbx_encoding`, `pub enum FbxEncoding { #[default] Binary, Ascii }`).

**Interfaces:**
- Produces: `pub(super) fn write_records_ascii(top: &[WRec], out: &mut Vec<u8>) -> Result<(), MeshError>`.

- [ ] Write `ascii_reads_back_as_binary_does` (every fixture), `ascii_key_attr_floats_are_bit_patterns`, `ascii_names_use_class_separator`, `ascii_embedded_textures_are_base64` and `ascii_declares_fbx_version_7400` (`FBXHeaderExtension { FBXVersion: 7400 }`, which three.js reads).
- [ ] Run `cargo test --lib mesh::fbx`. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test --lib mesh::`. Expected: pass. The binary output is byte-identical to before: compare with the matrix hashes.
- [ ] Negative control: decimal `KeyAttrDataFloat`. The bit-pattern test fails. Restore by rewriting.

### Task 4: The option on the converter and on generations, and the harness

**Files:**
- `litegen-core/src/api/handlers/convert.rs` (`PARAMS`, `parse_query`, the OpenAPI param naming who reads ASCII and that Blender's legacy importer refuses it)
- `litegen-core/src/types/mod.rs` (the request field `fbx_encoding`)
- the 3D request validation, `litegen-core/src/api/handlers/mod.rs` (the keep-nothing refusal), and `litegen-core/src/proxy/model3d_deliver.rs` (`DeliveryPlan.fbx_encoding`, used when writing FBX)
- the convert tests and `litegen-core/tests/model3d_api.rs`
- `docs/mcp/tools.md` (regenerated)
- `litegen-core/examples/mesh_matrix.rs` and `scripts/mesh-verify/verify.mjs` (the `fbx:ascii` variant)

- [ ] Ping the frontend/SDK session. Write the converter tests (an `ascii` body starts `; FBX 7.4.0 project file`; `to=glb&fbx_encoding=ascii` gives 400 `param_unsupported`; a bad value gives 400 `param_invalid`) and the generation tests from the spec (ASCII FBX beside an unchanged GLB, the default byte-identical, the keep-nothing refusal, the poller path).
- [ ] Run them. Expected: FAIL.
- [ ] Implement, then regenerate `docs/mcp/tools.md`.
- [ ] Run the harness `fbx:ascii` variant. Expected: three.js matches the binary FBX's picture at 0.00 %, and assimp imports every file.

### Task 5: Docs and gate

- [ ] Update the docs listed in the spec. Send litegen-a3 the matrix changes.
- [ ] Run the overview's full gate and the harness FBX rows, including `fbx:ascii`. Write the ledger.
