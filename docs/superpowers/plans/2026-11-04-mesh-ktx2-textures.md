# KTX2 Textures Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** KTX2 (Basis Universal) textures are transcoded to PNG on every production path, so every container keeps them instead of dropping them.

**Architecture:** A std-only `mesh/ktx2.rs` decodes level 0 of ETC1S/BasisLZ, UASTC and uncompressed RGBA8 KTX2 to RGBA8. Zstandard comes from the route chosen in D3. `normalize::transcode_textures` encodes the result as PNG, as it does WebP.

**Tech Stack:** Rust (`mesh/ktx2.rs`, `mesh/normalize.rs` with `image`); `ruzstd =0.9.1` (D3, settled); the `zstd` CLI 1.5.x (offline, once, for the Zstandard reference corpus); nightly Miri (once); Binomial `basisu` v2_50 or KTX-Software (offline, once, for the reference outputs); three.js `KTX2Loader` for the native harness check.

**Spec:** `docs/superpowers/specs/2026-11-04-mesh-ktx2-textures-design.md`

**Status: done on 2026-11-09 by session litegen-fc, not committed.** The ledger,
`.superpowers/sdd/2026-11-04-mesh-ktx2-textures/progress.md`, records each task's RED and
GREEN, every review, fix round and negative control, and the gate. It isn't committed; code
comments and the testdata READMEs cite its rulings by ID (K1–K21a).

The proof:
- **Rust gate,** on tree e8270b9 with cargo 1.99.1 and clippy 0.1.99:
  - lib 2,802 passed, 0 failed, 18 ignored;
  - the three load-sensitive tests alone, 3/3;
  - `--tests --no-fail-fast` with Postgres: 54 integration suites, 879 passed, 0 failed.
    The run's 56 targets also repeat the library's 2,802 and main.rs's none (3,681 in all);
  - clippy `--all-targets --all-features -D warnings` clean.
- **Release harness,** on a `mesh_matrix` built from the same tree:
  - every fixture 1,140/1,140;
  - six KTX2 imports with `--transcode-textures` (OwnTextured's `textured.basisu.glb` and
    Khronos's FlightHelmet, ChronographWatch, StainedGlassLamp, AnisotropyBarnLamp and
    CarConcept KTX-BasisU variants), 114/114 each;
  - item 2's plain Khronos set 798/798, so a scene with no KTX2 converts as before;
  - 0 conversion errors anywhere.
- **Decoders:** bit-exact against the reference RGBA on every committed ETC1S and UASTC input,
  and ruzstd 0.9.1 equal to libzstd 1.5.7's `zstd -d` byte for byte on all 87 encodings of the
  committed corpus. Negative controls failed their tests, and each file was restored with Edit
  and `touch`:
  - a swapped ETC1S selector order failed the bit-exact test on all three ETC1S inputs;
  - in the harness (Task 5), the same mutant failed the level-0 albedo and RGBA32 passes. Its
    smallest fault, 0.824 % of pixels in a GPU-format pass, is past that pass's 0.1 % limit,
    which was set after the run;
  - two Zstandard mutants, of the window limit and of the exact allocation, each failed its
    own test.
- **SDKs:** litegen-d0 regenerated for `ktx2_textures` (Task 4) and for the final wording of
  its converter description (after this gate).

The build differs from the steps below in these places:

- **Task split.** Task 1 ran as 1a (the container) and 1b (the corpus), and Task 3's corpus,
  sweeps and audit ran as 3b (K6). Implementers and reviewers ran on sonnet (K3); the final
  review ran on fable.
- **Reference outputs (K9).** The auto-mode classifier refused cloning and building Binomial's
  `basisu` (K6), so the reference RGBA comes from the Binomial transcoder that three.js r183
  bundles. It is stored as `.rgba.zst`, decompressed by ruzstd in the tests (K10).
- **Not run, waiting on the user:**
  - Miri needs the nightly toolchain (Task 3b), which isn't installed;
  - `basisu` (or an encoder) for UASTC with alpha (K11). Non-power-of-two sizes are covered by a
    patched strip header (K13).
- **`cargo audit`** (Task 3b) reports no advisory for ruzstd. Its 23 advisories elsewhere
  (quick-xml first) are the user's call.
