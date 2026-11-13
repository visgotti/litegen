# `reference/`: Google's own decoding of a Draco corpus of our meshes

What `draco_core_matches_googles_decoder` (`litegen-core/src/mesh/draco.rs`)
reads: Draco streams of our meshes written by google/draco's own encoder
library, and the SHA-256 of what google/draco's own decoder makes of each,
value for value and face for face. The test decodes every stream through
`draco.rs` and requires every count and hash to be Google's (mesh-gaps item 2,
Task 4b; Rulings B33–B36, B42 and B44). Hash equality is byte equality, so a
difference is a decoder disagreement to report, never a tolerance to widen.

## google/draco, as built

- **Commit** `d429de84b52a860705507da0884764c5d8321423`: tag 1.5.9 (`554628821c34`,
  "Draco 1.5.9 release.") and one commit, "Pin three.js example to r125
  (#1252)", which changes `javascript/example/webgl_loader_draco.html` alone;
  `git diff 1.5.9 d429de84b52a -- src cmake CMakeLists.txt` is empty. Cloned
  with `git clone --revision=<commit> --depth 1` (git ≥ 2.49), the commit
  checked with `git rev-parse HEAD` and the tree clean. The library is
  Apache-2.0.
- **Build:** CMake 4.4.3, Ninja 1.13.2, Apple clang 21.0.0 (clang-2100.1.1.101),
  macOS arm64. `cmake -S draco-src -B draco-build -G Ninja
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_EXPORT_COMPILE_COMMANDS=ON
  -DCMAKE_C_FLAGS=-ffp-contract=off -DCMAKE_CXX_FLAGS=-ffp-contract=off`, then
  `cmake --build draco-build --target draco_static draco_encoder draco_decoder
  -j 4` (142 steps, 20 of draco's own warnings, implicit int-to-float conversions and a missing `override`, exit 0). Every one of the 137
  compile commands is then `c++ -DDRACO_CMAKE=1 … -ffp-contract=off -O3
  -DNDEBUG -arch arm64 -Wno-deprecated-declarations`. That gives `libdraco.a`,
  `draco_encoder` and `draco_decoder`, built outside the repo (item 2's ledger,
  `tools/`).
- **`-ffp-contract=off`, and why (Ruling B45):** strict IEEE evaluation of
  draco's own source, which is what its x86-64 baseline builds (no FMA
  instruction without `-mfma`) and its WebAssembly builds (wasm has no fused
  multiply-add) compute, and what Rust computes: Rust never contracts. Clang's
  default, `-ffp-contract=on`, fuses `x * x + y * y + z * z` and `in_s *
  dequantization_scale_ - 1.f` in the octahedral normal transform
  (`normal_compression_utils.h:297-361`) into fused multiply-adds on arm64, and
  under that default 414 of the 445 streams' NORMAL values differ from the
  strict ones: 124,971 of 264,495 float components, by at most 2.7e-7 (and by
  up to 177 ulps where both values are at least 1e-3 in magnitude; 24,463 of the
  differences are on values under 1e-3). No other attribute, no connectivity
  and no count differs between the two builds, and the encoder writes
  byte-identical streams under both (all 506 pairs re-encoded and hashed).
  Google's own shipped WebAssembly decoder (`draco3dgltf` 1.5.9's
  `draco_decoder_gltf.wasm`, sha256 `39b78cec…34bf`, run with Node) decodes
  Box, WaterBottle, cube and Avocado to the strict normals byte for byte, and
  not to the contracted ones. The contracted build's dumps are kept in item
  2's ledger (`task4b/dumps-contract-on`), not here.
- **Our two tools,** compiled against that library with the flags CMake gives
  draco's own tools, its headers as system headers so that `-Wall -Wextra`
  speak of our code alone (no warning):
  `c++ -std=c++17 -O3 -DNDEBUG -DDRACO_CMAKE=1 -D_FILE_OFFSET_BITS=64
  -D_LARGEFILE_SOURCE -ffp-contract=off -Wall -Wextra -isystem draco-src/src
  -isystem draco-build <tool>.cc draco-build/libdraco.a -o <tool>`. A second
  compile from the same sources gave byte-identical binaries.
- Python 3.14.7, standard library only (`python3 -I`).

## The files

