# USD/USDZ, 3MF and COLLADA: implementation reference for a glTF-hub converter (Rust, in-process)

> Kept in the repo since 2026-10-12. Paths below that start `scratchpad/fbx/` or `scratchpad/usd3mfdae/` now live under `docs/superpowers/research/fbx/` and `usd3mfdae/`; only our scripts and hand-written inputs were kept (see [README](README.md)).

Researched 2026-10-12. The hub is glTF 2.0: node hierarchy, PBR metallic-roughness, embedded PNG/JPEG,
COLOR_0, skins + IBMs, morph targets, TRS/weight animations.

**Verification legend**
- **[L]**: verified locally in this session with a real tool. The tools were:
  - Apple USD Tools 0.25.2 (`/usr/bin/usdcat`, `usdchecker`, `usdzip`, `usdrecord`, `usdcrush`), built on USD 25.02.
  - assimp 6.0.
  - three.js r183 (0.183.2) loaders, run in Node 24 or in headless Chrome 154.
  - xmllint.
  - An independent stdlib-Python USDC decoder written for this report.
- **[S]**: verified against primary source text, either a spec or upstream source code. The file or URL is cited.
- **[UNVERIFIED]**: not confirmed from a primary source or a real tool.

Test assets, scripts and renders live in `scratchpad/usd3mfdae/`. File paths are listed in the appendix.

---

## 0. Summary: the facts that decide correctness

### 0.1 Does `usdchecker --arkit` accept a usda root layer?

**Yes**, with Apple USD Tools 0.25.2 [L].

- `usdzip quad_usda_root.usdz quad.usda textures/*.png` produced a package that `usdchecker --arkit` and `--arkit -t` (strict) report as `Success!`, exit 0.
- The old rule "The root layer of the package must be a usdc file…" (`ARKitRootLayerChecker`) existed in OpenUSD's Python checker through v23.11. It was removed in v24.03, under the changelog entry "Updated ARKit conformance checks to match spec from WWDC 2023" [S].
- The usdz spec source still says "Apple's current usdz implementation allows only a single usdc file, but this restriction will be lifted in future OS updates" [S]. That note dates from iOS 12.
- Apple's 2026 feature matrix lists `.usd / .usdc / .usda` as supported by RealityKit, Raytracer, Storm and SceneKit [S].
- Google model-viewer sends three.js `USDZExporter` output (root `model.usda`, which references `geometries/*.usda`) to AR Quick Look whenever `ios-src` is absent [S]. That is broad shipped evidence that Quick Look accepts usda roots.
- **Not device-tested here.** Apple's own packagers convert to usdc: `usdzip --arkitAsset` and `usdcrush` both rewrite a usda root to `quad.usdc` (crate 0.8.0) [L].

### 0.2 USDZ zip rules

**Stored entries only.** A deflated usdz cannot be opened at all: "compressed files are not supported" [L].

**Every file's data must start at a multiple of 64 bytes from the package start.**
- OpenUSD pads with a local-header extra field whose header ID is 0x1986.
- three.js pads with ID 0x3039. Both pass the checker [L][S].

**The root layer must be the first entry.** A package with the PNG first fails with "Failed to open stage" [L].

**OpenUSD reads the package by walking local headers sequentially**, using each header's 32-bit sizes [S]. Therefore:
- no data descriptors (flag bit 3);
- no zip64;
- no gaps between entries.

### 0.3 UVs for every target

glTF UV `(u, v)` maps to `(u, 1 - v)` in USD `st`, COLLADA `TEXCOORD` and 3MF `tex2coord`. All three are lower-left-origin.
- USD [S][L]: the UsdPreviewSurface spec says texture coordinates "must be 't flipped'". A usdrecord render was upright.
- COLLADA [L]: the three.js render was upright, and assimp's glTF→DAE export also flips V.
- 3MF [S][L]: Materials spec "origin in the lower left of the texture". The three.js render was upright.

### 0.4 UsdSkel

- **bindTransforms** = `inverse(glTF IBM)`: the joints' bind matrices in skeleton/world space [S][L]. Using the IBM itself visibly deforms the rest pose (render negative control).
- **restTransforms** = local joint TRS.
- **Joints must be ordered parents-first.** Otherwise the runtime rejects the skeleton ("Joint 0 has mis-ordered parent 1…") and usdrecord aborts. `usdchecker` does **not** catch this [L].
- **SkelAnimation attribute types are fixed:** `float3[] translations`, `quatf[] rotations`, `half3[] scales`. A `float3[] scales` is **silently ignored** at runtime, and usdchecker still passes [L].

### 0.5 Quaternion order

USDA text is `(w, x, y, z)`; the crate binary is `(x, y, z, w)` [L].

### 0.6 Matrix order

- **USD** is row-vector math. Writing glTF's 16 column-major floats in order as USD's 4 row tuples gives the correct matrix [L].
- **COLLADA** is column-vector math written row-major in the text, i.e. the transpose of the glTF array [S][L].
- **3MF** writes `m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32` in row-vector convention. These are glTF's column-major elements `[0,1,2,4,5,6,8,9,10,12,13,14]` [S][L].

### 0.7 Apple/RealityKit must-dos

From the Apple docs [S] and usdchecker [L]:
- Set `subdivisionScheme = "none"`. The default is catmullClark, and RealityKit subdivides.
- Apply `MaterialBindingAPI` / `SkelBindingAPI`.
- Author `wrapS`/`wrapT` explicitly. The default `useMetadata` falls back to *black* outside 0..1.
- Use `sourceColorSpace = "raw"` for data textures. `auto` treats 8-bit RGB/RGBA as sRGB.
- Normal maps need scale (2,2,2,1), bias (-1,-1,-1,0) and raw.
- `inputs:varname` must be `string`, not token.
- Colour inputs must be `color3f`.
- A `materialBind` GeomSubset family needs `uniform token subsetFamily:materialBind:familyType = "nonOverlapping"` or `"partition"`.
- Use one skeleton per model.
- RealityKit has no `doubleSided`, no texture bias, no scale on normal maps, one UsdTransform2d per material, 2 UV sets and one packed texture.

### 0.8 USD time

- Samples interpolate **linearly**: quats are slerped, arrays are element-wise only when sizes match (otherwise held), and value blocks are held [S].
- No layer metadata selects "held". `interpolationType` in a layer round-trips but has **no effect** [L].
- Emulate glTF STEP by emitting `(tᵢ, vᵢ)` plus `(tᵢ₊₁ − ε, vᵢ)`. Fractional time codes are legal [L].

### 0.9 three.js r183 USDLoader bugs that affect writers

- USDA `quatf[]` (SkelAnimation rotations) are not reordered from (w,x,y,z), so **skeletal animation from USDA is wrong**; USDC is correct [L].
- A USDC `double3` whose components are small integers (inlined in the crate) **throws** (`makeTranslation` of undefined). Prefer `float3 xformOp:translate/scale` [L].
- USDC `string` values decode wrongly ("st" read as "defaultPrim"), because three.js includes the STRINGS section's 8-byte count prefix in the table [L].
- Faces not covered by any GeomSubset are dropped [L].
- No BlendShape support; displayColor is used only as a constant material colour [S: source].

### 0.10 3MF

- OPC zip, Deflate or stored.
- Consumers MUST support ZIP64 and streaming; producers SHOULD avoid ZIP64 unless needed.
- Z-up, millimetre default.
- `type="model"` meshes MUST be manifold, consistently oriented, outward (CCW) and have at least 4 triangles.
- An object with property-bearing triangles MUST carry object-level `pid`/`pindex`.
- displaycolor is sRGB `#RRGGBB[AA]` [S].
- Consumers:
  - three.js reads basematerials, colorgroup and texture2d [L].
  - Bambu Studio reads `m:colorgroup` only (as filament mapping) [S: source].
  - PrusaSlicer 2.9.4 reads none of the Materials extension [S: source].
  - Cura reads texture2d/UVs only as a PNG "paint texture" [S: source].
- assimp 6.0's 3MF export is invalid: two `<mesh>` per object, empty objects, metadata inside `<resources>`, no transforms or units. three.js crashes on it [L].

### 0.11 COLLADA 1.4.1

- three.js ColladaLoader supports:
  - skins: joints from a `Name_array` of node **sids**, `INV_BIND_MATRIX`, `bind_shape_matrix`;
  - node animation of `matrix`/`translate`/`rotate`/`scale`, where matrix keys are decomposed to TRS and **STEP on matrix channels becomes linear**;
  - profile_COMMON constant/lambert/phong/blinn;
  - textures via relative `<init_from>`.
- three.js ColladaLoader does not support:
  - morph: any morph controller **crashes** the load, and so does a sid-less weight channel target;
  - per-vertex colours, which are loaded but `vertexColors` is never enabled [L].
- assimp 6.0:
  - its glTF→DAE export of an animated skin writes animation samplers whose `<source>`s are missing, so neither assimp nor three.js can read it back [L];
  - its COLLADA morph import is broken by a loop bug [S][L].
- Blender 5.0 removed COLLADA import/export [S].
- `.zae` loads in assimp (with or without `manifest.xml`) and in three.js via `KMZLoader`'s fallback [L].

---

## A. USD / USDZ

### A1. USDZ packaging

