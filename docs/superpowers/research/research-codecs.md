# Codec research: Draco, meshopt, KTX2/Basis Universal, USDC writing

Date: 2026-11-04. Written for the owners of `litegen-core/src/mesh/`, a std-only Rust
library. Today it may use serde_json, miniz_oxide, quick_xml and image. Every reader
there takes hostile input.

Nothing was built or run: no cargo, and no execution of downloaded code.

**How the facts were gathered**

- Crate facts come from the crates.io API (`/api/v1/crates/<name>`, versions, reverse
  dependencies) and from the published `.crate` tarballs, which were downloaded and
  read with `grep` and `wc`.
- "`unsafe` lines" counts the lines under `src/` that contain the word `unsafe`.
  Comments are included, so it is an upper bound. Any `forbid` or `deny` lint is
  named.
- "Code lines" means non-blank lines that are not `//` comments. It is approximate.
- Upstream repositories were shallow or sparse clones at the commits named.
- Sample-asset facts come from parsing the real files at glTF-Sample-Assets commit
  `8f492390ddc7` (2026-10-23).

**Confidence levels**

- **High**: read in the primary source.
- **Medium**: inferred from primary sources.
- **Low**: comes from a secondary or indirect source.

**A caution about the ecosystem**

Most pure-Rust candidates below were first published in 2026, several of them in the
last six weeks. Most have one maintainer, and several repos carry agent-workflow files
such as `AGENTS.md`. Treat their parity and fuzzing statements as claims made in
their own docs. Nothing here verified them.

---

## 1. Draco (KHR_draco_mesh_compression)

### 1.1 What the extension requires

- Spec: [KHR_draco_mesh_compression README](https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Khronos/KHR_draco_mesh_compression/README.md). (High)
  - "This extension specification is based on Draco bitstream version 2.2".
  - "the `mode` of `primitive` must be either `TRIANGLES` or `TRIANGLE_STRIP`".
  - Loaders "must ignore the `bufferView` and `byteOffset` of the `accessor`" and "must use the decompressed data to fill the `accessors`".
  - Draco "may change the order and number of vertices in a mesh".
  - When no uncompressed fallback is provided, the extension must be listed in `extensionsRequired`.
- The normative bitstream spec is <https://google.github.io/draco/spec/>, marked "Version 2,2 — Released 2017-10-25" (page last modified 2026-10-16). Its sections cover:
  - Sequential connectivity.
  - EdgeBreaker connectivity, plus the Valence and Prediction-Degree traversals.
  - Attribute decoders: sequential integer, quantization and normal.
  - Prediction schemes: difference, parallelogram, multi-parallelogram and constrained multi-parallelogram, tex-coords, and geometric normal.
  - Wrap and normal-octahedron transforms.
  - rANS decoding.

  The fetched page was truncated, so point-cloud and KD-tree coverage was not checked. (High)
