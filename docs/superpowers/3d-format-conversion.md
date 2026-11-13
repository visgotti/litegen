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
  vertex colours, each primitive triangles, line segments or points (glTF's modes 4, 1 and
  0; mesh-gaps item 10, 2026-11-08), so a scan's point cloud and a model's wireframe or
  measurement lines are carried where the container can hold them;
- metallic-roughness materials with embedded PNG/JPEG textures and texture transforms, and
  over them the layers of glTF's nine PBR extensions (clearcoat, transmission, volume, ior,
  specular, sheen, iridescence, anisotropy and dispersion), with their textures;
- skins with inverse bind matrices;
- morph targets;
- keyframed animation of node transforms and morph weights (LINEAR, STEP, CUBICSPLINE);
- glTF's punctual lights (directional, point and spot) and cameras (perspective and
  orthographic), placed by nodes, so the nodes' transforms and animation move them. The
  values are glTF's: candela, or lux for a directional light; half-angle cones and `yfov`
  in radians; lights shine and cameras look along −Z, with +Y up. Each codec converts them
  at its own boundary (§3–§7). This is mesh-gaps item 5 (2026-11-06); its rulings ("Ruling
  R23" and the like) are listed with their costs in
  [`plans/2026-11-04-mesh-lights-cameras.md`](plans/2026-11-04-mesh-lights-cameras.md).

It reads and writes eleven containers. Every pair of them is a supported direction.

| container | read | write (single file) | write (package) |
|---|---|---|---|
| glb | full scene | full scene | — |
| gltf | full scene, external `.bin` and images via sidecars | full scene, buffer embedded | `.gltf` + `.bin` + images |
| obj (+mtl) | objects and groups (`o`, `g`), per-material primitives, MTL (Kd/Ke/d/Ns/Pr/Pm, `map_Kd`/`map_Ke`/`norm` with `-o`/`-s`), vertex colours, `l` polylines and `p` points | world-space geometry, one `l a b` per segment and `p` per point, each kind in an object of its own; material colour baked into vertex colour | `.obj` + `.mtl` + textures |
| stl | ASCII and binary, Materialise facet colours (`COLOR=`) | geometry (triangles only) | — |
| ply | ASCII and binary (both byte orders), per-vertex and per-face colour, UVs (`s`/`t` and three.js's aliases), faces and triangle strips, `edge` elements as lines, a file with no faces as points | geometry, normals, UVs (`s`/`t`), per-vertex colour, lines as `edge`, points when there are no faces or edges | — |
| fbx | §4 | §4 | — |
| usdz / usda / usdc | §5 | §5 | usda or usdc + textures |
| 3mf | §6 | §6 | — |
| dae | §7 | §7 without images | `.dae` + images |

Both production paths ship one file per container by default, as the "single file" column
says. On request they ship the package as one zip of it (`mesh::zip::write_package`: the
files at their package paths, primary first, deterministic): the converter's `package=zip`,
and a 3D generation's `output_packages: true`, which adds an asset of kind `package`
(`package_<format>.zip`) for every requested container whose package is more than one file.
An OBJ package keeps its materials and textures, and Collada, USDA and USDC packages their
textures, where the one file loses them.

What each target keeps is a **capability set** (`MeshFormat::capabilities`), and what a
writer encodes is exactly `MeshFormat::prepare(scene)`. Everything a target cannot hold is
reported as a `Loss` (`mesh::write_scene` → `SceneWrite::losses`), computed from what the
scene actually contains: a mesh with no normals, materials or UVs loses nothing to STL,
while an animated character reports the animation, the skin and the materials by name.
A scene's lights and cameras are reported (`lights`, `cameras`) by every container that
holds none: OBJ, PLY, STL and 3MF. The containers that hold them report what other readers
lose: Collada and USD a light's range (`lights`), which has no form there and rides only in
litegen's own annotation (Ruling R27); FBX, USD and Collada what a camera cannot say
(`cameras`, §4, §5, §7).
A scene's points and lines are reported (`points`, `lines`), by primitive, by every container
that holds none. glTF, USD and OBJ hold both; PLY holds lines as `edge` and points only in a
file with neither faces nor edges (its content rule, `ply::losses`); Collada holds lines and
no points (1.4.1 has no point primitive); FBX, 3MF and STL hold neither (§3–§7). A scene
with nothing the target holds, a point cloud to STL, is refused by name, as `mesh_unsupported`
("stl holds triangles only; this scene has 9 points and no triangles"), where it used to be
`mesh_empty`. A file of points alone is a valid input to every container that holds points.
STL, OBJ and PLY carry no units or up axis and are written in glTF's frame (metres, +Y up);
the converter's `units`/`up` (and `from_units`/`from_up`) say otherwise for them (§10).

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
  `serde_json` is used by glb.rs, `miniz_oxide` by zip.rs and fbx.rs, `quick-xml` by
  xml.rs, `ruzstd` by ktx2.rs (Zstandard-supercompressed KTX2 levels), and `image` by
  normalize.rs (WebP to PNG, encoding a decoded KTX2 as PNG,
  packing separate metallic and roughness maps, a dropped map's channel means).
  `isolation_tests.rs` enforces this.

### Memory and limits

What a conversion holds grows with its input. Peak heap, counting the input itself, measured
on seven vendor meshes of 1.3–26 MB (`examples/mesh_peak.rs`, 2026-11-01):

| Step | Peak heap | At the 128 MB input cap |
|---|---|---|
| reading | 2–9 × the input (a USDZ decodes its textures) | up to ~1.1 GB |
| one conversion: input, scene and one output alive | 5–13 × | ~1.7 GB |
| all ten containers written from one read and held | 12–31 × | ~4 GB |
| delivery with `ktx2_textures=keep`, a KTX2 image and a glb or gltf requested | one scene copy more than the same delivery without `keep`, taken only then (its KTX2 images are smaller than the PNGs the other copy holds); not measured | not measured |

```text
cargo run --release --example mesh_peak -- <file>...
```

The bounds that make this a ceiling rather than a trend:

- `MAX_INPUT_BYTES` (128 MiB) on every input, and the scene caps (4M vertices, 12M indices,
  100k nodes, 20M animation keys, …) on what any reader builds.
- `MAX_EXPANDED_BYTES` (1 GiB) on what a compressed container expands to while it is read:
  FBX's zlib arrays and a zip package's entries. It is sized so that the scene caps, not this
  budget, bound reading litegen's own files: litegen's FBX of a dense textured mesh at the
  caps decodes to ~0.83 GB, and its 3MF inflates to ~0.5 GB (the test
  `our_own_fbx_and_3mf_of_any_scene_within_the_caps_fit_their_expansion_budgets`, and a
  release-mode round trip at the caps, `every_container_reads_back_what_it_writes_at_the_scene_caps`).
  Text and uncompressed containers outgrow the 128 MB input cap first at the caps (GLB
  144 MB, OBJ 517 MB), which is that cap working.
- A texture decodes within 256 MiB, a 64M-pixel RGBA map (`normalize::decode_image`).
- The callers bound how many run at once: the convert endpoint's conversion slots
  (`available_parallelism`, 2–8), each held from before the upload is buffered until the
  converted file is sent, and the delivery slots (2–4), shared by a client's poll and the
  poller.

## 3. glTF, OBJ, PLY, STL

- **glTF (the hub)** reads everything the IR models:
  - normalised and quantised (`KHR_mesh_quantization`) attributes, and sparse accessors
    (zero-based morph targets);
  - Draco (`KHR_draco_mesh_compression`) and meshopt (`EXT_meshopt_compression`,
    `KHR_meshopt_compression`) compressed geometry, decoded on read since mesh-gaps item 2
    (2026-11-06): meshopt in every mode and filter, both attribute stream versions, a view
    decoded once however many accessors share it, and a view whose extension is only used
    decoded rather than read from its fallback (a KHR-only stream version or filter under
    the EXT name is read with a warning); Draco through the `draco-core` crate
    (`mesh/draco.rs`), a TRIANGLES or TRIANGLE_STRIP primitive whose stream's faces are
    read as a list, under absolute limits (the scene's vertex and index caps, and
    24,000,000 bytes of decoded attributes a primitive, the ceiling set to bound the
    decoder's memory on the shapes measured, strips and grids, with a faces-per-point
    pre-check still open), with a primitive in any mode but TRIANGLES or TRIANGLE_STRIP,
    a point cloud, a pre-2.2 bitstream and a stream past the limits refused by name, and
    an accessor whose `count` disagrees with the decode read at the decoded count with a
    warning. The output is never compressed:
    the writers write neither extension, so a delivered glTF requires none, and the
    vendor's compressed file is kept as `original`. Proven on Khronos's sixteen
    `glTF-Draco` and six meshopt variants and on gltfpack's output, each against three.js
    r183's own `DRACOLoader` and `MeshoptDecoder` (the native check of
    `scripts/mesh-verify`, 2026-11-08);
  - every `JOINTS_n`/`WEIGHTS_n` pair and all six sampler minification modes;
  - `KHR_texture_transform`, `KHR_materials_emissive_strength` and `KHR_materials_unlit`;
  - the nine PBR layer extensions, `KHR_materials_clearcoat`, `_transmission`, `_volume`,
    `_ior`, `_specular`, `_sheen`, `_iridescence`, `_anisotropy` and `_dispersion`, with
    their textures, so GLB → GLB and glTF → glTF keep every layer. The writer adds each
    layer it writes to `extensionsUsed`, never to `extensionsRequired`, so a viewer
    without one still draws the base layer. A layer whose own JSON does not read (a field
    of the wrong type or past f32's range; an `ior` that is neither 0 nor at least 1, as
    the schema requires; a malformed texture reference or one past `textures`; a value
    that is not an object) is dropped with a warning naming the extension, and the field
    when one is at fault, and the rest of the file converts. So is a layer whose texture
    the document cannot give (a `source` that is no index, an image or a sampler index out
    of range): before the layers were read such a file converted, and it still does. Such
    a texture still refuses the file where a base map uses it. A refused value is quoted
    cut to 60 characters. Values are otherwise kept as the file states them, out of range
    ones too, as base factors are;
  - WebP/KTX2 images, carried as-is by the library. Generation delivery and the standalone
    converter first re-encode both as PNG (`normalize::transcode_textures`), so no container
    drops them and, by default, no glTF requires `EXT_texture_webp` or `KHR_texture_basisu`
    (`ktx2_textures=keep`, below, leaves a KTX2 in a glb or gltf, which then requires
    `KHR_texture_basisu`) (mesh-gaps item 3, 2026-11-08). A KTX2 (ETC1S/BasisLZ, UASTC LDR 4x4 or uncompressed RGBA8,
    Zstandard-supercompressed or not; `ktx2::decode_rgba8`) becomes a PNG of the values it
    stores, level 0 only, with no colour conversion: the Data Format Descriptor's colour
    tags are never applied, so a PNG carries what a viewer's own KTX2 loader would read from
    the file, and a base-colour map and a normal map both keep their bytes. The PNG is RGB
    when every texel is opaque and RGBA otherwise. One image is transcoded once, however
    many textures sample it. A KTX2 that does not decode (cube maps, arrays, 3D or 1D
    textures, a hardware-compressed `vkFormat`, ZLIB supercompression, a malformed file) is
    left out of the materials that use it, named as an `image_formats` loss with the
    decoder's reason (the converter's losses header; a delivered mesh's manifest notes), and
    the vendor's file, delivered beside as `original`, keeps it. Transcoding never takes a
    scene's encoded images past 64 MiB (`TEXTURE_BUDGET`, half of the input cap); images
    that are already PNG, JPEG or WebP are not dropped for it. The PNG and JPEG bytes
    already there count first, then each WebP's PNG, then each KTX2's one at a time in
    image order, and from the first KTX2 whose PNG would pass the budget every later KTX2 is
    left out and named the same way ("transcoding it would pass the 64 MiB texture budget").
    The opt-in `ktx2_textures=keep` (the converter's query parameter and the 3D request's
    field; `png` is the default) keeps the KTX2 bytes in a **glb or gltf** output, through
    `KHR_texture_basisu`, which the file then requires, for viewers that upload KTX2 to the
    GPU (three.js with KTX2Loader, Babylon.js, Godot); WebP still becomes PNG there, kept
    KTX2 counts nowhere against the PNG budget, and every other container still gets PNG.
    Delivery records the choice in the row's delivery plan (`delivery.ktx2_textures`) so
    the poller delivers what the client's own poll would;
  - points and lines (mesh-gaps item 10): POINTS (mode 0) is a points primitive, and LINES,
    LINE_LOOP and LINE_STRIP (modes 1 to 3) a lines primitive, with a triangle primitive's
    attributes, material, skin and morph targets. A loop of n vertices expands to n
    segments and a strip to n − 1, charged to the index budget at their expanded size before
    anything is allocated, so a huge strip is refused as a huge triangle fan is. The writer
    writes modes 0 and 1, never a loop or a strip. A Draco primitive can be neither (above);
  - core `cameras` and `KHR_lights_punctual` lights, as they are, since the scene keeps
    glTF's own values (§1), the cameras in the file's order, as GLTFLoader's
    `gltf.cameras` keeps them. A camera's view is scale-free: glTF derives it "from the
    rotation and translation components of the node's transformation matrix" (glTF 2.0,
    Cameras, View Matrix). So a camera under a scaled ancestor (FBX's centimetre root,
    Collada's `<unit>`, USD's `metersPerUnit`, `normalize::place`) is written where it
    stands, and three.js r183 views it correctly: its `Camera` excludes the world scale
    from the view matrix ("to be glTF conform", `src/cameras/Camera.js` L116-L126; Ruling
    R33). An engine that applies node scale to a camera would differ, seeing that
    camera's view scaled, near and far planes with it. The writer puts
    `KHR_lights_punctual` in `extensionsUsed`, never `extensionsRequired`, so a viewer
    without it still draws the model. A light or camera that does not read, or that
    breaks a bound of glTF's schemas, is dropped with a warning naming it and the bound:
    a colour component below 0, a negative intensity, a
    range that is not positive, a spot's inner cone below 0 or not below its outer cone, a
    `yfov` outside 0 to π, an aspect ratio that is not positive, a perspective `znear`
    that is not positive, an orthographic `xmag` or `ymag` of 0 or a negative orthographic
    `znear`, a `zfar` short of `znear`, or a value that is not finite. So is one that a
    node places past the file's array, or that no node places (Ruling R13). Its node
    stays: a reader never refuses a file over a light or camera. Three are kept instead,
    with a warning, as every reader keeps them: a colour past 1 has its peak moved into
    the intensity (Ruling R8; FBX, Collada and USD readers do that unwarned, as their
    colours hold colour × intensity legally); a spot wider than a hemisphere, its outer
    cone past π/2, is read as a point light (Ruling R32); and a hard-edged spot (equal
    cones, which glTF forbids and FBX, Collada and USD files carry) is softened, its inner
    cone moved to `outer × (1 − 1e-4)` (Ruling R5). A directional light's `range`, which
    glTF ignores, is dropped with a warning, and `prepare` drops one from a scene it
    writes, unreported, as the writer leaves it out (Ruling R13).

  All previous hostile-input bounds are kept, plus a decode budget, so that
  aliasing accessors cannot amplify a small file into gigabytes. The writer validates clean
  against the Khronos glTF-Validator: 0 errors and 0 warnings on every file of the
  harness's every-fixture run. An `--import` run's outputs are held to 0 errors, since a
  third-party source can carry warnings of its own into them. It also moves skinned
  meshes to untransformed roots, because three.js
  multiplies a skinned node's transform in where the spec says to ignore it. A skin the
  scene names no skeleton for gets its joints' closest common root as `skeleton` (glTF 2.0
  §5.28.2). So does a skin whose named skeleton isn't an ancestor of every joint, which
  glTF forbids. A skin whose joints hang from different roots gets no `skeleton`.
