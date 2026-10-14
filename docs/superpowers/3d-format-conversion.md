# 3D format conversion: what litegen converts, how, and how it is proven

Written 2026-10-12. Companion to `3d-provider-landscape.md` (which vendors return what) and
`specs/2026-08-19-3d-model-generation-design.md` §14 (the `output_formats` promise). Code:
`litegen-core/src/mesh/`. Verification harness: `scripts/mesh-verify/`. The format research,
with its sources, is in [`research/`](research/README.md); the code review of 2026-10-12,
with what is still wrong, is [`2026-10-12-mesh-conversion-review.md`](2026-10-12-mesh-conversion-review.md).

## 1. What changed

The first version of `mesh/` converted one merged, world-space triangle mesh between glb, obj,
stl and ply. That was enough to turn a GLB into an STL, but not to deliver an FBX that
Unity can rig, a USDZ that opens textured in AR Quick Look, or anything that moves. Its
documentation said so, and it rejected fbx and usdz outright.

The module now converts a **scene**:

- node hierarchy with transforms;
- meshes made of per-material primitives with normals, tangents, any number of UV sets and
  vertex colours;
- metallic-roughness materials with embedded PNG/JPEG textures and texture transforms;
- skins with inverse bind matrices;
- morph targets;
- keyframed animation of node transforms and morph weights (LINEAR, STEP, CUBICSPLINE).

It reads and writes ten containers. Every pair of them is a supported direction.

| container | read | write (single file) | write (package) |
|---|---|---|---|
| glb | full scene | full scene | — |
| gltf | full scene, external `.bin` and images via sidecars | full scene, buffer embedded | `.gltf` + `.bin` + images |
| obj (+mtl) | objects, per-material primitives, MTL (Kd/Ke/d/Ns/Pr/Pm, `map_Kd`/`map_Ke`/`norm` with `-o`/`-s`), vertex colours | world-space geometry; material colour baked into vertex colour | `.obj` + `.mtl` + textures |
| stl | ASCII and binary | geometry | — |
| ply | ASCII and binary LE, per-vertex colour | geometry, normals, per-vertex colour | — |
| fbx | §4 | §4 | — |
| usdz / usda | §5 | §5 | usda + textures |
| 3mf | §6 | §6 | — |
| dae | §7 | §7 without images | `.dae` + images |

What each target keeps is a **capability set** (`MeshFormat::capabilities`), and what a
writer encodes is exactly `MeshFormat::prepare(scene)`. Everything a target cannot hold is
reported as a `Loss` (`mesh::write_scene` → `SceneWrite::losses`), computed from what the
scene actually contains. Converting a static mesh to STL loses nothing worth reporting;
converting an animated character to STL reports the animation, the skin and the materials by
name.

## 2. Architecture

