# Draco and meshopt glTF Input Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The glTF reader decodes meshopt (EXT and KHR, std-only) and Draco (through `draco-core =2.3.3`, bit-exact against Google's decoder), so compressed vendor GLBs deliver instead of failing after billing.

**Architecture:** `mesh/meshopt.rs` decodes a compressed bufferView into an owned buffer of exactly `count × byteStride` bytes, which accessors read through the existing budgeted path. Draco primitives decode in `mesh/draco.rs` through `draco-core`, behind one function with absolute limits from the scene caps.

**Tech Stack:** Rust (`mesh/glb.rs`, new `mesh/meshopt.rs`, new `mesh/draco.rs`); the `draco-core =2.3.3` crate (D2, settled); Google's `draco_encoder`/`draco_decoder` at `d429de84b52a`, built once, offline, for the reference corpus; Node (meshoptimizer's JS reference decoder for test vectors; three.js `DRACOLoader`/`MeshoptDecoder` for the native harness check); gltfpack for our fixtures.

**Spec:** `docs/superpowers/specs/2026-11-04-mesh-compressed-gltf-input-design.md`

**Status: done on 2026-11-08 by session litegen-b2, not committed. Part of Task 4's Draco
limits work is open (B41).** The ledger,
`.superpowers/sdd/2026-11-04-mesh-compressed-gltf-input/progress.md`, records every ruling
with its cost if wrong and its evidence. The ledger isn't committed, so the rulings are
also listed at the end of this block, one line each. Code comments and docs cite them by
ID ("Ruling B22", "(B46)"). The build differs from the steps below in these places:

- **Open, for a later session (B41).** Task 4's review asked for Draco limits that reach
  a mesh decode. Its fix round is in the tree and passing:
  - the declared point and face counts are checked in the stream's header before
    draco-core decodes, for sequential and EdgeBreaker streams (B37, B37a);
  - only the attributes glb.rs reads are copied (B37c);
  - a decoded-bytes ceiling of 24,000,000 applies (B38a). The worst measured case is
    1,494 MB per Draco primitive, under convert.rs's ~1.7 GB per conversion slot.

  Still open:
  - a faces-per-point pre-check (B39);
  - a RED test for the ceiling;
  - the ceiling's final numbers in draco.rs's module doc;
  - the fix round's scoped re-review.

  The whole-item final review read this code (see below).
- **draco-core's features** are `["decoder", "edgebreaker_valence_decode"]` (B29), not
  `["decoder"]` alone, because 10 of the 16 Khronos Draco samples use valence
  EdgeBreaker. Point clouds and pre-2.2 streams stay refused. `cargo tree -p draco-core
  -e normal` lists bytemuck 1.25.0 and bytemuck_derive 1.12.1, a build-time
  proc-macro, not bytemuck alone. draco-core's own `unsafe_code = "forbid"` is recorded.
- **What the reader does:**
  - meshopt: EXT and KHR; every mode and filter; both stream versions. An EXT view with
    a v1 stream or the COLOR filter reads with a warning (B9). The four stricter rules
    stay Malformed (B16).
  - A KHR view that's only in `extensionsUsed` decodes too. A `.gltf` given its fallback
    `.bin` but not its stream's is refused as incomplete (B22).
  - A layer-only image in a meshopt view that doesn't decode drops its layer with a
    warning (B15b).
  - Draco: TRIANGLES and TRIANGLE_STRIP, with a strip's faces read as a list. Every
    other mode is Malformed; item 10 widened this from TRIANGLE_FAN alone.
  - A Draco stream decodes when the extension is only used (B27).
  - An accessor `count` that disagrees with the decode reads at the decoded count, with
    a warning (B32).
  - litegen writes neither compression.
- **Interfaces.** `mesh::glb::COMPRESSION_EXTENSIONS` (pub) is the one list delivery
  checks (B24). `mesh::meshopt` and `mesh::draco` are private to mesh (B8).