- **OBJ**:
  - each `o` becomes a mesh and each `usemtl` a primitive; MTL libraries and textures
    resolve through sidecars;
  - `v x y z r g b` vertex colours are read and written;
  - colours (`Kd`, `Ke`, vertex) are **sRGB**, as three.js, MeshLab and Unity read them.
    Blender reads `Kd` as linear: a real ecosystem disagreement, documented in `obj.rs`;
  - one uv set, which in a package is the one the textures sample (base colour's first), so
    a texture on a later set survives; the other sets are reported lost;
  - a package's MTL keeps four of glTF's layers, as factors, through the PBR extension
    Blender reads and writes, in Blender's units (`obj.rs` cites its v5.2.3 importer and
    exporter): `Pc` is four times the clearcoat factor, `Pcr` its roughness, `Ps` the sheen
    (one value a grey sheen colour, three the colour itself; written as `Ps w w w` from the
    brightest channel, so a coloured sheen is reported), and `aniso` and `anisor` the
    anisotropy's strength and its rotation as a fraction of a turn. A layer exists only
    above its weight's off value, and what the file leaves out is Blender's default: a coat
    roughness of 0.03, and a sheen roughness of 0.5, which MTL has no key for, so another
    one is reported. A value outside its glTF field's range is dropped with a warning on
    reading and reported on writing, never clamped. Layer maps are dropped and reported:
    the extension has no clearcoat or anisotropy map, and its `map_Ps` holds a weight, not
    glTF's colour, so the reader drops one with a warning. One file has no MTL, and keeps
    no layer;
  - `Ni` is the ior in a PBR MTL only, one with any of `Pr`, `Pm`, `Ps`, `Pc`, `Pcr`,
    `aniso`, `anisor`, `map_Pr` or `map_Pm`: classic exporters write `Ni 1`, which as an
    ior would take a dielectric's reflection away. There 1, 1.45 and 1.5 read as unset,
    since Blender writes them by default (1 without a Principled BSDF, 1.45 as 2.8–3.6's
    Principled default, 1.5 from 4.0, which is glTF's own), and 0 and below, or between 0
    and 1, are left out with a warning. The writer leaves an ior of 0, 1 or 1.45 out and
    reports it, and 1.5 out unreported, as it draws the same;
  - a PBR MTL's transmission weight (`Tf`, or `Kt`, which Blender reads as `Tf`) is named
    in a warning and not carried, as OBJ keeps no transmission; a classic MTL's `Tf` and
    `Kt` stay unread;
  - a package writes a BLEND material whose alpha lives in its base-colour texture alone
    as opaque, and reports it: MTL's `d` is the colour's alpha;
  - a single-file OBJ (what the proxy delivers) bakes material colour into vertex colour
    rather than referencing an `.mtl` that will not be there;
  - points and lines (item 10): `p v…` is a point for each vertex it names, and `l v…` the
    polyline through its vertices (a segment per pair), both read beside the object's faces
    and charged to the same index budget, with `v/vt` spellings read as a face corner's and
    colours from `v x y z r g b`. The writer writes one `l a b` per segment, never a
    polyline (three.js's OBJLoader pushes a polyline's vertices straight into
    `LineSegments`, so `l 1 2 3 4` would lose 2–3), each kind in an object of its own (one
    `l` or `p` turns the whole object it is in into lines or points there), with a
    `usemtl` of its own kind (`faces`, `lines`, `points`) in a file with no MTL, where
    three.js caches a material by its name alone. An `l` takes `v/vt` and no normal, and
    a `p` neither, so a line's normals and a point's normals and texture coordinates are
    left out and reported (`normals`, `uvs`). A point with no coordinate has nothing to
    sample a map by (three.js smears the map across each point sprite, and glTF refuses a
    map on a primitive with no `TEXCOORD`), so a package draws a points primitive with an
    untextured copy of its material (`paint_1` beside `paint`: factors kept, no map), or
    strips the material itself where only points use it, and reports the bindings
    (`textures`). A package of a scene with no materials gets an MTL of the white materials
    that name the kinds, where it wrote none;
  - a read lists 64 distinct warnings and counts the rest in a last line, as 3MF's does,
    and a warning quotes a name or path from the files cut to 256 characters.
- **PLY:** per-vertex colour in both directions, encoded **sRGB** (three.js, MeshLab and
  Blender). This fixed a bug: see §8. `binary_big_endian` is read too; the writer stays
  `binary_little_endian`, because every reader takes it. PLY keeps no material layer, and
  reports each. Points and lines (item 10): an `edge` element (`vertex1`, `vertex2`,
  optionally `red`/`green`/`blue`) is lines, and a file with no faces and no edges is a point
  cloud, a point per vertex with whatever the vertices carry (a file that used to read as
  `mesh_empty`: Stanford's `drill_1.6mm_0_cyb.ply` reads as its 4,223 points). A
  Gaussian-splat PLY reads the same way, and the splat properties (`f_dc_*`, `f_rest_*`,
  `opacity`, `scale_*`, `rot_*`) are named in one warning and not carried. PLY has one pool
  of vertices, so the writer puts triangles, lines and points in it in that order, writes
  `face` and `edge` elements, and keeps points only in a scene with neither triangles nor
  lines, where points beside them would read back as nothing: `ply::losses` names the points
  it leaves out.
- **STL:** geometry only; colour, UVs, normals and material layers are reported as lost.
  Materialise's facet colours behind a `COLOR=` header are read, as three.js reads them.
  Triangles only: points and lines are reported (`points`, `lines`), and a scene of nothing
  else is refused by name (§1).
- **Lights and cameras:** OBJ, PLY and STL hold neither, so each light and camera is
  reported (`lights`, `cameras`) and left out, as 3MF does (§6).

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
    disagree about a colour beside a map (next bullet). The specular colour is glTF's F0
    (0.04, or the base colour for a metal) faded by `(1 − roughness)²`, all in
    `SpecularColor`, which is what three.js reads, so a fully rough surface has no
    highlight rather than a white wash; OBJ's `Ks` is the same;
  - no material layer: FBX output stays Phong with Blender's PBR mapping, the form every
    FBX reader understands, so each layer a scene has is reported lost, its base layer kept;
  - normals, tangents and binormals `ByPolygonVertex`/`IndexToDirect`, each distinct value
    once, as UVs and colours are: per-corner doubles cost 24 bytes a corner each, which put
    litegen's own textured FBX past its own reader above ~1.92M triangles;
  - uv sets ordered so the ones its kept maps sample come first (base colour's, then the
    normal map's and emission's), with the bindings renumbered, because three.js's
    FBXLoader samples every texture from set 0;
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
  - material layers from three DCC families: Arnold's aiStandardSurface and Maya's
    Standard Surface (`Maya|coat`, `Maya|coatRoughness`, `Maya|transmission`,
    `Maya|specularIOR`, `Maya|specular`, `Maya|sheen`, `Maya|thinFilmThickness`, …),
    3ds Max Physical (`3dsMax|Parameters|coating`, `transparency`, `trans_ior`,
    `reflectivity`, …) and 3ds Max OpenPBR (`3dsMax|Parameters|coat_weight`,
    `transmission_weight`, `specular_ior`, `thin_film_*`, …). Names come from real exports
    in ufbx v0.23.1's test data, and meanings and units from the Standard Surface and
    OpenPBR specifications and Autodesk's Physical Material reference; a property is
    mapped only where those fix it (`LayerNames` in `fbx_read.rs`, with a test per
    family). So all three give clearcoat, transmission and ior; Standard Surface and
    Physical give specular, Standard Surface gives sheen, and the thin films of Standard
    Surface and OpenPBR give iridescence. A layer is read when its weight is above 0
    (specular when it is other than 1), so a material at its family's defaults reads none,
    and an ior of 1.5 or of the family's documented default (Standard Surface's and
    Physical's 1.52) reads as unset. Warned about and left out, since glTF has no slot of
    the same meaning: anisotropy; a solid surface's refraction, absorption and dispersion
    (glTF's volume needs a thickness, which none of the three stores); a transmission
    roughness of its own; OpenPBR's fuzz; a coat's colour, anisotropy, ior and effect on
    the layers under it, where they differ from glTF's or the family's default; the
    specular colours and OpenPBR's specular weight; a transmission tint other than the
    base colour, which glTF tints with; and textures on layer properties. Stingray,
    3ds Max's PBR material and Phong read no layer;
  - skins (inverse bind = cluster `Transform`) and blend shapes, including **in-between
    shapes**, exact against Maya's own exported frames;
  - every animation stack, each its own clip (the active one first), with GlobalSettings
    axes and units applied as one root correction; a take keyed before 0 starts at 0.

  **Curves follow ufbx's reconstruction of the SDK's evaluation**: auto/clamped/TCB/user
  tangents and weighted handles, checked against Maya and MotionBuilder reference values on
  37 files, with a maximum error of 1.1e-3 and most under 1e-5. A 3ds Max skinned file
  matches Max's own OBJ exports at two frames to within 6e-5 of model size, where Assimp is
  9.7 % off because it interpolates cubic keys linearly.
- **Lights and cameras** are `NodeAttribute`s of class `Light` and `Camera`, each on a
  model of its class. The authority is the Autodesk SDK's convention as ufbx v0.23.1 and
  Blender 5.2.3's importer and exporter implement it, never three.js's FBXLoader;
  `fbx.rs`, `fbx_write.rs` and `fbx_read.rs` cite the line behind each rule.
  - **Axes.** FBX aims a light along its model's −Y, and a camera along its +X with +Y up,
    where the scene aims both along −Z. A fixed quarter turn (Blender's
    `MAT_CONVERT_LIGHT`, π/2 about X, and `MAT_CONVERT_CAMERA`, π/2 about Y) goes on the
    model's rotation and on every key, and its inverse on each child, so nothing else
    moves. An FBX model holds one thing, so a light or camera that shares its node with a
    mesh, a joint or the other is given a child node of its own.
  - **Lights:** `LightType` 0 point, 1 directional, 2 spot. `Color` follows the request's
    `fbx_color_space`, as material colours do (Ruling R14): sRGB by default, as three.js
    reads it, or linear, as Blender, ufbx and assimp read it. `Intensity` is 100 × the
    candela or lux, which ufbx and Blender both divide by 100, and `DecayType` 2 is the
    inverse square. Blender takes the quotient as its own radiometric watts (a sun's as
    W/m²), though, where its glTF importer converts candela and lux at 683 lm/W, with
    4π for a point or spot light (io_scene_gltf2 v5.2.39, `blender/imp/light.py`
    L58-L71, its default mode). So by default litegen's FBX lights open in Blender about
    54× (point and spot) and 683× (sun) brighter than the same scene's glTF, and Blender's
    own FBX lights read that much dimmer here: Blender's export of `staged` (Ruling R28)
    brought its 3 lux sun back as 0.0044. The opt-in `fbx_light_units` (a convert query and
    a field of the generation request, as `fbx_color_space` is; it applies when `from` or
    `to` is fbx, so a same-format conversion is a no-op) chooses the units:
    `photometric`, the default, is the output above, byte for byte, and what ufbx and
    three.js's FBXLoader read; `blender` writes `Intensity` = 100 × E, where E is the
    `energy` Blender's glTF importer gives the same light (lux / 683 for a sun, candela ×
    4π / 683 for a point or spot, whatever its cone). That is lux × 0.14641 for a sun and
    candela × 1.83988 for a point or spot, and it reads the other way: lux = 6.83 ×
    `Intensity`, candela = 0.54351 × `Intensity`. Blender's FBX importer sets `energy` to
    `Intensity` / 100 (`fbx_import.cc` L231, `ufbx.c` L22937 at its v5.2.0 tag), and its
    exporter writes `energy` × 100 × 2^`exposure` (`export_fbx_bin.py` L605) with no
    `Exposure` property, so the reader takes `Intensity` / 100 as the whole `energy` and
    reads no exposure. Real Blender 5.2.0 LTS confirmed both ends on 2026-11-08
    (`scripts/blender-light-units/probe.py`, its numbers in
    [`research/research-blender-light-units.md`](research/research-blender-light-units.md)):
    `staged` written with `blender` imports through `wm.fbx_import` with the `energy` of
    its glTF's import (sun 0.0043924, bulb 0.36798, spot 0.73595), and Blender's own FBX
    export of that glTF-imported scene reads back here as 3 lux, 20 cd and 40 cd, to 3e-8.
    (Blender's Python "Legacy" importer divides by 100 as well, but raises on any light in
    5.2.0.) A spot's `InnerAngle` and `OuterAngle` are full cones in degrees, twice the
    scene's half-angles, as Blender writes its `spot_size`. A range is the SDK's far attenuation
    (`EnableFarAttenuation`, `FarAttenuationEnd`), which litegen reads back and ufbx and
    Blender ignore. It fades the light linearly to nothing from the light itself
    (`FarAttenuationStart` 0), so it only approximates glTF's range, a cutoff renderers
    reach smoothly over the inverse square.
  - **Cameras:** `CameraProjectionType`. A perspective camera has its `FocalLength` on a
    film 35 mm across its wider side, with `ApertureMode` 3 and `GateFit` 2 as Blender
    writes them, so three.js's 35 mm film gauge gives the same yfov as ufbx and Blender;
    its aspect ratio as `AspectWidth` over `AspectHeight`; and `NearPlane` and `FarPlane`
    in metres. An orthographic camera's `OrthoZoom` is its full width over 30, the units
    ufbx reads from every exporter but Blender. FBX always stores an aspect ratio and a far
    plane, so an unset aspect ratio is written as 16:9 (Blender's default render size), an
    infinite far plane as 1000 m (Blender's default clip end, or 10,000 × a near plane at
    or past it), and a mirrored orthographic view unmirrored. Each is reported as a
    `cameras` loss, since nothing in the file says otherwise.
  - **Reading** takes the same rules back, from the attribute and then its class's
    `PropertyTemplate`, with ufbx's defaults where neither says. `HotSpot` and `ConeAngle`
    stand in for missing cones, yfov follows `ApertureMode` and the film's gate fit, and a
    Blender-written `OrthoZoom` is Blender's `ortho_scale`. A colour past 1 has its peak
    moved into the intensity (Ruling R8), a hard-edged spot is softened (Ruling R5), and
    a spot wider than a hemisphere (a cone past 180°) is read as a point light, with a
    warning (Ruling R32). A `DecayType` other than quadratic is warned about, and the
    light is read with the inverse square. Warned about and left out: area and volume
    lights, a non-black `AmbientColor` (Ruling R15), camera switchers and stereo rigs, and
    curves on a light's or camera's own properties.
  - **three.js's FBXLoader** (r183) is no judge here. It turns no camera, so a camera
    looks 90° off, and builds no orthographic one. It divides `NearPlane` and `FarPlane` by
    1000. It aims a light only at a `LookAtProperty` target, so ours shine at the origin.
    It reads `InnerAngle` as a half-angle, and decays lights linearly.
- **A bug the work found:** from 7.2 on, ASCII `KeyAttrDataFloat` stores each float as the
  integer of its bit pattern. Reading those as numbers turned every Maya/Max ASCII curve
  with tangents into garbage.
- **Not carried, and reported:** double-sided, alpha mask, unlit, mirrored wrap, texture
  filters, a tint beside a texture, and every animation layer after the first. A sheared
  transform is baked into the meshes below it, or approximated by its nearest TRS where
  something animated or a joint hangs from it (`pose::bake_shear`), and reported.
- **Points and lines** (mesh-gaps item 10): a `Geometry` of subclass `Line` is read as a
  lines primitive: `Points` and `PointsIndex`, whose entries run through the points in
  strips, a negative entry `-(e + 1)` naming point `e` and ending its strip as
  `PolygonVertexIndex` ends a polygon. A strip of c points is c − 1 segments, and a trailing
  strip with no end stops at the array's end, with no closing segment (assimp wraps it back
  to the first index; the mesh path refuses, Ruling I10-R10). It takes the node's transform,
  the geometry's own transform and the first material, and is charged to the index budget as
  the others are. Nothing is written: there is no portable FBX point type and line geometry
  is rarely imported (three.js's FBXLoader builds only `NurbsCurve`, and assimp only `Line`),
  so `points` and `lines` are reported. NURBS, patches and curves still warn.
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
  - Lights and cameras are judged by assimp's reading of every FBX written: the harness's
    numeric oracle, and the expected geometry drawn through and lit by the cameras and
    lights assimp read (§8). A committed FBX that assimp wrote of the `staged` fixture
    (`testdata/fbx/staged_assimp.fbx`; assimp writes FBX lights but no cameras, Ruling
    R10) pins the reader against assimp's own conventions.

## 5. USD / USDZ

`mesh/usd.rs`, with `usd_stage.rs` (composition), `usda.rs` (text), `usdc.rs` (binary
crate, read and written), `usd_read.rs` and `usd_write.rs`. There are three format names:
**usdz** (one file, textures inside), **usda** (a text layer) and **usdc** (the same layer
as a binary crate). A usda's and a usdc's textures are sidecars in package mode and a
reported loss in a single file. `usd` stays unparsed: it names either encoding, and a
format name must work in both directions and mean one encoding.

- **Write:** the layer is USDA text, and a usdc is that text parsed back into a layer and
  encoded as a crate (the next bullet).
  - USDZ follows Apple's package rules: a stored (uncompressed) zip, every file 64-byte
    aligned, the root layer first (`model.usda`, or `model.usdc` with `usdz_root=usdc`,
    below), sizes and CRC in the local headers, and textures under `textures/`.
    `usdchecker --arkit` accepts every file written. It has accepted a USDA root since
    OpenUSD 24.03 (verified with Apple USD Tools 0.25.2), and model-viewer ships exactly
    that to AR Quick Look.
  - **`usdz_root=usda|usdc`** chooses a USDZ's root layer. It is a converter query, which
    applies when `to` is usdz, and a field of the generation request, recorded in the
    `DeliveryPlan`. `usda`, the default, is the root a litegen USDZ has always had.
    `usdc` writes the crate as the root, as Apple's own packagers do: `model.usdc` first,
    still stored and 64-byte aligned. `usdchecker --arkit` accepted both roots on every
    fixture of the time (ten fixtures, 20 USDZ files), and the two read back as one scene.
  - **Errors for `usdz_root` and `usd_clips`.** On the converter, a word other than
    `usda` or `usdc` (or `all` or `first`) is 400 `param_invalid`; `usdz_root` when `to`
    is not usdz, and `usd_clips` when `to` is not usda, usdc or usdz, are 400
    `param_unsupported`; a repeat is `param_repeated`. On a generation request a bad word
    is 400 `malformed_request`, as the other typed options give it. A request that keeps
    nothing (`X-Litegen-Keep-Outputs: false`) has no container to apply them to, so it
    can't ask for `usdz_root=usdc` or `usd_clips=all`: 400 `option_needs_keeping`, while
    `usdz_root=usda` and `usd_clips=first` pass.
  - Stage metadata: `defaultPrim`, Y up, `metersPerUnit = 1`, and 30 time codes per second,
    because three.js's USDA parser assumes 30.
  - Nodes are Xforms with translate, orient and scale ops, time-sampled when animated.
    Prim names are sanitised and made unique; the original name is kept in `customData`
    and restored on read.
  - Meshes carry their uv sets as `st`, `st1` … `st7` (V flipped, `st` first), normals,
    `displayColor` and `displayOpacity`. A USDA holds eight sets. A USDZ holds two, as
    RealityKit does, and they are the sets its textures sample: `degrade` puts those
    first, base colour's set leading, before it keeps two.
    Materials are UsdPreviewSurface networks; texture transforms are baked into the uvs.
  - Material layers: UsdPreviewSurface's `clearcoat` and `clearcoatRoughness`, each a value
    or a channel of a `UsdUVTexture` (`r` for the clearcoat, `g` for its roughness) with the
    factor in the texture's scale, and `ior`, 0 included (UsdPreviewSurface's F0,
    ((1 − ior)/(1 + ior))², gives it glTF's meaning). A clearcoat's normal is the base
    normal map, as UsdPreviewSurface defines it, so `prepare` gives every clearcoat the
    base's map, or none, and a coat whose own normal differs is reported.
    UsdPreviewSurface's ior also sets the clearcoat's reflectance, where glTF's clearcoat
    always reflects with 1.5, so a clearcoat over another ior is reported and both are
    written as they are. An unmapped clearcoat of 0 and an ior of 1.5 read back unset
    (below), so `prepare` leaves them out, unreported, as they draw the same. The other
    seven layers have no input and are reported; the
    written opacity stays the base alpha, since `opacity = 1 − transmission` would make
    full glass invisible in every USD reader.
  - Skins are UsdSkel: Skeleton, SkelAnimation and joint influences. Morph targets are
    BlendShapes whose weights live in the SkelAnimation, so a morph-only mesh gets a
    skeleton with no joints. `prepare` folds a non-joint node between two joints into the
    child joint.
  - Animation clips (several, below). STEP keys become a hold plus a 1 ms ramp, and
    CUBICSPLINE is resampled at 30 fps. Channels that never move their node are dropped.
  - What the SkelAnimation carries is keyed from time 0, holding its first value, because
    SceneKit otherwise starts a skeleton's clip at its first sample. xformOps get no such
    key: with one, SceneKit stretches the op's animation (both measured, §8).
- **USDC writing** (`usdc::write`, called by `usd::write_usdc`; mesh-gaps item 6).
  `usd_write::write_layer` writes the USDA text, `usda::parse` reads it into a `Layer`,
  and `usdc::write(&Layer)` encodes that layer. The text and the crate cannot drift: the
  crate is the parsed text. A crate is a standalone layer, as a usda is.
  - **The layer keeps what a crate needs.** Both parsers (`usda.rs` and `usdc.rs`) fill
    each attribute's variability (`uniform`, `varying`), its metadata beyond
    `interpolation` and `elementSize`, and `token` apart from `string` (`Value::Token` and
    `Value::Tokens`, which `as_str` and `strs` also accept). Untyped layer metadata is
    typed by Sdf's schema (`defaultPrim`, `upAxis` and `colorSpace` are tokens,
    `metersPerUnit` and the time codes are floats), and a bare metadata string is
    `comment` and `doc` is `documentation`, as Sdf names them.
  - **Crate version 0.8.0**, the version `usdcat` writes by default. Nothing litegen
    writes needs a later one (time-code values are 0.9, path expressions 0.10, relocates
    0.11, splines 0.12 and up).
  - **Compression is as cheap as the format allows.** Arrays are uncompressed, with the
    compressed bit clear, which the reader takes. TOKENS is one chunk of literal-only LZ4,
    which is valid LZ4 and needs no match finder. STRINGS is raw. FIELDS, FIELDSETS, PATHS
    and SPECS are Sdf integer-coded, with FIELDS' value reps in the same LZ4. The output is
    deterministic: no hash map's order reaches the bytes.
  - **Types.** An attribute's crate type comes from its declared `type_name`. A metadata
    or dictionary value has no declared type, so its `Value` variant gives it, and a float
    there is a double: a `Layer` cannot say float from double, so litegen's own camera and
    light `customData` numbers are written as `double` and `double3` in the text too, and
    `usdcat` prints the same for both. A type the writer cannot write (`timecode`, which
    needs 0.9.0, or an unknown one) is `Unsupported` and named. There is no fallback type.
  - **Media type.** `application/octet-stream`: IANA's model registry (`model.csv`,
    checked 2026-11-10) lists `model/vnd.usda` and `model/vnd.usdz+zip` and no type for a
    binary crate, and litegen does not invent one.
  - **Checked** (Apple USD Tools 0.25.2, by hand, on the ten fixtures of the time in both
    write modes, 20 pairs): `usdcat` of a `.usda` and of its `.usdc` print identical text
    (it prints both encodings through one canonical printer, so a difference would be a
    value, type, variability or metadata the crate lost), and plain `usdchecker` reports
    success on every crate. The unit tests are
    `usdc_round_trips_every_layer_we_write` (every fixture, both modes: the parsed crate
    equals the parsed text, with no warnings, a `float` compared at the f32 a crate stores
    and everything else exactly), `usdc_scenes_equal_usda_scenes` (the two scenes differ
    only in a node transform's last bits, at the precision of the `float3` and `quatf`
    ops litegen writes), `usdc_encodes_every_type_we_write`, `usdc_output_is_deterministic`,
    `the_bootstrap_says_0_8_0` and the codec pieces (integer coding, literal-only LZ4).
    The crate reader's truncation and bit-flip sweeps also run on the writer's output:
    three fixtures by default, every fixture in an `#[ignore]`d run. The harness's `usdc`
    row and column are in "Harness results" below.
- **Read:** USDA text, and every section of a USDC crate: a bounded hand-written LZ4,
  integer and float compression, time samples, dictionaries, list ops and the path tree.
  A reader keys on the bytes, not the name: `from=usda` reads a crate and `from=usdc`
  reads a text layer, as every USD reader does, and only a refusal names the format
  asked for.
  - Composition: sublayers; internal and external references and payloads (through
    sidecars, otherwise a warning); inherits and specializes; variants by their authored
    selection (and the default prim's `Animation` set as clips, below); class, over and
    `active`; layer offsets and time codes per second as Pcp
    scales them: a sublayer by its listing layer's rate over its own, a reference by the rate
    of the layer that authors it over the referenced layer's, an internal arc by its authored
    offset alone, and a layer without `timeCodesPerSecond` at its `framesPerSecond`. A site
    joins a prim's composition once, at its strongest occurrence, so arcs that rejoin cost
    linear time.
  - Values: the strongest opinion with any value decides, and in it time samples beat a
    default, as usdrecord shows the start time code. Animation the scene carries (transform
    ops, joint animation, blend shape weights) keeps a default beside its samples as the rest
    pose, which is how litegen writes one.
  - Transforms: Euler, pivot, matrix and `!invert!` op stacks. Z-up and centimetre stages
    are converted.
  - Meshes: faceVarying primvars, n-gons, holes, leftHanded orientation, GeomSubsets, and
    constant primvars inherited from ancestors (a blocked one lets the ancestor's through).
  - UV sets are numbered by name. A mesh's sets are `st`, then `st1` … `st7` as it has
    them, in suffix order with gaps closed, then any other primvar its textures read
    (Blender's `UVMap`), in first-read order, eight at most. A texture's set is its
    primvar's place in that list, so litegen's files read back as written. A mesh with no
    `st` takes its first `texCoord2*` primvar as set 0.
  - Material layers: `inputs:clearcoat` and `inputs:clearcoatRoughness`, as values or
    through a `UsdUVTexture`'s `r` and `g`, an unauthored roughness reading as USD's 0.01,
    and `inputs:ior`. What glTF cannot hold is left out whole with a warning, value and map
    alike: a coat input from another channel, a biased map or another shader, or outside
    0 to 1; an ior that is not finite, or neither 0 nor at least 1, or that comes from a
    shader. A clearcoat beside an ior other than 1.5 is read, with a warning. An
    unconnected `clearcoat` of 0 and an `ior` of exactly 1.5 read as unset, silently, as the
    OBJ, FBX and Collada readers read their exporters' defaults: three.js's USDZExporter
    writes both on every physical material, and they draw as their absence does. Each
    warning is cut to 1,000 characters, and a read keeps 500.
  - Skins: `skel:joints` subsets, `geomBindTransform`, constant bindings, each inherited from
    ancestors inside the SkelRoot. A SkelAnimation without all of translations, rotations
    and scales leaves the rest pose, as UsdSkel does.
  - Hostile input is bounded with explicit stacks, checked counts, and caps on instancing
    and layers. Truncating or flipping any byte of the crate test files never panics.
- **Lights and cameras** follow the UsdLux and UsdGeomCamera schemas (OpenUSD v26.11.1),
  whose lights and cameras face −Z with +Y up, as the scene's do, so nothing turns. A
  node's light or camera is its prim, and one that shares its node with a mesh, a joint
  or the other is given a child of its own (USD's joints are no prims).
  - **Lights:** a directional light is a `DistantLight` of angle 0, and a point light a
    `SphereLight` of radius 0 with `treatAsPoint`. A spot is that `SphereLight` with
    `ShapingAPI`: `shaping:cone:angle` is its outer cone in degrees, off the axis, and
    `shaping:cone:softness` 1 − inner/outer. The intensity is photometric, as the schema
    defines it (Ruling R6): normalised, a `SphereLight`'s `inputs:intensity` is 4 × the
    candela and a `DistantLight`'s the lux. UsdLux has no range. Blender's USD IO works
    in Blender's radiometric watts instead, as its FBX IO does (§4): it writes a point
    light 683 times dimmer than litegen does for the same glTF light (`usd_write.rs`
    cites its exporter), and its export of `staged` (Ruling R28) wrote the 3 lux sun as
    a `DistantLight` of 0.0011. Blender's importer multiplies a sphere's intensity by π
    and a distant light's by 4 (`usd_reader_light.cc` L134-L148), so by default litegen's
    USD lights open in Blender 683× (point and spot) and 2732× (sun) brighter than the
    same scene's glTF, and Blender's read that much dimmer here. The opt-in
    `usd_light_units` (a convert query and a field of the generation request, as
    `fbx_light_units` is in §4; it applies when `from` or `to` is usdz or usda, so a
    same-format conversion is a no-op) chooses the units: `photometric`, the default, is
    the schema's intensity above, byte for byte; `blender` writes what Blender's exporter
    writes for E, the `energy` Blender's glTF importer gives the same light (lux / 683
    for a sun, candela × 4π / 683 for a point or spot, whatever its cone). A point or
    spot light's `inputs:intensity` is then E / π = 4 × candela / 683 on the same
    normalised, zero-radius `SphereLight`, and a sun's E / 4 = lux / 2732
    (`usd_writer_light.cc` L133-L141), clamped to a float. It reads a light prim that has
    no litegen `customData` the way Blender's importer does: E = `inputs:intensity` ×
    2^`inputs:exposure` (Ruling L3: the photometric reading folds the exposure in too) ×
    π for a `SphereLight` or × 4 for a `DistantLight`, then candela = 170.75 × and lux =
    2732 × that intensity. `inputs:normalize`, `inputs:radius`, `inputs:angle` and the
    cone leave E alone, as they do in Blender's importer. The `customData` wins in
    either units, so litegen's own files read back exactly. Real Blender 5.2.0 LTS
    confirmed both ends on 2026-11-08 (`scripts/blender-light-units/probe.py`, its
    numbers in
    [`research/research-blender-light-units.md`](research/research-blender-light-units.md)):
    `staged` written with `blender` as USDA and as USDZ imports through `wm.usd_import`
    with the `energy` of its glTF's import, and Blender's own USD export of that
    glTF-imported scene reads back here as 3 lux, 20 cd and 40 cd, to 3e-8, where the
    photometric reading of the same file gives 0.0011 lux, 0.029 cd and 0.059 cd.
  - **Cameras:** `projection`. A perspective camera's `focalLength` and apertures, in
    tenths of a metre, sit on a 24 mm film back and give yfov = 2·atan(verticalAperture /
    (2·focalLength)) and the aspect ratio horizontalAperture / verticalAperture. An
    orthographic camera's apertures are 20 × its `xmag` and `ymag`, and `clippingRange`
    holds the near and far planes in metres. For other readers an unset aspect ratio is
    written as 16:9, an infinite far plane as 1000 m and a mirrored orthographic view
    unmirrored, each reported.
  - **The exact values**, a light's range among them, ride in the prim's `customData`, a
    `litegen` dictionary named after glTF's properties, so a litegen round trip is exact.
    Other readers see the schema attributes, which have no range, so they light past it:
    a write reports a light's range as a `lights` loss (Ruling R27).
  - **Other writers' stages** read by the same schemas. A `SphereLight` that authors a
    cone angle is a spot, as Blender reads one, and so is one that applies `ShapingAPI`
    without one, at the schema's fallback of 90°, a hemisphere (its softness's fallback
    of 0 makes the edge hard, softened with a warning). A cone wider than 90° makes it a
    point light, with a warning (Ruling R32). `inputs:intensity` × 2^`inputs:exposure` is
    photometric, through `normalize`, an unnormalised `DistantLight`'s angle and a
    `SphereLight`'s radius. A colour past 1 has its peak moved into the intensity (Ruling
    R8). A colour temperature other than 6500 K, an IES profile, a shaping focus, a diffuse
    or specular multiplier other than 1, an aperture offset, and an animated attribute
    (read at its first sample) are each warned about. A light or camera prim whose
    transform does not read is dropped, with what is under it and a warning (Ruling R21).
  - **Readers:** Storm (usdrecord) draws a normalised `SphereLight` four times too bright
    and shapes no `SphereLight` spot (`hdSt/light.cpp` L159-L161, L244-L248). SceneKit
    reads no UsdLux light (measured). three.js's USDLoader reads neither lights nor
    cameras, and nor, by Apple's own table, does RealityKit (research Q8).
- **Several clips** (`usd_clips=all|first`; mesh-gaps item 6). It is a converter query,
  which applies when `to` is usda, usdc or usdz, and a field of the generation request,
  recorded in the `DeliveryPlan`. USD has one timeline, so a scene's clips are held as
  alternatives or the first alone.
  - **`first`** is the output from before item 6, byte for byte: the first clip is kept,
    and the rest are one `animation` loss ("has one timeline; only the first clip is
    kept").
  - **`all`** writes Apple's layout, the one its `variant_combiner.py` builds:
    - one SkelAnimation prim per clip under the Skeleton, outside the variant set, named
      by the clip's sanitised name (two clips that sanitise alike get distinct names).
      The original name is kept in `customData`, as prim names are, and restored on read;
    - a variant set `Animation` on the default prim, one variant per clip. A variant
      authors only `rel skel:animationSource = </…/Skel/<clip>>` on every Skeleton. The
      first clip is the authored selection, because USD applies no variant without one;
    - nothing a variant varies is authored outside the variant set. A local opinion is
      stronger than a variant's, so `skel:animationSource` appears only inside the
      variants;
    - one timeline: `endTimeCode` is the longest clip's end, and a shorter clip holds its
      last pose. Each SkelAnimation's `customData.litegen.clipEnd` (seconds) records its
      own end, so litegen reads exact durations. A clip that moves none of a skeleton's
      joints lists no joints, and that skeleton rests. The reader takes such an
      animation silently when it is litegen's own (it has `customData.litegen`) or weighs
      blend shapes, and names any other as animating nothing;
    - at most 64 clips (`MAX_CLIPS`, the reader's bound). A scene with more has the rest
      left out, reported as a second `animation` loss.
  - **Rigid (xformOp) animation stays the first clip's only.** The first clip's rigid
    channels are authored outside the variants, exactly where a one-clip file has them,
    so every reader shows them. A variant can vary an xformOp only when nothing outside
    the variants authors it, and then readers that skip variants lose the node's rest
    transform (by the research, three.js's USDA parser doesn't parse variant bodies, and
    its USDC path adds variant children as extra prims). The other clips' rigid channels
    are one `animation` loss, "rigid motion of clips [run, wave]", because USD "switches
    skeleton bindings between clips, not transforms; only the first clip's rigid motion
    is kept". Every clip's skeletal and blend-shape animation goes through the
    SkelAnimations, which every reader sees. A scene whose clips are all rigid is written
    as under `first`, with no variant set, whichever word was asked, and the other clips
    are reported as they were.
  - **Defaults.** `all` for usda and usdc. For usdz, `first`, because device behaviour of
    variant-switched animation is unestablished: the AR Quick Look device check (below) was
    skipped on 2026-11-10. If a later one passes, `USDZ_DEFAULT_CLIPS` in `mesh/mod.rs`
    flips to `all`, one constant and its test. A caller can ask a USDZ for `all`. The
    errors are above.
  - **What readers then do.** The research (`research-consumers.md` Q1) said, and the
    harness now measures (see "Harness results" below): three.js r183 returns one
    `AnimationClip` per SkelAnimation prim, named after the prim; Storm (usdrecord) plays
    the selected variant; `usdchecker --arkit` validates each variant. SceneKit does not
    play the selection: it plays every SkelAnimation prim at once. The rest is by the
    research alone: Storm's usdview and Blender play the selected variant; Apple says
    Quick Look shows default-prim variants as configuration options; RealityKit apps see
    the active variant's timeline.
  - **Reading** recognises this convention, and only this one. A default prim whose
    variant set `Animation` has a variant that authors `skel:animationSource` is composed
    once per variant and read as one clip per variant: the authored selection first, then
    the rest in the variant set's order. The composition is a stage-level selection map
    over the layers already loaded, so nothing is fetched or parsed twice. A clip is named
    for its SkelAnimation prim (the original name from `customData`), else its variant.
    Durations come from `clipEnd`. Rigid xformOp samples, authored outside the variants,
    belong to the authored selection's clip only. A set of more than 64 variants is
    `Unsupported`, naming the cap (`MAX_ANIMATION_VARIANTS`), and the composition budget
    (twice `MAX_PRIMS`) is charged per variant: a clip past it is left out, with a
    warning. Any other variant set, an `Animation` set that switches nothing, and several
    SkelAnimations with only one bound (some exporters write that) read as before: the
    bound or selected one.
  - **Checked:** unit tests (`fixtures::clips()`: one skinned rig and a prop box, three
    clips, "walk" 1 s, "run" 0.6 s and "wave" 2 s, where walk and run also move the
    prop): `usd_keeps_every_clip` (usda, usdc and usdz read back with names, order and
    durations equal, walk's rigid motion kept and run's one `animation` loss naming
    `run`), `the_layout_is_apples`, `an_all_rigid_multi_clip_scene_writes_as_today`,
    `usd_clips_first_is_todays_output`, `other_variant_sets_read_as_today` and
    `a_hostile_animation_variant_count_is_capped`, and the scene matrix (every
    container's clip count follows its capability: glTF, FBX, Collada and USD keep three,
    a USDZ one unless asked for `all`, and the static containers none). The per-variant
    renders are in "Harness results" below.
- **Unlit** (`KHR_materials_unlit`; mesh-gaps item 6). UsdPreviewSurface has no unlit
  mode, so an unlit material is written as the recipe every glTF-to-USD converter uses
  (google/usd_from_gltf, `convert/materializer.cc`), which the research measured exact
  in Storm (`research-consumers.md` Q2):
  - `diffuseColor` (0, 0, 0), `metallic` 1 and `roughness` 1. A metal with a black base
    has an F0 of black, so no specular remains: Storm drew the pixel equal to the linear
    base colour at 0°, 70° and 85°, where `metallic` 0 left 1–2 levels of specular;
  - `emissiveColor` = the base colour: its value; or the base-colour texture's
    `outputs:rgb` (sRGB) with the texture's `scale` = the factor; or, on a primitive with
    vertex colours, the `VertexColor` reader's `outputs:result`, with the base colour as
    the value beside it, as a lit material has it. The emission is written even when
    black, because the reader takes the base colour from it;
  - opacity as for a lit material;
  - three.js builds the same recipe as a black `MeshPhysicalMaterial` with `emissive` =
    the base colour.
  - **The marker.** The Material prim carries `customData = { litegen = { unlit = true } }`
    (written as `dictionary litegen = { bool unlit = 1 }`). No standard marker exists.
    A reader restores `unlit: true`, and the base colour and texture from the emissive
    connection, only for a Material that has it. Without the marker the recipe reads as
    a lit material with a black diffuse and an emission, as before, because guessing
    intent from the recipe could misread a real emissive design. A marker that is not a
    boolean (`int unlit = 1`, a string) is no marker either, and is named in the warnings.
  - **An unlit material in USD is its base colour and opacity alone.** glTF makes every
    lighting property inert under `KHR_materials_unlit`, and a clearcoat or an ior beside
    the recipe would add a specular lobe of its own. So `prepare` and the writer (they
    share `usd_write::unlit_surface`, so the prepared scene is what the file reads back
    as) set metallic and roughness to 1 and drop the metallic-roughness, normal,
    occlusion and emissive maps, the emissive factor, the clearcoat and the ior,
    silently. `usd::losses` no longer reports unlit.
  - `usdcat` reads the marker as `bool unlit = 1`, and `usdchecker --arkit` accepts the
    recipe. The unit tests are `unlit_round_trips_through_usd` (the recipe is asserted in
    the written text) and `a_black_diffuse_emissive_material_without_the_marker_reads_lit`.
    The two-light render check is in "Harness results" below.
  - Known device risk, in "Not yet known" below: an sRGB emissive texture in Quick Look.
  - Not built: RealityKit's MaterialX unlit node (§9).
- **Harness results** (mesh-gaps item 6, task H; run on 2026-11-10 from tree `533b7f8`,
  where `litegen-core` is identical to the `13a8dd7` the binary was first built from;
  Apple USD Tools 0.25.2, three.js r183, SceneKit on macOS 26.6.2). The release run
  (`mesh_matrix --formats glb,usda,usdc,usdz --usd-variants`, then `verify.mjs`) passes
  444 of 444 cases. The full format set (`mesh_matrix --dae-keep --usd-variants`, every
  source into every target) passes 2,232 of 2,232. Both have 0 conversion errors.
  `--usd-variants` adds three cases for every direction into USDZ: `usdz-all` (every
  clip, USDA root), `usdz-crate` (every clip, `usdz_root=usdc`) and `usdz-crate-first`
  (`usdz_root=usdc`, the default single clip: the plain crate-rooted USDZ a device check
  needs). The full set holds 888 USD files, all clean in `usdcat`; 156 usda and usdc
  pairs with identical text (23,575 lines) and 1,149 of 1,149 identical usdrecord
  pictures; and 30 clip files, 90 variants and 630 usdrecord frames, as below.
  - **The `usdc` row and column** (every fixture and source direction of glb, usda, usdc
    and usdz). `usdcat` exits 0 with nothing on stderr on 384 of 384 USD files.
    `usdcat model.usda` and `usdcat model.usdc` print identical text on 72 of 72 sources
    (12,406 lines), and usdrecord draws 711 of 711 identical pictures of the two (0.00 %,
    maximum channel delta 0). `usdchecker` runs 384 times (144 plain on the usda and
    usdc, 240 with `--arkit` on the USDZ) and reports 384 ok, 0 errors and 0 warnings. It
    reports each variant of the multi-clip files, below. three.js's `USDLoader` reads the
    `usdc` cases: 72 of 72 pass (6 sources by 12 fixtures), the crates of the `morph`
    fixture being a cited skip (the last bullet).
  - **Several clips**, fixture `clips()` (walk 1 s, run 0.6 s, wave 2 s): 18 files (usda
    5, usdc 5, usdz-all 4, usdz-crate 4). `usdz-crate-first` and a plain `usdz` hold the
    first clip alone, so the clip checks do not run on them. They pass every ordinary
    judge (`usdchecker --arkit`, usdrecord, three.js, SceneKit).
    - usdrecord draws each variant through a session layer that selects it
      (`over "<root>" (variants = { string Animation = "<clip>" }) {}`), at rest and at six
      sample times of that clip's own range (walk 0, 0.17, 0.36, 0.58, 0.83, 1; run 0,
      0.102, 0.216, 0.348, 0.498, 0.6; wave 0, 0.34, 0.72, 1.16, 1.66, 2). That is 54
      variants and 378 frames, and every frame has 0 differing pixels (maximum ratio 0)
      against that clip's expected picture: the clip's skeleton plus the first clip's
      rigid motion. A guard fails a case whose clip draws as the first clip at every time;
      it passes.
    - `usdchecker` reports and passes each variant: 54 variants on the 18 files
      ("Validation Result for Variants: Animation:<name> on Prim: /Root", each
      "Success!"), and `--arkit` passes the 8 USDZ files.
    - three.js lists one clip per SkelAnimation with the right duration (walk 1.000 s, run
      0.600 s, wave 2.000 s) and the first clip's rigid motion as a `TransformAnimation`
      (1.000 s), judged on a crate (the file itself, or a `usdcat` crate copy of a USDA):
      18 of 18. The text files as they are agree on the names, 9 of 9. Their durations are
      a cited skip: three.js reads a USDA at 30 time codes per second whatever the file
      says (`USDAParser.js:439-459`, `USDComposer.js:87`).
    - SceneKit, measured on USDA, USDC, `usdz-all` and `usdz-crate` alike. Its USD importer
      keys one animation player per SkelAnimation prim by its path (`/clips/leg/walk`,
      `/clips/leg/run`, `/clips/leg/wave`) and plays all of them at once. It reads no
      variant selection and no `skel:animationSource`: the result is the same with the
      variant set flattened away. Left alone, its silhouette at the five walk times was
      25.61 %, 43.13 %, 47.01 %, 32.44 % and 35.04 % apart from the selected clip's
      expected picture (limit 1 %), nearest the file's last prim (wave) and never the bound
      one (walk). It also keys a USD xformOp animation's samples over the whole stage's time
      range, not the span they were authored in: here the stage runs 0 to 60 at 30 per
      second and the prop's translate samples sit at 0 and 30, so the `transform` key
      lasts 2.0 s against the walk player's 1.0 s. A USDZ of one clip, whose stage ends with
      its clip, shows no lag. So when a job names a clip, `render.swift` removes the
      players of the other SkelAnimation prims and plays the rigid keys at the speed that
      stage span over clip span gives. With both, SceneKit's picture of the selected clip
      matches the expected one at every walk time (the prop's row 150.3 against
      usdrecord's 150.4 at 1 s), and silhouette and albedo pass at every sample time of
      the first clip on 18 of 18 files. Past the clip's end the prop moves back, because
      SceneKit loops it where USD holds the last sample, so the judged times end at the
      clip's end. SceneKit cannot be told to switch a variant, so its other two variants of
      each file (36) are cited skips, drawn by usdrecord instead. Its variant support stays
      "partial" (`research-consumers.md:77`).
  - **Unlit**, fixture `unlit()` from `scene` and `glb` into usda, usdc, usdz, usdz-all,
    usdz-crate and usdz-crate-first (12 cases). The unlit box sits beside lit controls in
    one frame (the flat wedge and the vertex-coloured panel), and each renderer draws the
    scene under two light levels. The unlit pixels must match across the two within
    1/255, the lit control must move, and the base colour is judged against the albedo.
    The 12 cases give identical numbers per renderer, since every format gives the same
    picture. Under a distant light:

    | renderer | light, two levels | unlit pixels differing | max delta | lit control moved by more than 8/255 | base colour against the albedo |
    |---|---|---|---|---|---|
    | usdrecord | normalised distant light, 0.5 and 1.5 | 0 of 5,388 | 0 | 98.5 % of 6,610 pixels (max 64/255) | 7.61 % past 20/255, limit 8 %, judged |
    | SceneKit | ambient and directional, 150 and 400 lumens | 0 of 5,268 | 0 | 100 % of 6,446 (max 85) | 3.09 %, recorded only |
    | three.js | ambient 0.4 and 1.0, directional 0.8 and 2.0 | 0 of 5,265 | 1 (mean 0.013) | 100 % of 6,445 (max 71) | 0 %, recorded only |

    The recipe is invariant under a distant light in all three renderers, and the lit
    control moves in each. usdrecord's sun is normalised (`inputs:normalize = 1`), because
    Storm draws an unnormalised distant light almost black (intensity 100 gives a mean red
    of 2.2 of 255, and 1000 gives 18.3), where the lit control would not move. The 7.61 %
    is the edges of the quadrant texture, 0.39 point under the limit. SceneKit and three.js
    record the base colour without judging it: SceneKit reads UsdPreviewSurface colours as
    linear Display P3 and skips primvar readers, and three.js decodes linear base and
    emissive colours as sRGB (`USDComposer.js:2512-2520` and `:2541-2562`). usdrecord
    judges the base colour, and every renderer judges the light invariance.
  - **The dome residual, a measured limitation and not a judge** (Ruling U22: usdrecord's
    invariance is judged under the distant light alone, at 1/255, and no tolerance is
    widened). Under usdrecord's dome light, at levels 0.15 and 0.45, the unlit pixels are
    not invariant: 751 of 5,388 (13.94 %) differ by more than 1/255, the mean delta is
    0.741, the maximum is 6/255 (a white dome), and 0.11 % are past 3/255. The cause is
    Storm's: for a dome light it adds `prefilter * (F * brdf.x + brdf.y)`
    (`/usr/lib/usd/usd/usdShaders/resources/shaders/previewSurface.glslfx:327`, Apple USD
    Tools 0.25.2), and `brdf.y` survives a black F0. So a surface with the unlit recipe
    (diffuse black, metallic 1, roughness 1, emissive = the base colour) still picks up a
    small dome-dependent term. The same residual shows with `useSpecularWorkflow` and a
    black specular colour, and with metallic 0 and ior 1. The recipe is the standard one,
    and an IBL-lit viewer may show the same small leak on an unlit USDZ. It is recorded in
    the report's `summary.usd.unlit.domeResidual`.
  - **The three.js crate skip.** three.js's USDCParser reads an empty array's payload 0 as
    an offset: `USDCParser._readValue` seeks to the payload of any array that is not
    inlined and has no payload-0 case (`USDCParser.js:1087-1102`, r183), where OpenUSD
    packs an empty array as payload 0 and unpacks it as empty (`crateFile.cpp:1775-1777`
    and `:1805-1808` at v26.11.1). Measured: a crate Apple's own `usdcat` wrote of
    `uniform token[] joints = []` fails in `USDCParser.parseData` with "Offset is outside
    the bounds of the DataView", one of `uniform matrix4d[] m = []` with "Invalid typed
    array length", and `token[] names = ["x", "y"]` parses. The `morph` fixture holds
    `bindTransforms`, `joints` and `restTransforms` empty, so 16 of its crates (6 usdc, 5
    usdz-crate, 5 usdz-crate-first) are a cited three.js skip in the release run, and 22
    (8, 7, 7) in the full set. usdrecord judges those crates, and three.js judges the same
    data in the USDA twin. It is not a litegen defect: litegen's crate packs an empty
    array as payload 0, as OpenUSD does (`usdc.rs`, `array()` and `array_at`).
- **Not carried, and reported:** texture filters, the material layers other than
  clearcoat and ior (above), the clips after the first under `usd_clips=first`, and the
  rigid motion of the clips after the first under `all` (above). Tangents are not
  written, and not reported: readers derive them, as from every container that drops them.
  Resampling a clip into USD's linear interpolation is not a loss either: the clip is kept.
  Animated joint scales go through UsdSkel's `half3[]`, about 1e-3 relative. A
  scale past 65,504, the largest half, is written as 65,504 in USDA and USDC alike, and
  reported as a `skins` loss ("joint scales past 65,504"). The crate writer refuses a
  finite half or float past its type's range rather than write an infinity, as it refuses
  an integer that doesn't fit.
- **Points and lines** (mesh-gaps item 10; `OpenUSD@v26.11.1 pxr/usd/usdGeom/schema.usda`
  L1877 `Points`, L1594-1812 `BasisCurves`): a `Points` prim is a points primitive, and a
  `BasisCurves` of `type = "linear"` a lines primitive, a segment between each curve vertex
  and the next and from the last to the first where `wrap = "periodic"`. Primvars follow the
  schema's interpolation (on a `Points` prim every point is an element; on a curve `uniform`
  is one per curve and the rest one per curve vertex). The writer writes a `Points` prim for
  a points primitive and one linear, nonperiodic `BasisCurves` with a 2-vertex curve for each
  segment of a lines primitive, each index a vertex of its own (a USD curve shares none),
  both with `widths` authored as one constant, 0.25 % of the diagonal of the scene's bounds at
  rest (a centimetre where there is none), so that Storm draws no point a metre wide. A
  curve's normals, which would orient a ribbon, are not written or read. Storm (usdrecord)
  draws both; three.js's USDLoader and RealityKit drop them and SceneKit drops `Points`
  (§8), so usdrecord is USD's judge.
- **Not read, with a warning:** relocates, value clips, splines, crate array edits, nested
  USDZ references, PointInstancer, lights that are not punctual (`RectLight`, `DiskLight`,
  `CylinderLight`, `DomeLight`, …), separate metallic and roughness textures, the
  specular workflow, and curves other than linear `BasisCurves` (cubic ones, `NurbsCurves`,
  `HermiteCurves`).
- **Oracles.** three.js's USDLoader is a weak oracle, and `page/render.js` cites the source
  line behind each of its gaps:
  - it reads linear colours as sRGB and keeps only `displayColor[0]`;
  - it misreads every SkelAnimation rotation in USDA text, because `quatf[]` is never
    reordered;
  - it has no BlendShape support;
  - it builds uv sets 0 and 1 only (`uv` and `uv1`), so sets 2 and up, and a texture on
    one of them, are out of its reach;
  - it cannot read a crate that holds an empty array, so usdrecord judges those crates
    (`USDCParser.js:1087-1102`; "Harness results" above).

  Where it is blind, other oracles judge:
  - **usdrecord** (Pixar's Hydra Storm, the reference renderer) renders every USD output
    through the harness camera, at the default time and at every sample time. It produces
    silhouettes (`cover`) and albedo, with lighting removed by session-layer overrides.
  - **`crate:geom`**: three.js renders a `usdcat` crate copy of each skinned file, whose
    rotations it reads correctly.
  - **SceneKit** silhouettes, and uv sets 2 and 3, drawn through its own `mappingChannel`
    (the harness's per-set `uv` modes). The exception is a file with BlendShapes, where no
    reader judges the uvs apart from the pose, and the harness skips them with that reason.
    Sets 4–7 have no render judge: three.js's materials address four sets, so not even the
    expected picture can draw them, and the harness skips them, citing that. The unit tests
    read all eight back exactly.
  - **Lights and cameras:** SceneKit and usdrecord (`--camera`) draw through the output's
    own camera, usdrecord draws each light alone, and every Camera prim's frame and optics
    are read from the flattened text and compared with the scene's (§8). Storm draws no
    cone and SceneKit reads no UsdLux light, so a light's intensity, a spot's cone,
    position and direction, and a light beyond the expected scene's have no judge beyond
    the unit tests (§8).

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
  convert the root layer to USDC; ours is USDA by default, which `--arkit` accepts, and
  USDC with `usdz_root=usdc`. SceneKit does not evaluate a primvar reader feeding
  `diffuseColor`, so the writer also writes the base colour beside the connection.
  Whether RealityKit evaluates it is untested. Four more questions need an AR Quick Look
  device check, which opens three USDZ files on an iPhone (a multi-clip one with
  `usd_clips=all`, one with `usdz_root=usdc`, and a plain one). Mesh-gaps item 6's plan
  ended with that check; the user skipped it on 2026-11-10, so all four stay open:
  - **Variant-switched clips:** whether the default clip plays and whether the picker
    switches. USDZ's `usd_clips` default stays `first` until this passes. If it does,
    `USDZ_DEFAULT_CLIPS` flips to `all`, one constant and its test. If it doesn't, the
    default stays and the result is recorded here.
  - **A USDC-rooted USDZ in Quick Look:** `usdchecker --arkit` accepts it, and Apple's own
    packagers and most of Apple's Quick Look gallery write crate roots, but a litegen one
    has not been opened on a device.
  - **Quick Look's treatment of an emissive texture as linear:** usd_from_gltf writes an
    unlit material's emissive texture linear because Quick Look treated it as linear. Our
    recipe writes it sRGB, the base colour's space, so whether that still holds is
    unestablished, and an unlit textured USDZ may look brighter in Quick Look than in
    Storm until a device check settles it.
  - **The rim light:** RealityKit and Quick Look draw the unlit recipe close to unlit, and
    the research cites a rim-light factor in Quick Look. It is not measured on a device.

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
    filter mapped), dropping its tint and any vertex colour. It is drawn from the one uv set
    3MF holds, the one its base colour samples, and the other sets are reported lost;
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
  - every material layer;
  - skins, morphs and animation, posed at rest;
  - grouping nodes, flattened to placements;
  - lights and cameras, which 3MF does not hold;
  - points and lines (mesh-gaps item 10): the Core spec has no element for them (the beam
    lattice extension is a non-goal), so `threemf::prepare` strips them and they are
    reported by primitive. A scene of nothing else is refused by name (§1).

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
  - Up to eight uv sets, `TEXCOORD` inputs `set="0"` … `"7"`, each its own `<source>`, on
    the `<triangles>` (inside `<vertices>`, three.js gives set 1 an item size of 0). In a
    package, `prepare` puts the sets its kept textures sample first (base colour's set as
    set 0, then emissive's, then the rest by index) and renumbers the bindings, because
    three.js's ColladaLoader samples every texture from set 0 and builds sets 0 and 1
    only. A scene textured on set 0 writes as before.
  - Effects are `profile_COMMON`:
    - lambert when fully rough, else phong with shininess `2/r⁴−2`;
    - diffuse colour **or** texture (the profile cannot multiply them);
    - emission, and `A_ONE` transparency for BLEND;
    - the ior as `<index_of_refraction>`, the one material layer Collada keeps (the other
      eight are reported). An ior of 0, 1 or 1.45 is left out and reported, since it would
      read back unset (below), and 1.5, glTF's own default, is left out unreported;
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
- **Morph targets are baked by default, and written on request.** three.js's ColladaLoader
  fails the whole load on any morph controller (see "Who refuses it" below), and assimp
  6.0's morph import has a loop bug. So by default `prepare` bakes morphs at their
  effective weights and reports the loss. The reader handles NORMALIZED and RELATIVE morphs,
  skin over morph, and weight animation; it was checked against SceneKit on six morph files.
  With `dae_morphs: keep` (`WriteOptions.dae_morphs`; the converter's and the 3D requests'
  `dae_morphs`, mesh-gaps item 8) the writer writes them instead:
  - **What it writes:**
    - one `<controller>` per morphed mesh, over the mesh's `<geometry>`, with `<morph
      method="NORMALIZED">`, an `IDREF_array` of target `<geometry>` elements (each target's
      absolute positions and normals, base + offset), a `float_array` of the default weights,
      and `MORPH_TARGET` and `MORPH_WEIGHT` inputs. NORMALIZED with absolute targets is the
      reading every implementation agrees on, `base·(1−Σw) + Σwᵢ·targetᵢ`;
    - `POSITION` and `NORMAL` both inside `<vertices>`;
    - a weight animation as one channel per weight, targeting
      `<morph id>-weights(i)`: the id of the `MORPH_WEIGHT` `<source>`, which is the one id
      SceneKit binds (the `float_array`'s builds empty animation groups);
    - a skinned and morphed mesh as `<skin source="#…-morph">`: skin over morph.
  - **Who reads it:** SceneKit (Xcode's scene editor builds on it; not measured). The spike
    (macOS 26.6.2, 2026-11-09; tables in
    `docs/superpowers/research/research-scenekit-collada-morphs.md`)
    measured it on hand-written files from the `morph` fixture's numbers: the default
    weights at rest, six times of a weight clip (with weights summing to 1.48, so NORMALIZED
    is not clamped), and skin over morph, each picture 0.00 % from a control with the morph
    applied by hand, on three cameras, and the morphed normals too (lit pictures
    pixel-identical to the blended-normals control, different from a base-normals one by up
    to 21,442 pixels). Godot reads morphs (from its source; not measured). Unity and
    SketchUp were not established, and Blender dropped Collada in 5.0.
  - **Who refuses it:** three.js r183 and assimp 6.0.5, each the whole file. In three.js an
    animated morph fails in `ColladaParser.parseAnimationChannel`, whose `sid.indexOf` meets
    the `/`-less target; a static one in `ColladaComposer.buildNode`, which reads
    `controller.skin.joints`; a static skin over one in `buildController`. Hence off by
    default. A scene with no targets writes the same file under `keep` as under `bake`,
    which every reader opens.
  - **The harness** (`mesh_matrix --dae-keep`, `scripts/mesh-verify/README.md`, "Collada
    morphs: the `dae-keep` variant"): three.js and assimp must refuse a file that holds a
    `<morph>` (if one loads it, the case fails, "documented reader limitation no longer
    holds"); xmllint validates every such file against the 1.4.1 XSD (fetched at run time,
    its sha-256 pinned, its `xml.xsd` import resolved offline through a catalog); and
    SceneKit, the only judge of the geometry, renders every kept file at rest and at every
    sample time. Its pictures are judged as follows:
    - the rest uv frame against SceneKit's own picture of the plain Collada file (the bake,
      which three.js judged), not the expected picture: on its own SceneKit draws the two
      textured Khronos morph models 4.07 % and 5.20 % from the expected one, and `keep`
      moves no texture coordinate (Ruling DM20);
    - when the clips of a file animate the same node, SceneKit binds their keys by target
      and plays none, so nothing judges that file's animated frames: they are left
      unjudged, cited, and counted apart in the run's summary (`animatedUnjudged`), the
      rest frame alone judging it (Ruling DM21);
    - every remaining frame (the silhouette and albedo at rest and at each sample time, the
      animated uv frames) against the expected picture; the animated uv frames only drop
      out, cited, where the plain file's own rest uv already fails the limit.

    SceneKit's renderer needed one fix for this: it hangs a weight animation on the
    `SCNMorpher`, which the harness did not drive, so a rest picture showed the animation's
    first key (silhouette 1.24 % apart, uv 8.83 %); with it, 0.05 % and 0.55 %, and no other
    SceneKit picture moved (992 byte-identical).
  - **Results, 2026-11-09,** from item 8's gate: tree 7e02650, a release `mesh_matrix`, the
    reports under `gate/` in item 8's ledger (`.superpowers/sdd/2026-11-04-mesh-dae-morph-output/`):
    - every fixture with `--dae-keep`: 1,260 of 1,260 cases. 120 are `dae-keep`, and 7 of
      them, all from `morph`, hold a `<morph>`. three.js and assimp refused all 7. SceneKit
      judged every frame of them (147 pictures): silhouette at most 0.062 % apart, albedo
      0.00 %, uv at most 1.675 %. xmllint: 120 of 120 valid;
    - three Khronos morph imports with `--dae-keep`, 126 of 126 each. In each, 7 of the 12
      `dae-keep` files hold a `<morph>`, and three.js and assimp refused all 7; xmllint: 12
      of 12 valid. AnimatedMorphCube, with a weight clip: 98 SceneKit pictures, silhouette
      at most 0.055 %. MorphPrimitivesTest, a morphed mesh of two primitives: 21 pictures at
      rest, silhouette at most 0.112 %, albedo 0.151 %. MorphStressTest: 64 pictures judged,
      silhouette at most 0.089 %; 5 of its cases leave the animated frames unjudged, because
      SceneKit plays no clip that shares a node with another (Ruling DM21).

    An earlier run of `morph`, `skinned` and `staged` alone (204 cases, 27 `dae-keep`) gave
    the same verdicts.
- **Reading** also covers:
  - triangles, polylist, polygons, trifans and tristrips (concave n-gons up to 64 corners
    ear-clipped);
  - multi-index inputs, `<vertices>` indirection, up to 8 UV sets with `bind_vertex_input`;
  - unit and X/Z-up as one root correction, and `instance_node` and `library_nodes` with
    cycles refused;
  - skins with the bind-shape matrix folded into the inverse binds;
  - LINEAR/STEP/BEZIER/HERMITE channels on matrices, elements and components, resampled to
    TRS;
  - `<index_of_refraction>` as the ior, except 1, 1.45 and 1.5, which read as unset:
    exporters write them by default (SceneKit 1 on every effect; Blender 2.8–4.5 its
    Principled IOR, 1.45 before 4.0 and 1.5 since). One of 0 or below is no refractive
    index, and is left out with a warning, as OBJ's `Ni 0` is: read as glTF's 0 it would
    turn the material into a perfect mirror, and an OBJ's `Ni 0` reaches Collada through
    assimp unchanged. A value `KHR_materials_ior` cannot hold, or one that is not a number,
    is left out with a warning too;
  - animation clips;
  - `<lines>` and `<linestrips>` as lines (mesh-gaps item 10), through the same multi-index
    inputs, with the segments charged to the index budget from the token counts before
    anything is allocated.

  `<skew>`, holes (`<ph>`), B-splines and cross-document references are refused by name.
- **Points and lines:** the writer writes `<lines>` (pp. 5-72, 5-74 and 5-80 of the 1.4.1
  spec), never `<linestrips>`: three.js keeps only the last `<p>` of one and SceneKit builds
  a geometry with no elements from it. 1.4.1 has no point primitive, so `points` is reported
  and a scene of points alone is refused by name (§1). `dae::prepare` does not compact
  vertices: the reader numbers the vertices a primitive's corners name, so a lines
  primitive with a vertex no segment names reads back with one fewer, the segments the
  same, as a triangle primitive does.
- **Lights and cameras** follow the COLLADA 1.4.1 spec's text, with SceneKit and assimp
  as oracles; `dae.rs` quotes the pages and cites each reader.
  - **Axes** need nothing: Collada's lights shine, and its cameras look, along −Z with +Y
    up, as glTF's do.
  - **Colour:** Collada has no intensity, so `<color>` is the colour × the intensity,
    linear (Ruling R17), and a point or spot light's attenuation is 0, 0, 1: the spec's
    formula makes that glTF's inverse square.
  - **Cones:** `<falloff_angle>` is the full cone in degrees, twice the outer half-angle
    (Ruling R23). The spec only names it; SceneKit (measured) and Blender's Collada IO read
    it as the full cone. Its default of 180 is a hemisphere, and a wider cone is read as a
    point light, with a warning (Ruling R32).
    `<falloff_exponent>` carries the inner cone by one documented rule (both beams fall to
    half at the same angle), and an exponent too weak to halve the beam inside the cone,
    the default 0 among them, is a hard edge, softened.
  - **Cameras:** `<perspective>` holds `<yfov>` in degrees and `<aspect_ratio>` when there
    is one, and `<orthographic>` holds `<xmag>` and `<ymag>` as half the view's width and
    height, as Blender reads them (the spec says it both ways). `<zfar>` is required, so
    an infinite far plane is written as 1000 m, and reported.
  - **The exact values**, a light's range and each name among them, ride in an
    `<extra><technique profile="litegen">` on each `<light>` and on each camera's
    `<optics>`, so a litegen round trip is exact: "Key Light" comes back as it was, where
    the `name` attribute, an NCName, holds `Key_Light`. Other readers see the common
    profile, which has no range, so they light past it: a write reports a light's range as
    a `lights` loss (Ruling R27).
  - **Other writers' files**, without that extra, read by the documented approximate
    mapping. `<color>` splits into a colour and an intensity at its peak (Ruling R8). The
    attenuation folds into the intensity as it stands at 1 m, and any falloff but the
    inverse square is warned about. No light has a range. Every field of view the spec
    allows is read, the aspect ratio being tan(xfov/2) / tan(yfov/2), and a lone `<xfov>`,
    `<xmag>` or `<ymag>` takes the other at 16:9, with a warning. An `<ambient>` light is
    dropped, and named (Ruling R15).
  - **Readers depart from the spec.** three.js's ColladaLoader (r183) ignores `<xfov>`,
    halves `<xmag>` and `<ymag>`, aims no light along its node (so each shines at the
    origin), reads `<falloff_angle>` as the half-angle (so our spots draw twice as wide),
    ignores `<falloff_exponent>`, cuts every point and spot light off 1 m away (Ruling
    R18) and decodes `<color>` as sRGB. assimp 6.0.5 reads `<falloff_angle>` as its inner
    cone and guesses the outer one from `<falloff_exponent>`, and reads no optics for a
    `<camera>` without a `name` attribute (`ColladaParser.cpp` L1049-L1051), so litegen
    names a camera that has no name by its id there (its extra keeps it unnamed). SceneKit
    decodes `<color>` as sRGB, as three.js does (measured on its pictures' hue), clamps
    each channel to 1 and reads none of the attenuation, its start and end distances
    staying 0 (both read back from the light SceneKit builds), and draws a point light
    without the inverse-square falloff (measured on its pictures); all 2026-11-06.
  - **A committed file** that assimp wrote of the `staged` fixture
    (`testdata/dae/assimp/staged_assimp.dae`) pins the reader against assimp's own
    conventions.
- **Oracles.** three.js's ColladaLoader rendered every mode for all cases (static frames
  0.00 %, animated frames ≤ 0.19 %, from resampling the cubic channel). SceneKit confirmed
  silhouettes (≤ 0.32 %) and albedo (≤ 0.83 %), including the animated skin. Where
  three.js misreads valid Collada (`bind_vertex_input`, primitive-level `COLOR` through an
  off-by-one, any morph), the native checks defer to SceneKit, citing the loader's source
  lines. uv sets 2 and 3, which ColladaLoader does not build, are judged by SceneKit
  instead, which draws each through its `mappingChannel`. Sets 4–7 have no render judge:
  three.js's materials address four sets, so the harness skips them with that reason. The
  unit tests read all eight back exactly.

  SceneKit has a limitation of its own. It binds animation data to its target rather than
  to the clip that instances it. So when two clips animate the same node, every clip carries
  the last clip's keys over the first clip's duration. On the Khronos Fox, the "Survey" clip
  held "Run"'s keys and "Walk" came out empty. three.js plays each clip correctly. For such
  files SceneKit judges only the rest pose.

## 8. How it is proven

`scripts/mesh-verify/README.md` has the mechanics. In short, every direction (A → B, plus
the scene written directly as B) for ten fixtures:

| fixture | built to catch |
|---|---|
| **textured** | flips, swapped axes, lost transforms, lost mirrors, reversed winding, lost per-part materials: an asymmetric texture, a non-uniformly scaled wedge, a vertex-coloured panel, a mirrored instance |
| **skinned** | interpolation errors between keyframes: a banded tube on a three-joint chain, bent by LINEAR, STEP and CUBICSPLINE channels |
| **morph** | wrong morph targets or weight animation: two targets, one with normal offsets, animated weights |
| **cube** | the original flat, attribute-complete fixture |
| **metal** | a container that keeps, folds or drops metalness: a gold metal, a brushed one, and a part whose metal and roughness come from a map beside a base-colour map |
| **multiuv**, **multiuv_shared** | a uv set lost, swapped or renumbered: four sets on one quad, base colour on set 3 and occlusion on set 1; then two primitives sharing that material |
| **layered** | a material layer lost or changed: seven boxes whose materials carry all nine of glTF's layers between them, five of them with layer maps |
| **staged** | a light or camera lost, moved, turned or mis-converted: a directional light, a point light with a range, a spot (cones of 0.2 and 0.6 rad), a perspective camera (yfov 0.8, aspect 1.5) and an orthographic one, on nodes that a 2 s clip turns about a cube, which the cameras, the sun and the spot face |
| **wire** | points or lines lost, moved, recoloured or drawn in the wrong place: a unit cube, a four-segment coloured ring under an animated node (`Swing`, 7 frames) and a 3 x 3 grid of coloured points |

Each output is rendered by **three.js r183's own loader for its format** (the engine behind
the dashboard's `<model-viewer>`) and compared pixel by pixel with the expected picture
(`to.prepare(from.prepare(fixture))` via GLTFLoader). The comparison runs in four unlit
modes (geometry, normals, albedo, UV), at six sample times between keyframes. The UV mode
draws set 0, and the per-set modes `uv1`–`uv3` draw sets 1–3 where the expected scene
keeps them. Passing cases are 0.00 % apart.

Material layers have a mode of their own, **`lit`**, drawn wherever the expected file keeps
a layer; among the fixtures that is `layered`, seven boxes whose materials carry all nine
layers between them. Each loader's own material is drawn under three.js's
`RoomEnvironment`, with no tone mapping and a fixed exposure, through the fixture's one
camera. GLTFLoader judges the glb and gltf outputs at 0.00 %, with a per-channel tolerance
of 1/255: every expected file against itself, and GLB → GLB, measure 0 pixels apart.
USDLoader judges the USD outputs as `lit_coat`, at 0.00 % too, on the meshes it reads
right: it takes `clearcoat`, `clearcoatRoughness` and `ior` as values only, and decodes
base colours as sRGB. usdrecord's dome-lit picture stays in the report but judges nothing:
Storm adds the coat on top of the base, and the USD control below moved Storm's picture
of the lacquer by at most 10/255, no pixel past the 20/255 tolerance; only in the four
usd → usd cases, where car_paint's coat map was lost, did it move 434 pixels, by
up to 44/255. The OBJ, Collada, FBX, 3MF, PLY and STL loaders build Phong, Lambert or
no materials and read no layer, so an OBJ package's clearcoat, sheen and anisotropy are
judged through obj → glb. What `lit` can see is measured per expected file, removing each
layer in turn: dispersion moves no pixel at all, and the iors Collada and OBJ keep
(glass's 1.52, plastic's 1.6; both leave the lacquer's 1.45 out as an exporter default)
none past the tolerance, so `lit` judges those iors nowhere in the fixture run (the
`--import` run judges OBJ- and Collada-kept iors in 24 cases). A case whose layers all move
nothing is skipped, citing the measurement. In all, on 2026-11-05, 26 of the fixture
run's 45 layer cases were judged and 19 skipped with a citation; of the 382 in the
sixteen Khronos layer samples (`--import`), 205 were judged and 177 skipped with one.
`lit` cannot see the glTF writer dropping a layer, because the expected picture comes
from that writer too: `every_pbr_extension_round_trips` pins it against hand-written
JSON, and the `--import` run against each Khronos source, comparing its layer extensions
field by field and drawing the original beside our reading in `lit`. The two negative
controls, the glTF writer without `KHR_materials_sheen` and the USD writer putting
`clearcoatRoughness` into `clearcoat`, each fail their unit test and the lit cases that
judge that layer. The runs passed 912 of 912 fixture cases and 1,824 of 1,824 sample
cases, with no glTF-Validator error. The harness README's "The `lit` mode: material
layers" has the settings, the calibration and every skip's citation.

Lights and cameras have two modes and a numeric oracle of their own, run wherever the
expected file keeps a light or a camera; among the fixtures that is `staged`.

- **`camera`** draws the geometry mode through the file's own camera (the one named like
  the expected scene's first), at that camera's aspect ratio, at rest and at every sample
  time, against the expected GLB drawn through its own. A camera lost, moved or turned
  (the 90° of a missing FBX turn), or given a wrong fov or aspect, is a large difference
  where a right one is 0.00 %. A near or far plane moves a pixel only where it cuts
  geometry, so the clip planes are the oracle's to judge.
- **`lit_lights`** draws each loader's own material lit by the file's lights alone, with
  no environment, through the fixture's camera, at linear tone mapping and an exposure of
  0.4, the brightest at which no pixel of `staged` clips.
- **The numeric oracle** (`camlight-oracle.mjs`) reads assimp's reading of every FBX,
  Collada and glTF output (its `assjson` export, since assimp 6.0's glTF exporter writes
  no camera or light: Ruling R11) and every USD output's own flattened text, for its
  cameras alone. It compares
  each camera's eye, forward, up, yfov, aspect ratio and near and far planes (and an
  orthographic camera's size, where the reader keeps it), and each light's kind,
  position, direction, colour × intensity and cones, with the expected scene's, within
  1e-4, and the shape of its falloff (the inverse square), and each camera's frame again
  at every sample time. Where assimp's
  convention differs from the format's (it halves FBX's `FieldOfView`, and takes
  Collada's `<falloff_angle>` as its inner cone), the comparison is adjusted and the
  source line cited.

The judges are GLTFLoader for glTF, at 0.00 %; for FBX, the expected geometry drawn
through, and lit by, the cameras and lights assimp read; for Collada and USD, SceneKit
through the file's own camera, and for USD usdrecord `--camera` too. Each light of a
Collada or USD output is drawn alone on white surfaces, by SceneKit for Collada and by
usdrecord for USD, and compared with three.js's picture of that light for where it falls
and its colour. three.js's FBXLoader, ColladaLoader and USDLoader are no judge of either
(§4, §5, §7), so their cases are skips that cite the loader's lines. Every camera and every
light must end with a judge of its own, or the case fails naming it. A glTF original read
with `--import` is drawn beside our reading of it in both modes too (Ruling R24).
Calibrated on 2026-11-06: each expected file against itself, and GLB → GLB, are 0 pixels
apart in both modes, and the oracle finds no difference on the expected GLBs (the worst
direction 4.6e-5° apart). Dropping the FBX camera turn from the writer fails the oracle's
forward check by exactly 90°, and each of fourteen damaged outputs (a camera's yfov, near
plane, frame or orthographic size, a light's colour or position, a Collada cone written as
the half-angle, …) fails the judge aimed at it; only the oracle sees the near planes.
These have no judge beyond the unit tests: an FBX or Collada orthographic camera's size
(assimp reads neither); an FBX light's range (assimp reads no `FarAttenuationEnd`); and
in USD, where SceneKit reads no UsdLux light and the oracle reads cameras alone, every
light's intensity (usdrecord's pictures are exposed to show each light, so its brightness
is recorded, not judged), a spot's cone, position and direction (Storm draws no cone on a
`SphereLight`, so its footprint is skipped and only its colour judged), and a light the
output has beyond the expected scene's (each picture lights one expected light alone).
The debug run of `staged`, `textured` and `skinned` passed 342 of
342 cases, with 2,737 oracle checks passing and 98 skipped with a citation. The harness
README's "Lights and cameras (mesh-gaps item 5)" has the settings, the calibration, the
controls and every skip's citation.

Six harness judges were wrong about Khronos's plain glTF files. They were fixed on
2026-11-08 with no change to any converter (item 5's follow-ups; the harness README has each
measurement):
- **Unnamed nodes.** An unnamed node is paired by its depth-first rank, the order litegen
  numbers nodes in, not by its array index in the original (Ruling R25, amended).
- **Collada cameras with only `<yfov>`.** A Collada camera with `<yfov>` and no `<xfov>` or
  `<aspect_ratio>`, which is how litegen writes an unset aspect ratio, reads as `aiCamera`'s
  default in assimp: π/4 taken for degrees, 0.0137078 (`ColladaLoader.cpp` L418-L436). So
  the oracle skips assimp's yfov for that camera alone, and judges the file's own `<yfov>`.
- **usdrecord's aspect.** usdrecord draws a camera at the camera's own aspect, so the
  expected picture of a camera with no aspect ratio is drawn 16:9, as it is written.
- **SceneKit's Collada clips.** SceneKit's Collada importer knows a clip by its name, and
  registers none whose name is the id of a node, a geometry or an animation. This is also a
  limit of litegen's output: the writer names a clip after its animation, so a litegen `.dae`
  whose animation is named like a node (Khronos's CesiumMilkTruck: `Wheels`) has a clip
  SceneKit can't play. three.js judges those frames. A writer fix would rename the clip,
  which other readers would show, so it's left to item 8 as an opt-in.
- **SceneKit's USD texture scale.** SceneKit's USD importer ignores `UsdUVTexture`'s
  `inputs:scale`, so it draws a tinted texture untinted. usdrecord and three.js judge it.
- **Repeated material names.** The layer-extension check pairs repeated material names in
  the order the reader keeps them.

Results:
- The plain set (Duck, VirtualCity, CesiumMilkTruck, MorphPrimitivesTest, SunglassesKhronos,
  BrainStem and RiggedFigure, 798 cases) passes 798 of 798, with every native file
  matching. The oracle passes 16,053 checks, fails 0 and skips 785 with a citation. Before
  the fixes it was 761 of 798, with two native differences.
- Every fixture passes 1,140 of 1,140, and item 5's camera set (seven imports, 114 cases
  each) passes 114 of 114 in each. In both, every verdict, oracle count and picture ratio is
  identical to the earlier reports.

Points and lines (mesh-gaps item 10) have a judge of their own, since no two renderers
draw a point at the same size. The page draws a loader's points as `THREE.Points` and its
lines as `THREE.LineSegments`, with materials of its own, in every mode, so the expected
picture (GLTFLoader's) and a loader's agree on what is drawn and where. Each format is
judged by its own three.js loader where that loader builds the primitive; where it cannot
(OBJLoader gives a line no vertex colour, PLYLoader ignores `edge` elements, USDLoader
reads neither `Points` nor `BasisCurves`), the comparison is a skip that cites the
loader's source line, and the triangles stay compared whole against a picture drawn
without what the reader cannot draw. SceneKit judges Collada's `<lines>` and USD's
`BasisCurves`, and a PLY's points; Model I/O reads neither `p` nor `l`. For USD,
usdrecord draws the points and the lines in passes of their own, at 1024 px, and the
**wire judge** compares the bounding box and the centroid of the covered pixels with the
expected picture's, each within 1 % of the frame. A cloud whose points run together is
judged by the box and the centroid of the convex hull of its covered pixels, plus a
precision check and a coarse occupancy one, and a scene a format cannot hold is a refusal
by name, not a failure; the harness README's "How the points are measured" and "A scene a
format cannot hold is a refusal" have both. The first run, on 2026-11-08 (a debug
build, `wire`, `cube` and `textured`, 342 cases) had 43 failures before the work, all of
them `wire`, and passed all 342 after it, with 64 point and line comparisons skipped with
a citation; usdrecord's points came within 1.63 px of the expected picture's
and its lines within 1.13 px, against a limit of 2.56. The negative control, writing OBJ
`l` indices one too high, fails 8 of the OBJ unit tests and the harness's three OBJ `wire`
cases. The harness README's "Points and lines (mesh-gaps item 10)" has the settings and
every skip's citation.

Second opinions:

- **SceneKit** (Apple's renderer via Model I/O, the Quick Look stack);
- **usdrecord** (Pixar's Hydra) for USD, with **usdchecker --arkit** for USDZ and plain
  **usdchecker** for USDA and USDC;
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
- **Khronos lights and cameras samples** (mesh-gaps item 5; a debug build, 2026-11-06):
  `Cameras`, `LightsPunctualLamp`, `DirectionalLight`, `PointLightIntensityTest` and
  `PlaysetLightTest`. Every punctual-lights sample the repository's model index lists
  is here, with `LightVisibility` too, which requires `KHR_node_visibility` and is refused
  by name, as a reader without a required extension must refuse. Beside them are Blender
  5.2.0's FBX and USDA exports of `staged`, made from our GLB through its glTF importer
  (Ruling R28). Each glTF original, drawn by GLTFLoader beside our reading of it, is
  0.00 % apart in `camera` wherever it has a camera and in `lit_lights` wherever it has a
  light. All 798 cases pass, with no native failure: 570 of 570 on the Khronos models and
  228 of 228 on the two Blender exports. Task 6b's first run passed 473 of the 570, and
  each cause of the 97 failures (a case could fail for more than one, so the counts below
  add up to more) was then resolved in the harness, or cited where it lies in an asset:
  - **Names (75 cases), fixed.** The harness paired a camera or light by its name, which
    litegen's Collada names and ids and USD prim names change where they cannot hold a
    character (a space, a dot, …); SceneKit also turns a dot in a Collada node's name into
    a dash. It now pairs each by the writer's sanitised form too.
  - **SceneKit's light colour (8), modelled.** SceneKit decodes a Collada light's
    `<color>` as sRGB (measured; Ruling R26 keeps it linear), so its judge compares with an
    expected picture drawn with that decode.
  - **A glTF `range` (23), modelled.** Collada and USD have no form for it (Ruling R27), so
    SceneKit and Storm light past it, and their expected pictures are drawn without it.
  - **SceneKit's falloff (8), modelled.** SceneKit draws a Collada point light with no
    distance falloff (measured: its brightness over the inverse square grows with the
    distance), so its expected picture of a point or spot light has none.
  - **usdrecord's albedo (14), fixed.** `PlaysetLightTest`'s own lights, a 512 lux sun
    among them, lit usdrecord's albedo pass. Every usdrecord pass but the lights judge's
    now switches the stage's lights off.
  - **`DirectionalLight`'s spheres (15), an asset limit.** They are inside out (every
    normal points in, and their materials are single-sided), so Storm and three.js draw
    them from opposite sides. usdrecord's footprint of that file's sun is skipped, keyed by
    the file's SHA-256, and its colour is still judged.
  - **`LightsPunctualLamp`'s `Point_003` (15), an asset limit.** Storm samples each pixel
    four times where three.js samples once, and this light grazes rims of the lamp's stem
    thinner than a pixel: its footprints part by 8.59 % against the 2 % limit, and no
    other cause measured closes that. usdrecord's footprint of that light is skipped, keyed
    the same way, and its colour is still judged.

  Outside the harness, a script kept in the item's local, git-ignored ledger
  (`.superpowers/sdd/2026-11-04-mesh-lights-cameras/t6b/usd-light-frames.mjs`) compared
  every USD output's lights in Task 6b's run (position, direction, colour and intensity;
  no sample has a spot) with the expected scene, and found no difference.

- **Points and lines** (mesh-gaps item 10; a debug build, 2026-11-08). Three files, each
  through every direction with its own native check:
  - **Khronos `MeshPrimitiveModes`** (CC0, KhronosGroup/glTF-Sample-Assets at
    `4995a638c96dbc94376d3d6164e70fee3ac7f7a2`; one mesh of each of glTF's seven modes,
    as a `.gltf` with its `.bin` and as an embedded `.gltf`). Our reading is one points
    primitive of 7 points, three line primitives and 16 triangles, and GLTFLoader's
    picture of the original is 0.00 % from ours in `geom` and `albedo`. All 228 cases
    pass, with no glTF-Validator error, and 128 point and line comparisons skipped with a
    citation.
  - **Stanford's drill** (`drill_1.6mm_0_cyb.ply`, big-endian, 4,223 vertices and a
    `range_grid` element, no faces). It reads as **4,223 points**, as PLYLoader reads it
    (0.00 % apart in `geom` and `albedo`), and its glb, gltf and obj outputs pass. The
    stl, 3mf, dae and fbx outputs are refused by name ("this scene has 4223 points and no
    triangles"): 69 cases, counted as refusals, not failures. Of the 114, **45 pass, 69
    are refused by name and 0 fail, and the native check is ok**. A first run had 22 fail
    in the harness's judges, not in a file (usdrecord's points were 5.7 px from the
    expected picture's on a 256 px frame, SceneKit's 4.2 px, and assimp refuses a PLY with
    no faces): the pixel centroid had been calibrated on a 9-point grid, and over a cloud
    whose points run together it moves with each renderer's footprint (the expected
    picture covers 8,444 pixels, SceneKit's 6,325). So the cloud, whose 4,223 points make
    2 blobs, is judged by the box and the centroid of the convex hull of its covered
    pixels, which a dot's size moves by half a dot at most (0.03 and 0.19 px apart in
    usdrecord's picture, 0.01 and 0 px in SceneKit's), and by two checks of its contents,
    precision and coarse occupancy (0 pixels outside, and 0 of 178 cells missing and 0
    extra, in the 23 judgements). 38 point comparisons are skipped with a citation
    (three.js 15, SceneKit 23), and 7 assimp validations (a PLY with no faces) apart from
    them. The harness README's "How the points are measured" has the method and the
    measurements.
  - **A `usdcat` crate of `wire`** (our content, committed as
    `src/mesh/testdata/usd/wire.usdc`; Apple USD Tools 0.25.2), wrapped in a USDZ for the
    run, since `--import` picks a reader by extension and has none for `.usdc`. All 114
    cases pass; usdrecord's points are at most 1.63 px from the expected picture's and its
    lines 0.88 px, and three.js's USDLoader, which reads neither `Points` nor
    `BasisCurves`, is a skip citing `USDComposer.js:621-676`, where the generic branch
    that makes them empty objects is `:663-673`.

  A textured points primitive read from a GLB (`texpoints`) passed 8 of 14 cases before
  OBJ gave its points a material without maps (three.js smeared the map across each
  point, 87.5 % of the albedo, and glTF's validator refused the OBJ package's read-back
  for a map with no `TEXCOORD`); it passes 14 of 14 after.

- **KTX2 textures** (mesh-gaps item 3; release builds, 2026-11-08). Khronos
  `glTF-Sample-Assets` at `8f49239`, in their `glTF-KTX-BasisU` form: FlightHelmet (CC0),
  ChronographWatch, StainedGlassLamp and AnisotropyBarnLamp (CC BY 4.0), and CarConcept's
  `glTF-KTX-BasisU-Draco`; plus our `textured.basisu.glb`. The original is drawn by
  GLTFLoader with three r183's `KTX2Loader` and Binomial's own transcoder, which shares
  no code with `mesh/ktx2.rs`, and compared with our PNG reading (`mesh_matrix
  --transcode-textures`, the step production runs). Through the transcoder's RGBA32
  output the albedo from level 0 alone has no value different at all (0 pixels, largest
  channel difference 0, on all six), and `lit` is within one channel step (0 pixels past
  a tolerance of 1; the largest difference 0, and 1 on ChronographWatch). Through this
  machine's GPU formats (ASTC and ETC2) the level-0 albedo is 0 pixels past a difference
  of 20 (largest 1), which sets that limit at 0.1 % of pixels, and the ordinary albedo is
  at most 9.54 % (AnisotropyBarnLamp, the original's mip chain against the GPU's), which
  sets that limit at 10 %. All six pass all 114 cases. StainedGlassLamp first passed 112:
  its two `obj` failures were `obj::prepare` keeping an `alphaMode` of `MASK` that OBJ
  cannot write (a KTX2-free copy of the model failed the same case), fixed in
  `mtl_alpha_and_sides` without moving the OBJ bytes. Every other fixture (1,140 cases)
  is 1,140 of 1,140 with every ratio unchanged. The layer-extension check accepts a KTX2
  image against its PNG when the RGBA8 pixels are equal. A negative control (reversing
  an ETC1S selector's bit order) fails the decoder's reference test on all three ETC1S
  inputs, and in the harness it fails the level-0 albedo passes on `OwnTextured` and
  ChronographWatch and ChronographWatch's `lit` (the with-mips albedo pass does not
  catch it). The
  harness README's "KTX2 originals" has the method and the numbers. The closing gate of
  2026-11-09 (tree e8270b9, a fresh release build) repeated the runs: all six 114 of 114,
  the RGBA32 level-0 albedo again 0 pixels different on all six, every fixture 1,140 of
  1,140, and item 2's plain Khronos set 798 of 798, so models with no KTX2 convert as
  before.

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

`cargo test` adds about 870 unit tests in `mesh/` (868 on 2026-11-05), including the scene
matrix: every fixture, every pair, both write modes, at rest and at every sample time.

Plain `cargo test` (no browser) runs the same matrix structurally in `scene_matrix_tests.rs`.
For each pair it compares the posed scene's triangle count, area, signed volume (which
flips under a mirror or reversed winding), bounds and centroid at every sample time. It also
checks that animation, skins, morphs, textures and vertex colours survive exactly where the
capabilities say they should, and that every material layer arrives with the values the
expected scene holds, field by field and exact as f32, its maps compared by image bytes, uv
set and transform.

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

- **Compressed glTF output:** litegen writes neither Draco nor meshopt. Both are read
  since mesh-gaps item 2 (§3), and a compressed input comes out uncompressed. Still
  refused by name, within them: a Draco point cloud, a pre-2.2 Draco bitstream, a Draco
  primitive in any mode but TRIANGLES or TRIANGLE_STRIP, and a Draco primitive past the
  decoder's limits.
- **KTX2 beyond Basis Universal and uncompressed RGBA8:** WebP and KTX2 are re-encoded as
  PNG on every production path (§3, mesh-gaps item 3). Still left out, with an
  `image_formats` loss that names the file's own value: cube maps, texture arrays, 3D and
  1D textures, a `vkFormat` that is a hardware-compressed block format (BC, ASTC, ETC2),
  ZLIB supercompression, a Basis payload past ETC1S and UASTC LDR 4x4, and a texture past
  the pixel budget. Nor does litegen write a KTX2 or Basis-encode anything (there is no
  PNG-to-KTX2 direction); `ktx2_textures=keep` only keeps what the vendor sent.
- **`.blend`, Alembic, X3D/VRML, STEP:** no meaningful demand from AI-3D consumers; each is
  a large codec.
- **USD beyond what item 6 wrote, and rigid animation per clip:** since mesh-gaps item 6
  (2026-11-10) litegen writes USDC, several clips and unlit (§5). Still not built:
  - **USDC with compressed arrays, or a real LZ4 match finder.** Both are optional (§5:
    arrays are uncompressed and the LZ4 is literal-only), and can come later as a size
    optimisation;
  - **Value clips, splines and the crate features from 0.9 on** (time-code values, path
    expressions, relocates, splines). Nothing litegen writes needs them, and a `timecode`
    attribute is refused by name;
  - **Any variant set litegen didn't write.** Only the default prim's `Animation` set is
    read as clips; other variant sets read by their authored selection, as before;
  - **Switching rigid (xformOp) animation between clips.** Only the first clip's rigid
    motion is kept, and the others' is reported (§5). The follow-up would rig rigidly
    animated nodes as joints with constant bindings (UsdSkel), so that every clip becomes
    a SkelAnimation;
  - **RealityKit's MaterialX unlit surface** (`ND_realitykit_unlit_surfaceshader`). It is
    exact in RealityKit, but it isn't UsdPreviewSurface: Storm renders it white and
    prefers `outputs:mtlx:surface` when both outputs exist, and three.js and SceneKit
    don't draw it. It belongs in a later opt-in "RealityKit profile", not the default.
    Unlit is written as the UsdPreviewSurface recipe (§5);
  - **A `usd` format name.** It names either encoding, so a written format must be `usda`
    or `usdc`; a read takes either under either name (§5).
  The work and its decisions:
  [`plans/2026-11-04-mesh-usd-usdc-clips-unlit.md`](plans/2026-11-04-mesh-usd-usdc-clips-unlit.md)
  and its [design](specs/2026-11-04-mesh-usd-usdc-clips-unlit-design.md).
- **A package as converter input:** the converter reads one file. A zip of an `.obj` with
  its `.mtl` and textures is not unpacked as input; packages go out, as zips, on request
  (§1).
- **Material variants, other shading models, and layers in FBX output:** since mesh-gaps
  item 4 (2026-11-05) the scene carries glTF's nine PBR layer extensions (§1), and each
  container keeps those it has a slot for (§3–§7, and the table in `mesh/mod.rs`). Still
  not supported: `KHR_materials_variants`, alternative material bindings rather than a
  layer, whose variants are dropped with a warning even when the file requires it;
  MaterialX, OpenPBR or MDL networks in USD output, which stays UsdPreviewSurface, the
  model RealityKit and every USD reader draws (MaterialX's `gltf_pbr` would carry the
  layers, but three.js and SceneKit ignore it); layer properties in FBX output (§4); and
  `KHR_materials_pbrSpecularGlossiness`, archived by Khronos, which is refused when
  required and dropped with a warning otherwise. The work and its rulings, each with its
  cost: [`plans/2026-11-04-mesh-material-layers.md`](plans/2026-11-04-mesh-material-layers.md)
  (its status block lists the rulings the code and the harness cite, "Ruling A41" and
  the like).
- **Curves beyond polylines, point size and line width, splat attributes, and 3MF's beam
  lattice:** since mesh-gaps item 10 (2026-11-08) the scene carries points and line
  segments (§1, §3–§7). Still not carried: cubic `BasisCurves`, NURBS, `HermiteCurves` and
  OBJ free-form geometry (`curv`, `surf`), each named in a warning or a refusal; point
  size and line width as data (glTF has neither and every rasteriser draws a pixel; USD's
  `widths` is written as one constant, §5); a Gaussian-splat PLY's `f_dc_*`, `opacity`,
  `scale_*` and `rot_*` (the file reads as its points and colours, with one warning naming
  them); and 3MF's beam lattice. The work and its rulings, each with its cost:
  [`plans/2026-11-04-mesh-points-lines.md`](plans/2026-11-04-mesh-points-lines.md) and its
  [design](specs/2026-11-04-mesh-points-lines-design.md).
- **Lights other than punctual ones, ambient light, and animated light and camera
  properties:** since mesh-gaps item 5 (2026-11-06) the scene carries glTF's punctual
  lights and its cameras (§1). Still not carried, each warned by name: area, dome,
  environment and IES lights (USD's `RectLight`, `DiskLight`, `CylinderLight`,
  `DomeLight`, …, an IES profile; FBX's area and volume lights); ambient light (Collada's
  `<ambient>`, FBX's `AmbientColor`); and animated light or camera properties
  (`KHR_animation_pointer`, FBX curves on `Intensity` or `FieldOfView`, time samples on a
  USD light's or camera's attributes, read at the first). Node transforms still animate
  lights and cameras. Shadows, light linking, depth of field and a physical camera's
  exposure are not read either. The work and its rulings, each with its cost:
  [`plans/2026-11-04-mesh-lights-cameras.md`](plans/2026-11-04-mesh-lights-cameras.md)
  (its "Rulings" section).

## 10. The review of 2026-10-31 to 2026-11-01

A second pass over every conversion path closed the open findings of
[`2026-10-12-mesh-conversion-review.md`](2026-10-12-mesh-conversion-review.md) that change what
a caller gets. Each fix has a test that failed first. What now behaves differently:

- **Hostile input.** Every reader charges what it builds before building it: morph values,
  skin entries, resampled keys, instanced geometry, OBJ/PLY indices, 3MF components, XML
  attributes. Linear-time name uniquifiers, scene validation and lookups replace the quadratic
  ones a deep or wide file could exploit.
- **Polygons.** Concave n-gons are ear-clipped in every reader (`polygon.rs`), as ufbx and
  three.js do, instead of fanned across their notches.
- **glTF writer.** Only valid documents: every local matrix decomposes (glTF 2.0 §5.25;
  shear is baked or approximated and reported), no key time is negative (§3.11), no mesh is
  empty, and a rigid primitive under a skin keeps its node's transform. The reader refuses a
  number past f32's range rather than turning it into infinity.
- **Animation.** STEP keys land on their key in every container; the delivery `fps` reaches
  the containers that resample (FBX declares it as `TimeMode`, USD as `framesPerSecond`, at 30
  time codes per second, the rate three.js assumes). Collada clips play their `start`–`end`.
- **FBX.** SDK and ufbx defaults: `TransparentColor` black, the legacy `Emissive` already holding
  its factor, `RotationActive` on when stated nowhere (ufbx.c:22964). MSVC's `1.#IND` and other
  non-finite numbers, and ASCII files in a legacy code page, are read with a warning.
- **USD.** Shader inputs connected to a material's interface (`inputs:varname.connect`, as
  Blender writes them) are followed; separate metallic and roughness maps are packed into
  glTF's one map (as Blender's glTF exporter merges them); `\xNN` escapes are UTF-8; a value of
  a crate type this reader does not know skips its field, not the layer; Mixamo's `mixamorig:`
  joint names survive as `jointNames`.
- **OBJ, PLY, STL.** Aligned with three.js r170's loaders: `g` groups are parts, `mtllib` names
  may hold spaces, `-s u` scales u alone, unknown map options are named; PLY reads UV and
  colour aliases, face colours and triangle strips, and writes UVs; STL reads facet colours.
  Lines and points were skipped with a warning then; since mesh-gaps item 10 they are read
  and written where the container holds them (§3).
- **Collada and 3MF.** XML text split by a comment is one text; ids never hold `.`;
  technique-level `<newparam>`s are found; a `<constant>` emission map is the unlit colour;
  3MF writes the alpha a material is drawn with and reads a translucent colour as BLEND.
- **Losses.** Each says what is lost, once: default filtering is not a loss, single-file USDA
  keeps the vertex colours it used to report dropped, OBJ and Collada report the alpha they
  cannot draw.
- **The standalone converter** (`POST /v1/utils/models3d/convert`) has its own conversion
  slots, a streamed and timed upload (413, 400 `body_unreadable`, 408 `body_timeout`), litegen
  errors for repeated or unknown parameters, WebP-to-PNG as delivery does, reader warnings in
  `X-Litegen-Conversion-Warnings`, and `units`/`up` (`from_units`/`from_up`) for STL, OBJ and
  PLY: `?to=stl&units=mm&up=z` is what a slicer expects.
- **Two choices left to the client.** Where the evidence did not settle a default
  (review findings 6 and 21), the default is unchanged and a request can override it:
  `format_source` (`native`, the default, asks a vendor for its own fbx/usdz; `glb` asks
  for its GLB and litegen makes the rest) and `fbx_color_space` (`srgb`, the default, as
  three.js reads FBX colours; `linear`, as Blender, Unity, Unreal, ufbx and assimp do),
  which applies to the vendor's FBX a request reads and every FBX it writes and is a converter parameter too.
- **Humanoid rigs.** The VRM, Unity and Mixamo tables name eyes, jaw and fingers (VRM 1.0
  humanoid.md; Unity `HumanBodyBones`; Mixamo's X Bot as three.js ships it), so joint naming
  renames the hands too.

## 11. The open items, closed on 2026-11-01

What was still open in the review's status after §10 is closed, each fix with a test that
failed first:

- **The converter keeps its slot until the file is sent.** A conversion slot was freed when
  the conversion finished, so slow readers could hold any number of converted files in
  memory. The response now streams in 64 KiB copies, declares its `Content-Length`, and
  holds its slot until the last byte or until a send deadline (120 s plus a second per MiB).
  At the deadline the file and the slot are freed, and the transfer fails rather than ending
  short (tested over a real socket).
- **A finished 3D job is delivered once.** The client's poll and the poller each converted
  and re-hosted the same finished job. A delivery now claims a slot of the process's
  delivery slots and a lease on the row (`metadata.leases.delivery`, a compare-and-swap like
  the stage claims, on SQLite and Postgres). An observer without it reports `processing`,
  and the holder's terminal write answers its next poll. The poller polls rows eight at a
  time, so one slow row no longer holds up the tick.
- **Only the observer that delivers a finished job downloads it** (review 2026-11-11, P5).
  The Tripo, Meshy and fal adapters download a finished job's files inside their poll, so
  every observer used to download them before the claim, and the losers dropped them. Now
  an adapter asks a gate (`providers::FetchGate`, answered by the server's
  `delivery_lease::DeliveryGate`) once the vendor says the job finished and before it
  downloads anything: the claim comes first, and an observer without it downloads nothing.
  The delivery slots are therefore held through the downloads too (up to 600 s per file on a
  slow CDN), and the lease (`DELIVERY_LEASE`) is 15 minutes, to cover them. An observer that
  waited 10 s or more for its slot reads the job again for fresh links (Tripo and Meshy sign
  theirs; fal's result is read after the gate anyway). An adapter that downloads nothing in
  its poll (the mock) never asks, and is claimed for after the poll, as before.
- **Litegen reads back what it writes.** FBX normals, tangents and binormals are indexed
  (§4), and compressed containers share one 1 GiB expansion budget (§2), so litegen's own
  textured FBX and 3MF read back at the scene caps.
- **USD, as OpenUSD v26.11.1's source defines it** (§5): time codes per second for sublayers,
  references, internal arcs and `framesPerSecond`; samples beating a default; inherited skin
  bindings and constant primvars; an incomplete SkelAnimation at rest; composition linear in
  its sites (a 6-wide lattice of references cost six times more per level).
- **Materials.** The Phong specular follows the material (§4), so rough surfaces no longer
  wash white in three.js; a dropped metallic-roughness map is decoded once per image, within
  a decode budget, with no second copy.
- **Losses.** A texture on a dropped uv set is reported as a lost texture; resampled USD
  clips and dropped tangents are no longer reported as lost animation and normals.
- **The harness.** A native file that differs, an import our reader refuses, and a
  refusal that is not the expected scene's own each fail the run. Single-file outputs (OBJ,
  glTF, Collada, USDA) are rendered as the converter ships them. The uv mode compares
  whenever a mesh has UVs. A `metal` fixture covers metal and metallic-roughness maps.
  `matrix_tests` runs through the scene API production calls, and the mock derives what it
  does not write from its GLB.

## 12. References

The specifications and reference implementations the code cites, pinned. Paths in source
links are relative to each repository at the version named.

- **glTF 2.0** (Khronos, 2.0.1): https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html:
  the GLB format (`#glb-file-format-specification`), accessors (`#accessors`,
  `#sparse-accessors`), primitive modes (`#meshes-overview`), skins (`#skins`), animation and
  CUBICSPLINE (`#animations`, `#interpolation-cubic`), metallic-roughness
  (`#metallic-roughness-material`) and samplers (`#textures`, `#_filtering`).
- **USD** (OpenUSD v26.11.1, https://github.com/PixarAnimationStudios/OpenUSD/tree/v26.11.1):
  - documentation: https://openusd.org/release/spec_usdpreviewsurface.html (UsdPreviewSurface),
    https://openusd.org/release/spec_usdz.html (USDZ, 64-byte alignment),
    https://openusd.org/release/api/_usd_skel__schemas.html (UsdSkel) and
    https://openusd.org/release/api/class_usd_geom_mesh.html (UsdGeomMesh);
  - source:
    - the crate format: `pxr/usd/sdf/crateFile.h`, `crateFile.cpp`, `crateDataTypes.h` and
      `integerCoding.cpp`;
    - the usda grammar: `pxr/usd/sdf/textFileFormatParser.h` and `parserHelpers.cpp`;
    - time scaling: `pxr/usd/pcp/layerStack.cpp` and `primIndex.cpp`;
    - value resolution: `pxr/usd/usd/stage.cpp`;
    - the skinning cache: `pxr/usd/usdSkel/cacheImpl.cpp`, `animQueryImpl.cpp` and
      `skeletonQuery.cpp`;
    - primvar inheritance: `pxr/usd/usdGeom/primvarsAPI.cpp` and
      `pxr/imaging/hd/flattenedPrimvarsDataSourceProvider.cpp`.
- **FBX** (no public specification):
  - the Autodesk FBX SDK 2020 reference, https://help.autodesk.com/cloudhelp/2020/ENU/FBX-Developer-Help/files/cpp_ref/:
    `FbxLayerElement` mapping and reference modes, `FbxAnimCurveDef`, `FbxSurfacePhong` and
    `FbxPose`;
  - ufbx v0.23.1 (https://github.com/ufbx/ufbx/blob/26a482ae66871d7de36eb722aa060bce95bce274/ufbx.c):
    the binary node format, `ufbxi_read_mesh`, `ufbxi_read_vertex_element`,
    `ufbxi_read_animation_curve` and the property-template defaults;
  - Blender v5.2.3's importer, `scripts/addons_core/io_scene_fbx/parse_fbx.py` and
    `import_fbx.py` (https://github.com/blender/blender/tree/v5.2.3).
- **COLLADA 1.4.1** (Khronos, 2nd edition, 2008): https://www.khronos.org/files/collada_spec_1_4.pdf:
  - the primitives, 5-96 to 5-130: `<polygons>`, `<polylist>`, `<triangles>`, `<trifans>`,
    `<tristrips>`;
  - `<input>`, 5-39;
  - `<skin>`, 5-112, and the skinning equation, 4-6;
  - `<animation>`, `<sampler>` and `<channel>`, 5-11, 5-21 and 5-102;
  - curve interpolation, 4-1 to 4-6;
  - `<morph>`, 5-81;
  - address syntax, 3-1.
- **3MF**: Core Specification 1.3.0 (https://github.com/3MFConsortium/spec_core/blob/1.3.0/3MF%20Core%20Specification.md)
  and the Materials and Properties Extension 1.2.1
  (https://github.com/3MFConsortium/spec_materials/blob/1.2.1/3MF%20Materials%20Extension.md).
- **OBJ, MTL, PLY and STL**:
  - OBJ: https://paulbourke.net/dataformats/obj/;
  - MTL: https://paulbourke.net/dataformats/mtl/ (`Tr` is an exporter extension, not in
    the text);
  - PLY: https://paulbourke.net/dataformats/ply/;
  - STL: https://www.fabbers.com/tech/STL_Format, with colour conventions at
    https://www.loc.gov/preservation/digital/formats/fdd/fdd000504.shtml. The 1989 3D Systems
    specification has no stable URL.
- **three.js r170's loaders**, the browser behaviour the converter matches
  (https://github.com/mrdoob/three.js/tree/r170/examples/jsm/loaders):
  - `GLTFLoader.js`: default sampler filters;
  - `OBJLoader.js` and `MTLLoader.js`: `o`/`g`, and `Ks`, `Ns`, `d` and `Tr`;
  - `PLYLoader.js`: property aliases;
  - `STLLoader.js`: Magics `COLOR=`;
  - `FBXLoader.js` and `USDLoader.js`.
