# Mock vendor files

Vendor files as the mock 3D provider returns them, or as delivery's tests feed
them, each loaded with `include_bytes!` from this folder. litegen code never
reads `mesh/testdata/` (the mesh library will be lifted into its own crate), so
a file taken from there is a copy.

| file | sha256 | source | read by |
|---|---|---|---|
| `skinned.cc.glb` | `4196f2fb927f55998cbd7a8080bbb49de1280aafcaabda871e30196ce34b1627` | a copy of `litegen-core/src/mesh/testdata/gltf/gltfpack/skinned.cc.glb`: gltfpack `-cc`, made as the `gltfpack/` section of `litegen-core/src/mesh/testdata/gltf/README.md` says | `mock/meshopt-3d` (`mock.rs`) |
| `skinned.cf.noq.glb` | `c7c040494ac3a3057afb921eb796a86a9775203954bfe491bec4584a8de8889f` | gltfpack `-cf -noq`, below | `model3d_deliver.rs`'s tests |
| `skinned.draco.glb` | `f7997c911589be5707caacd8043e1263c9738d5fe5c0c108d3b52a8c01c6e8f5` | glTF-Transform's `draco`, below | `mock/draco-3d` (`mock.rs`) |
| `textured.basisu.glb` | `33dec76705eeffc1dfa1577e1f69998f0aa044a023b0acfddddd395142e396ea` | a copy of `litegen-core/src/mesh/testdata/gltf/ktx2/textured.basisu.glb`: the `textured` fixture with a KTX2 base-colour texture, made as that folder's README says (no gltfpack). Its one texture is CarConcept's `Dot_C.ktx2`, **not our content**: Creative Commons Attribution 4.0 International (<https://creativecommons.org/licenses/by/4.0/legalcode>), "(c) 2024, Darmstadt Graphics Group GmbH. Eric Chadwick for Model and textures", from a public domain 3D model by Unity Fan, as the Khronos glTF-Sample-Assets `Models/CarConcept/` `LICENSE.md` says; as-is, no warranties | `mock/basisu-3d` (`mock.rs`), `tests/model3d_api.rs` and `tests/convert_api.rs` |

The first three come from the mesh library's `skinned` fixture as its GLB writer
writes it (the ignored test `write_the_gltfpack_inputs` in `mesh/glb.rs`; input
sha256 `2ea6eb3c261be28a88b1e8027248aee0f0f498b76d18c5647041576dcd35ee13`).
The content is litegen's; neither gltfpack (MIT) nor glTF-Transform (MIT) nor
Google's Draco encoder inside it (Apache-2.0) attaches a licence to its output.

The two gltfpack files were made by the same gltfpack: 1.3.0 from npm, run
with Node (`library.wasm` sha256
`ddd1658ec64b95916dc2b8d5ecff906501c178b5aa2df2fbf22eb8ae5422a417`, `cli.js`
sha256 `c7f3a81fa93a3609847fb75817e4777c339bd5f94963bdbb9aa224a6fdb038ce`).

- `skinned.cc.glb` requires `EXT_meshopt_compression` and
  `KHR_mesh_quantization`. It uses the OCTAHEDRAL (normals), QUATERNION
  (rotation keys) and EXPONENTIAL (translation and scale keys) filters, and has
  no fallback data.
- `skinned.cf.noq.glb` only uses `EXT_meshopt_compression` and requires
  nothing. `-cf` keeps an uncompressed fallback, and `-noq` turns quantization
  off (without it, the file still requires `KHR_mesh_quantization`):

  ```sh
  node node_modules/gltfpack/cli.js -i <dir>/skinned.glb -o skinned.cf.noq.glb -cf -noq
  ```

  gltfpack writes that fallback beside the GLB as `skinned.cf.noq.fallback.bin`,
  which the GLB's buffer 1 names by `uri`. It isn't included here: litegen
  decodes the compressed streams and never decodes from the fallback, and the
  tests read the GLB without it. A second run gave byte-identical files.
- `skinned.draco.glb` requires `KHR_draco_mesh_compression` (mesh-gaps item
  2, Ruling B26: our own content, compressed by an encoder independent of the
  `draco-core` decoder litegen reads it with). It was made by glTF-Transform's
  CLI, `@gltf-transform/cli` 4.5.1 from npm (installed with
  `npm install --ignore-scripts --save-exact @gltf-transform/cli@4.5.1`),
  whose `draco` command runs Google's own Draco encoder, `draco3dgltf` 1.5.9
  (`draco_encoder.wasm` sha256
  `89579de81b9d64cd1dc8accd57ab0c0793aa045fad851c97f465e99a5948c0fb`), under
  Node v26.9.0, with its defaults (EdgeBreaker, speed 5, positions at 14 bits,
  normals 10, texture coordinates 12, other attributes 12):

  ```sh
  node node_modules/@gltf-transform/cli/bin/cli.js draco <dir>/skinned.glb skinned.draco.glb
  ```

  Its one primitive's JOINTS_0, NORMAL, POSITION, TEXCOORD_0 and WEIGHTS_0
  are all in the Draco stream, with no fallback; the PNG texture, the skin
  (hip, knee, ankle) and the clip `Bend` are kept as they were. A second run
  gave a byte-identical file.