- **Delivery.** A vendor glTF that uses a compression extension counts as changed. It's
  re-derived uncompressed, and the vendor's file is kept as `original` (B23). A
  keep-nothing request hands out the vendor's file as it came.
- **The mock.**
  - `mock/meshopt-3d` and `mock/draco-3d` return committed GLBs of our own content
    (B20, B21, B26).
  - `mock/partial-3d` now requires `EXT_litegen_mock_undecodable`, so it keeps meaning
    "a vendor file litegen refuses".
- **The proof:**
  - draco-core matches Google's decoder bit for bit on 445 streams: 22 inputs × 23
    settings, each byte-distinct stream stored once (B42). The reference is Google's
    decoder built with `-ffp-contract=off` (B45). Two streams Google's decoder refuses
    are checked against an inferred expectation (B44).
  - The meshopt differentials: against meshoptimizer's JS reference, against
    MeshoptCubeTest's fallback, and against gltfpack's uncompressed twins (B12, B18,
    B19).
  - Draco's negative controls are three mutants (B30).
  - The render harness, Task 5:
    - Draco: 1781 of 1824 cases pass;
    - meshopt: 6 × 114/114;
    - gltfpack: 4095 of 4104.

    Every failure recurs on the uncompressed twin or follows a WebP texture. One native
    check is a cited skip: three r183's meshopt decoder mis-decodes one v1 view (B46).
- **The final review** (whole item): no Critical issue, and the decoders were ready to
  close as code. Two Important findings:
  - the release-timed sweeps weren't recorded, and the gate ran them (B47);
  - the Draco memory bound read as proven, and was reworded as measured (B48).
- **The final wave** (one dispatch, B52) took the drafted items and the review's "fix
  now" minors:
  - B28's UV bake;
  - B15b's and B22's tests;
  - delivery's check over every vendor glTF;
  - Draco's indices count warning;
  - legacy prediction schemes as Unsupported;
  - Draco pinned in `COMPRESSION_EXTENSIONS`;
  - the google-refused rows asserted;
  - the meshopt byteStride check;
  - B49's unlisted-attribute rule;
  - docs.

  Its re-review found one regression, B49's first shape: attributes × mappings work and
  one warning per mapping, 30.7 s from a ~0.9 MB file. The residual fix (B54) made it one
  pass and one warning, 9 ms on the same shape. Every mutant failed on its own
  assertion. The mutant restore protocol changed mid-wave (B55).
- **P6:** default output is unchanged for every fixture × container. Every listed
  difference was item 10's in-flight `wire` fixture (B56).