**Spec (usdz spec v1.3)** [S] (`docs/spec_usdz.rst`, https://openusd.org/dev/spec_usdz.html):
- "A usdz package is a zero compression, unencrypted zip archive."
- "the data for each file begin at a multiple of 64 bytes from the beginning of the package."
- "the first file in the package must be a native usd file" and it is "always the first file in the package" (the Default Layer). A root-level `defaultLayer.usd` is also recognised by the packaging API.
- Allowed types: **usda, usdc, usd** (nested **usdz** is allowed), **png, jpeg, OpenEXR, AVIF** (AVIF was added in v1.3), and **m4a, mp3, wav**.
- The spec does not mention zip64.

**OpenUSD's zip implementation** [S] (`pxr/usd/sdf/zipFile.cpp`, release branch):

```cpp
// Compute the number of padding bytes (including header) needed to align
uint16_t requiredPadding = _DataAlignment - (offset % _DataAlignment);   // _DataAlignment = 64
if (requiredPadding == _DataAlignment) requiredPadding = 0;              // already aligned: no extra field
else if (requiredPadding < _HeaderSize) requiredPadding += _DataAlignment; // need room for 4-byte header
header.headerId = 0x1986; header.dataSize = numPaddingBytes - _HeaderSize;
```

- `offset` is the position just after `LFH(30) + filename`.
- All size fields are `uint32`. There is no zip64 code anywhere in the file.
- The **reader iterates local file headers** from offset 0: it reads an LFH, skips `compressedSize`, and reads the next LFH. It does not use the central directory to find entries.

**Measured `usdzip` output** [L] (`scripts/zipinspect.py`):

```
LFH @0:    ver=10 flags=0x0000 method=0 csize=usize=5606 name='quad.usda' extra=[0x1986 len=21] data@64   (mod64=0)
LFH @5670: ver=10 flags=0x0000 method=0 csize=141 name='textures/basecolor.png' extra=[0x1986 len=34] data@5760 (mod64=0)
LFH @5901: ver=10 flags=0x0000 method=0 csize=73  name='textures/mr.png'        extra=[0x1986 len=2]  data@5952 (mod64=0)
CD: made_by=0x0000 need=10 flags=0 method=0 (CD extra repeats the padding field); no zip64 EOCD
```

That listing has CRC filled in and sizes in the LFH. three.js `USDZExporter` writes `ver=20`, padding ID 0x3039 and `fflate zipSync({level:0})`, and also passes `--arkit` [L].

**Rust writer recipe.** For each file, in order (root layer first):
1. Compute CRC-32 and size.
2. Compute the extra-field padding from the current offset with the formula above.
3. Write the LFH:
   - signature `0x04034b50`, version 10 (or 20), flags 0, method 0;
   - dos time/date, CRC, csize = usize, name length, extra length;
   - the name, using ASCII and `/` separators;
   - the extra field: `[0x1986:u16][n:u16][n zero bytes]`.
4. Write the raw bytes.

Then write the central directory, mirroring each entry with the same fields and LFH offset, followed by the EOCD. Constraints:
- No data descriptors, no zip64, no encryption, no directory entries required.
- Total size must stay below 4 GiB.
- Reference textures with **layer-relative** asset paths, e.g. `@textures/a.png@` from a root layer at package root.

**Negative tests** [L]:

| Package | Result |
|---|---|
| python `zipfile` DEFLATED | Stage won't open: `Cannot open quad.usda in …deflate.usdz: compressed files are not supported` |
| python `zipfile` STORED (unaligned) | `UsdzPackageValidator.ByteMisalignment … File 'quad.usda' … has an invalid offset 39` (one error per file) |
| PNG listed first | `Failed to open layer` / `Failed to open stage` |
| `.tga` in package | `FileExtensionValidator.UnsupportedFileExtensionInPackage … 'tga'` |
| `.avif` in package | accepted, even though the validator's doc string lists only exr/jpg/jpeg/png |
| texture referenced outside package | `PackageEncapsulationValidator.AssetNotInPackage` **warning**; exit 0 unless `-t/--strict` |

Two tool notes:
- `usdzip -c` (checkCompliance) crashes with SIGBUS (exit 138) in this build, for both usda and usdc roots [L].
- `usdzip --arkitAsset quad.usda out.usdz` flattens, converts the root to `quad.usdc` and relocates textures to `0/*.png` [L].

### A2. ARKit / RealityKit / AR Quick Look constraints

**`usdchecker --arkit -d`** (Apple USD Tools 0.25.2; new C++ validation framework), verbatim [L]:

```
[usdGeomValidators:StageMetadataChecker]:
	Doc: All stages must declare their 'upAxis' and 'metersPerUnit'.
	Keywords: UsdGeomValidators
	isSuite: False
[usdGeomValidators:SubsetFamilies]:
	Doc: Validates all of the geom subset families authored beneath an Imageable prim.
	Keywords: UsdGeomValidators, UsdGeomSubset
	SchemaTypes: UsdGeomImageable
	isSuite: False
[usdGeomValidators:SubsetParentIsImageable]:
	Doc: Validates that GeomSubset prims are direct descendants of an Imageable prim.
	Keywords: UsdGeomValidators, UsdGeomSubset
	SchemaTypes: UsdGeomSubset
	isSuite: False
[usdShadeValidators:EncapsulationRulesValidator]:
	Doc: Connectable prims (e.g. Shader, Material, etc) can only be nested inside other Container-like Connectable prims. Container-like prims include Material, NodeGraph, Light, LightFilter. Shader is not a Container-like prim.
	Keywords: UsdShadeValidators
	isSuite: False
[usdShadeValidators:MaterialBindingApiAppliedValidator]:
	Doc: Verify a prim has the MaterialBindingAPI applied if it has a material binding relationship.
	Keywords: UsdShadeValidators
	isSuite: False
[usdShadeValidators:MaterialBindingCollectionValidator]:
	Doc: Verify that a collection defining a material binding is well-formed
	Keywords: UsdShadeValidators
	SchemaTypes: MaterialBindingAPI
	isSuite: False
[usdShadeValidators:MaterialBindingRelationships]:
	Doc: All properties named 'material:binding' or in that namespace should be relationships.
	Keywords: UsdShadeValidators
	isSuite: False
[usdShadeValidators:NormalMapTextureValidator]:
	Doc: UsdUVTexture nodes that feed the _inputs:normals_ of a UsdPreviewSurface must ensure that the data is encoded and scaled properly. Specifically, since normals are expected to be in the range [(-1,-1,-1), (1,1,1)], the Texture node must transform 8-bit textures from their [0..1] range by setting its _inputs:scale_ to (2, 2, 2, 1) and _inputs:bias_ to (-1, -1, -1, 0). Normal map data is commonly expected to be linearly encoded.  However, many image-writing tools automatically set the profile of three-channel, 8-bit images to SRGB.  To prevent an unwanted transformation, the UsdUVTexture's _inputs:sourceColorSpace_ must be set to raw.
	Keywords: UsdShadeValidators
	SchemaTypes: UsdShadeShader
	isSuite: False
[usdShadeValidators:ShaderSdrCompliance]:
	Doc: Shader prim's input types must be conforming to their appropriate sdf types in the respective sdr shader.
	Keywords: UsdShadeValidators
	SchemaTypes: UsdShadeShader
	isSuite: False
[usdShadeValidators:SubsetMaterialBindFamilyName]:
	Doc: Geom subsets with authored material bindings should have the 'materialBind' family name.
	Keywords: UsdShadeValidators, UsdGeomSubset
	SchemaTypes: UsdGeomSubset
	isSuite: False
[usdShadeValidators:SubsetsMaterialBindFamily]:
	Doc: Geom subsets of the 'materialBind' family should have a restricted family type.
	Keywords: UsdShadeValidators, UsdGeomSubset
	SchemaTypes: UsdGeomImageable
	isSuite: False
[usdSkelValidators:SkelBindingApiAppliedValidator]:
	Doc: Verify a prim has the SkelBindingAPI applied if it has a UsdSkelBinding property.
	Keywords: UsdSkelValidators
	isSuite: False
[usdSkelValidators:SkelBindingApiValidator]:
	Doc: Verify that a prim with SkelBindingAPI applied, is either of SkelRoot type or parented by a SkelRoot prim.
	Keywords: UsdSkelValidators
	SchemaTypes: UsdSkelBindingAPI
	isSuite: False
[usdUtilsValidators:FileExtensionValidator]:
	Doc: Only valid core layer extensions (.usd, .usda, .usdc, .usdz), valid core texture extensions (.exr, .jpg, .jpeg, .png) and embedded audio files (.M4A, .MP3, .WAV) are allowed in a package.
	Keywords: UsdUtilsValidators, UsdzValidators
	isSuite: False
[usdUtilsValidators:MissingReferenceValidator]:
	Doc: The composed USD stage should not contain any unresolvable asset dependencies (in every possible variation of the asset), when using the default asset resolver.
	Keywords: UsdUtilsValidators
	isSuite: False
[usdUtilsValidators:PackageEncapsulationValidator]:
	Doc: If the root layer is a package, then its recommended for the composed stage to not contain references to files outside the package. The package should be self-contained, warn if not.
	Keywords: UsdUtilsValidators, UsdzValidators
	isSuite: False
[usdUtilsValidators:UsdzPackageValidator]:
	Doc: Files within all usdz packages within a stage should not be compressed or encrypted, and should be aligned to 64 bytes.
	Keywords: UsdUtilsValidators, UsdzValidators
	isSuite: False
[usdValidation:CompositionErrorTest]:
	Doc: Validator aims at providing all composition errors, which were generated while composing the stage.
	Keywords: UsdCoreValidators
	isSuite: False
[usdValidation:StageMetadataChecker]:
	Doc: Stages that can be consumed as referenceable assets must have a valid 'defaultPrim' specified.
	Keywords: UsdCoreValidators
	isSuite: False
```

- `--arkit` only *adds* `FileExtensionValidator`, `PackageEncapsulationValidator` and `UsdzPackageValidator` to the default set. There is no prim-type, root-layer or shader-id allow-list any more [L].
- Upstream OpenUSD history [S] (CHANGELOG):
  - 24.03: "Updated ARKit conformance checks to match spec from WWDC 2023".
  - 25.11: `--arkit` deprecated; usdchecker now validation-framework only.
  - 26.x: "Removed deprecated `--arkit` command line option".
- The legacy Python ARKit rules (≤23.11) were:
  - `ARKitRootLayerChecker`: "The root layer of the package must be a usdc file and must not include any external dependencies that participate in stage composition."
  - `ARKitPrimTypeChecker`, which allowed `'' Scope Xform Camera Shader Material Mesh Sphere Cube Cylinder Cone Capsule GeomSubset Points SkelRoot Skeleton SkelAnimation BlendShape SpatialAudio PhysicsScene…`; PointInstancer was rejected.
  - `ARKitShaderChecker`: info:id must be `Usd*` (later also `ND_*`).
  - `ARKitLayerChecker`, `ARKitMaterialBindingChecker`, `ARKitFileExtensionChecker`.

**What the 0.25.2 checker catches and misses** [L] (`neg/`, `usd/`):

| Authored problem | Result |
|---|---|
| no `upAxis`/`metersPerUnit`/`defaultPrim` | 3 errors (StageMetadataChecker ×2 + MissingDefaultPrim) |
| `token inputs:varname` | `ShaderSdrCompliance.MismatchedPropertyType … Expected 'string'; got 'token'` |
| `float3 inputs:emissiveColor` | `… Expected 'color3f'; got 'float3'` |
| `rel material:binding` without `MaterialBindingAPI` | `MaterialBindingApiAppliedValidator.MissingMaterialBindingAPI` |
| `info:id = "MyCustomShader"` | `ShaderSdrCompliance.MissingShaderIdInRegistry` |
| GeomSubset family without family type | `SubsetsMaterialBindFamily.InvalidFamilyType … 'unrestricted' … should be 'nonOverlapping' or 'partition'` |
| PointInstancer | **not flagged** |
| Skeleton joints child-before-parent | **not flagged**, but the runtime rejects the skeleton |
| SkelAnimation `float3[] scales` | **not flagged**, but the runtime ignores it |
| usda root layer in usdz | **passes** |

**Apple renderer support (Apple doc "Validating feature support for USD files", 2026)** [S] (https://developer.apple.com/documentation/usd/validating-usd-files.md):

| Feature | RealityKit | Notes |
|---|---|---|
| .usdz, .usd/.usdc/.usda | ✔ | |
| Transform animation | ✔ | |
| Vertex animation | ✗ | Storm and Raytracer do support it |
| Double-sided meshes | ✗ | |
| Subdivision | ✔ | RealityKit subdivides UsdPreviewSurface meshes |
| Geometry subsets, vertex colours, primitive shapes (no plane), purpose | ✔ | |
| Point instancing | ✗ | |
| Skeletons, skeleton animation, **blend shapes** | ✔ | all four renderers |
| UsdPreviewSurface, textures, AVIF, wrap modes, colorSpace | ✔ | |
| Texture channel refs | partial | "only a single packed texture per material" |
| Texture transforms | partial | "a single `UsdTransform2d` per material" |
| Multiple UV sets | partial | two UV sets |
| Scale | partial | "except for normal map textures" |
| Bias | ✗ | |
| Specular workflow | ✗ | |
| Lights | ✗ | RealityKit ignores USD lights |

From the same docs, "Creating USD files for Apple devices" [S]:
- "In macOS 26, QuickLook uses RealityKit to render USDZ files."
- "RealityKit uses Y-up orientation and meters".
- USDZ "may only include JPEG, PNG, EXR, and AVIF".
- "The default `subdivisionScheme` value in USD is `catmullClark`… explicitly set `subdivisionScheme` to `none`".
- "Limit rigged models to a single skeleton".
- "USD requires that any geometry with an applied material use the `MaterialBindingAPI`… any geometry that has a skeleton use the `SkelBindingAPI`".
- "RealityKit expects normal maps in OpenGL format". glTF normal maps are already OpenGL (+Y).
- Variants on the defaultPrim show as Quick Look configuration options.

**Quick Look and usda roots.** Evidence that Quick Look accepts usda-rooted usdz:
- The Apple matrix above.
- model-viewer `packages/model-viewer/src/features/ar.ts` [S]: `const generateUsdz = !this.iosSrc; … objectURL = generateUsdz ? await this.prepareUSDZ() : this.iosSrc; … anchor.setAttribute('rel','ar')`, where `prepareUSDZ()` calls `new USDZExporter().parseAsync(...)`.
- three.js `USDZExporter`'s `quickLookCompatible` option exists to work around Quick Look texture-transform bugs (FB10036297, FB11442287), i.e. Quick Look renders its usda files.

**[UNVERIFIED on device]**: no iOS device was used. Recommendation: USDA root is fine for current Apple OSes. If you ever need iOS 12-era compatibility or Apple-tool parity, ship a usdc root.

### A3. USDA grammar subset a reader/writer needs

Everything below parsed with `usdcat` and round-tripped [L] (`usd/grammar.usda`).

```usda
#usda 1.0
(
    "Layer doc string"
    customLayerData = {
        string creator = "litegen"
    }
    defaultPrim = "Root"
    metersPerUnit = 1
    upAxis = "Y"
    startTimeCode = 0
    endTimeCode = 48
    timeCodesPerSecond = 24
    framesPerSecond = 24
)

# line comment    // also a comment    /* block comment */   (all three accepted; usdcat drops them)

class "_Proto" { custom float protoValue = 1 }      # class = abstract prim, inherited via inherits = </_Proto>

def Xform "Root" (
    kind = "component"
    inherits = </_Proto>
    prepend apiSchemas = ["MaterialBindingAPI"]      # applied API schemas are a prepend list-op
)
{
    string s1 = "quote\" backslash\\ newline\n tab\t"   # escapes \" \\ \n \t verified; \r, \' [UNVERIFIED]
    string s2 = 'single "quoted" text'
    string s3 = """multi
line"""                                              # ''' or """ for multi-line
    asset a1 = @textures/my file.png@                 # spaces OK inside @...@
    asset a2 = @@@weird@name.png@@@                   # triple-@ when the path contains '@'
    custom double myCustom = 3.5                      # 'custom' = not part of the schema
    uniform token tk = "uniformToken"                 # 'uniform' = not animatable
    token[] tokArr = ["a", "b"]
    int[] emptyArr = []
    bool flag = true                                  # usdcat writes 1
    float blocked = None                              # value block
    double3 xformOp:translate.timeSamples = {
        0: (0, 0, 0),
        2.5: (1, 0, 0),                               # fractional time codes are fine
        10: (2, 0, 0),
    }
    quatf xformOp:orient = (1, 0, 0, 0)               # TEXT ORDER IS (w, x, y, z)
    float3 xformOp:scale = (1, 1, 1)
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:orient", "xformOp:scale"]
    rel material:binding = </Root/Materials/Mat>      # relationship target
    color3f inputs:diffuseColor.connect = </Root/Materials/Mat/Tex.outputs:rgb>   # connection

    def Scope "Child_1" { }
}

over "OverOnly" { def Xform "AddedByOver" { } }      # 'over' = opinion without defining
```

Rules learned [L]:
- **One spec per path per layer.** `over "Root"` after `def Xform "Root"` in the same file fails with "Duplicate prim 'Root'".
- **Prim names**:
  - `Bad-Name`, `1Leading` and `has space` fail ("not a valid prim name").
  - `Café` and `Grüße_日本` are accepted. OpenUSD ≥24.03 supports UTF-8 identifiers.
  - For portability (three.js's USDA parser uses `\w` regexes; older iOS) emit `[A-Za-z_][A-Za-z0-9_]*` and sanitize glTF names. Keep the original name in `customData` or `displayName`.
- `usdcat` canonicalises order: metadata and properties come out alphabetised, with double-quoted strings unless they contain newlines/quotes, in which case `'''` is used.
- Unknown layer metadata (e.g. `bogusKey = "x"`) round-trips **without error**, so don't treat a successful parse as proof that a field is meaningful.

**Attribute types used by this converter:**

| glTF data | USD type | Notes |
|---|---|---|
| positions | `point3f[] points` | |
| normals | `normal3f[] normals` (or `primvars:normals`) | interpolation `"vertex"` or `"faceVarying"` |
| TEXCOORD_n | `texCoord2f[] primvars:st`, `st1`… | |
| COLOR_0 | `color3f[] primvars:displayColor` + `float[] primvars:displayOpacity` | |
| extent | `float3[] extent` | `[(minx,miny,minz),(maxx,maxy,maxz)]` |
| node TRS | `double3`/`float3 xformOp:translate`, `quatf xformOp:orient`, `float3 xformOp:scale` | or `matrix4d xformOp:transform` |
| joint matrices | `uniform matrix4d[] bindTransforms/restTransforms` | |
| skin | `int[] primvars:skel:jointIndices`, `float[] primvars:skel:jointWeights` | `elementSize = N`, `interpolation = "vertex"` |
| anim | `float3[] translations`, `quatf[] rotations`, `half3[] scales`, `float[] blendShapeWeights` | **exact types required** |
| morph target | `uniform vector3f[] offsets`, `uniform int[] pointIndices`, `uniform vector3f[] normalOffsets` | |

**Primvar interpolation** is metadata in parentheses after the value:
- `constant`: 1 value;
- `uniform`: 1 per face;
- `vertex`: 1 per point;
- `faceVarying`: 1 per face-vertex, in `faceVertexIndices` order;
- `varying` also exists (treat like vertex for meshes).

An indexed primvar adds `int[] primvars:NAME:indices`. Its values array then holds the unique values and the indices array has the interpolation's element count.

### A4. Meshes

The verified static example is `usd/quad.usda`. It passes `--arkit` and is rendered by usdrecord in `renders/quad.png`.

```usda
def Mesh "Strip" (
    prepend apiSchemas = ["MaterialBindingAPI"]
)
{
    float3[] extent = [(-0.5, 0, 0), (0.5, 2, 0)]
    int[] faceVertexCounts = [3, 3, 3, 3]
    int[] faceVertexIndices = [0, 1, 3, 0, 3, 2, 2, 3, 5, 2, 5, 4]
    point3f[] points = [(-0.5, 0, 0), (0.5, 0, 0), (-0.5, 1, 0), (0.5, 1, 0), (-0.5, 2, 0), (0.5, 2, 0)]
    normal3f[] normals = [(0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1)] (
        interpolation = "vertex"
    )
    texCoord2f[] primvars:st = [(0, 0), (1, 0), (0, 0.5), (1, 0.5), (0, 1), (1, 1)] (
        interpolation = "faceVarying"
    )
    int[] primvars:st:indices = [0, 1, 3, 0, 3, 2, 2, 3, 5, 2, 5, 4]
    uniform token subdivisionScheme = "none"
    rel material:binding = </Root/Materials/Textured>
}

def Mesh "TwoMat" (
    prepend apiSchemas = ["MaterialBindingAPI"]
)
{
    float3[] extent = [(1, 0, 0), (2, 1, 0)]
    int[] faceVertexCounts = [3, 3]
    int[] faceVertexIndices = [0, 1, 2, 0, 2, 3]
    point3f[] points = [(1, 0, 0), (2, 0, 0), (2, 1, 0), (1, 1, 0)]
    color3f[] primvars:displayColor = [(1, 0, 0), (0, 1, 0), (0, 0, 1), (1, 1, 1)] (
        interpolation = "vertex"
    )
    uniform token subdivisionScheme = "none"
    uniform bool doubleSided = 1
    uniform token subsetFamily:materialBind:familyType = "nonOverlapping"   # REQUIRED by validator
    rel material:binding = </Root/Materials/Red>                            # fallback for faces not in a subset

    def GeomSubset "Face1" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        uniform token elementType = "face"
        uniform token familyName = "materialBind"
        int[] indices = [1]
        rel material:binding = </Root/Materials/Green>
    }
}
```

**Topology.** `faceVertexCounts` holds per-face vertex counts (3 for glTF triangles). `faceVertexIndices` holds point indices. Polygons are allowed: USD readers must triangulate n-gons, and three.js uses fan for quads and earcut for n>4.

**Orientation.** `uniform token orientation` defaults to `"rightHanded"`: faces wound CCW are front-facing, as in glTF.
- `"leftHanded"` reverses that. Rendered with usdrecord [L] (`renders/orient.png`): a CCW triangle with `leftHanded` shades like a CW triangle with `rightHanded`, i.e. back-facing.
- Reader rule: if `leftHanded`, reverse each face's vertex order. Reverse faceVarying primvars and indices with the same permutation.
- Writer rule: never write leftHanded.

**Normals.**
- `vertex` normals map directly to glTF.
- `faceVarying` normals (common from DCCs) require splitting vertices.
- If normals are absent, compute them.
- If both are authored, **`primvars:normals` takes precedence** over `normals` [S: UsdGeomPointBased docs]. Readers must check both; three.js accepts either.
- "'normals' should not be authored on any Mesh that is subdivided" [S], which is another reason to set `subdivisionScheme = "none"`.

**st.** `faceVarying` + `indices` is the most common DCC output; `vertex` (the three.js exporter and Apple's glTF importer) is also fine. Always flip: `st = (u, 1 - v)`.

**displayColor / displayOpacity.**
- glTF COLOR_0 (RGBA) maps to `primvars:displayColor` (color3f, vertex) + `primvars:displayOpacity` (float, vertex).
- Apple's glTF→USD importer does exactly this [L].
- glTF COLOR_0 is linear; USD colour values are linear (the three.js exporter writes linear `diffuseColor`).
- In UsdPreviewSurface, displayColor is **not** automatically multiplied into the material. In Storm a material-less gprim renders with displayColor.
- To get glTF "baseColor × COLOR_0" semantics, add `UsdPrimvarReader_float3` (varname `displayColor`) as a multiplier. [UNVERIFIED whether RealityKit honours a primvar reader feeding diffuseColor.]
- three.js only uses the first displayColor value as a material colour [S: USDComposer.js].

**doubleSided.** `uniform bool doubleSided` lives on the gprim, not the material; glTF `material.doubleSided` must be pushed onto every mesh using that material. RealityKit ignores it [S]; three.js ignores it [S].

**subdivisionScheme.** Always author `uniform token subdivisionScheme = "none"` for glTF meshes. The default `catmullClark` makes Storm and RealityKit subdivide and smooth your triangles [S: Apple doc].

**extent.** `float3[] extent` is the local AABB. It is optional, and usdchecker did not complain when it was missing [L]. Authoring it avoids bound computation and is expected by DCCs [S: UsdGeomBoundable docs]. Animated or skinned meshes should still author the rest-pose extent.

**GeomSubset rules (glTF primitive → USD).** Either use one Mesh per glTF primitive (Apple's importer does this), or merge primitives into one Mesh with `GeomSubset`s. If merging:
- `elementType="face"`, `familyName="materialBind"`, `int[] indices` of face indices.
- Each subset gets `MaterialBindingAPI` + `rel material:binding`.
- The parent needs `subsetFamily:materialBind:familyType` = `"nonOverlapping"` or `"partition"` [L].
- **three.js drops faces not covered by any subset** [L], so make the subsets a full partition.

### A5. UsdPreviewSurface network (spec v2.6) and glTF mapping

Primary source: `docs/spec_usdpreviewsurface.rst` [S] (https://openusd.org/release/spec_usdpreviewsurface.html).

**UsdPreviewSurface inputs** (`info:id = "UsdPreviewSurface"`), as name, type, fallback:

| Input | Type | Fallback | glTF source |
|---|---|---|---|
| diffuseColor | color3f | (0.18,0.18,0.18) | baseColorFactor.rgb, or texture rgb |
| emissiveColor | color3f | (0,0,0) | emissiveFactor × emissiveTexture (× KHR_materials_emissive_strength) |
| useSpecularWorkflow | int | 0 | always 0 (Apple: specular workflow unsupported in RealityKit) |
| specularColor | color3f | (0,0,0) | (specular workflow only) |
| metallic | float | 0.0 | metallicFactor, or MR texture **.b** |
| roughness | float | 0.5 | roughnessFactor, or MR texture **.g** |
| clearcoat / clearcoatRoughness | float | 0.0 / 0.01 | KHR_materials_clearcoat |
| opacity | float | 1.0 | baseColor alpha (factor or texture .a) |
| opacityMode | token | transparent | new in 2.6; avoid for older consumers [UNVERIFIED support] |
| opacityThreshold | float | 0.0 | alphaMode MASK → alphaCutoff; BLEND → 0; OPAQUE → don't connect opacity |
| ior | float | 1.5 | KHR_materials_ior |
| normal | normal3f | (0,0,1) | normalTexture via UsdUVTexture (scale/bias/raw) |
| displacement | float | 0.0 | none |
| occlusion | float | 1.0 | occlusionTexture **.r** |
| outputs:surface, outputs:displacement | token | | |

**UsdUVTexture** (`info:id = "UsdUVTexture"`) inputs:
- `asset file`;
- `float2 st`;
- `token wrapS`, `wrapT` ∈ {black, clamp, repeat, mirror, useMetadata}. The default `useMetadata` falls back to **black**, so always author them. glTF 10497 → repeat, 33071 → clamp, 33648 → mirror;
- `float4 fallback` (0,0,0,1);
- `float4 scale` (1,1,1,1);
- `float4 bias` (0,0,0,0), where `output = texel*scale + bias`;
- `token sourceColorSpace` ∈ {raw, sRGB, auto}. `auto` marks 8-bit 3/4-channel images as sRGB.

Outputs:
- `float r, g, b, a` and `float3 rgb`.
- A 1-channel image replicates into r, g, b with a = 1. A 2-channel image puts channel 0 in r, g, b and channel 1 in a.
- Declare every output you connect to (`float3 outputs:rgb`, `float outputs:a`, …).

**UsdPrimvarReader_float2:** `string inputs:varname` (**string**; token is rejected [L]), `float2 inputs:fallback`, `float2 outputs:result`.

**UsdTransform2d:** `float2 inputs:in`, `float inputs:rotation` (degrees, CCW), `float2 inputs:scale`, `float2 inputs:translation`. The spec formula is `result = in * scale * rotate * translation`.

**glTF texture wiring** (the verified network is in `usd/quad.usda`, Material `Textured`; `--arkit` passes):

```usda
def Material "Textured"
{
    token outputs:surface.connect = </Root/Materials/Textured/PBR.outputs:surface>

    def Shader "PBR"
    {
        uniform token info:id = "UsdPreviewSurface"
        color3f inputs:diffuseColor.connect = </Root/Materials/Textured/BaseColorTex.outputs:rgb>
        float inputs:opacity.connect = </Root/Materials/Textured/BaseColorTex.outputs:a>
        float inputs:metallic.connect = </Root/Materials/Textured/MetalRoughTex.outputs:b>
        float inputs:roughness.connect = </Root/Materials/Textured/MetalRoughTex.outputs:g>
        normal3f inputs:normal.connect = </Root/Materials/Textured/NormalTex.outputs:rgb>
        color3f inputs:emissiveColor = (0, 0, 0)
        float inputs:occlusion = 1
        float inputs:ior = 1.5
        float inputs:opacityThreshold = 0
        int inputs:useSpecularWorkflow = 0
        token outputs:surface
    }
    def Shader "StReader"
    {
        uniform token info:id = "UsdPrimvarReader_float2"
        string inputs:varname = "st"
        float2 inputs:fallback = (0, 0)
        float2 outputs:result
    }
    def Shader "BaseColorTex"
    {
        uniform token info:id = "UsdUVTexture"
        asset inputs:file = @textures/basecolor.png@
        float2 inputs:st.connect = </Root/Materials/Textured/StReader.outputs:result>
        token inputs:sourceColorSpace = "sRGB"
        token inputs:wrapS = "repeat"
        token inputs:wrapT = "repeat"
        float4 inputs:scale = (1, 1, 1, 1)     # = baseColorFactor (rgba) — texture * factor
        float4 inputs:bias = (0, 0, 0, 0)
        float4 inputs:fallback = (1, 0, 1, 1)
        float3 outputs:rgb
        float outputs:a
    }
    def Shader "MetalRoughTex"                 # glTF MR texture: G = roughness, B = metallic
    {
        uniform token info:id = "UsdUVTexture"
        asset inputs:file = @textures/mr.png@
        float2 inputs:st.connect = </Root/Materials/Textured/StReader.outputs:result>
        token inputs:sourceColorSpace = "raw"
        # float4 inputs:scale = (1, roughnessFactor, metallicFactor, 1)   # factors fold into scale
        float outputs:g
        float outputs:b
    }
    def Shader "NormalTex"
    {
        uniform token info:id = "UsdUVTexture"
        asset inputs:file = @textures/normal.png@
        float2 inputs:st.connect = </Root/Materials/Textured/StReader.outputs:result>
        token inputs:sourceColorSpace = "raw"
        float4 inputs:scale = (2, 2, 2, 1)     # glTF normalTexture.scale s -> (2s, 2s, 2, 1), bias (-s, -s, -1, 0)
        float4 inputs:bias = (-1, -1, -1, 0)
        float3 outputs:rgb
    }
}
```

**Mapping notes.**
- Factors fold into `UsdUVTexture.scale`, because output = texel × scale + bias.
  - base colour: `scale = baseColorFactor`;
  - MR: `scale = (1, roughnessFactor, metallicFactor, 1)`;
  - occlusion: `scale` and `bias` implement `1 + strength*(occ-1)` (scale = strength, bias = 1 − strength).
- Apple's own glTF importer puts the texture paths on the Material interface (`asset inputs:baseColorTexture`) inside a `NodeGraph` [L], but plain shaders under the Material are simpler and validate.
- RealityKit caveats [S]:
  - "Scale" is partial ("except for normal map textures") and "Bias" is ✗. A normal map is decoded RealityKit's own way; you still must author scale/bias for the validator.
  - Factors folded via `scale` work in RealityKit, but **bias** (used above for occlusion strength) does not [S]. Prefer baking occlusion strength into the image, or drop it.
- glTF `normalTexture.scale` ≠ 1 (scale `(2s,2s,2,1)`) makes usdchecker warn [L]: `NormalMapTextureValidator.NonCompliantScaleValues … inputs:scale must be set to (2, 2, 2, 1)`. That is exit 0, but it fails under `--strict`, and RealityKit ignores normal-map scale anyway. Always write `(2,2,2,1)/(-1,-1,-1,0)` and bake s≠1 into the image if it matters.
- three.js reads `UsdUVTexture.scale` as the material colour/normalScale. It always samples **G** for roughness and **B** for metalness from the connected texture [S: USDComposer.js], so a glTF-packed MR texture works.
- Alpha: MASK → `opacity.connect = BaseColorTex.outputs:a` + `opacityThreshold = alphaCutoff`. BLEND → the same with threshold 0. Do not connect opacity for OPAQUE textures that have alpha.
- `emissiveColor` texture → `sourceColorSpace = "sRGB"`, scale = emissiveFactor. All non-colour textures use `raw`.
- Colour constants (`diffuseColor`, `emissiveColor`) are **linear**, as in glTF factors. Do not sRGB-encode them.

**V flip** [S][L]. Spec: "This mapping is shared by MaterialX and MDL, though unfortunately not glTf, so when converting between glTf and USD, texture coordinates must be "t flipped" (i.e. tFinal = 1.0 - t)". Two equivalent strategies:
- (a) flip data, `st = (u, 1-v)`. Used by the three.js exporter and this report; render verified upright.
- (b) keep data and insert `UsdTransform2d` scale (1,−1), translation (0,1). Apple's glTF importer does this per texture [L].

Prefer (a): RealityKit honours only one UsdTransform2d per material [S].

**KHR_texture_transform → UsdTransform2d** (on flipped st). With glTF `offset o`, `rotation θ` (rad) and `scale s`:

```
rotation_deg = -θ * 180/π
scale        = (s.x, s.y)
translation  = (o.x - s.y*sin θ,  1 - o.y - s.y*cos θ)
```

This is derived as `F∘(T·R·S)∘F` with `F(u,v) = (u,1-v)`, using the KHR GLSL definition and the spec formula. It was checked numerically over 10k random cases, max error 3.6e-15 [L-math]. For θ=0 it reduces to the three.js exporter's `offset.y = 1 - offset.y - repeat.y`. [UNVERIFIED by render for θ≠0.]

### A6. UsdSkel

Primary sources [S]:
- `UsdSkelSkeleton`: `uniform matrix4d[] bindTransforms` — "Specifies the bind-pose transforms of each joint in **world space**, in the ordering imposed by joints."
- `uniform matrix4d[] restTransforms` — "…rest-pose transforms of each joint in **local space**…".
- `uniform token[] joints` — "path tokens… The parent-child relationships of the corresponding paths determine the parent-child relationships of each joint".
- Optional `uniform token[] jointNames` (unique display names).
- Schemas doc: "all parent joints come before any of their children in the array". SkelAnimation "does not have this ancestral order restriction".
- `primvars:skel:geomBindTransform`: "the world space transform of a skinned primitive at bind time".
- Joint influences: `elementSize` = influences per point. Pad unused slots with 0 index and 0 weight. "Aside from vertex interpolation, the only other valid primvar interpolation for joint influences is constant".
- "Skinned geometry primitives are rendered in skeleton space": the gprim's own xform has no effect.
- SkelAnimation joint transforms are "joint local" and composed "scale-rotate-translate".

**Verified rig** (`usd/skel.usda`, `--arkit` passes; usdrecord frames 0 and 24 in `renders/skel_000.png` and `skel_2e+01.png`):

```usda
#usda 1.0
(
    defaultPrim = "Root"
    metersPerUnit = 1
    upAxis = "Y"
    startTimeCode = 0
    endTimeCode = 48
    timeCodesPerSecond = 24
    framesPerSecond = 24
)

def Xform "Root"
{
    double3 xformOp:translate.timeSamples = {
        0: (0, 0, 0),
        48: (1, 0, 0),
    }
    uniform token[] xformOpOrder = ["xformOp:translate"]

    def SkelRoot "Rig"
    {
        def Skeleton "Skel" (
            prepend apiSchemas = ["SkelBindingAPI"]
        )
        {
            uniform token[] joints = ["Bone0", "Bone0/Bone1"]
            uniform matrix4d[] bindTransforms = [( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1) ), ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 1, 0, 1) )]
            uniform matrix4d[] restTransforms = [( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1) ), ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 1, 0, 1) )]
            rel skel:animationSource = </Root/Rig/Skel/Anim>

            def SkelAnimation "Anim"
            {
                uniform token[] joints = ["Bone0", "Bone0/Bone1"]
                float3[] translations = [(0, 0, 0), (0, 1, 0)]
                quatf[] rotations.timeSamples = {
                    0: [(1, 0, 0, 0), (1, 0, 0, 0)],
                    24: [(1, 0, 0, 0), (0.9238795, 0, 0, 0.38268343)],
                    48: [(1, 0, 0, 0), (1, 0, 0, 0)],
                }
                half3[] scales = [(1, 1, 1), (1, 1, 1)]
                uniform token[] blendShapes = ["Bulge"]
                float[] blendShapeWeights.timeSamples = {
                    0: [0],
                    24: [1],
                    48: [0],
                }
            }
        }

        def Mesh "Strip" (
            prepend apiSchemas = ["SkelBindingAPI", "MaterialBindingAPI"]
        )
        {
            float3[] extent = [(-0.5, 0, 0), (0.5, 2, 0)]
            int[] faceVertexCounts = [3, 3, 3, 3]
            int[] faceVertexIndices = [0, 1, 3, 0, 3, 2, 2, 3, 5, 2, 5, 4]
            point3f[] points = [(-0.5, 0, 0), (0.5, 0, 0), (-0.5, 1, 0), (0.5, 1, 0), (-0.5, 2, 0), (0.5, 2, 0)]
            normal3f[] normals = [(0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1)] (
                interpolation = "vertex"
            )
            texCoord2f[] primvars:st = [(0, 0), (1, 0), (0, 0.5), (1, 0.5), (0, 1), (1, 1)] (
                interpolation = "vertex"
            )
            int[] primvars:skel:jointIndices = [0, 0, 0, 0, 0, 1, 0, 1, 1, 0, 1, 0] (
                elementSize = 2
                interpolation = "vertex"
            )
            float[] primvars:skel:jointWeights = [1, 0, 1, 0, 0.5, 0.5, 0.5, 0.5, 1, 0, 1, 0] (
                elementSize = 2
                interpolation = "vertex"
            )
            matrix4d primvars:skel:geomBindTransform = ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1) )
            uniform token[] skel:blendShapes = ["Bulge"]
            rel skel:blendShapeTargets = [</Root/Rig/Strip/Bulge>]
            rel skel:skeleton = </Root/Rig/Skel>
            uniform token subdivisionScheme = "none"
            rel material:binding = </Root/Materials/Textured>

            def BlendShape "Bulge"
            {
                uniform vector3f[] offsets = [(0.5, 0, 0), (0.5, 0, 0)]
                uniform int[] pointIndices = [4, 5]
            }
        }
    }
    # (Materials scope as in A5; Camera prim omitted here)
}
```

**glTF skin → UsdSkel, exact recipe.**

1. **Skeleton placement.** Put `SkelRoot` → `Skeleton` and the skinned `Mesh` prims under the same parent. That parent should correspond to the glTF node that is the common ancestor of the joints, or to the scene root with identity. Skinned points render in skeleton space, so the mesh's own xform is ignored. That matches glTF, which says the skinned mesh node's transform is ignored.
2. **joints.** Path tokens built from the joint hierarchy, e.g. `"Hips"`, `"Hips/Spine"`.
   - **Topologically sort parents-first.** glTF `skin.joints` order is arbitrary.
   - Remap `JOINTS_0` through the permutation.
   - Sanitize names to valid identifiers; paths must be unique, names needn't be.
   - Non-joint intermediate glTF nodes between joints must be folded into the child's rest/bind transforms. A path element must correspond to a joint.
3. **bindTransforms[j] = inverse(IBM[j])**, expressed in skeleton space. Write glTF's column-major 16 floats sequentially as USD rows. A glTF `T(0,1,0)` becomes `((1,0,0,0),(0,1,0,0),(0,0,1,0),(0,1,0,1))`.
   - [L] Using the IBM directly (`(0,-1,0,1)`) stretched the rest pose (`renders/skelwrong_000.png`); the inverse rendered undeformed.
   - If the skeleton prim is not at the glTF skin's coordinate frame, pre-multiply by the frame change.
4. **restTransforms[j]** = the joint's local matrix (from glTF node TRS at rest) relative to its parent joint.
5. **Influences.**
   - `primvars:skel:jointIndices` (`int[]`, elementSize = 4·k for JOINTS_0…k) and `primvars:skel:jointWeights` (`float[]`).
   - `interpolation = "vertex"`; renormalise weights to sum 1.
   - Unused slots: index 0, weight 0.
   - `primvars:skel:geomBindTransform` = identity when mesh points are already in skeleton space.
6. **Bindings.** `rel skel:skeleton` on the mesh, or inherited from SkelRoot. `rel skel:animationSource` on the Skeleton. `prepend apiSchemas = ["SkelBindingAPI"]` on every prim carrying `skel:*` properties: Mesh, and Skeleton because of `skel:animationSource`.
7. **SkelAnimation.**
   - `joints` may be a subset or any order. Use the same list for simplicity.
   - `float3[] translations`, `quatf[] rotations` (text `(w,x,y,z)`), `half3[] scales` (1.2 becomes 1.2002 in half), each time-sampled with one array per sample containing **all** joints. Glue per-joint glTF channels by resampling to the union of key times.
   - **Types are mandatory:** a `float3[] scales` rendered unscaled (2478 px coverage vs 9031 px for `half3[]`) while usdchecker passed [L].
8. **Morph targets → BlendShape.**
   - One `BlendShape` prim per target under the Mesh, with `uniform vector3f[] offsets` and optionally `uniform int[] pointIndices` (sparse) and `uniform vector3f[] normalOffsets`.
   - On the mesh: `uniform token[] skel:blendShapes` (names) + `rel skel:blendShapeTargets` (same order).
   - On SkelAnimation: `uniform token[] blendShapes` + time-sampled `float[] blendShapeWeights`.
   - **Blend shapes require the UsdSkel binding**: a SkelRoot, and a Skeleton + animationSource to drive weights, even for unskinned meshes. Use a 1-joint dummy skeleton. [UNVERIFIED that a joint-less Skeleton works everywhere.]
   - [L] Weights animated as expected in the render: the top verts shifted +0.5 X at frame 24.
   - glTF default `mesh.weights` → a SkelAnimation default value (`float[] blendShapeWeights = [...]`).
9. Apple's glTF importer (Adobe USD-Fileformat-plugins 1.0.8) is **not** a reference [L]. It:
   - wrote `(0,0,0,0)` quaternions for unanimated joints;
   - dropped the morph target entirely ("Unsupported import of GLTF blend weight animation");
   - duplicated joints as Xforms;
   - used joint paths of node indices (`n0/n2`) plus `jointNames`;
   - lost STEP.

**Quick Look / RealityKit:** skeletons, skeletal animation and blend shapes are ✔ [S, Apple matrix]. Apple forum reports say blend-shape weights may not display while `playAnimation` is running in RealityKit [UNVERIFIED, forum only]. Use a single skeleton [S].

**three.js USDLoader:**
- Skeleton: bones from `joints`; `boneInverses = inverse(bindTransforms)`; rest from `restTransforms`; `geomBindTransform` honoured.
- SkelAnimation: translations/rotations/scales are supported.
- BlendShapes are **not** supported [S].
- USDA `quatf[]` is mis-ordered (bug, [L]).

### A7. Time and animation

**Layer metadata.** `timeCodesPerSecond` (TCPS) defines the time-code→seconds mapping; `framesPerSecond` is a playback hint; `startTimeCode`/`endTimeCode` give the range.
- Writer: choose TCPS (e.g. 24 or 60) and write `t_code = t_seconds × TCPS`. Set FPS = TCPS: three.js uses `framesPerSecond || timeCodesPerSecond || 30` and ignores `startTimeCode` [S].
- Apple's importer used TCPS = 1 with seconds as codes [L]. That is legal, but low-precision tooling that rounds codes could suffer.

**Fractional time codes** are legal (`2.5:` parsed and round-tripped [L]); crate stores times as doubles.

**Interpolation** [S] (`pxr/usd/usd/interpolators.cpp`):
- Linear by default.
- `GfQuath/f/d` use `GfSlerp`.
- Arrays interpolate element-wise; on size mismatch they "Fall back to held".
- A value block or the `UsdInterpolationTypeHeld` stage setting gives held.

The stage interpolation type is **runtime API only**. A layer `interpolationType = "held"` changed nothing in a usdrecord render [L].

glTF mapping:
- LINEAR → samples as-is. Ensure consecutive quaternions have dot ≥ 0, because consumers may not take the shortest path.
- STEP → for each key i>0 add a sample at `t_i − ε`, using a tiny ε in time codes, holding value i−1.
- CUBICSPLINE → bake to dense linear samples. (OpenUSD 24.11+ has splines, crate ≥0.12, but no Apple/three.js support [UNVERIFIED].)

**Transform animation:** `xformOp:translate` (double3/float3), `xformOp:orient` (quatf), `xformOp:scale` (float3/half3), with `uniform token[] xformOpOrder = ["xformOp:translate","xformOp:orient","xformOp:scale"]`, i.e. T·R·S like glTF.
- A default value plus `.timeSamples` may coexist. Samples win when present.
- Prefer **float3 translate** for three.js USDC compatibility (see A9).
- Pivot ops use `!invert!xformOp:translate:pivot`.

**Morph weight animation** goes only through SkelAnimation `blendShapeWeights` (A6).

### A8. USDC ("crate") binary format, for reading

Primary sources [S] (`pxr/usd/sdf/crateFile.cpp`, `crateFile.h`, `crateDataTypes.h`, `crateValueInliners.h`, `integerCoding.cpp`, `pxr/base/tf/fastCompression.cpp`). Every point was cross-checked with an independent decoder (`scripts/crate.py`) against `usdcat`-written files [L].

**Versions** [S]:

| Version | Change |
|---|---|
| 0.15.0 | loopBoundaryTime splines; GfTimeCode splines |
| 0.14.0 | ArrayEdits |
| 0.13.0 | spline tangent algorithms |
| 0.12.0 | splines |
| 0.11.0 | relocates |
| 0.10.0 | pathExpression |
| 0.9.0 | timecode, timecode[] |
| 0.8.0 | SdfPayloadListOp, payload layer offsets |
| 0.7.0 | **array sizes uint64** |
| 0.6.0 | **compressed float arrays** |
| 0.5.0 | **compressed (u)int(64) arrays; drop the uint32 rank prefix** |
| 0.4.0 | **compressed structural sections** |
| 0.3.0 | broken, unused |
| 0.2.0 | prepend/append list ops |
| 0.1.0 | path header layout fix |
| 0.0.1 | initial |

Software version is 0.15.0; **default write version is 0.8.0**, upgraded on demand. Measured: `usdcat` → `00 08 00` (0.8.0); `usdcrush -c widest` and `-c latest`, and `usdzip --arkitAsset`, also wrote 0.8.0 [L]. A reader should support 0.4.0–0.10.0 fully; reject ≥0.11 unknown features gracefully.

**Bootstrap: 88 bytes at offset 0** [S][L]:

```
00: "PXR-USDC"                      ident[8]
08: 00 08 00 00 00 00 00 00         version[8] = major, minor, patch, 5 unused
10: tocOffset  int64 LE             (e.g. 0x25c = 604 for tiny.usdc)
18: reserved   int64[8]
58: value data starts here (arrays, out-of-line scalars); structural sections follow; TOC is last
```

**TOC** at `tocOffset`: `uint64 nSections`, then `nSections × { char name[16] (NUL-padded, ≤15 chars), int64 start, int64 size }`. The six known sections, in write order, are `TOKENS STRINGS FIELDS FIELDSETS PATHS SPECS`. Unknown sections must be skipped.

**Compression primitives.**
- **TfFastCompression** blob:
  - `byte0 == 0` → the rest is **one raw LZ4 block** (no LZ4 frame).
  - `byte0 = k > 0` → k chunks, each `[int32 compressedSize][LZ4 block]`, each decompressing to ≤ `LZ4_MAX_INPUT_SIZE` (0x7E000000) bytes.
  - three.js reads all chunk sizes up front and assumes 64 KiB chunks. That is wrong, but irrelevant below ~2 GB.
- **Compressed ints** = `uint64 compressedSize` + TfFastCompression(blob). The blob decodes to `commonValue:int32`, then `ceil(2n/8)` code bytes (2 bits per int, **LSB first**, 4 per byte), then variable ints.
  - Each code: 0 = add commonValue; 1 = int8; 2 = int16; 3 = int32.
  - The value is a running sum (delta decoding), starting at 0.
  - For 64-bit ints (`Sdf_IntegerCompression64`), commonValue is int64 and codes 1/2/3 are int16/int32/int64.
  - Working buffer size = `sizeof(Int) + ceil(2n/8) + n*sizeof(Int)`.

**Structural sections (version ≥ 0.4.0)** [S][L]:

| Section | Layout |
|---|---|
| TOKENS | `uint64 numTokens, uint64 uncompressedSize, uint64 compressedSize, blob` → NUL-separated UTF-8 strings. `usdcat` writes token 0 = `";-)"`, a sentinel; never assume index 0 is "" |
| STRINGS | `uint64 n, uint32 tokenIndex[n]`: a string is a token index (**not compressed**; indices into this table skip the 8-byte count) |
| FIELDS | `uint64 n; compressedInts(tokenIndex[n]); uint64 repsSize + TfFastCompression(n × uint64 ValueRep)` |
| FIELDSETS | `uint64 n; compressedInts(fieldIndex[n])`, as runs terminated by `0xFFFFFFFF` |
| PATHS | `uint64 numPaths` (table size); `uint64 numEncoded; compressedInts(pathIndex); compressedInts(int32 elementTokenIndex); compressedInts(int32 jump)` |
| SPECS | `uint64 n; compressedInts(pathIndex); compressedInts(fieldSetIndex); compressedInts(specType)` |

Pre-0.4.0 files store these as plain vectors: uint64 count + raw structs, with path items as a recursive `{uint32 index, uint32 token, uint8 bits}` + sibling offsets. three.js implements that path; see its `_readPathsRecursive`.

`SdfSpecType`: 0 Unknown, 1 Attribute, 2 Connection, 3 Expression, 4 Mapper, 5 MapperArg, 6 Prim, 7 PseudoRoot, 8 Relationship, 9 RelationshipTarget, 10 Variant, 11 VariantSet.

**Path tree decoding** (`_BuildDecompressedPathsImpl`). Iterate from entry 0 with `parent = empty`:
- The first entry is `/`.
- Element token `e`: if `e < 0`, append property `tokens[-e]` (`parent.prop`); else append child `tokens[e]`. This may be a variant element like `{look=red}`.
- `jump`: `-2` = leaf; `-1` = has child only (next entry is its child); `0` = has sibling only (next entry is a sibling); `>0` = child **and** sibling, where the sibling subtree starts at `thisIndex + jump` and the child is the next entry.
- Example from `tiny.usdc` [L]:

```
(pathIdx, elemTok, jump): (0,4'' ,-1) (1,2'A',-1) (2,8'B',+2) (4,-12'faceVertexCounts',-2) (3,9'C',-2)
→ paths[0]='/', [1]='/A', [2]='/A/B', [4]='/A/B.faceVertexCounts', [3]='/A/C'
```

**Specs and fields.** For each spec: path = `paths[pathIndex]`, type = `specType`. Its fields are `fieldsets[fieldSetIndex …]` up to `0xFFFFFFFF`. Each field gives `(name = tokens[tokenIndex], value = unpack(ValueRep))`.
- Typical prim fields: `specifier`, `typeName`, `primChildren` (TokenVector), `properties` (TokenVector), `apiSchemas` (TokenListOp), `kind`, `variantSelection`, `variantSetNames`, `variantSetChildren`, `references` (ReferenceListOp), `payload`, `customData` (Dictionary).
- Attribute fields: `typeName` (token like `"point3f[]"`), `variability` (0 varying, 1 uniform), `custom` (bool), `default`, `timeSamples`, `interpolation`, `elementSize`, `connectionPaths` (PathListOp).
- Relationship fields: `targetPaths` (PathListOp).
- Pseudo-root fields: layer metadata (`defaultPrim`, `upAxis`, `metersPerUnit`, …).

**ValueRep (uint64)** [S]:

```
bit 63 IsArray | bit 62 IsInlined | bit 61 IsCompressed | bit 60 IsArrayEdit (0.14+)
bits 48..55 TypeEnum | bits 0..47 payload (file offset, or inline data in low 32 bits)
```

**TypeEnum** [S] (`crateDataTypes.h`), identical to three.js:

| Code | Type | Code | Type | Code | Type | Code | Type |
|---|---|---|---|---|---|---|---|
| 1 | Bool | 2 | UChar | 3 | Int | 4 | UInt |
| 5 | Int64 | 6 | UInt64 | 7 | Half | 8 | Float |
| 9 | Double | 10 | String | 11 | Token | 12 | AssetPath |
| 13 | Matrix2d | 14 | Matrix3d | 15 | Matrix4d | 16 | Quatd |
| 17 | Quatf | 18 | Quath | 19 | Vec2d | 20 | Vec2f |
| 21 | Vec2h | 22 | Vec2i | 23 | Vec3d | 24 | Vec3f |
| 25 | Vec3h | 26 | Vec3i | 27 | Vec4d | 28 | Vec4f |
| 29 | Vec4h | 30 | Vec4i | 31 | Dictionary | 32 | TokenListOp |
| 33 | StringListOp | 34 | PathListOp | 35 | ReferenceListOp | 36 | IntListOp |
| 37 | Int64ListOp | 38 | UIntListOp | 39 | UInt64ListOp | 40 | PathVector |
| 41 | TokenVector | 42 | Specifier (0 def, 1 over, 2 class) | 43 | Permission | 44 | Variability |
| 45 | VariantSelectionMap | 46 | TimeSamples | 47 | Payload | 48 | DoubleVector |
| 49 | LayerOffsetVector | 50 | StringVector | 51 | ValueBlock | 52 | Value |
| 53 | UnregisteredValue | 54 | UnregisteredValueListOp | 55 | PayloadListOp | 56 | TimeCode |
| 57 | PathExpression | 58 | Relocates | 59 | Spline | 60 | AnimationBlock |

**Inlining** [S]:
- **Always inlined**: bitwise types ≤ 4 bytes (bool, uchar, int, uint, half, float, **Vec2h as raw 2×half**, Specifier, Permission, Variability), stored as their bytes in the payload. Also string (StringIndex), token (TokenIndex), path (PathIndex) and **scalar assetPath (TokenIndex)**.
- **Conditionally inlined:**
  - `double` stored as float bits when exactly representable as float. three.js handles this.
  - int64/uint64 when they fit in 32 bits.
  - **GfVec types > 4 bytes** when every component is exactly an int8, stored as int8 components (e.g. `double3 (0,1,0)`).
  - **matrices** when off-diagonal entries are 0 and diagonal entries are int8, stored as int8 diagonal, e.g. identity `matrix4d`.
  - **empty arrays**: array bit set + payload 0.
- Quats are never inlined.
- Out-of-line non-array scalars are written once and deduplicated, so many reps may share an offset.

**Arrays** (payload → data):
- `uint32 rank(=1)` only for < 0.5.0.
- Then the count: `uint32` if < 0.7.0, else `uint64`.
- Then the elements, little-endian, contiguous. Uncompressed arrays are 8-byte aligned.
- `Int/UInt/Int64/UInt64` arrays with **count ≥ 16** are compressed (flag bit 61): `compressedInts` (64-bit coding for Int64/UInt64).
- `Half/Float/Double` arrays with count ≥ 16 and version ≥ 0.6 may be compressed:
  - `int8 code`. `'i'`: compressedInts32 of exact integer values (also for half/double).
  - `'t'`: `uint32 lutSize; T lut[lutSize]` (T = element type: 2/4/8 bytes), then `compressedInts32(uint32 indexes[count])`.
  - Otherwise uncompressed, and **not** flagged.
- Verified on `crate/stress.usdc` [L]: 40-int array compressed; float 'i'; float 't' (LUT 2); double 't' (LUT 2); half 't'; 32 non-integral floats uncompressed.
- Vec/Quat/Matrix arrays are never compressed.
- Element memory layouts:
  - **GfQuat = (imaginary x, y, z, real w)**: text `(0.9238795,0,0,0.38268343)` was stored as `(0,0,0.3827,0.9239)` [L].
  - **GfMatrix4d = 16 doubles row-major**, rows as written in USDA.
  - Token arrays are uint32 token indices.
  - **String and AssetPath array elements are STRINGS indices** (`tokens[strings[i]]`), unlike scalar inlined assetPath, which is a token index [S][L].

**Composite values.**
- **TimeSamples** (payload = offset P) [S][L]. Times are a `DoubleVector`, deduplicated across attributes (identical payload offset in `skel.usdc`).

```
at P:      int64 off1                ; seek P+off1
           ValueRep timesRep         ; DoubleVector (uint64 n + n doubles), dedup-shared
           int64 off2 (at its own position Q)  ; seek Q+off2
           uint64 numValues
           ValueRep valueRep[numValues] ; each unpacked normally (inlined or offset)
```

- **VtValue** nested in dictionaries and similar containers: `[int64 forward offset][nested payload bytes][ValueRep]`. The reader seeks `pos+offset`, reads the rep and **continues after it** (`_RecursiveWrite`/`_RecursiveRead`). three.js's dictionary reader is broken: it uses a nonexistent `reader.position` and `new ValueRep(oneArg)`.
- **Dictionary**: `uint64 n; n × { uint32 keyStringIndex; VtValue }`.
- **VariantSelectionMap**: `uint64 n; n × {uint32 strIdx, uint32 strIdx}`.
- **TokenVector / PathVector / DoubleVector / StringVector**: `uint64 n` + n × (uint32 index | double).
- **ListOp**: `uint8 bits` (`1 IsExplicit, 2 HasExplicit, 4 HasAdded, 8 HasDeleted, 16 HasOrdered, 32 HasPrepended, 64 HasAppended`). Then the item vectors, each `uint64 n + items`, in this order: **explicit, added, prepended, appended, deleted, ordered**.
  - `usdcat` writes connections and relationship targets as `{isExplicit, explicit:[…]}`, and `prepend apiSchemas` as `{prepend:[…]}` [L].
- **Reference**: `string assetPath(StringIndex); path primPath; double layerOffset.offset, .scale; Dictionary customData`.
- **Payload**: `string; path; LayerOffset` (≥0.8).
- **Variant paths** appear as path elements `{set=}` (VariantSet spec) and `{set=sel}` (Variant spec), e.g. `/Root/{look=red}.c` [L].

**three.js `USDCParser` coverage and bugs** [S][L]. three.js handles:
- ≥0.4 compressed sections, and <0.4 uncompressed;
- the 0.7.0 array size switch;
- int/float compression ('i','t' for **Float only**; Half/Double LUT and Int64 arrays are unsupported);
- TimeSamples, PathListOp, TokenVector, PathVector, DoubleVector, VariantSelectionMap.

Its defects:
1. The STRINGS table is misindexed (+8 bytes), so every `string`-typed value is wrong [L].
2. Inlined Vec2d/3d/4d (and Vec3h/4h) values are returned as a raw number, which crashes `applyTransform` [L].
3. Dictionaries are broken.
4. TokenListOp/ReferenceListOp are ignored.
5. The <0.5.0 rank is not skipped.
6. Multi-chunk LZ4 is wrong (>2 GB only).
7. The ArrayEdit bit is ignored.

### A9. three.js r183 `USDLoader` as a consumer (source-read + Node runs)

Root and packaging:
- **Root layer**: the first zip entry must be `.usda`, `.usdc` or `.usd`; usda and usdc roots both load [L].
- Textures resolve relative to the root's directory, else by basename search.
- **Upaxis**: Z-up → `group.rotation.x = -π/2`. `metersPerUnit` is ignored.

Transforms:
- `xformOp:transform` (rows read as USD row-vector), translate, translate:pivot, scale, rotateX/Y/Z/XYZ and orient are handled in `xformOpOrder` order.
- `!invert!` prefixes are supported.

Mesh:
- Triangulation: fan for quads, earcut for n>4.
- `normals` or `primvars:normals` (vertex/faceVarying/indexed).
- The first primvar whose `typeName` contains `texCoord` is uv0, else `primvars:st`/`UVMap`; `primvars:st1` → uv1.
- GeomSubsets → material groups; uncovered faces are dropped [L].

Materials (UsdPreviewSurface):
- diffuse (sRGB map; colour = scale), emissive, normal (normalScale = scale.xy), roughness (map → roughness = 1), metallic, occlusion (aoMap), ior, clearcoat, opacity (connected → `transparent`, or `alphaTest = opacityThreshold`), wrapS/T, UsdTransform2d (scale→repeat, translation→offset, rotation deg).
- Not read: `doubleSided`, `orientation`, `subdivisionScheme`.

Animation:
- SkelAnimation → per-joint position/quaternion/scale tracks.
- `xformOp:translate/orient/scale/rotateXYZ/transform` timeSamples on Xform/Scope/Mesh prims → a `TransformAnimation` clip, with tracks named by **prim name**. Duplicate names collide.
- No BlendShapes, no displayColor vertex colours, no lights/cameras.

**Bugs** [L]:
- USDA quat arrays are mis-ordered.
- USDC inlined double vectors crash.
- USDC strings are wrong.
- The USDA parser is line-based: multi-line timeSample values inside `{ }` are not joined, so write each time sample on one line, as usdcat does.

---

## B. 3MF

Sources [S]:
- 3MF Core Specification **v1.4.0** (https://github.com/3MFConsortium/spec_core, `3MF Core Specification.md`).
- Materials & Properties Extension **v1.2.1** (https://github.com/3MFConsortium/spec_materials).

### B1. OPC package

```
[Content_Types].xml
_rels/.rels                         package rels: StartPart (+ thumbnail, mustpreserve, core props)
3D/3dmodel.model                    the 3D Model part (StartPart target)
3D/_rels/3dmodel.model.rels         rels FROM the model part: 3D textures, object thumbnails, printticket
3D/Textures/<name>.png|jpg          spec-recommended folder "/3D/Textures/" (three.js also accepts "/3D/Texture/")
Metadata/thumbnail.png              package thumbnail (conventional location)
```

`[Content_Types].xml` (verified with three.js and assimp [L]):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
  <Default Extension="png" ContentType="image/png"/>
  <Default Extension="jpg" ContentType="image/jpeg"/>   <!-- add per extension you use (jpeg too) -->
</Types>
```

`_rels/.rels` and `3D/_rels/3dmodel.model.rels`:

```xml
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rel0" Target="/3D/3dmodel.model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
  <Relationship Id="rel1" Target="/Metadata/thumbnail.png" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail"/>
</Relationships>
<!-- 3D/_rels/3dmodel.model.rels -->
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="tex0" Target="/3D/Textures/basecolor.png" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dtexture"/>
</Relationships>
```

Spec rules [S]:

**Zip.**
- Entries MUST be Deflate (8) or Stored (0).
- "The consumers MUST support both the PKWARE ZIP64 extension and streaming extension… producers SHOULD produce plain ZIP files if the 3MF data fits".
- assimp's writer emits data descriptors (flag 0x0808) and a zip64 EOCD even for a 1 KB file [L]. That is legal per spec but unfriendly to old consumers.

**Relationships.**
- The StartPart relationship MUST point to the model part.
- "There MUST NOT be more than one relationship of a given relationship type from one part to a second part".
- Thumbnail, PrintTicket and StartPart targets MUST exist.
- Texture parts "MUST also be the target of a 3D Texture relationship from the 3D Model part".
- The texture content type in the Materials appendix is `application/vnd.ms-package.3dmanufacturing-3dmodeltexture`. In practice use `image/png`/`image/jpeg` Default entries. The `texture2d@contenttype` attribute must be `image/png` or `image/jpeg`.

**Part names.**
- Part names must be absolute (`/…`).
- The recommended model part is "/3D/3dModel.model"; OPC names are case-insensitive. three.js requires the root model to match `^3D/[^/]*\.model$`, and textures under `3D/Texture(s)/`.
- Producers must not reference resources outside the package.

**File naming.** Core 1.4 adds double-extension naming recommendations (`.model.3mf`, `.build.3mf`, …). This is optional.

### B2. Core model XML

Namespace `http://schemas.microsoft.com/3dmanufacturing/core/2015/02`. Rules [S]:

**Coordinate space.** Right-handed; "x-axis increasing to the right… y-axis… to the back… z-axis… to the top" of the build platform, so **Z-up**. glTF (Y-up) → 3MF: `(x, y, z) → (x, −z, y)`, i.e. rotate +90° about X. Do this either by transforming vertices or by using the build item transform `1 0 0 0 0 1 0 -1 0 0 0 0`.
- Check: that string as rows is `[1 0 0; 0 0 1; 0 -1 0]` with zero translation. Row-vector `p·M` maps (x,y,z) → (x, −z, y).

**unit** ∈ micron, millimeter (default), centimeter, inch, foot, meter. Cura scales by `micron 0.001, millimeter 1, centimeter 10, meter 1000, inch 25.4, foot 304.8` [S]. three.js reads the unit but does not scale [S]. glTF metres: either write `unit="meter"`, or scale ×1000 and write millimeter. Millimetre is the safest for slicers.

**Element order.** "Producers MUST define each element prior to referencing it". The `<model>` children are `metadata*`, `resources`, `build`. Inside resources, put material groups before the objects that use them, and component objects before their assemblies.

**`<object>`.**
- Attributes: `id` (unique among *all* resources), `type` ∈ model (default) | solidsupport | support | surface | other, `name`, `partnumber`, `thumbnail`, `pid`, `pindex`.
- It contains exactly one `<mesh>` **or** one `<components>`.
- "If this object contains any triangles with assigned properties, the object MUST specify pid and pindex".
- "Producers MUST NOT assign pid or pindex attributes to objects that contain components".
- Build items MUST NOT reference `type="other"`.

**Mesh** (type model/solidsupport) MUST have:
- manifold edges: each edge shared by exactly 2 triangles, by vertex index;
- consistent orientation;
- outward normals (normal = (B−A)×(C−A)): "v1, v2, v3 MUST be specified in counter-clockwise order";
- distinct indices;
- at least 4 triangles.
- Self-intersections are allowed (consumers apply the positive fill rule).
- "The producer SHOULD NOT include duplicate vertices unless coalescing duplicates would create non-manifold edges".
- `support`/`surface`/`other` may break the manifold rules.

**Transforms.** `"m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32"`, row-major affine with an implicit last column (0,0,0,1). Points are row vectors: `p' = [x y z 1]·M`. **Translation is m30 m31 m32.**
- From a glTF column-major `e[16]`: `e0 e1 e2 e4 e5 e6 e8 e9 e10 e12 e13 e14` [S]. Verified: three.js builds `Matrix4.set(t0,t3,t6,t9, t1,t4,t7,t10, t2,t5,t8,t11, 0,0,0,1)`; assimp produced glTF node matrices with translation in `[12..14]`; Cura fills column 0 from t0..t2 and column 3 from t9..t11 [L][S].
- "When applying a transformation with a negative determinant, the resultant shape MUST NOT change the sign of its volume": consumers re-orient mirrored meshes.

**Components.** `<components><component objectid="2" transform="…"/></components>`. They compose, and relative placement must be respected. Use them for glTF node hierarchies. Note there is no non-mesh grouping node: flatten transforms into components or vertices.

**Build.** `<build><item objectid transform? partnumber?/></build>`. "A consumer MUST NOT output any 3D objects not referenced by an <item>". Item transform = placement on the plate.

**Metadata.**
- `<metadata name="Title|Designer|Description|Copyright|LicenseTerms|Rating|CreationDate|ModificationDate|Application" preserve? type?>value</metadata>` as children of `<model>`.
- Any other name MUST be namespace-prefixed (`lg:SourceFormat` with `xmlns:lg=…` declared on `<model>`).
- No duplicates.

**requiredextensions.**
- A space-delimited list of **namespace prefixes**, not URIs. Consumers that don't support a listed extension MUST NOT process the file.
- "Producers SHOULD NOT require extensions unless this model-file would lose key meaning". **Do not list `m`** for appearance-only colours/textures.
- `recommendedextensions` exists in core 1.4.

### B3. Materials & Properties extension

Namespace `http://schemas.microsoft.com/3dmanufacturing/material/2015/02` (prefix `m` by convention).

**Core `<basematerials id>`.** Holds `<base name displaycolor="#RRGGBB[AA]"/>` entries with an implicit 0-based index.
- **sRGB**, "6 or 8 digit hexadecimal", alpha defaults to FF [S].
- Triangles using basematerials MUST NOT form gradients: p1 = p2 = p3, or only p1.
- "displaycolor… meant to be used for rendering purposes only" [S].

**`<m:colorgroup id [displaypropertiesid]>`.** Holds `<m:color color="#RRGGBB[AA]"/>` entries: "vertex colors", sRGB. "Colors are assumed to be fully opaque… except when used as a non-base layer inside a <multiproperties>" [S]. So COLOR_0 alpha is lost unless you use multiproperties.

**`<m:texture2d>`.** Attributes:
- `id`, `path` (part name);
- `contenttype` = `image/png` | `image/jpeg`;
- `tilestyleu`/`tilestylev` ∈ wrap (default), mirror, clamp, none;
- `filter` ∈ auto (default), linear, nearest.
- The `box` attribute is deprecated.
- Colour channels are sRGB; alpha is linear [S].

**`<m:texture2dgroup id texid [displaypropertiesid]>`.** Holds `<m:tex2coord u v/>` with an implicit index. "**The lower left corner of the texture is the u, v coordinate (0,0)**, and the upper right coordinate is (1,1)" [S]. glTF uv → `(u, 1−v)`.

**Triangle properties.**
- `<triangle v1 v2 v3 [pid] [p1] [p2] [p3]/>`. `pid` overrides the object pid. `p1..p3` index into the group per corner, so UVs and colours are **per triangle corner**.
- "If p2 or p3 is unspecified then p1 is used for the entire triangle"; a consumer that can't do gradients uses p1.
- Unspecified p1 → object `pindex`.
- `pid` on a triangle forces all three properties to come from one group.

**Other elements.** `<m:compositematerials>`, `<m:multiproperties pids blendmethods>` (layering, e.g. texture × colour; alpha only meaningful in non-base layers), and display properties (`<m:pbmetallicdisplayproperties>`: `<m:pbmetallic name metallicness roughness>`; `pbspeculardisplayproperties`, translucent). three.js reads pbmetallic [S].

**glTF → 3MF material mapping.**
- Base colour texture → texture2d + texture2dgroup.
- Constant colour → basematerials (sRGB-encode the linear glTF factor!).
- COLOR_0 → colorgroup, linear→sRGB, alpha dropped.
- metallic/roughness factors → `pbmetallicdisplayproperties` (optional; display only).
- Normal, occlusion and emissive maps have no 3MF equivalent; drop them.
- Combining base-colour texture with a factor or COLOR_0 needs multiproperties, which is poorly supported. Bake the factor into the texture instead.

### B4. What consumers honour

| Consumer | Geometry/transforms | basematerials | colorgroup | texture2d | Evidence |
|---|---|---|---|---|---|
| three.js r183 `ThreeMFLoader` | components + item transforms ✔; unit ignored; no Z-up conversion | ✔ (per-triangle split into meshes; alpha → opacity but `transparent` not set) | ✔ (vertex colours, interpolated) | ✔ (needs rel + `3D/Texture(s)/` path; flipY true = lower-left origin) | [L] headless Chrome on `ref.3mf` (render upright); [S] source |
| assimp 6.0 import | ✔ transforms → node matrices | ✔ (but 6-digit `#RRGGBB` → **alpha 0**) | per **vertex** (last write wins); **only on triangles that carry their own `pid`** | same caveats as colorgroup; UVs flipped to assimp's V-up | [L] round-trip to glTF; [S] `XmlSerializer.cpp` v6.0.0 |
| PrusaSlicer 2.9.4 | ✔ (repairs/reports mesh errors) | ✗ | ✗ | ✗ (reads only its own `slic3rpe:mmu_segmentation` and Bambu `paint_color`) | [S] `src/libslic3r/Format/3mf.cpp` @version_2.9.4: no basematerials/colorgroup/texture2d code. master (3.0 alpha) has basematerials parsing commented out |
| Bambu Studio (master) | ✔ | ✗ | ✔ `m:colorgroup` mapped to **filament/extruder** slots | ✗ | [S] `bbs_3mf.cpp`: `COLOR_GROUP_TAG "m:colorgroup"`, `color_group_index_to_extruder_id_map`; no basematerials/texture2d. Forum: "colors only applied to faces" in v2.5 [UNVERIFIED] |
| Cura (main) | ✔, applies unit scale | ✗ | ✗ | PNG texture + per-vertex UVs loaded as a "paint texture" (Cura's own painting feature, not colour printing) | [S] `plugins/3MFReader/ThreeMFReader.py` |
| Windows 3D Viewer / 3D Builder | Microsoft's own format; historically full colour/texture support [UNVERIFIED] | | | | 3D Viewer deprecated Feb 2026, removed from Store 1 Jul 2026 [S: news]; 3D Builder status [UNVERIFIED] |

**Non-manifold input.**
- The spec says model meshes MUST be manifold.
- Slicers generally repair: PrusaSlicer stores `RepairedMeshErrors` stats per volume [S] and reports repairs [UNVERIFIED wording].
- three.js and assimp don't check [L].
- Do not rely on any consumer rejecting or repairing. Weld vertices by position (A–B below) and, for open meshes, consider `type="surface"`. Many slicers mishandle surfaces, so the choice is [UNVERIFIED].

### B5. What `assimp export test.gltf out.3mf` writes [L]

```
zip: [Content_Types].xml, 3D/3DModel.model, _rels/.rels   (Deflate, data descriptors, zip64 EOCD)
<model unit="millimeter" xmlns="…core/2015/02">
<resources>
<metadata name="SourceAsset_Generator">make_gltf.py</metadata>     <!-- INVALID: metadata inside resources, un-namespaced -->
<basematerials id="1"><base name="TexturedMat" displaycolor="#ffffffff"/>…</basematerials>
<object id="2" type="model"><mesh>…strip…</mesh><mesh>…triangle…</mesh></object>  <!-- INVALID: 2 meshes -->
<object id="3" type="model"></object>                                         <!-- INVALID: empty (Bone0 node) -->
</resources>
<build><item objectid="2"/><item objectid="3"/></build>
```

Source (`code/AssetLib/3MF/D3MFExporter.cpp` v6.0.0) [S]:
- It iterates only `root->mChildren` (no recursion, no transforms) and writes every mesh of a node into one `<object>`.
- `unit="millimeter"` is hard-coded with no scaling (metres written as mm).
- UVs, vertex colours, textures and `pid` per triangle beyond basematerials are dropped. Y-up is not converted.
- three.js **throws** on it (`Cannot read properties of undefined (reading 'build')`, from the empty object) [L].
- **Do not use assimp as a 3MF reference.**

### B6. Verified reference 3MF model part

`threemf/ref.3mf` loads in three.js (render `renders/three_ref_3mf.png`) and assimp. For assimp per-corner reads, also put `pid` on each property-bearing triangle (`ref_pid.3mf`); this is legal and redundant.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US"
       xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
       xmlns:m="http://schemas.microsoft.com/3dmanufacturing/material/2015/02"
       xmlns:lg="urn:example:litegen">
  <metadata name="Title">litegen reference</metadata>
  <metadata name="Application">make_ref_3mf.py</metadata>
  <metadata name="lg:SourceFormat">glTF 2.0</metadata>
  <resources>
    <basematerials id="1">
      <base name="Red" displaycolor="#FF0000FF"/>
      <base name="White" displaycolor="#FFFFFFFF"/>   <!-- always 8 digits: assimp reads 6-digit as alpha 0 -->
    </basematerials>
    <m:colorgroup id="5">
      <m:color color="#FF0000"/><m:color color="#00FF00"/><m:color color="#0000FF"/><m:color color="#FFFF00"/>
    </m:colorgroup>
    <m:texture2d id="6" path="/3D/Texture/basecolor.png" contenttype="image/png" tilestyleu="wrap" tilestylev="wrap" filter="auto"/>
    <m:texture2dgroup id="7" texid="6">
      <m:tex2coord u="0" v="0"/><m:tex2coord u="1" v="0"/><m:tex2coord u="1" v="1"/><m:tex2coord u="0" v="1"/>
    </m:texture2dgroup>
    <object id="2" type="model" name="TexturedCube" pid="7" pindex="0">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0"/> <vertex x="20" y="0" z="0"/> <vertex x="20" y="20" z="0"/> <vertex x="0" y="20" z="0"/>
          <vertex x="0" y="0" z="20"/> <vertex x="20" y="0" z="20"/> <vertex x="20" y="20" z="20"/> <vertex x="0" y="20" z="20"/>
        </vertices>
        <triangles>   <!-- per face (bl,br,tr,tl seen from outside): (bl,br,tr) p=0,1,2 and (bl,tr,tl) p=0,2,3 -->
          <triangle v1="0" v2="1" v3="5" p1="0" p2="1" p3="2"/>  <triangle v1="0" v2="5" v3="4" p1="0" p2="2" p3="3"/>
          <triangle v1="1" v2="2" v3="6" p1="0" p2="1" p3="2"/>  <triangle v1="1" v2="6" v3="5" p1="0" p2="2" p3="3"/>
          <triangle v1="2" v2="3" v3="7" p1="0" p2="1" p3="2"/>  <triangle v1="2" v2="7" v3="6" p1="0" p2="2" p3="3"/>
          <triangle v1="3" v2="0" v3="4" p1="0" p2="1" p3="2"/>  <triangle v1="3" v2="4" v3="7" p1="0" p2="2" p3="3"/>
          <triangle v1="4" v2="5" v3="6" p1="0" p2="1" p3="2"/>  <triangle v1="4" v2="6" v3="7" p1="0" p2="2" p3="3"/>
          <triangle v1="1" v2="0" v3="3" p1="0" p2="1" p3="2"/>  <triangle v1="1" v2="3" v3="2" p1="0" p2="2" p3="3"/>
        </triangles>
      </mesh>
    </object>
    <object id="3" type="model" name="ColoredTetra" pid="5" pindex="0">
      <mesh>
        <vertices><vertex x="0" y="0" z="0"/><vertex x="20" y="0" z="0"/><vertex x="10" y="20" z="0"/><vertex x="10" y="7" z="20"/></vertices>
        <triangles>
          <triangle v1="0" v2="2" v3="1" p1="0" p2="2" p3="1"/> <triangle v1="0" v2="1" v3="3" p1="0" p2="1" p3="3"/>
          <triangle v1="1" v2="2" v3="3" p1="1" p2="2" p3="3"/> <triangle v1="2" v2="0" v3="3" p1="2" p2="0" p3="3"/>
        </triangles>
      </mesh>
    </object>
    <object id="4" type="model" name="BaseMatTetra" pid="1" pindex="0">
      <mesh>
        <vertices><vertex x="0" y="0" z="0"/><vertex x="20" y="0" z="0"/><vertex x="10" y="20" z="0"/><vertex x="10" y="7" z="20"/></vertices>
        <triangles>
          <triangle v1="0" v2="2" v3="1"/> <triangle v1="0" v2="1" v3="3"/> <triangle v1="1" v2="2" v3="3"/>
          <triangle v1="2" v2="0" v3="3" pid="1" p1="1"/>   <!-- per-triangle base material override -->
        </triangles>
      </mesh>
    </object>
    <object id="10" type="model" name="Assembly">         <!-- no pid/pindex on component objects -->
      <components>
        <component objectid="2"/>
        <component objectid="3" transform="1 0 0 0 1 0 0 0 1 30 0 0"/>
        <component objectid="4" transform="1 0 0 0 1 0 0 0 1 60 0 0"/>
      </components>
    </object>
  </resources>
  <build>
    <item objectid="10" transform="1 0 0 0 1 0 0 0 1 50 50 0"/>
  </build>
</model>
```

The three.js result [L]:
- Assembly at (50,50,0); children at (30,0,0) and (60,0,0).
- Cube: 36 non-indexed verts with uv, map 64×64 loaded.
- Tetra: `vertexColors:true`.
- Base-material tetra: 2 meshes (Red with 3 tris, White with 1 tri).

**glTF → 3MF writer recipe.**
1. Flatten the scene to world space, or to components per mesh node; 3MF has no empty grouping nodes.
2. Convert Y-up → Z-up and metres → mm, or write `unit="meter"`.
3. **Weld vertices by exact position** within each object. glTF splits vertices at UV/normal seams; 3MF needs shared vertices for manifold edges, and its UVs and colours are per-corner anyway.
4. Emit per-corner `p1/p2/p3` into a deduplicated `texture2dgroup` (u, 1−v) or `colorgroup`.
5. Keep CCW winding. Mirrored glTF node transforms (det<0) are fine in item/component transforms: consumers must keep positive volume.
6. Set object `pid`/`pindex` defaults whenever any triangle has properties. Add `pid` on each triangle for assimp.
7. Colours: linear glTF → sRGB 8-bit, 8-digit `#RRGGBBAA`.
8. Don't set `requiredextensions="m"`.

---

## C. COLLADA (DAE) 1.4.1

Primary source [S]: *COLLADA – Digital Asset Schema Release 1.4.1 Specification (2nd Edition), March 2008* (https://www.khronos.org/files/collada_spec_1_4.pdf). Text was extracted and searched locally.

### C1. Document skeleton

`dae/ref.dae` passed xmllint and loads in three.js (render `renders/three_ref_dae.png`, upright texture) and in assimp (2 meshes, 1 animation, 3 channels, 2 materials) [L]. It is condensed here; `…` marks repeated parts.

```xml
<?xml version="1.0" encoding="utf-8"?>
<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
  <asset>
    <contributor><authoring_tool>litegen</authoring_tool></contributor>
    <created>2026-10-12T00:00:00</created><modified>2026-10-12T00:00:00</modified>   <!-- both REQUIRED by schema -->
    <unit name="meter" meter="1"/>
    <up_axis>Y_UP</up_axis>
  </asset>
  <library_images>
    <image id="basecolor-img" name="basecolor"><init_from>basecolor.png</init_from></image>  <!-- no whitespace inside init_from -->
  </library_images>
  <library_effects>
    <effect id="TexturedMat-fx">
      <profile_COMMON>
        <newparam sid="basecolor-surface"><surface type="2D"><init_from>basecolor-img</init_from></surface></newparam>
        <newparam sid="basecolor-sampler"><sampler2D><source>basecolor-surface</source></sampler2D></newparam>
        <technique sid="common">
          <lambert>   <!-- or phong/blinn (+specular, shininess) or constant -->
            <emission><color sid="emission">0 0 0 1</color></emission>
            <diffuse><texture texture="basecolor-sampler" texcoord="UVSET0"/></diffuse>
            <transparent opaque="A_ONE"><color>1 1 1 1</color></transparent>   <!-- or <texture …/> for alpha from map -->
            <transparency><float sid="transparency">1</float></transparency>
          </lambert>
          <extra><technique profile="GOOGLEEARTH"><double_sided>1</double_sided></technique></extra>  <!-- three.js reads this -->
        </technique>
      </profile_COMMON>
    </effect>
    <effect id="RedMat-fx"><profile_COMMON><technique sid="common"><phong>
      <emission><color sid="emission">0 0 0 1</color></emission>
      <diffuse><color sid="diffuse">1 0 0 1</color></diffuse>
      <specular><color sid="specular">0.04 0.04 0.04 1</color></specular>
      <shininess><float sid="shininess">20</float></shininess>
    </phong></technique></profile_COMMON></effect>
  </library_effects>
  <library_materials>
    <material id="TexturedMat-mat" name="TexturedMat"><instance_effect url="#TexturedMat-fx"/></material>
    <material id="RedMat-mat" name="RedMat"><instance_effect url="#RedMat-fx"/></material>
  </library_materials>
  <library_geometries>
    <geometry id="StripMesh-geo" name="StripMesh"><mesh>
      <source id="StripMesh-geo-pos">
        <float_array id="StripMesh-geo-pos-array" count="27">-0.5 0 0 0.5 0 0 … 1.5 1 0</float_array>
        <technique_common><accessor source="#StripMesh-geo-pos-array" count="9" stride="3">
          <param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/></accessor></technique_common>
      </source>
      <source id="StripMesh-geo-nrm">…count="1" stride="3"… 0 0 1</source>
      <source id="StripMesh-geo-uv">…stride="2" params S,T… (u, 1-v) per vertex</source>
      <source id="StripMesh-geo-col">…stride="4" params R,G,B,A…</source>
      <vertices id="StripMesh-geo-vtx"><input semantic="POSITION" source="#StripMesh-geo-pos"/></vertices>
      <triangles count="4" material="TexturedMat-sym">
        <input semantic="VERTEX"   source="#StripMesh-geo-vtx" offset="0"/>
        <input semantic="NORMAL"   source="#StripMesh-geo-nrm" offset="1"/>      <!-- multi-index: offsets select columns in <p> -->
        <input semantic="TEXCOORD" source="#StripMesh-geo-uv"  offset="0" set="0"/>
        <input semantic="COLOR"    source="#StripMesh-geo-col" offset="0" set="0"/>
        <p>0 0 1 0 3 0 0 0 3 0 2 0 2 0 3 0 5 0 2 0 5 0 4 0</p>                    <!-- stride = max offset + 1 = 2 -->
      </triangles>
      <triangles count="1" material="RedMat-sym">…<p>6 0 7 0 8 0</p></triangles>
    </mesh></geometry>
  </library_geometries>
  <library_controllers>
    <controller id="Strip-skin"><skin source="#StripMesh-geo">
      <bind_shape_matrix>1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1</bind_shape_matrix>
      <source id="Strip-skin-joints"><Name_array id="Strip-skin-joints-array" count="2">Bone0 Bone1</Name_array>   <!-- node SIDs -->
        <technique_common><accessor source="#Strip-skin-joints-array" count="2" stride="1"><param name="JOINT" type="name"/></accessor></technique_common></source>
      <source id="Strip-skin-ibm"><float_array id="Strip-skin-ibm-array" count="32">
          1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1   1 0 0 0 0 1 0 -1 0 0 1 0 0 0 0 1</float_array>        <!-- row-major text: T(0,-1,0) -->
        <technique_common><accessor source="#Strip-skin-ibm-array" count="2" stride="16"><param name="TRANSFORM" type="float4x4"/></accessor></technique_common></source>
      <source id="Strip-skin-w"><float_array id="Strip-skin-w-array" count="2">0.5 1</float_array>…</source>
      <joints><input semantic="JOINT" source="#Strip-skin-joints"/><input semantic="INV_BIND_MATRIX" source="#Strip-skin-ibm"/></joints>
      <vertex_weights count="9">
        <input semantic="JOINT" source="#Strip-skin-joints" offset="0"/>
        <input semantic="WEIGHT" source="#Strip-skin-w" offset="1"/>
        <vcount>1 1 2 2 1 1 1 1 1</vcount>
        <v>0 1 0 1 0 0 1 0 0 0 1 0 1 1 1 1 0 1 0 1 0 1</v>                           <!-- (jointIndex, weightIndex) pairs -->
      </vertex_weights>
    </skin></controller>
  </library_controllers>
  <library_animations>
    <animation id="Bone1-anim">
      <source id="Bone1-anim-in"><float_array id="Bone1-anim-in-array" count="3">0 1 2</float_array>   <!-- seconds -->
        <technique_common><accessor source="#Bone1-anim-in-array" count="3" stride="1"><param name="TIME" type="float"/></accessor></technique_common></source>
      <source id="Bone1-anim-out"><float_array id="Bone1-anim-out-array" count="48">…3 baked 4x4, row-major…</float_array>
        <technique_common><accessor source="#Bone1-anim-out-array" count="3" stride="16"><param name="TRANSFORM" type="float4x4"/></accessor></technique_common></source>
      <source id="Bone1-anim-interp"><Name_array id="Bone1-anim-interp-array" count="3">LINEAR LINEAR LINEAR</Name_array>
        <technique_common><accessor source="#Bone1-anim-interp-array" count="3" stride="1"><param name="INTERPOLATION" type="name"/></accessor></technique_common></source>
      <sampler id="Bone1-anim-sampler">
        <input semantic="INPUT" source="#Bone1-anim-in"/><input semantic="OUTPUT" source="#Bone1-anim-out"/>
        <input semantic="INTERPOLATION" source="#Bone1-anim-interp"/></sampler>
      <channel source="#Bone1-anim-sampler" target="Bone1/transform"/>               <!-- node id / matrix sid -->
    </animation>
    …Root-anim (target "Root/transform"), Bone0-anim (STEP)…
  </library_animations>
  <library_visual_scenes>
    <visual_scene id="Scene" name="Scene">
      <node id="Root" name="Root" type="NODE">
        <matrix sid="transform">1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1</matrix>
        <node id="Strip" name="Strip" type="NODE">
          <matrix sid="transform">1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1</matrix>
          <instance_controller url="#Strip-skin">
            <skeleton>#Bone0</skeleton>                                              <!-- root joint NODE ID -->
            <bind_material><technique_common>
              <instance_material symbol="TexturedMat-sym" target="#TexturedMat-mat">
                <bind_vertex_input semantic="UVSET0" input_semantic="TEXCOORD" input_set="0"/>
              </instance_material>
              <instance_material symbol="RedMat-sym" target="#RedMat-mat"/>
            </technique_common></bind_material>
          </instance_controller>
        </node>
        <node id="Bone0" name="Bone0" sid="Bone0" type="JOINT">
          <matrix sid="transform">1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1</matrix>
          <node id="Bone1" name="Bone1" sid="Bone1" type="JOINT">
            <matrix sid="transform">1 0 0 0 0 1 0 1 0 0 1 0 0 0 0 1</matrix>         <!-- translation (0,1,0) in 4th column -->
          </node>
        </node>
      </node>
    </visual_scene>
  </library_visual_scenes>
  <scene><instance_visual_scene url="#Scene"/></scene>
</COLLADA>
```

**Spec rules that matter** [S]:

**Matrices.** "Matrices in COLLADA are column matrices in the mathematical sense. These matrices are written in row-major order to aid the human reader", so translation is at text indices 3, 7, 11. glTF column-major `e` → text `e0 e4 e8 e12  e1 e5 e9 e13  e2 e6 e10 e14  e3 e7 e11 e15`. `bind_shape_matrix` and float4x4 sources use the same convention [S][L: assimp writes IBM T(0,−1,0) as `1 0 0 0 0 1 0 -1 …`].

**Node transforms.** A node may hold any sequence of `<matrix>`, `<translate>`, `<rotate>` (axis + **degrees**), `<scale>`, `<lookat>` and `<skew>`, applied in document order (post-multiplied). For glTF, write either one `<matrix sid="transform">`, or `<translate sid="location"> <rotate sid="rotationZ">… <scale sid="scale">`. glTF quaternions have no COLLADA element: decompose to axis-angle with one `<rotate>`, or use the matrix.

**Skinning equation.** `v_out = Σ_i ((v · BSM) · IBM_i · JM_i) · JW_i`, where BSM is the bind-shape matrix, IBM_i the inverse bind matrix of joint i, JM_i the joint's current transform and JW_i the weight.
- "If <bind_shape_matrix> is not specified then an identity matrix may be used".
- glTF IBMs map directly to INV_BIND_MATRIX, with BSM = identity.
- Joints come from `<instance_controller><skeleton>` (the root node(s) to search) and the `JOINT` `Name_array` of node **sids** (`IDREF_array` = node ids is also legal). Skin weights must sum to 1.

**Morph.** `<controller><morph source="#base" method="NORMALIZED|RELATIVE">` with a MORPH_TARGET source (`IDREF_array` of geometry ids) and a MORPH_WEIGHT source, plus `<targets>` inputs.
- NORMALIZED: `(1−Σw)·Base + Σ w_i·Target_i` (absolute targets).
- RELATIVE: `Base + Σ w_i·Target_i` (delta targets). The default method is NORMALIZED.
- Targets "must all have the same set of vertices… only interpolates the data in their <vertices>".
- glTF deltas → RELATIVE with delta targets, or NORMALIZED with absolute `base+delta` targets. NORMALIZED with absolute targets is the most common in the wild [UNVERIFIED survey].
- A skin's `source` may point to the morph controller (skin-over-morph).

**Animation target addressing.** `target = <id> ( "/" <sid> )* [ "." member | "(" i ")" [ "(" j ")" ] ]`. Examples:
- `Bone1/transform` (whole matrix, stride 16);
- `Bone1/transform(0)(3)` (element, row 0 col 3);
- `Bone1/location.X`;
- `Bone1/rotationZ.ANGLE`;
- `Bone1/scale.Y`.

Morph weights: `<weightsArrayId>(k)` (zero sids, valid per spec, but see C2). Times (`INPUT`, `TIME` param) are in **seconds**.

**INTERPOLATION source.** A `Name_array` per key: `LINEAR`, `STEP`, `BEZIER` (+ `IN_TANGENT`/`OUT_TANGENT` sources), `HERMITE`, `BSPLINE`, `CARDINAL`.

**Transparency.** Only A_ONE and RGB_ZERO exist in 1.4.1; A_ZERO and RGB_ONE are 1.5.
- A_ONE: `result = fb·(1 − transparent.a·transparency) + mat·(transparent.a·transparency)`, i.e. opacity = `transparent.a × transparency`.
- RGB_ZERO: per-channel `opacity = 1 − transparent.rgb × transparency`.
- Missing `<transparent>` = colour (1,1,1,1) with A_ONE; missing `<transparency>` = 1.0.
- glTF BLEND/MASK with a texture → `<transparent opaque="A_ONE"><texture texture="sampler" texcoord="UVSET0"/></transparent>`. COLLADA has no alpha cutoff, so MASK becomes blend [UNVERIFIED alternative extras].

**Texture coordinates.** COLLADA is V-up: `(u, 1−v)` [L: assimp flips glTF V on export; the three.js render of `ref.dae` is upright with flipY=true].

**Colour values.** The spec doesn't define a colour space. three.js treats `<color>` values (diffuse, specular, emission) **and per-primitive COLOR inputs as sRGB**, converting them to linear [S: ColladaComposer.js]. So write sRGB-encoded colours if three.js fidelity matters. Other consumers [UNVERIFIED].

**`<image><init_from>`.**
- 1.4.1 form: `<init_from>relative/uri.png</init_from>`. URIs must be valid: spaces as `%20`, `/` separators [S].
- Hex-embedded `<data>` exists in the 1.4.1 schema but three.js doesn't read it.
- `data:` URIs: three.js's TextureLoader would accept them [UNVERIFIED for others]. Prefer sidecar files or `.zae`.

### C2. three.js r183 ColladaLoader support

Source: `loaders/collada/ColladaParser.js`, `ColladaComposer.js` [S]. Test: headless Chrome 154 on `ref.dae`, `morph*.dae`, `assimp.dae`, `ref.zae` [L].

**Geometry.**
- `triangles`, `polylist` (fan), `lines` and `linestrips` are read. **`polygons` is unsupported** (warn).
- Inputs: VERTEX (POSITION/NORMAL/COLOR/TEXCOORD inside `<vertices>`), NORMAL, COLOR, TEXCOORD (set 0 → `uv`, set>0 → `uv1`), with offsets.
- All primitives of the same type merge into one geometry with groups per primitive, which become material indices via the `material` symbol.
- `bind_vertex_input` is **ignored**. The texcoord set is decided by the input `set`.

**Vertex colours** [L]:
- They load into the `color` attribute, but `material.vertexColors` is never enabled, so they're invisible by default.
- If a geometry mixes primitives with and without COLOR, the arrays misalign (12 vs 15 in `ref.dae`).
- The per-primitive COLOR sRGB→linear conversion has an off-by-one index bug (`startIndex = array.length - sourceStride - 1`).
- COLOR inside `<vertices>` is not converted.

**Materials.**
- Only `profile_COMMON`. `phong`/`blinn` → MeshPhongMaterial, `lambert` → MeshLambertMaterial, `constant` → MeshBasicMaterial.
- Read: diffuse (colour/texture, sRGB), specular, shininess, emission, ambient texture (as lightMap), bump texture (as normalMap), transparent/transparency (all 4 opaque modes), and the `extra/technique/double_sided` → DoubleSide [L].
- MAYA/FCOLLADA texture extras: `repeatU/V`, `offsetU/V`, `wrapU/V`.

**Images.**
- `<library_images><image><init_from>TEXT</init_from>`: the first `init_from`'s textContent is used verbatim, so don't pretty-print whitespace into it.
- It is loaded relative to the .dae URL (`LoaderUtils.extractUrlBase`) or `resourcePath`; `.tga` goes through TGALoader [L: 64×64 PNG loaded].
- The sampler → surface → image chain is resolved by sid. If the sampler is missing, the texture attribute is treated as an image id ("see #12530").

**Scene.**
- `<node>` with `matrix/translate/rotate/scale`. Maya pivot sids (`rotatePivot…`) are handled by a transform hierarchy.
- `type="JOINT"` → `Bone`, whose name is the node **sid** (not the name).
- `instance_geometry`, `instance_controller`, `instance_node`, `instance_camera`, `instance_light`.
- `asset/unit@meter` → scene scale; `Z_UP` → rotate −90° about X (vertex data untouched).

**Skinning** [L]:
- Requires `skin source` = a **geometry** id and a joints `Name_array` that matches bone (node **sid**) names.
- `INV_BIND_MATRIX` (transposed on read), `bind_shape_matrix` → `bindMatrix`, `<skeleton>` → root node id (or visual scene id).
- At most 4 influences per vertex (sorted by weight, renormalised).
- `vertex_weights/v` is assumed to have exactly 2 inputs (JOINT, WEIGHT).
- `IDREF_array` joint sources are not parsed.

**Animation** [L]:
- Channels target `nodeId/sid` with member (`.X`/`.ANGLE`) or array (`(i)(j)`) syntax.
- `matrix` channels (full or element) are decomposed per key into position/quaternion/scale tracks. **STEP/BEZIER on matrix channels become LINEAR**: the `Bone0` STEP track came out as `interp=2301`, i.e. InterpolateLinear.
- translate/scale per-axis or full vectors, and rotate ANGLE (composed in node order), honour uniform STEP and BEZIER (full-vector only).
- Clips come from `<library_animation_clips>`, otherwise a single "default" clip holds everything. Time is in seconds.

**Morph:** "Morph target animation not supported yet." [L]:
- skin-over-morph → `TypeError … reading 'sources'`;
- morph-only `instance_controller` → `… reading 'joints'`;
- a channel target without `/` (e.g. `Strip-morph-weights-array(0)`) → `parseAnimationChannel` throws (`reading 'indexOf'`).
- **Any morph in a DAE makes three.js fail the whole load. Don't emit morphs if three.js matters.** Alternatively, bake them as vertex animation, which COLLADA can't express.

**assimp's own DAE** (C3) throws in `aggregateAnimationChannels` (missing sources) [L].

**Writer rules for three.js-compatible DAE:**
- `<triangles>`;
- a joints `Name_array` of sids, with sid = unique joint name;
- `type="JOINT"` on every joint node;
- `<skeleton>#rootJointId</skeleton>`;
- one `<matrix sid="transform">` per node, animated via `id/transform` with baked keys (≥ glTF key density, plus extra samples for rotations > 90°/segment);
- no morphs;
- sRGB colours;
- relative image paths with no whitespace.

### C3. What `assimp export test.gltf out.dae` writes [L]

The input glTF has 2 primitives, a texture, MR + normal maps, COLOR_0, a 2-joint skin, a morph target, rotation/translation/STEP-scale/weights animations.

**Asset, images, effects.**
- `<unit name="meter" meter="1"/>`, `Y_UP`.
- Images: `TexturedMat-diffuse-image` → `basecolor.png`, `TexturedMat-normal-image` → `normal.png`. The **MR texture is dropped**.
- Effects: `phong` with `diffuse <texture texture="…-sampler" texcoord="CHANNEL0"/>`, `bump <texture …/>`, `transparency 1` (no `<transparent>`), and `shininess` 0 or 250 (roughness 0.5 became 250).

**Geometry.**
- One geometry per glTF primitive. `polylist` with every input at **offset 0** (single index).
- Vertices are re-ordered.
- TEXCOORD V is **flipped**: the glTF bottom-left uv (0,1) became (0,0).
- COLOR is RGB only, with alpha dropped.

**Controllers.**
- One skin per primitive: `bind_shape_matrix` identity, `Name_array` `Bone0 Bone1`.
- INV_BIND_MATRIX is **row-major text** (`1 0 0 0 0 1 0 -1 …`).
- Weights are deduplicated into a source, with vcount/v pairs.
- **No morph controller.**

**Scene.**
- The glTF root node became the `<visual_scene id="Root">` itself, so its transform and animation were lost.
- Joints are `type="JOINT"` with `id=sid=name` and `<matrix sid="matrix">`.
- The skinned node gets **two** `instance_controller`s with `<skeleton>#Bone0</skeleton>`, and `bind_vertex_input semantic="CHANNEL0" input_semantic="TEXCOORD" input_set="0"`.

**Animations.**
- Samplers + channels with targets `Bone0/matrix`, `Root/matrix` (Root is not a node, so the target dangles) and `Bone1/matrix`.
- The **`<source>` elements they reference are missing entirely**.

**Result.**
- `assimp info out.dae` → "Unable to load input file".
- three.js → TypeError.
- Don't use assimp's DAE exporter for animated content.

assimp 6.0 also cannot import COLLADA morphs at all [S][L]. In `ColladaParser::ReadController` the `<targets>` loop is `for (XmlNode currentChildNode = node.first_child(); currentNode; currentNode = currentNode.next_sibling())`: it reads the wrong node, so `mMorphTarget` stays "" and the import fails with "Unable to resolve library reference ''". assimp also recognises morph-weight animation channels only when the target id contains `"morph-weights"` (`ColladaLoader.cpp`) [S]. It logs a non-fatal "Collada: Unexpected effect input mapping" on `bind_vertex_input` files [L].

### C4. .zae (zipped DAE)

COLLADA 1.5 defines `.zae` as a zip with a `manifest.xml` (`<dae_root>./model.dae</dae_root>`) plus the DAE and its images [UNVERIFIED: 1.5 spec text not re-read here; structure per common usage].

| Reader | Result |
|---|---|
| assimp 6.0 | `.zae` in `listext`; `ref.zae` with manifest **and** a variant without manifest (DAE in a subfolder) both import [L] |
| three.js | no ZAELoader. `KMZLoader.parse(zip)` looks for `doc.kml`, else the first `*.dae` entry, and resolves textures by path suffix inside the zip. `ref.zae` loaded fully, with texture, skin and animation [L], plus the warning "KMZLoader: Missing doc.kml file." |
| Blender | COLLADA import/export removed in **5.0** (release notes: "Collada (`.dae`) import & export has been removed.") [S]. Blender ≤4.5's importer was `.dae` only [UNVERIFIED] |

**Verdict.**
- Worth supporting as an *output convenience* if you already write DAE: zip the `.dae` + images, `manifest.xml` first, Deflate is fine.
- Low priority as an *input*: few producers emit it.
- For web delivery prefer glTF; for COLLADA consumers ship `.dae` + sidecar images or `.zae`.

### C5. glTF → DAE mapping summary

| glTF | COLLADA 1.4.1 |
|---|---|
| node TRS / matrix | `<node id sid name type="NODE">` + `<matrix sid="transform">` (row-major text of the column-vector matrix) |
| mesh primitive | `<triangles material="sym">` inside one `<mesh>` (merge primitives sharing vertices) or one geometry per primitive |
| POSITION / NORMAL / TEXCOORD_n / COLOR_0 | `<source>` + `float_array` + `accessor` (X,Y,Z / S,T / R,G,B,A); `TEXCOORD set="n"` with `(u, 1−v)` |
| material (PBR) | `profile_COMMON` `<lambert>`/`<phong>`; diffuse = baseColor (texture via surface/sampler2D newparams; factor lost unless baked); emission; `<transparent opaque="A_ONE">` + `<transparency>` for alpha. Metallic/roughness/normal/occlusion have no common-profile equivalent (normal → `bump` is read by three.js and assimp but not standard) |
| doubleSided | `<extra><technique profile="GOOGLEEARTH"><double_sided>1</double_sided></technique></extra>` inside `<technique sid="common">` (three.js) [S]; MAX3D/FCOLLADA variants exist [UNVERIFIED] |
| skin | `<controller><skin source="#geo">` with BSM = identity, joints `Name_array` (sids), INV_BIND_MATRIX = glTF IBMs (transposed text), weights; scene `<instance_controller><skeleton>#rootJointId</skeleton>` in an identity-transform node |
| morph targets | `<morph method="RELATIVE">` with delta target geometries; weights animated via `targetArray(k)`. **Breaks three.js and assimp 6.0**: omit unless the target consumer is known |
| animations | one `<animation>` per channel; INPUT = seconds; OUTPUT float4x4 baked matrices for rotation channels (or `rotate` ANGLE per axis); INTERPOLATION Name_array LINEAR/STEP (STEP lost on matrix channels in three.js); CUBICSPLINE → bake |

---

## Appendix 1: environment and local artifacts

**Tools.**
- Apple USD Tools 0.25.2 (`/usr/bin`). Its glTF plugin reports "USD-Fileformat-Plugins 1.0.8, USD version 2502".
- assimp 6.0 (`/opt/homebrew/bin/assimp`).
- three.js 0.183.2 (`dashboard/node_modules/three`, read-only).
- Node 24.15, Chrome 154 headless (isolated profile), python 3.14 (stdlib only), xmllint, pdftotext.

**Artifacts** under `scratchpad/usd3mfdae/`:
- **Inputs:** `gltf/test.gltf` (+ `.bin`, `basecolor.png`, `mr.png`, `normal.png`; generated by `scripts/make_gltf.py`).
- **USD:**
  - `usd/quad.usda`, `usd/skel.usda`, `usd/grammar.usda`, `usd/orient.usda`;
  - packages `usd/quad_usda_root.usdz`, `quad_usdc_root.usdz`, `quad_arkitAsset.usdz`, `three_export.usdz`;
  - `usd/apple_from_gltf.usda` (Apple's glTF import);
  - `neg/` negative tests (`scripts/negtests.sh`).
- **Crate:** `scripts/crate.py` (independent USDC decoder); `crate/stress.usda|usdc`; `usd/tiny.usdc`.
- **3MF:** `threemf/assimp.3mf`, `threemf/ref.3mf`, `ref_pid.3mf` (`scripts/make_ref_3mf.py`); assimp round-trips in `threemf/rt*/`.
- **DAE:** `dae/assimp.dae`, `dae/ref.dae`, `dae/morph*.dae` (`scripts/make_ref_dae.py`); `web/files/ref.zae`.
- **Harnesses:** `web/test.html` + `scripts/probe.sh` (headless Chrome running three.js loaders); `node/*.mjs` (three.js USD parsers in Node); `probe_*.json` outputs.
- **Renders:**
  - usdrecord/Storm: `renders/quad.png`, `skel_000.png`, `skel_2e+01.png`, `skelwrong_000.png`, `held_2e+01.png`, `orient.png`, `scale_*`;
  - three.js: `three_ref_dae.png`, `three_ref_3mf.png`.

## Appendix 2: sources

**OpenUSD** (release branch, fetched 2026-10-12):
- `pxr/usd/sdf/crateFile.cpp`, `crateFile.h`, `crateDataTypes.h`, `crateValueInliners.h`, `integerCoding.cpp`, `zipFile.cpp`
- `pxr/base/tf/fastCompression.cpp`
- `pxr/usd/usd/interpolators.cpp`
- `docs/spec_usdz.rst`, `docs/spec_usdpreviewsurface.rst`, `CHANGELOG.md` — https://github.com/PixarAnimationStudios/OpenUSD
- `pxr/usd/usdUtils/complianceChecker.py` @v21.11…v24.11

**OpenUSD docs:**
- https://openusd.org/dev/spec_usdz.html
- https://openusd.org/release/spec_usdpreviewsurface.html
- https://openusd.org/dev/api/_usd_skel__schemas.html
- https://openusd.org/dev/api/class_usd_skel_skeleton.html

**Apple:**
- https://developer.apple.com/documentation/usd/validating-usd-files.md
- https://developer.apple.com/documentation/usd/creating-usd-files-for-apple-devices.md
- Forum threads on RealityKit blend shapes: https://developer.apple.com/forums/thread/766484, https://developer.apple.com/forums/thread/793431

**model-viewer:** https://github.com/google/model-viewer/blob/master/packages/model-viewer/src/features/ar.ts

**glTF KHR_texture_transform:** https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Khronos/KHR_texture_transform/README.md

**3MF:**
- Core v1.4.0: https://github.com/3MFConsortium/spec_core
- Materials v1.2.1: https://github.com/3MFConsortium/spec_materials

**Slicers:**
- PrusaSlicer `src/libslic3r/Format/3mf.cpp` @version_2.9.4 and `src/slic3r-shared/.../Format/3mf/Model3mf.cpp` @master — https://github.com/prusa3d/PrusaSlicer
- Bambu Studio `src/libslic3r/Format/bbs_3mf.cpp` @master — https://github.com/bambulab/BambuStudio
- Cura `plugins/3MFReader/ThreeMFReader.py` @main — https://github.com/Ultimaker/Cura
- 3D Viewer deprecation: https://windowsreport.com/microsoft-officially-deprecates-3d-viewer-app-cut-off-date-set-for-july-2/

**assimp v6.0.0:** `code/AssetLib/Collada/ColladaParser.cpp`, `ColladaLoader.cpp`, `code/AssetLib/3MF/D3MFExporter.cpp`, `XmlSerializer.cpp` — https://github.com/assimp/assimp

**COLLADA:** 1.4.1 2nd ed. spec, https://www.khronos.org/files/collada_spec_1_4.pdf

**Blender 5.0 release notes:** https://developer.blender.org/docs/release_notes/5.0/pipeline_io/

**three.js r183 sources (local, MIT):**
- `examples/jsm/loaders/USDLoader.js`, `usd/USDAParser.js`, `usd/USDCParser.js`, `usd/USDComposer.js`
- `exporters/USDZExporter.js`
- `loaders/3MFLoader.js`, `ColladaLoader.js`, `collada/ColladaParser.js`, `collada/ColladaComposer.js`, `KMZLoader.js`
