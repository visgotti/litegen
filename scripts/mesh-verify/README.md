# mesh-verify

Render-verifies `litegen-core/src/mesh`: every direction of conversion between every
supported 3D container, judged by **independent** readers rather than by the converter's
own.

```sh
cd litegen-core
cargo run --example mesh_matrix -- /tmp/matrix                 # write every file, every direction
cd ../scripts/mesh-verify && npm ci
node verify.mjs /tmp/matrix /tmp/report                        # render, compare, validate
open /tmp/report/report.html
```

`--fixtures textured,skinned` and `--formats glb,fbx,usdz` narrow a run.
`cargo run --example mesh_matrix -- /tmp/m --fixtures none --import <dir>` runs third-party
files instead (each read with the format its extension names), and adds a **native** check:
the original rendered by its own loader against our reading of it.

You can get real vendor output for `--import` without paying for a generation. fal's
OpenAPI document for each endpoint embeds example output files, and they are public:
`https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=<slug>`. Grep the JSON for
`https://….(glb|fbx|usdz|obj)` URLs. On 2026-10-12 the documents for these endpoints held
example meshes:

- `fal-ai/meshy/v6-preview/image-to-3d`: one model as FBX, USDZ, OBJ and GLB.
- `fal-ai/hyper3d/rodin` and `rodin/v2`.
- `fal-ai/hunyuan3d/v2` and `fal-ai/hunyuan3d-v21`.
- `tripo3d/tripo/v2.5/image-to-3d`.

Keep these files out of the repo, since their licences are the vendors'. The 8-file vendor
set is 880 cases, takes about 30 minutes and uses about 4 GB of disk.

## What a run proves

The generator (`examples/mesh_matrix.rs`) writes, for each fixture in
`mesh::fixtures` and each pair of containers *A → B*:

- `src/<A>/…` — the fixture written as *A* by our writer;
- `conv/<A>__<B>/…` — that file read back by our *A* reader and written as *B*;
- `expected/<hash>.glb` — `B.prepare(A.prepare(fixture))`: the fixture reduced to what
  both containers can hold. This is the lossiness table as code. A conversion is correct
  when it looks exactly like this, no more and no less.

`verify.mjs` then renders every file **with three.js r183's own loader for its format**
(the engine behind `<model-viewer>`, and so behind the dashboard), unlit, at a fixed camera,
in four modes, at each animation sample time (chosen between keyframes, where
interpolation mistakes show):

| mode | shows | catches |
|---|---|---|
| `geom` | face orientation from screen-space derivatives | moved, scaled, rotated or missing geometry; reversed winding (back faces are culled) |
| `normals` | vertex normals as colour | lost, unnormalised or untransformed normals |
| `albedo` | base colour × texture × vertex colour | wrong colour space, lost or misplaced textures, V-flips, dropped materials |
| `uv` | a UV-gradient texture | wrong, flipped or swapped texture coordinates |

Each picture is compared pixel by pixel with the expected picture rendered by
`GLTFLoader`. Identical data rasterises identically, so passing cases are 0.00 % apart;
the thresholds only absorb texture filtering and 8-bit rounding.

Independent checks on top:

- **Khronos glTF-Validator** on every `.glb`/`.gltf` (errors fail the case);
- **`usdchecker --arkit`** on USD (Apple's USD tools, macOS);
- **`assimp info`** on everything assimp imports (a second, independent parser);
- **SceneKit** (macOS, `scenekit/render.swift`, built on demand): Apple's renderer, via
  Model I/O, the stack behind Quick Look. It renders OBJ, PLY, STL, USD and Collada
  output, and its pictures are compared with the same expected picture: silhouettes
  within 1 %, albedo within 2 % with a one-pixel neighbourhood (two renderers sample
  textures with sub-texel offsets).
- **usdrecord** (Pixar's Hydra Storm, USD's reference renderer) on every USD output,
  through the harness camera (put in a session layer, so the file's own time codes are
  untouched), at the default time and every sample time:
  - `usdrecord:cover` compares silhouettes with the expected double-sided coverage
    (within 3 %);
  - `usdrecord:albedo` routes each UsdPreviewSurface's diffuse input to emission with
    lighting off, then compares colour away from edges, within 8 % (the two renderers
    filter one-texel stripes differently).
- **`crate:geom`**: three.js renders a `usdcat` crate copy of each skinned USD output.
  three.js reads crate rotations correctly, and USDA ones not (below).

A case is **refused** rather than failed when the converter declines it and the expected
scene cannot be written either. The one case is a model whose rest pose has no area (every
triangle degenerate), which no 3MF can hold.

## Oracle conventions it encodes, and why

Every one of these is a documented disagreement between readers. None is a tolerance:

- **V direction.** GLTFLoader keeps glTF's top-left UV origin; the OBJ, FBX, Collada, 3MF
  and PLY loaders keep their formats' bottom-left one and flip textures instead.
  `render.js` flips those UVs before the `uv` mode compares them (`V_UP`).
- **Missing normals and UVs.** OBJLoader invents normals when a file has none, while
  GLTFLoader does not. So the `normals` and `uv` modes run only when the expected file
  really has that data.
- **Asynchronous textures.** MTL, FBX, Collada and 3MF loaders return before their
  textures decode. The renderer waits for every texture, and fails the case if one never
  loads.
- **Vertex-colour space.** Model I/O reads OBJ and PLY vertex colours as linear, while
  three.js, MeshLab and Blender read sRGB (which litegen writes). The SceneKit renderer
  is told the convention (`vertexColorsAreSRGB`) instead of skipping the comparison.

Where a reader cannot show something however the file is written, that one comparison is
skipped. The report gives the reason, citing the reader's source or a measurement, and
another oracle judges the same data:

| reader | cannot | judged instead by |
|---|---|---|
| three.js FBXLoader | apply `DeformPercent` (default morph weights) | the geom/albedo modes at animated times, and our reader in the default pose |
| three.js FBXLoader | morph normals ("TODO: add morph normal support") | the geom mode at the same times; unit tests compare every shape normal |
| three.js FBXLoader | a classic material's colour beside its map: it multiplies the colour in, where the SDK, Maya and Blender use the map alone | for an FBX original, the same comparison with mapped colours left to their maps (`albedo_map`) |
| three.js USDLoader | linear colours (read as sRGB), per-vertex `displayColor`, USDA `quatf[]` rotations, BlendShapes, zero-scale rest joints | usdrecord, and `crate:geom` |
| SceneKit, USD | linear sRGB colours (reads Display P3), primvar readers, a SkelAnimation's default `blendShapeWeights`; it also draws meshes under undefined `over` prims (where Apple's importer keeps instancing prototypes) | usdrecord |
| SceneKit, OBJ | `Kd` beside `map_Kd` (it uses the map alone; the MTL spec multiplies) | three.js |

SceneKit draws a USD stage in the file's own numbers: it applies neither `metersPerUnit`
nor `upAxis`. So for a USD original, its camera is moved into the file's space, as the
usdrecord camera is.
| SceneKit, Collada | clips that animate the same node (it binds keys by target, so the clips overwrite each other) | three.js, which plays each clip; SceneKit still judges the rest pose |

SceneKit draws the rest pose with every animation removed, and an animated frame with only
the compared clip playing. By default it would play every clip at once.

USD has one timeline. three.js's USDLoader splits it into a clip per kind of data (a
SkelAnimation's and the xformOps'), so for USD files every clip plays together.

Third-party files that need a different camera (a model in centimetres, say) are compared
**auto-framed**, so loaders that ignore unit metadata can still be compared for shape,
orientation and colour.

## Tools

A full run needs Node (for Playwright's Chromium and the validator, from `npm ci`), plus:

- `assimp`;
- on macOS, `swiftc` (for SceneKit);
- `usdrecord`, `usdcat` and `usdchecker` (Apple's USD tools, or an OpenUSD build).

Any tool that is missing is named at the end of the run and in `report.json`
(`summary.tools`), and its checks are skipped. A missing usdrecord fails the USD cases
instead, since three.js alone cannot judge them.
