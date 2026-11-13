# Draco and meshopt glTF input — design

Part of the mesh-gaps set ([overview](2026-11-04-mesh-gaps-overview-design.md)). Size: L
(meshopt M, Draco L). Research: [`research-codecs.md`](../research/research-codecs.md)
§1–2. It's git-ignored, so its key facts are copied here.

## Why

The glTF reader refuses compressed geometry by name:

- `KHR_draco_mesh_compression`, when required → "{where} is Draco-compressed"
  (`glb.rs`);
- `EXT_meshopt_compression` on any bufferView → "bufferView {n} is
  EXT_meshopt_compression-encoded", including when it's only *used*, because its
  fallback may be placeholder bytes.

A generation whose vendor ships either fails **after the vendor has billed**. Delivery
reads the vendor's mesh first, so the promised containers can't be made.
`mock/partial-3d` models exactly that failure. Two other things depend on this:

- litegen-41 deferred Tencent TokenHub's Hunyuan 3D routes partly over this risk
  (`scripts/model-drift/providers/tokenhub.mjs`: "a live run … must first prove its GLB
  reads (no Draco/meshopt)");
- gltfpack, the most common glTF optimiser, writes meshopt by default, and every
  Khronos Draco sample requires Draco with no fallback.

## What the research established

- **meshopt:**
  - `EXT_meshopt_compression` is ratified, and `KHR_meshopt_compression` is a Release
    Candidate (added 2026-01-23).
  - KHR adds vertex format v1 (header `0xa1`) and a COLOR filter. v0 is identical in
    both.
  - Three modes:
    - ATTRIBUTES: `0xa0` or `0xa1`, 16-element groups, byte lanes;
    - TRIANGLES: `0xe1`, a code byte per triangle, LEB128 extras, a 16-byte code-aux
      table;
    - INDICES: `0xd1`, zig-zag LEB128 deltas.
  - Filters: OCTAHEDRAL (byteStride 4 or 8), QUATERNION (8), EXPONENTIAL (multiple of
    4; must decode exactly) and COLOR (4 or 8; KHR only, YCoCg). Every filter but
    EXPONENTIAL may be off by 1 ULP.
  - The decode paths are small: about 258 + 190–250 + 125–150 C++ code lines, and the
    JS reference decoder is 349 code lines.
  - An existing 804-line safe Rust port suggests about 1 k lines for ours.
  - Upstream warns its index decoder "may produce garbage data (e.g. out of range
    indices)" on hostile input.
- **Draco:**
  - Every Khronos sample is bitstream 2.2 TRIANGLES (15 EdgeBreaker, one sequential:
    MorphPrimitivesTest), and all require the extension with no fallback.
  - No decoder is both pure Rust and mature:
    - `draco-core` 2.3.3 (Apache-2.0) is pure Rust and `unsafe_code = "forbid"`,
      fuzzed, with `DecodeLimits`, depending only on `bytemuck`. But it was first
      published 2026-06-23 by one maintainer, with 16 releases and two majors in four
      months.
    - `draco-oxide` is alpha, 2.2-only, and has `get_unchecked` in hot paths.
    - The other crates wrap C++ (CMake and a C++ compiler at build time).
  - A std-only hand port of the 2.2 mesh decoder is estimated at **6–10 k lines**.
  - **Draco can't be bounded by its input size.** A constant attribute entropy-codes
    to a few bytes whatever its count: 100,000 points in 171 bytes. So the limit must
    be absolute.

## Decisions

### meshopt (D2, settled: std-only, no crate)

- A new std-only file, `mesh/meshopt.rs`, decodes EXT (v0) and KHR (v0 and v1), all
  three modes and all four filters. It follows the KHR spec's appendices, with
  meshoptimizer's JS reference decoder as the cross-check. It isn't a translation of the
  C++ SIMD paths. It joins `mod.rs` and `MODULE_FILES`, and uses no crate.