- `scene.rs`: the IR, shaped like glTF's document model, in glTF's conventions (right-handed,
  +Y up, metres, UV origin top-left, linear colour, skinning evaluated as
  `Σ wᵢ · world(jointᵢ) · inverseBindᵢ` with the holding node's transform ignored). Every
  codec converts at its own boundary and nowhere else. It also holds the scene validation
  that every reader runs before returning and every writer before writing, and the
  capabilities and loss reporting.
- `pose.rs`: one evaluator for the whole module. It does animation sampling (slerp; glTF's
  scaled-tangent Hermite), morphing, linear-blend skinning, baking into world space,
  merging, and `degrade`, the lossiness table as code. Static containers (OBJ, PLY, STL,
  3MF) receive the model as a viewer shows it with no clip playing, evaluated once, here.
- `math.rs`: column-major `f64` matrices, quaternions, Euler orders (for FBX and Collada),
  decomposition, and the sRGB transfer.
- `package.rs`: multi-file containers. Readers ask a `Sidecars` resolver for files named
  inside the model. `normalize_relative` refuses any reference that escapes the package
  (`..`, absolute paths, schemes) before a resolver sees it.
- `zip.rs` / `xml.rs`: ZIP (USDZ, 3MF) and XML (3MF, Collada). Both are bounded against
  hostile input:
  - zip: a lazy, size-capped inflate (no zip bombs); CRC checks; duplicate and escaping
    names refused;
  - xml: namespaces resolved by URI, no entity expansion beyond XML's five, depth and
    element caps.
- Dependencies: the module stays std-only except for crates **already linked into the
  binary** through other dependencies, each confined to the one file that needs it.
  `serde_json` is used by glb.rs, `miniz_oxide` by zip.rs and fbx.rs, and `quick-xml` by
  xml.rs. `isolation_tests.rs` enforces this.

## 3. glTF, OBJ, PLY, STL

- **glTF (the hub)** reads everything the IR models:
  - normalised and quantised (`KHR_mesh_quantization`) attributes, and sparse accessors
    (zero-based morph targets);
  - every `JOINTS_n`/`WEIGHTS_n` pair and all six sampler minification modes;
  - `KHR_texture_transform`, `KHR_materials_emissive_strength` and `KHR_materials_unlit`;
  - WebP/KTX2 images, carried as-is.

  It still refuses Draco and meshopt compression, because those decoders are separate
  codecs. All previous hostile-input bounds are kept, plus a decode budget, so that
  aliasing accessors cannot amplify a small file into gigabytes. The writer validates clean
  against the Khronos glTF-Validator: 0 errors and 0 warnings on every file the harness
  produced. It also moves skinned meshes to untransformed roots, because three.js
  multiplies a skinned node's transform in where the spec says to ignore it.
- **OBJ**:
  - each `o` becomes a mesh and each `usemtl` a primitive; MTL libraries and textures
    resolve through sidecars;
  - `v x y z r g b` vertex colours are read and written;
  - colours (`Kd`, `Ke`, vertex) are **sRGB**, as three.js, MeshLab and Unity read them.
    Blender reads `Kd` as linear: a real ecosystem disagreement, documented in `obj.rs`;
  - a single-file OBJ (what the proxy delivers) bakes material colour into vertex colour
    rather than referencing an `.mtl` that will not be there.
- **PLY:** per-vertex colour in both directions, encoded **sRGB** (three.js, MeshLab and
  Blender). This fixed a bug: see §8.
- **STL:** unchanged. It holds geometry only; colour, UVs and normals are reported as lost.

## 4. FBX

`mesh/fbx.rs` (with `fbx_dom.rs`, `fbx_read.rs` and `fbx_write.rs`). The earlier objection,
that open writers produce files DCC tools reject, is answered here by measurement, not
assumption. The file layout follows what the Autodesk SDK writes (checked byte by byte
against SDK-written Mixamo and 3ds Max files), and the reader is checked against DCC-evaluated
ground truth.

- **Write:** binary 7400 in the SDK's layout:
  - header extension and NULL records, including the `AnimationLayer` one whose absence
    makes Assimp drop all animation;
  - the Blender/Assimp FileId/CreationTime/footer triple, and zlib arrays;
  - IDs below 2^53 (three.js reads them as JS numbers), and names made unique (three.js binds
    animation tracks by name);
  - GlobalSettings Y-up with `UnitScaleFactor = 100`, so the data stays in metres;
  - Phong materials using Blender's PBR mapping, with embedded textures. Under a texture
    the colour is written white and only a factor scales the map, because readers
    disagree about a colour beside a map (next bullet);
  - Skin/Cluster with a BindPose (three.js mis-skins without one), and BlendShape channels
    with `FullWeights` and `DeformPercent` written both ways;
  - animation curves at 46,186,158,000 ticks/s, resampled at 30 fps plus every source key,
    with Euler XYZ unwrapped, all axes keyed at identical times (three.js requires it), and
    STEP emulated with an extra key;
  - extra keys wherever a rotation curve strays from the scene's rotation by more than
    0.05° between keys. FBX readers interpolate each Euler angle on its own, the scene
    slerps, and near gimbal lock the two part by degrees inside one 30 fps step (see §8,
    bugs);
  - takes that start at 0, with a key there holding the first pose, as playback does;
  - a node scaled to exactly zero written with a millionth instead: three.js composes
    and then decomposes every node transform, and its `Matrix4.decompose` turns a
    singular matrix into unit scale.

  Colours are written as sRGB, which is how three.js's FBXLoader reads them.
- **Read:**
  - binary 7100–7700 (32- and 64-bit records) and ASCII;
  - the full FBX transform stack: pivots, offsets, pre/post rotation, rotation order,
    InheritType 0/1/2, and geometric transforms baked into the geometry only;
  - every layer-element mapping, with per-polygon materials split into primitives;
  - Phong/Lambert, Stingray, Arnold, and 3ds Max Physical/OpenPBR materials; textures
    embedded or from sidecars. A map replaces the colour beside it, and only the factor
    (`DiffuseFactor`, `EmissiveFactor`) scales it. That is how the SDK, Maya, Blender and
    ufbx read classic Phong, and what the author saw: Blender writes its socket default 0.8
    beside every diffuse map, and Maya writes 0.5. three.js, Unity and Assimp multiply the
    colour in instead, and so draw such files darker;
  - skins (inverse bind = cluster `Transform`) and blend shapes, including **in-between
    shapes**, exact against Maya's own exported frames;
  - the first animation stack, with GlobalSettings axes and units applied as one root
    correction.

  **Curves follow ufbx's reconstruction of the SDK's evaluation**: auto/clamped/TCB/user
  tangents and weighted handles, checked against Maya and MotionBuilder reference values on
  37 files, with a maximum error of 1.1e-3 and most under 1e-5. A 3ds Max skinned file
  matches Max's own OBJ exports at two frames to within 6e-5 of model size, where Assimp is
  9.7 % off because it interpolates cubic keys linearly.
- **A bug the work found:** from 7.2 on, ASCII `KeyAttrDataFloat` stores each float as the
  integer of its bit pattern. Reading those as numbers turned every Maya/Max ASCII curve
  with tangents into garbage.
- **Not carried, and reported:** double-sided, alpha mask, unlit, mirrored wrap, texture
  filters, a tint beside a texture, and every animation stack and layer after the first.
  Sheared nodes are refused.
- **Licensing** (research, not legal advice): nothing restricts writing the format itself
  without Autodesk's SDK, no patents or legal action were found, and "FBX" is a trademark,
  so it is used referentially. Assimp (BSD-3) and ufbx-write (MIT) are permissive
  precedents; Blender's exporter is GPL and none of it was used. The SDK licence's SaaS
  clauses argue against running the SDK itself server-side.
- **Oracles:**
  - three.js's FBXLoader rendered every case, and assimp imports every FBX written.
  - 25 third-party FBX files (Mixamo, Maya, 3ds Max, Blender, ufbx's corpus) ran in both
    directions, used locally only. Each of the 15 native differences against three.js
    traces to a cited FBXLoader limitation: it ignores UpAxis and DeformPercent, takes
    only the first in-between shape, interpolates cubic keys linearly, and mispairs
    rotation axes with different key times.

## 5. USD / USDZ

`mesh/usd.rs`, with `usd_stage.rs` (composition), `usda.rs` (text), `usdc.rs` (binary
crate), `usd_read.rs` and `usd_write.rs`. There are two format names: **usdz** (one file,
textures inside) and **usda** (a text layer, whose textures are sidecars in package mode
and a reported loss in a single file). `usd` and `usdc` stay unparsed: crates are read,
but nothing writes one, and a format name must work in both directions.

- **Write:** the root layer is USDA text.
  - USDZ follows Apple's package rules: a stored (uncompressed) zip, every file 64-byte
    aligned, `model.usda` first, sizes and CRC in the local headers, and textures under
    `textures/`. `usdchecker --arkit` accepts every file written. It has accepted a USDA
    root since OpenUSD 24.03 (verified with Apple USD Tools 0.25.2), and model-viewer
    ships exactly that to AR Quick Look.
  - Stage metadata: `defaultPrim`, Y up, `metersPerUnit = 1`, and 30 time codes per second,
    because three.js's USDA parser assumes 30.
  - Nodes are Xforms with translate, orient and scale ops, time-sampled when animated.
    Prim names are sanitised and made unique; the original name is kept in `customData`
    and restored on read.
  - Meshes carry `st` (V flipped), `st1`, normals, `displayColor` and `displayOpacity`.
    Materials are UsdPreviewSurface networks; texture transforms are baked into the uvs.
  - Skins are UsdSkel: Skeleton, SkelAnimation and joint influences. Morph targets are
    BlendShapes whose weights live in the SkelAnimation, so a morph-only mesh gets a
    skeleton with no joints. `prepare` folds a non-joint node between two joints into the
    child joint.
  - One animation clip. STEP keys become a hold plus a 1 ms ramp, and CUBICSPLINE is
    resampled at 30 fps. Channels that never move their node are dropped.
  - What the SkelAnimation carries is keyed from time 0, holding its first value, because
    SceneKit otherwise starts a skeleton's clip at its first sample. xformOps get no such
    key: with one, SceneKit stretches the op's animation (both measured, §8).
- **Read:** USDA text, and every section of a USDC crate: a bounded hand-written LZ4,
  integer and float compression, time samples, dictionaries, list ops and the path tree.
  - Composition: sublayers; internal and external references and payloads (through
    sidecars, otherwise a warning); inherits and specializes; variants by their authored
    selection; class, over and `active`; layer offsets and time codes per second.
  - Transforms: Euler, pivot, matrix and `!invert!` op stacks. Z-up and centimetre stages
    are converted.
  - Meshes: faceVarying primvars, n-gons, holes, leftHanded orientation, GeomSubsets.
  - Skins: `skel:joints` subsets, `geomBindTransform`, constant bindings.
  - Hostile input is bounded with explicit stacks, checked counts, and caps on instancing
    and layers. Truncating or flipping any byte of the crate test files never panics.
- **Not carried, and reported:** extra clips, unlit, tangents, texture filters. Animated
  joint scales go through UsdSkel's `half3[]`, about 1e-3 relative.
- **Not read, with a warning:** relocates, value clips, splines, crate array edits, nested
  USDZ references, PointInstancer, cameras, lights, separate metallic and roughness
  textures, the specular workflow, clearcoat.
- **Oracles.** three.js's USDLoader is a weak oracle, and `page/render.js` cites the source
  line behind each of its gaps:
  - it reads linear colours as sRGB and keeps only `displayColor[0]`;
  - it misreads every SkelAnimation rotation in USDA text, because `quatf[]` is never
    reordered;
  - it has no BlendShape support.

  Where it is blind, other oracles judge:
  - **usdrecord** (Pixar's Hydra Storm, the reference renderer) renders every USD output
    through the harness camera, at the default time and at every sample time. It produces
    silhouettes (`cover`) and albedo, with lighting removed by session-layer overrides.
  - **`crate:geom`**: three.js renders a `usdcat` crate copy of each skinned file, whose
    rotations it reads correctly.
  - **SceneKit** silhouettes.

  Negative controls (deliberately damaged outputs) fail every time:

  | damage | measured |
  |---|---|
  | joint rotations conjugated | 70–74 % |
  | a blend shape negated | 7–29 % |
  | `st` V-flipped | 24 % |
  | red and blue swapped | 18 % |

  The thresholds are 3 % (cover) and 8 % (albedo). Correct files measure 0.00 % cover and
  0.2–5.7 % albedo, all of it along one-texel texture stripes, which the two renderers
  filter differently.
- **Third-party USD:** 12 files ran in every direction: our content through Apple's packager
  and glTF importer, three.js's USDZ exporter, and the research packages.
- **Not yet known:** behaviour on a device (Quick Look, RealityKit). Apple's own packagers
  convert the root layer to USDC; ours stays USDA, which `--arkit` accepts. SceneKit does
  not evaluate a primvar reader feeding `diffuseColor`, so the writer also writes the base
  colour beside the connection. Whether RealityKit evaluates it is untested.

## 6. 3MF

`mesh/threemf.rs`. An OPC package: `[Content_Types].xml`, `_rels/.rels` naming
`/3D/3dmodel.model`, and textures under `3D/Textures/` with 3dtexture relationships.

- **Units.** Written as `unit="meter"` with the scene's numbers unchanged. That is faithful
  and bit-exact, and PrusaSlicer 2.9.4, Bambu Studio and Cura all scale `meter` ×1000 on
  import. On read, every unit from micron to foot converts to metres.
- **Axes.** 3MF is Z-up. The Y-up→Z-up turn lives in the build item's transform, so vertex
  numbers are untouched; it is undone at the roots on read.
- **Structure.** One `type="model"` object per mesh, with every triangle carrying its
  property (`pid`/`p1..p3`).
  - Several placements of one mesh become a single assembly of `<component>`s, so instancing
    survives (the spec lets consumers rearrange build items, not components).
  - Mirrored placements stay transforms (Core §3.3 allows them). Sheared ones are baked into
    their own object, because three.js decomposes transforms and loses shear.
- **One property group per triangle** is the format's real limit, and `threemf::prepare`
  encodes it exactly:
  - a textured material → `texture2d` + `texture2dgroup` (V flipped, wrap → `tilestyle`,
    filter mapped), dropping its tint and any vertex colour;
  - otherwise vertex colour → `colorgroup`, with the material colour baked in;
  - otherwise → `basematerials`.

  Colours are sRGB `#RRGGBBAA`, as the spec defines them.
- **Reading** streams the model part (`xml::stream`), resolves namespaces by URI (including
  `requiredextensions` prefixes), and also covers:
  - components, with cycles refused, depth capped and the node count bounded;
  - the production extension's `p:path`;
  - per-triangle property defaults;
  - composite materials and multiproperties, approximated with a warning;
  - ZIP64 archives, which 3MF requires consumers to support and assimp always writes.

  Every 3MF Consortium MUSTPASS and example file read, and every MUSTFAIL file tried was
  refused with a specific message.
- **Lost:**
  - normals (3MF has none) and colour-group alpha;
  - texture tint;
  - UVs on untextured parts;
  - skins, morphs and animation, posed at rest;
  - grouping nodes, flattened to placements.

  All are reported as losses.
- **What slicers read** (research): PrusaSlicer 2.9.4 reads none of the colour, material or
  texture extension; Bambu Studio maps colour groups to filaments; Cura loads textures only
  for its own painting feature; three.js reads all of it.

## 7. Collada

`mesh/dae.rs`, COLLADA 1.4.1. Every document the matrix produced validates against the
official 1.4.1 XSD.

- **Write:**
  - Asset in metres and `Y_UP`, with fixed dates. Ids are unique, deterministic NCNames
    (the XSD requires them, and SceneKit cuts names at the first space).
  - One `<geometry>` per primitive, as `<triangles>` with one shared index. UVs are flipped
    to (u, 1−v) and written in f64 so they round-trip bit-exactly.
  - Effects are `profile_COMMON`:
    - lambert when fully rough, else phong with shininess `2/r⁴−2`;
    - diffuse colour **or** texture (the profile cannot multiply them);
    - emission, and `A_ONE` transparency for BLEND;
    - double-sided and wrap modes through the extras readers honour.
  - Skins are hoisted to identity roots, with transposed inverse bind matrices and normalised
    weights. Animation is one matrix channel per node, sampled at 30 fps plus every source
    key, with STEP emulated by an extra key 1e-4 s before each step (three.js linearises STEP
    on matrix channels).
  - Every clip is an `<animation_clip>` starting at 0, because three.js takes `end − start`
    as a clip's duration, and every animated node has a key at 0 holding its first pose.
  - A scale of exactly zero, on a node or in a key, is written as a millionth: three.js
    turns a singular matrix into unit scale, so a hidden part would show full size.
  - Textures are `textures/*` sidecars in package mode. A single-file document carries none,
    and the loss is reported.
- **Colour, by measurement:**
  - Material `<color>` is **sRGB**: three.js r183 loads 0.5 as 0.214, and SceneKit renders
    it as 127.
  - Vertex `COLOR` is **linear**, inside `<vertices>`: three.js reads it as-is there, and
    SceneKit and assimp treat it as linear.
- **Morph targets are read, not written.** three.js's ColladaLoader fails the whole load on
  any morph controller (`buildNode` reads `controller.skin.joints`), and assimp 6.0's morph
  import has a loop bug. So `prepare` bakes morphs at their effective weights and reports the
  loss. The reader handles NORMALIZED and RELATIVE morphs, skin over morph, and weight
  animation; it was checked against SceneKit on six morph files.
- **Reading** also covers:
  - triangles, polylist, polygons, trifans and tristrips (concave n-gons up to 64 corners
    ear-clipped);
  - multi-index inputs, `<vertices>` indirection, up to 8 UV sets with `bind_vertex_input`;
  - unit and X/Z-up as one root correction, and `instance_node` and `library_nodes` with
    cycles refused;
  - skins with the bind-shape matrix folded into the inverse binds;
  - LINEAR/STEP/BEZIER/HERMITE channels on matrices, elements and components, resampled to
    TRS;
  - animation clips.

  `<skew>`, holes (`<ph>`), B-splines and cross-document references are refused by name.
- **Oracles.** three.js's ColladaLoader rendered every mode for all cases (static frames
  0.00 %, animated frames ≤ 0.19 %, from resampling the cubic channel). SceneKit confirmed
  silhouettes (≤ 0.32 %) and albedo (≤ 0.83 %), including the animated skin. Where
  three.js misreads valid Collada (`bind_vertex_input`, primitive-level `COLOR` through an
  off-by-one, any morph), the native checks defer to SceneKit, citing the loader's source
  lines.

  SceneKit has a limitation of its own. It binds animation data to its target rather than
  to the clip that instances it. So when two clips animate the same node, every clip carries
  the last clip's keys over the first clip's duration. On the Khronos Fox, the "Survey" clip
  held "Run"'s keys and "Walk" came out empty. three.js plays each clip correctly. For such
  files SceneKit judges only the rest pose.

## 8. How it is proven

`scripts/mesh-verify/README.md` has the mechanics. In short, every direction (A → B, plus
the scene written directly as B) for four fixtures:

| fixture | built to catch |
|---|---|
| **textured** | flips, swapped axes, lost transforms, lost mirrors, reversed winding, lost per-part materials: an asymmetric texture, a non-uniformly scaled wedge, a vertex-coloured panel, a mirrored instance |
| **skinned** | interpolation errors between keyframes: a banded tube on a three-joint chain, bent by LINEAR, STEP and CUBICSPLINE channels |
| **morph** | wrong morph targets or weight animation: two targets, one with normal offsets, animated weights |
| **cube** | the original flat, attribute-complete fixture |

Each output is rendered by **three.js r183's own loader for its format** (the engine behind
the dashboard's `<model-viewer>`) and compared pixel by pixel with the expected picture
(`to.prepare(from.prepare(fixture))` via GLTFLoader). The comparison runs in four unlit
modes (geometry, normals, albedo, UV), at six sample times between keyframes. Passing
cases are 0.00 % apart.

Second opinions:

- **SceneKit** (Apple's renderer via Model I/O, the Quick Look stack);
- **usdrecord** (Pixar's Hydra) and **usdchecker --arkit** for USD;
- **assimp** for every format it imports;
- the **Khronos glTF-Validator** for glTF.

Inputs beyond the fixtures, each run through every direction, with the original also
rendered by its own loader and compared with our reading of it:

- **Khronos glTF samples:** ten GLBs (skinned and animated characters, morph targets, the
  interpolation test, multi-UV, vertex colour, orientation) and three glTFs that keep
  their buffers and images in separate files.
- **USD written by other tools:** twelve files, which are litegen's fixtures as written by
  Apple's packager and glTF importer and by three.js's USDZ exporter, plus composition
  tests.
- **Real vendor output:** fal's public example files for Meshy v6 (one model as FBX, USDZ,
  OBJ and GLB), Hyper3D Rodin, Tencent Hunyuan3D v2 and v2.1, and Tripo v2.5 (up to
  215,758 triangles). Rodin's 500,000-triangle example was converted into all ten formats
  and read back outside the render run, to keep the run's disk use down.

Third-party files are used locally only; their licences vary.

### Results, 2026-10-12

| set | models | cases | result |
|---|---|---|---|
| litegen's fixtures | 4 | 440 | 440 pass |
| Khronos glTF samples (GLB) | 10 | 1,100 | 1,100 pass; 10 of 10 originals match our reading |
| Khronos glTF samples (separate files) | 3 | 330 | 330 pass; 3 of 3 |
| USD from other tools | 12 | 1,320 | 1,302 pass, 18 rightly refused; 12 of 12 |
| Vendor output (fal's examples) | 8 | 880 | 880 pass; 8 of 8 |
| **total** | **37** | **4,070** | **4,052 pass, 18 refused, none fail** |

The 18 refusals are Apple's skinned USDZ written as 3MF. Its rest pose has no area, so
no 3MF of it exists, and the expected scene cannot be written either.

Behind those verdicts:

- 18,196 three.js picture comparisons, 6,166 SceneKit, 3,808 usdrecord, and 606 on
  `usdcat` crate copies.
- 812 glTF files validated with no errors. The 84 warnings are all
  `MESH_PRIMITIVE_GENERATED_TANGENT_SPACE`, inherited from vendor and USD sources whose
  normal maps come without tangents.
- 812 USD files accepted by `usdchecker --arkit`.
- assimp imported 3,207 files. The 33 it refused are the collapsed Apple file, whose
  expected GLB it refuses alike.
- 4,846 comparisons skipped with a stated reason. 1,724 of them are because the source has
  no UVs or normals; each of the rest is a reader limitation listed in the harness README,
  and another oracle judges the same data.

`cargo test` adds 585 unit tests in `mesh/`, including the scene matrix: every fixture,
every pair, both write modes, at rest and at every sample time.

Plain `cargo test` (no browser) runs the same matrix structurally in `scene_matrix_tests.rs`.
For each pair it compares the posed scene's triangle count, area, signed volume (which
flips under a mirror or reversed winding), bounds and centroid at every sample time. It also
checks that animation, skins, morphs, textures and vertex colours survive exactly where the
capabilities say they should.

### Bugs this found

In code that already shipped:

- **PLY colours were written as linear values in sRGB bytes.** Every reader (three.js,
  MeshLab, Blender) decodes PLY colour as sRGB, so every GLB→PLY conversion rendered too
  dark: linear 0.5 was stored as byte 128 and displayed as linear 0.216.
- **PLY flattened every part to one colour.** The first non-white material's colour was
  painted on the whole model. A textured box beside an orange wedge came out entirely
  orange.
- **glTF sampler modes were collapsed.** `NEAREST_MIPMAP_LINEAR` was written back as
  `NEAREST_MIPMAP_NEAREST`, so even GLB→GLB changed texture filtering. This was found on
  Khronos's CesiumMan.
- **Binary input that was not PLY was called malformed PLY.** The magic was checked after
  UTF-8 decoding, so any binary file failed as `Malformed` instead of `NotThisFormat`.

In the new codecs, before they shipped:

- **Textures from vendor FBX files came out 40 % darker.** The FBX reader multiplied
  `DiffuseColor` into the map beside it, as three.js does. Blender's exporter writes its
  socket default, 0.8, there, which decodes to linear 0.6. Meshy's GLB of the same model
  (base colour factor 1.0) showed the intent: the map replaces the colour, as the SDK,
  Maya and Blender read it. This mattered at once, because for Rodin `[glb, fbx]` litegen
  now derives the GLB from the vendor's FBX.
- **FBX rotations strayed between keys near gimbal lock.** Our FBX reader's reading of
  CesiumMan, written to FBX, was 2.5° off on the right leg at t = 1.178. XYZ Euler angles
  are singular at Y = ±90°, and the leg turns about 96° there. FBX readers (the SDK, so
  Maya, 3ds Max, Unity and Unreal; Blender; ours) interpolate each Euler angle on its own,
  while the scene's rotation follows the arc. Within one 30 fps step the two part by
  degrees. three.js converts each key to a quaternion and slerps, so it never showed the
  error. The writer now adds keys until every segment is within 0.05°. A regression test
  replays four of CesiumMan's keys: 1.10° off before the fix, 0.00° after.
- **Collada clips stopped short in three.js.** A clip's `start` was its first key, which
  the spec allows, but three.js takes `end − start` as the duration. CesiumMan, whose keys
  begin at 1/24 s, stopped 1/24 s early. Clips now start at 0.
- **A part scaled to zero counted as shear.** `math::decompose` refused singular matrices.
  So a node hidden at scale 0 (the hip of Apple's skinned USDZ at rest) could not be
  written to FBX after a Collada round trip. A USD op stack animated from scale 0 was also
  dropped, with a "sheared transform" warning. Decomposition now handles degenerate
  scales exactly, for every reader, and only true shear is refused.
- **Our FBX reader refused our own 500k-triangle FBX.** Each compressed array was charged
  to the decode budget twice, once inflated and once decoded, which halved the budget.
  Found on Rodin's 500k-triangle example output; a test now pins the accounting.
- **Clips that start late played early in SceneKit.** It starts a Collada clip, and a USD
  skeleton's clip, at the first key. CesiumMan's keys begin at 1/24 s, so it drew 1/24 s
  ahead. Collada and FBX now key every animated node at 0, and USD keys its
  SkelAnimation there. USD xformOps are the exception: a key at 0 makes SceneKit stretch
  them (BoxAnimated's rotation held still until 1.6 s).
- **Baked Collada matrices came back as three animated channels per node,** even where
  only one moved. Through USD, SceneKit then bakes the op stack into one matrix sampled
  at about 120 Hz, and lost InterpolationTest's last step. Channels that never move
  their node are now dropped before USD is written.
- **A part scaled to zero showed full size in three.js's Collada and FBX loaders,** which
  decompose node transforms and turn a singular matrix into unit scale. Zero scales are
  now written as a millionth in both.
- **OBJ's `usemtl` is sticky.** A part with no material, written after a red one, rendered
  red, because the writer omitted `usemtl`. Every primitive now names a material.
- **Baking a texture transform could move another texture.** When two maps shared a UV set
  and only one was transformed, the transform was baked into the set the untransformed map
  also read. The first map to reference a set now claims it.
- **`linear_to_srgb(1.0)` was 0.99999994** in f32, and text formats wrote it out as such.
  The transfer is now computed in f64 with its endpoints pinned.

In the harness:

- **A texture race.** MTL, FBX, Collada and 3MF loaders return before their textures
  decode, so identical files passed in one run and failed in the next. The renderer now
  waits for every texture.
- **The uv mode flipped V in place and never flipped it back,** so every later frame of a
  V-up file sampled its texture upside down. The flip is now kept per attribute and
  undone.
- **SceneKit's "rest" picture was the first frame of the animation,** and SceneKit played
  every clip of a file at once. It now removes animations for the rest pose and plays only
  the clip being compared.
- **three.js split one USD timeline into two clips,** so the harness played the skeleton
  and left every animated xform at rest. All of a USD file's clips now play together.

## 9. Not supported, and why

- **Draco and meshopt compressed glTF:** refused by name (`Unsupported`). Each is a
  separate decoder with its own attack surface; no vendor in the catalog was seen emitting
  them.
- **KTX2/WebP to PNG/JPEG transcoding:** such textures are carried into glTF but dropped,
  with a reported loss, for containers that accept only PNG/JPEG. Decoding images is out of
  scope for the module.
- **`.blend`, Alembic, X3D/VRML, STEP:** no meaningful demand from AI-3D consumers; each is
  a large codec.
- **USDC writing:** the root layer is USDA (§5); USDC is read.
- **Multi-file delivery through the proxy:** `obj` + `mtl` + textures and `dae` + images
  need rehosting to keep relative names together under the generation prefix. The proxy
  therefore delivers single files today, and the losses say what that costs.