- **Choices the spec left open:**
  - A KTX 1.1 file is `Unsupported`, not `Malformed` (K8). `layerCount` 0 or 1 and
    `levelCount` 0 are accepted (K7).
  - The harness transcodes only with the opt-in `--transcode-textures` flag of
    `examples/mesh_matrix.rs`, as delivery and the converter do. Without it, the matrix
    output is byte-identical (K20).
  - OBJ: a MASK material reads back as OPAQUE at or above its cutoff and as BLEND with alpha 0
    below it, matching OBJ's reader, so prepare equals read-back (K21, K21a).
- **Parked minors,** left as they are:
  - the doc re-wrap near 3d-format-conversion.md ~188;
  - a textured MASK below the cutoff in the K21 test;
  - the "image N" label of an unnamed KTX2 shifting when `keep`'s first pass prunes a dead
    WebP (a label only).

Files to commit (47). † marks a file that also holds edits from item 5's follow-ups,
item 10 or the light-units option, all of them done, so they can share a commit.
‡ marks a file that also holds other sessions' uncommitted work (the README and skills
edits, the horizontal-scaling dependency, the 2026-11-09 provider audit), so check with
those first. Both test-data folders are new; commit them whole.

```
.claude/skills/model3d-provider/SKILL.md ‡
README.md ‡
docs/mcp/tools.md †
docs/superpowers/3d-format-conversion.md †
docs/superpowers/plans/2026-11-04-mesh-gaps-handoff.md †
docs/superpowers/plans/2026-11-04-mesh-ktx2-textures.md
docs/superpowers/specs/2026-11-04-mesh-gaps-overview-design.md †
docs/superpowers/specs/2026-11-04-mesh-ktx2-textures-design.md
litegen-core/Cargo.lock ‡
litegen-core/Cargo.toml ‡
litegen-core/examples/mesh_matrix.rs
litegen-core/src/api/handlers/convert.rs †
litegen-core/src/api/handlers/mod.rs †
litegen-core/src/mesh/isolation_tests.rs
litegen-core/src/mesh/ktx2.rs
litegen-core/src/mesh/mod.rs †
litegen-core/src/mesh/normalize.rs †
litegen-core/src/mesh/obj.rs †
litegen-core/src/mesh/scene.rs †
litegen-core/src/mesh/testdata/gltf/ktx2/
litegen-core/src/mesh/testdata/ktx2/
litegen-core/src/mesh/tests.rs †
litegen-core/src/providers/mod.rs †
litegen-core/src/providers/model3d/fal.rs † ‡
litegen-core/src/providers/model3d/meshy.rs †
litegen-core/src/providers/model3d/mock.rs †
litegen-core/src/providers/model3d/mock_data/README.md
litegen-core/src/providers/model3d/mock_data/textured.basisu.glb
litegen-core/src/providers/model3d/tripo.rs †
litegen-core/src/proxy/config_conformance.rs †
litegen-core/src/proxy/model3d_deliver.rs †
litegen-core/src/proxy/poller.rs †
litegen-core/src/proxy/router.rs †
litegen-core/src/types/mod.rs †
litegen-core/tests/convert_api.rs †
litegen-core/tests/live_model3d.rs †
litegen-core/tests/model3d_api.rs †
litegen-core/tests/model3d_validation.rs †
scripts/mesh-verify/README.md †
scripts/mesh-verify/camlight-oracle.mjs †
scripts/mesh-verify/layer-extensions.mjs
scripts/mesh-verify/page/camera-view.js
scripts/mesh-verify/page/node-rank.js
scripts/mesh-verify/page/render.js †
scripts/mesh-verify/pixel-digest.mjs
scripts/mesh-verify/pixel-digest.test.mjs
scripts/mesh-verify/verify.mjs †
```

litegen-d0 lists the regenerated SDK and landing files with its own work.

## Global Constraints

