# A glTF with a `KHR_texture_basisu` texture

One GLB whose base-colour texture is a KTX2 (Basis Universal) image, the input
of mesh-gaps item 3's tests: `ktx2_textures_survive_every_container` builds the
same file in memory, `the_committed_ktx2_glb_carries_the_corpus_texture` reads
this one, and the mock vendor `mock/basisu-3d` and the converter and delivery
tests of `litegen-core/tests/` serve it as a vendor's file.

| file | bytes | sha256 |
|---|---:|---|
| `textured.basisu.glb` | 8,160 | `33dec76705eeffc1dfa1577e1f69998f0aa044a023b0acfddddd395142e396ea` |

## What it is

`fixtures::textured()` with its base-colour image (`quadrants`, a PNG) replaced
by `../../ktx2/Dot_C.ktx2` (ETC1S with BasisLZ, 128 x 128, 1,374 bytes; see
`../../ktx2/README.md`), written by this tree's own GLB writer. The writer
puts the texture under `KHR_texture_basisu` and lists the extension in both
`extensionsUsed` and `extensionsRequired`, which is what a GLB from gltfpack's
`-tc` (ETC1S) or `-tu` (UASTC) looks like to a reader. The image is the
KTX2 file byte for byte, so the pixels it must decode to are the
reference `../../ktx2/Dot_C.rgba.zst` (the Basis Universal transcoder's output
for it, 65,536 bytes).

## How it was made

From `litegen-core`, with a folder to write to:

```sh
KTX2_GLB_OUT=src/mesh/testdata/gltf/ktx2 cargo test --lib -- --ignored mesh::tests::write_the_ktx2_glb
```

The ignored test `write_the_ktx2_glb` in `src/mesh/tests.rs` is that command's
whole content: `glb::write_scene` of the `textured` fixture with `Dot_C.ktx2`
as its image. The writer is deterministic, so a run gives these bytes again
while the writer and the fixture are as they were.

## Licence

The geometry, materials and nodes are `fixtures::textured()`, litegen's own.
The one texture is CarConcept's `Dot_C.ktx2`, **not our content**: Creative
Commons Attribution 4.0 International
(<https://creativecommons.org/licenses/by/4.0/legalcode>). "(c) 2024,
Darmstadt Graphics Group GmbH. Eric Chadwick for Model and textures", from a
public domain 3D model by Unity Fan; the Khronos glTF-Sample-Assets
`Models/CarConcept/` model's `LICENSE.md` says so. This file is provided as-is,
with no warranties of any kind.

## What is not here

gltfpack's own `-tc` and `-tu` encodings of these fixtures. Building gltfpack
(or Binomial's encoder) is building external code, which waits on the user
(Ruling K17), so no file here came from either. A GLB the real tools write is
the case to add if the user allows the build.

## The Khronos validator

`gltf-validator` (the version in `scripts/mesh-verify`) reports 0 errors on
this file, with two warnings and three infos that all follow from the
validator not knowing `KHR_texture_basisu`: it cannot validate the extension,
names `image/ktx2` a value outside its list, does not recognise the image's
format, and calls the image and the texture coordinates unused.
