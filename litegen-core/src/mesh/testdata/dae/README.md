# Collada test vectors

Collada files written by tools other than litegen's own writer, read by
`dae.rs`'s tests so the reader is exercised on other exporters' conventions
rather than only on its own output. Every file's content is litegen's own
deterministic fixture data (`mesh::fixtures`), so nothing here carries a
third-party licence: all files are under the repository's licence.

The textures these files reference (`textures/quadrants.png`,
`textures/bands.png`, `textured_assimp_texture_0001.png`) are byte-identical to
`fixtures::quadrant_png()` / `fixtures::bands_png()`, so the tests supply them
from there instead of committing copies.

## `scenekit/` — Apple SceneKit's Collada exporter

`skinned_scenekit.dae`, `textured_scenekit.dae` and `cube_scenekit.dae` were
written by SceneKit ("SceneKit Collada Exporter v1.0", macOS 26 / Swift 6.3)
from litegen's own Collada of the `skinned`, `textured` and `cube` fixtures,
with the textures beside them:

```sh
cargo run --example mesh_matrix -- /tmp/m --formats dae       # in litegen-core
swiftc -O export.swift -o export                              # source below
./export /tmp/m/skinned/src/dae/model.dae /tmp/m/skinned/src/dae/skinned_scenekit.dae
./export /tmp/m/textured/src/dae/model.dae /tmp/m/textured/src/dae/textured_scenekit.dae
./export /tmp/m/cube/src/dae/model.dae /tmp/m/cube/src/dae/cube_scenekit.dae
```

```swift
import Foundation
import SceneKit
let input = URL(fileURLWithPath: CommandLine.arguments[1])
let out = URL(fileURLWithPath: CommandLine.arguments[2])
let data = try! Data(contentsOf: input)
let src = SCNSceneSource(data: data, options: [.assetDirectoryURLs: [input]])!
let scene = try! src.scene(options: [.assetDirectoryURLs: [input]])
print("write ok:", scene.write(to: out, options: nil, delegate: nil, progressHandler: nil))
```

They encode things litegen's writer does not: nested `<animation>` elements
whose samplers have no INTERPOLATION input, transform sids renamed
(`SID_1`), joint sids that differ from node ids, `<triangles>` with
NORMAL at its own offset, and a single texture coordinate set numbered
`set="1"` bound through `<bind_vertex_input semantic="CHANNEL2" input_set="1">`.

## `assimp/` — assimp 6.0's Collada exporter

`textured_assimp.dae` and `cube_assimp.dae`: assimp's export of litegen's GLB of
the `textured` and `cube` fixtures (static scenes: assimp 6.0 writes animated
skins with missing `<source>`s, which no reader loads).

```sh
assimp export /tmp/m/textured/src/glb/model.glb textured_assimp.dae
assimp export /tmp/m/cube/src/glb/model.glb cube_assimp.dae
```

They use `<polylist>` with every input at offset 0, a geometry per primitive,
effect-local texture names and assimp's `CHANNEL0` texcoord naming.

## `research/` — hand-authored references

`ref.dae` and `morph.dae` were written by
`docs/superpowers/research/usd3mfdae/scripts/make_ref_dae.py <dir>` during the
format research (2026-10-12), and `basecolor.png` by `make_gltf.py <dir>`
beside it; on 2026-10-12 both scripts reproduced the committed files byte for
byte. `ref.dae` is a reference document checked against the 1.4.1
specification, which loads in three.js r183 and assimp 6.0; `morph.dae` is a
morph-over-skin variant with a morph-weight channel (three.js cannot load it —
see `dae.rs`). `morph_relative_full.dae` was edited by hand from `morph.dae`:
the weight animation removed, the scene instancing the morph controller
directly (no skin), `method="RELATIVE"`, its target rewritten as offsets from
the base, and the weight set to 1, so the morph shows in a static frame;
SceneKit's rendering of it matches litegen's reading.