- The overview's "Rules every plan carries" apply.
- Zstandard is `ruzstd = { version = "=0.9.1", default-features = false, features = ["std"] }` (D3, settled), with `ALLOWED` gaining it for `ktx2.rs` only. "Fully tested" (the user's bar): byte-equal to the reference `zstd -d` over the committed corpus, the hostile sweeps, one Miri run, and `cargo audit`. All are rerun whenever the pin changes.
- The `ktx2_textures` option (`png` default, `keep` for glb and gltf) is OpenAPI-visible. Ping the frontend/SDK session with its shape before Task 4.
- Level 0, layer 0, face 0 only. No colour conversion from the DFD.
- Pixel budget: width × height ≤ 64M, checked from the header first.
- Scene texture budget: transcoded images stop before the scene's encoded image bytes pass `MAX_INPUT_BYTES / 2` (64 MiB). Note text, exact: `texture {t} (KTX2) was left out: transcoding it would pass the 64 MiB texture budget`.
- Committed inputs: the CC0 FlightHelmet files, the CC-BY CarConcept files (with attribution), and our own encodings. Expected outputs are raw RGBA8 from the reference tool, with the tool and version in `testdata/ktx2/README.md`.

## Review Focus

1. Non-power-of-two sizes that are multiples of 4 (for example 1,020 × 516): the block grid and the output stride are exact, with no padding columns in the PNG.
2. An ETC1S file whose alpha slice is absent while the DFD declares alpha: alpha 255, and the decode matches the reference.
3. A KTX2 image referenced by two textures with different samplers: transcoded once, and both textures point at the one PNG.
4. A material map (normal or ORM) tagged sRGB in the DFD: its bytes are unchanged, the DFD is ignored, and the glTF slot decides how it's read.
5. A KTX2 whose `levelCount` says 11 but whose level index lists fewer entries: `Malformed` before any level is read.

---

### Task 1: Reference corpus and the container

**Files:** `litegen-core/src/mesh/ktx2.rs` (new; add to `mod.rs` and `MODULE_FILES`), `litegen-core/src/mesh/testdata/ktx2/` (inputs, `.rgba` expected outputs, README), and a one-off script `testdata/ktx2/make_reference.sh` (it documents the exact tool commands; it isn't run in CI).

**Interfaces:**
- Produces: `pub fn decode_rgba8(bytes: &[u8]) -> Result<Rgba8, MeshError>` with `pub struct Rgba8 { pub width: u32, pub height: u32, pub pixels: Vec<u8> }`.
- Produces: `pub fn probe(bytes: &[u8]) -> Result<Ktx2Info, MeshError>` (format, dimensions, level count, supercompression), used by the tests and by error messages.

- [ ] Build the corpus: copy the four sample files, encode our two fixture PNGs in the four modes, and write the expected `.rgba` files with the reference tool. Record versions.
- [ ] Write the container tests: `levels_are_taken_by_offset_not_by_order`, `level_count_zero_means_the_base_level`, `zlib_supercompression_is_refused_by_name`, `cube_and_array_textures_are_refused_by_name`, `a_texture_past_the_pixel_budget_is_refused_before_allocating`, `slice_offsets_outside_level_zero_are_malformed`, and `a_short_level_index_is_malformed` (Review Focus 5).
- [ ] Run `cargo test --lib mesh::ktx2`. Expected: FAIL (undefined).
- [ ] Implement the container parse and the uncompressed `R8G8B8A8` path.
- [ ] Run `cargo test --lib -- mesh::ktx2 mesh::isolation_tests`. Expected: the container tests pass.

### Task 2: ETC1S/BasisLZ

**Files:** `litegen-core/src/mesh/ktx2.rs`.

- [ ] Write `every_reference_texture_transcodes_bit_exactly`, covering the ETC1S inputs here (it extends in Task 3), plus `an_etc1s_alpha_slice_becomes_alpha`, `an_absent_alpha_slice_is_opaque` (Review Focus 2), `a_malformed_huffman_table_is_malformed`, `npot_multiples_of_four_decode_exactly` (Review Focus 1), and `dfd_tags_never_change_the_bytes`.
- [ ] Run `cargo test --lib mesh::ktx2`. Expected: FAIL.
- [ ] Implement from KTX2 Appendix D, with Binomial's transcoder at `073eb6a0a34a` as reference.
- [ ] Run it again. Expected: pass, bit-exact.
- [ ] Negative control: swap the ETC1 selector bit order. The bit-exact test fails on every ETC1S input. Restore by rewriting.

### Task 3: UASTC, and Zstandard through `ruzstd`

**Files:**
- `litegen-core/src/mesh/ktx2.rs`
- `litegen-core/Cargo.toml` (`ruzstd = { version = "=0.9.1", default-features = false, features = ["std"] }`)
- `isolation_tests.rs` `ALLOWED` (`ruzstd` for `ktx2.rs` only)

- [ ] Extend the bit-exact test to the UASTC and UASTC + Zstd inputs. Write `a_zstd_payload_longer_than_its_level_is_malformed`, `a_zstd_frame_past_the_cap_stops_at_the_cap` and `a_zstd_dictionary_frame_is_refused`.
- [ ] Run `cargo test --lib mesh::ktx2`. Expected: FAIL.
- [ ] Implement. The Zstandard output is capped at the exact expected length.
- [ ] Run `cargo test --lib -- mesh::ktx2 mesh::isolation_tests`. Expected: pass.
- [ ] Add the truncation and bit-flip sweeps on all six committed inputs. Expected: pass.

### Task 3b: `ruzstd`, fully tested

**Files:** `litegen-core/src/mesh/testdata/ktx2/zstd/` (the corpus, the `zstd -d` outputs, and a README with the exact `zstd` commands and version; our content).

- [ ] Make the corpus with the `zstd` CLI: our fixtures' UASTC level data, an RLE buffer, a random buffer, and structured data, at `-1`, `-3`, `-9`, `-19` and `--ultra -22`, each with and without `--check`, plus `--long=27`, three concatenated frames, and a skippable frame first. Record `zstd -d` of each.
- [ ] Write `ruzstd_matches_reference_zstd` (every file byte-equal), and the corpus truncation and bit-flip sweep (no panic, `Malformed` only).
- [ ] Run them. Expected: pass.
- [ ] Run the Zstandard tests once under Miri: `cargo +nightly miri test --lib mesh::ktx2::zstd_` (install the nightly `miri` component if missing, and use only the small corpus files). Expected: no undefined behaviour reported. Record the output, and treat any Miri error as a blocker reported upstream.
- [ ] Run `cargo audit` and `cargo tree -p ruzstd -e normal`. Expected: no advisory, and no runtime dependencies. Record both.

### Task 4: Production paths

**Files:**
- `litegen-core/src/mesh/normalize.rs` (`transcode_textures`: KTX2 → `ktx2::decode_rgba8` → PNG; a dropped KTX2's note carries the decoder's reason)
- `litegen-core/src/mesh/tests.rs`
- `litegen-core/src/api/handlers/convert.rs` (tests; the OpenAPI text)
- `litegen-core/tests/model3d_api.rs`
- a mock row returning the `-tc` GLB (`models/mock.yaml`, `providers/model3d/mock.rs`)
- the gltfpack `-tc`/`-tu` GLBs of `textured` in `testdata/gltf/ktx2/`

- [ ] Write the `ktx2_textures` option tests from the spec (`ktx2_textures_keep_leaves_glb_its_ktx2`, `ktx2_textures_keep_still_gives_other_containers_png`, `ktx2_textures_png_is_todays_output`, the bad-value and keep-nothing refusals, the poller path). Then write `ktx2_textures_survive_every_container`, `one_ktx2_image_two_textures_transcode_once` (Review Focus 3), `material_maps_tagged_srgb_keep_their_bytes` (Review Focus 4), `the_texture_budget_drops_the_rest_and_says_so` (64 MiB of encoded images: later KTX2 images dropped with the note; the written GLB reads back), the converter test (no `image_formats` loss; a broken KTX2 keeps the loss), and the delivery test (PNG in the delivered GLB; `original` byte-identical).
- [ ] Run `cargo test --lib -- mesh::tests api::handlers::convert` and `cargo test --test model3d_api`. Expected: FAIL (the texture is dropped).
- [ ] Implement, including the option: converter `PARAMS`, the 3D request field, `DeliveryPlan.ktx2_textures`, and delivery writing glb/gltf from the untranscoded scene under `keep`. Regenerate `docs/mcp/tools.md`.
- [ ] Run the same command, plus `cargo test --lib -- api::openapi mcp::`. Expected: pass.

### Task 5: Harness, docs and gate

**Files:** `scripts/mesh-verify/page/render.js` (native originals load with `KTX2Loader`, its transcoder path set to three r183's `examples/jsm/libs/basis/`), `scripts/mesh-verify/README.md` (the calibrated albedo threshold), and the docs listed in the spec.

- [ ] Calibrate the KTX2 native threshold: the original against itself through `KTX2Loader` twice is 0.00 %, and against our PNG reading the measured maximum gives the threshold, recorded.
- [ ] Run `--import` over the five Khronos KTX-BasisU variants (local). Expected: native within the threshold, and every direction passes.
- [ ] Update the docs. Run the overview's full gate. Write the ledger, with the Miri and `cargo audit` records.
