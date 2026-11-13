# mesh-verify

Render-verifies `litegen-core/src/mesh`: every direction of conversion between every
supported 3D container, judged by **independent** readers rather than by the converter's
own.

Ruling IDs ("Ruling A35") are mesh-gaps item 4's rulings, listed with their costs in the
status block of `docs/superpowers/plans/2026-11-04-mesh-material-layers.md`.

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
the original rendered by its own loader against our reading of it (geom and albedo, each
file framed on itself; for a glTF original with a material layer, `lit` too, through one
camera: see "The native check in `lit`" below). A glTF original is loaded with three.js's
own decoders for the two compression extensions, set on the native check's `GLTFLoader`
(`loaderFor`, `page/render.js`): `DRACOLoader` with r183's wasm decoder
(`node_modules/three/examples/jsm/libs/draco/gltf/`) for `KHR_draco_mesh_compression`,
and `MeshoptDecoder` (`examples/jsm/libs/meshopt_decoder.module.js`) for
`EXT_meshopt_compression` and `KHR_meshopt_compression`. So a compressed original is the
reference, read by nothing of litegen's (mesh-gaps item 2, 2026-11-08: Khronos's sixteen
`glTF-Draco` and six meshopt variants and gltfpack's 36 files, every native geom, albedo
and lit comparison 0.00 % except two skinned models' auto-framed frames, BrainStem's
0.02-0.12 % and RiggedFigure's 0.01-0.03 %, within the 5 % threshold; the uncompressed
BrainStem is 0.02-0.18 % from three.js by the same measure, so that is the model's; and
one gltfpack file, `layered.cz.glb`, whose native check is a cited skip because three's
bundled meshopt decoder mis-decodes one of its streams: Ruling B46, under "Asset
limits" below).
Without them GLTFLoader throws on a Draco file and reads only a meshopt view's fallback
buffer.

A third decoder, `KTX2Loader`, reads `KHR_texture_basisu` images (KTX2, Basis Universal
ETC1S or UASTC), so a KTX2 original is drawn by three.js's own transcoder too (see "KTX2
originals" below). Add `--transcode-textures` to the generator for such an import: it runs
`mesh::normalize::transcode_textures` on each file right after reading it, the step that
production (`delivery`, the converter) runs, so our reading holds PNG textures where the
original holds KTX2. Without the flag an import keeps the KTX2 bytes, both sides of the
native check are then drawn through `KTX2Loader`, and the check proves nothing about our
decoder. The flag is for imports only and off by default.

`--dae-keep` adds, for every direction into Collada, a second case whose file is written with
`dae_morphs: keep` (mesh-gaps item 8, Ruling DM15), named `dae-keep`. SceneKit renders it,
three.js and assimp are expected to refuse it, and xmllint validates it ("Collada morphs: the
`dae-keep` variant", below). It is off by default, and without it a run is byte-identical to
one made before the flag existed.

`--usd-variants` adds, for every direction into USDZ, three cases: `usdz-all` and `usdz-crate`
keep every clip, `usdz-crate-first` is a crate-rooted USDZ with the default single clip
(mesh-gaps item 6; "USDC, several clips and unlit in USD", below). Off by default,
and without it a run is as it was.

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
in four modes and the per-set uv modes `uv1`–`uv3`, at each animation sample time
(chosen between keyframes, where interpolation mistakes show):

| mode | shows | catches |
|---|---|---|
| `geom` | face orientation from screen-space derivatives | moved, scaled, rotated or missing geometry; reversed winding (back faces are culled) |
| `normals` | vertex normals as colour | lost, unnormalised or untransformed normals |
| `albedo` | base colour × texture × vertex colour | wrong colour space, lost or misplaced textures, V-flips, dropped materials |
| `uv` | a UV-gradient texture | wrong, flipped or swapped texture coordinates |
| `uv1`–`uv3` | the same gradient through uv sets 1–3 (the set selector) | a set lost, swapped with another, or written with the wrong data |
| `lit` | the loader's own material under a fixed image-based light (one of the three lit modes, with `lit_coat` and `lit_flat`) | a material layer lost or changed: clearcoat, sheen, transmission, volume, iridescence, ior, specular, anisotropy (each measured below); not dispersion, which moves no pixel |
| `camera` | the geom mode through the file's own camera (the one named like the expected scene's first), at that camera's aspect | a camera lost, moved, turned (the 90° of a missing axis fix), or given the wrong fov or aspect; a near or far plane only where it cuts geometry, so the numeric oracle judges the clip planes (below) |
| `lit_lights` | the loader's own material lit by the file's own lights alone, with no environment | a light lost, moved, turned, recoloured, dimmed, or given the wrong cones |

The per-set modes run where the expected file has that set. A case is asked for each set
its expected scene keeps, and each one is judged by a reader:

- three.js, where the target's loader builds the set. GLTFLoader and FBXLoader build
  every set, and three.js's materials address four. ColladaLoader and USDLoader build
  two, so their sets 2 and up are skipped, citing the loader's lines.
- SceneKit's `uv` mode, for those Collada and USD sets. It draws the gradient through the
  set's `mappingChannel`. It also draws any set that three.js judged in no frame for a
  reason of its own, in every format SceneKit reads (Collada, USD, OBJ, PLY and STL).
  Unlike the sets above, these fail nothing when SceneKit is missing: they had no judge
  before.
- Nothing, for the uv sets of a USD file with BlendShapes. three.js cannot morph it, and
  SceneKit's rest pose is skipped (it ignores default blend-shape weights). SceneKit's
  animated frames are pictures of the pose: a BlendShape moves the points and leaves the
  uvs alone. The skip cites all three, and usdrecord's and SceneKit's silhouettes judge
  the pose.
- Nothing, for a set past the fourth: three.js's materials address four
  (`three.module.js` `getChannel`), so not even the expected picture can be drawn. That
  skip says so.

Each picture is compared pixel by pixel with the expected picture rendered by
`GLTFLoader`. Identical data rasterises identically, so passing cases are 0.00 % apart;
the thresholds only absorb texture filtering and 8-bit rounding.

### The `lit` mode: material layers

The layers show in nothing unlit, so `lit` draws the loader's own material, as loaded,
under a fixed image-based light, through the fixture's camera at each of its sample times
(`page/render.js`). The spec's "four existing camera poses" (Ruling A34) match nothing
here: the harness has one camera per fixture (`camera` in the manifest, framed on the
fixture's bounds over its animation), plus the sample times of an animated fixture;
`layered` is static, so its lit pictures are one view. Its settings:

- the light is three.js's `RoomEnvironment` (r183's, the model-viewer room: a point
  light, six boxes and seven area lights in a box), prefiltered by
  `PMREMGenerator.fromScene(room, 0.04)` (the documented blur) and set as
  `scene.environment`;
- tone mapping is off (`NoToneMapping`, so no ACES), and the exposure is
  `scene.environmentIntensity = 0.5` (`LIT_EXPOSURE`; `toneMappingExposure` applies only
  under a tone-mapping pass). The exposure was scanned on an earlier six-box `layered`:
  at 1.0 the room clipped 47 % of the fixture's pixels (a clipped pixel hides a changed
  layer), at 0.5 0.65 %, at 0.35 0.11 %, and two expected files with different layer
  sets differed on 28–96 % of pixels at 0.5, 15–85 % at 0.35, 11–79 % at 0.25 (darker
  pictures put less past the then 20/255 tolerance), so 0.5 is the brightest exposure
  that does not clip. On the final seven-box fixture at 0.5, the full expected picture
  clips 1 of its 11,921 pixels (0.01 %, the worst of its 19 expected files);
- `lit_flat` is the same under a uniform white environment (`ColorEnvironment`) at
  exposure 1.0 (nothing clips: its brightest pixel is 242), the expected picture for
  usdrecord's dome-lit render below.

The mode is asked for where the expected file keeps a layer (read off GLTFLoader's
`MeshPhysicalMaterial`: `report.json` lists them per case as `layers`) and the target's
loader can build one. Who judges:

