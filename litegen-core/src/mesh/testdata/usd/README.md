# USD test vectors

Files read by the tests in `usd.rs` (`include_bytes!`). Every one carries
litegen's own content — the fixtures in `fixtures.rs`, or layers written for
these tests — encoded by a tool that is not ours, so the reader is tested on
bytes our writer never produced.

Licence: the content is litegen's. The tools that encoded it are Apple USD
Tools 0.25.2 (built on OpenUSD 25.02, Modified Apache 2.0) and three.js r183
(MIT); neither licence attaches to its output.

## Authored for these tests (text), then re-encoded by Pixar's crate writer

| file | what it exercises |
|---|---|
| `stress.usda` | every value encoding the crate format has: compressed int and int64 arrays, float arrays compressed as integers (`'i'`) and through a lookup table (`'t'`, also for double and half), uncompressed floats, short arrays, int8-inlined vectors and diagonal matrices, out-of-line vectors and matrices, quaternions, inlined strings/tokens/assets, asset arrays, float-exact and inexact doubles, time samples, nested dictionaries, relationship targets, connections |
| `comp.usda` (+ `comp_sub.usda`, `comp_payload.usda`, `comp_anim.usda`) | composition: a sublayer (weaker than the root), an internal reference whose material binding must be remapped, inherits from a class, a selected and an unselected variant set, an inactive prim, an `over` with a `def` under it, an external payload, and an external reference with a layer offset (offset 10, scale 2) |

```sh
usdcat stress.usda -o stress.usdc
usdcat comp.usda -o comp.usdc      # arcs are kept, not flattened
```

The tests require the crate files to read back value for value as their text
originals do, and `comp` to compose as `usdcat --flatten comp.usda` shows.

## Our writer's output, re-encoded by Pixar's crate writer

`cube.usdc`, `skinned.usdc`, `morph.usdc`: the `cube`, `skinned` and `morph`
fixtures written as USDA by this module (`mesh_matrix ... --formats usda`,
`<fixture>/src/usda/model.usda`), then

```sh
usdcat model.usda -o <fixture>.usdc
```

## Packaged by Apple's tools

`textured_arkit.usdz`: the `textured` fixture written as a USDA package by this
module (`model.usda` + `textures/quadrants.png`), then

```sh
usdzip --arkitAsset model.usda textured_arkit.usdz
```

which re-encodes the root as `model.usdc` and moves the texture to
`0/quadrants.png` — the shape Apple's packagers ship.

## Imported from glTF by Apple's tools

`textured_apple.usdz`, `skinned_apple.usdz`: the `textured` and `skinned`
fixtures as GLB (this module's GLB writer), imported by Apple's glTF file-format
plugin (USD-Fileformat-Plugins 1.0.8):

```sh
usdextract -o ext textured.glb               # ext/textured.usdc + ext/quadrants.png
usdcat ext/textured.usdc -o fixed.usda
sed -i '' -E 's#@[^@]*\.glb\[([^]]*)\]@#@\1@#g' fixed.usda
usdcat fixed.usda -o textured.usdc
usdzip textured_apple.usdz textured.usdc quadrants.png
```

(and the same for `skinned` with `bands.png`). The one `sed` substitution is
the only edit: `usdextract` writes the PNG out but leaves the texture's asset
path pointing into the source GLB by absolute path
(`@/…/textured.glb[quadrants.png]@`), which no other machine can resolve; it is
rewritten to the extracted file beside the layer. Everything else is Apple's
importer verbatim, including its quirks: a NodeGraph and Material interface
inputs between the texture and the surface, unflipped `st` with a
`UsdTransform2d` V-flip on every texture, instancing through an internal
reference, joints duplicated as animated Xforms, a Skeleton marked `invisible`
and `proxy`, `(0, 0, 0, 0)` quaternions for unanimated joints, unnormalised
quaternions, and cubic-spline keys whose in-tangents were read as values (so
the hip's scale really is zero at t = 0).

## Exported by three.js

`textured_three.usdz`: the `textured` fixture as GLB, loaded with three.js
r183's `GLTFLoader` and written by its `USDZExporter` in headless Chromium
(the exporter needs a canvas to encode textures):

```js
const gltf = await new GLTFLoader().loadAsync('textured.glb');
const bytes = await new USDZExporter().parseAsync(gltf.scene, { quickLookCompatible: false });
```

Each mesh lives in its own layer (`geometries/Geometry_N.usda`) pulled in by an
external reference, the box twice; materials sit in a root prim outside the
default prim.
