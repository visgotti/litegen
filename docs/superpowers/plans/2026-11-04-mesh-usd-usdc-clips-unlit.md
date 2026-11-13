# USD: USDC, Several Clips, Unlit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** litegen writes USDC crate files (0.8.0), keeps every skeletal and blend-shape clip in USD as Apple's `Animation` variant set on the default prim, and writes unlit materials that USD readers draw unlit and litegen reads back as unlit.

**Architecture:** `usdc::write` encodes the `Layer` that `usda::parse` makes of our own USDA text, with literal-only LZ4 and integer-coded sections and uncompressed arrays. The USD writer authors clips as variants, and the reader composes once per variant for that one convention. Unlit is a UsdPreviewSurface recipe with a litegen `customData` marker.

**Tech Stack:** Rust (`mesh/usdc.rs`, `usda.rs`, `usd_write.rs`, `usd_read.rs`, `usd_stage.rs`, `usd.rs`, `mod.rs`); the installed Apple USD Tools 0.25.2 (`usdcat`, `usdchecker`, `usdrecord`; there's no `usddiff`) in the harness.

**Spec:** `docs/superpowers/specs/2026-11-04-mesh-usd-usdc-clips-unlit-design.md`

**Status (2026-11-10, litegen-fc): done and not committed. The user skipped the AR Quick
Look device check (Task 5), so USDZ's `usd_clips` default stays `first`.** The ledger is
`.superpowers/sdd/2026-11-04-mesh-usd-usdc-clips-unlit/progress.md`. It holds every ruling
(U1–U22) and every parked follow-up.

- **Gate, on tree 533b7f8** (`gate/` in the ledger folder):
  - the lib: 3,098 passed and 0 failed;
  - the nine load-sensitive tests, run alone: 9/9;
  - every test target, with Postgres: 57 binaries, 4,020 passed, 0 failed, and `no-skips.sh` clean;
  - clippy `--all-targets --all-features -D warnings`: clean;
  - the ignored full USDC truncation and bit-flip sweep over every fixture: 1/1.
- **Harness, on the same tree** (`harness/`; report `task-h-report.md`):
  - glb, usda, usdc and usdz with `--usd-variants`: 444/444; every format: 2,232/2,232;
  - usdcat: clean on 384/384 files;
  - usda against usdc: 72/72 identical texts, and 711/711 identical usdrecord pictures;
  - clips: 18 files, 54 variants, 378 usdrecord frames with 0 differing pixels, and usdchecker reported all 54;
  - unlit under the distant light: 0 differing pixels in usdrecord, SceneKit and three.js.
- **Deviations, by ruling:**
  - one harness task after the code (U3, U19);
  - half scales rounded at print (U13b);
  - litegen's camera and light numbers written as doubles (U15);
  - comparisons at the declared precision (U16, U18, U18b);
  - a request's bad word is `malformed_request` (U21).
- **The user's ruling, U22:** usdrecord's unlit invariance is judged under the distant
  light alone. The dome residual is recorded as a measurement: at most 6/255, from Storm's
  dome specular, `previewSurface.glslfx:327`.
- **Follow-up pass, 2026-11-10 afternoon** (the user's "ok go"; the ledger's "2026-11-10
  follow-up pass", rulings U23–U30). It fixed five parked findings, plus what the work and
  its review turned up. Each new test was run red first.
  - The USD reader no longer costs the square of a crafted file:
    - joint parents come from a sorted pass;
    - the skeleton, `skel:joints` and blend-shape tables are built once;
    - a SkelAnimation shared by many skeletons is read once;
    - the animation-name scan is gone;
    - joint and blend-shape keys are charged to the budget before they're copied.
  - The writer finds half-scaled joints in one pass. Joint scales past 65,504 (clamped in
    f64) are a `skins` loss, judged over the clips kept, and the crate writer refuses a number
    past its type.
  - A mistyped unlit marker and an animation that animates nothing are named in the warnings.
  - The harness fails a file that should hold several clips but has fewer than two variants.
  - CI's `TIMING_TESTS` lists the gate's eleven timing tests.
  - Reviewed by fable: 0 Critical; I1, I2, M1 and M2 fixed; M3 (timing margin) noted.
  - Gate on tree 2a89978:
    - the lib: 3,106 passed and 0 failed;
    - the eleven timing tests alone: 11/11;
    - every target with Postgres: 57 binaries, 4,028 passed, 0 failed, no-skips clean;
    - clippy: clean;
    - the full USDC sweep: 1/1 (337 s).
  - The release harness wasn't rerun. No fixture's output changes, and the new verify.mjs
    check is unit-tested (32/32).
- **Known limits:**
  - SceneKit plays every SkelAnimation at once, so its other variants are cited skips.
  - three.js's USDCParser misreads an empty array in a crate (a cited skip).
  - usdrecord's unlit base colour is 7.61 % against the 8 % threshold.
  - The four device questions in `3d-format-conversion.md` §5.
- **Open:**
  - The device check: the user skipped it on 2026-11-10. Its three USDZs stay in the
    ledger's `device-check/` folder for a later check, all passing `usdchecker --arkit`:
    - `1-clips-walk-run-wave.usdz` (`usd_clips=all`);
    - `2-textured-usdc-root.usdz` (`usdz_root=usdc`);
    - `3-textured-plain.usdz`.
    If the multi-clip file ever passes, flip `USDZ_DEFAULT_CLIPS` to `All`, with its test.
  - The parked follow-ups the follow-up pass left (the ledger's `Final: parked` and
    `minor (deferred)` lines; that pass's entry says which it closed).
- **Files to commit:** the ledger's `files-to-commit.txt`, 52 paths.
  - Shared files also carry other sessions' work, so commit them with care: README.md,
    `api/handlers/mod.rs`, `proxy/{router,poller,config_conformance}.rs`, the providers,
    `docs/superpowers/3d-format-conversion.md`, and the CI files (`.github/workflows/ci.yml`,
    `docs/ci.md`; this pass changed only the timing-test list).
  - litegen-d0's SDK and landing regeneration, and its converter package, are its own.

## Global Constraints

- The overview's "Rules every plan carries" apply. Start after items 11 and 4.
- **Every "Negative control" step below is void** (2026-11-09: the user banned mutation
  testing in the global `CLAUDE.md`). Prove each test red before the change and green after.
  For a test of behaviour that already exists, record which regression it would catch.
- The crate version is exactly 0.8.0. Arrays are written uncompressed. LZ4 is literal-only.
- Clips follow Apple's layout ([research-consumers.md](../research/research-consumers.md) Q1):
  - one SkelAnimation per clip, outside the variants;
  - variant set `Animation` on the default prim, whose variants author only `skel:animationSource`;
  - rigid (xformOp) animation for the first clip only, outside the variants as today, with the other clips' rigid motion reported lost;
  - the first clip selected;
  - nothing a variant varies authored outside the variant set;
  - `endTimeCode` = the longest clip.
- Unlit is `diffuseColor` 0, `metallic` 1, `roughness` 1, `emissiveColor` = base (texture `rgb` with `scale` = factor), plus `customData.litegen.unlit` ([research-consumers.md](../research/research-consumers.md) Q2). No MaterialX output.
- D4 is settled: `usdc` is a public format, `usdz_root=usda|usdc` (default `usda`), and `usd_clips=all|first` (default `all` for USDA and USDC, and `first` for USDZ until the device check passes). Each is a converter query parameter and a 3D request field, recorded in the `DeliveryPlan`. Keep-nothing with a non-default value is 400 `option_needs_keeping`.
- The OpenAPI changes (`Model3dFormat` adds `usdc`, and the two options) go to the frontend/SDK session first.

## Review Focus

1. Dictionary-valued fields (`customData`, `customLayerData`) with nested dicts and mixed value types: encoded with the exact crate value types, so `usdcat` prints the same values.
2. Empty arrays, and attributes with both a default and time samples (litegen writes both for animated values): both encoded, and the reader keeps both.
3. A clip whose channels animate only blend-shape weights: its SkelAnimation holds only `blendShapeWeights`, and the joints stay at the rest pose (UsdSkel's rule for an incomplete SkelAnimation, which the reader already follows).
4. Two clips with the same sanitised name ("walk" and "Walk!"): distinct variant names, with the originals restored from customData.
5. An unlit material with an alpha texture or a vertex colour: the emissive path carries the colour, opacity stays connected, and the vertex colour still multiplies through the displayColor reader.

---

### Task 0: The Layer holds what the crate needs

**Files:** `litegen-core/src/mesh/usd_stage.rs` (`AttrSpec.variability`, `AttrSpec.metadata`, `Value::Token`/`Value::Tokens`), `usda.rs` (fills them: no more skipping `uniform`/`config`/`varying`; every attribute metadata key kept; dictionary entries typed), `usdc.rs` (its reader fills them too), and `usd_read.rs` (every match on `Value::Str` checked for `Value::Token`).

**Interfaces:**
- Produces: `pub enum Variability { #[default] Varying, Uniform, Config }`, `AttrSpec { variability: Variability, metadata: Vec<(String, Value)>, .. }`, and `Value::Token(String)`, `Value::Tokens(Vec<String>)`. `as_str()` and `strs()` accept both.

- [ ] Write `usda_keeps_variability_metadata_and_token_types`, and `usdc_reader_fills_the_same_fields` (a `usdcat`-written crate of the same text gives an equal Layer; commit the crate as our content).
- [ ] Run `cargo test --lib -- mesh::usda mesh::usdc`. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test --lib mesh::`. Expected: pass, and every existing USD reading is unchanged. Compare the read scenes of every `testdata/usd/` file before and after.

### Task 1: The crate writer

**Files:** `litegen-core/src/mesh/usdc.rs` (writer half: bootstrap and TOC, TOKENS, STRINGS, FIELDS, FIELDSETS, PATHS, SPECS, value reps with variability, integer encoder, literal-only LZ4 framing).

**Interfaces:**
- Produces: `pub fn write(layer: &Layer) -> Result<Vec<u8>, MeshError>`.
- Produces: `fn encode_ints(values: &[i64], wide: bool) -> Vec<u8>`, the inverse of `decode_ints`.
- Produces: `fn fast_compress(src: &[u8]) -> Vec<u8>`, the inverse of `fast_decompress`.

- [ ] Write `integer_coding_round_trips`, `literal_only_lz4_round_trips_through_our_decoder`, `the_bootstrap_says_0_8_0`, `usdc_round_trips_every_layer_we_write`, `usdc_encodes_every_type_we_write`, `usdc_output_is_deterministic`, and Review Focus 1–2 as `nested_dictionaries_keep_their_types` and `defaults_beside_samples_survive`.
- [ ] Run `cargo test --lib mesh::usdc`. Expected: FAIL (undefined).
- [ ] Implement.
- [ ] Run `cargo test --lib -- mesh::usdc mesh::usd`. Expected: pass.
- [ ] Run the crate reader's existing truncation and bit-flip sweeps over our writer's output for every fixture. Expected: no panic.

### Task 2: `MeshFormat::Usdc`, the harness row, and the API

**Files:**
- `litegen-core/src/mesh/mod.rs` (`MeshFormat::Usdc`, `ALL`, `as_str`, `parse`, `capabilities`, `prepare`, `write_scene`, `read_scene_with`)
- `litegen-core/src/mesh/usd.rs` (`write_usdc`)
- `litegen-core/src/proxy/model3d_convert.rs` (`derivable_formats`)
- `litegen-core/src/types/mod.rs` (`Model3dFormat`; the request fields `usdz_root` and `usd_clips`)
- `litegen-core/src/api/handlers/convert.rs` (`PARAMS`: `usdz_root`, `usd_clips`) and the 3D request validation
- `litegen-core/src/proxy/model3d_deliver.rs` (`DeliveryPlan.usdz_root`, `usd_clips`)
- `litegen-core/src/proxy/storage.rs` (`model3d_content_type`: `"usdc"` gets the media type IANA registers for binary USD, if it registers one; the builder checks the IANA registry and cites it, and uses `application/octet-stream` otherwise)
- `scripts/mesh-verify/verify.mjs` (`usdc` handled like `usda`; `usdcat` and `usdchecker` runs; the `usdcat model.usda` versus `usdcat model.usdc` text comparison)

- [ ] Write:
  - `usdc_scenes_equal_usda_scenes`;
  - the `scene_matrix_tests` row and column (automatic once `ALL` grows; check the count);
  - the API tests: `output_formats: [usdc]` delivers, and the converter round-trips `to=usdc`;
  - `usdz_root_usdc_writes_a_crate_root`, plus each option's bad-value, keep-nothing and poller-path tests.
- [ ] Run `cargo test --lib mesh::`. Expected: FAIL.
- [ ] Implement. Ping the frontend/SDK session before the `Model3dFormat` change.
- [ ] Run `cargo test --lib -- mesh:: api::openapi` and `cargo test --test model3d_api`. Expected: pass.
- [ ] Run the harness with `--formats glb,usda,usdc,usdz`. Expected: `usdcat` gives no warnings, `usdcat model.usda` and `usdcat model.usdc` print identical text, `usdchecker` reports 0 errors, usdrecord is 0.00 % between `.usda` and `.usdc`, and three.js renders the `.usdc`.
- [ ] Negative controls: (a) a wrong width code, where `usdcat` and the round-trip test fail; (b) drop `uniform` from the crate, where the `usdcat` text comparison fails. Restore by rewriting.

### Task 3: Several clips

**Files:**
- `litegen-core/src/mesh/fixtures.rs` (`clips()`)
- `usd_write.rs` (one SkelAnimation per clip; the `Animation` variant set switching `skel:animationSource`; the first clip's rigid xformOps outside the variants, as today; `clipEnd` customData)
- `usd_read.rs` and `usd_stage.rs` (per-variant composition for `Animation` on the default prim; rigid samples go to the selected clip; the 64 cap)
- `usd.rs` (`prepare` keeps every clip's skeletal and blend-shape channels and the first clip's rigid channels; `losses` reports the other clips' rigid motion)
- the harness's Node check (`three_js_lists_every_clip`)

- [ ] Write `usd_clips_first_is_todays_output` (byte-identical), `usd_keeps_every_clip` (under `all`), `the_layout_is_apples`, `an_all_rigid_multi_clip_scene_writes_as_today`, `other_variant_sets_read_as_today`, `a_hostile_animation_variant_count_is_capped`, and Review Focus 3–4 as `a_weights_only_clip_keeps_the_rest_pose` and `clip_names_that_sanitise_alike_stay_distinct`.
- [ ] Run `cargo test --lib -- mesh::usd mesh::scene_matrix_tests`. Expected: FAIL (one clip).
- [ ] Implement.
- [ ] Run `cargo test --lib mesh::`. Expected: pass.
- [ ] Harness: add per-variant usdrecord rendering (`--sessionLayer` with the selection) and the three.js clip check. Run it. Expected:
  - every variant matches its clip;
  - `--arkit` accepts every USDZ and reports each variant;
  - three.js lists walk, run and wave with matching durations, and plays walk's rigid motion as its `TransformAnimation`;
  - SceneKit matches the selection, and its other variants are cited skips.
- [ ] Negative controls: (a) bind every variant to the first SkelAnimation; (b) author `skel:animationSource` outside the variant set too. Each fails the per-variant cases. Restore by rewriting.

### Task 4: Unlit

**Files:** `usd_write.rs` (`write_material`: the unlit recipe and the marker), `usd_read.rs` (restore `unlit` from the marker), `usd.rs` (`losses` no longer reports unlit; `prepare` keeps `unlit`).

- [ ] Cite the recipe's sources in `usd_write.rs`: usd_from_gltf `convert/materializer.cc`, and the Storm measurements in [research-consumers.md](../research/research-consumers.md) Q2.
- [ ] Write `unlit_round_trips_through_usd`, `a_black_diffuse_emissive_material_without_the_marker_reads_lit`, and Review Focus 5 as `unlit_keeps_alpha_and_vertex_colour`.
- [ ] Run `cargo test --lib mesh::usd`. Expected: FAIL.
- [ ] Implement.
- [ ] Harness: the two-light invariance check (usdrecord, SceneKit, three.js) with a lit control material. Run it. Expected: the unlit pixels are identical across the two light levels within 1 % and match the base colour, and the lit control changes.
- [ ] Negative control: diffuse = base colour. The invariance check fails. Restore by rewriting.
- [ ] If the recipe itself fails the invariance check, stop. Report the measurement to the user, and don't widen a tolerance.

### Task 5: Docs, gate and the device check

- [ ] Update the docs listed in the spec. Send the matrix-page changes to litegen-a3.
- [ ] Run the overview's full gate and the full harness (all fixtures, all formats including `usdc`). Write the ledger.
- [ ] **Device check (needs the user).** Hand the user three USDZs: `clips()` with `usd_clips=all`, `textured()` with `usdz_root=usdc`, and a plain one. They open each in AR Quick Look on an iPhone and report whether the default clip plays, whether the configuration picker switches clips, and whether both roots open. If the multi-clip file passes, flip USDZ's `usd_clips` default to `all` (one constant, with its test updated). Otherwise it stays `first`. Record the result in the ledger and in `3d-format-conversion.md` §5. **Skipped by the user on 2026-11-10: the default stays `first`.**