- **three.js GLTFLoader**, for `glb` and `gltf`: every `KHR_materials_*` layer the
  expected file shows (next paragraph), at **0.00 %** (`THRESHOLD.lit = 0`) with a
  per-channel tolerance of **1/255** (`LIT_TOLERANCE`, `page/render.js`): the same
  loader, renderer and environment as the expected picture, so a correct case is bit-exact
  (max delta 0 on every judged case of every run), and 1 absorbs the rasterisation noise a
  differently ordered geometry can give (a PLY-sourced GLB read max delta 1 against its
  expected picture in round 0) while the unlit modes' 20/255 — meant for two loaders'
  texture filtering — hid four layers (anisotropy, specular, ior and dispersion moved at
  most 12, 13, 3 and 0). Calibrated 2026-11-05 on the final `layered`: each of its 19
  expected files against itself, 0 pixels apart at tolerances 0, 1 and 2; GLB → GLB and
  every other glTF case, 0 pixels apart; an expected file against another expected key (a
  different layer set), 74–100 % apart.

  **What `lit` can see is measured per expected file** (`layerVisibility`,
  `page/render.js`; the reviewer's probe of 2026-11-05 built in): each `KHR_materials_*`
  extension is removed from every material in turn, the variant is rendered through
  GLTFLoader and compared with the expected picture at the lit tolerance; a layer is seen
  in a mode when a pixel moves. `report.json` records it as `litSeen` per expected file,
  and a case is judged in a mode only where one of its layers is seen there; a case whose
  layers all move nothing is a measured skip, and a judged case lists its unseen layers as
  `litUnseen`. On `layered`'s full expected file (every layer), at tolerance 1:

  | layer removed | `lit` pixels moved | max delta | | layer removed | `lit` pixels moved | max delta |
  |---|---|---|---|---|---|---|
  | clearcoat | 23.02 % | 191 | | specular | 33.76 % | 13 |
  | sheen | 32.96 % | 190 | | ior | 10.48 % | 3 |
  | transmission | 13.51 % | 70 | | anisotropy | 10.70 % | 12 |
  | volume | 10.12 % | 29 | | **dispersion** | **0.00 %** | **0** |
  | iridescence | 8.36 % | 29 | | (TANGENT attribute removed) | 8.12 % | 188 |

  So dispersion is seen nowhere (three.js spreads the refraction by
  `(ior − 1) · 0.025 · dispersion`, 0.004 in ior for the glass, under a pixel at 256 px),
  and ior is seen through the lacquer alone (three.js fixes a coat's F0 at 0.04,
  `lights_physical_fragment.glsl.js:60`; the ior moves the black base's F0 under it): in
  the expected files where only
  glass's 1.52 and plastic's 1.6 remain (dae → glb, where Collada dropped the lacquer's
  1.45 as one of its defaults; obj → glb) removing ior moves at most 1/255, so dae → glb and
  dae → gltf are measured skips and obj → glb's ior is listed unseen. In `lit_coat`
  (exposure 4, the lacquer alone) a removed clearcoat moves 100 % of the coat's pixels
  (max delta 63) and a removed ior 100 % (max delta 6).
- **three.js USDLoader**, for `usdz` and `usda`, through **`lit_coat`**: `lit` with only
  the meshes USDLoader reads right drawn, on both sides, at **0.00 %**
  (`THRESHOLD.lit_coat = 0`). It builds a `MeshPhysicalMaterial` and reads
  `inputs:clearcoat`, `inputs:clearcoatRoughness` and `inputs:ior` as values
  (`USDComposer.js:2661-2671`, `:2626-2630`), but decodes a base or emissive colour as sRGB
  (`:2512-2520`, `:2541-2562`; right only at 0 and 1), keeps `displayColor[0]` alone
  (`:1089-1096`) and follows no coat texture (its one connection-following path,
  `_applyTextureOrValue` `:2453-2489`, is not used for them). So `lit_coat` draws a mesh
  when its material, as loaded, has unit base and emissive colours, no vertex colours and
  no coat map (`usdLitJudgeable`, `page/render.js`); a colour written wrongly hides the
  mesh on one side and fails on coverage. Its exposure is 4.0 (`LIT_COAT_EXPOSURE`): a
  coat over a black base reflects F0 0.04 of the room. The exposure was scanned at the
  then 20/255 tolerance: at 0.5 a coat of 1 against one of 0.2 moves no pixel past it
  (max delta 20), at 2 and 3 they differ on 29.9 %, at 4 on 38.1 % (max delta 47), with
  nothing clipping (brightest pixel 184). At the lit tolerance of 1 and exposure 4, a
  removed clearcoat moves 100 % of the coat's pixels (max delta 63), a removed ior 100 %
  (max delta 6), and control (a)'s coat of 0.2 100 %. `layered`'s `lacquer` (black,
  clearcoat 1 at roughness 0.2, ior 1.45) is such a mesh; its 13 USD cases that keep its
  coat are judged there, each 0 pixels apart (dae → usdz/usda keep no layer on it, since
  Collada drops 1.45, and skip). The whole-file `lit` of a USD case stays a cited skip
  with its picture kept. One measured `lit_coat` skip is listed by case
  (`LIT_COAT_MEASURED`, Ruling A39): Khronos ToyCar's single-file USDA, whose body (its
  coat untextured there, clearcoat 1 at roughness 0) GLTFLoader and USDLoader draw 3 of
  2,068 pixels apart where the data agree (with the source's rotations in the expected
  file, the two loaders' geom and normals are bit-exact and 2 pixels still differ: a
  roughness-0 coat mirrors the room's area-light edges); it is skipped while the
  difference stays within those 3 pixels, and the threshold stays 0.
- **usdrecord**, for `usdz` and `usda`, under a white `UsdLuxDomeLight` in the session
  layer (intensity 0.35, through a constant white texture file: Storm lights nothing
  specular from a dome without a texture, "Dome light has no texture asset path",
  `pxr/imaging/hdSt/simpleLightingShader.cpp:277` at OpenUSD v25.02 and v25.05, the
  installed Apple USD Tools 0.25.2; `:283` at v25.08, `:289` at v25.11), compared
  with `lit_flat`. **Not a judge** (Ruling A32): Storm adds the coat lobe on top of the
  base (`/usr/lib/usd/usd/usdShaders/resources/shaders/previewSurface.glslfx:330-344`,
  Apple USD Tools 0.25.2, with F0 from the ior, `:296`) where three.js weights the base by
  the coat's Fresnel, so a correct coated file differs from `lit_flat` on 55.47–59.89 %
  of its pixels (every USD layer case, 38.64–59.89 %); and under a uniform dome a coat
  barely moves Storm's own picture: control (a) (the lacquer's coat written as 0.2
  instead of 1, car_paint's coat map lost) moves the lacquer by at most 10/255, no pixel
  past the 20/255 tolerance (only where a usd → usd case lost car_paint's coat map does
  Storm move 434 pixels by up to 44/255), and the ratio by 2.1–6.5 points, to
  61.75–62.01 %. The margin, about two points on a ~60 % baseline, is pixels sitting at
  the tolerance edge, not a coat Storm shows: too fragile to judge. (Measured 2026-11-05
  on the final fixture, `harness/usdlit-diff.mjs`, r2-report against r2-ctl-a-report; an
  earlier six-box fixture gave 48.71 % for both, with a 0.35 dome intensity chosen as the
  best of 0.2–0.7.) Its picture, diff and ratio stay in the report as `usdrecord:lit`,
  skipped with that reason; USD coats are judged by `lit_coat` and the usd → glb cases.
- **nothing**, for `obj`, `dae`, `fbx`, `3mf`, `ply` and `stl`: their loaders build Phong
  or Lambert materials and read no layer property (`LIT_SKIP` in `verify.mjs`, each
  citing the loader's lines: MTLLoader's property switch has no case for `Pc`, `Pcr`,
  `Ps`, `Ni`, `aniso` or `anisor`; ColladaLoader reads no `<index_of_refraction>`). The
  mode is not drawn for them. The layers an OBJ keeps (clearcoat, sheen, anisotropy; its
  ior too, unseen) are judged by the obj → glb cases. Collada keeps ior alone and drops
  1.45 as one of its defaults, so a dae → glb case carries only glass's 1.52 and
  plastic's 1.6, which move at most 1/255: dae → glb and dae → gltf are measured skips,
  and no lit case judges Collada's ior.

A case whose expected scene keeps no layer skips the mode, saying so; so does a case
whose layers all move nothing when removed (a measured skip, with each layer's max delta
in the reason).

**What `lit` cannot see: the glTF writer dropping a layer.** The expected file is written
by the same glTF writer as the output under test, so a writer that drops a layer drops
it from the oracle too, and every glTF case still matches at 0.00 % (Ruling A33; the
controls below ran against known-good expected files to show the judge itself works).
That writer is pinned instead by `every_pbr_extension_round_trips`' hand-written JSON
(`glb.rs`) and by the `--import` run's source-against-output check (Ruling A20). `lit`
guards every reader and every other writer.

**The native check in `lit`** (Ruling A35). For a glTF original with a material layer
(on either side, as GLTFLoader builds them), the native check also draws the original
and our reading of it (`reference.glb`) in `lit`, through one camera, the fixture's
(`renderNativeLit`), and judges them at `THRESHOLD.lit` (0) with the lit tolerance; the
two sides' layer sets must match as well, so a layer `lit` cannot see (dispersion) still
has to come through. Here the oracle is the original itself, not litegen's writer, so
this pass also catches the writer dropping a layer. Calibrated on the 16 Khronos
originals (2026-11-05, task 5b): 15 are 0 pixels apart. ToyCar is 1 pixel past the
tolerance (max delta 40), because litegen's glTF reader renormalizes a node rotation
(`glb.rs:1181-1190`) and its GLB writer writes it in f64 (`glb.rs:3170-3171`), where
three.js composes the original's f32-rounded, slightly non-unit quaternion as given. So
where the pass fails, it is repeated on our reading with the original's rotations put
back (nodes paired by index and name). The one excuse is a repeat at 0 pixels in which
no restored quaternion differs from ours by more than 1e-6 in any component (q and −q
being one rotation; `LIT_ROTATION_BOUND`, the size of a renormalization), recorded with
the citation (`nativeLitRotations`): ToyCar's is 0 with its 11 rotations put back, none
moved by more than 4.85e-8. A rotation off by 1° moves one by about 0.0087, and the
pass fails even though putting it back closes the difference (the final fix wave's
control).
The existing native thresholds (tolerance 20, 5 %, each side auto-framed) are not a
`lit` judge: two of three altered readings of ToyCar passed them (Glass's transmission
at 0.9, 0.90 %; the coat map pointed at another image, 3.61 %). This pass fails all
three (task 5b's controls): transmission 0.9 at 1.37 % (154 pixels, 153 with the
rotations put back), Fabric's sheen deleted at 81.41 % and on the layer sets, and the
coat map at 14.70 %.

Every judge must fail under a negative control. The two for the layers (2026-11-05, on
`layered`, each run against the known-good expected files, since a mutant writer would
otherwise write the oracle too):

- the glTF writer with `KHR_materials_sheen` dropped: `every_pbr_extension_round_trips`
  fails, and the nine lit cases whose expected scene keeps sheen fail, 32.96 % of pixels
  apart (scene, single, glb, gltf → glb/gltf) and 15.28 % (obj → glb/gltf), against 0
  pixels on the correct writer (at the 1/255 lit tolerance; 6.82 % and 3.03 % at the
  earlier 20/255);
- the USD writer with `clearcoatRoughness` written into `clearcoat` (its value, and its
  map's G channel into the coat's connection): `clearcoat_and_ior_round_trip_through_usd`
  fails; the thirteen `lit_coat` cases (every USD target whose expected keeps the lacquer's
  coat) fail, 100 % of the coat's pixels apart; and the four usd → glb/gltf lit cases
  fail, 22.03 % apart (our reader refuses the coat read from G, so car_paint's coat map is
  lost), all against 0 pixels; `usdrecord:lit` moved as recorded under A32 above, which is
  why it is not a judge.

Independent checks on top:

- **Khronos glTF-Validator** on every `.glb`/`.gltf` (errors fail the case; for our own
  fixtures' outputs a warning fails it too — they carry none, 0 across 161 files on
  2026-11-04 — while a third-party original's cases are held to errors alone);
- **`usdchecker --arkit`** on USDZ, and plain **`usdchecker`** on USDA and USDC (Apple's
  USD tools, macOS). `--arkit` judges a file for AR, and for AR a USD file is a USDZ. Its
  extra rules are what a package for AR Quick Look and RealityKit must meet. Until
  2026-11-04 every USD output got `--arkit`;
- **`assimp info`** on everything assimp imports (a second, independent parser). One
  refusal is excused, when the expected GLB is refused alike: assimp 6.0.5 puts
  `KHR_materials_specular`'s `specularTexture` in its SPECULAR slot 0 and
  `specularColorTexture` in slot 1 (`code/AssetLib/glTF2/glTF2Importer.cpp:293-294`), then
  its own validation rejects the gap (`code/PostProcessing/ValidateDataStructure.cpp:514-516`,
  "Specular #1 is set, but there are only 1 Specular textures"), though the extension makes
  the two textures independent;
- **SceneKit** (macOS, `scenekit/render.swift`, built on demand): Apple's renderer, via
  Model I/O, the stack behind Quick Look. It renders OBJ, PLY, STL, USD and Collada
  output, and its pictures are compared with the same expected picture: silhouettes
  within 1 %; albedo on **4×4 block means** (Ruling A38, `compareImageBlocks`): the mean
  colour of each block covered throughout on both sides, at the albedo tolerance of
  24/255, with at most 2 % of blocks past it. The per-pixel figure (a one-pixel
  neighbourhood, for textures sampled with sub-texel offsets) stays in the report as
  `pixelRatio`. Two renderers minify a texture finer than a pixel differently pixel by
  pixel, where block means agree: Khronos SheenCloth's fabric, tiled 30 × 30, put 6.30 %
  of SceneKit's pixels past the per-pixel tolerance and 0 of its 504 blocks past this
  one; DispersionTest's checker 3.67 % and 0 of 943 (2026-11-05, task 5b). The worst
  correct case, in the 16 imports and the every-fixture run alike, is 3 blocks of about
  640 (0.47 %). A wrong texture moves whole blocks (task 5b's controls): `textured`'s
  quadrants replaced by a fabric 44.32 % of blocks, mirrored left to right 43.70 %, and
  SheenCloth's fabric replaced by the quadrants 100 %. Where no block is covered
  throughout, the per-pixel verdict stands. Its `uv` mode draws the
  gradient through a chosen set (`mappingChannel`) for the Collada and USD sets three.js
  does not build, within 2 %.
- **Alpha, in SceneKit and usdrecord** (Ruling A37): the expected albedo and coverage
  pictures draw every material as one opaque colour (`albedoMaterial`), while SceneKit
  and Storm draw a translucent material blended and a texture's alpha. So where the
  expected file has a translucent or cut-out material (BLEND, MASK, an opacity below 1),
  a base-colour texture with an alpha below 1, or vertex colours with an alpha below 1
  (`stats.translucent`, `stats.textureAlpha`, `stats.vertexAlpha`, read per expected
  file), their albedo and silhouette comparisons are skipped, citing why, and their
  pictures kept. Measured on the Khronos imports (task 5b): TransmissionTest's Collada
  through SceneKit 3.38 % of pixels apart, 0.00 % with its textures' alpha removed;
  GlassVaseFlowers 39.27 %, 0.06 % made opaque and without texture alpha;
  IridescenceMetallicSpheres' silhouette 4.52 %, 0.27 % without its transparency;
  GlassVaseFlowers' ply → dae (the glass's alpha baked into a PLY's vertex colours,
  read back with no material, which glTF draws opaque) 34.82 %, 0.00 % with that alpha
  at 1. three.js still judges those colours in albedo and their layers in `lit`. A USD
  case that ends with no judge of its geometry at all, three.js's geom and normals,
  usdrecord's cover and SceneKit's silhouette each skipped in every frame (BlendShapes,
  which three.js does not read, under this alpha gate), fails (`USD_NO_GEOMETRY_JUDGE`);
  no input does so today.
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

### The layer-extension check

`lit` cannot see the glTF writer dropping a layer, and the native check sees only what
`lit` and the layer sets show. So in a matrix made with `--import`, every glTF output of
a glTF original, written from our reading of it (`scene`, `single`) or through glb or
gltf (`glb`, `gltf` → `glb`, `gltf`: seven outputs per original), is compared with the
original by `layer-extensions.mjs` (Ruling A20), material by material:

- the same set of the nine layer extensions;
- each field equal once the extension's schema defaults are filled in, and a field the
  schema doesn't name compared as given;
- each layer texture by the sha-256 of the image bytes it resolves to, its texCoord, its
  `KHR_texture_transform` and a normal texture's `scale`. A texture whose image resolves
  to nothing, on either side, fails;
- no output material that pairs with no source material carries a layer.

**Pairing** (followups 2026-11-08, item F). A source material goes with the output
material at its own index when the counts match and every named source material is kept
at its index. Otherwise, when two materials the reader keeps share a name (so a name
cannot pair them: Khronos VirtualCity has 167 materials under 33 names, `_22jyj` 35
times and `Scrape04` 20), by the order litegen's reader keeps them in (`readerMaterialOrder`). The reader
allocates a material when the first primitive that uses it is read (`glb.rs` `material`,
L2191-L2266, called at the end of `primitive`, L2171-L2175), walking the default scene's
nodes in the depth-first order of `page/node-rank.js` and reading each node's mesh at
its first sight (`nodes`, L2794-L2864 and L2868-L2873; a file with no scene takes every
mesh in file order, L2800-L2807), and the writer writes `scene.materials` in that order
with no reordering (L4040-L4121). So an output's `k`th material is the source's `k`th by
first use, and the pairing is used only when the counts match and every named source
material keeps its name at its place, so a material a wrong order would misplace still
fails by name. Otherwise a name used once pairs with its namesake and a name used more
than once with none (the old behaviour, unchanged for a file whose names are unique).
A source material no primitive of the scene uses is kept by neither reader nor writer
and stays an unpaired source, as before. The pairing used is in each case's
`layerExtensions.pairing` (`index`, `reader order` or `name`).

Floats compare as f32 (Ruling A20a): two numbers that round to the same f32 are equal,
since the scene holds f32, and each such pair is listed in the case's `layerExtensions`
in `report.json`. Any other difference fails the case, however small. A layer's `extras`
and its own nested extensions are excluded and listed. The run's last lines count the
outputs checked and the f32 pairs (`summary.layerExtensions`). Outputs through the other
containers lose layers by design: those losses are reported, and `lit` judges what they
keep.

The check runs alone too, on any glTF pair:

```sh
node layer-extensions.mjs <source.glb|.gltf> <output.glb|.gltf> [<output> ...] [--json <file>]
```

Its tests, from the repo root:

```sh
node --test scripts/mesh-verify/layer-extensions.test.mjs
KHRONOS_SCHEMAS=<folder> node --test scripts/mesh-verify/layer-extensions.test.mjs
```

The second form also checks the defaults table against Khronos's nine schema files; without
`KHRONOS_SCHEMAS` that one test is skipped, saying so. The files are Khronos's, so they
are not in the repo. Fetch them at the commit the table was written from,
KhronosGroup/glTF `9d23a5a5b1d10f22d1554e847f2a1a3f24222392`, into one folder:

```sh
for l in anisotropy clearcoat dispersion ior iridescence sheen specular transmission volume; do
  curl -fLO "https://raw.githubusercontent.com/KhronosGroup/glTF/9d23a5a5b1d10f22d1554e847f2a1a3f24222392/extensions/2.0/Khronos/KHR_materials_$l/schema/material.KHR_materials_$l.schema.json"
done
```

Then check them against these sha-256 sums (`shasum -a 256 -c`):

```
bedd1b5c202bae64430f99358f89dd43da9ee1d2e4d5af6e32a0653ba997ff0f  material.KHR_materials_anisotropy.schema.json
56201df5bc73bebc9185f693ac9b69ad0108fdb4143b6274c3929b97c7cd2911  material.KHR_materials_clearcoat.schema.json
90b96906ca94f520658df714dddc9002c59df536786cff0ef06f239aec0f5cb1  material.KHR_materials_dispersion.schema.json
cd9649bf71d15eb0e3f2842329720ce74eccfdc2d1b383776c000ae58f660e57  material.KHR_materials_ior.schema.json
76be44f7c6fdc3cab3d10c4072228138003c0338cd35e089d8d55e6aa879f6a3  material.KHR_materials_iridescence.schema.json
fbb0f4a3a3d3e9b7b0dac4495dd4a37f77573fe1211bc9c5b6300f02193899ce  material.KHR_materials_sheen.schema.json
543465f7f2aa185b2f15c242b88df67759c4b488438282a3650002794bb70228  material.KHR_materials_specular.schema.json
ba7f35963c83c3def22235cc6958d94fa605d663ec49a65865ebc1ec61fbbea7  material.KHR_materials_transmission.schema.json
581681f6c15ab1983190c4db7e6d7b10697dea4bbf93773face7e21f2f52ff27  material.KHR_materials_volume.schema.json
```

### Lights and cameras (mesh-gaps item 5)

A file's own lights and cameras (the spec's "Render harness"; Ruling IDs R1–R32 are
item 5's, listed with their costs in the "Rulings" section of
`docs/superpowers/plans/2026-11-04-mesh-lights-cameras.md`) are judged in two modes
and by a numeric oracle (`CAMLIGHT` in `verify.mjs` and `page/render.js`):

- **`camera`**: the geom mode drawn through the file's own camera, at rest and at each
  sample time, 256 px high and 256 × the expected first camera's aspect wide (384 × 256
  for `staged`), against the expected GLB through its own first camera (GLTFLoader's
  `gltf.cameras[0]`, or the first it places). Every judge picks the case's camera by that
  camera's node name, a node with no name by its depth-first rank (Ruling R25 as amended,
  below), never by its place in a reader's list: a reader orders its cameras its own way
  (SceneKit's USD importer, below), and GLTFLoader clones a camera two nodes share onto
  each. A case is drawn at its expected picture's size, so a wrong aspect squeezes it.
  `camera_cover` is the same view's coverage, both sides drawn, for usdrecord, which
  draws a camera at the camera's own aspect: for a camera with no aspect ratio of its
  own, which litegen writes at 16:9 to USD (`usd_write.rs` `write_camera`, its
  `aspect.unwrap_or(fbx::UNSET_ASPECT)`; `fbx.rs` `UNSET_ASPECT`; `docs/superpowers/3d-format-conversion.md` §5),
  the expected picture is drawn at 16:9 too (`UNSET_ASPECT`, `page/camera-view.js`;
  the expected key `<key>#aspect`), where SceneKit and the other judges keep the
  expected scene's own square view. A camera that has an aspect keeps it on both sides,
  so a changed aspect still fails (negative controls, below). The geom picture shows a
  near or far plane only where it cuts geometry (on `staged` a near plane up to about 18×
  too large, or a far plane anywhere from 7.5 to 64, moves no pixel), so the clip planes
  are the numeric oracle's to judge.
- **`lit_lights`**: the loader's own material lit by the file's own lights alone (no
  environment, `LinearToneMapping` at `LIT_LIGHTS_EXPOSURE` 0.4), through the fixture's
  camera. The exposure was scanned on `staged`'s expected file at rest and at its six
  sample times: at 1.0, 3,718–4,773 of its 12,450 pixels clip, at 0.6 134–178, at 0.5
  0–19, at 0.4 none (brightest 243); 0.4 is the brightest that does not clip.
- **the numeric oracle**, `camlight-oracle.mjs` (below), on every FBX, Collada, glTF and
  USD output.

Who judges each target:

| target | `camera` | `lit_lights` |
|---|---|---|
| glb, gltf | three.js GLTFLoader, within geom's 1 %; and the oracle | GLTFLoader, at 0.00 % with the lit tolerance (1/255); and the oracle |
| fbx | assimp's reading (Ruling R11): the expected geometry drawn through a camera built from assimp's assjson at each time; and the oracle | the expected GLB lit by lights built from assimp's reading, at 0.00 %; and the oracle |
| dae | SceneKit's silhouette through the camera named like the expected first camera, within 1 %; and the oracle | SceneKit, one light at a time (below); and the oracle |
| usdz, usda | SceneKit's silhouette through that camera, within 1 %, and usdrecord `--camera <its Camera prim>`, within 3 %; and the oracle's USD camera check | usdrecord, one light at a time (below); SceneKit reads no USD light |
| obj, ply, stl, 3mf | none: the expected scene keeps no camera | none: it keeps no light |

The pictures draw through the expected first camera alone. Every camera's frame (eye,
forward and up) is the oracle's to judge, at rest and, on an animated case, at each sample
time, in every format above that keeps a camera.