| file | what |
|---|---|
| `make_reference.sh` | The one-off that makes everything below, step by step (`WORK=<scratch> sh make_reference.sh [tools|fixtures|geometry|streams|dumps|manifest]`), with every command. |
| `reference.py` | The glTF reading (`geometry`), the dedup (`store`), the manifest and listing (`manifest`), the `equivalence` and `crosscheck` checks. |
| `encode_reference.cc` | One stream of one input at one setting, through `draco::Encoder` and an `ExpertEncoder`, with every attribute: `draco_encoder` reads OBJ and PLY, which carry no tangents or generic attributes (Ruling B35). |
| `dump_reference.cc` | Google's decoding of one stream, raw: `connectivity.bin` and one `attribute-<unique id>.bin` a point map, as `draco::Decoder::DecodeMeshFromBuffer` leaves them; `draco_decoder`'s text outputs would round them. |
| `inputs.tsv` | The inputs: each triangle primitive read, its source file and sha256, points, faces, attributes (glTF semantic : Draco type : data type), and which primitives repeat another. The source sha256 covers the `.gltf` JSON only, not the `.bin` a sample's geometry sits in. |
| `streams/` | 445 streams, `<input>.<setting>.drc`, 819,208 bytes in all. |
| `manifest.txt` | Google's decoding of each stream, hashed (its header says the row formats). |
| `streams.inc` | The streams as `(name, include_bytes!)` pairs, which `draco.rs` `include!`s. |

## The inputs: 22 geometries from 31 triangle primitives

`reference.py geometry` reads every `mode 4` primitive of each file, tightly
packed, with every attribute, and leaves out a primitive whose points, faces
and attribute bytes equal one already taken (`inputs.tsv` says which). Morph
targets are not encoded (`KHR_draco_mesh_compression` leaves them in plain
accessors). The inputs:

- **`fixture-*`:** this module's nine fixtures as its own GLB writer writes them
  (the ignored test `mesh::glb::tests::write_the_gltfpack_inputs`, the
  `fixtures` step): cube (8 points, 12 faces), layered (car_paint with TANGENT,
  and glass; velvet, plastic, soap_film, brushed_steel and lacquer repeat
  glass), metal (blade 24/8 and plate 25/32; ingot repeats layered-glass),
  morph (121/200, a grid), multiuv (4/2, TEXCOORD_0..3), multiuv_shared (left,
  right), skinned (425/768, JOINTS_0 uint16 and WEIGHTS_0 float32), staged
  (ground 4/2; cube repeats layered-glass), textured (panel 25/32 with COLOR_0
  float32 × 4; box repeats layered-glass, wedge repeats metal-blade). All
  float32 apart from the joints.
- **`sample-*`:** the five Khronos samples' `glTF` variants (`../<model>/glTF`,
  their licences in `../<model>/LICENSE.md`): Box (24/12), MorphPrimitivesTest's
  two primitives (21/24 and 9/8), RiggedSimple (160/188, skinned), Avocado
  (406/682, TANGENT) and WaterBottle (2,549/4,510, TANGENT). The streams made
  from them are derived works of those samples: CC-BY-4.0 (Box, Morph-Primitives
  Test, Rigged Simple) and CC0-1.0 (Avocado, Water Bottle).
- **`quantized-textured-*`:** `../ours/textured.q.glb`'s three primitives, whose
  positions and normals are int16 normalized, UVs uint16 normalized and colours
  uint8 normalized (`KHR_mesh_quantization`).
- **`gltfpack-skinned`:** `../../gltfpack/skinned.glb`, gltfpack's quantization:
  POSITION uint16, NORMAL int8 normalized, TEXCOORD_0 uint16 normalized,
  JOINTS_0 uint8, WEIGHTS_0 uint8 normalized.

So every attribute kind the extension carries is here: POSITION, NORMAL,
TEX_COORD (up to four), COLOR, and GENERIC for TANGENT, JOINTS_n and
WEIGHTS_n; data types int8, uint8, int16, uint16 and float32, normalized and
not.

## The settings: 23 an input

`encode_reference` calls `draco::Encoder` as `draco_encoder` does
(`draco_encoder.cc:337-359` at that commit): `SetAttributeQuantization` for
POSITION (`-qp`), TEX_COORD 10 (`-qt`), NORMAL 8 (`-qn`) and GENERIC 8
(`-qg`), `SetSpeedOptions(10 - cl, 10 - cl)`. The mesh is built one value a
point, then `DeduplicateAttributeValues` and `DeduplicatePointIds`, as draco's
own OBJ and PLY loaders and its JavaScript encoder do before encoding.

| setting | what |
|---|---|
| `cl<C>-qp<Q>`, C in 0, 1, 5, 7, 10 and Q in 8, 11, 14, 16 | `draco_encoder -cl C -qp Q`, its other defaults kept. `-cl 0` is sequential; the rest EdgeBreaker, whose coder is standard when the speed (10 − C) is 5 or more or the mesh has under 1,000 faces, and valence otherwise (`mesh_edgebreaker_encoder.cc:34-46`): valence for WaterBottle at `-cl 7` and `-cl 10`. |
| `seqc-qp11` | `-cl 0 -qp 11` with the global option `compress_connectivity`, which `draco_encoder` never sets: entropy-coded index deltas. |
| `swap-qp11` | `-cl 10 -qp 11` with `edgebreaker_method` set to the coder the rule above doesn't pick at that level: valence under 1,000 faces, standard from 1,000 on. |
| `meta-qp11` | `-cl 7 -qp 11` with geometry metadata (a string, an int, an int array, a double and a sub-metadata) and a `name` entry on every attribute. |