- Upstream is [google/draco](https://github.com/google/draco) at `d429de84b52a` (2026-10-16), Apache-2.0. (High)
  - 1.5.9 (2026-10-16) fixed JS/WASM exports.
  - 1.5.8 (2026-10-15) lists "Security fixes" and adds experimental "Polygonal Mesh Coding (PMC)".
  - Before that, 1.5.7 (2024-01-17).

### 1.2 Rust candidates

| Crate | Latest (date), first release | Licence | Kind, and build needs | `unsafe` | Maintenance | What it decodes |
|---|---|---|---|---|---|---|
| [draco-core](https://crates.io/crates/draco-core) | 2.3.3 (2026-11-01); first 1.0.0 on 2026-06-23; 16 releases | Apache-2.0 | Pure Rust, no C/C++. One dependency: `bytemuck` with `derive`, which brings proc-macro build deps. MSRV 1.88 | `[lints.rust] unsafe_code = "forbid"`. No `unsafe` code; its 3 mentions are comments | Repo [Filyus/draco-rust](https://github.com/Filyus/draco-rust): created 2026-04-26, 5 stars, 1 contributor (1,532 commits), contains `AGENTS.md`. SECURITY.md: malformed input must "fail as a `DracoError`, not panic, hang, or over-allocate". cargo-fuzz and ClusterFuzzLite targets include `decode_drc` and `draco_gltf_import`. Callers can set `DecodeLimits`, which default to 256 M points, 512 M faces and 2 GiB decoded. An input-ratio allocation backstop runs behind them | **Meshes:** sequential, EdgeBreaker standard and valence, and legacy predictive connectivity. Every bitstream from 1.1 to 2.2. **Prediction schemes:** NONE (−2), DIFFERENCE (0), PARALLELOGRAM (1), MULTI_PARALLELOGRAM (2), TEX_COORDS_DEPRECATED (3), CONSTRAINED_MULTI_PARALLELOGRAM (4), TEX_COORDS_PORTABLE (5), GEOMETRIC_NORMAL (6). **Transforms:** WRAP, NORMAL_OCTAHEDRON (legacy), NORMAL_OCTAHEDRON_CANONICALIZED. **Entropy coding:** rANS bit and symbol coding, tagged and raw symbols, direct and folded-bit32 coders. **Also:** quantization, every integer and float type (64-bit types are carried raw through GENERIC), metadata, and point clouds (sequential and KD-tree). Claims byte-exact parity with C++ Draco 1.5.7 (SUPPORT_MATRIX.md in the crate) |
| [draco-oxide](https://crates.io/crates/draco-oxide), with [-decoder](https://crates.io/crates/draco-oxide-decoder) and -core | 0.1.0-alpha.12 (2026-10-20); first alpha 2025-07-14 | MIT OR Apache-2.0 | Pure Rust. The decoder depends on -core, remain and thiserror. -core depends on kiddo, paste, seq-macro, serde, serde_json, thiserror and draco-nd-vector | 44 lines in the decoder (`get_unchecked`, `unsafe fn next_unchecked` and unchecked EdgeBreaker tables in hot paths). 160 lines in -core | Repo [reearth/draco-oxide](https://github.com/reearth/draco-oxide) belongs to an org. 23 stars, 7 contributors (58 commits by the lead). The API is alpha | **Bitstream 2.2 meshes only.** The header check `(major, minor) != (2, 2)` returns `UnsupportedVersion`. Sequential and EdgeBreaker standard and valence connectivity. Prediction schemes 0, 1, 2, 4, 5, 6 and NONE; not 3. rABS, plus a dequantization feature. Point clouds behind a feature, KD-tree 2.3 only; sequential point clouds are `Unimplemented` |
| [draco_decoder](https://crates.io/crates/draco_decoder) | 0.0.31 (2026-09-25); 31 releases since 2025-08-10 | MIT OR Apache-2.0 | Bindings through `cxx` to a vendored copy of Google Draco C++: 465 C/C++ files, about 77 k lines. `build.rs` runs `cmake`, so it **needs CMake and a C++ compiler**; on Windows only MSVC. WASM builds use Google's Emscripten JS decoder | FFI | jiangheng90, 5 stars, described as "wip" | Everything C++ Draco decodes |
| [draco-rs](https://crates.io/crates/draco-rs) | 0.1.3 (2025-04-28) | MIT | `autocxx` and `cmake` bindings to a *fork* of Draco. Needs a C++ toolchain | FFI | 0 stars, 4 releases, all in April 2025 | C++ (fork) |
| [spatial_codec_draco](https://crates.io/crates/spatial_codec_draco) | 0.2.5 (2026-03-06) | MIT OR Apache-2.0 | `cmake` and `cbindgen` wrapper of upstream Draco | FFI | — | **Point clouds only.** No use for glTF meshes |
| [bevy_gltf_draco](https://crates.io/crates/bevy_gltf_draco) | 0.2.1 (2026-09-03) | MIT OR Apache-2.0 | Bevy 0.19 plugin built on `draco_decoder`, so C++ | — | 6 stars | Through C++ |
| [draco-gltf](https://crates.io/crates/draco-gltf) | 0.7.0 (2026-10-10) | Apache-2.0 | glTF and GLB layer on top of draco-core | 0 lines | Same repo as draco-core | — |
| [oxideav-gltf](https://crates.io/crates/oxideav-gltf) | 0.0.4 (2026-09-01) | MIT | Pure-Rust glTF codec | `forbid` | — | **Does not decode Draco.** Its README says "the Draco bitstream inflate path is out of scope for this round" |

The crates.io crate named `draco` (0.1.2, 2019) is an unrelated web framework. (All rows: High)

### 1.3 Production use

- **Bevy:** the [bevy_gltf 0.20.1 docs](https://docs.rs/bevy_gltf/latest/bevy_gltf/) (2026-10-28) mark `KHR_draco_mesh_compression` ❌, along with `EXT_meshopt_compression` ❌ and `KHR_texture_basisu` ❌. Bevy users rely on the third-party `bevy_gltf_draco`, which wraps C++. (High)
- **The `gltf` crate** (1.4.1, 2024-05-10) has no Draco feature or support. (High)
- **The only production-grade user found** is the Foxglove SDK ([`foxglove`](https://crates.io/crates/foxglove) 0.30.3, about 682 k downloads). It takes draco-core ^2.0 as an optional dependency behind the `remote-access` feature. Its `src/draco.rs` uses the point-cloud *encoder*; the decoder appears only in tests. That is not mesh decoding. (High)
- **Other reverse dependencies of draco-core:** three-rs, viewport-lib-io, catwalk3d and brainglobe-atlasapi.
- I found no widely used Rust glTF loader that decodes Draco natively. (Medium)

### 1.4 How big a std-only hand port would be

Upstream C++ at `d429de84b52a`:

- **`src/draco/compression` (non-test `.h`/`.cc`):** 26,084 lines. By filename: decoder 9,947, encoder 11,235, shared 4,902.
- **Per directory** (dec / enc / shared lines):
  - `attributes`: 1,956 / 1,776 / 802
  - `attributes/prediction_schemes`: 2,025 / 2,192 / 1,138
  - `bit_coders`: 566 / 578 / 43
  - `entropy`: 381 / 713 / 839
  - `mesh`: 2,586 / 2,226 / 129
  - `mesh/traverser`: 0 / 76 / 598
  - `point_cloud`: 466 / 846 / 0
  - `point_cloud/algorithms` (KD-tree): 1,105 / 1,069 / 269
  - `pmc` (new PMC codec, not part of KHR_draco): 613 / 759 / 473
- **Include closure of `compression/decode.cc`:** 185 files, 25,361 lines (16,459 code lines). It drags in materials, textures and structural metadata because `Mesh` includes them.
- **Mesh-decoding subset of that closure:** 141 files, 20,088 lines (13,168 code lines). It drops KD-tree point clouds, materials, textures, structural metadata, mesh features, encoders and options.

Rust ports, for scale:

- **draco-oxide-decoder:** 5,943 lines (4,810 code lines) for 2.2 meshes plus KD-tree point clouds. It also relies on draco-oxide-core, 6,915 lines that it shares with the encoder.
- **draco-core, decoder-side files:** about 24,094 lines (17,318 code lines). That covers every legacy bitstream and the C++-parity work. The whole crate, encoder included, is 46,833 lines.

**Estimate:** a decode-only port for 2.2 meshes, with both connectivity coders and all prediction schemes, would be **about 6–10 k lines of Rust before tests**. Legacy bitstreams (≤2.1) would add more. (Medium)

### 1.5 Test assets

All glTF-Sample-Assets models with a `glTF-Draco` variant live in `Models/<Model>/glTF-Draco/<Model>.gltf`. Licences come from each model's `metadata.json` (SPDX). Stream facts come from parsing the Draco headers of every compressed primitive.

| Model | .gltf+.bin | Whole folder | Draco streams | Licence |
|---|---:|---:|---|---|
| Avocado | 11.5 KiB | 7.7 MiB (PNGs) | 1 × v2.2 EdgeBreaker standard; POSITION, NORMAL, TANGENT, TEXCOORD_0 | CC0-1.0 |
| BarramundiFish | 45.5 KiB | 11.8 MiB | 1 × valence | CC0-1.0 |
| BoomBox | 72.8 KiB | 10.0 MiB | 1 × valence | CC0-1.0 |
| Box | 2.5 KiB | 2.5 KiB | 1 × standard; POSITION, NORMAL | CC-BY-4.0 (Cesium) |
| BrainStem | 1,942.7 KiB | same | 37 standard + 22 valence; JOINTS_0, WEIGHTS_0 | **LicenseRef-Poser-EULA** |
| CesiumMan | 138.1 KiB | 291.4 KiB | 1 × valence; skinned | CC-BY-4.0 + Cesium trademark |
| CesiumMilkTruck | 20.8 KiB | 234.7 KiB | 3 standard + 1 valence | CC-BY-4.0 + Cesium trademark |
| Corset | 227.8 KiB | 12.5 MiB | 1 × valence | CC0-1.0 |
| Duck | 13.8 KiB | 29.8 KiB | 1 × valence | **SCEA** shared-source licence |
| Lantern | 85.7 KiB | 9.0 MiB | 1 standard + 2 valence | CC0-1.0 |
| MorphPrimitivesTest | 6.0 KiB | 54.4 KiB | 2 × **sequential** (the only sequential sample) | CC-BY-4.0 |
| RiggedFigure | 55.8 KiB | same | 1 × standard; skinned | CC-BY-4.0 |
| RiggedSimple | 11.8 KiB | same | 1 × standard; skinned | CC-BY-4.0 |
| SunglassesKhronos | 46.1 KiB | 61.7 KiB (WebP) | 5 valence + 3 standard; also requires `EXT_texture_webp` | CC-BY-4.0 + Khronos trademark |
| VirtualCity | 1,840.9 KiB | 2.6 MiB | 167 × standard | **LicenseRef-3DRT-Testing** |
| WaterBottle | 51.8 KiB | 8.5 MiB | 1 × valence | CC0-1.0 |

- Every stream is a **bitstream v2.2 mesh in TRIANGLES mode**.
- All 16 models list `KHR_draco_mesh_compression` in `extensionsRequired`, with no uncompressed fallback.
- Two more Draco assets also carry KTX2 textures:
  - `CarConcept/glTF-KTX-BasisU-Draco`: 3.4 MiB, CC-BY-4.0 plus Khronos trademarks.
  - `ABeautifulGame/glTF-Binary-KTX-ETC1S-Draco`: one 11.5 MiB GLB written by glTF-Transform v4.0.8, CC-BY-4.0 (ASWF).
- (All High)

### 1.6 What this means for a std-only decoder

- **No Draco decoder is both pure Rust and mature.**
  - The pure-Rust choices are draco-core (four months on crates.io, one maintainer, no `unsafe`, fuzzed, with limits) and draco-oxide (alpha, `get_unchecked` in decoder hot paths, 2.2 only).
  - Every C++ wrapper needs CMake and a C++ compiler at build time.
- **If the owner allows one crate, draco-core fits the reader rules best.** Use it with `default-features = false, features = ["decoder"]`.
  - It forbids `unsafe`, documents a hostile-input posture, and depends only on `bytemuck`.
  - The risks are bus factor 1 and churn: 16 releases and two major versions in four months.
- **A hand port is large** (about 6–10 k lines). The 2.2-only scope matches every sample asset. Use the Draco spec and the C++ as references.
- **Draco cannot be bounded by its input size.** draco-core's docs measure 100,000 points in 171 bytes, because a constant attribute entropy-codes to a size independent of its count. A reader needs an **absolute** ceiling on decoded points, faces and bytes. A ratio to input size alone would reject valid files.
- **Commit only CC0 or CC-BY fixtures.**
  - Box (tiny).
  - MorphPrimitivesTest (sequential).
  - RiggedSimple (skinning).
  - The .gltf and .bin of Avocado, WaterBottle or Corset (valence and tangents).
  - Avoid BrainStem, VirtualCity and Duck, whose licences are not CC.

---

## 2. meshopt (EXT_meshopt_compression and KHR_meshopt_compression)

### 2.1 Specs and status

- **EXT_meshopt_compression:** [spec](https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Vendor/EXT_meshopt_compression/README.md), status "Complete, Ratified by the Khronos Group". Last spec change: `5ccac1cb0f` (2025-09-05), "Fix K-bits range for octahedral filter". (High)
- **KHR_meshopt_compression:** [spec](https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Khronos/KHR_meshopt_compression/README.md), status **"Release Candidate"**. It was added on 2026-01-23 (`3e9dfff91d`, PR #2517). It is not in the registry index's ratified list as of glTF `main` at `9d23a5a5b1` (2026-11-03). (High)
  - Appendix C lists the differences from EXT:
    - the vertex format v1, with control modes and channel modes;
    - v0 is still allowed and is "identical to that defined in `EXT_meshopt_compression`";
    - a new COLOR filter.
  - meshoptimizer's [v1 notes](https://meshoptimizer.org/v1.html) say KHR "is in the process of being finalized" and "gltfpack continues to use EXT_meshopt_compression by default".

### 2.2 Bitstream sections, from the KHR spec appendices

**Mode 0, ATTRIBUTES**

- Header byte: `0xa0` for v0 or `0xa1` for v1. EXT allows only `0xa0`.
- Blocks of up to `min((8192 / byteStride) & ~15, 256)` elements.
- Within a block: 16-element groups; one data block per byte lane, with 2-bit group headers; sentinel encodings of 1, 2 or 4 bits.
- v1 adds a per-block control header (2 bits per byte lane):
  - mode 0: bit lengths {0, 1, 2, 4}
  - mode 1: bit lengths {1, 2, 4, 8}
  - mode 2: "All delta bytes are 0; no data is stored for this byte"
  - mode 3: literal
- v1 also adds per-channel delta modes in the tail block.
- The tail block holds the baseline element and is padded to at least 24 bytes (v1) or 32 bytes (v0).

**Mode 1, TRIANGLES**

- Header `0xe1`.
- One code byte per triangle, then varint-7 (LEB128) extra data, then a 16-byte trailing code-aux table.
- meshoptimizer also has a v0 `0xe0`, but the glTF specs require `0xe1`.

**Mode 2, INDICES**

- Header `0xd1`.
- Zig-zag varint-7 deltas.

**Filters** (ATTRIBUTES only)

| Filter | Allowed `byteStride` | Notes |
|---|---|---|
| 1 OCTAHEDRAL | 4 or 8 | |
| 2 QUATERNION | 8 | |
| 3 EXPONENTIAL | multiple of 4 | Must be decoded exactly |
| 4 COLOR | 4 or 8 | KHR only, YCoCg |

All other filters may be off by 1 ULP.

**JSON constraints**

- `byteLength` = `byteStride` × `count`.
- ATTRIBUTES needs a `byteStride` that is a multiple of 4, between 4 and 256.
- In TRIANGLES mode `count` % 3 == 0.
- The index modes need a `byteStride` of 2 or 4.

**Validity**

- "If the decoding procedure reaches the end of stream too early, or any unprocessed bytes remain after decoding and before tail, the stream is invalid."
- Fallback buffers may have no `uri`, with `"fallback": true`.

(All High)

### 2.3 Upstream source sizes

From [zeux/meshoptimizer](https://github.com/zeux/meshoptimizer) at `f0a684f11ead` (2026-10-14; v1.3 was released 2026-09-25; MIT). Code lines are counted by line range. (High for totals; Medium for the decode-only split)

| File | Total | Scalar decode path | Other parts |
|---|---:|---|---|
| `src/vertexcodec.cpp` | 1,897 | ≈258 code lines: shared helpers 33 (L120–165), scalar decoder 154 (L598–798), decode API 71 (L1799–1897) | SIMD decode 626; encoder 431 |
| `src/indexcodec.cpp` | 708 | ≈190–250 code lines: `meshopt_decodeIndexBuffer` 127 (L372–582), `meshopt_decodeIndexSequence` 36 (L652–708), part of 122 helper lines (L10–168) | encoders 157 |
| `src/vertexfilter.cpp` | 1,503 | ≈125–150 code lines: scalar filters 94 (L76–214), decode API 53 (L1211–1274, includes SIMD dispatch) | SIMD 619; encoders 168 |
| `js/meshopt_decoder_reference.js` | 472 | 349 code lines; "reference decoder implementation by Jasper St. Pierre". Covers v0 and v1, triangles, indices and all four filters including COLOR | — |

- `meshoptimizer.h` says the index decoder "is safe to use for untrusted input, but it may produce garbage data (e.g. out of range indices)".
- libFuzzer harnesses live in `tools/codecfuzz.cpp`.
- **A Rust port already exists inside another crate:** `bevy_3d_tiles_prepare` 0.3.0 has `src/meshopt.rs`, 804 lines including tests. It ports the JS reference to Rust and covers v0 and v1, all three modes and all five filters, with no `unsafe`.
- **Estimate for a complete std-only decoder: about 700–1,200 lines of Rust, plus tests.** (High)

### 2.4 Rust crates

| Crate | Latest (date) | Licence | Kind | Has the glTF decoders? | `unsafe` / notes |
|---|---|---|---|---|---|
| [meshopt](https://crates.io/crates/meshopt) | 0.7.0 (2026-10-17) | MIT OR Apache-2.0 | FFI. `build.rs` compiles the vendored meshoptimizer **1.3 C++** with `cc`, so **a C++ compiler is required** | Yes: `decode_vertex_buffer`, `decode_index_buffer`, `decode_index_sequence`, and `decode_filter_oct` / `_quat` / `_exp` / `_color` | 757 k downloads, 30 reverse deps including `bevy_pbr` (for meshlets, not glTF loading). Repo has 225 stars and is maintained |
| [meshopt-rs](https://crates.io/crates/meshopt-rs) | 0.1.2 (2022-11-12) | MIT | Pure Rust port at "meshoptimizer v0.15 feature level" | v0 only (errors when `version > 0`), so EXT only; no COLOR filter | 17 lines (transmute, `from_raw_parts`, unions). No release since 2022 |
| [meshopt-pure](https://crates.io/crates/meshopt-pure) | 1.3.0 (2026-10-30), the only release | MIT | Pure Rust port of v1.3 at commit `f0a684f`, claimed bit-identical | Yes: v0 and v1, triangles, sequences, all filters including COLOR | `#![deny(unsafe_code)]`; `unsafe` only in the SIMD module behind the `simd` feature, **on by default**. 1 star; repo created 2026-10-30 |
| [meshoptimizer-rs](https://crates.io/crates/meshoptimizer-rs) | 0.2.17 (2026-11-03); 19 releases since 2026-10-04 | MIT | Pure Rust port of 1.3, "byte-identical". Deps: `libm` (=0.2.16), and `rayon` (optional) | Yes: `decode_vertex_buffer`, `decode_index_buffer`, `decode_index_sequence`, `decode_buffer_view`, and filters including color | `forbid(unsafe_code)` when `simd` is off; default features are `std` and `simd`. Per-call work and byte `Limits`. 0 stars |
| [meshoptimizer](https://crates.io/crates/meshoptimizer) and -sys | 0.1.1 (2026-06-26) | MIT OR Apache-2.0 | FFI (`cc` + `bindgen`) to C++ 1.1.1 | Yes | Hosted on Codeberg; 73 downloads |
| [awsm-renderer-codec-meshopt](https://crates.io/crates/awsm-renderer-codec-meshopt) | 0.39.1 (2026-11-03) | MIT OR Apache-2.0 | EXT decode and encode through `meshopt` (C++) | Yes | — |
| [bevy_3d_tiles_prepare](https://crates.io/crates/bevy_3d_tiles_prepare) | 0.3.0 (2026-10-08) | MIT OR Apache-2.0 | Pure Rust, one file, deps serde_json and thiserror | Yes (see 2.3) | No `unsafe`. MSRV 1.95 |

- Bevy 0.20.1 does not support EXT_meshopt_compression, per the table in 1.3. (High)

### 2.5 Test assets

These are in glTF-Sample-Assets at `8f492390ddc7`. Header bytes were read from every compressed bufferView.

| Variant folder | Extension (generator) | Size | Streams | Filters | Licence |
|---|---|---:|---|---|---|
| `BrainStem/glTF-Meshopt` | KHR (gltfpack 1.0) | 400.4 KiB | 7 × ATTRIBUTES `0xa1`, 1 × TRIANGLES `0xe1` | OCTAHEDRAL, EXPONENTIAL, QUATERNION | **LicenseRef-Poser-EULA** |
| `BrainStem/glTF-Meshopt-EXT` | EXT (gltfpack 0.20) | 419.3 KiB | 7 × `0xa0`, 1 × `0xe1` | same | same |
| `DragonAttenuation/glTF-Meshopt` | KHR | 1,408.8 KiB (567 KiB of it images) | 3 × `0xa1`, 2 × `0xe1` | OCTAHEDRAL | CC0-1.0 + LicenseRef-Stanford-Graphics |
| `DragonAttenuation/glTF-Meshopt-EXT` | EXT | 1,685.8 KiB | 3 × `0xa0`, 2 × `0xe1` | OCTAHEDRAL | same |
| `MeshoptCubeTest/glTF-Meshopt` | KHR ("khrmeshopt-gen") | 97.5 KiB | 33 × `0xa0` + 11 × `0xa1` ATTRIBUTES, 12 × TRIANGLES, 4 × INDICES `0xd1` | NONE, OCTAHEDRAL, QUATERNION, EXPONENTIAL, **COLOR** | **CC0-1.0**. Best fixture: it covers every mode, filter and version |

- All five variants require `KHR_mesh_quantization` and the meshopt extension.
- Each one has a `"fallback": true` buffer with no `uri`.
- **Making more assets with gltfpack** (read from `gltf/gltfpack.cpp` and `gltf/stream.cpp` at `f0a684f1`):
  - `-c`: EXT_meshopt_compression with vertex v0.
  - `-cc`: EXT at a higher level, adding the OCTAHEDRAL filter (normals and tangents), QUATERNION and EXPONENTIAL.
  - `-cz` (= `-ce khr -cc`): KHR with vertex v1 (`meshopt_encodeVertexBufferLevel(..., version = level > 0)`), plus COLOR for colours (`col = filters && compresskhr && compressmore`).
  - `-ce khr|ext`: picks the extension.
  - `-cf`: adds an uncompressed fallback.
  - `-tc` / `-tu`: write KTX2 textures (ETC1S or UASTC).
- (High)

### 2.6 What this means for a std-only decoder

- This is the cheapest of the four gaps.
  - The format is small and fully specified.
  - There is a 349-line reference decoder in JS.
  - Upstream fuzzes its decoders.
  - A port is about 1 k lines and needs no new crate.
- Support EXT (v0) and KHR (v0 and v1, plus COLOR). gltfpack still writes EXT by default.
- Allocation is fixed by the JSON before decoding (`count` × `byteStride` = `bufferView.byteLength`), so cap the declared sizes absolutely.
  - Do not use an input-ratio check. v1 control mode 2 encodes a whole zero byte lane of a block in 2 bits, so legitimate expansion is very large.
- Decoded indices can be out of range by design ("may produce garbage data"). Validate them against the vertex count.
- Reject streams that end early, or that leave bytes before the tail; the spec makes those invalid.

---

## 3. KTX2 and Basis Universal → RGBA8

### 3.1 Spec facts

- **KTX 2.0:** [KTX File Format Specification](https://registry.khronos.org/KTX/specs/2.0/ktxspec.v2.html), "version 2.0.4, 2025-02-20". (High)
  - **Supercompression schemes:** 0 None, 1 BasisLZ, 2 Zstandard (RFC 8478), 3 ZLIB (RFC 1950, Deflate).
  - **Zstandard:** "Only Zstandard frames are required. Inflators may skip Skippable frames". After inflation a level has the uncompressed layout, which for UASTC is 16 bytes per 4×4 block.
  - **BasisLZ:** `uncompressedByteLength` "must be 0".
  - Appendix D, "BasisLZ/ETC1S Bitstream Specification", is normative. It covers compressed Huffman tables, endpoint codebooks, selector codebooks, slice Huffman tables and slice decoding.
- **KHR_texture_basisu:** [spec](https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Khronos/KHR_texture_basisu/README.md). (High)
  - **Allowed payloads:**
    - ETC1S with BasisLZ: `supercompressionScheme` "MUST be `1`", and the DFD colour model is `KHR_DF_MODEL_ETC1S`.
    - UASTC with scheme `0` (None) or `2` (Zstandard), and the colour model `KHR_DF_MODEL_UASTC`.
    - **ZLIB is not allowed** for Basis payloads.
  - **Mipmaps:** "When a texture refers to a sampler with mipmap minification or when the sampler is undefined, the KTX image **SHOULD** contain a full mip pyramid." That is a SHOULD, not a MUST.
  - **Colour spaces:**
    - Colour data: `colorPrimaries` MUST be `KHR_DF_PRIMARIES_BT709` and `transferFunction` MUST be `KHR_DF_TRANSFER_SRGB`.
    - Non-colour data: primaries MUST be `UNSPECIFIED` and the transfer `LINEAR`.
    - Red and Red-Green textures "MUST NOT be encoded with the sRGB transfer function".
    - KTX2 on its own "allows free choice of transferFunction when vkFormat is VK_FORMAT_UNDEFINED".
  - **Other rules:**
    - `pixelWidth` and `pixelHeight` MUST be multiples of 4.
    - `KTXswizzle` must be `rgba` or omitted; `KTXorientation` must be `rd` or omitted.
    - Material maps must be 2D.
    - The premultiplied-alpha flag must not be set unless the material requires it.
- **Picking the first mip:** use `levels[0]`, which is the base (largest) level. (High)
  - The Level Index "is ordered starting with level_base (the level with the largest size images) at index 0".
  - But "Mip level data is ordered from the level with the smallest size images". So take `levels[0].byteOffset` and `byteLength`; do not take the bytes that follow the index.
  - "levelCount = 0 … means that a file contains only the base level … loaders should generate other levels". This is "allowed, except for block-compressed formats". The reference Basis transcoder rejects `levelCount < 1`.
  - BasisLZ `imageDescs` are ordered level, then layer, then face, then z_slice, so level 0's descriptors come first.
  - `rgbSliceByteOffset` and `alphaSliceByteOffset` are offsets "within the levelImages of its mip level".
- **No colour conversion happens when transcoding.** The reference UASTC→RGBA32 path calls `unpack_uastc(..., srgb=false)`. The output is the stored values, sRGB-encoded or not, which is what PNG and the glTF material slots expect. (High)

### 3.2 Crates

| Crate | Latest (date) | Licence | Kind | `unsafe` | Notes |
|---|---|---|---|---|---|
| [ktx2](https://crates.io/crates/ktx2) | 0.5.0 (2026-04-19) | Apache-2.0 | **Pure Rust** container parser; `#![no_std]` with a `std` feature; one dependency, `bitflags` | 1 line (`new_unchecked` in a const macro) | 6.1 M downloads, 25 reverse deps including bevy_image. Parses the header, levels, DFD and key/value data. It does not decompress or transcode |
| [ruzstd](https://crates.io/crates/ruzstd) | 0.9.1 (2026-10-11) | MIT | **Pure Rust** Zstandard (RFC 8878). The README says the decompressor is "fully operational", 1.4–3.5× slower than C. With `default-features = false, features = ["std"]` it has no runtime deps | 38 lines in `decoding/ringbuffer.rs` (raw-pointer ring buffer) and 2 in `decode_buffer.rs` | Since 2019: 21 releases, 77 M downloads, 308 reverse deps. Users include bevy_image (`zstd_rust`), `object` (`compression` feature), `backtrace` (optional `ruzstd` feature), basisu and draco-texture. Fuzz targets: decode, decode_dict, encode, fse, huff0 and interop. Repo has 458 stars |
| [basis-universal](https://crates.io/crates/basis-universal) and -sys | 0.3.1 (2023-11-02) | MIT OR Apache-2.0; the C++ is Apache-2.0 | FFI. `cc` compiles the vendored **basisu 1.16** encoder and transcoder (C++11), so **a C++ compiler is required**. Built with `BASISD_SUPPORT_KTX2_ZSTD=0`. Its API has the `.basis` `Transcoder` and `LowLevelUastcTranscoder`; **there is no KTX2 API** | FFI | Repo last pushed 2024-04-28, 13 open issues. Upstream is now at v2.50 |
| [basisu](https://crates.io/crates/basisu) | 0.1.0 (2026-07-18), the only release | Apache-2.0 | **Pure Rust** port of the reference transcoder, "tracking upstream v2_1_0". Reads .ktx2 and .basis. Decodes ETC1S, UASTC LDR and HDR, ASTC and XUASTC to 21+ targets including RGBA32, "byte-identical" over the author's corpus. Deps: `portable-atomic`, plus optional `ruzstd` 0.7 | 8 lines (`pod_bytes`, a `OnceBox`, one table cast) | 3 stars; repo created the same day. 679 k lines, about 640 k of them generated tables (8 tables of about 77 k lines each). The RGBA-relevant modules are about 3.75 k lines. 24 k downloads; reverse deps are concinnity-cook, ez-gfx-assets, three-rs and viewport-lib-io |
| draco-texture (in [Filyus/draco-rust](https://github.com/Filyus/draco-rust/tree/main/crates/draco-texture), `publish = false`) | not on crates.io | Apache-2.0 | **Pure Rust** KTX2 + ETC1S + UASTC LDR, to RGBA8 and BCn/ETC/ASTC. Uses `ruzstd` 0.7 | `unsafe_code = "forbid"` | STATUS.md (2026-07-28): gated byte-for-byte against Binomial's build, with a libFuzzer target and malformed-input sweeps; HDR codecs left out on purpose. The RGBA-relevant files are 4,026 lines (2,962 code) at `18d16cdc29c9` |
| [ktx-rs](https://crates.io/crates/ktx-rs) | 4.4.3 (2026-10-30); first 2026-10-27 | Apache-2.0 AND BSD-3-Clause AND MIT AND Zlib | Pure-Rust port of KTX-Software, including Basis encoders and transcoders and GPU loaders (`ash`, `libloading`) | 139 lines | 2 stars, 27 downloads, 250 k lines |
| [basisu_c_sys](https://crates.io/crates/basisu_c_sys) / [bevy_basisu_loader](https://crates.io/crates/bevy_basisu_loader) | 0.9.1 / 0.6.1 | MIT OR Apache-2.0 | FFI to the Basis Universal C API through `cc`. On wasm it uses a vendored musl and libc++ | FFI | The Bevy community's KTX2 Basis loader |
| [basis_transcoder](https://crates.io/crates/basis_transcoder) | 0.1.3 (2026-09-03) | MIT OR Apache-2.0 | `cxx` plus vendored C++ (wip) | FFI | — |
| ktx2-rw / libktx-rs | 0.2.4 (2025-11) / 0.3.3 (2021) | MIT OR Apache-2.0 / Apache-2.0 | FFI to libktx. ktx2-rw downloads sources at build time with reqwest | FFI | — |

**What Bevy uses** (bevy_image 0.20.1, `src/ktx2.rs`): (High)

- The container comes from `ktx2` 0.5.
- Zstandard comes from `ruzstd` 0.9 (feature `zstd_rust`) or the C `zstd` crate (feature `zstd_c`).
- ZLIB comes from `flate2`.
- UASTC is transcoded with the C++ `basis-universal` `LowLevelUastcTranscoder`.
- **BasisLZ and ETC1S in KTX2 are not supported.** BasisLZ falls through to "Unsupported supercompression scheme", and the ETC1S branch says `// TODO: Implement transcoding`.
- bevy_gltf does not accept the `KHR_texture_basisu` JSON syntax (issue #19104, cited in its docs).

### 3.3 Upstream transcoder size

From [BinomialLLC/basis_universal](https://github.com/BinomialLLC/basis_universal) at `073eb6a0a34a` (2026-10-30). The latest release is v2_50 (2026-08-03). Apache-2.0.

- **`transcoder/basisu_transcoder.cpp`:** 46,065 lines (35,790 code lines), 1.69 MB. Its `.inc` tables, about 1.6 MB, serve ETC1S→DXT1/ATC/PVRTC2/BC7/ASTC and are not needed for RGBA.
- **ETC1S → RGBA path, about 1,100–1,500 code lines:**

  | Part | Code lines | Location |
  |---|---:|---|
  | Huffman decoding table | 130 | `basisu_transcoder_internal.h` |
  | Bitwise decoder | 229 | `basisu_transcoder_internal.h` |
  | Approximate move-to-front | 50 | `basisu_transcoder_internal.h` |
  | `decode_palettes` | 147 | L8257–8439 |
  | `decode_tables` | 57 | L8441–8509 |
  | Slice index decoding in `transcode_slice` | 273 | L8511–8860 |
  | RGBA32 / RGB32 / A32 output cases | 77 | L9086–9184 |
  | ETC1 colour helpers in `decoder_etc_block` | ≤507 | L496–1128 |

- **UASTC LDR 4×4 → RGBA path, about 1,000 code lines:**

  | Part | Code lines | Location |
  |---|---:|---|
  | `unpack_uastc`, all three overloads | 601 | L15364–16095 |
  | Mode, partition, BISE and weight tables | 267 | L14353–14670 |
  | `uastc_init` | 67 | L19727–19816 |
  | Texel partition | 55 | L14961–15022 |
  | `astc_interpolate` | 17 | `basisu_transcoder_uastc.h` |

- **KTX2 container and BasisLZ global data** (the `ktx2_transcoder` methods, L19872–21185, all formats): 1,023 code lines. A subset limited to ETC1S and UASTC would be smaller.
- **Zstandard is not in that file**; it calls the zstd library. Size references for a decoder:
  - facebook/zstd's [educational decoder](https://github.com/facebook/zstd/blob/dev/doc/educational_decoder/zstd_decompress.c): 2,323 lines, about 1,300 code lines.
  - ruzstd's decoding modules: about 8 k lines.
- **Rust data points:** the basisu crate's RGBA-relevant subset is about 3.75 k lines, and draco-texture is about 4.0 k lines. Both include the container and exclude zstd.
- **Estimate:** ETC1S + UASTC → RGBA8 with a KTX2 container would be **about 3–4.5 k lines of std-only Rust**. A hand-written zstd decoder (needed by every UASTC sample) would add **about 1.5–2.5 k**. (Medium)

### 3.4 Test assets

Headers and DFDs of every `.ktx2` file were parsed with Range requests.

| Variant folder | KTX2 files | ETC1S / BasisLZ | UASTC | Mips | Licence |
|---|---|---:|---:|---|---|
| `AnisotropyBarnLamp/glTF-KTX-BasisU` | 4 (4.6 MiB) | 0 | 4, all Zstd | **levelCount 1** (2048², no mips) | CC-BY-4.0 (Wayfair) |
| `ChronographWatch/glTF-KTX-BasisU` | 8 (1.4 MiB) | 7 | 1, Zstd | full | CC-BY-4.0 + DGG and Khronos trademarks |
| `FlightHelmet/glTF-KTX-BasisU` | 15 (29.3 MiB) | 5 | 10, Zstd | full | **CC0-1.0** |
| `StainedGlassLamp/glTF-KTX-BasisU` | 19 (8.6 MiB) | 6 (one RGB + AAA alpha slice) | 13, Zstd (one RGBA) | full | CC-BY-4.0 (Wayfair) |
| `CarConcept/glTF-KTX-BasisU-Draco` | 14 (1.3 MiB) | 6 | 8, Zstd | full | CC-BY-4.0 + Khronos trademarks |
| `ABeautifulGame/glTF-Binary-KTX-ETC1S-Draco` | 33 `image/ktx2` inside one GLB | ETC1S (by its name; not parsed) | — | — | CC-BY-4.0 |

- No sample uses UASTC without supercompression, and none uses ZLIB.
- Every KTX-BasisU variant lists `KHR_texture_basisu` in `extensionsRequired`, and **no texture has a fallback `source`**.
- **Small fixtures:**
  - CC0: `FlightHelmet_Materials_LensesMat_Normal.ktx2`, 1,685 B, UASTC + Zstd, 1024², 11 levels.
  - CC0: `FlightHelmet_Materials_LensesMat_BaseColor.ktx2`, 93,420 B, ETC1S, 1024².
  - CC-BY: CarConcept `Rib_N.ktx2`, 622 B, UASTC 32×32.
  - CC-BY: CarConcept `Dot_C.ktx2`, 1,374 B, ETC1S 128².
- **The colour tags in the samples break the extension's rules.**
  - ORM, occlusion or thickness textures tagged `SRGB`: CarConcept `Mechanical_ORM` (UASTC), `Occlusion` and `Thickness` (ETC1S); ChronographWatch `dgg_rm`, `khronos_rm` and `watchface_orm`; StainedGlassLamp `glass_transmission-clearcoat`.
  - Non-colour textures tagged primaries `BT709`: AnisotropyBarnLamp and CarConcept.
- (High)

### 3.5 What this means for a std-only decoder

- **The container is easy and the codecs are moderate.** `ktx2` is a small, pure-Rust, `no_std` parser, but it saves only about 500 lines.
- **The real work is ETC1S/BasisLZ** (Huffman tables, codebooks and slice decoding, all specified in KTX2 Appendix D) **and UASTC** (an ASTC-like block unpack).
- **Zstandard is the biggest unknown for a no-new-crate policy.** Every UASTC texture in the samples is Zstd-compressed.
  - `ruzstd` is the mature pure-Rust option. It is MIT, widely used, fuzzed, and has no deps with `default-features = false`. But it has raw-pointer `unsafe` in its ring buffer.
  - The repo's existing `miniz_oxide` only covers ZLIB, which Basis payloads may not use.
- **Total cost:** a full std-only port is about 5–7 k lines with zstd, or 3–4.5 k plus `ruzstd`. The draco-texture and basisu ports show this can be done without `unsafe`.
- **Decode `levels[0]` only.**
  - Bound the allocation by the header (`pixelWidth` × `pixelHeight` × 4) under an absolute pixel cap.
  - Check that UASTC's inflated length equals ceil(w/4) × ceil(h/4) × 16 × layers × faces.
  - Check that BasisLZ slice offsets lie inside level 0.
- **Do not derive colour space from the DFD;** the sample assets show its tags are unreliable. Write the transcoded bytes as stored, and let the glTF material slot decide.

---

## 4. Writing USDC crate files

### 4.1 Versions, from OpenUSD `crateFile.cpp`

This is verbatim from the [v26.11.1 `pxr/usd/sdf/crateFile.cpp`](https://github.com/PixarAnimationStudios/OpenUSD/blob/v26.11.1/pxr/usd/sdf/crateFile.cpp). Its `_SoftwareVersion` is 0.16.0. (High)

```
// 0.16.0: Added support for GfDuration values.
// 0.15.0: Added support for loopBoundaryTime-delimited spline extrapolation
//         loops and GfTimeCode-native splines.
// 0.14.0: Added support for ArrayEdits.
// 0.13.0: Support for splines with tangent algorithms None, Custom, AutoEase.
// 0.12.0: Added support for splines.
// 0.11.0: Added support for relocates in layer metadata.
// 0.10.0: Added support for the pathExpression value type.
//  0.9.0: Added support for the timecode and timecode[] value types.
//  0.8.0: Added support for SdfPayloadListOp values and SdfPayload values with
//         layer offsets.
//  0.7.0: Array sizes written as 64 bit ints.
//  0.6.0: Compressed (scalar) floating point arrays that are either all ints or
//         can be represented efficiently with a lookup table.
//  0.5.0: Compressed (u)int & (u)int64 arrays, arrays no longer store '1' rank.
//  0.4.0: Compressed structural sections.
//  0.3.0: (broken, unused)
//  0.2.0: Added support for prepend and append fields of SdfListOp.
//  0.1.0: Fixed structure layout issue encountered in Windows port.
//  0.0.1: Initial release.
```

**Reading rule** (`pxr/usd/sdf/fileVersion.h`): `CanRead` means "same major and `fileVer.minver <= minver`". The patch number is ignored. So a release reads files up to its own software minor version.

**Per release**, read from `crateFile.cpp` at each tag. The file lived in `pxr/usd/usd/` until 25.05 and in `pxr/usd/sdf/` from 25.08. (High)

| OpenUSD release | Software version (newest it reads and writes) | Default for new files (`DEFAULT_NEW_VERSION`) |
|---|---|---|
| 20.11 – 23.08 | 0.9.0 | 0.8.0 |
| 23.11 | 0.10.0 | 0.8.0 |
| 24.03 | 0.10.0 | 0.9.0 |
| 24.05, 24.08 | 0.11.0 | 0.9.0 |
| 24.11 | 0.12.0 | 0.9.0 |
| 25.02 | 0.12.0 | **0.10.0** |
| 25.05 | 0.12.0 | **0.8.0** (changelog: "Changed the default version for newly created binary .usd files from 0.10.0 to 0.8.0. In the future we intend to adopt this as the minimum supported version") |
| 25.08 | 0.13.0 | 0.8.0 |
| 25.11, 26.03, 26.05 | 0.14.0 | 0.8.0 |
| 26.08 | 0.15.0 | 0.8.0 |
| **26.11 / 26.11.1** (2026-10-21) | **0.16.0** | **0.8.0** |

**Default write version**

- `usdcat` and every OpenUSD 25.05+ build write **0.8.0** by default, set by the env var `USD_WRITE_NEW_USDC_FILES_AS_VERSION`.
- "This is only for new files; saving edits to an existing file preserves its version."
- The writer upgrades the version on demand. `RequestWriteVersionUpgrade` warns "Upgrading crate file … from version … to …", for example 0.14.0 for `VtArrayEdit` values.
- The 20.11 changelog: "New .usdc files now default to version 0.8.0. These files cannot be read in USD releases prior to v19.03."

**Oldest readable version** (High)

- 26.11 adds `PXR_USDC_OLDEST_SUPPORTED_VERSION 0.4.0`.
  - Code: "Cannot read asset … with obsolete version … The oldest version this software supports is 0.4.0".
  - Changelog: "Dropped support by default for `.usd` and `.usdc` binary files older than version 0.4.0. Obsolete versions are now rejected immediately at read time … Files from 0.4.0 up to but not including 0.8.0 remain deprecated but readable, with a warning."
  - Security advisory: GHSA-75r4-2rhh-hrxw.
- `PXR_USDC_EMIT_DEPRECATION_WARNINGS` became opt-in in 26.05 and is on by default since 26.08. 26.08 also added `usdupdatecrate`.
- The [AOUSD announcement](https://forum.aousd.org/t/openusd-26-11-disables-support-for-binary-usd-files-older-than-version-0-4-0/3057) (2026-10-08) adds:
  - 0.4.0 has been the default "in USD release 0.8.2, December 2017";
  - 0.4.0–0.7.x "will remain readable in 27.05".
- 26.05 and 26.08 defined `OLDEST_SUPPORTED_VERSION` as "0.0.1"; 25.11 and 26.03 had no such constant.

### 4.2 Can a writer avoid LZ4?

**Not for any version that current OpenUSD reads.** (High)

**Below 0.4.0 the sections are uncompressed, but 26.11 rejects those files.** The `_WriteTokens`, `_WriteFields`, `_WriteFieldSets`, `_WritePaths` and `_WriteSpecs` functions each branch on `writeVersion < 0.4.0` ("Old-style uncompressed …").

**At 0.4.0 and later, these sections must be compressed:**

| Section | What is written |
|---|---|
| TOKENS | `TfFastCompression`, i.e. LZ4 |
| FIELDS | Token indexes through `Sdf_IntegerCompression`, then value reps through `TfFastCompression` |
| FIELDSETS, PATHS (three arrays), SPECS (three arrays) | `Sdf_IntegerCompression` |
| STRINGS | Written raw (`w.Write(_strings)`) |

**What stays cheap at 0.8.0:**

- **LZ4 blocks can be literal-only.**
  - `TfFastCompression` writes byte 0 = 0 for a single chunk (otherwise the number of chunks, each prefixed by an `int32` size), then raw LZ4 blocks.
  - It decodes with `LZ4_decompress_safe`, which accepts any valid block.
  - A block with one literal-only sequence is valid LZ4, so a writer needs no match finder.
- **Arrays can stay uncompressed.** Array compression is optional for each value: ints from 0.5.0, floats from 0.6.0, and only for arrays of at least `MinCompressedArraySize = 16`.
  - The reader takes the uncompressed path when `!rep.IsCompressed()`; see `_ReadPossiblyCompressedArray`.
- **The integer encoder is small.** It writes deltas, 2-bit width codes and a common-value header, the inverse of what `usdc.rs` already decodes.
  - Data point: `cadmpeg-codec-usd`'s `src/usdc/write/encode.rs` is 240 lines. That includes a greedy LZ4 match finder, the chunk framing and the integer coding.

### 4.3 Apple tools and runtimes

- **Measured:** the `.usdc` inside every USDZ in Apple's [AR Quick Look gallery](https://developer.apple.com/augmented-reality/quick-look/). I read the zip local header and then the `PXR-USDC` bootstrap of each. (High for these files)
  - **0.8.0:** football, stratocaster, robot, slide, solar_panels, teapot, biplane, drummer.
  - **0.9.0:** hummingbird, seahorse.
  - **0.10.0:** chameleon, baseball glove, pancakes, toy_car.
  - All are STORE entries, with data at 64-byte-aligned offsets.

  So Apple ships 0.10.0 files for Quick Look today. Which tool wrote them is not recorded.
- **usdzconvert 0.66** (Apple's USDPython): two mirrors of Apple's README ([vche/usdpython](https://github.com/vche/usdpython), [KarpelesLab/usdpython](https://github.com/KarpelesLab/usdpython)) say the bundled library "was compiled using version 22.03". The heading says 19.11, so the README contradicts itself.
  - USD 22.03 writes 0.8.0 by default and reads up to 0.9.0.
  - The script itself has no crate-version option; its `-iOS12` switch concerns textures. (Low to Medium)
- **Reality Converter and Reality Composer Pro:** not established; I found no Apple statement and analysed no sample output.
- **RealityKit and AR Quick Look version limits:** none documented that I could find.
  - The gallery shows that current Quick Look reads 0.10.0.
  - Whether old iOS (12 or 13) reads 0.8.0 or later is unknown. 0.8.0 needs USD ≥ 19.03, and iOS 12 shipped in September 2018.

### 4.4 Rust crates that write USDC

| Crate | Latest (date) | Licence | Writes USDC? | Notes |
|---|---|---|---|---|
| [openusd](https://crates.io/crates/openusd) (mxpv) | 0.8.0 (2026-10-11) | MIT | **Yes.** `CrateWriter` always emits **0.12.0** (`const WRITER_VERSION: Version = version(0, 12, 0)`). LZ4 comes from `lz4_flex` 0.14, whose default features are safe; integer arrays of 16 or more are compressed | Pure Rust, 119 k lines. Reads and writes usda, usdc and usdz, with composition. 14 deps (lz4_flex, zip, logos, regex-lite, derive_more, strum, half, bytemuck, …). MSRV 1.96, 152 stars, active. **Its output needs OpenUSD ≥ 24.11 to read** |
| [oxideav-usdz](https://crates.io/crates/oxideav-usdz) | 0.0.5 (2026-10-04) | MIT | **Yes.** A typed writer at **0.8.0** (`usdc_writer.rs`), with LZ4 through `compcol` | `#![forbid(unsafe_code)]`. Part of the OxideAV project |
| [cadmpeg-codec-usd](https://crates.io/crates/cadmpeg-codec-usd) | 0.6.8 (2026-11-04, first release) | Apache-2.0 | **Yes.** `src/usdc/write.rs` writes **0.8.0** with its own LZ4 and integer encoder (240 lines) | Brand new; a 272-star monorepo created 2026-07 |
| [openusd-rs](https://crates.io/crates/openusd-rs) | 0.1.0 (2025-07-24) | MIT OR Apache-2.0 | No (`write` is `unimplemented!()`) | Reader only |
| [usd](https://crates.io/crates/usd), [pxr](https://crates.io/crates/pxr), [rust-usd](https://crates.io/crates/rust-usd) | 0.0.9 (2020) / 0.2.0 (2024) / 0.0.5 (2026) | Apache-2.0 / MIT OR Apache-2.0 / Apache-2.0 | Through C++ OpenUSD | Each needs a full OpenUSD C++ build |
| [usdz](https://crates.io/crates/usdz) | 0.0.1 (2023) | MIT OR Apache-2.0 | No | A `nom` parser |

**Background:** the AOUSD Core Specification 1.0, ratified in December 2025, includes "Detailed specifications for USDA (human-readable), USDC (binary), and USDZ" ([AOUSD](https://aousd.org/uncategorized/foundations-of-open-3d-development-introducing-aousd-core-specification-1-0/)). oxideav and cadmpeg both cite its sections. I did not read the spec text itself.

### 4.5 What this means for a std-only writer

- **Target 0.8.0**, the writer side of what `usdc.rs` already reads.
  - It is what `usdcat` writes by default.
  - OpenUSD 26.11 reads it without a deprecation warning.
  - USD ≥ 19.03 can read it.
  - Pixar has said it intends to adopt 0.8.0 as the minimum supported version.
  - Most of Apple's own Quick Look assets use it.
- **Nothing in a mesh, material, skeleton or plain animation export needs a later version.** Later versions add timecode values (0.9), path expressions (0.10), layer relocates (0.11) and splines (0.12+).
- **Compression can't be skipped, but it is cheap.**
  - Emit literal-only LZ4 blocks with the one-byte `TfFastCompression` header.
  - Emit integer-coded structural sections.
  - Write arrays uncompressed, with the compressed bit clear.
  - This mirrors code the repo already has. A real match finder is optional, about 100 more lines.
- **Of the existing crates, only oxideav-usdz and cadmpeg write 0.8.0.** Both are new. openusd writes 0.12.0, which shuts out OpenUSD < 24.11 and possibly older Apple runtimes.

---

## What I could not establish

- **Apple's writers:** which crate version Reality Converter and Reality Composer Pro write. usdzconvert is only inferred, from a mirrored and self-contradicting README.
- **Apple's readers:** whether RealityKit or AR Quick Look has a documented crate-version limit, and from which iOS version each crate version is readable.
- **The pure-Rust ports' claims:** byte-exact parity, fuzzing coverage, and absence of panics for draco-core, draco-oxide, meshopt-pure, meshoptimizer-rs, basisu and draco-texture. I read their docs and code layout; I did not build, run or fuzz them.
- **Inside the Draco sample streams:** which prediction schemes they use. Only the stream headers were parsed.
- **ABeautifulGame's KTX2 images:** their headers were not parsed. The 11.5 MiB GLB was not downloaded, and ETC1S is inferred from the folder name.
- **Point-cloud and KD-tree coverage** of the Draco bitstream spec page: the fetch was truncated.
- **The decode-only line counts** for meshoptimizer and basisu are range-based. Shared helpers are counted in full.
