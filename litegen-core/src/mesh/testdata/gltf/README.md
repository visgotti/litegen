# glTF test vectors

Files read by the tests in `glb.rs` and `draco.rs` (`include_bytes!`): glTF
whose bufferViews are meshopt-compressed (`EXT_meshopt_compression`,
`KHR_meshopt_compression`), which the reader decodes through `meshopt.rs`, and
glTF whose primitives are Draco-compressed (`KHR_draco_mesh_compression`),
which it decodes through `draco.rs` (mesh-gaps item 2).

## `MeshoptCubeTest/`: Khronos's meshopt sample

From [glTF-Sample-Assets](https://github.com/KhronosGroup/glTF-Sample-Assets)
at `8f492390ddc7a2ae221a1369ef560589cf8c70fb`, `Models/MeshoptCubeTest/`.
Licence: CC0-1.0 (the model, by Arseny Kapoulkine, 2025); `LICENSE.md` is the
sample's own, copied unchanged.

It is a grid of 25 cubes and 10 label quads whose 60 meshopt views use every
mode (ATTRIBUTES, TRIANGLES, INDICES), both attribute formats (v0 `0xa0`, v1
`0xa1`) and every filter (OCTAHEDRAL, QUATERNION, EXPONENTIAL, COLOR).

| file | what it is |
|---|---|
| `glTF-Meshopt/MeshoptCubeTest.gltf` | requires `KHR_meshopt_compression`; its fallback buffer (buffer 1) has no uri |
| `glTF-Meshopt/MeshoptCubeTest.bin` | the stream buffer (buffer 0): the plain views and the compressed streams interleaved, row by row (the plain views 37–40, 56–59, 75–78 and 94–97 sit between the streams, at offsets 4360, 6200, 8136 and 9832) |
| `glTF-Meshopt/*.png` | the ten label images |
| `glTF/MeshoptCubeTest.gltf` | the same document, `KHR_meshopt_compression` only used, and buffer 1 the file below |
| `glTF/MeshoptCubeTestFallback.bin` | the fallback: every meshopt view's plain data, at the view's own byteOffset |
| `reference-decode.bin` | every meshopt view of `glTF-Meshopt/MeshoptCubeTest.gltf` as meshoptimizer's JS reference decoder reads it (below) |

The `glTF` variant's `MeshoptCubeTest.bin` and images are byte-identical to
`glTF-Meshopt`'s, so they are kept once, in `glTF-Meshopt/`; the tests supply
them to both variants. The sample's SHA-256 sums:

```text
6578c1d82c5cc2b228e9513e37f348ca89cdb24b5985aa0567efef8d3c014360  glTF-Meshopt/MeshoptCubeTest.bin
b5947609f3d8aba58de3d43101df3b635ffaaab5849431f8518af6a98a040433  glTF-Meshopt/MeshoptCubeTest.gltf
8721150e3409425acf83aa21986e55880360ab084c17e96409c25fac53477f72  glTF/MeshoptCubeTest.gltf
8d3d779653780e85a75eda988110ab235ea85cd3d174361ffb318c6b657dee07  glTF/MeshoptCubeTestFallback.bin
```

`reference-decode.bin` is made by `scripts/mesh-verify/meshopt-vectors.mjs`,
from the repo root, with meshoptimizer's `js/meshopt_decoder_reference.js` at
`f0a684f11ead064e176775c9b796f82a55910e92` (MIT; sha256 `207f6459…8dc`, which
the script checks):

```sh
node scripts/mesh-verify/meshopt-vectors.mjs <meshopt_decoder_reference.js> \
  --views litegen-core/src/mesh/testdata/gltf/MeshoptCubeTest/glTF-Meshopt/MeshoptCubeTest.gltf \
  litegen-core/src/mesh/testdata/gltf/MeshoptCubeTest/reference-decode.bin
```

It holds a record a view, in view order: the view's index and its decoded
length as u32 little-endian, then the decoded bytes (60 views, 10,464 bytes,
sha256 `50276e8d66bb0ad8c6a2810f3a4ba94b02b5697fda3b2868394ccdd93bf964e4`).
These are a CC0 sample's own views, decoded. None of these streams reaches the
reference decoder's known control-mode-2 bug, which leaves a lane's spilled
deltas uncleared where meshoptimizer's C++ clears them.

## `gltfpack/`: our fixtures through gltfpack

Every `fixtures::all()` scene (`cube`, `layered`, `metal`, `morph`, `multiuv`,
`multiuv_shared`, `skinned`, `staged`, `textured`) as this module's GLB writer
writes it, then optimised by gltfpack 1.3.0 (npm `gltfpack@1.3.0`, run with
Node; `library.wasm` sha256 `ddd1658ec64b95916dc2b8d5ecff906501c178b5aa2df2fbf22eb8ae5422a417`,
`cli.js` sha256 `c7f3a81fa93a3609847fb75817e4777c339bd5f94963bdbb9aa224a6fdb038ce`).
Licence: the content is litegen's; gltfpack (MIT) attaches no licence to its
output.

The inputs come from the ignored test `write_the_gltfpack_inputs` in `glb.rs`.
Each input `<fixture>.glb` then gives four files:

```sh
GLTFPACK_INPUTS_OUT=<dir> cargo test --lib -- --ignored mesh::glb::tests::write_the_gltfpack_inputs
node node_modules/gltfpack/cli.js -i <dir>/<fixture>.glb -o <fixture>.glb          # uncompressed, quantized
node node_modules/gltfpack/cli.js -i <dir>/<fixture>.glb -o <fixture>.c.glb -c     # EXT_meshopt_compression, v0, no filters
node node_modules/gltfpack/cli.js -i <dir>/<fixture>.glb -o <fixture>.cc.glb -cc   # EXT, with OCTAHEDRAL, QUATERNION, EXPONENTIAL
node node_modules/gltfpack/cli.js -i <dir>/<fixture>.glb -o <fixture>.cz.glb -cz   # KHR_meshopt_compression, v1, and COLOR too
```

gltfpack is deterministic: a second run gives byte-identical files. The tests
compare each compressed file with the uncompressed one, never with our
fixture, so what gltfpack changes in the scene is the same on both sides. It
does change it: every file quantizes (`KHR_mesh_quantization`, with
`KHR_texture_transform` where uvs are), node names go, unused `TEXCOORD_0`
goes from `cube`, `morph` and `staged`, meshes merge (`metal` 3 to 1,
`multiuv_shared` 2 to 1, `staged` 2 to 1, `textured` 3 to 2), quantization
adds nodes (`layered` 8 to 14), and animations are resampled to 61 keys at
30 Hz (`skinned`'s CUBICSPLINE becomes LINEAR).

What compression changes, against the uncompressed file: under `-c`, nothing
but the corner each triangle starts at (meshoptimizer's index encoder turns
triangles, winding kept), so gltfpack quantizes alike with and without `-c`.
Under `-cc` and `-cz`, gltfpack also orders vertices and triangles its own
way, and the filters round normals, tangents, rotations, translations, scales
and (under `-cz`) colours to their own precision.

## `draco/`: Khronos's Draco samples, and Draco files of our own

### The samples

From [glTF-Sample-Assets](https://github.com/KhronosGroup/glTF-Sample-Assets)
at `8f492390ddc7a2ae221a1369ef560589cf8c70fb`, `Models/<model>/`: each
model's `glTF-Draco` variant, which requires `KHR_draco_mesh_compression` and
has no fallback, and its `glTF` variant, uncompressed, each a `.gltf` and the
one `.bin` it names. Each `LICENSE.md` is the sample's own, copied unchanged.
The images the samples name were not copied: the tests read both variants
without them, the textures dropped with a warning on each side alike.

| model | licence | its Draco streams |
|---|---|---|
| `Box/` | CC-BY-4.0: Box, by Cesium (2017) | one, standard EdgeBreaker: 24 points, 12 faces |
| `MorphPrimitivesTest/` | CC-BY-4.0: Morph-Primitives Test, by ft-lab (2018); its Draco compression by Frank Galligan (2020) | two, sequential (the only samples' streams that keep the vertex order), each primitive with one morph target in plain accessors |
| `RiggedSimple/` | CC-BY-4.0: Rigged Simple, by Cesium (2017) | one, standard EdgeBreaker, skinned (JOINTS_0 and WEIGHTS_0 as generic attributes) |
| `Avocado/` | CC0-1.0: Avocado, by Microsoft (2017) | one, standard EdgeBreaker, with TANGENT |
| `WaterBottle/` | CC0-1.0: Water Bottle, by Microsoft (2017) | one, valence EdgeBreaker (which 10 of the 16 Khronos Draco samples use), with TANGENT |

Every stream is bitstream 2.2 with POSITION at 11 bits, TEXCOORD_0 (where
there is one) at 10, and NORMAL octahedral at 8. Tangents, joints and weights
are stored unquantized. `glb.rs`'s `DRACO_SAMPLES` holds each stream's
quantization as Google's own decoder reads it (three r183's bundled
`examples/jsm/libs/draco/draco_decoder.js`, Ruling B31), with the script and
command that read it. The BrainStem, VirtualCity and Duck samples aren't CC0 or
CC-BY, so they are not here.

```text
2759bbffe3b8aebe789958875a098045fe4039eb3e2291ef0fd09c65994bdddc  Box/LICENSE.md
610dc6e08aba7c2720c8e4ec0578efd91cf2d88a5e638dab7811a22f0235bf2e  Box/glTF-Draco/Box.bin
3c46acecdfa90b012ec9052d8a1dfa61358e6d56a9e333504189cc78a2de4d1b  Box/glTF-Draco/Box.gltf
4a0d69eecfce0672a50b71dc218cbacec6c53fe2445040c235c6314b1b2c41b9  Box/glTF/Box.gltf
3266a8e39b9f425b3341cbe5eec7849f44310256bfa651e6b8b40c85ce0ccafb  Box/glTF/Box0.bin
1833fb1fc75ec566bab1a3c447fcbb33b3237953003f933692b99aea945633bc  MorphPrimitivesTest/LICENSE.md
4c600738509168bd0dfad3b67bf8ff9b9ae0bf331b7a1531958029ff9d76e14d  MorphPrimitivesTest/glTF-Draco/MorphPrimitivesTest.bin
6133d2a743b536dca87be279deeef1e1c50761ee9a932814561696221db576d4  MorphPrimitivesTest/glTF-Draco/MorphPrimitivesTest.gltf
59ec1d7ca5c8d7605b69aa57cee3da2994384831a290e5271ecd963002c31c76  MorphPrimitivesTest/glTF/MorphPrimitivesTest.bin
a51388b330a49586388e052dc66f74e95cdbc1efcdddea4248f65e200e1d335a  MorphPrimitivesTest/glTF/MorphPrimitivesTest.gltf
4343fbd97e5ecfc4c872a85cb1d755a3006f8848e15135fd6c4534839909bdbb  RiggedSimple/LICENSE.md
ef345d43419377a4a1c255b1a779723e3d2d3b4321ee33b1f9833840306d5449  RiggedSimple/glTF-Draco/RiggedSimple.gltf
c0196c3e1d642ea4d78898c0b4e9b1644828a2083145264fb5f9b18b0fb29229  RiggedSimple/glTF-Draco/RiggedSimple0.bin
18cd3e4d50ecadc93f6b6a720a4ee0d5154443c2fa331bc0dd3bef7499c02eb0  RiggedSimple/glTF/RiggedSimple.gltf
b18e39089acca03219a7a3aa4dd3f00eaedebf438bc3bd58fdcda855f64eafe6  RiggedSimple/glTF/RiggedSimple0.bin
56357fc8277990d763480c4207551cd1f4d58451c6ea5f16d9fdb7d2fa7eb00c  Avocado/LICENSE.md
535123a66a19c0427298d0bbe81b0ba5efa6d8155b7c659e11c23d8c6d6fe2c6  Avocado/glTF-Draco/Avocado.bin
63ec45999c1df77adc3837857c95d16dde7aee062b4a1d4475ca968514ec8609  Avocado/glTF-Draco/Avocado.gltf
3bf7ce0ec994562eabf0cb72a52bbc7976818d630d62609a966b53a5ff9b9180  Avocado/glTF/Avocado.bin
c9bde1d2f09514dad0a16971df2222beb582fad5dfc86203de1d56389d52feda  Avocado/glTF/Avocado.gltf
33cddcace3d46b3dec14275eff753c5879d5c55462a1bdce7981db4a5adb9a6b  WaterBottle/LICENSE.md
eb9fda9f5bf32e20f53b107646cb57be32bb2fbefd2f18abf39ae134743b3a53  WaterBottle/glTF-Draco/WaterBottle.bin
ab6ad670f58d7af300219aa4348ad6f8ba8522cedb5a165d55bde1e61557339f  WaterBottle/glTF-Draco/WaterBottle.gltf
e4921f2d0c0a03cf65286bd195f3688d4f99d7b6a58f25935e70d516d744156e  WaterBottle/glTF/WaterBottle.bin
0596f4e61dc781439d254fdfb5e3462daf1762c18715e3e3ac13001aa8f3f547  WaterBottle/glTF/WaterBottle.gltf
```

### `crafted/constant-4000001.drc`

A sequential Draco 2.2 mesh of 4,000,001 points, every one at (0.25, 0.5,
0.75), its position quantized at 11 bits, with one face (0, 1, 2): 72 bytes,
sha256 `d1e8cdf91c29d9d2d5a4e676c24c9e2e9ae7e2c56be0bc19ed0f793c5c517f8d`.
Its position attribute is constant, so it entropy-codes to a size independent
of its count. The tests write other counts of 2^21 or more over its header's
(the varint at bytes 12..16; from 2^21 points on, a sequential mesh's indices
are 32-bit, so the rest of the stream reads as before), for the limits' tests.
Our content, written once by `draco-core` 2.3.3's own encoder (Apache-2.0),
the same crate's `encoder` feature, from a probe crate in item 2's ledger
(`.superpowers/sdd/2026-11-04-mesh-compressed-gltf-input/task4/probe`,
`src/bin/encode_constant.rs`), never from litegen's build. Its core, as run:

```rust
let mut mesh = Mesh::new();
mesh.set_num_points(points);
let mut position = PointAttribute::new();
position.init(GeometryAttributeType::Position, 3, DataType::Float32, false, points);
for i in 0..points {
    position.buffer_mut().write(i * 12, &value); // 0.25, 0.5, 0.75 as f32 LE
}
mesh.add_attribute(position);
mesh.set_num_faces(1);
mesh.set_face(FaceIndex(0), [PointIndex(0), PointIndex(1), PointIndex(2)]);
let mut options = EncoderOptions::new();
options.set_encoding_method(0); // sequential
options.set_attribute_quantization(0, 11);
encoder.set_mesh(mesh);
encoder.encode(&options, &mut out)?;
```

`encode_constant 4000001 constant-4000001.drc` gave byte-identical files on
two runs.

### `crafted/narrow-u8-1000.drc`

A sequential Draco 2.2 mesh of 1,000 points whose one attribute is GENERIC,
one unsigned byte a point (every point 7), with one face (0, 1, 2): 47 bytes,
sha256 `72465068b684cde6e14a772911ed70a5b820bba21d5faf5929c00783fc4b3e04`. Its
header's counts are the varints at bytes 11 (num_faces) and 12..14
(num_points). `draco.rs`'s tests hold it to a point limit of 999 (mesh-gaps
item 2, Task 4 fix round 1): an attribute one byte a point is charged one byte
by draco-core's bytes limit, so it is the shape that limit lets past the point
limit. Our content, written once by the same probe crate's
`src/bin/encode_narrow.rs` (`encode_narrow 1000 1 narrow-u8-1000.drc`, with
`draco-core` 2.3.3's `encoder` feature: `init(GeometryAttributeType::Generic, 1,
DataType::Uint8, false, points)`, sequential, no quantization), byte-identical
on two runs.

### `crafted/constant-2000000.drc`

`constant-4000001.drc`'s mesh at 2,000,000 points: every point at (0.25, 0.5,
0.75), its position quantized at 11 bits, with one face (0, 1, 2): 62 bytes,
sha256 `e10c2010032e3776445e1e4f009311f6263895df9cc4fd2db823f26792087720`. Its
positions decode to 24,000,000 bytes, exactly `draco.rs`'s decoded-bytes
ceiling, `MAX_DECODED_BYTES` (mesh-gaps item 2, Task 4 fix round 1, Ruling
B38), so it is the largest constant stream the reader admits. Under 2^21
points a sequential mesh's indices are varints, so this count can't be written
over `constant-4000001.drc`'s header; it is a stream of its own, from the same
probe's `encode_constant 2000000 constant-2000000.drc`, byte-identical on two
runs.

### `crafted/narrow-u8x48-500001.drc`

A sequential Draco 2.2 mesh of 500,001 points whose one attribute is GENERIC,
48 unsigned bytes a point (every byte 7), with one face (0, 1, 2): 45 bytes,
sha256 `365bebc83aa1d6bb2669a36cb67d701d7a9b3f3d7a9988ebef32453d2b1c70c9`. It
declares 24,000,048 bytes of attributes: past the decoded-bytes ceiling by one
point's worth, and within the point and face limits (Ruling B38). From the same
probe's `encode_narrow 500001 48 narrow-u8x48-500001.drc`, as
`narrow-u8-1000.drc` but 48 components, byte-identical on two runs.

### `ours/`: our `textured` fixture through glTF-Transform

The `textured` fixture as this module's GLB writer writes it (the input
`textured.glb` of the `gltfpack/` files above, sha256
`ff0ea594669e00e8cc913182b952b77b82342dfe5670f0daae62a150fccefaa2`), through
glTF-Transform's CLI, `@gltf-transform/cli` 4.5.1 from npm (Google's Draco
encoder inside, `draco3dgltf` 1.5.9), run with Node v26.9.0. The content is
litegen's; neither tool attaches a licence to its output.

| file | sha256 | how |
|---|---|---|
| `textured.q.glb` | `d2b4596d12f89c5900284a21f9c6011eb9909d6f5670d284eb4bc6f26ba7e1d0` | `node cli.js quantize textured.glb textured.q.glb`: `KHR_mesh_quantization`, with SHORT and UNSIGNED_SHORT normalized attributes and UNSIGNED_BYTE colours |
| `textured.q.draco.glb` | `7ade2e2e0017c05db14b13690b5290be5c5e745fb712aca24cfd502e09cf7152` | `node cli.js draco textured.q.glb textured.q.draco.glb`: its streams hold those integers as they are, and drop 6 vertices no triangle uses (73 to 67) |
| `textured.q.draco.meshopt.glb` | `ab98b4ba27c5e307ef320db4efbd303fd190691d50c484a7ac3e3264ece04fba` | `textured.q.draco.glb` with bufferViews 1 and 2, two of its three Draco streams, wrapped in `EXT_meshopt_compression` (ATTRIBUTES, byteStride 4, required, the plain bytes nowhere in the file) by meshoptimizer 1.2.0's own JS encoder, `encodeGltfBuffer`, run by item 2's ledger script `task4/scripts/wrap-meshopt.mjs`: `node wrap-meshopt.mjs <meshopt_encoder.js> textured.q.draco.glb textured.q.draco.meshopt.glb 1 2` |

`cli.js` is `node_modules/@gltf-transform/cli/bin/cli.js`. A second run of all
three gave byte-identical files.

### `reference/`: Google's own decoding of a corpus of our meshes

`draco_core_matches_googles_decoder` (`draco.rs`) proves `draco-core` equal,
byte for byte, to google/draco's own decoder: 445 streams of 22 of our
geometries (this module's fixtures, the five samples above, `ours/` and
`gltfpack/`, with normals, UVs, colours, tangents, joints and weights) at 23
encoder settings each, written by google/draco `d429de84b52a`'s library and
decoded by it, each decoding hashed in `reference/manifest.txt`. The README
there has the build (`-ffp-contract=off`, and why), the tools' sources, the two
streams Google's decoder refuses and how they are held, and the commands that
regenerate it all (mesh-gaps item 2, Task 4b).