- **The gate** (B53, in two parts around item 10's in-flight work; Rust 1.99.1):
  - **Release sweeps:** 52 sweeps pass, plus the full MeshoptCubeTest.bin sweep, 53,296
    readings. Item 2's slowest single case is 36.7 ms (staged.cc.glb), under the
    one-second bar.
  - **The lib:** 2677 passed, 0 failed.
  - **The three load-sensitive tests:** pass serially. mesh::fbx's one read 864 ms
    against an 83 ms parse under load, its known signature.
  - **`--tests --no-fail-fast` with Postgres:** all 55 integration suites pass. Two of
    item 10's new usd tests failed while its fix round was mid-edit, and pass on re-run.
  - **clippy -D warnings:** clean.
  - **The harness fixture run:** 1140 cases, 1097 pass, 0 refused, no native failure
    or skip, and the camlight oracle agrees on 45 of 45. All 43 failures are item 10's
    in-flight `wire` fixture (points and lines). Points and lines rendering in the
    harness comes with item 10's Task 6a, which litegen-fc confirms. Every other fixture
    passes in every direction.

**Files to commit** (item 2's; other items edit some of the same files):
Each file is either new or carries item 2's hunks. Other items edit glb.rs, mod.rs,
isolation_tests.rs, pose.rs, mock.rs, model3d_deliver.rs, the harness and the docs too.
- **New, item 2's alone:**
  - `litegen-core/src/mesh/meshopt.rs` and `draco.rs`;
  - `litegen-core/src/mesh/testdata/gltf/**`;
  - `litegen-core/src/providers/model3d/mock_data/**`;
  - `scripts/mesh-verify/meshopt-vectors.mjs`.
- **Shared, with item 2's hunks:**
  - `litegen-core/src/mesh/glb.rs`, `mod.rs`, `isolation_tests.rs`, `matrix_tests.rs`
    and `pose.rs` (B28);
  - `litegen-core/src/providers/model3d/mock.rs`;
  - `litegen-core/src/proxy/model3d_deliver.rs`;
  - `litegen-core/src/proxy/config_conformance.rs`, B40's one line. That file is
    another session's and untracked.
  - `litegen-core/tests/model3d_api.rs`;
  - `models/mock.yaml`, the two rows;
  - `litegen-core/Cargo.toml` and `litegen-core/Cargo.lock`, the draco-core line and
    its additions;
  - `scripts/mesh-verify/page/render.js`, `verify.mjs` and `README.md`.
- **Docs and rules:**
  - `docs/superpowers/3d-format-conversion.md` and `param-mapping-matrix.md`;
  - `.claude/skills/model3d-provider/SKILL.md`;
  - `.claude/rules/mesh-standalone.md`;
  - this plan, its spec, and the overview's row 2.

**Follow-ups for the user:**
- **A security advisory.** quick-xml 0.32.0 has RUSTSEC-2026-0194 and -0195. It parses
  untrusted Collada and 3MF in mesh's `xml.rs`, and is fixed in ≥ 0.41.0. This is
  pre-existing. litegen-bd took it first, and on 2026-11-10 litegen-cf claimed the bump
  to 0.42. rust-s3 0.35.1 and aws-creds 0.37.0 still require 0.32, so their copy stays
  in the lock and `cargo audit` keeps listing it until they move too.
- **Found by the harness, outside item 2:**
  - usd → fbx with partial UVs;
  - unnamed camera nodes paired by index (Duck, VirtualCity), sent to litegen-fc;
  - VirtualCity's cameras in the Collada oracle and in usdrecord;
  - `layer-extensions.mjs` pairing materials by name;
  - CesiumMilkTruck's clip name in SceneKit;
  - MorphPrimitivesTest obj → usd;
  - a WebP texture under OBJ.
- **Upstream notes, drafted and not filed:**
  - draco-core: its point limit doesn't reach a mesh decode;
  - google/draco: a sequential-decoder check refuses two valid streams;
  - three.js and meshoptimizer: the bundled decoder mis-decodes a v1 view.
- **The TokenHub note:** the text for `scripts/model-drift/providers/tokenhub.mjs:349`
  is in the ledger's `task-5-report.md`. litegen-41 isn't running.
- **Disk.** The ledger keeps 11 GB of harness pictures (`task5/report/`) and Google's
  source (`tools/draco-src`, 152 MB).

**Rulings.** Code comments and docs cite these IDs. Each line gives the decision, then
what it costs if it's wrong.

*Pre-flight and Task 1 (the meshopt decoder):*
- **B1.** `decode_view` validates its own arguments: the stride rules, `count % 3` for
  TRIANGLES, mode and filter compatibility, and the header byte. *If wrong:* a future
  caller outside glb.rs that skips the budget can allocate count × stride.
- **B2.** Task 1's RED is an assertion failure against a stub. *If wrong:* nothing; the
  stub is replaced in the same task.
- **B3.** Test streams come from meshoptimizer's encoder run over our own data, or are
  built by hand. Each test proves its case is in the stream. *If wrong:* an
  encoder-made stream may miss a corner a hand-built one would hit.
- **B4.** A mock row is added only if no existing one can return the file. The landing's
  model list carries no mock rows, so nothing is regenerated. *If wrong:* one catalog
  row to regenerate.
- **B5.** Task 4's oversized stream may be made by crafting a header or by draco-core's
  encoder as a dev-dependency. The production build stays decoder-only. *If wrong:* one
  dev-dependency feature.
- **B6.** The meshopt decoder accepts the KHR superset (v0 and v1, all four filters).
  Whether EXT accepts v1 or COLOR is glb.rs's call (B9). *If wrong:* nothing at the
  decoder.
- **B7.** Done-bar line 7 covers the decoder itself, so Task 1 sweeps its own streams.
  *If wrong:* a few seconds of suite time.
- **B8.** `mod meshopt;` is private, a helper of glb.rs. *If wrong:* a later need for a
  public decoder widens it.

*Task 2 (glTF reads meshopt):*
- **B9.** Both meshopt extensions join `IGNORABLE_REQUIRED_EXTENSIONS` and
  `CARRIED_EXTENSIONS`. An EXT view with v1 or COLOR reads with a warning. *If wrong:* a
  non-conformant EXT file converts, with a warning.
- **B10.** The sweeps' release timing runs once, at the item's end. *If wrong:* a slow
  sweep case is found late.
- **B11.** A view decodes once, and its accessors slice it, in a budgeted cache. *If
  wrong:* a design the review sends back.
- **B12.** MeshoptCubeTest is compared three ways: with the JS reference's decode, with
  the plain variant's fallback slices, and as a scene against the plain variant. *If
  wrong:* about 130 KB more testdata than needed.
- **B13.** `testdata/gltf/` has a README in the usd one's shape. *If wrong:* nothing.
- **B14.** The gltfpack set: gltfpack 1.3.0 over every fixture GLB, with `-c`, `-cc` and
  `-cz` and the uncompressed twin, recording what gltfpack drops. *If wrong:* one
  regeneration when a fixture changes.
- **B15, B15a.** Task 2 changes only the module doc's meshopt sentences; Task 5 does the
  docs. *If wrong:* nothing.
- **B15b.** A layer-only image in a meshopt view that fails as Malformed drops its layer
  with a warning (item 4's A16c line). Only the budget, an unsupplied external stream
  buffer and a non-base64 `data:` URI refuse. *If wrong:* one doc sentence and one test.
- **B16.** The decoder's four stricter validity rules stay Malformed. *If wrong:* a
  non-conformant third-party stream that three.js loads is refused (no known encoder
  writes one).
- **B17.** matrix_tests.rs's
  `a_required_extension_is_refused_however_it_is_spelled` keeps its string-valued cases.
  Its array cases become one `["EXT_litegen_mock_undecodable"]`, and its meshopt-view
  part is reworded. *If wrong:* fewer spellings tested.
- **B18.** gltfpack `-c` is bit-equal for vertex data. Triangles compare as any cyclic
  rotation with the winding kept. *If wrong:* a decoder bug that only rotates triangles
  would pass, though it draws identically.
- **B19.** EXPONENTIAL exactness is proven against the JS reference on MeshoptCubeTest.
  Against gltfpack's twin, 2⁻¹⁴ of the channel's largest value is allowed. *If wrong:* a
  slightly lossy decode on gltfpack files would pass; the reference comparison would
  still catch it.
- **B22.** A KHR view that's only used decodes too, so a `.gltf` given its fallback
  `.bin` but not its stream's is refused, where it read before. *If wrong:* such a file,
  which converted before, is refused.

*Task 3 (the mock and delivery):*
- **B20.** One mock row per codec. *If wrong:* two catalog rows where one with a knob
  might do.
- **B21.** The mock embeds its own copy under `mock_data/`, with a README naming the
  source and sha256, and never `include_bytes!` into `mesh/testdata/`. *If wrong:* about
  20 KB duplicated.
- **B23.** A vendor glTF that uses meshopt or Draco counts as changed and is rewritten,
  not only one that requires an extension. *If wrong:* one more re-encode per such file.
- **B24.** One list of decoded compression extensions, `mesh::glb::COMPRESSION_EXTENSIONS`,
  which delivery checks. *If wrong:* one more pub item in `mesh::glb`.
- **B25.** The delivery test asserts no compression extension is used and nothing is
  required. `KHR_texture_transform`, which gltfpack adds, may stay. *If wrong:* nothing
  for the spec's intent.
- **B28.** Texture transforms are baked into the UVs whenever a container keeps UVs but
  not transforms, not only when it keeps textures. *If wrong:* textureless single-file
  outputs of any input with a texture transform change their UVs, as intended.

*Task 4 (Draco):*
- **B26.** The mock's Draco GLB is our own content, compressed by glTF-Transform's CLI,
  pinned and recorded. *If wrong:* a dev-time npm tool.
- **B27.** A Draco primitive decodes even when the extension is only used. *If wrong:* a
  used-only Draco file whose stream is corrupt no longer reads its fallback.
- **B29.** `features = ["decoder", "edgebreaker_valence_decode"]`. *If wrong:* the
  valence decoder's code in every build.
- **B30.** "Skip dequantization" can't be planted, since draco-core dequantizes inside,
  so three mutants stand in: ids by order, no point map, and positions two quantization
  steps off. *If wrong:* nothing; three controls instead of one.
- **B31.** The quantization parameters are read once with three r183's bundled Google
  decoder and stored as constants with their provenance. *If wrong:* nothing; it's a
  measuring tool.
- **B32.** An accessor `count` that disagrees with the Draco decode reads with a
  warning. Refusing would fail a vendor file after billing. *If wrong:* a non-conformant
  file converts, with a warning.
- **B37, B37a.** The declared point and face counts of sequential and EdgeBreaker streams
  are checked before draco-core decodes. *If wrong:* header layouts in our code that
  must track draco-core's.
- **B37b.** The working-buffer amplification is accepted under the byte ceiling,
  superseded in its numbers by B38a. *If wrong:* the docs' bound is per primitive.
- **B37c.** Only the attributes glb.rs reads are copied. *If wrong:* nothing; less
  copying.
- **B38, B38a.** The decoded-bytes ceiling is 24,000,000, with a measured worst case of
  1,494 MB per Draco primitive. *If wrong:* Draco meshes past ~500k points with
  POSITION, NORMAL, TEXCOORD_0 and TANGENT, or ~750k without TANGENT, are refused by
  name.
- **B39 (open).** Faces per point are bounded before decoding. *If wrong:* a
  non-manifold or very high-genus Draco mesh past the rule is refused by name.
- **B40.** config_conformance's adapter scan reads only `.rs` files, so the mock's data
  folder isn't read as a file. *If wrong:* nothing; one filter line in a test helper.
- **B41.** Task 4 is parked, with its fix round's scoped re-review open. *If wrong:* the
  fix round's code ships with its own review unconfirmed; the final review is the
  remaining check.

*Task 4b (bit-exact against Google):*
- **B33.** The repo gets the `.drc` streams and a manifest of SHA-256s of Google's
  decoded bytes; the full dumps stay in the ledger. *If wrong:* a mismatch names the
  stream and attribute but not the byte.
- **B34.** Google's tools are built from google/draco d429de84b52a. *If wrong:* nothing.
- **B35.** A small C++ encoder and dump tool against Google's library carry tangents and
  generic attributes. *If wrong:* two small C++ files in testdata.
- **B36.** cargo-audit, cargo tree and the forbid line are recorded. Pre-existing
  advisories are listed, not fixed. *If wrong:* nothing.
- **B42.** Each byte-distinct stream is stored once, and the manifest maps every
  (input, setting) pair. *If wrong:* about 1.1 MB of testdata.
- **B43.** Task 4b ran on fable while opus was out. *If wrong:* an implementer on a
  less-proven tier, still reviewed.
- **B44.** Two streams Google's decoder refuses carry the hashes of Google's decode of
  the same input's cl0-qp11 stream, backed by a recorded 20/20 equivalence. *If wrong:*
  two streams pinned to an inferred expectation.
- **B45.** The reference is Google's decoder built with `-ffp-contract=off`. Its default
  arm64 build fuses the normal transform into FMAs, and 414 streams' normals differ
  there by up to 2.7e-7. *If wrong:* the reference differs from a contracted native
  build, by a recorded amount.

*Task 5 (the harness and the docs):*
- **B46.** verify.mjs skips the native check for `layered.cz.glb` alone, keyed by its
  sha256. three r183's bundled meshopt decoder mis-decodes its one v1 ATTRIBUTES view
  (60 of 576 bytes), while the reference decoder agrees with the original; the other 136
  of 137 views are identical. *If wrong:* one file's native picture is unjudged while
  three's decoder has the bug.

*The final review and the final wave:*
- **B47.** The release-timed sweeps run as the controller's gate step. *If wrong:*
  nothing; one release test build.
- **B48.** The Draco ceiling is described as the worst measured over strips and grids,
  with B39 open. *If wrong:* nothing; the docs claim less.
- **B49.** An extension-mapped Draco attribute the primitive doesn't list is skipped with
  one warning when litegen wouldn't read it, and refused when it would. *If wrong:* a
  non-conformant file converts with a warning.
- **B50.** draco_values' doc states its bound. Charging Draco's indices before allocation
  is parked with B41. *If wrong:* they're charged after the decode, as before.
- **B51.** octahedral takes the reference's form for −0.0. *If wrong:* nothing; no
  encoder writes it.
- **B22a.** B22 stays, and its test names the alternative. *If wrong:* a rare
  post-billing refusal.
- **B52.** One fix dispatch: the drafted items and the review's "fix now" minors. The
  rest are parked by the review's reasons. *If wrong:* about thirty small edits.
- **B53.** The gate ran in two parts around item 10's in-flight work. *If wrong:* one
  more release build.
- **B54.** B49's first shape was a regression, fixed in a residual round, not parked.
  *If wrong:* one more short round.
- **B55.** A mutant is restored by reversing only its lines, and the diff against the
  saved copy is shown. *If wrong:* nothing; a stricter restore.
- **B56.** P6 is accepted from the recomputed listings, where every difference is item
  10's. *If wrong:* an unintended matrix change goes unseen.

## Global Constraints

- The overview's "Rules every plan carries" apply.
- meshopt needs no crate. Draco uses `draco-core = { version = "=2.3.3", default-features = false, features = ["decoder", "edgebreaker_valence_decode"] }` (D2, settled; the second feature by Ruling B29), with `ALLOWED` gaining it for `draco.rs` only.
- "Fully tested" (the user's bar): bit-exact equality with Google's `draco_decoder` over the reference corpus, the hostile sweeps, `cargo audit`, and `cargo tree` (only `bytemuck`). All are rerun whenever the pin changes.
- Only CC0 and CC-BY samples are committed (README with licence lines): MeshoptCubeTest, Box, MorphPrimitivesTest, RiggedSimple, Avocado's and WaterBottle's `.gltf`/`.bin`. WaterBottle is the valence EdgeBreaker case. BrainStem, VirtualCity and Duck stay local.
- Draco limits are absolute: points ≤ `MAX_SCENE_VERTICES`, faces ≤ `MAX_SCENE_INDICES / 3`, and decoded bytes ≤ glb.rs's `MAX_DECODED_VALUES × 4`.
- litegen's glTF output stays uncompressed.

## Review Focus

1. A meshopt bufferView shared by two accessors with different `byteOffset`s inside it: decode the view once, then both accessors slice the decoded buffer.
2. A meshopt view whose fallback buffer is the GLB's BIN chunk (the case the old guard was written for): the decoded data must win, and the fallback bytes are never read as geometry.
3. A Draco primitive with an attribute the extension maps but the primitive's `attributes` doesn't list, or the reverse: refused with a message, never misassigned.
4. A Draco primitive in TRIANGLE_STRIP mode (the extension allows it): the decoded indices form a list, and the strip isn't expanded twice.
5. A file using both Draco and meshopt, or `KHR_mesh_quantization` with either: each layer decodes in the right order, with quantized values dequantized after decompression.

---

### Task 1: The meshopt decoder

**Files:** `litegen-core/src/mesh/meshopt.rs` (new; add to `mod.rs` and `MODULE_FILES` in `isolation_tests.rs`), `scripts/mesh-verify/meshopt-vectors.mjs` (new; runs meshoptimizer's `js/meshopt_decoder_reference.js` at `f0a684f11ead` over our hand-built streams and prints the expected bytes), and the committed vectors as `const` arrays in the test module.

**Interfaces:**
- Produces: `pub fn decode_view(data: &[u8], count: usize, stride: usize, mode: Mode, filter: Filter) -> Result<Vec<u8>, MeshError>`, with `pub enum Mode { Attributes, Triangles, Indices }` and `pub enum Filter { None, Octahedral, Quaternion, Exponential, Color }`.

- [ ] Write the mode and filter tests from the spec, with the vectors from the script: `attributes_v0_decode`, `attributes_v1_control_modes_decode`, `triangles_decode`, `index_sequence_decodes`, `octahedral_filter_8_and_16_bit`, `quaternion_filter`, `exponential_filter_is_exact` and `color_filter_ycocg`.
- [ ] Write the validity tests: `a_stream_that_ends_early_is_malformed`, `bytes_left_before_the_tail_are_malformed`, `a_bad_header_byte_is_malformed` and `stride_rules_are_enforced`.
- [ ] Run `cargo test --lib mesh::meshopt`. Expected: FAIL (undefined).
- [ ] Implement from the KHR spec's appendices.
- [ ] Run `cargo test --lib -- mesh::meshopt mesh::isolation_tests`. Expected: pass.
- [ ] Negative control: flip the zig-zag sign in INDICES. `index_sequence_decodes` fails. Restore by rewriting.

### Task 2: glTF reads meshopt

**Files:** `litegen-core/src/mesh/glb.rs` (`view_bytes` decodes extension views into owned buffers, cached per view; `CARRIED_EXTENSIONS`; budget checks), `litegen-core/src/mesh/testdata/gltf/MeshoptCubeTest/` (CC0; README line), and `testdata/gltf/gltfpack/` (our fixtures, `-c`, `-cc` and `-cz`; README naming the gltfpack version).

- [ ] Write:
  - `declared_sizes_are_budgeted_before_allocating`;
  - `out_of_range_decoded_indices_are_refused`;
  - `a_meshopt_view_with_a_missing_fallback_reads`;
  - the MeshoptCubeTest differential (against the JS reference's bytes and against its `glTF` variant);
  - the gltfpack differentials (`-c` bit-equal; `-cc`/`-cz` within filter tolerance; record the one-time quantization check);
  - Review Focus 1 and 2 as `one_view_two_accessors_decode_once` and `a_bin_chunk_fallback_is_never_read_as_geometry`;
  - the truncation and bit-flip sweeps.
- [ ] Run `cargo test --lib mesh::glb`. Expected: FAIL (refused by name).
- [ ] Implement.
- [ ] Run `cargo test --lib mesh::`. Expected: pass.

### Task 3: The mock, delivery and docs for meshopt

**Files:** `litegen-core/src/providers/model3d/mock.rs` (`generate_undecodable_glb` requires `EXT_litegen_mock_undecodable`; a meshopt mock row if needed), `models/mock.yaml` (if a row is added), `litegen-core/src/proxy/model3d_deliver.rs` (comment), `litegen-core/tests/model3d_api.rs`.

- [ ] Write `mock_partial_3d_still_refuses_its_glb` and `a_vendor_meshopt_glb_is_delivered`.
- [ ] Run `cargo test --test model3d_api` and `cargo test --lib -- proxy::model3d_deliver providers::model3d::mock`. Expected: the new tests fail, and `mock/partial-3d`'s existing tests pass with the changed construct.
- [ ] Implement.
- [ ] Run the same command. Expected: pass.

### Task 4: Draco decoding

**Files:**
- `litegen-core/src/mesh/draco.rs` (new; add to `mod.rs`, `MODULE_FILES`, and to `ALLOWED` for `draco_core`)
- `litegen-core/Cargo.toml` (`draco-core = { version = "=2.3.3", default-features = false, features = ["decoder", "edgebreaker_valence_decode"] }`, Ruling B29)
- `litegen-core/src/mesh/glb.rs` (the primitive path calls `draco::decode_primitive` when the extension is present)
- `testdata/gltf/draco/` (Box, MorphPrimitivesTest, RiggedSimple, Avocado and WaterBottle `.gltf`/`.bin`, with licences)

**Interfaces:**
- Produces: `pub fn decode_primitive(stream: &[u8], attributes: &[(String, u32)], limits: Limits) -> Result<DecodedPrimitive, MeshError>`, where `pub struct Limits { pub max_points: usize, pub max_faces: usize, pub max_bytes: usize }` and `DecodedPrimitive { indices: Vec<u32>, attributes: Vec<(String, DecodedAttribute)> }`.

- [ ] Write:
  - the four sample differentials (positions within the header's quantization step, the triangle set by position triples, attributes within quantization, skin weights within 1/255);
  - `a_constant_attribute_cannot_expand_past_the_limits`;
  - `a_draco_primitive_respects_the_scene_caps`;
  - Review Focus 3–5 as `a_draco_attribute_mismatch_is_refused`, `a_draco_strip_decodes_as_a_list` and `draco_with_quantization_dequantizes_after_decoding`;
  - the sweeps on Box and MorphPrimitivesTest;
  - the isolation test for `draco_core`.
- [ ] Run `cargo test --lib -- mesh::glb mesh::draco mesh::isolation_tests`. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test --lib mesh::`. Expected: pass.
- [ ] Negative control: skip dequantization. The Box differential fails. Restore by rewriting.

### Task 4b: Bit-exact against Google's decoder, and the supply chain

**Files:** `litegen-core/src/mesh/testdata/gltf/draco/reference/` (encoded streams and Google-decoded expected dumps, our content; README with the exact commands and the google/draco commit), and a one-off `make_reference.sh` there documenting them.

- [ ] Build `draco_encoder` and `draco_decoder` from google/draco `d429de84b52a` (CMake, once, outside the repo). Encode the corpus: every fixture's geometry plus the committable samples' meshes, at `-cl 0,1,5,7,10` and `-qp 8,11,14,16`, with normals, UVs, colours, tangents and generic attributes. Decode each with `draco_decoder`, and dump attributes and indices.
- [ ] Write `draco_core_matches_googles_decoder`: for every stream, our `draco::decode_primitive` output equals Google's exactly, values and connectivity.
- [ ] Run it. Expected: pass. Any difference is a decoder disagreement to report upstream and record, never a tolerance to widen.
- [ ] Run `cargo audit` (install `cargo-audit` if missing) and `cargo tree -p draco-core -e normal`. Expected: no advisory, and `bytemuck` as the only dependency. Record both and the crate's `unsafe_code = "forbid"` line in the ledger.

### Task 5: Harness, docs and gate

**Files:** `scripts/mesh-verify/page/render.js` (a native original loads with `DRACOLoader` and `MeshoptDecoder` from three r183's `examples/jsm/libs`), and the docs listed in the spec.

- [ ] Run `--import` over every Khronos Draco and meshopt variant (local) and the gltfpack files. Expected: native 0.00 %, and every direction passes.
- [ ] Update the docs: `glb.rs`'s module doc, `3d-format-conversion.md` §3 and §9, and the model3d-provider skill's delivery section (including the quantization correction). Tell litegen-41 the TokenHub blocker status.
- [ ] Run the overview's full gate. Write the ledger.