three.js's FBXLoader, ColladaLoader and USDLoader are no judge of either (the spec's
"Who decides each mapping"); each of their cases is a skip citing the loader's lines
(`CAMERA_THREE_SKIP`, `LIGHTS_THREE_SKIP`). A glTF output must also hold as many
cameras and lights as its expected scene. And each camera and each light of the expected
scene must end with a judge of its own, or the case fails naming it (`camlightJudged`).
A camera's frame is judged by the oracle's eye, forward and up checks of it, at rest or
at a sample time (its yfov, aspect and clip planes are its own values, which say nothing
of where it stands), or, for the camera the pictures draw through, by a camera picture
judged in some frame. A light is judged by the oracle's checks of it passing (its
position and direction among them), by its own one-light picture (SceneKit, usdrecord)
judged in some frame, or by the whole-scene `lit_lights` picture (three.js, or assimp's
reading) judged in a frame where that light lights something the fixture's camera sees
(the expected `light:<node>` pictures count its lit pixels, `lightLit`). So a light that
is dark in every frame is never passed by a picture of the others.

**The numeric oracle.** The spec's "assimp's glTF conversion" cannot carry them:
assimp 6.0's glTF2 exporter writes no camera and no light (Ruling R11, the controller's
probe). So the oracle runs `assimp export <file> <out>.assjson -fassjson` on every FBX,
Collada, GLB and glTF output and on every expected GLB that has a light or camera, and compares
assimp's reading with the case's expected GLB, read from its own glTF JSON: each
camera's eye, forward, up, yfov, aspect, near and far, and each light's kind, position,
direction, colour × intensity and cones, within 1e-4 (scale-aware: assjson prints 6
significant digits, `json_exporter.cpp` L171-L174 and L228; directions by angle), and
the shape of each light's falloff, not its numbers: the inverse square, a constant and a
linear term of 0 and a quadratic one above 0. On an animated case each camera's eye,
forward and up are compared again at each sample time (`eye@<t>` and so on,
`sampleTimeChecks`): the expected scene's first
animation sampled from its GLB's own keys as glTF 2.0 defines it (LINEAR, rotations by
slerp; STEP; CUBICSPLINE; the end keys held), against assimp's first animation (its keys
in ticks; a component without keys is the identity's, as assimp's glTF importer writes
none for a component the node does not write, `glTF2Importer.cpp` L1330-L1480) or a USD
stage's time samples (below). Where assimp's convention differs, the comparison is
adjusted and the reason cited (assimp v6.0.5; the table is `ASSIMP` in
`camlight-oracle.mjs`):

| importer | what assimp does | adjustment |
|---|---|---|
| FBX | stores half of `FieldOfView` as the horizontal fov (`FBXConverter.cpp` L501-L504) | doubled |
| FBX | looks a camera along the model's +X with +Y up (L483-L484), aims a light along −Y (L411-L414) | the node's world transform applied to assjson's vectors |
| FBX | multiplies `Intensity`/100 into `Color`, read as written (L401-L407) | compared with srgb(colour) × intensity: litegen's default `FbxColorSpace::Srgb` (Ruling R14) |
| FBX | takes `InnerAngle` and `OuterAngle` as written (L425-L428), litegen's full cones | halved |
| FBX | reads no `CameraProjectionType` or `OrthoZoom` (L472-L508) | the projection and an orthographic camera's magnifications skipped, cited |
| FBX | reads `DecayType` 2 as (0, 0, 2/`DecayStart`²) (L457-L461) | the falloff checked as (0, 0, q > 0) |
| Collada | stores the full horizontal fov (`ColladaLoader.cpp` L422-L436) | yfov = 2·atan(tan(hfov/2)/aspect) |
| Collada | sets the horizontal fov from `<xfov>` (L422-L423) or from `<yfov>` together with `<aspect_ratio>` (L430-L432), and from `<yfov>` alone leaves `aiCamera`'s default of π/4 (`camera.h` L187), which L436 turns from degrees to radians: 0.0137078 (followups 2026-11-08, item B). litegen writes `<yfov>` alone for a camera with no aspect ratio, which COLLADA 1.4.1 allows | assimp's yfov is skipped for exactly that case (`COLLADA_YFOV_ALONE`) and the file's own `<yfov>`, in degrees, is judged in its place (`colladaCameraOptics`), against the scene's, to the oracle's tolerance. A file with `<xfov>` or `<aspect_ratio>` keeps assimp's number |
| Collada | makes `<falloff_angle>`, the full cone (Ruling R23), its inner cone and guesses its outer from `<falloff_exponent>` (L357-L372) | the outer cone is half of assimp's inner; the inner comes from the exponent recovered from the guess, by `dae.rs`'s documented rule (`colladaInnerCone`) |
| Collada | reads `<orthographic>` as a perspective camera, `xmag` and `ymag` into the fov fields (L400-L403; `ColladaParser.cpp` L396-L401) | the projection, aspect and magnifications skipped, cited |
| Collada | reads no optics for a `<camera>` with no `name` attribute (`ColladaParser.cpp` L1037-L1058: it `continue`s before `ReadCamera`, L1049-L1051), which COLLADA 1.4.1 makes optional (`collada_schema_1_4_1.xsd` L2379-L2385): the camera keeps Collada's defaults (no fov, near 0.1, far 1000). litegen names a camera that has no name by its id since item 5's final fix wave, so another writer's file is what still needs this | assimp reads a copy in which each nameless `<camera>` is named after its id, nothing else changed (`namedColladaCameras`; the case's `camlightOracle.note` says so) |
| glTF | the full horizontal fov (`glTF2Importer.cpp` L924-L926); an orthographic camera as fov 0, aspect xmag/ymag (L929-L937); cones as written (L998-L1000) | none: the calibration |
| glTF | keeps `xmag` as `mOrthographicWidth` (L933), which assjson does not write | an orthographic camera's xmag and ymag read from the file's own glTF JSON, on the node assimp named the camera after (`withGltfMagnifications`) |
| assjson | writes no camera position, nor `mOrthographicWidth` (`json_exporter.cpp` L684-L709) | a camera's eye is its node's origin (every importer here leaves `mPosition` at 0) |

assimp names a camera or a light after its node (`FBXConverter.cpp` L399/L476,
`ColladaLoader.cpp` L337/L407, `glTF2Importer.cpp` L1249-L1255), so the two sides pair
by node name, and a camera or light on one side only fails. A node with no name pairs by
its depth-first rank (Ruling R25 as amended, `nameCandidates`), under each reader's name
for it: `node <rank>` in the expected scene and on the page, `node<rank>` in litegen's FBX
(`fbx_write.rs`, the `node{i}` fallback it hands `Names::unique`) and as its Collada id (`dae.rs` L5129, no name written: assimp
names a Collada node after its id, `ColladaLoader.cpp` L1801-L1822, and `render.swift`
names a nameless SceneKit node by it), `node_<rank>` in its USD (`usd_write.rs`, the `node_{n}` fallbacks),
and `nodes[<rank>]` to assimp's glTF importer (`glTF2Importer.cpp` L1106-L1108,
`glTF2Asset.inl` L506). So an unnamed camera is judged, not failed.

*The rank, not the glTF's array index* (followups 2026-11-08, item A). litegen's glTF
reader numbers nodes in a depth-first walk of the default scene (roots in the scene's
order, then each node's children in theirs; an explicit stack, `glb.rs` `nodes`,
L2794-L2864), and every writer keeps that order, so the `node{i}`, `node_{i}` and
`nodes[i]` above are ranks. An original whose `nodes` array is not already in that order
has a different array index for the same node: Khronos's Duck (`Duck.gltf`: the root's
children run `[2, 1]`, so its camera, array index 1, is the third node visited, rank 2)
paired `node 1` with `node 2`: the plain set's 2 native-file differences before the fix
were this (Duck: "the original has [node 1:perspective], our reading
[node 2:perspective]"; VirtualCity's 14 unnamed cameras the same: array index 39 is rank 196, 212 is
rank 23). `page/node-rank.js` (`depthFirstRanks`, `unnamedKey`), imported by the page
and by the oracle, is the one place that numbers them; GLTFLoader's own association key (`parser.associations`, `GLTFLoader.js`
L4394-L4405) gives the array index, and `lightNodeName` translates it through the rank.
A glTF node reached twice keeps its first visit's rank, as the reader does.

A named node keeps its name in glTF and FBX (`fbx_write.rs` `Names::unique`
replaces control characters alone) and pairs under each other writer's sanitised form of
it too, as litegen writes it: the Collada `name` (`dae.rs` `ncname`, L4898-L4919: an
NCName, so a space becomes `_`; SceneKit names a Collada node by it), the Collada id
(`Ids::make`, L4869-L4895: ASCII letters, digits, `_` and `-`, at most 64, so a `.`
becomes `_` too; assimp names a Collada node by it) and the USD prim name (`usd_write.rs`
`identifier`: ASCII letters, digits and `_`, at most 96), and SceneKit's reading of the
Collada name, each `.` made `-` (measured: `a.b.c`, `Lámpara.1` and `m·n.o` read as
`a-b-c`, `Lámpara-1` and `m·n-o`). Khronos's `Generated Camera`, `Light 4 - White` and
`Point.002_Orientation` reach the readers as `Generated_Camera`, `Light_4_-_White` or
`Light_4___White`, and `Point_002_Orientation` or `Point-002_Orientation`. A writer's
de-duplicating suffix (`-2`, `_1`) is not foreseen.

A USD output's cameras are read from its own flattened text (`usdcat --flatten`;
`usdCameras`, `USD_TEXT`): each
Camera prim's own attributes, its metadata (a `customData` dictionary) left out, a
missing one at the schema's default, as UsdGeomCamera defines them (OpenUSD v25.05,
`pxr/usd/usdGeom/schema.usda` L2352-L2373: apertures and focal length in tenths of a
scene unit, `clippingRange` in scene units; `pxr/base/gf/camera.cpp` L274-L297 and
L311-L326, the same at v26.11.1: yfov = 2·atan(verticalAperture / (2·focalLength)),
the aspect horizontalAperture / verticalAperture, an orthographic half-width of
aperture × 0.1 / 2). Its frame is its prim's xformOps composed with its ancestors'
(`usdWorld`, `USD_TEXT.xformCite`; OpenUSD v25.05 `xformable.cpp` L1001-L1028, ops
multiplied in reverse, and L645-L651, `!resetXformStack!`; `xformOp.cpp` L521-L705,
each op's matrix, `!invert!` its inverse): at the default values for the rest pose, and
at time code seconds × `timeCodesPerSecond` for a sample time (linear, quaternions by
slerp, the end samples held: `usd/interpolators.h` L140-L163, `sdf/data.cpp`
L337-L352), turned Y-up and into metres as litegen's reader turns a stage. Its eye,
forward, up, projection, yfov, aspect, xmag, ymag, near and far are compared, so a
camera no picture draws through is judged. `report.json` lists
every check per case (`camlightOracle`), and `summary.camlightOracle` counts files (by
format) and checks.
It runs alone too: `node camlight-oracle.mjs <matrix> [--fixtures a,b] [--json <file>]`,
or `--file <file> --expected <expected.glb>`. Its tests, from the repo root:

```sh
node --test scripts/mesh-verify/camlight-oracle.test.mjs
```

The rules the page, `verify.mjs` and the oracle must agree on live in small pure modules,
so one change serves every caller (followups 2026-11-08): `page/node-rank.js`
(`depthFirstRanks`, `unnamedKey`) and `page/camera-view.js` (`UNSET_ASPECT`,
`cameraViewFor`, `externalPictureSize`), under `page/` because the static server hands
that folder to the browser, and `scenekit-limits.mjs` (`colladaClips`, `sceneKitClip`,
`usdTextureTint` and the skip texts), outside it because `verify.mjs` is a script with
top-level `await` that nothing can import. `camlight-oracle.test.mjs` tests all of them.

**The FBX judge** builds its camera and lights from the same assjson
(`assimpView`): each node's world matrix evaluated at each sample time from assjson's
keys (linear, and slerp for rotations, in ticks), the camera as a three.js
`PerspectiveCamera` (assimp's camera named like the expected first camera, or by its
index's names), the lights
as GLTFLoader builds them (`GLTFLoader.js` L690-L752).
assimp holds srgb(colour) × intensity, so the colour is decoded with the expected
light's intensity, and it reads no `FarAttenuationEnd` (L445-L469), so a light's range
is the expected scene's (`ASSIMP_FBX_LIGHTS`). An orthographic first camera has no
assimp judge (`ASSIMP_FBX_ORTHO`).

**The cross-renderer lights judge** (`compareLightPicture`, `page/render.js`). SceneKit
and Storm keep their own light units and falloff, so for Collada and USD each of the
expected scene's lights is drawn alone on white Lambert surfaces through the fixture's
camera, by three.js (the expected pictures, each at an exposure that puts its brightest
pixel near 230; a light too faint to show at exposure 1 is first brightened 16× at a time
until a pixel shows, as SceneKit's side is too: Blender's FBX export writes `staged`'s
3 lux sun as 0.0044) and by the judge. Each reader's expected picture is drawn as that
reader draws the file's light, each way measured or cited:

- for usdrecord, `light:<node>`: with no `range` cutoff. glTF `range` has no native
  Collada or USD form (item 5's Ruling R27: it rides only in litegen's annotation), so
  Storm and SceneKit light past it; GLTFLoader makes it three.js's `distance`, whose
  window `pow2(saturate(1 - pow4(d / range)))` (`lights_pars_begin.glsl.js` L56-L69) is
  left out (`RANGE_NOT_CARRIED`). Khronos PointLightIntensityTest's red light: Storm's
  footprint 1.44 % apart from that picture;
- for SceneKit, `lightsk:<node>`: no range cutoff either; for a point or spot light no
  distance falloff at all, three.js's `decay` 0 (`SCENEKIT_NO_FALLOFF`: SceneKit reads
  `<quadratic_attenuation>` 1 as attenuation start and end 0, and its brightness over
  three.js's inverse square grows with the distance, Khronos PlaysetLightTest's LEDlight
  p10 1.222, median 1.747, p90 2.284, where the directional light sits at 1.000; against
  the no-falloff picture its footprint is 0.01 % apart, from 14.28 %, and the ratio
  0.986–1.000); and its colour as SceneKit reads `<color>`, litegen's linear colour ×
  intensity (Ruling R17) decoded as sRGB, each channel cut at 1 (Ruling R26,
  `SCENEKIT_SRGB_COLOR`: on Khronos DirectionalLight's sun, `<color>` (0.9, 0.8, 0.1),
  SceneKit's lit pixels hold (1, 0.7670, 0.0127) over the largest channel, the sRGB decode
  (1, 0.7669, 0.0127); its hue is 0.00 % apart from that picture, 100 % from the linear
  one).

The two are compared on:

- *where it falls*: a pixel is lit when a channel reaches 3 of 255; a pixel lit on one
  side only differs unless one within a pixel of it is lit on the other. Within 2 % of
  the pixels lit on either side (`LIGHT_FOOTPRINT_LIMIT`). A wrong direction, outer cone
  or (through the surfaces it faces) position moves it. A frame is skipped only where
  both pictures are dark; a light lit on one side only fails there;
- *its colour*: on pixels lit on both sides, unclipped and at least 128 of 255, each
  side's linear colour over its largest channel, a pixel differing past 0.06 in a
  channel. Within 2 % of those pixels (`LIGHT_HUE_LIMIT`);
- *its brightness*: the median ratio of the two sides' linear largest channels,
  recorded, not judged (each renderer's exposure is its own).

What SceneKit and Storm read, measured on macOS 26.6.2 and Apple USD Tools 0.25.2
(2026-11-06), and how the judges allow for it:

- SceneKit reads no UsdLux light: Model I/O gives a `DistantLight` or `SphereLight`
  prim as a plain `MDLObject` (no `MDLLight`, in both the `inputs:` and the pre-21.02
  attribute names). USD lights are judged by usdrecord alone (`SCENEKIT_USD_LIGHTS`).
- SceneKit clamps each channel of a Collada light's `<color>` to 1 ((2, 0.5, 0.25) reads
  as (1, 0.5, 0.25)), and litegen's `<color>` is colour × intensity (Ruling R17), past 1
  for every `staged` light: its colour check is skipped there (`SCENEKIT_LIGHT_CLAMP`),
  and the oracle judges it.
- SceneKit draws `spotOuterAngle` as the full cone, though `SCNLight.h` calls it the
  angle from the spot direction (an outer angle of 30 lights a cone of half-angle
  15.14°, 20 one of 9.98°), and its Collada importer copies `<falloff_angle>` into it:
  SceneKit reads `<falloff_angle>` as the full cone, as Blender's Collada IO does, and
  as litegen writes it (Ruling R23, which revised R4 on this measurement). Its spot
  footprint is compared with no adjustment.
- SceneKit misreads an orthographic camera in both importers: a Collada one's
  `orthographicScale` (the view's half-height) comes out as xmag × ymag (`staged`'s 3 and
  2 read as 6; the Blender FBX reading's 3 and 1.6875 as 5.0625, where the half-height is
  the ymag), a USD one's as 1 whatever its apertures. So SceneKit's camera judge skips an
  orthographic first camera (`SCENEKIT_ORTHO`); the oracle judges its frame, and for USD
  usdrecord `--camera` its picture.
- SceneKit's USD importer lists a prim's children alphabetically (`staged`'s
  `ortho_camera_node` before `persp_camera_node`), one reason every judge draws through
  the camera named like the expected first camera (`cameraNodes`). And through an imported USD
  camera node SceneKit draws almost nothing (9 of 44,154 pixels), while a copy of the
  same camera on the same frame draws them all, so `render.swift` draws through a copy
  on a child of the camera's node, which keeps its frame and animation.
- SceneKit's Collada importer leaves a node with an id and no name (as litegen writes a
  node with none) nameless; `SCNSceneSource.entryWithIdentifier` returns the same node
  for its id (measured on `staged` written with its names stripped), so `render.swift`
  names it by its id, as assimp does.
