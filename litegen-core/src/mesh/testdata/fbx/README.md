# FBX test vectors written by an independent tool

These files are read by the tests in `../../fbx.rs` (`include_bytes!`). They
exist so the FBX reader is tested on bytes our own writer never produced: they
were written by **Assimp 6.0** (`assimp export`, BSD-3-Clause), from
litegen's own fixtures (`../../fixtures.rs`). No third-party content is
involved; the files carry no licence beyond litegen's own.

| file | Assimp output | source |
|------|---------------|--------|
| `textured_assimp_7500.fbx` | binary FBX 7500 (`-ffbx`) | `textured` fixture |
| `morph_assimp_ascii.fbx` | ASCII FBX 7.5 (`-ffbxa`) | `morph` fixture |
| `cube_assimp_ascii.fbx` | ASCII FBX 7.5 (`-ffbxa`) | `cube` fixture |
| `skinned_small_assimp_7500.fbx` | binary FBX 7500 (`-ffbx`) | `skinned_small_source.glb` |
| `skinned_small_source.glb` | — (our own glTF writer) | `skinned` fixture, reduced |
| `staged_assimp.fbx` | binary FBX 7500 (`-ffbx`), Assimp 6.0.5 | `staged` fixture, as is |

## How they were made

Each source is written as GLB by litegen's own writer, then converted:

```
assimp export textured.glb      textured_assimp_7500.fbx      -ffbx
assimp export morph.glb         morph_assimp_ascii.fbx        -ffbxa
assimp export cube.glb          cube_assimp_ascii.fbx         -ffbxa
assimp export skinned_small_source.glb skinned_small_assimp_7500.fbx -ffbx
assimp export staged.glb        staged_assimp.fbx             -ffbx
```

`staged.glb` is `fixtures::staged()` written by litegen's own glTF writer
(`mesh::write_scene(&fixtures::staged(), MeshFormat::Glb,
WriteOptions::default())`, 4,804 bytes on 2026-11-06), with no wrapper: the
fixture has three roots, which Assimp keeps under a root of its own. Assimp
6.0.5 (Homebrew; the file's `Creator` says 6.0.0) wrote `staged_assimp.fbx`
on 2026-11-06. Running the command again gives the same bytes but for its
`CreationTimeStamp`.

The sources differ from the fixtures in two ways, each working around an
Assimp exporter defect so that the FBX carries the fixture faithfully:

- **`textured`, `morph`, `cube`**: the fixture's root nodes are placed under
  one identity node named `wrapper`. Assimp turns a glTF scene's single root
  node into the FBX scene root and drops that node's transform (the
  `textured` fixture would lose its 20° root rotation).
- **`skinned_small`**: the `skinned` fixture with its tube decimated to every
  4th ring and side (Assimp writes arrays uncompressed; the full tube is
  150 KB), and with only its LINEAR animation channels. Assimp writes a STEP
  channel as linear keys, and a CUBICSPLINE channel as three keys per key at
  one time (in-tangent, value and out-tangent as separate keys).

`litegen-core/examples/fbx_testdata.rs` writes these sources
(`cargo run --example fbx_testdata -- <fixture> out.glb`, or
`-- skinned_small out.glb`). On 2026-10-12 it reproduced the committed
`skinned_small_source.glb` byte for byte.

## What Assimp does differently from our writer (and the tests cover)

- binary **7500**: 64-bit record headers and 25-byte NULL records;
- no array compression;
- **no BindPose** (skinning comes from the clusters' `Transform` alone);
- `UnitScaleFactor 1` on metre data, so the reader's one root correction
  scales the model by 1/100 — the honest reading of what the file declares;
- `DeformPercent 0` whatever the glTF default weights were;
- the scale animation property spelled `"Lcl Scale"` (the SDK's is
  `"Lcl Scaling"`), and every key's `KeyAttrFlags` 0;
- colours written as linear values, which this reader (like three.js) decodes
  as sRGB: material and vertex colours are therefore not compared;
- lights in its own conventions (`staged_assimp.fbx`): `Color` is the colour
  times the intensity, `Intensity` is always 1000, `InnerAngle` and
  `OuterAngle` are the glTF half-angles in degrees, `DecayType` is 0, a
  point light's range goes on its model as `PBR_LightRange`, and each light
  model keeps the glTF node's rotation, so FBX's −Y points where glTF's −Z
  did not; and **no cameras** at all (its exporter's `// TODO: cameras`).
  `fbx.rs`'s `reads_assimps_lights_under_its_own_conventions` cites the
  Assimp source for each, and reads the file with
  `FbxColorSpace::Linear`, since Assimp writes its light colours linear.