- **In `glb.rs`:** a bufferView carrying either extension is decoded into an owned
  buffer of exactly `count × byteStride` bytes, and accessors read that.
  - The declared sizes are checked against the existing decode budget
    (`MAX_DECODED_VALUES`, as `count × byteStride / 4` values) **before** allocating.
  - The JSON constraints are enforced: `byteLength = byteStride × count`, ATTRIBUTES
    `byteStride` a multiple of 4 in 4..=256, TRIANGLES `count % 3 == 0`, the index
    modes `byteStride` 2 or 4, and the filters' stride rules.
  - A stream that ends early, or leaves bytes before its tail, is `Malformed` (the
    spec's validity rule).
  - Decoded indices are range-checked against the vertex count, as every index already
    is.
  - Fallback buffers (`"fallback": true`, no `uri`) are allowed to be missing.
- Both extensions leave `IGNORABLE_REQUIRED_EXTENSIONS`' refusal path and join
  `CARRIED_EXTENSIONS`. Read, not written: litegen's glTF output stays uncompressed, so
  no delivered file requires an extension.

### Draco (D2, settled with the user: `draco-core`, fully tested)

The options weighed, and the one chosen:

1. **Chosen: `draco-core`:** pinned exactly
   (`=2.3.3`), with `default-features = false, features = ["decoder",
   "edgebreaker_valence_decode"]` (the second feature added under the plan's Ruling
   B29: `["decoder"]` alone can't decode valence EdgeBreaker, which 10 of the 16
   Khronos Draco samples use), confined to a
   new `mesh/draco.rs` that the isolation test's `ALLOWED` list names. Widening
   `ALLOWED` is the user's call under the standalone rule, and this is that call.
   - Its `DecodeLimits` are set from the scene caps: at most `MAX_SCENE_VERTICES`
     points, `MAX_SCENE_INDICES / 3` faces, and a decoded-bytes ceiling equal to
     glb.rs's budget.
   - Every decoded attribute is copied into the scene through the same budgeted
     accessor path as uncompressed data.
   - Upgrades are deliberate: the version pin changes only with a re-run of the Draco
     test set.
   - Risk: bus factor 1 and a young API. Mitigation: everything outside `draco.rs`
     sees one function, `decode_primitive(bytes, attribute_ids, limits) ->
     Result<DecodedPrimitive, MeshError>`, so swapping the decoder touches one file.
2. *Rejected:* hand-port the 2.2 mesh decoder (6–10 k lines, std-only). This is the only option
   needing no crate. It's large, and every prediction scheme and both connectivity
   coders must match Google's output.
3. *Rejected:* keep refusing Draco and build only meshopt, revisiting when a carried vendor's
   real output needs it. The TokenHub live run is the likely trigger.

**"Fully tested", for a decoder we didn't write.** The user's bar for the new crates
(D3: "make sure it's fully tested completely") applies here too:
- **Bit-exact against Google's own decoder.** Build Google's `draco_encoder` and
  `draco_decoder` once from google/draco at `d429de84b52a` (1.5.9). Then encode a
  corpus with `draco_encoder`:
  - every fixture's geometry;
  - plus the committable samples' meshes;
  - at compression levels 0 (sequential), 1, 5, 7 and 10 (EdgeBreaker standard and
    valence);
  - at position quantization 8, 11, 14 and 16 bits, with normals, UVs, colours,
    tangents and generic attributes.

  Decode every stream with `draco_decoder`, and commit its output (raw attribute and
  index dumps, our content) as expected values. `draco_core_matches_googles_decoder`
  requires our decode to equal it **exactly**: the same dequantized values and the
  same connectivity, in Google's order.
- **Hostile input:** the overview's sweeps on every committed stream, plus crafted
  headers (huge declared counts, bad prediction-scheme ids, truncated rANS data). No
  panic, and each fails inside the absolute limits.
- **Supply chain:**
  - `cargo audit` reports no advisory for `draco-core =2.3.3`;
  - `cargo tree` shows `bytemuck` as its only runtime dependency;
  - the crate's `unsafe_code = "forbid"` is checked in its manifest at the pinned
    version and recorded.

  All three are rerun whenever the pin changes, and the pin changes only with this
  whole set rerun.

### The mock's undecodable GLB

`generate_undecodable_glb` declares `KHR_draco_mesh_compression` required, which
becomes readable. It switches to a required extension litegen will never implement:
`EXT_litegen_mock_undecodable`. The construct stays "a file litegen refuses", and
`mock/partial-3d`'s tests keep their meaning. `model3d_deliver.rs`'s and
`tests/model3d_api.rs`'s Draco comments follow.

## Non-goals

- Writing Draco or meshopt.
- Draco point clouds (no glTF use) and Draco bitstreams before 2.2 (no sample uses
  them). They're refused by name.
- `KHR_mesh_quantization` changes. It's already read. The model3d-provider skill's
  claim that it's refused is corrected here.

## What proves it is done

### meshopt unit tests (meshopt.rs, glb.rs), RED first

- **One test per mode and filter, from hand-built streams:**
  - `attributes_v0_decode`, `attributes_v1_control_modes_decode` (modes 0–3 and the
    per-channel delta modes);
  - `triangles_decode`, `index_sequence_decodes`;
  - `octahedral_filter_8_and_16_bit`, `quaternion_filter`,
    `exponential_filter_is_exact`, `color_filter_ycocg`.

  Expected values come from meshoptimizer's JS reference decoder
  (`js/meshopt_decoder_reference.js` at `f0a684f11ead`), run once by a script whose
  outputs are committed as test vectors: our streams, so our content.
- **The validity rules:**
  - `a_stream_that_ends_early_is_malformed`;
  - `bytes_left_before_the_tail_are_malformed`;
  - `a_bad_header_byte_is_malformed` (`0xa2`, `0xe0`, `0xd0`);
  - `stride_rules_are_enforced`;
  - `declared_sizes_are_budgeted_before_allocating` (a `count` × `byteStride` past the
    budget is refused with no allocation; the test pins the error);
  - `out_of_range_decoded_indices_are_refused`.
- `a_meshopt_view_with_a_missing_fallback_reads`: `"fallback": true`, no `uri`.

### meshopt differential tests

- **`MeshoptCubeTest/glTF-Meshopt`** (CC0, 97.5 KiB, so committable in `testdata/gltf/`
  with a README line). It covers v0 and v1, every mode, and every filter including
  COLOR. Our read must equal the JS reference decoder's reading of every bufferView:
  byte-equal, with 1 ULP allowed per the spec except for EXPONENTIAL. The scene must
  equal its `glTF` (uncompressed) variant's within the quantization tolerance.
- **gltfpack on our own fixtures:** every `fixtures::all()` GLB through gltfpack 1.0+,
  committed as our content.
  - With `-c` (no filters), each compressed file's accessors must be **bit-equal** to
    those of gltfpack's uncompressed output of the same input (the same command without
    `-c`; both quantize). The builder checks once that gltfpack quantizes alike with
    and without `-c`, and records it. If it doesn't, the comparison falls back to the
    next bullet's tolerance.
  - With `-cc` and `-cz` (filters, and KHR v1 with COLOR), the decoded values are
    compared with the uncompressed output within each filter's quantization:
    OCTAHEDRAL normals and tangents within the filter's bit depth, QUATERNION within
    its, EXPONENTIAL exactly, COLOR within 1/255 at 8 bits.