- Storm draws a normalized `SphereLight` as a point light of its full intensity
  (`pxr/imaging/hdSt/light.cpp` L159-L161, the same at OpenUSD v25.02, v25.05 and
  v26.11.1), 4× the candela litegen writes it from (Ruling R6): the judge lowers a
  sphere light's exposure by 2 stops, and the brightness ratio left is 1.000 for the bulb
  (and the sun's `DistantLight`, which needs none, 1.000), the 4× measured
  (`STORM_SPHERE_4X`).
- Storm shapes only rect and disk lights (`SetSpotCutoff`, `hdSt/light.cpp` L244-L248),
  so it draws a `SphereLight` spot with no cone: the spot's footprint is skipped
  (it differs from three.js's cone on 5–28 % of the lit pixels), its colour judged
  (`STORM_NO_CONE`). With SceneKit reading no USD light and the oracle reading a USD
  file's cameras alone, **these have no judge here beyond litegen's unit tests: a USD
  spot's cone, position and direction (its footprint skipped), every USD light's
  intensity (its brightness recorded, not judged), and a USD light beyond the expected
  scene's (each picture lights one expected light alone).**
- usdrecord lights a stage with its own lights in every pass, so every pass but the
  lights judge's draws with the file's light prims switched off (`active = false` in the
  session layer, `usdLightsOff`): the albedo and cover passes, the `lit` pass, the
  camera pass and a USD original's native passes. Without that, Khronos
  PlaysetLightTest's 512.25 lux sun washed the albedo picture white (93.30 % of the
  pixels past 16/255, 9.47 % with the lights off; task 6b's experiment).
- usdrecord's `--disableCameraLight` is a bare flag in OpenUSD's Python usdrecord. The
  CLI11 build in `/usr/bin` reads a value, and only `--disableCameraLight=0` turns the
  camera light off (with every light prim deactivated, the bare flag, `=1` and `=true`
  render `staged` lit, median 178 of 255, as no flag does; `=0` renders it black). The
  lights judge passes `USDRECORD_NO_CAMERA_LIGHT`. The older passes keep the bare flag,
  so their pictures have the camera light on: the albedo pass sets every diffuse to 0
  and reflects none of it, while `usdrecord:lit` (not a judge, Ruling A32) was measured
  with it on.
- usdrecord takes the picture's height as the width over the camera's aspect, truncated
  (`pxr/usdImaging/usdAppUtils/frameRecorder.cpp` L386-L391 at v25.05). litegen's f32
  apertures 0.36 / 0.24 make that 255 rows at a width of 384: exactly one row short is
  scaled away (half a pixel at the far edge, inside the silhouette's one-pixel
  tolerance) and said as `rescaled`; any other size is an error (`externalPictureSize`,
  `page/camera-view.js`). Followups 2026-11-08, item C: the same call draws a camera at
  the camera's own aspect, and litegen writes a camera with no aspect ratio at 16:9
  (`usd_write.rs` `write_camera`'s `aspect.unwrap_or(fbx::UNSET_ASPECT)`, `fbx.rs` `UNSET_ASPECT`;
  `3d-format-conversion.md` §5: "For other readers an unset aspect ratio is written as
  16:9", reported as a `cameras` loss), so usdrecord's picture of such a camera is 16:9 by
  design and the size check, which compared it with a square expected picture, failed all
  13 VirtualCity USD cases whose first camera has no aspect ratio (256 × 143, not
  256 × 256). The expected picture for usdrecord's `camera_cover` is now drawn at 16:9
  too (the `<key>#aspect` entry of `window.renderExpected`, only for a camera whose
  expected glTF camera has no `aspectRatio`, `stats.cameraAspectUnset`). The size check
  is not relaxed: a camera that has an aspect ratio is drawn at it on both sides, and the
  negative controls (below) show a changed aspect still fails.
- SceneKit is not shown to judge a Collada `<yfov>` with no `<xfov>` or `<aspect_ratio>`
  (what litegen writes for a camera with no aspect ratio), and assimp's reading of the
  same camera is wrong (item B, the table above). Measured 2026-11-08 (macOS 26.6.2):
  negative control B edits the `<yfov>` of
  VirtualCity's `scene → dae` camera from 57.30° to 40° and SceneKit's `camera` pictures
  stay 0 % apart at every time, so no picture judges that fov; the oracle's check of
  the file's own `<yfov>` is the one that fails it (below). three.js's ColladaLoader
  builds a `PerspectiveCamera` from a lone `<yfov>` with its default aspect of 1, and is
  skipped for every Collada camera in any case (`CAMERA_THREE_SKIP.dae`).
- SceneKit's Collada importer registers an `<animation_clip>` under its name, under its id
  when it has none, and registers none whose key is the id of a node, a geometry or an
  animation of the file (item D; `scenekit-limits.mjs`, `colladaClips`/`sceneKitClip`;
  measured 2026-11-08, macOS 26.6.2, with `SCNSceneSource.identifiersOfEntries` and
  `entryWithIdentifier(_, withClass: CAAnimation)` on the CesiumMilkTruck `.dae` litegen
  writes (clip id `Wheels-2`, name `Wheels`; nodes `Wheels` and `Wheels_001`; animations
  `Wheels-anim` and `Wheels_001-anim`; geometry `Wheels-mesh`) and copies edited as
  below):

  | clip | SceneKit's entry |
  |---|---|
  | id `Wheels-2`, name `WheelsClip` | `WheelsClip`; `Wheels-2` is nil |
  | id and name `WheelsClip` | `WheelsClip` |
  | no name, id `Zed-2` | `Zed-2` |
  | name `Walk.001` | `Walk.001` (verbatim: a `.` is not made `-`, as it is in a node's name) |
  | name `Wheels` or `Wheels_001` or `Cesium_Milk_Truck` (a node's id), `Wheels-mesh` (a geometry's id) or `Wheels-anim` (an animation's id) | none, whichever of its id and name is asked for; the file's own animation entries stay |
  | name `Scene` (the visual scene's id) or `Wheels.001` (a node's name, not its id) | an entry |

  So the key SceneKit knows a clip by is `name ?? id`, and a clip whose key is the id of a
  node, a geometry or an animation has no entry and cannot be played. litegen's Collada
  ids are as `dae.rs` documents (`Ids::make`, L4869-L4895: unique, so the clip of an
  animation named like a node is `Wheels-2`) but its clip name is the animation's
  own name, `Wheels`, the id of the node `Wheels`. `render.swift` is asked for
  the key (`clip`), the harness's old mirror of this rule asked for the id and the
  eight CesiumMilkTruck `→ dae` cases failed with "no animation clip Wheels-2"; they now
  play the key where SceneKit has an entry and, where it has none, skip SceneKit's
  animated frames citing this measurement (`SCENEKIT_CLIP_SHADOWED`; the rest pose is still
  judged, and three.js plays the clip).
- SceneKit's USD importer ignores `UsdUVTexture`'s `inputs:scale` ("Scale to be applied to
  all components of the texture. value \* scale + bias", Apple USD Tools 0.25.2,
  `/usr/lib/usd/usd/usdShaders/resources/shaders/shaderDefs.usda` L196-L202, default
  (1, 1, 1, 1)), where litegen writes a base colour that tints a base-colour texture
  (Khronos MorphPrimitivesTest's OBJ original: two materials of one texture, scales
  (1, 0, 0, 1) and (0, 1, 0, 1)). Measured 2026-11-08 (item E) on the case's own
  `model.usda` and on a copy with every scale set to (1, 1, 1, 1): SceneKit's two
  albedo pictures are byte-identical, the grey checker whatever the file says
  (100.00 % of 4×4 blocks apart from the tinted expected picture), while three.js's
  USDLoader and usdrecord, which follow the scale, agree with the expected picture on
  the correct file (0.00 % and 0.99 %, MorphPrimitivesTest `obj → usdz`/`usda` in
  `r-morph-usd`). So SceneKit's albedo is skipped where the texture on a material's
  `inputs:diffuseColor` has a scale whose colour channels are not (1, 1, 1)
  (`SCENEKIT_USD_TEXSCALE`, `usdTextureTint` over `usdcat --flatten`), and usdrecord's and
  three.js's judge. The gate is the base-colour texture alone: a normal map's
  (2, 2, 2, 1) scale (bias (-1, -1, -1, 0)) tints nothing, and SceneKit's albedo of
  Khronos LightsPunctualLamp's USD, whose normal maps carry it, is 0 % apart, so the first
  version of this gate, which looked at every texture, wrongly skipped 14 passing cases of
  item 5's camera set (found by comparing picture ratios with that set's last reports; the
  gate now has a test for it). An alpha scale is not the opaque albedo's either. `bias`
  was not measured and stays judged.

**Asset limits** (`ASSET_LIMITS`, keyed by the SHA-256 of a third-party original, so a
changed file loses its excuse): a measured property of one asset that keeps one judge
from judging it there, cited where it applies.

| asset (Khronos glTF-Sample-Assets 8f492390) | judge skipped | why (measured) |
|---|---|---|
| DirectionalLight.glb, its sun | usdrecord's light footprint | the three spheres are inside out (outward normals 0 of 5,374, outward-facing triangles 0 of 10,600 on each, single-sided), and our USD keeps that; three.js and SceneKit cull back faces and draw the lit far interior, Storm draws the near side, dark. Storm emits along −Z as UsdLux says: the file turned 180° lights, and an analytic `Sphere` under the same light lights. The sun is exact numerically |
| LightsPunctualLamp.glb, `Point.003_Orientation` | usdrecord's light footprint | Storm samples each pixel 4 times (coverage alpha in quarters), three.js once; the light grazes rims of the lamp's stem thinner than a pixel, which Storm's samples catch and three.js's pixel centre misses: 68 of the 88 differing pixels (8.59 %) are partially covered edges, 20 faint slivers. The light's specular off: 8.59 %; the expected drawn double-sided: 7.80 %; supersampled 2×2: 6.69 %, 4×4: 4.80 %. The light is exact numerically, and SceneKit's footprint of it in Collada is 0.24 % |
| `litegen-core/src/mesh/testdata/gltf/gltfpack/layered.cz.glb`, sha256 `909596012abc…17cb535` (gltfpack 1.3.0 `-cz`: `KHR_meshopt_compression`, v1 streams; mesh-gaps item 2, Ruling B46) | the native check (three.js's own picture of the original); every other check on the file runs | three r183's bundled `meshopt_decoder.module.js` ("Built from meshoptimizer 1.0", wasm) mis-decodes its bufferView 3 (ATTRIBUTES, filter NONE, v1 header `0xa1`, 72 × 8 bytes, the quantized u16 positions): 60 of 576 bytes differ from meshoptimizer's reference decoder (`js/meshopt_decoder_reference.js` at f0a684f, the one `meshopt-vectors.mjs` checks by sha), while the other 136 meshopt views of the 27 compressed gltfpack files decode byte-identically in both (137 views, 136 identical). Dequantized, the reference decode lands on the 168 vertices of the repo's uncompressed `litegen-core/src/mesh/testdata/gltf/gltfpack/layered.glb` and three's is off on 156 of 168 (worst 12.4 units): its picture is a sliver and four squares, 93.5 % of geom pixels from ours, which matches the reference decoder (item 2's Task 2 differential). Measured 2026-11-08 |

To re-check the layered.cz entry, decode each meshopt view twice and compare: for every
`bufferView` with a `KHR_meshopt_compression` (or `EXT_`) extension, call
`MeshoptDecoder.decodeGltfBuffer(target, count, byteStride, stream, mode, filter)` once
on `node_modules/three/examples/jsm/libs/meshopt_decoder.module.js` (after `await
MeshoptDecoder.ready`) and once on the reference decoder, and count differing bytes
(`task5/compare-decoders.mjs`, a local script in item 2's git-ignored ledger,
`.superpowers/sdd/2026-11-04-mesh-compressed-gltf-input/`, does exactly that over
`gltfpack/*.c.glb *.cc.glb *.cz.glb`: on 2026-11-08, 137 views, 136 identical,
`layered.cz.glb view 3 ATTRIBUTES NONE count 72 stride 8 header 0xa1: 60 of 576 bytes
differ`). When a newer three decodes that view like the reference, drop the entry.

**Calibration** (2026-11-06, debug matrices, rerun on the final code of task 6a's
follow-up round): every expected file with a light or camera, drawn as a case against itself, is 0
pixels apart in `camera` and `lit_lights` at every time (max delta 0); GLB → GLB is
0.00 % in both; the FBX judge's path fed with assimp's reading of the expected GLB itself
is 0 pixels past the tolerances (max delta 1, assjson's 6 digits); the oracle on both
expected files agrees with them (67 checks each, 36 of them the two cameras' frames at
the six sample times; the worst direction 4.6e-5° apart, yfov 7.3e-7). The native passes
(below), on `staged`'s own GLB imported as an original, are 0 pixels apart in `camera`
and `lit_lights` at every time (max delta 0). Over `staged`, `textured` and `skinned`
every `camera` and `lit_lights` comparison of a glTF case, and every assimp one, is
0.00 %; SceneKit's camera silhouettes are at most 0.26 % apart and its Collada
footprints 0.01 % (0.14 % before SceneKit's expected pictures dropped the falloff it does
not draw); usdrecord's camera, footprints and colours 0. The oracle's frames
agree everywhere: of USD stages within 6.2e-6° and 3.8e-7 (eye), of assimp's readings
within 9.4e-5° and 6.3e-6.

**Unnamed nodes** (Ruling R25; `harness/unnamed/unnamed.glb`, a local file in item 5's
git-ignored ledger, `.superpowers/sdd/2026-11-04-mesh-lights-cameras/`: `staged` with
the names of its nodes, cameras and lights stripped, imported as a glTF original): all 114 cases
pass, with the same numbers as `staged`. Each judge found the unnamed cameras and lights
under its reader's name for them (SceneKit `node3` or `node_3`, usdrecord's prim
`node_3` or `node3`, assimp `nodes[3]`, `node3` or `node_3`, as the file was written),
the oracle judged both cameras' frames in every
one of its 45 files, at rest and at the six sample times, and the native passes are 0
pixels apart with both sides' cameras and lights paired as `node 3` … `node 7`.

**The `--import` set** (2026-11-06, debug, the follow-up round's final code; local only,
never committed, in `.superpowers/sdd/2026-11-04-mesh-lights-cameras/import/`): six
Khronos glTF-Sample-Assets samples at 8f492390 (Cameras, DirectionalLight,
PointLightIntensityTest, LightsPunctualLamp, PlaysetLightTest, LightVisibility; task 6b
chose and fetched them) and the spec's Blender-exported FBX and USD of a lit, camera'd
scene (Ruling R28): Blender 5.2.0 LTS, read-only and headless
(`harness/blender-export.py`, a local script in the same git-ignored ledger), imported
our `staged` GLB with its glTF importer and
exported `staged_blender.fbx` and `staged_blender.usda` with lights, cameras and the
clip. Blender's USD exporter writes no orthographic camera, and its stage is Z-up. Each
model is its own matrix: 798 of 798 cases pass, no native file differs, and every
camera and light pairs; LightVisibility is refused by name, as it must be (glTF
`extensionsRequired` `KHR_node_visibility`). Every skip carries its citation.

**Controls** (made by `harness/make-controls.py`, and the native one by
`harness/make-native-control.py`: local scripts in item 5's git-ignored ledger,
`.superpowers/sdd/2026-11-04-mesh-lights-cameras/`, never committed): each output
altered as a writer bug would alter it fails its judge:

| control | fails |
|---|---|
| a GLB's first camera at yfov 0.9 | `camera`, 9.62–25.80 %; the oracle's yfov |
| a GLB's sun recoloured | `lit_lights`, 85.76–100 %; the oracle's colour × intensity |
| a Collada spot's `<falloff_angle>` written as the half-angle (the old R4), where Ruling R23 writes the full cone | the oracle's cones (outer 0.6 → 0.3); SceneKit's footprint, 49.77–53.93 % |
| a Collada sun turned to the identity | the oracle's direction, 53.343° apart; SceneKit's footprint, 84.41–97.35 % |
| a Collada camera at a yfov of 55° | the oracle's yfov; SceneKit's camera, 13.79–31.91 % |
| a USDA camera's `verticalAperture` 0.24 → 0.3 | the USD camera check's yfov and aspect; SceneKit's camera, 16.16–33.76 %; usdrecord's picture 384 × 320 |
| a USDA sun recoloured | usdrecord's colour, 100 % |
| a USDA bulb moved | usdrecord's footprint, 12.46 % in 5 of 7 frames (in 2 the moved bulb lights the same surfaces) |
| a GLB's near plane 0.125 → 2 | the oracle's znear alone: no picture moves |
| a USDA's `clippingRange` (0.125, 64) → (2, 64) | the USD camera check's znear alone: no picture moves |
| a USDA's second camera moved from (−3, 2.5, 3) to (3, 0.5, −3) and unturned | the USD camera check's eye, forward (50.238° apart) and up (25.239°) on that camera, at rest and at every sample time: no picture draws through it |
| a GLB's second, orthographic camera at xmag 4 | the oracle's xmag (3 → 4, read from the file's own JSON) and aspect (1.5 → 2): no picture draws through it |
| a Collada bulb moved from (−1.5, 2, 1) to (1.5, 2, −2) | the oracle's position; SceneKit's footprint (drawn with no falloff), 12.94 % in 5 of 7 frames |
| Khronos DirectionalLight's Collada sun `<color>` (0.9, 0.8, 0.1) → (0.1, 0.8, 0.9) | the oracle's colour × intensity; SceneKit's colour (against the sRGB decode), 100 % |
| our reading of a glTF original (`reference.glb`) with its camera at yfov 0.9 and its sun recoloured, named or unnamed | the native `camera`, 9.62–25.80 %, and `lit_lights`, 85.76–100 % |
| Khronos Duck's `reference.glb` (our reading) with its unnamed camera moved from node 2 to node 1 (a JSON edit, `edit-glb-json.mjs move-camera 2 1`, a local script in the followups ledger; followups 2026-11-08, item A) | the native check: "the original has [node 2:perspective], our reading [node 1:perspective]; camlight camera@static: our reading has no camera on a node named node 2". The rank pairing does not pair a camera with another node's |
| VirtualCity's `scene → dae` with the `<yfov>` of its two lone-`<yfov>` cameras 57.30° → 40° (a one-line edit of `src/dae/model.dae`; item B) | the oracle's check of the file's own `<yfov>`, 1 → 0.6981 rad, on both cameras; SceneKit's `camera` is 0 % apart at every time, so no picture would catch it |
| Duck's `scene → usda` (explicit aspect 1.5) with its camera's `horizontalAperture` 0.36 → 0.4267, 16:9 over the 0.24 `verticalAperture` (item C) | the USD camera check's aspect, 1.5 → 1.7778, and usdrecord's picture "384x215, not 384x256" |
| VirtualCity's `scene → usda` (no aspect, expected at 16:9) with its camera's `horizontalAperture` 0.4267 → 0.32, 4:3 (item C) | usdrecord's picture "455x341, not 455x256" at every time |

The code-level control, the spec's (task 6a, 2026-11-06, run again on the final code of
each of its fix rounds and its follow-up round): with the FBX camera turn dropped in the writer (`fbx.rs`,
`turn_of`), the oracle's forward check fails by 90.0000° on both of `staged`'s cameras
written from the scene, at rest and at all six sample times (180° through fbx → fbx,
whose reader undoes a turn that was never written), and the FBX `camera` judge differs
on 100 % of the pixels in every frame while its `lit_lights` stays at 0.00 %; with the
file restored, all agree again.

**The native check, in `camera` and `lit_lights`** (item 5's Ruling R24; the spec: "The
native check runs in camera and lit mode"). A glTF original with a camera or a light on
either side, as GLTFLoader builds them, is drawn with our reading of it
(`renderNativeCamlight`): in `camera`, each through its own camera (ours named like the
original's first, at the original camera's aspect), and in `lit_lights`, each lit by its
own lights through the fixture's camera, at rest and at the sample times; both are
judged at 0.00 % (the unlit and the lit tolerance), and the two sides' cameras and lights
must match by name and kind. As in the lit pass, the one excuse is a repeat with the
original's rotations put back that closes every difference, no rotation moved past
`LIT_ROTATION_BOUND`.

### Points and lines (mesh-gaps item 10)

The `wire` fixture is a unit cube, a four-segment coloured ring (a line loop, under an
animated node: `Swing`, 7 frames) and a 3 x 3 grid of coloured points. The page draws a
loader's points as `THREE.Points` (`PointsMaterial`, 4 px, `sizeAttenuation: false`) and
its lines as `THREE.LineSegments` (`LineBasicMaterial`), in every mode, with materials of
its own (`drawables`, `wireMaterial` in `page/render.js`), so the expected picture
(GLTFLoader's) and a loader's own (OBJLoader draws a point at 10 px, with an MTL) agree:

- `geom`, `normals`, `uv` and the `camera` modes: one fixed colour.
- `albedo`: the source material's colour, its `map` and its vertex colours. A points
  `map` is sampled through the geometry's `uv` attribute where it has one (`USE_POINTS_UV`,
  three r183 `WebGLPrograms.js:300`, `points.glsl.js:12-23`), so a textured points
  primitive shows its texture.
- `cover`, `camera_cover`: white. `wire_cover_points` and `wire_cover_lines` are white
  coverage of one kind alone, kept for the wire judges below.
- The `light` modes and `lit_coat` leave points and lines out (nothing there lights them).
- A PLY geometry with no index draws as `THREE.Points` (PLYLoader sets an index only when
  it read a face, `PLYLoader.js:419-421`); a PLY with an `edge` element and no face draws
  nothing, as PLYLoader ignores the edges (`PLYLoader.js:480-561`).

Each format is judged by its own three.js loader where that loader builds the primitive,
and where it cannot, that comparison is a **skip that cites the loader's source line**
(`WIRE_*` in `verify.mjs`; `report.summary` counts them as `wireSkipped`, with `wireSkips`
split by judge, and the run's last line ends "; N point/line comparison(s) skipped with a
citation (a three.js, b SceneKit)", after any native-check skips, and, when a PLY with no
faces had its assimp judge skipped (below), with a note of its own: "; N assimp validation(s)
skipped with a citation (a PLY with no faces)", counted apart in `validationSkipped`). The
triangles stay compared whole: the expected picture is drawn a second
time as the reader would draw it, without the lines' colours (`#linecolors`), without the
lines (`#lines`) or without the points and lines (`#wire`).

| target | three.js | SceneKit | usdrecord |
|---|---|---|---|
| glb, gltf | compared, as any mode | not a SceneKit format | |
| obj | points and lines compared; `l` carries no vertex colour in OBJLoader (`OBJLoader.js:400`, `:406-425`), so the albedo is compared with `#linecolors` (skip) | skipped: Model I/O reads neither `p` nor `l` (research Q6, `research-consumers.md:377`, and the Model I/O row `:378`, measured on macOS 26.6.2) | |
| dae | `<lines>` compared | `wire` compared | |
| ply | points compared (a vertex-only file); lines skipped, `#lines` (`PLYLoader.js:480-561`) | lines skipped (`research-consumers.md:377`); points `wire` | |
| usda, usdz | skipped, `#wire` (`USDComposer.js:621-676`, the Mesh branch `:650`, the generic one `:663-673`) | BasisCurves `wire` compared; Points skipped (`research-consumers.md:377`) | `wire_points`, `wire_lines` |
| fbx, 3mf, stl | the expected scene keeps neither | | |

A point or a line cannot be compared pixel for pixel across renderers (Storm draws a point
at its world-space `widths`, SceneKit at its own size), so the **wire judge**
(`window.compareWire`, `WIRE_LIMIT`) compares a picture of the points and lines alone with
the expected `wire_cover_points` or `wire_cover_lines` picture, on the bounding box and the
centroid of the covered pixels: each of the six numbers within 1 % of the frame (2.56 px at
256). Points and lines are drawn apart (usdrecord deactivates the Mesh prims and the other
kind), because the two renderers weigh them differently (the expected picture covers 144
pixels with points and about 235 with lines; Storm's 1024 px picture, 53 and about 1,650)
and the centroid of both together read 35 to 47 px off, measured.

**How the points are measured** (I10-R20): a renderer's dot size is its own (4 px squares
here, round points in SceneKit, a pixel or two in Storm), so which method judges a points
picture depends on whether the expected picture's points can be told apart at 256 px:

- **Separable: the expected picture has as many connected blobs (8-connectivity) as the file
  has point vertices**, so no two points touch. This is the rule and its threshold: blobs ==
  point vertices (`stats.pointVertices`), nothing fixed. The points are then counted (a
  point lost or merged fails) and the centroid is the mean of the blobs' own centres. The
  `wire` fixture's 3 x 3 grid is this case: 9 blobs for 9 vertices.
- **Dense: fewer blobs than points** (they run together). A centroid of covered pixels
  weighs each place by dot size, and one dot size is not another's: on Stanford's drill
  (4,223 points in 2 blobs of 5,130 and 3,314 px in the expected picture;
  `drill_1.6mm_0_cyb.ply`, run by task 6b)
  the pixel centroid was 4.17 px off in SceneKit's picture and 5.72 px in usdrecord's, past
  the 2.56 px limit, while the boxes agreed within 1.75 px and three.js's PLYLoader agreed
  with our reading at 0.00 %. The covered-pixels centroid is the spec's own wording, and it
  does not survive a change of dot size: on a synthetic uneven cloud drawn twice in the same
  places, as 4 px squares and as 1 px dots, it reads 6.5 to 10.9 px apart (a covered-cell
  centroid, 4 px or 8 px cells, 0.9 to 3.1 px). So the dense judge keeps the box of covered
  pixels and takes the centroid of the **convex hull** of the covered pixels' centres, which a
  dot's size moves by half a dot at most: the same pair reads 0.08 to 0.18 px, and a cloud
  moved by 2, 4 and 8 px reads 2.18, 4.18 and 8.05. On the drill: hull centroids 0.03 / 0.19
  px apart in usdrecord's picture and 0.01 / 0 px in SceneKit's; the boxes, which a dot's
  size moves too, within 1.38 and 1 px (expected pixel centroid (131.33, 136.13),
  usdrecord's (125.61, 140.88)). Where either picture's covered pixel centres have no hull
  (fewer than three, or all in a line), neither side's centroid is compared: both report
  `method: 'box'` and `cannotJudge`, and the box decides, with the two content checks below,
  which still run.
  The hull sees a cloud's **outline**, not its contents: a band of the drill erased from the
  middle of its length, or an interior block, moves it by nothing a pixel can show (the dot
  size's 1.13 px). So a dense cloud also passes two content checks (`denseChecks`, strict: 0
  of each):
  - *precision*: every covered pixel of ours lies within 1 px (the expected picture's
    pixels) of a covered pixel of the expected picture. A smaller dot cannot fail it (it lies
    inside the 4 px square the expected picture draws); a larger one can, by 1 px a side: a 6 px
    dot passes and an 8 px dot fails, which is the point (neither SceneKit's round points nor
    Storm's did, measured).
  - *coarse occupancy*: the covered 8 px cells of each picture, with a tolerance of one cell:
    `missing` counts the expected picture's cells with no covered cell near them in ours
    (content lost), `extra` ours with none near them in the expected picture (content invented).
  They run in the dense branch only. The 3 x 3 grid is blob-counted: a point lost fails the
  count, one moved fails the centroid of centres and the box. Measured on the drill, in all
  15 usdrecord frames (1024 px) and 7 SceneKit frames and the native check's: precision 0 of
  32,518 covered pixels outside for usdrecord and 0 of 6,325 for SceneKit, occupancy 0 of 178
  cells missing and 0 extra. Their reach, from `dense-checks.mjs` (render.js's own functions
  on a cloud made of the drill's real vertices, projected to 256 px: 10,051 covered pixels,
  206 cells; run `node dense-checks.mjs`) and `stat.mjs` (compares the candidate centroids:
  pixels, cells, hull), both in `.superpowers/sdd/2026-11-04-mesh-points-lines/harness/`, the
  git-ignored ledger:
  the same cloud as 1 px dots (77,692 pixels at 1024) or as round 4 px dots passes; the last
  12 % of its length erased fails occupancy (17 of 206 cells missing) and the box; a band from
  30 to 40 % and one from 45 to 60 % of its length erased fail occupancy only (5 and 3 cells
  missing, the hull and box unmoved); an interior block of 40 and 60 px erased fails it (6 and
  12 cells), of 24 px does not (a hole under about three cells, a ninth of the frame's width,
  is not seen); a cloud moved by 3 and 8 px fails precision (249 and 6,792 pixels outside) as well as
  the box and hull; a stray point 20 px outside the outline fails the box, precision (20
  pixels outside) and occupancy (1 extra cell); every third point dropped passes, as no
  coverage check sees density.
  The report's `want.method` and `got.method` say which one judged (`blobs`, `hull`, `box`,
  or `pixels` for lines); a dense cloud's `want` also holds `blobsSeen` and `points`, and its
  `got` the `checks`.
- **The switch is a cliff.** A single pair of touching points in an otherwise sparse cloud
  makes the expected picture's blobs fewer than its points, and the whole judge goes from
  exact counting to the outline and the content checks. Keep a file that must be counted
  with its points apart by more than a dot (4 px at 256 px).

**Who judges a cloud's contents, by format.** Not three.js everywhere:

| output | what judges the points |
|---|---|
| glb, gltf, obj | three.js's own loader, pixel for pixel against the expected GLB (`geom`, `albedo`) |
| ply | three.js (PLYLoader, drawn as `THREE.Points`, pixel for pixel) and SceneKit's `wire` (the judge above) |
| usda, usdz, and a USD original in the native check | usdrecord's `wire_points` alone: counting, or the judge above. three.js and SceneKit both drop USD Points, so nothing else sees them in a picture; `usdchecker` judges the file and the Rust round trips read the points back |
| dae, fbx, 3mf, stl | refuse a scene of points by name (`namedRefusal`), as they hold none |

usdrecord draws the wire passes at 1024 px (`WIRE_USD_WIDTH`). litegen writes `widths` as
0.25 % of the scene's diagonal, a little over half a pixel on `wire` at 256 px, and Storm
covers a pixel by quarters (alpha 0/64/128/191/255): measured 2026-11-08, at 256 px it drew
8 of the 9 grid points, one of them as two pixels of alpha 64, and the centroid was 8.61 px
off; at 1024 px all nine are drawn. The `cover` and `albedo` passes deactivate the Points and
BasisCurves and compare with the `#wire` picture, so the triangles are judged as before.

Measured on the 2026-11-08 run (`--fixtures wire,cube,textured`, 342 cases): before this
work 299 passed and **43 failed**, all of them `wire` (obj 9, ply 8, dae 9, usdz 8, usda 9);
after it **342 pass**, `wire` among them (114 of 114), with 64 point/line comparisons
skipped with a citation (three.js: obj 9, ply 8, usdz 8, usda 9; SceneKit: obj 9, ply 8,
usdz 6, usda 7). usdrecord's points are at most 1.63 px from the expected picture's and its
lines 1.13 px (the limit is 2.56); SceneKit's `wire` pictures of Collada and USD lines are at
most 1 px.

**assimp and a point cloud** (I10-R20). `assimp info` runs ValidateDataStructure, which rejects
a mesh with no faces ("Mesh contains no faces", `code/PostProcessing/ValidateDataStructure.cpp`
L342-343 at v6.0.5; research Q6, `research-consumers.md:382`, the assimp row: the step is on in
`assimp info`'s default preset and `-fd` replaces the preset), and its PLY importer makes a
vertex-only file exactly such a mesh. So the assimp judge of a PLY with no faces is a cited skip
(`WIRE_ASSIMP_NO_FACES`), and the file is judged by the other readers. It is a *validation*
skip, not a point or line comparison skipped: it is counted in the report's `validationSkipped`
(and `wireSkips` holds only `three` and `scenekit`), the console line says "assimp
validation(s) skipped", and it applies only when the expected scene has no triangles
(`expectedStats(...).triangles === 0`), so a PLY that has faces still gets its assimp judge.
**A scene a format cannot hold is a refusal, not a failure:** a points-only scene
written as stl, 3mf, dae or fbx is refused by name by the writer ("unsupported mesh feature: stl
holds triangles only; this scene has 4223 points and no triangles"), and the expected scene,
reduced to what the format holds, is empty ("mesh contains no triangles, lines or points");
`verify.mjs` counts the two sentences, and a conversion from such a format ("no stl source was
written"), as one refusal (`namedRefusal`, whose format word is pinned to the case's own target
`to`, or source `from`, so a refusal that names another format is a failure; a refusal with any
other pair of messages is still a failure).

**SceneKit and a vertex-only PLY** (the release harness run, tree 64bb3cc). Model I/O gives a PLY with vertices and no faces a
point element of **three indices per vertex**, 0 to 3n - 1, and the 2n that name no vertex read
past the vertex source: SceneKit draws them at the origin, a point the file does not hold.
Measured 2026-11-08 on macOS 26.6.2 (`SCNScene(url:)`, then the first geometry's element): 1, 2,
4, 9, 50, 500 and 4,223 vertices read 3, 6, 12, 27, 150, 1,500 and 12,669 indices. The release
run's `import-tex` (4 points at the corners of a square, centred on the origin) failed 7 cases
(every X to ply: "it draws 5 points where the expected picture has 4"): SceneKit's picture was
five separate 2 x 2 dots, the four points and a fifth at the frame centre (127.5, 127.5), which
is where the origin lies, 60 px or more from any of them. A cluster split by rasterisation it
is not, and a gap merge in the blob counting (a dilation, or 8-connectivity plus a 1 px gap)
could not join it; the blob rule is unchanged. `scenekit/render.swift` (`keepingElements`) now
keeps the indices that name a vertex: the same picture has 4 blobs of 4 pixels, the same four
places, and no dot at the centre. Until then the dense clouds (the drill, `cloud.ply`) had
passed with the stray: precision counted 0 pixels outside, so it fell on the cloud or out of
the frame; only a sparse cloud, counted, could show it. The nearest two points of the `wire`
grid are 132 px apart in usdrecord's 1024 px frame (about 33 px at 256, against a 4 px dot), so
no gap merge of a pixel or two could join them either. After it, the release matrices: `import-tex` 45
pass, 69 refused, 0 fail (the seven cases now read 64 px / 4 blobs against 16 px / 4 blobs, box within
1 px, centroid 0.25 px), every fixture 1140 of 1140, `import` 501 pass and 69 refused of 570 (0 fail),
`import-modes` 228 of 228, `import-drill` 45 pass and 69 refused (0 fail), `import-usd` 114 of 114.

Stanford's drill, before and after (task 6b's `--import`, 114 cases; before = `report-drill`,
after = `report-drill6`, the final one, both under
`.superpowers/sdd/2026-11-04-mesh-points-lines/harness/`, a git-ignored ledger): before, 23
pass, 22 fail on a point judge or on assimp, 1 native check (SceneKit's `wire`) differs and 69
refusals are reported as failures; after, **45 pass, 69 refused by name, 0 fail, native check
ok**, with 38 point comparisons skipped with a citation (three.js 15, SceneKit 23) and 7 assimp
validations skipped (`validationSkipped`). The 23 dense judgements (15 usdrecord frames, 7
SceneKit frames, the native check's): precision 0 pixels outside (32,518 covered for usdrecord,
6,325 for SceneKit), occupancy 0 of 178 cells missing, 0 extra.

On the regression sets the dense branch runs once: `matrix-import`'s 500-point cloud in the
native SceneKit check (1,914 covered pixels, 255 cells; 0 outside, 0 missing, 0 extra). The
`wire` fixture's grid, `matrix-usd`, `matrix-tex2` and the MeshPrimitiveModes set never reach it
(blob-counted or lines), so the checks cannot fail them: `--fixtures wire,cube,textured` 342 of
342, `matrix-import` 10 of 10, `matrix-usd` 114 of 114, `matrix-tex2` 14 of 14,
`matrix-modes` 228 of 228.

The native check handles points and lines the same way (`wirePlan` for the original's
format, the reading drawn through `#linecolors`, `#lines` or `#wire` where its own loader
cannot draw them, `check.wire` holding the citation; SceneKit's triangle comparisons against
`<key>#wire`; a USD original's Points and curves through usdrecord's `wire_points` and
`wire_lines` passes). Checked on a synthetic big-endian point-cloud PLY with a `range_grid`
element (the shape of Stanford's `drill_1.6mm_0_cyb.ply`) and on the `wire` fixture's own
obj, ply, dae and usda files used as originals; the real files are the `--import` set's.

**Negative control.** Writing OBJ `l` indices one too high (`obj.rs`, the `Topology::Lines`
writer) fails 8 of the 99 OBJ unit tests (`wire_round_trips_through_obj` among them) and
the harness's three OBJ `wire` cases (`scene`, `single` and `obj` to obj: geom and albedo
3.73 % apart, and the obj to obj read refused, "vertex index 29 is out of range for 28
entries"). `obj_reads_p_and_l` parses a hand-written file, not the writer's, so it does not
fail; the writer's round trips are the tests that do.

### KTX2 originals (mesh-gaps item 3)

A glTF original whose images are KTX2 (`KHR_texture_basisu`, ETC1S/BasisLZ or UASTC, with or
without Zstd) is drawn by `GLTFLoader` with three r183's `KTX2Loader` (`page/render.js`:
`setTranscoderPath('/three/examples/jsm/libs/basis/')`, `detectSupport(renderer)`), Binomial's
own transcoder, which shares nothing with litegen's `mesh/ktx2.rs`. Our side is the reading
`mesh_matrix --import … --transcode-textures` makes of it: the PNG that production writes.
`stats.ktx2` (textures, GPU formats, support flags) goes to `report.json` under the native
check's `ktx2`.

The two sides can never be the same bytes. `KTX2Loader` hands the GPU a compressed format
(on this machine ASTC 4x4 for UASTC, ETC2 for ETC1S) with the file's own mip levels, and our
PNG is uncompressed with mips the GPU builds. So the check has four parts, strictest first:

- **The original against itself**, loaded twice (geom and albedo): 0.00 %, or the harness
  is not deterministic and the run fails.
- **The `lit` pass through `KTX2Loader`'s RGBA32 fallback** (`ktx2Rgba32` in `render.js`: a
  loader whose `workerConfig` offers no compressed format, so the transcoder writes its own
  RGBA8). Litegen's decoder reproduces that byte for byte (the unit test
  `mesh::ktx2::tests::every_reference_texture_transcodes_bit_exactly`), so this is judged like
  any `lit` pass: no pixel more than 1/255 (`LIT_TOLERANCE`) in a channel from the original's
  (`THRESHOLD.lit` 0), and the largest channel difference measured is recorded
  (`nativeCheck.lit.maxDelta`: 0, and 1 on ChronographWatch). The same picture through the
  GPU format is recorded as `ktx2.litGpu` (pixels, maximum delta), not judged.
- **Albedo from level 0 alone** (`baseLevelOnly`: every texture `LinearFilter`, no mips),
  once in the GPU format (`ktx2.level0`, limit `KTX2_NATIVE_LEVEL0` 0.1 % of pixels past a
  channel difference of 20) and once through the RGBA32 fallback (`ktx2.rgba32`, ratio 0 at
  channel tolerance `KTX2_RGBA32_TOLERANCE` 0: not one value differs, and `maxDelta` is
  recorded beside it, 0 on all six). Content and format are all that can differ here.
  `baseLevelOnly` also gives a KTX2 texture the colour space of its slot, because
  `KTX2Loader` tags a texture with the file's own colour space (an ORM map is often tagged
  sRGB) and `GLTFLoader` corrects only four slots; every measured model came out the same
  either way, so this is a guard.
- **Albedo with mips** (the ordinary native albedo comparison), limit `KTX2_NATIVE_ALBEDO`
  10 % instead of 5 %.

**Calibration.** Method: each model alone through `mesh_matrix --fixtures none --import
<dir> --transcode-textures`, then `verify.mjs`, reading `nativeCheck.ktx2` and
`nativeCheck.albedo` from `report.json`. The original against itself is the control (0 on
every model); the original against our PNG reading, with mips, gives the albedo limit
(the maximum, rounded up to a whole percent); with level 0 alone, the other limits.
This Mac's Chromium (macOS 26.6.2, arm64, Playwright 1.60, three 0.183.2, headless: ASTC,
BPTC, S3TC, ETC1 and ETC2, no PVRTC), 2026-11-08; **the numbers are this machine's**, and a GPU
that offers other formats (BC7 where this one has ASTC) will move the with-mips and GPU
columns: on a new machine, run the six imports again and read these columns before trusting
the two GPU-format limits. The RGBA32 columns do not depend on the GPU. Pixels are "past
the pass's tolerance" (20 per channel for the GPU-format albedo columns, 0 for the RGBA32
level-0 column, 1 for `lit`) over the pixels either
picture covers; "max" is the largest channel difference (0 to 255) of any covered pixel.

| model | textures | GPU formats | self | albedo, mips | level 0, GPU (pixels, max) | level 0, RGBA32 (pixels, max) | lit, GPU (pixels, max) | lit, RGBA32 (pixels, max) |
|---|---:|---|---:|---:|---|---|---|---|
| `OwnTextured` (`textured.basisu.glb`) | 1 | ETC2 RGB | 0 | 0.0682 % | 0 / 11,725, 0 | 0 / 11,725, 0 | not in `lit` | not in `lit` |
| FlightHelmet | 15 | ASTC 10, ETC2 RGB 5 | 0 | 0.4642 % | 0 / 14,218, 0 | 0 / 14,218, 0 | 9 / 14,218, 3 | 0 / 14,218, 0 |
| ChronographWatch | 12 | ETC2 RGB 11, ASTC 1 | 0 | 0.0445 % | 0 / 17,959, 0 | 0 / 17,959, 0 | 133 / 17,959, 11 | 0 / 17,959, **1** |
| StainedGlassLamp | 19 | ASTC 13, ETC2 RGB 5, ETC2 RGBA 1 | 0 | 3.3012 % | 0 / 8,300, 1 | 0 / 8,300, 0 | 12 / 8,295, 5 | 0 / 8,295, 0 |
| AnisotropyBarnLamp | 4 | ASTC 4 | 0 | **9.5445 %** | 0 / 9,901, 1 | 0 / 9,901, 0 | 268 / 9,901, 16 | 0 / 9,901, 0 |
| CarConcept | 35 | ETC2 RGB 23, ASTC 12 | 0 | 0 | 0 / 13,279, 0 | 0 / 13,279, 0 | 50 / 13,279, 7 | 0 / 13,279, 0 |

The with-mips figure is the original's own mip chain against the GPU's, not a decoding
error: with level 0 alone the same pictures are 0 pixels apart. Through the RGBA32 fallback
the albedo values themselves are equal (maximum difference 0 on all six, which is why that
pass is judged at channel tolerance 0), and `lit` is within one channel step (0, and 1 on
ChronographWatch, which is its judge's own tolerance). The GPU-format level-0 pair is 0
pixels past 20 everywhere, with a largest difference of 1 (ASTC's rounding), so its limit
is 0.1 % of pixels: nothing measured, and well under the negative control's smallest fault,
0.824 % (control below). A decoding fault shows in these columns at once, not in the
with-mips one.

**The layer-extension check (Ruling A20)** compares a glTF original's material fields with
the glTF litegen wrote. A KTX2 image becomes a PNG by design, so its bytes differ and a byte
hash fails. `pixel-digest.mjs` reads both: a KTX2 image by Binomial's transcoder to RGBA32
(level 0, the call `KTX2Loader`'s fallback makes), a PNG by an inflate and unfilter of its
own (8-bit, not interlaced; grey, grey and alpha, RGB, RGBA and palette), and
`layer-extensions.mjs` accepts the pair only when the RGBA8 digests are equal, listing it as
`ktx2Pixels` in `report.json` (an information line, not a difference). One changed pixel, or
a pair that is not one KTX2 and one PNG, is still `texture-image`. `pixel-digest.test.mjs`
runs this on committed bytes (`node --test scripts/mesh-verify/pixel-digest.test.mjs`).

**The import set** is local, never committed (the models' licences are theirs):
`.superpowers/sdd/2026-11-04-mesh-ktx2-textures/import/`, one folder per model with
`SHA256SUMS`, `fetch.json` and the licence files. Khronos `glTF-Sample-Assets` at commit
`8f492390ddc7a2ae221a1369ef560589cf8c70fb`, the `glTF-KTX-BasisU` variant of FlightHelmet
(CC0 1.0), ChronographWatch, StainedGlassLamp and AnisotropyBarnLamp (CC BY 4.0), and the
`glTF-KTX-BasisU-Draco` variant of CarConcept (CC BY 4.0, Draco and KTX2 in one file), and
our own `litegen-core/src/mesh/testdata/gltf/ktx2/textured.basisu.glb`.

**Results** (2026-11-08, release builds, this machine; each import through `--fixtures none
--import <model> --transcode-textures`, 114 cases a model, one `verify.mjs` at a time):

| set | result |
|---|---|
| every fixture (1,140 cases), the controller's release binary from before the flag existed | **1,140 of 1,140**; all 12,549 ratios equal to item 10's last run's (`compare-ratios.mjs`, worst difference 0). The binary writes the same bytes as the flag build without the flag (`diff -r` of an import's matrix), and as the K21 build for the 1,140 fixtures (`diff -r` of the two full matrices: 0 files differ). This run has no native checks: a non-KTX2 original's is the `--import` sets' |
| `OwnTextured` (`textured.basisu.glb`) | 114 of 114, native ok |
| FlightHelmet | 114 of 114, native ok |
| ChronographWatch | 114 of 114, native ok |
| AnisotropyBarnLamp | 114 of 114, native ok; 14 layer textures KTX2 in the original and PNG here, equal pixel for pixel |
| CarConcept (Draco and KTX2) | 114 of 114, native ok |
| StainedGlassLamp | 114 of 114, native ok; 14 layer textures equal pixel for pixel (112 of 114 before ruling K21, below) |

Every native check is ok: geom 0.00 % on all six, albedo as in the calibration table, `lit`
0 pixels past the 1/255 tolerance through the RGBA32 fallback on the five that have a layer.
The release binary for the imports was rebuilt after K21, and every import regenerated and
verified again.

**StainedGlassLamp's two OBJ failures (ruling K21, fixed).** `obj -> glb` and `obj -> gltf`
failed `lit` at 0.30 % (25 of 8,300 pixels, maximum delta 221). `obj::prepare` (the scene as
the OBJ reads back, which is the expected picture's scene) kept `alphaMode` `MASK` on `grill`
and `stainedglass`, while OBJ has no alpha test (`obj::losses`: "each draws as wholly opaque
or not at all") and its reader gives such a material back OPAQUE. Where the base-colour
texture's alpha is below the cutoff, GLTFLoader cut the expected picture out and drew ours.
Measured before the fix: the geometry of all eight primitives bit for bit equal; the model's
own PNG reading (`reference.glb`, no KTX2 left in it), imported as an original with
`--formats glb,obj`, failed the same case at the same 0.30 %; and with those two materials
made `OPAQUE` in that copy all 7 cases passed. The fix is in `mtl_alpha_and_sides`
(`obj.rs`): a MASK is prepared as the MTL's `d` says it, OPAQUE where its colour's alpha is at
or above the cutoff and a BLEND of alpha 0 where it is below ("not at all"). The OBJ bytes do
not move (`src/` and `conv/` of a StainedGlassLamp matrix, `--formats obj`, are identical
under the binaries before and after; only the two OBJ expected scenes change), and the test
`a_mask_is_prepared_as_the_wholly_opaque_or_absent_material_an_mtl_reads_back` was red on
`prepare` before the change.

**Negative control.** Reversing the bit order of an ETC1S selector (`selector_at` in
`mesh/ktx2.rs`, `row >> (2 * (3 - x))` for `row >> (2 * x)`) fails the library's own
`every_reference_texture_transcodes_bit_exactly` on all three ETC1S inputs and on none of
the three UASTC ones (FlightHelmet's lens map 153,628 of 1,048,576 pixels, `Dot_C` 458 of
16,384, the StainedGlassLamp grill's 245,092 of 1,048,576). A release `mesh_matrix` built
with it, through the harness as it was in the first round (the pairs counted pixels past a
channel difference of 20, and the GPU-format level-0 limit was 1 %):

| model | what failed | what did not |
|---|---|---|
| `OwnTextured` | albedo from level 0 alone, 2.217 % (260 of 11,725 pixels), in the GPU format (limit then 1 %) and in RGBA32 (limit 0) | albedo with mips, 0.0853 % (10 % limit) |
| ChronographWatch | RGBA32 level 0, 0.824 % (limit 0); `lit` through RGBA32, 19.81 % (3,557 of 17,959 pixels, maximum delta 212, limit 0) | albedo with mips, 0.0445 %; GPU-format level 0, 0.824 % (limit then 1 %) |

The GPU-format level-0 pair equalled the RGBA32 pair in both control runs (2.217 %, 0.824 %),
so lowering that limit to 0.1 % means it now fails the 0.824 % that passed (by the number: no
new mutant was built). The RGBA32 level-0 pair is now judged with no value different at all,
and none was in the six good runs. The with-mips albedo (10 %) still passes the control
(0.0853 % and 0.0445 %), so the level-0 and `lit` passes carry the proof. Restoring the line (an edit of that line, then `touch`) gave the file back byte for
byte, and the 106 `mesh::ktx2` tests pass.

### Collada morphs: the `dae-keep` variant (mesh-gaps item 8)

litegen's Collada writer bakes morph targets by default, because three.js and assimp fail the
whole file on any `<morph>`. Under `dae_morphs: keep` (`WriteOptions.dae_morphs`) it writes
them: NORMALIZED morph controllers, weight animation and skin over morph, in the form a
SceneKit spike measured playing (`docs/superpowers/3d-format-conversion.md` §7).
`mesh_matrix --dae-keep` puts that output through the harness. Ruling IDs "DM3", "DM6", "DM7",
"DM10" and "DM15" are this item's controller rulings (`.superpowers/sdd/2026-11-04-mesh-dae-morph-output/`).

**The cases.** With the flag, and `dae` among the formats, every fixture gets one more case for
each direction into Collada, its file written with `DaeMorphs::Keep` and named `dae-keep`:

| directory | case | expected scene |
|---|---|---|
| `<fixture>/src/dae-keep/` | `scene → dae-keep` | `dae::prepare` of the fixture, under `Keep` |
| `<fixture>/src_single/dae-keep/` | `single → dae-keep` (self-contained) | the same, self-contained |
| `<fixture>/conv/<from>__dae-keep/` | `<from> → dae-keep`, for each format in the run | `<from>`'s `prepare`, then `dae::prepare` under `Keep` |

A `dae-keep` file is never read back, so there is no `dae-keep → X`, and `dae → dae-keep`
starts from the plain `src/dae`, which is already baked. In the manifest `to` is `"dae-keep"`.
A plain and a kept case share `from` and, for every judge, the format, so a name of their own
keeps their picture folders (`img/<fixture>/<from>__dae-keep/`), the report page's column and
the tools that read the manifest apart: the standalone `camlight-oracle.mjs` skips a `to` it
does not know, so it never hands assimp a file that assimp must refuse. `verify.mjs` reads
such a case as `to: 'dae'` with `variant: 'dae-keep'` for every judge, SceneKit's at rest and
at every sample time included, and `report.json` keeps `to: "dae-keep"`. Without the flag the
generator's output is byte-identical to before, case counts included (2026-11-09: morph,
skinned and staged in seven formats, 177 cases, `diff -r` against the output of the binary
that preceded the flag: no difference).

**Who must refuse what** (`dae-keep.mjs`). The rule is the file's, not the case's name: a file
that holds a `<morph` is expected to be refused, and a fixture without targets writes the
file `bake` does, an ordinary Collada file that every judge treats as `dae` and none refuses.

- **three.js `ColladaLoader`** must fail to load it. The page's error must be one of the three
  TypeErrors measured on r183 (0.183.2), which `THREE_KEEP_CITE` cites: a static morph fails in
  `ColladaComposer.buildNode` reading `controller.skin.joints` (`ColladaComposer.js:2353`;
  `ColladaParser.parseController` stored the morph with no skin, `ColladaParser.js:445-448`), a
  static skin over a morph in `buildController` reading `geometry.sources` of the morph
  controller's id (`ColladaComposer.js:961-969`), and a morph with a weight animation earlier,
  in `ColladaParser.parseAnimationChannel`: its channel target is `<morph id>-weights(i)`, the
  form SceneKit plays, which has no `/`, so `sid` is undefined and `sid.indexOf` throws
  (`ColladaParser.js:347-354`). The animated one is the case for every fixture with a clip.
  A load that succeeds fails the case, "documented reader limitation no longer holds", so the
  docs get updated; a different error fails it too. Nothing three.js drew is compared.
- **assimp** (`assimp info`) must refuse it with `Unable to resolve library reference ""`
  (`assimp` v6.0.5: `ColladaParser::ReadController`'s `<targets>` loop advances `currentNode`
  where it means `currentChildNode`, `ColladaParser.cpp:902-914`, the loop at `:903`, so
  `mMorphTarget` stays empty; `ColladaLoader.cpp:685` resolves it; the message is
  `ColladaParser.h:309-312`). Read, or refused otherwise, it fails the case. The research found
  assimp master the same, so no later release is used.
- **assimp's camera and light oracle** (`camlight-oracle.mjs`) cannot read such a file, and is
  skipped for it with that citation; SceneKit judges its cameras and lights.
- **SceneKit** is the only judge of a `<morph>` file's geometry: its silhouette, albedo and
  uv pictures at rest and at every sample time, within today's thresholds. The silhouette must
  be judged at rest and at **every** sample time (Ruling DM18, `keepSilhouetteVerdict`): no
  SceneKit, a skipped frame (a clip SceneKit does not register, `notPlayed`: its name is a
  node, geometry or animation id), a frame that errored or one never rendered fails the case,
  and the reason names the frames (`rest`, `t=0.5`). So a `--dae-keep` run needs macOS.
  Without the flag a run is as before everywhere.
  - **One exception, cited and counted (Ruling DM21).** When clips animate the same node,
    SceneKit's importer binds keys by target and plays none of them (`SCENEKIT_SHARED_CLIPS`,
    `scenekit-limits.mjs`; the Khronos MorphStressTest has several clips on one node), and
    three.js refuses the `<morph>` file, so nothing judges its animated geometry. If **every**
    animated frame of the silhouette is skipped for that one reason, and the rest frame is judged
    and passes, the case passes as "animated frames unjudged, cited": the case's report entry
    carries `animatedUnjudged` (the citation), `summary.daeKeep.animatedUnjudged` counts such
    cases, and the run's dae-keep line says "N case(s) with animated frames unjudged (SceneKit's
    shared clips; three.js refuses <morph>)", zero included, so they cannot hide in the passes.
    `notPlayed` (a clip name shadowed by an id), a render error, a missing frame, any other skip,
    and a mix of judged and skipped frames still fail. What no harness judge sees there, two
    clips animating one node's weights, is pinned by the unit test
    `kept_morph_weights_in_two_clips_round_trip` in `litegen-core/src/mesh/dae.rs`.
- **A `<morph>` must be there.** Where the expected scene has morph targets
  (`expectedStats().morphed`), a `dae-keep` file that holds no `<morph>` fails the case, "the
  expected scene has morph targets but the dae-keep file holds no <morph>": a writer that
  regressed to baking would otherwise write an ordinary Collada file that every judge passes
  (`keepMissingMorph`).
- The three.js uv sets 0 and 1, which three.js judges for a plain Collada file, go to SceneKit
  for such a file, as the sets three.js judged in no frame always do.
- **SceneKit's uv judge for a `dae-keep` file** (Ruling DM20, `keepUvJudge`). `Keep` changes
  no texture coordinate: SceneKit's uv picture of the plain `<from>__dae` file (the bake) and
  of the `<from>__dae-keep` file are byte-identical, at rest and at a sample time (the
  controller rendered both with the case's camera, 2026-11-09). What no run had asked before is
  SceneKit's own uv picture of a textured model against three.js's expected one, and for a
  file that holds a `<morph>` SceneKit is the only uv judge. MorphPrimitivesTest failed 4.07 %
  at rest against the 2 % limit, and MorphStressTest 5.20 %. So the frames are judged apart:
  - **at rest** against SceneKit's own uv picture of the plain counterpart (the same fixture
    and source, written without the morphs, whose uvs three.js judged), at the uv limit, with
    the 4x4-neighbourhood tolerance every uv comparison uses (`compareNearPixels`, the page's
    `compareNear` run on the two PNGs here). The ratio against the expected picture is kept
    beside it as `expectedRatio`, and the counterpart's as `counterpart`. A case with no plain
    counterpart (or one whose picture cannot be read) is judged against the expected picture;
  - **animated frames** against the expected picture, as for any file, unless the counterpart's
    own ratio against the expected picture at rest already exceeds the limit (it fails there
    too): then they are skipped, with the ratio named and the reason (SceneKit draws the bake
    and keep files' uvs identically at rest, and the silhouette judges the animated geometry).
    The `morph` fixture (1.675 % at rest) keeps its animated uv frames judged against the
    expected picture.

`report.json` records each refusal per case (`refusals.three`, `refusals.assimp`: the citation
and the error read) and totals them in `summary.daeKeep`.

**xmllint** (Ruling DM7) validates every `dae-keep` file, with or without a `<morph>`, against
the COLLADA 1.4.1 schema: `xmllint --nonet --noout --schema collada_schema_1_4_1.xsd <file>`,
0 errors. Its record is in the case's `validation` (`kind` `valid`, `invalid` with a count and
the first messages, or `schema` when the schema itself did not compile, which says nothing of
the file and never passes it).

- *Fetched at run time* (Ruling DM6), never committed: `https://www.khronos.org/files/collada_schema_1_4_1.xsd`
  (414,285 bytes, sha-256 `49266165027329388e710ade6a424116570cda6776cbf3623f7b8c78c727b0ff`,
  the file its `<xs:schema version="1.4.1">` header and the line numbers this README cites,
  L2379-L2385, agree with) and W3C's `https://www.w3.org/2001/03/xml.xsd` (4,726 bytes,
  `0ed94db44402526e7a683424d0bd4b65e5bc50e65d9b9a762b11b30bae06d0c8`), which the Collada
  schema imports at its line 22 for `xml:base`. The pins are `SCHEMA_PINS` in `dae-keep.mjs`.
- *Cached outside the repo*, in `$MESH_VERIFY_CACHE`, else `$XDG_CACHE_HOME/litegen-mesh-verify`,
  else `~/.cache/litegen-mesh-verify`. Each file is downloaded into a new, empty folder, its
  sha-256 is checked against the pin, and only then is the folder kept, as
  `<file>-<sha-256>/<file>`. Nothing is run on it but xmllint reading it as a schema.
- *A file whose sha-256 is not the pin is refused, not used*, whether the server changed it
  or the cache did: every `dae-keep` file then fails, naming both sums. Review the new file,
  and update the pin. A cached file is checked again on every run.
- *Resolved offline.* The schema's import names `http://www.w3.org/2001/03/xml.xsd`, which
  xmllint would fetch. The harness writes `catalog.xml` into the cache, mapping that URL to
  the pinned local copy, and runs xmllint with `XML_CATALOG_FILES=<it>` and `--nonet`, so
  nothing is fetched at validation time and nothing else can be (measured: without the
  catalog the import is skipped and the schema fails to compile, `kind: schema`, a failure).
  The first run needs the network; after that none does.
- *Skipped, not passed*, when xmllint is not installed or the schema cannot be fetched and is
  not cached, or when the cache folder cannot be written: the run prints why and
  `summary.daeKeep.schema` records it (`status` and `reason`); `summary.tools.xmllint` says only
  whether the tool is installed, and appears only in a run that has `dae-keep` cases (a plain run
  never probes it, so its output and `summary.tools` are as before). The pass counts as missing
  (a weaker run), like any other missing tool. `summary.daeKeep.schema` writes the cache's paths
  from `~`, so `report.json` names no user.
- The judge is shown to discriminate (2026-11-09, the real schema): a `dae-keep` file with
  `<morph method="BOGUS">` fails with 1 error ("The value 'BOGUS' is not an element of the set
  {'NORMALIZED', 'RELATIVE'}"), and one with a stray `<junk/>` in `<targets>` with 1 error.

**The SceneKit renderer and the weight animation** (Ruling DM10). SceneKit hangs a Collada
weight animation on the node's `SCNMorpher`, not on the node, and `render.swift` drove and
removed only `node.animationKeys`. Unpatched, a rest picture showed the animation's first key
instead of the default weights (a time with no clip would show it throughout). `render.swift`
now drives and removes `node.morpher`'s animation players the same way (two hunks, 22 added lines
as committed, comment included; the spike measured its own patch at about 17,
`docs/superpowers/research/research-scenekit-collada-morphs.md`). It moves no other picture: on `morph`, `skinned` and `staged` in
seven formats (`glb`, `dae`, `usdz`, `usda`, `obj`, `ply`, `stl`; 204 cases), all 992
SceneKit pictures of the 177 plain cases are byte-identical with and without the patch (438
dae, 185 usdz, 219 usda, 54 obj, 48 ply, 48 stl), and so are 458 of the 473 `dae-keep` ones.
The 15 that differ are the rest pictures of the five `dae-keep` files that hold a `<morph>`,
in `silhouette`, `albedo` and `uv`: silhouette 1.24 % apart before and 0.05 % after, uv 8.83 %
before and 0.55 % after, albedo 0.00 % in both (the picture follows the pose, and the colour
compared does not change). Every animated frame was identical before and after: the harness
plays a Collada clip by name, which Task 0 measured exact unpatched.

**Results, 2026-11-09** (`morph`, `skinned`, `staged`; `glb`, `dae`, `usdz`, `usda`, `obj`,
`ply`, `stl`; `--dae-keep`): 204 cases, 204 pass (177 plain, 27 `dae-keep`). Of the 27, five
hold a `<morph>` (the `morph` fixture from `scene`, `single`, `glb`, `usdz` and `usda`); the
other 22 are ordinary Collada.

- three.js refused 5 of 5 and assimp 5 of 5, each with the documented error (three.js: the
  `parseAnimationChannel` one, since `morph` has a clip).
- xmllint: 27 of 27 files valid, 0 errors.
- SceneKit on the five `<morph>` files, at rest and at six sample times (7 frames each, 35):
  silhouette at most 0.062 % apart (limit 1 %), albedo 0.00 % (4×4 block means), uv set 0 at
  most 1.675 % (limit 2 %: a gradient drawn on a morphing mesh, whose sub-texel sampling is
  the limit's reason, as for a USD file's BlendShapes). The 22 ordinary files: silhouette at
  most 0.323 %, albedo 0.00 %, and `staged`'s camera pictures at most 0.261 %.
- The camera and light oracle ran for the 22 ordinary files (assimp reads them), and had no
  camera or light to judge in the five `<morph>` ones.

### USDC, several clips and unlit in USD (mesh-gaps item 6)

`usdc` is a format of the matrix like `usda` (`--formats glb,usda,usdc,usdz`): every USD judge
reads it (`USD_FORMATS`, `SCENEKIT_FORMATS`, `ORACLE_FORMATS` in `verify.mjs`), and three.js's
`USDLoader` reads a crate correctly where it misreads a USDA's skeletal rotations
(`usdLimitation`, `page/render.js`), so a USDC's animated frames are judged by three.js
directly and `crate:geom`'s usdcat copy is skipped for it. One crate three.js cannot read: its
`USDCParser` takes an array whose ValueRep has payload 0 for an array at offset 0, where OpenUSD
packs and unpacks an empty array that way (`USDCParser.js:1087-1102`; `crateFile.cpp:1775-1777`
and `:1805-1808` at v26.11.1), so a crate holding an empty array (the `morph` fixture's `joints`,
`bindTransforms` and `restTransforms`) is a cited skip for three.js (`THREE_CRATE_EMPTY_ARRAY`,
`crateEmptyArrays`), measured on a crate Apple's own `usdcat` wrote. usdrecord judges that crate,
and three.js judges the same data in its USDA twin. The pure rules of the checks below,
and their tests, are `usd-variants.mjs` and `usd-variants.test.mjs`
(`node --test usd-variants.test.mjs`); the drawing and the tools are in `verify.mjs`
(`usdItem6Checks`, `usdClipChecks`, `usdUnlitChecks`, `usdcPairChecks`).

`mesh_matrix --usd-variants` (with `usdz` among the formats) adds, for every direction into
USDZ, three more cases, as `--dae-keep` does for Collada: `usdz-all` (every clip, a USDA root
layer), `usdz-crate` (every clip, `usdz_root: usdc`, a USDC root layer) and `usdz-crate-first`
(`usdz_root: usdc` with the default single clip: the plain crate-rooted USDZ a device check
needs). The manifest's `to` is the variant's name, `verify.mjs` reads such a case as `usdz` with
`variant` naming it (`usdVariantCase`), and its pictures go under `<from>__usdz-all`,
`<from>__usdz-crate` and `<from>__usdz-crate-first`. A plain `usdz` keeps the first clip alone, as
before.

What each check does:

- **usdcat.** Every USD file is read with `usdcat <file>`: exit 0 and nothing on stderr. For each
  source, `usdcat model.usda` and `usdcat model.usdc` must print the same text (the canonical
  printer: any difference is a value, type, variability or metadata the crate lost), and every
  usdrecord picture of the two files (cover, albedo, each clip, each unlit picture) must be
  identical pixel for pixel, compared at tolerance 0 with the largest channel delta recorded
  (`usdcPairChecks`, `report.json` `usdcVsUsda` per case and `summary.usd.usdcVsUsda`).
- **Several clips** (a file with an `Animation` variant set, read from usdcat's text;
  `fixtures::clips()`: walk 1 s, run 0.6 s, wave 2 s). usdrecord draws each variant through a
  session layer (`over "<root>" (variants = { string Animation = "<clip>" })`) at rest and at six
  sample times of that clip's own range, against that clip's expected picture: the clip's
  skeletal channels beside the first clip's rigid ones, which a USD file plays under every
  variant (`alsoRigidOf`, `page/render.js`). A clip whose expected picture equals the first
  clip's at every time, or that usdrecord draws as the first at every time, fails the case, since
  such a check could not tell the variants apart. `usdchecker` (with `--arkit` on a USDZ) must
  report each variant ("Validation Result for Variants: Animation:<clip> on Prim: /Root") and
  pass it. three.js lists the clips on a crate (the file itself, or a usdcat crate copy; a USDA is
  read at 30 time codes per second, `THREE_USDA_FPS`): a clip per SkelAnimation named after it,
  with the clip's duration, and the first clip's rigid motion as `TransformAnimation`. SceneKit
  draws the authored selection in its ordinary passes; the other variants are cited skips
  (`SCENEKIT_VARIANT_SKIP`). What SceneKit does with such a file (measured, macOS 26.6.2): its
  importer keys one player per SkelAnimation prim by its path and plays them all at once,
  whatever `skel:animationSource` binds and whichever variant is selected; and it keys a
  USD xformOp animation's samples over the whole stage's time range, so the first clip's rigid
  motion runs slow by stage span over clip span (the prop's row at scene time 2 s equals
  usdrecord's at 1 s). `render.swift`'s `playClip` removes the other players and plays the
  rigid keys at that ratio, so the selected clip is judged on where its parts are.
- **Unlit** (the `unlit` fixture from `scene` and from `glb`, into every USD format). The unlit
  box is written as an emissive-only surface; the flat wedge and the vertex-coloured panel stay
  lit as the control. Each renderer draws the unlit group alone and the lit control alone at two
  light levels (`UNLIT_LEVELS`): usdrecord with a normalised distant light in the session layer
  (an unnormalised one of intensity 1 to 10 lights nothing in Storm, which a lit control shows),
  SceneKit's own materials under an ambient and a directional light (`render.swift`, `lit`), and
  three.js's loader materials under an ambient and a directional light
  (`renderLightLevels`). usdrecord's dome light is drawn too, and its residual on the unlit
  group is recorded, not judged: Storm's dome adds `prefilter * (F * brdf.x + brdf.y)`
  (`previewSurface.glslfx:327`), and `brdf.y` survives a black specular colour (`STORM_DOME_RESIDUAL`;
  measured, 13.9 % of the pixels past 1/255, at most 6/255, 0.11 % past 3/255). On the pixels only the unlit group covers, the two levels must be
  identical within 1 % of pixels (per-channel tolerance 1/255), and equal to the base colour
  within the albedo threshold (`UNLIT_LIMITS`); the lit control must change (at least half its
  pixels by more than 8/255). The residual (`maxDelta`, `meanDelta`) is recorded. A renderer
  measured to read the base colour differently records it without judging it, citing its
  source line (SceneKit: `SCENEKIT_USD_ALBEDO`, `SCENEKIT_USD_TEXSCALE`; three.js:
  `THREE_USD_BASE_SKIP`), and every renderer still judges the invariance.

Measured 2026-11-10 (`mesh_matrix --formats glb,usda,usdc,usdz --usd-variants`, 444 cases, all pass;
Apple USD Tools 0.25.2, three.js r183, macOS 26.6.2): usdcat clean on 384 of 384 files; the 72
usdc/usda pairs print the same text (12,406 lines) and 711 of 711 usdrecord pictures are identical
(largest channel delta 0); 18 multi-clip files, 54 variants drawn by usdrecord at rest and at six times
(378 frames) with 0 differing pixels against each clip's expected picture, usdchecker reporting all 54
and `--arkit` passing the USDZ ones, three.js listing every clip with its duration (walk 1.000 s, run
0.600 s, wave 2.000 s) and the first clip's rigid motion as `TransformAnimation` on 18 crates; unlit
on 12 cases: 0 of 5,388 pixels differ between the two lights in usdrecord (max delta 0), 0 of 5,268
in SceneKit (0), 0 of 5,265 in three.js (max delta 1, mean 0.013), and the lit control moves 98.5 %
(max 64/255), 100 % (85) and 100 % (71) of its pixels; usdrecord's base-colour ratio is 7.61 % against
the 8 % limit, SceneKit's and three.js's are recorded, not judged.

## Oracle conventions it encodes, and why

Every one of these is a documented disagreement between readers. None is a tolerance:

- **V direction.** GLTFLoader keeps glTF's top-left UV origin; the OBJ, FBX, Collada, 3MF
  and PLY loaders keep their formats' bottom-left one and flip textures instead.
  `render.js` flips the set a uv mode draws before it compares them (`V_UP`).
- **Missing normals and UVs.** OBJLoader invents normals when a file has none, while
  GLTFLoader does not. So the `normals` and `uv` modes run only when the expected file
  really has that data, and a per-set mode only for a set it has.
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
| three.js USDCParser | a crate that holds an empty array (payload 0 is read as an offset, `USDCParser.js:1087-1102`; OpenUSD unpacks it as empty, `crateFile.cpp:1805-1808` at v26.11.1; measured on Apple's own usdcat crate) | usdrecord on the crate, and three.js on its USDA twin |
| three.js USDLoader | uv sets past 1 (it builds `uv` and `uv1` alone, `USDComposer.js:1340-1398`), and a base-colour texture on one of them (any varname but `st1` and `st2` gets channel 0, `:3130`, `:3159-3171`) | SceneKit's `uv` mode; usdrecord's and SceneKit's albedo |
| three.js ColladaLoader | TEXCOORD sets past 1 (`ColladaComposer.js:1755-1763`) | SceneKit's `uv` mode |
| three.js, any loader | uv sets past 3 (its materials address `uv` to `uv3`) | nothing: no expected picture can be drawn, and the skip says so |
| three.js USDLoader, `lit` | a coat written through a texture connection (it reads `inputs:clearcoat`, `inputs:clearcoatRoughness` and `inputs:ior` as values alone, `USDComposer.js:2661-2671`, `:2626-2630`), and any base colour between 0 and 1 (decoded as sRGB, `:2512-2520`) | `lit_coat`, the same loader on the meshes it reads right; and the usd → glb cases, through our reader and GLTFLoader at 0.00 %; the pictures stay in the report |
| usdrecord, `lit` | a coat under a uniform dome (Storm adds the lobe on top of the base, `previewSurface.glslfx:330-344`; control (a) moves its picture by at most 10/255, Ruling A32) | `lit_coat` and the usd → glb cases; its picture and ratio stay in the report, skipped |
| three.js GLTFLoader, `lit` | a clearcoat normal map's frame, as a conformant viewer reads it: with a `TANGENT` attribute present GLTFLoader stops flipping `clearcoatNormalScale.y` (`GLTFLoader.js:3438`, `:3501-3505`) while the shader still derives the frame from the derivatives, since it reads the attribute only with a base normal map or anisotropy (`three.module.js:7716`); the same GLB with and without its TANGENT differs by 8 % in `lit` | nothing: both sides carry the same tangents, so the comparison holds; the tangents' handedness is pinned by `car_paint_tangents_follow_gltf` (`fixtures.rs`) |
| three.js MTLLoader, ColladaLoader, FBXLoader, ThreeMFLoader, PLYLoader, STLLoader, `lit` | any layer: Phong or Lambert materials (`MTLLoader.js:511`, `ColladaComposer.js:1170`/`:1174`, `FBXLoader.js:536-547`, `3MFLoader.js:1404`/`:1410`) or no material at all (`PLYLoader.js:776`, `STLLoader.js:279`); MTLLoader's property switch (`:387-507`) has no case for `Pc`, `Pcr`, `Ps`, `Ni`, `aniso` or `anisor`, and ColladaParser reads no `<index_of_refraction>` (`ColladaParser.js:751-767`) | the obj → glb cases, which carry the layers an OBJ keeps; Collada's ior (glass's 1.52, plastic's 1.6, the lacquer's 1.45 dropped) moves at most 1/255, so dae → glb is a measured skip and nothing judges it in `lit` |
| SceneKit, USD | linear sRGB colours (reads Display P3), primvar readers, a SkelAnimation's default `blendShapeWeights`; it also draws meshes under undefined `over` prims (where Apple's importer keeps instancing prototypes) | usdrecord |
| SceneKit, OBJ | `Kd` beside `map_Kd` (it uses the map alone; the MTL spec multiplies) | three.js |
| SceneKit, Collada | clips that animate the same node (it binds keys by target, so the clips overwrite each other) | three.js, which plays each clip; SceneKit still judges the rest pose |
| SceneKit, Collada | a clip whose name (its id when nameless) is the id of a node, a geometry or an animation: it registers no entry for it (measured, item D, above) | three.js, which plays the clip; SceneKit still judges the rest pose |
| three.js ColladaLoader, a `<morph>` | the whole file: it throws (`ColladaParser.js:445-448` stores a morph controller with no skin, `ColladaComposer.js:2353` reads `controller.skin.joints`, a skin over a morph fails at `:969`, a weight channel at `ColladaParser.js:347-354`) | SceneKit; the case is expected to be refused and fails if three.js loads it (`dae-keep`, below) |
| assimp, a Collada `<morph>` | the whole file: its `<targets>` loop advances the wrong node (`ColladaParser.cpp:902-914` at v6.0.5), so the import fails resolving `""` (`ColladaLoader.cpp:685`) | SceneKit; expected to be refused, as for three.js |
| SceneKit, USD | `UsdUVTexture`'s `inputs:scale`, which tints a base-colour texture by the base colour (the two pictures with and without the tint are byte-identical, measured, item E, above) | usdrecord and three.js USDLoader, which follow it; SceneKit still judges the silhouette |
| assimp, Collada camera | the field of view of a `<perspective>` camera with `<yfov>` alone: `aiCamera`'s default π/4, read as degrees, 0.0137078 (`ColladaLoader.cpp` L418-L436, `camera.h` L187; item B) | the file's own `<yfov>`, in degrees, against the scene's |
| SceneKit and usdrecord, albedo and silhouette | the expected scene's translucency and texture alpha: the expected pictures draw every material as one opaque colour (`albedoMaterial`), SceneKit and Storm draw it blended (Ruling A37, measured on the Khronos imports) | three.js's albedo, and `lit`; the pictures stay in the report |
| SceneKit, albedo per pixel | a texture far finer than a pixel (two renderers minify it differently, pixel by pixel) | SceneKit's own albedo on 4×4 block means (Ruling A38), which a wrong texture still fails |
| three.js FBXLoader, `camera` and `lit_lights` | an FBX camera's +X axis (`createCamera`, `FBXLoader.js:1064-1153`), an orthographic camera (`:1137-1140`), `NearPlane` and `FarPlane` (divided by 1000, `:1097`, `:1104`); a light's aim without `LookAtProperty` (`:1423-1459`), its cones (`:1242`, `:1246-1253`) and its falloff (decay 1, `:1225`) | assimp's reading: the oracle, and the expected geometry drawn through it |
| three.js ColladaLoader, `camera` and `lit_lights` | `<xfov>` (`ColladaComposer.js:1410-1417`), `<xmag>`/`<ymag>` (halved, `:1427-1428`); a light's aim (no target, `:1466-1494`), its cone (`<falloff_angle>`, the full cone of Ruling R23, made a `SpotLight`'s angle, a half-angle, `:1492` and `SpotLight.js:79-85`: every spot twice as wide), `<falloff_exponent>`, its falloff (a √(1/quadratic) cutoff, `ColladaParser.js:1185-1188`) and its linear `<color>` (`:1175-1178`) | SceneKit, and assimp's reading |
| three.js USDLoader, `camera` and `lit_lights` | any camera or light: their prims become empty `Object3D`s (`USDComposer.js:663-673`) | SceneKit (cameras) and usdrecord |
| SceneKit, USD lights | any UsdLux light (Model I/O reads none, measured) | usdrecord |
| SceneKit, Collada light colour | a channel past 1 (clamped, measured), which litegen's colour × intensity has | assimp's reading |
| SceneKit, an orthographic camera | its scale: a Collada one's read as xmag × ymag, a USD one's as 1 (measured) | the oracle; usdrecord for USD |
| Storm (usdrecord), a USD spot's cone | any cone on a `SphereLight` (`hdSt/light.cpp` L244-L248) | nothing here: litegen's unit tests alone |
| assimp, FBX and Collada orthographic cameras | their projection and magnifications (`FBXConverter.cpp` L472-L508; `ColladaLoader.cpp` L400-L403) | the unit tests; an orthographic first camera has no picture judge in FBX. A glTF output's magnifications are read from its own JSON (`withGltfMagnifications`), and a USD stage's from its text |

The colour defects of three.js's USDLoader and SceneKit's USD importer act only on
colours strictly between 0 and 1 (sRGB, linear and Display P3 agree at 0 and 1) and on
vertex colours. So where the expected scene has neither, USD albedo is compared in both,
as for any other format. That is how a USDZ's base-colour texture is judged in three.js,
SceneKit and usdrecord alike.

SceneKit draws a USD stage in the file's own numbers: it applies neither `metersPerUnit`
nor `upAxis`. So for a USD original, its camera is moved into the file's space, as the
usdrecord camera is, its up direction with it (+Z for a Z-up stage: render.swift aims a
camera with +Y up unless told; Blender's Z-up export of `staged`, the first Z-up USD
original here, drew rolled, 58.39 % apart, and 0.05 % with its up).

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
- `usdrecord`, `usdcat` and `usdchecker` (Apple's USD tools, or an OpenUSD build);
- `xmllint` (libxml2; `/usr/bin/xmllint` on macOS), for the `dae-keep` variant's schema pass,
  and network access once for the schema's first fetch (it is cached after that). A run
  without `dae-keep` cases never looks for it.

Any tool that is missing is named at the end of the run and in `report.json`
(`summary.tools`), and its checks are skipped. A missing usdrecord fails the USD cases
instead, since three.js alone cannot judge them.

## `read-gltf.mjs`

`node read-gltf.mjs <file.glb>` reads one glTF or GLB with `@gltf-transform/core`, a reader
that shares no code with litegen, and prints what a consumer of the file sees as JSON: each
animation's name, channel count and last key time, the skins' joint names, and the
extensions the file requires. The 3D chain test (`litegen-core/tests/model3d_chain.rs`) runs
it on the final animated GLB after `npm ci` here.

## `meshopt-vectors.mjs`

The test vectors of `litegen-core/src/mesh/meshopt.rs` (mesh-gaps item 2). The script
builds meshopt streams by hand from Appendix A of the `KHR_meshopt_compression` spec,
forcing the encodings each test needs (every v0 header mode, every v1 control mode, both
v1 bit tables, every channel mode, every triangle code, both index baselines), decodes
each with meshoptimizer's own reference decoder, checks that it reads back the data the
stream was built from, and prints what it read as the Rust consts the tests compare with:

```sh
curl -O https://raw.githubusercontent.com/zeux/meshoptimizer/f0a684f11ead064e176775c9b796f82a55910e92/js/meshopt_decoder_reference.js
node scripts/mesh-verify/meshopt-vectors.mjs meshopt_decoder_reference.js
```

The reference decoder (MIT) is the only third-party code involved and is not kept in the
repo; its sha256 is checked before it runs (`REFERENCE_SHA256` in the script). With
`--views <file.gltf> <out.bin>` the script decodes a glTF file's own meshopt views
through the reference decoder instead, which is how the Khronos and gltfpack
differentials of `glb.rs`'s tests were made.