Of the 445 stored streams, 76 are sequential with raw connectivity, 22
compressed sequential, 317 standard EdgeBreaker and 30 valence EdgeBreaker
(the size rule above picks valence for WaterBottle at `-cl 7` and `-cl 10`,
whose speed is under 5, and `swap` picks it for every other input); 22 carry
metadata.

## Once a byte-distinct stream (Ruling B42)

22 inputs × 23 settings is 506 (input, setting) pairs. `-qp` is a no-op on an
input whose positions are integers (the four quantized inputs), so their
streams repeat across the position bits, and `quantized-textured-wedge`'s
`cl7-qp8` equals its `cl5-qp8` (24 points, 8 faces, no UVs). `reference.py
store` keeps each byte-distinct stream once, under the first pair that wrote
it, and the manifest records every other pair as a `same` row: 445 streams, 61
`same` rows. The test decodes each stream once and checks it against its row;
a `same` row's expectations are its target's, byte for byte.

## Two streams Google's decoder refuses (Ruling B44)

`draco_decoder` and `dump_reference` refuse `fixture-morph.seqc-qp11.drc`
(121 points, 200 faces, 506 bytes) and `fixture-skinned.seqc-qp11.drc` (425
points, 768 faces, 2,052 bytes) with "Failed to decode geometry data." The
cause is `mesh_sequential_decoder.cc:58` at that commit:

```cpp
  if (faces_64 > buffer()->remaining_size() / 3) {
    // The number of faces is unreasonably high, because face indices do not
    // fit in the remaining size of the buffer.
    return false;
  }
```

That assumes a byte a face index, true of raw connectivity and not of
`compress_connectivity`'s entropy-coded deltas: these two streams have 491 and
2,036 bytes after their counts, under three a face. A probe that calls
`DecodeSymbols` on the same bytes decodes all 600 and 2,304 indices, in range.
draco-core 2.3.3's `src/mesh_decoder.rs:393-399` declines to port the check
for the same reason ("Adopting it would import an upstream bug rather than
parity"). The finding is drafted for google/draco in item 2's ledger.

Their manifest rows end `google-refused expected-from <input>.cl0-qp11.drc`,
and carry Google's decoding of that stream: the same input at the same
quantization, sequential, so the same attribute coding and point order, with
raw connectivity. That substitute is evidence, not an assumption:
`reference.py equivalence` compares, for every input, Google's decoding of
`seqc-qp11` with its decoding of `cl0-qp11`, every `attribute-*.bin` and
`connectivity.bin` byte for byte, and the counts:

```text
$ python3 -I reference.py equivalence $WORK/dumps $WORK/pairs.tsv
20 of 22 inputs: Google's decoding of seqc-qp11 equals its decoding of cl0-qp11, byte for byte; refused seqc-qp11: fixture-morph, fixture-skinned
```

The test requires draco-core to decode the two refused streams to those hashes.

## Checks on the reference itself

- **`draco_decoder` against `dump_reference`** (`reference.py crosscheck`):
  `draco_decoder -i <stream> -o <stream>.ply` writes a binary PLY of the first
  POSITION, NORMAL, COLOR and TEX_COORD attributes, and names `float`, `uchar`
  and `int` alone (`ply_encoder.cc`). For every stream whose first such
  attributes have those types, its vertex rows, face indices and face texture coordinates
  equal `dump_reference`'s bytes: 412 of 445 streams; 31 have int16 or uint16
  positions, which the PLY writer can't name, and 2 are the refused streams.
- **The header against the decoding:** `reference.py manifest` reads each
  stream's header itself (the method, the metadata flag, the connectivity or
  EdgeBreaker coder byte) and refuses a manifest whose rows disagree with the
  decoder's.
- **Every attribute is packed,** one value a point through the point map, which
  the manifest step checks against each dump's size.

## Regenerating

```sh
WORK=/path/to/scratch sh make_reference.sh
```

runs `tools` (clone, verify, build, compile the two tools), `fixtures` (cargo,
in this checkout, after waiting out other builds), `geometry`, `streams`,
`dumps` and `manifest`. Each step can be run alone. The raw dumps, the PLYs,
`pairs.tsv` and the geometry files stay in `$WORK`; only `inputs.tsv`,
`streams/`, `manifest.txt` and `streams.inc` are written here. A second run of
`streams` gave byte-identical streams.