- **Hostile input:** the overview's truncation and bit-flip sweeps on MeshoptCubeTest
  and on one gltfpack file per mode.

### Draco tests, RED first

- **Committable CC0/CC-BY samples** (`testdata/gltf/draco/`, README with licences):
  - `Box` (2.5 KiB, standard EdgeBreaker);
  - `MorphPrimitivesTest` (the only sequential stream);
  - `RiggedSimple` (skinned, standard EdgeBreaker);
  - `Avocado`'s `.gltf` and `.bin` (standard EdgeBreaker, with TANGENT);
  - `WaterBottle`'s `.gltf` and `.bin` (CC0, **valence** EdgeBreaker, which 10 of the 16
    samples use).

  Each Draco variant's scene must equal the same model's `glTF` variant:
  - positions within the Draco quantization step (from each stream's header);
  - the same triangle set, compared as sets of position triples, since Draco may
    reorder vertices;
  - normals, UVs and tangents within quantization;
  - skin weights within 1/255;
  - morph targets within quantization.
- Never committed (licences): BrainStem, VirtualCity, Duck. They're run locally in
  `--import`.
- `a_constant_attribute_cannot_expand_past_the_limits`: a crafted stream of a few
  hundred bytes declaring 100M points is refused by the absolute limit, not by
  allocation. (draco-core's docs measure exactly this shape of input.)
- `a_draco_primitive_respects_the_scene_caps`: a sample re-encoded at a vertex count
  past `MAX_SCENE_VERTICES` is refused `Unsupported`, naming the cap.
- The sweeps on Box and MorphPrimitivesTest.
- `isolation_tests` with `draco_core` added to `ALLOWED` for `draco.rs` only, and a
  test that no other mesh file names it.
- `draco_core_matches_googles_decoder`: the bit-exact corpus above.

### Delivery and mock

- `mock_partial_3d_still_refuses_its_glb`: the new `EXT_litegen_mock_undecodable` GLB
  is refused, and `mock/partial-3d`'s existing API tests pass unchanged.
- `a_vendor_meshopt_glb_is_delivered` (model3d_api): a mock row returning gltfpack
  `-cc` output delivers every promised container, and the delivered GLB is
  uncompressed (no meshopt in `extensionsUsed`, so `gltf_extensions` reports none).
  The same for a Draco GLB.

### Render harness

`--import` the Khronos Draco and meshopt variants (local, all of them) and the gltfpack
files. The **native** check renders the original with three.js `GLTFLoader` plus its
`DRACOLoader` and `MeshoptDecoder` (in `scripts/mesh-verify/node_modules/three`),
against our reading: 0.00 %, every direction passing.

### Negative controls

- meshopt: flip the zig-zag sign in INDICES mode. `index_sequence_decodes` and the
  MeshoptCubeTest differential fail.
- Draco: skip the dequantization step. The Box differential fails.

Restore each by rewriting.

### Docs

- `glb.rs`'s module doc ("Refused: Draco and meshopt" goes);
- `3d-format-conversion.md` §3 and §9;
- `.claude/skills/model3d-provider/SKILL.md`: the delivery section's "Draco, meshopt
  and `KHR_mesh_quantization` are all refused" becomes what's read. Quantization has
  been read since 2026-10-12;
- tell litegen-41: the TokenHub note's Draco/meshopt blocker is lifted.

### Gate

The overview's full gate, and the harness `--import` run above.

## Left to the builder

- The internal shape of `meshopt.rs`. Port from the spec appendices and the JS
  reference.
- The test-vector script for the JS reference (Node, run once, outputs committed).
