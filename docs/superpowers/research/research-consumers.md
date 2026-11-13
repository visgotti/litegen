# Consumer research for the mesh-gap specs

Date: 2026-11-04. Written for the specs in `docs/superpowers/specs/2026-11-04-mesh-*`.

This note builds on `docs/superpowers/research/research-usd-3mf-dae.md` and `research-fbx.md`, and it does not repeat what they settle. Nothing was built: no cargo and no build, and no repo file was written except this one. Three parallel forks of this research agent researched Q1 and Q2, Q3 and Q4, and Q8 and Q10. Their sections are kept as they reported them, with one change: a quote from the Blender 5.0 release notes was added to Q4. A sample of their citations was re-read and held: FBXLoader's layer warning and version checks, Blender's ASCII refusal, USDComposer's clip loop, and USDA's fixed 30 fps. Q5, Q6, Q7 and Q9 were done directly.

**Tags**

- **[S]** is a primary source at the pinned ref named: a spec, upstream source or vendor docs. Source citations read `<repo>@<tag> <path>:L<n>`.
- **[L]** was measured locally with the tools below.
- **[UNVERIFIED]** was not confirmed.

**Pinned refs**

- **three.js r183.** The local copy `dashboard/node_modules/three` (npm 0.183.2) is byte-identical to tag r183 for every file cited here. That was checked with `cmp` against raw.githubusercontent.com for:
  - the 11 GLTF, OBJ, PLY, FBX, USD and Collada loader files;
  - `3MFLoader.js` and `USDZExporter.js`;
  - `WebGLProgram.js`, `WebGLPrograms.js` and `uv_vertex.glsl.js`.

  r187 is checked where a question asks whether something changed.
- **Blender v5.2.3.** v4.5.15 is used for the Collada importer that 5.0 removed.
- **assimp v6.0.5**, the latest release (2026-04-30). Master at de6d25f is used where noted.
- **ufbx v0.23.1, OpenUSD v26.11.1 and Godot 4.7.2-stable.**
- **Babylon.js 9.33.2 and glTFast v6.0.0-pre.2.**
- **The glTF spec at `KhronosGroup/glTF@9d23a5a`, and glTF-Validator 2.0.0-dev.3.10.**
- **google/usd_from_gltf@6d288cc**, its last commit.
- **MaterialX 1.39.5 and 3MF Beam Lattice 1.2.0.**
- **The PLY writers** named in Q9.

**Local tools**

- Apple USD Tools 0.25.2 (`usdchecker`, `usdcat`, `usdrecord`; USD 25.02).
- assimp 6.0.5 (Homebrew).
- three.js r183 in Node 26.9.
- SceneKit and Model I/O on macOS 26.6.2, through `swift` scripts (Swift 6.3.3).

The probes ran in a temporary local folder that wasn't kept. The appendix gives the recipe for each one.

Contents:
1. Several animation clips in USD
2. Unlit materials in USD
3. FBX animation layers
4. ASCII FBX output
5. COLLADA morph controllers
6. Points and lines
7. UV sets beyond two
8. glTF cameras, KHR_lights_punctual and skin.skeleton
9. Big-endian PLY
10. glTF material extensions in other formats

---

## Q1. Several animation clips in USD

**Decides the design**
- USD has one timeline per stage (`startTimeCode`, `endTimeCode` and `timeCodesPerSecond` are layer metadata), and nothing in USD stands for a "clip". Apple documents one way to put alternative clips in a single USDZ: a variant set on the defaultPrim whose variants each switch `skel:animationSource`. Apple's own `variant_combiner.py` builds this, and Apple says defaultPrim variants appear in Quick Look as configuration options.
- In that layout every SkelAnimation sits outside the variant set, and each variant authors only `rel skel:animationSource` on the Skeleton. Two writer rules follow:
  - Author the default selection. USD applies no variant without one [L]; three.js instead falls back to the first variant.
  - Never author `skel:animationSource`, or an xformOp that a variant animates, outside the variant set. The local opinion is stronger than the variant's, and "an authored default in a stronger layer/primSpec **is stronger** than an animated value authored in a weaker layer" [S][L].
- All variants share one timeline length. Apple's tool sets the end to the min (its default) or the max of the clips. Use max: a shorter clip holds its last sample [L].
- The same file gives three.js every clip. three.js r183 ignores both the binding and the variants and returns one `AnimationClip` per SkelAnimation prim, named after the prim.
- RealityKit apps see one animation per USD timeline ("default subtree animation"). Apple offers two app-side workflows: one file per clip in an `AnimationLibraryComponent`, or one timeline with the clips end to end, cut up in Reality Composer Pro or with `AnimationView` `trimStart`/`trimEnd`.
- Rigid (xformOp) clips can sit inside variants as per-clip `.timeSamples`. Storm honours them, but three.js drops them and Apple's tool varies only SkelAnimation bindings.

| Consumer | What it does | Source |
|---|---|---|
| three.js r183 USDLoader | It makes one clip per SkelAnimation prim spec anywhere in the layer, named after the prim. It adds one `TransformAnimation` clip for xformOp samples, but only on prims typed Xform, Scope or Mesh. `skel:animationSource` is read and never used. In USDA, `variantSet` bodies and `over` prims are not parsed. In USDC, variant children are added as extra prims rather than merged, and the selection order is authored, then first variant. USDLoader passes no selections. Our test file gave clips "Bend" and "Twist", lost the variant xformOp samples, and duplicated empty `Box`/`Rig`/`Skel` objects. Separately, USDA ignores TCPS and FPS and always uses 30. | `three.js r183 examples/jsm/loaders/usd/USDComposer.js:L3586-3611, L3618-3628, L3456-3468, L3981-3982, L509-547, L682-729`; `USDAParser.js:L2, L440-459, L470`; `USDLoader.js:L226-273` [S][L] |
| Blender v5.2.3 USD importer | It opens the stage with its authored variant selections; there is no variant option. It imports only the bound animation (`GetAnimQuery`) as one action named after the SkelAnimation. Animated xforms become a Transform Cache constraint that reads the stage timeline. | `blender@v5.2.3 source/blender/io/usd/intern/usd_capi_import.cc:L150`, `usd_skel_convert.cc:L105-123`, `usd_reader_xform.cc:L43-53, L161` [S] |
| Blender v5.2.3 USD exporter | It writes one SkelAnimation per armature, named after the active action and sampled over the scene range. It cannot export several actions. | `usd_writer_armature.cc:L190-198, L118-121` [S] |
| usdview (OpenUSD v26.11.1) | A variant combo box calls `SetVariantSelection`. It has one timeline. | `pxr/usdImaging/usdviewq/variantComboBox.py:L11-24` [S] |
| usdrecord/Storm (Apple USD Tools 0.25.2) | It honours the selection, and `--sessionLayer` switches it. Bend and Twist rendered differently at frames 12 and 24. Variant xformOp samples played only when nothing outside the variant authored the attribute. With no selection authored, nothing moved. | [L]; `usdrecord.py:L141-144, L271-296` @v26.11.1 [S] |
| usdchecker --arkit 0.25.2 | It validates each variant of the defaultPrim separately ("Validation Result for Variants: Animation:Bend on Prim: /Root"), and all passed. Unknown `RealityKitComponent` prims are not flagged; only missing referenced files are. | [L] |
| AR Quick Look | Apple: "Variants that are defined in this `defaultPrim` hierarchy path are shown to the user as configuration options when using QuickLook with USDZ files", and "QuickLook allows you to change variants on the default prim". Apple's combiner supports "skeleton animation variants". | developer.apple.com/documentation/usd/creating-usd-files-for-apple-devices, …/validating-usd-files [S]; not device-tested |
| RealityKit (apps) | It loads the active variant. Changing it after load "depends on the context". A Blender USDZ yields one "default subtree animation". An Apple engineer (Aug 2025) recommends sequencing the clips on one timeline and cutting them in Reality Composer Pro. In 2020 an Apple engineer said "no way of storing multiple animations in a single USDZ file". | validating-usd-files [S]; developer.apple.com/forums/thread/797061, /650515 [S, forum] |
| Reality Composer Pro / `AnimationLibraryComponent` | Available from iOS 18, macOS 15 and visionOS 2. It is serialized as `def RealityKitComponent "AnimationLibrary" { uniform token info:id = "RealityKit.AnimationLibrary" }`, with `def RealityKitAnimationFile` children holding `uniform asset file = @clip.usdz@` and `uniform string name`. One child refers to `name = "default subtree animation"`. Apple says the library is persisted by "packaging them together into a `.reality` file". | developer.apple.com/documentation/realitykit/animationlibrarycomponent [S]; RCP files `simongraeber/FlexAlert@309befa …/Affe.rkassets/Scene.usda:L26-41`, `SilverWei/Kei-BA-USD-Anim@9b77221 …/Kei.usda:L265-291` [S] |
| Apple glTF importer (usdcat/Preview; Adobe USD-Fileformat-Plugins 1.0.8) | By default it keeps only the first glTF animation. With `x.gltf:SDF_FORMAT_ARGS:gltfAnimationTracks=true` it joins the tracks end to end, offsetting each by the previous track's max + 1. It records the ranges in `customLayerData.animationTracks.<name> = {minTime, maxTime, offset, displayName}` plus `defaultAnimationTrack`, and sets `endTimeCode` to the first track's max. | [L]; `adobe/USD-Fileformat-plugins@v1.2.0 utils/src/layerWriteSdfData.cpp:L81-97, L1271-1342`, `gltf/src/fileFormat.cpp:L36, L72` [S] |
| SceneKit | Apple rates its variant support "partial". | validating-usd-files [S] |

**Conventions compared**
- (a) **A variant set on the defaultPrim, each variant switching `skel:animationSource`.** Apple's tool builds it and Quick Look exposes it. Storm, usdchecker and Blender (default variant only) handle it, and three.js lists every clip.
  - Source: `Apple USD-Support-Scripts.zip variant_combiner.py:L338-408` (clears the base binding at L360, authors it inside each variant at L403, selects the first variant at L406, sets the end at L408).
  - Option: `README.md:L44, L60` ("--timecode-mode" min|max).
- (a′) **Variants holding per-clip xformOp samples.** Storm only [L]; three.js loses them [L]; RealityKit unknown.
- (b) **Several SkelAnimations with one bound.** Storm and Blender use the bound one; three.js exposes all of them.
- (c) **Clips end to end with ranges in `customLayerData`.** Adobe and Apple's importer write this [L]. RealityKit apps can trim it with `AnimationView` (iOS 15+, developer.apple.com/documentation/realitykit/animationview) [S]. Every other consumer sees one long clip.
- (d) **Value clips.** These sequence samples from several layers onto one timeline. Per the glossary they combine clips "similar to how non-linear video editing tools" do (`OpenUSD@v26.11.1 docs/glossary.rst:L3748-3751`; UsdSkel `schemas.dox:L14-16`); they are not alternatives. three.js has no support (no `clips` handling in USDComposer.js), and they are absent from Apple's feature tables.
- (e) **RCP AnimationLibrary.** It needs separate clip files. Support in plain USDZ or Quick Look is unknown.

**Recipe that was verified [L]**
```usda
def Xform "Root" ( variants = { string Animation = "Bend" }  prepend variantSets = "Animation" ) {
  def SkelRoot "Rig" { def Skeleton "Skel" ( prepend apiSchemas = ["SkelBindingAPI"] ) {
      def SkelAnimation "Bend" {…}  def SkelAnimation "Twist" {…} }  … }
  variantSet "Animation" = {
    "Bend"  { over "Rig" { over "Skel" { rel skel:animationSource = </Root/Rig/Skel/Bend> } } }
    "Twist" { over "Rig" { over "Skel" { rel skel:animationSource = </Root/Rig/Skel/Twist> } } } }
}
```

**Test without a device**
1. Run `usdchecker --arkit out.usdz`. It reports each variant [L].
2. For each clip, run `usdrecord --sessionLayer sel.usda -f 0:N out.usdz f_###.png`, where `sel.usda` is `over "Root" (variants = {string Animation = "<clip>"}) {}`. Frames must differ between clips [L].
3. Load the file with three.js USDLoader in Node and check that the clip names and durations match. Ship USDC, or use TCPS 30, because USDA durations assume 30 fps [L].
4. For RealityKit on a Mac, flatten one copy per variant with a wrapper layer (`subLayers = [@out.usdz@]` plus the selection), then run `usdcat --flatten`. Copy `start/endTimeCode`/`timeCodesPerSecond` into the wrapper, because flattening drops the sublayer's metadata [L]. Then load each copy with RealityKit `Entity(contentsOf:)` in a Swift script, check `availableAnimations`, and open the original with `qlmanage -p` (described only, not run).

**Not established**
- Device or Quick Look behaviour of an animation variant: whether the clip switches and restarts, and whether it loops over the shared range.
- Whether RealityKit honours variant-authored xformOp samples, unbound SkelAnimations, or `endTimeCode` as a clip bound.
- Whether a `RealityKitComponent "AnimationLibrary"` in a plain USDZ works in Quick Look.

## Q2. Unlit materials in USD

**Decides the design**
- UsdPreviewSurface has no unlit switch. Every converter emulates unlit as emission: diffuse black and `emissiveColor` = base colour.
- In Storm the recipe is exactly unlit [L]:
  - With `metallic = 1` (or `useSpecularWorkflow = 1` with a black `specularColor`), the pixel equals the linear base colour at 0°, 70° and 85° tilt.
  - With `metallic = 0`, 1–2/255 of specular remains.
- RealityKit has no exact UsdPreviewSurface-only unlit:
  - It does not support the specular workflow.
  - AR Quick Look adds a "rim light factor" (usd_from_gltf).
  - Only Apple's unlit node exposes a tone-map switch; the RealityKit PBR nodedef has none.
- Exact unlit in RealityKit takes the `ND_realitykit_unlit_surfaceshader` node, connected through `token outputs:mtlx:surface`, with:
  - `color3f inputs:color`
  - `float inputs:opacity`
  - `float inputs:opacityThreshold`
  - `bool inputs:applyPostProcessToneMap`
  - `bool inputs:hasPremultipliedAlpha`

  RealityKit reads MaterialX from USD on iOS 18, macOS 15 and visionOS 1 (`ShaderGraphMaterial` availability). Apple's own Godot plugin maps "unshaded" to this node with tone mapping off.
- `usdchecker --arkit` 0.25.2 accepts both that node and MaterialX `ND_surface_unlit`, and type-checks their inputs, for example "Expected 'color3f'; got 'float3'" [L]. Apple's tools ship the MaterialX libraries including `libraries/realitykit/realitykit_defs.mtlx:L53-60`.
- Writing both outputs breaks Storm:
  - Apple's Storm renders the RealityKit node opaque white whatever its colour.
  - Storm picks `outputs:mtlx:surface` over `outputs:surface` when both exist [L].
  - three.js reads only the UsdPreviewSurface.
- **Recommendation:**
  - Default to UsdPreviewSurface with `diffuseColor` (0,0,0), `metallic` 1, `roughness` 1 and `emissiveColor` = base factor, or the base texture `outputs:rgb` (sRGB) with scale = factor, plus opacity as for a lit material.
  - Offer the RealityKit unlit `mtlx` output as an opt-in "RealityKit" profile.
  - No standard marker survives the trip, so a reader has to detect the recipe to restore `KHR_materials_unlit`.

| Consumer | What it does | Source |
|---|---|---|
| google/usd_from_gltf | UsdPreviewSurface with `metallic = 1`, `roughness = 1`, `emissiveColor` = base colour (factor or texture) and `diffuseColor` black. When the material blends, it uses a black base texture that keeps the source alpha. Its README: "emulates these with a pure emissive material … differences due a rim light factor in the AR Quick Look renderer". It writes emissive textures linear (`emissive_is_linear = true`) because Quick Look "treats the emissive texture as linear". | `google/usd_from_gltf@6d288cc convert/materializer.cc:L91-93, L458-560`; `README.md:L99-100`; `common/config.h:L207-210` [S] |
| Apple usdzconvert 0.66 | It has no unlit handling: such a material becomes lit PBR, and `metallicFactor` defaults to 1.0 when absent. | `KarpelesLab/usdpython@0d063f9 usdzconvert/usdStageWithGlTF.py:L654-713` [S] |
| Apple glTF importer (Preview/usdcat; Reality Converter's successor, Adobe plugins 1.0.8) | Unlit gives `emissiveColor` = base, `diffuseColor` black, and keeps the glTF metallic/roughness. The swap is skipped when the material also has an emissive. | [L] `usdcat unlit.gltf`; `adobe/USD-Fileformat-plugins@v1.2.0 gltf/src/gltfImport.cpp:L1359-1382` [S] |
| Blender v5.2.3 (glTF import, then USD export) | The glTF importer builds Emission plus a Light Path "Is Camera Ray" mix with Transparent, and no BSDF. The USD exporter looks only for a Principled or Diffuse BSDF and returns without a surface shader, so the material comes out empty. The MaterialX network is off by default; when on, Emission becomes `uniform_edf` and "Is Camera Ray" becomes 1. | `scripts/addons_core/io_scene_gltf2/blender/imp/KHR_materials_unlit.py:L8-49`; `source/blender/io/usd/intern/usd_writer_material.cc:L456-459, L1090-1099, L1739-1744`; `editors/io/io_usd.cc:L679-690`; `nodes/shader/nodes/node_shader_emission.cc:L36-47`, `node_shader_light_path.cc:L51-57` [S] |
| NVIDIA Omniverse Asset Converter | `KHR_materials_unlit` is missing from its supported glTF extension list, and materials import as MDL by default (`export_preview_surface` switches to UsdPreviewSurface). Cesium's "Handle unlit materials" has been open since 2022. | docs.omniverse.nvidia.com/extensions/latest/ext_asset-converter.html [S]; github.com/CesiumGS/cesium-omniverse/issues/36 [S] |
| three.js r183 USDZExporter | A mesh whose material is not MeshStandardMaterial is skipped with "Unsupported material type", and so is its whole subtree. This includes GLTFLoader's unlit MeshBasicMaterial. | `examples/jsm/exporters/USDZExporter.js:L445-477, L489-494` (identical to r183) [S][L] |
| three.js r183 USDLoader (read-back) | It always builds a MeshPhysicalMaterial. It applies only shaders with `info:id` `UsdPreviewSurface` or `arnold:openpbr_surface`, and ignores render-context outputs. The emission recipe gives `color` black and `emissive` = base; ND_* shaders alone give default white lit. | `examples/jsm/loaders/usd/USDComposer.js:L2389, L2419-2446` [S][L] |
| Storm (Apple usdrecord 0.25.2) | The emission recipes come out exact, and `ND_surface_unlit` is exact. The RealityKit unlit node renders white, with HdMtlx "Validation warnings … atan2". | [L] |
| RealityKit | It has `UnlitMaterial` ("doesn't respond to lights", `applyPostProcessToneMap` option). Its Shader Graph lists Unlit, PBR, Hair and Occlusion surfaces plus "Preview Surface" only; standard `surface_unlit` is not listed. MaterialX: ✔. Specular workflow: ✗. | developer.apple.com/documentation/realitykit/unlitmaterial, /shadergraph/realitykit/unlit-surface-(realitykit), /shadergraph/surface, /realitykit/shadergraphmaterial, /usd/validating-usd-files [S] |
| Apple plugins-for-godot | It maps `SHADING_MODE_UNSHADED` to `ND_realitykit_unlit_surfaceshader(baseColor, opacity, threshold, false, false)`. | `apple/plugins-for-godot@d213351 godotrealitykit/GodotRealityKit/Materials/base_material_builder.mm:L580-587` [S] |
| Reality Composer Pro output | It writes `token outputs:mtlx:surface.connect`, `token outputs:realitykit:vertex`, `uniform token info:id = "ND_realitykit_unlit_surfaceshader"` and `bool inputs:applyPostProcessToneMap = 1`. | `ynagatomo/SGMExamples@4344a4b …/Ex06.usda:L27-41` [S] |

**Storm measurements [L]**

Setup: linear base colour (0.8, 0.2, 0.2), so the expected sRGB value is (231, 124, 124). Default camera light; scene lights and `--disableCameraLight` had no effect in this build.

| Material | Result |
|---|---|
| Lit diffuse | 226 → 173 → 132 at 0°/70°/85° tilt |
| Diffuse 0 + emissive, specular workflow with black specular | 231 at every tilt |
| Diffuse 0 + emissive, metallic 1 | 231 at every tilt |
| Diffuse 0 + emissive, metallic 0 | 232/126 (slight specular) |
| `ND_surface_unlit` | 231 at every tilt |
| `ND_realitykit_unlit_surfaceshader` | 255 white |

**Not established**
- RealityKit or Quick Look rendering of either recipe on a device: the rim size, the tone mapping of emissive, and whether the linear emissive texture quirk still exists.
- Which output RealityKit picks when both `outputs:surface` and `outputs:mtlx:surface` exist, and what iOS 17 and earlier do with an mtlx-only material.
- Whether RealityKit accepts MaterialX `ND_surface_unlit`.
- Whether macOS Preview's Storm renders the RealityKit node correctly; Apple's usdrecord build does not.

## Q3. FBX animation layers

**Decides the design**
- **How layers combine (SDK).** An `AnimationStack` holds an ordered list of `AnimationLayer`s; the order is the file's OO connection order and layer 0 is the base. Each later layer is folded into the result so far:
  - `BlendMode`: 0 additive (the template default), 1 override, 2 override-passthrough, scaled by `Weight` (a percentage that can be animated).
  - `RotationAccumulationMode`: 0 composes quaternions, 1 adds Euler components.
  - `ScaleAccumulationMode`: 0 multiplies, using the weight as an exponent; 1 adds.
  - `Mute` and `Solo` change what is evaluated. `Lock` and `Color` only matter in the editor.
- **Only ufbx evaluates layers among the oracles.** None of the others reproduces the blended pose, as measured below:
  - three.js r183 plays layer 0 and warns.
  - assimp 6.0.5 pools every layer's curves and lets the last curve win for each component. It ignores weight and mode.
  - Both Blender 5.2.3 importers make one Action per layer and assign only the first.
- **Recommendation.** On read, bake each stack's layers into one glTF animation per stack using ufbx's formulas. Godot (`ufbx_bake_anim`), Unreal (`MergeAllLayerAnimation`) and Maya's "Bake Animation Layers" do the same. On write, emit one layer per stack, as Blender's exporter does (export_fbx_bin.py:L2055–L2075 and L3162–L3167 @v5.2.3). SDK-written Mixamo files are also one layer per stack.
- **Where ufbx departs from the SDK docs.** The spec must choose; ufbx is what Blender 5.x and Godot users see.
  - ufbx ignores layer 0's weight and mode. The SDK sample multiplies the base layer by its weight.
  - ufbx ignores `Weight` on a pure Override layer. MotionBuilder says the weight "still controls how much of the layer's animation plays".
  - ufbx never reads `Mute`, `Solo` or `BlendModeBypass`.
- **Who writes multi-layer files.** In practice only Maya and MotionBuilder. AI-vendor FBX are Blender-made (Meshy) or SDK-made (Mixamo), with one layer per stack.

**How layers appear in a file** (ufbx corpus `maya_anim_layers_over_acc_7500_ascii.fbx`, written by SDK 2019.2):
- The template is at L116–L131: `PropertyTemplate: "FbxAnimLayer"` with:
  - `P: "Weight","Number","","A",100`
  - `"Mute","bool"…0`, `"Solo","bool"…0`, `"Lock","bool"…0`
  - `"Color","ColorRGB","Color","",0.8,0.8,0.8`
  - `"BlendMode","enum"…0`, `"RotationAccumulationMode","enum"…0`, `"ScaleAccumulationMode","enum"…0`
  - `"BlendModeBypass","ULongLong"…0`
- Each layer stores only what differs (L846–L862):
  - `BaseLayer` is empty.
  - `X` has RotationAccumulationMode 1 and ScaleAccumulationMode 1.
  - `Y` has `"Weight","Number","","A+",0` and `BlendMode 2`.
- An animated weight is an `AnimCurveNode::Weight` with channel `d|Weight`. It is connected OO to the layer and OP to the layer's `"Weight"` property (`maya_anim_layer_anim_7700_ascii.fbx` L958–L977).

**Property semantics.** Left: FBX SDK 2020 `FbxAnimLayer` reference [S]. Right: ufbx v0.23.1 `ufbx.c` [S].

| Property | FBX SDK 2020 docs | ufbx v0.23.1 |
|---|---|---|
| `Weight` | "the percentage of influence this layer has during the evaluation. Default value is 100.0". The class is described as having "an animatable weight" | Divided by 100 and clamped to [0,1] (L22241–L22248). The animated value is read only for blended layers (L25780). Unused for layer 0 and for pure Override |
| `BlendMode` | Order is from fbxanimlayer.h:L140–L142; the HTML shows each enumerator's text one row off.<br>• 0 `eBlendAdditive`: "adds its animation to layers that precede it in the stack".<br>• 1 `eBlendOverride`: "overrides the animation of any layer that shares the same attributes and precedes it".<br>• 2 `eBlendOverridePassthrough`: "the Weight value influence how much animation from the preceding layers is allowed to pass-through … If the Weight is 50.0, half of this layer animation is mixed with half of the animation of the preceding layers".<br>• Default: "eModeAdditive". | • 0: `r += w·v`.<br>• 1: `r = v`.<br>• 2: `r = (1−w)·r + w·v`.<br>• Unknown values: `r = v`.<br>(L22221–L22238, L25722–L25756) |
| `RotationAccumulationMode` | • 0 `eRotationByLayer` (the default): "weighted per layer … concatenated quaternion values".<br>• 1 `eRotationByChannel`: "weighted per component … adding each independent Euler XYZ value". | `compose_rotation = (mode == 0)` (L22250). In the node's `RotationOrder`:<br>• Additive: `q·slerp(1, q_layer, w)`.<br>• Passthrough: `slerp(q, q_layer, w)`.<br>• Mode 1: adds or lerps each component.<br>(L25711–L25750) |
| `ScaleAccumulationMode` | • 0 `eScaleMultiply` (the default): "layer weight value as an exponent … multiplying each independent XYZ scale value".<br>• 1 `eScaleAdditive`: "adding each independent XYZ value". | `compose_scale = (mode == 0)` (L22251).<br>• Additive: `s *= abs(v)^w`.<br>• Passthrough: `abs(s)^(1−w) · abs(v)^w`.<br>• Mode 1: adds or lerps. |
| `Mute` / `Solo` | Mute "should be excluded from the evaluation"; Solo "the only one that should be processed during the evaluation". Both default false | Not read: ufbx.c contains no "Mute" or "Solo" |
| `Lock`, `Color` | Lock means "should no longer receive keyframes"; Color: "The FBX SDK does not use it" | Not read |
| `BlendModeBypass` | A flag per data type: "The blend mode in the animation layer is ignored … the value of the properties in the animation layer override the values in any previous layers" (SDK guide, "Bypassing the Blend Mode") | Not read |
| Layer 0 | SDK sample `Animation/main.cxx`: the base layer's value is the "key (or property, if not animated) value multiplied by the weight" | Always replaces the property: `if (layer_ix == 0) prop->value_vec3 = v` (L25813–L25817) |

More from ufbx:
- It exposes `weight`, `weight_is_animated`, `blended`, `additive`, `compose_rotation` and `compose_scale` on each layer (ufbx.h:L3124–L3129).
- It builds an `anim` for each layer and for each stack (ufbx.h:L3060); `scene->anim` is the first stack's (ufbx.c:L23496).
- Callers can choose layers and weights through `ufbx_anim_opts.layer_ids` and `override_layer_weights` (ufbx.h:L4990–L4993).

MotionBuilder help (2022 and 2024) [S]:
- "A new layer is set to Additive mode by default."
- With Override and Passthrough off, "Animating the Weight value still controls how much of the layer's animation plays in the result".
- "In MotionBuilder, Per Layer (known as Quaternion Rotation Accumulation mode in Maya) is the default. In Maya, Per Channel." That is why Maya layers carry `RotationAccumulationMode 1`.

**What each consumer does with several layers**

| Consumer | What it does | Source |
|---|---|---|
| three.js r183 FBXLoader | Warns "Encountered an animation stack with multiple layers, this is currently not supported. Ignoring subsequent layers.", then uses the stack's first child connection. Never reads weight or mode | `three.js r183 examples/jsm/loaders/FBXLoader.js:L2635, L2778–L2786` [S][L] |
| assimp 6.0.5 | • Makes one `aiAnimation` per stack (L2647) and pools every layer's curve nodes per property (L2677–L2686, L2909).<br>• `InterpolateKeys` overwrites `result[component]` for each curve (L3576), so the last layer's curve wins at the union of key times.<br>• `LayerMap` is unused (`/*layers*/`, L3634). The comment at L3640–L3641 admits "There is a FBX property in the layer to specify the behaviour".<br>• Layer properties are parsed but unused (`FBXAnimation.cpp:L184`). | `assimp@v6.0.5 code/AssetLib/FBX/FBXConverter.cpp` [S][L] |
| Blender 5.2.3 default importer (C++ on ufbx, default since v5.0.0) | Source comment: "mixing is completely ignored for now, each layer results in an independent set of actions".<br>• One Action per (stack, layer), named `Stack\|Layer`.<br>• Each layer is evaluated alone through `flayer->anim`.<br>• Actions get a fake user.<br>• An Action is assigned only to an ID that has none, so the first layer plays.<br>• Cubic keys are forced to linear (L339). | `blender@v5.2.3 source/blender/io/fbx/importer/fbx_import_anim.cc:L519–L562, L602` [S] |
| Blender 5.2.3 "FBX (.fbx) (Legacy)" (Python) | "Recreate an action per stack/layer/object combinations. Only the first found action is linked to objects, more complex setups are not handled". Actions are named `obj\|stack\|layer` | `blender@v5.2.3 scripts/addons_core/io_scene_fbx/import_fbx.py:L1061–L1096` [S] |
| Godot 4.7.2 | One Animation per stack, from `ufbx_bake_anim(fbx_scene, fbx_anim_stack->anim, …)` at the import fps. Layers blend by ufbx's rules | `godot@4.7.2-stable modules/fbx/fbx_document.cpp:L1354–L1382` [S] |
| Unreal (legacy FBX importer, SDK 2020.2) | Has `FFbxImporter::MergeAllLayerAnimation(FbxAnimStack*, int32 ResampleRate)`: "Merge all layers of one AnimStack to one layer." When it is called is [UNVERIFIED] | UE 5.4 API, UnrealEd `FbxImporter.h` [S] |
| Maya (FBX plug-in) | Import option "Bake Animation Layers": "When … active, the plug-in collapses all layers into the base layer. If you disable [it], all the layers in the FBX file will be imported" | Maya 2026 help, "The FBX Import" (GUID-0CD41066…) [S] |
| Unity | Imports through the SDK; how it treats layers is not documented | [UNVERIFIED] |

**Measured [L].** File: `maya_anim_layer_anim_7700_ascii.fbx`, node pCube1, 24 fps. Values are translate.y / rotate.x in degrees. The file has a BaseLayer (T.y 0→−4) and AnimLayer1 (additive, Euler by channel, weight animated 0→100→0, T.y 0→4, R.x 45). The assimp row went through an assimp export to GLB, then three.js `GLTFLoader` and `AnimationMixer`.

| Reader | Frame 6 | Frame 12 |
|---|---|---|
| ufbx v0.23.1 test expectation (`test/test_animation.h:L2788, L2794`) | −1.0 / 22.5 | 0.0 / 45 |
| three.js r183 FBXLoader | −2.0 / 0, with the warning | −4.0 / 0 |
| assimp 6.0.5 | +2.0 / 45 (AnimLayer1's raw curves) | +4.0 / 45 |

**Public files with more than one layer.** All are in ufbx v0.23.1 `data/`, mostly Maya 2019–2020 SDK files with ASCII and binary twins:
- `maya_anim_layers_7500_{ascii,binary}.fbx`, `maya_anim_layers_acc_7500_*`, `maya_anim_layers_over_7500_*` and `maya_anim_layers_over_acc_7500_*`. Each has layers `X` and `Y`; the tests are at `test/test_animation.h:L1224–L1265`.
- `maya_anim_layers_over_acc_orders_7500_*`: six cubes, one per rotation order. `maya_anim_layers_over_acc_orders_18.obj` is Maya's OBJ export of frame 18.
- `maya_anim_layer_anim_7700_ascii.fbx` (animated weight) and `maya_anim_layer_quat_to_euler_7700_ascii.fbx`.
- `synthetic_rotation_order_layers_7700_ascii.fbx`: layers BaseLayer, Base and Rotation, tested with weight overrides at `test/test_parse.h:L1473`.

No stack in three.js r183 `examples/models/fbx` has more than one layer: Samba has two stacks with one layer each, and three.js raised no warning [L]. The 11 files in assimp v6.0.5 `test/models-nonbsd/FBX/2013_ASCII` have one layer each [L].

**Not established**
- How the SDK's `FbxAnimEvaluator` actually computes pure Override with a weight below 100, Mute, Solo and BlendModeBypass. These come from docs only; no SDK was run.
- What Unity and Unreal do with several layers by default.
- Maya's additive-scale convention: whether layer scale values are offsets from 1.
- Maya stores the export-time pose in the static `Lcl` values. three.js with zero tracks matched the frame-18 OBJ exactly (error 0.0000), so a frame-18 check cannot catch a reader that drops layers. Test at other frames.

## Q4. ASCII FBX output

**Decides the design**
- **Who reads ASCII 7.x.** Every oracle except Blender's Python importer. That importer was the only FBX importer through Blender 4.4 and the default in 4.5. Since 5.0.0 it is "FBX (.fbx) (Legacy)", and the C++ importer on ufbx is the default (commit acebf4b6d5, 2025-06-28: "make new FBX importer the default one, mark Python as legacy"). The 5.0 release notes say: "New C++ based FBX importer (added in 4.5 as experimental) is the default FBX importer now" (developer.blender.org/docs/release_notes/5.0/pipeline_io/).
- **Minimum versions differ:**
  - three.js: 7000 for ASCII, 6400 for binary.
  - assimp: 7100, and it warns above 7400.
  - Blender legacy: binary only, 7100 or later.
- **Reasons to write ASCII:**
  - Text that can be diffed and searched (Maya help: "lets you search the file for information retrieval").
  - No `FileId`, `CreationTime` record or footer, so no hashes to get wrong. SDK-written ASCII has none [L, two files]. In ufbx issue #229 the SDK refused a hand-written binary ("None of the registered readers can process the file") while "ASCII transmute works in FBX SDK".
  - litegen already writes the known-good triple (research-fbx §1.4), so this matters only if an SDK test fails.
- **Reasons not to:**
  - Blender up to 4.5's default importer, and the Legacy one, refuse ASCII.
  - It is bigger on mesh-heavy files.
  - Arrays can't be deflated, and embedded textures become base64 (+33%).
- **Recommendation.** Keep binary 7400 as the only output; offer ASCII at most as a debug switch. An ASCII writer must include `FBXHeaderExtension { FBXVersion: 7400 }`, because three.js reads the version only from that line.

| Consumer | ASCII 7.x | Source |
|---|---|---|
| Blender Python importer (≤4.4, 4.5 default, "Legacy" in 5.x) | **Refuses it**: `operator.report({'ERROR'}, rpt_("ASCII FBX files are not supported %r") …)`. A file counts as ASCII when its first 24 characters decode as UTF-8. Binary below 7100 is also refused (L3107) | `blender@v5.2.3 scripts/addons_core/io_scene_fbx/import_fbx.py:L3079–L3094` [S] |
| Blender ≥5.0 default (`wm.fbx_import`, ufbx) | Reads it: the file goes to `ufbx_load_stdio` with no format check. Aras's blog: "It supports ASCII FBX files, as well as binary FBX files that are older than FBX 7.1". In v4.5.0 the menu entry was "FBX (.fbx) (experimental)" | `blender@v5.2.3 source/blender/io/fbx/importer/fbx_import.cc:L345–L375`; `scripts/startup/bl_ui/space_topbar.py:L387–L388` [S] |
| three.js r183 FBXLoader | Reads it:<br>• Binary is detected by its magic bytes; anything else is parsed as ASCII by `TextParser`.<br>• `getFbxVersion(FBXText) < 7000` throws.<br>• Embedded base64 `Content` becomes a data URL (L363–L365, L3362–L3365).<br>• r183 ships the ASCII 7500 file `examples/models/fbx/stanford-bunny.fbx`.<br>[L]: ASCII 7100, 7400, 7500 and 7700 load, from both the SDK and assimp; 6100 throws "FBX version not supported, FileVersion: 6100". | `three.js r183 examples/jsm/loaders/FBXLoader.js:L147–L167, L4096, L4126–L4134, L3176` [S][L] |
| assimp 6.0.5 | Reads it: `"Kaydara FBX Binary"` goes to `TokenizeBinary`, anything else to `Tokenize`. Minimum 7100; above 7400 it warns and tries. It also writes ASCII (`-ffbxa`, "; FBX 7.5.0 project file"). [L]: ASCII 7100 to 7700 load; 6100 fails with "unsupported, old format version" | `assimp@v6.0.5 code/AssetLib/FBX/FBXImporter.cpp:L153–L158`, `FBXDocument.cpp:L284–L312` [S][L] |
| Godot 4.7.2 | Reads it.<br>• The default importer is ufbx: the `fbx/importer` enum is "ufbx,FBX2glTF" with default `FBX_IMPORTER_UFBX`.<br>• It loads through `ufbx_load_stream`.<br>• ufbx parses ASCII (`ufbxi_ascii_parse_node`, ufbx.c:L10280; `metadata.ascii`, ufbx.h:L3761).<br>• The optional FBX2glTF path uses the SDK. | `godot@4.7.2-stable modules/fbx/editor/editor_scene_importer_ufbx.cpp:L99`, `fbx_document.cpp:L2069` [S] |
| Maya, 3ds Max (FBX plug-in, SDK) | Read and write it: "Save your FBX file as one of these two types, Binary or ASCII." The ufbx corpus holds ASCII files from 7100 to 7700 written by Maya and by 3ds Max | Maya 2016 help, "Advanced options" [S] |
| Unreal 5.x | Reads it through the SDK; not tested. UE 5.8 docs: "The Unreal Engine FBX import pipeline uses FBX 2020.2". Interchange in 5.7 also has `use_ufbx_parser` | Epic docs [S] |
| Unity | Imports through the SDK, according to the reporter of ufbx #229. Unity's FBX Exporter can write ASCII (`ExportFormat.ASCII`). Not tested | [UNVERIFIED] |

**Sizes [L].** ASCII divided by binary.
- **SDK-written pairs of the same scene** (195 pairs in the ufbx corpus, FBX 5800–7700):
  - The median is 0.65, because small scenes are dominated by property records.
  - Mesh-heavy scenes go the other way: `maya_slime_7500` is 1.82× (1.09 MB against 0.60 MB) and `motionbuilder_thumbnail_7700` is 2.02×.
- **assimp 6.0.5 output.** Its binary doesn't deflate arrays.

  | Model | ASCII | Binary | Ratio |
  |---|---|---|---|
  | stanford-bunny | 7.00 MB | 5.41 MB | 1.29× |
  | Samba | 14.9 MB | 11.6 MB | 1.28× |
  | monkey_embedded_texture | 518 KB | 416 KB | 1.25× |

  - gzip -9 nearly evens them out: 1.19 MB against 1.15 MB for the bunny, 2.32 MB against 2.18 MB for Samba.
  - The SDK's compressed binary of Samba is 3.68 MB, about a quarter of assimp's ASCII.

**Not established**
- Unity reading ASCII: not tested, and no Unity doc says so.
- Whether litegen's binary FBX loads in an SDK tool (Maya, Unity, Unreal). If it doesn't, the #229 header hashes are the first suspect.
- Parse speed of ASCII against binary: not measured.

## Q5. COLLADA morph controllers

**Decides the design**
- Of the oracles, only SceneKit reads `<controller><morph>`. It handles NORMALIZED and RELATIVE morphs, skin over morph, and the weight animation [L]. Both other oracles fail the whole file: three.js r183 (and r187 still does), and assimp 6.0.5, the latest release. Keep baking morphs on DAE output, as litegen does today.
- assimp's `<targets>` loop bug is in every release through v6.0.5 and on master at de6d25f (2026-09-08), and no open fix was found. A second bug means assimp can never detect `method="RELATIVE"`. So if a morph DAE is ever offered (a SceneKit or Godot profile), write NORMALIZED morphs with absolute targets. That is the spec default, and it is the only method an assimp without the loop bug would blend correctly.
- Blender 5.0 removed Collada entirely. Blender up to 4.5 imported morphs as shape keys, and Godot 4.7 imports them as blend shapes.
- Unity reads DAE through its FBX-SDK import chain, and Autodesk documents that SDK's COLLADA reader as reading morphs. This was not tested.

| Consumer | What it does with `<controller><morph>` | Source |
|---|---|---|
| SceneKit (macOS 26.6.2) | Builds an `SCNMorpher` with the target, with `calculationMode` normalized for NORMALIZED and additive for RELATIVE. On skin over morph it keeps both the morpher and the skinner, and it lists the weight animation (`Strip-morph-anim`). Apple: "A morpher and its target geometries may be loaded from a scene file". Load the DAE from `Data` with `SCNSceneSource(data:)`. From a file URL, SceneKit imports in an XPC service, and that fails under a sandbox. | [L] `probe.swift` on `litegen-core/src/mesh/testdata/dae/research/morph.dae` and `morph_relative_full.dae`; developer.apple.com/documentation/scenekit/scnmorpher [S]; `scripts/mesh-verify/scenekit/render.swift:L113-127` |
| assimp 6.0.5 | Fails the import: "Unable to resolve library reference """.<br>• The `<targets>` loop sets `currentChildNode = node.first_child()` (the `<morph>` element) once and advances the outer iterator instead, so MORPH_TARGET and MORPH_WEIGHT are never stored.<br>• `method` is read with `as_int()`, which is 0 for "RELATIVE", so RELATIVE is never set.<br>• Morph weight channels count only when the target id contains "morph-weights". | `assimp@v6.0.5 code/AssetLib/Collada/ColladaParser.cpp:L902-915, L868-876`; `ColladaLoader.cpp:L1228, L1397` [S]; [L] |
| assimp master | The same loop, at L903-916, in de6d25f (2026-09-08, the last commit to the file). Issue and PR searches found only closed #2066 and #2641. | [S] |
| three.js r183 ColladaLoader | Warns "Morph target animation not supported yet." and stores a controller with no `skin`. Then:<br>• a morph-only `<instance_controller>` throws at `controller.skin.joints`;<br>• skin over morph throws at `geometry.sources`, because the skin's source is the morph controller, not a geometry;<br>• a weight channel target with no `/` (`X-weights-array(0)`) throws at `sid.indexOf`.<br>Each fails the whole load. r187 is unchanged. | `three.js r183 examples/jsm/loaders/collada/ColladaParser.js:L445-448, L354`; `collada/ColladaComposer.js:L2353, L961-969`; r187 `ColladaParser.js:L447-449`, `ColladaComposer.js:L2453` [S]; failures measured in research-usd-3mf-dae.md C2 [L] |
| Blender 5.0 and later | No Collada at all: "Collada (.dae) import & export has been removed. (PR#139337)" | developer.blender.org/docs/release_notes/5.0/pipeline_io/ [S] |
| Blender 4.5 and earlier | The OpenCOLLADA importer made each morph target a shape key, with the authored weight | `blender@v4.5.15 source/blender/io/collada/ArmatureImporter.cpp:L921-983` [S] |
| Godot 4.7.2 | Parses `<morph>` (source, method, MORPH_TARGET and MORPH_WEIGHT) and adds one blend shape per target, in RELATIVE or NORMALIZED mode. Its docs call DAE "an older format that is supported" and recommend glTF "for complex scenes or scenes that contain animations". | `godot@4.7.2-stable editor/import/3d/collada.cpp:L1226-1292`, `editor_import_collada.cpp:L472-489`; docs.godotengine.org/en/stable/tutorials/assets_pipeline/importing_3d_scenes/available_formats.html [S] |
| Unity 6 | Lists `.dae` among the formats it reads, and "uses the .fbx file format as its internal importing chain". Autodesk's notes on the SDK's COLLADA reader mark Morph as supported ("corresponds to FBX Shape Deformation"), along with skin, lines and linestrips. Not tested in Unity. | docs.unity3d.com/6000.2/Documentation/Manual/3D-formats.html; download.autodesk.com/us/fbx/2013/FBXconverter/files/GUID-B6630F5E-A4C7-4D9D-BFD9-4A8CD5BC5A63.htm [S]; [UNVERIFIED] |
| SketchUp | Its help documents COLLADA 1.4.1 import with two options, "Validate COLLADA File" and "Merge Coplanar Faces". SketchUp has no skin or blend-shape model, so at most the base mesh can arrive. | help.sketchup.com/en/sketchup/importing-and-exporting-collada-files [S]; morph result [UNVERIFIED] |

**Not established**
- What Unity 6 and SketchUp actually do with a morph controller; neither is installed here.
- Whether Godot handles skin over morph; only its morph path was read.

## Q6. Points and lines

**Decides the design**
- **Which containers hold them.** glTF, OBJ, PLY, USD, COLLADA (lines only) and FBX can. STL cannot. 3MF can only through the Beam Lattice extension, whose beams are solids with a radius rather than lines. The first table cites each.
- **glTF is the safe carrier.** It is the one container whose points and lines every oracle that reads it builds as points and lines (three.js and assimp). glTF says "Points and Lines SHOULD have widths of 1px", and three.js draws them unlit at 1 px.
- **Writer rules, so that three.js reads what was written:**
  - **OBJ.** Write each segment as its own `l a b`. OBJLoader pushes a polyline's vertices straight into `LineSegments`, so `l 1 2 3 4` draws 1–2 and 3–4 and loses 2–3 [L].
    - Put points and lines in their own `o`. One `l` in an object turns the whole object, faces included, into `LineSegments` [L].
    - Give them their own `usemtl`. Without an MTL, OBJLoader caches materials by name and the first object's material type wins: a triangle after a `p` object got a `PointsMaterial` [L].
  - **COLLADA.** Use `<lines>`, not `<linestrips>`. three.js keeps only the last `<p>` of a `<linestrips>`, and SceneKit builds a geometry with no elements from one [L]. assimp reads both [L].
  - **USD.** Points and BasisCurves reach only Storm and SceneKit:
    - three.js and RealityKit drop both.
    - SceneKit drops Points, and draws BasisCurves as straight segments through the control points [L].
    - Quick Look renders with RealityKit, so a USDZ of points or curves shows nothing there.
  - **FBX.** three.js reads only `NurbsCurve` (as a fixed-colour `Line`), and assimp reads only `Line` geometry. Neither reads both.
  - **PLY.** A vertex-only file is a point cloud to SceneKit and assimp. three.js returns an unindexed geometry and leaves Mesh or Points to the app. Every oracle tested ignores the `edge` element.
- **Apple's feature table disagrees with what was measured here.** It marks Points ✔ and basis curves ✗ for both SceneKit and Storm. On macOS 26.6.2 with USD Tools 0.25.2, SceneKit dropped Points and read the curves, and usdrecord's Storm rendered both. Apple: "In the case of Storm, feature support may differ from the open source version."

**Which formats hold points and lines**

| Format | Points | Lines | Source |
|---|---|---|---|
| glTF 2.0 | mode 0 POINTS | modes 1 LINES, 2 LINE_LOOP, 3 LINE_STRIP. "SHOULD have widths of 1px"; with no NORMAL they are unlit (base colour plus emissive) | `KhronosGroup/glTF@9d23a5a specification/2.0/Specification.adoc:L1752-1790, L2942-2945`; `schema/mesh.primitive.schema.json:L26-61` [S] |
| OBJ | `p v1 v2 v3 …` | `l v1/vt1 v2/vt2 …`, a polyline; also free-form `curv` | paulbourke.net/dataformats/obj/ (Wavefront spec) [S] |
| PLY | a file with no `face` element | `element edge` with `vertex1` and `vertex2`: "two pointers to the vertices that delineate the edge" | paulbourke.net/dataformats/ply/ (Turk) [S] |
| USD | `Points` (points, widths, ids) | `BasisCurves` (type linear or cubic; basis bezier, bspline or catmullRom; wrap) and `NurbsCurves` | `OpenUSD@v26.11.1 pxr/usd/usdGeom/schema.usda:L1877, L1594-1812, L1821` [S] |
| COLLADA 1.4.1 | none: `<mesh>` holds only "`<lines>`, `<linestrips>`, `<polygons>`, `<polylist>`, `<triangles>`, `<trifans>`, and `<tristrips>`" | `<lines>` (pairs) and `<linestrips>` (one `<p>` per strip) | COLLADA 1.4.1 spec, pp. 5-72, 5-74, 5-80 [S] |
| FBX | no point-cloud class; ufbx counts one-vertex mesh faces (`num_point_faces`) | `Geometry: …, "Line"` (FbxLine: `Points` plus `PointsIndex`, where a negative index ends a segment), `NurbsCurve`, and two-vertex mesh faces (`num_line_faces`) | `assimp@v6.0.5 code/AssetLib/FBX/FBXMeshGeometry.cpp:L716-728`, `FBXConverter.cpp:L1030-1068`; `ufbx@v0.23.1 ufbx.h:L1131-1132, L1668-1690`; help.autodesk.com/cloudhelp/2020/ENU/FBX-API-Reference/cpp_ref/class_fbx_line.html [S] |
| STL | none | none (facets only) | — |
| 3MF | none in Core | Beam Lattice 1.2.0: `<beamlattice radius>` in `<mesh>`, holding `<beam v1 v2 r1 r2 cap1 cap2>` and `<ball>`. These are solids made of cylinders, cones and spheres, and "the beamlattice extension MUST always be a required extension". three.js 3MFLoader has no beam support. | `3MFConsortium/spec_beamlattice@1.2.0 "3MF Beam Lattice Extension.md":L94-168, L220-243` [S] |

**What each consumer builds**

| Consumer | What it builds | Source |
|---|---|---|
| three.js r183 GLTFLoader | Modes 0–3 become `Points` (`PointsMaterial` with `sizeAttenuation = false`, so 1 px), `LineSegments`, `LineLoop` and `Line` (`LineBasicMaterial`, unlit). Modes 5 and 6 become triangles, and any other mode throws. Skinning applies only to triangle modes. | `examples/jsm/loaders/GLTFLoader.js:L3834-3880, L3442-3478` [S]; [L] `modes.gltf` |
| three.js r183 OBJLoader | `p` becomes `Points` (1 px, or 10 px when an MTL material is converted). `l` becomes `LineSegments` with the polyline's vertices in order. A file with only `v` lines becomes `Points`. The geometry type is per object, and the material cache key leaves it out. | `OBJLoader.js:L389-425, L655-686, L780-781, L812-860, L885-907, L925-943` [S]; [L] `pl.obj`, `mixed.obj`, `vonly.obj` |
| three.js r183 PLYLoader | Returns a `BufferGeometry`, never a Mesh or Points, and its documented use is `new THREE.Mesh( geometry )`. A vertex-only file gives positions with no index. `edge` and other elements are parsed and ignored. Faces with more than 4 corners are dropped without a warning. | `PLYLoader.js:L20-23, L480-561` [S]; [L] `pts.ply`, `edges.ply` |
| three.js r183 FBXLoader | `NurbsCurve` becomes a `Line` tessellated at 12 points per control point, with a fixed `LineBasicMaterial` 0x3300ff. `Line` geometry is not parsed, and a model of attribute type `Line` becomes an empty `Group`. | `FBXLoader.js:L975-997, L1375-1391, L1612-1621, L2449-2494` [S] |
| three.js r183 ColladaLoader | `<lines>` becomes `LineSegments` and `<linestrips>` becomes `Line`, with Phong or Lambert swapped for `LineBasicMaterial`. `parseGeometryPrimitive` overwrites `primitive.p` at every `<p>`, so only the last strip survives, and strip groups count `count × 2` vertices. `<polygons>` warns "Unsupported primitive type". | `collada/ColladaParser.js:L1233-1241, L1349`; `collada/ColladaComposer.js:L1618-1622, L2560-2627` [S] (not run: the loader needs a DOM) |
| three.js r183 USDLoader | Points, BasisCurves and every other non-Mesh gprim become an empty `Object3D`. | `usd/USDComposer.js:L621-676` [S]; [L] `pc.usda` |
| SceneKit (macOS 26.6.2) | `SCNGeometryPrimitiveType` has `.point` and `.line`.<br>• DAE `<lines>` gives a line element. `<linestrips>` gives a geometry with no elements, so nothing draws.<br>• USD BasisCurves give line elements through the control points, and a cubic is not evaluated. USD Points give no node at all.<br>• A vertex-only PLY gives a point element, and PLY edges are ignored.<br>• OBJ `p` and `l` are dropped, and a vertex-only OBJ logs "OBJ file has no faces". | [L] `probe.swift`; developer.apple.com/documentation/scenekit/scngeometryprimitivetype [S] |
| Model I/O (macOS 26.6.2) | `MDLGeometryType` has points and lines. OBJ, PLY and USD give the same results as SceneKit. It cannot import dae, gltf, glb, fbx or 3mf (`canImportFileExtension`). | [L] |
| usdrecord / Storm (Apple USD Tools 0.25.2, renderer "Metal") | Points render as squares sized by `widths`. At the default `--complexity low`, BasisCurves render as 1 px wires, with a cubic drawn as its control polygon. At `--complexity veryhigh` they render as ribbons of the authored width, with the cubic evaluated. | [L] renders `pc.png`, `pc_vh.png` |
| usdchecker --arkit 0.25.2 | Points and BasisCurves pass without a flag. | [L] |
| RealityKit | Apple's table marks Points ✗ and basis curves ✗; only Raytracer has partial, catmull-rom-only curve support. `MeshDescriptor.Primitives` has only `triangles`, `polygons` and `trianglesAndQuads`. | developer.apple.com/documentation/usd/validating-usd-files; developer.apple.com/documentation/realitykit/meshdescriptor/primitives-swift.enum [S] |
| assimp 6.0.5 | Uses `aiPrimitiveType_POINT` and `_LINE`:<br>• glTF modes 0–3 give points or lines; a 4-vertex loop gives 4 segments and a strip 3.<br>• In OBJ, `p` gives points, `l 1 2 3 4` gives 3 segments, and a mixed object is split by type.<br>• In COLLADA it reads both `<lines>` and `<linestrips>` (2 strips gave 4 segments).<br>• In FBX, `Line` geometry gives lines. `NurbsCurve` is not read: only Mesh, Shape and Line geometry classes are.<br>• Vertex-only PLY and OBJ files give point meshes with zero faces, which its own `ValidateDataStructure` rejects ("Mesh contains no faces"). That step is on in `assimp info`'s default preset; `-fd` replaces the preset.<br>• PLY edges are ignored. | [L] `assimp info`; `FBX/FBXDocument.cpp:L128-138`, `FBXConverter.cpp:L1030-1068`, `Ply/PlyLoader.cpp:L281-287`, `Obj/ObjFileImporter.cpp:L213-225`, `PostProcessing/ValidateDataStructure.cpp:L342-343` @v6.0.5 [S] |

**Not established**
- Whether RealityKit draws points or lines built in code: `LowLevelMesh` takes a Metal primitive type, but nothing here loads that from a file [UNVERIFIED].
- three.js ColladaLoader's `<linestrips>` behaviour comes from reading its source. It was not run, because the loader needs a DOM.

## Q7. UV sets beyond two

**Decides the design**
- **No tool here rejects or warns about a third UV set:**
  - `usdchecker --arkit` 0.25.2 passes a mesh with ten texCoord primvars [L].
  - SceneKit and Model I/O read all ten, from USD and from DAE [L].
  - assimp keeps 8 and drops the rest silently, except in FBX, where it warns [L][S].
- **What breaks is a texture bound to a set beyond the first two:**
  - three.js USDLoader builds only `uv` and `uv1`, so a texture reading `st2` gets channel 2 with no `uv2` attribute.
  - three.js ColladaLoader reads TEXCOORD sets 0 and 1 only.
  - ColladaLoader and FBXLoader sample every texture from set 0.
  - Apple: "RealityKit supports two UV Sets."
- **Rules that follow:**
  - Carry sets 2 and up as data.
  - Keep textures on sets 0 and 1 unless the target is Storm, SceneKit or Blender.
  - In USD, name the sets `st` and `st1`, and author `st` first. three.js takes the first texCoord-typed primvar as `uv`, and it finds `st1` only by that name.
  - In COLLADA, put TEXCOORD inputs on the primitive, not in `<vertices>`. Inside `<vertices>`, three.js gives set 1 an item size of 0.
- **The limits:**
  - three.js reads 4 sets (`uv` to `uv3`).
  - assimp reads 8 (`AI_MAX_NUMBER_OF_TEXTURECOORDS`).
  - Blender reads 8 (`MAX_MTFACE`), and its glTF importer stops at 8.
  - RealityKit reads 2.
  - SceneKit, Model I/O, Storm and usdchecker all handled 10, the most tried.

| Consumer | UV sets it reads, and what a third set does | Source |
|---|---|---|
| three.js r183 core | Built-in materials take `uv`, `uv1`, `uv2` and `uv3`, chosen by `texture.channel` 0–3. Channel n becomes `#define MAP_UV uvn`, and only `uv1` to `uv3` are ever declared. | `src/renderers/webgl/WebGLProgram.js:L532, L620-645`; `WebGLPrograms.js:L40-48, L262, L373-375` [S] |
| three.js r183 GLTFLoader | `TEXCOORD_0` to `_3` become `uv` to `uv3`, and `texCoord` n sets `texture.channel` = n. `TEXCOORD_4` and up load as `texcoord_4` and so on, which no material reads. A texture with `texCoord` 4 would need `uv4`, which the shader never declares [inference: the program fails to compile]. | `GLTFLoader.js:L2217-2228, L3389-3392, L4787-4794` [S]; [L] `modes.gltf` |
| three.js r183 USDLoader | `uv` is the first primvar whose type contains `texCoord`, in field order, or else `st` or `UVMap`. `uv1` is `primvars:st1`, found by name only, and no `uv2` is ever built. A texture whose primvar reader has varname `st1` gets channel 1, `st2` gets channel 2, and any other name gets channel 0. So a map on `st2` samples a missing attribute [inference: it shows one texel's colour]. | `usd/USDComposer.js:L1752-1784, L1371-1398, L3125-3201` [S]; [L] `uv3.usda` gave `position,normal,uv,uv1` |
| three.js r183 ColladaLoader | A TEXCOORD input with `set` 0, or no set, becomes `uv`, and set 1 becomes `uv1`. Sets 2 and up are ignored: silently on the primitive, and with a "Semantic … not handled" warning inside `<vertices>`. Inside `<vertices>`, set 1 also writes `uv.stride` instead of `uv1.stride`, which leaves `uv1` with item size 0. Textures never set `channel`, so they sample set 0. | `collada/ColladaParser.js:L1338`; `collada/ColladaComposer.js:L1726-1737, L1755-1763, L1776-1777` [S] |
| three.js r183 FBXLoader | Every `LayerElementUV` becomes `uv`, `uv1`, `uv2` and so on. Textures never set `channel`, and the `UVSet` property is not read. | `FBXLoader.js:L1721-1725, L1803-1816` [S] |
| three.js r183 OBJLoader, PLYLoader | One set each: `vt`, and `s`/`t` or `u`/`v`. | `OBJLoader.js` (`vt`); `PLYLoader.js:L362, L492-494` [S] |
| SceneKit (macOS 26.6.2) | Read 10 texcoord sources from USD and 10 from DAE. A USD texture read through `st9` got `mappingChannel` 9. Apple: "A geometry can have multiple independent sources of texture coordinates, each of which defines a unique mapping channel number". | [L] `uv10.usda`, `uv10.dae`; developer.apple.com/documentation/scenekit/scnmaterialproperty/mappingchannel [S] |
| Model I/O | Read ten attributes from USD, `textureCoordinate` through `textureCoordinate9`. | [L] |
| RealityKit | "RealityKit supports two UV Sets.", and the feature table rates multiple UV sets as partial. Which two primvars it takes is not documented. | developer.apple.com/documentation/usd/validating-usd-files [S] |
| usdrecord / Storm 0.25.2 | Samples whatever primvar the `UsdPrimvarReader_float2` names. A texture read through `st2`, and in another file `st9`, rendered from that set. | [L] `uv3.png`, `uv10.png` |
| usdchecker --arkit 0.25.2 | `Success!` with `st` through `st9`. | [L] |
| assimp 6.0.5 | `AI_MAX_NUMBER_OF_TEXTURECOORDS` is 8. glTF and Collada stop at 8 silently (a 10-set DAE kept 8). The FBX reader warns "ignoring UV layer, maximum number of UV channels exceeded". | `include/assimp/mesh.h:L108-113`; `glTF2/glTF2Importer.cpp:L627`; `Collada/ColladaParser.cpp:L2038`; `FBX/FBXMeshGeometry.cpp:L316-318` [S]; [L] `assimp dump` |
| Blender 5.2.3 | `MAX_MTFACE` is 8. The glTF importer reads `TEXCOORD_n` while n < `UV_MAX` (8). The USD importer reads every float2 primvar with vertex, faceVarying or varying interpolation as a UV map, and makes `st` the active one when present. | `source/blender/makesdna/DNA_customdata_types.h:L172`; `scripts/addons_core/io_scene_gltf2/blender/imp/mesh.py:L34-35, L208`; `source/blender/io/usd/intern/usd_reader_mesh.cc:L816-831` [S] |

**Not established**
- Which two USD primvars RealityKit picks, and what it does with a texture bound to a third.
- The two three.js failures marked "inference" (the missing `uv2` and the undeclared `uv4`) come from reading the source; they were not rendered.
- The glTF 2.0.1 spec sets no minimum number of sets that a client must support. The "at least two" note some remember was not found in it.

## Q8. glTF cameras, KHR_lights_punctual and skin.skeleton

**Decides the design**
- **No consumer needs `skin.skeleton`.**
  - three.js r183 never reads it.
  - Blender, Babylon and glTFast fall back to the joints' lowest common ancestor.
  - A value that is not a common root is a glTF-Validator **error** (`SKIN_SKELETON_INVALID`). Omit it, or write the joints' closest common root.
- **FOV is a ratio:** yfov = 2·atan(h/(2f)), with h and f in the same unit.
  - USD: "tenths of a scene unit".
  - FBX: FilmHeight in inches × 25.4, against FocalLength in mm.
  - COLLADA is in degrees and glTF in radians.
- **Axes.**
  - glTF, USD and COLLADA cameras and lights look down local −Z.
  - FBX cameras look down +X; FBX lights point down −Y.
  - Blender applies R_y(+90°) to cameras and R_x(+90°) to lights. three.js FBXLoader applies neither.
- **Light units differ in every format:**
  - glTF: candela (point/spot) and lux (directional).
  - UsdLux: intensity is luminance in nits. A normalised DistantLight's intensity is in lux.
  - FBX: Intensity is 100 × a unitless DCC value.
  - COLLADA has no intensity.
  - Blender converts with 683 lm/W for glTF and W/π for USD.
- **Readers.** RealityKit ignores USD cameras and lights, and three.js USDLoader drops both. three.js FBXLoader and ColladaLoader read both, with losses.

| Consumer | What it does | Source |
|---|---|---|
| glTF 2.0 spec | skeleton "MUST be the root of all nodes listed in the `joints` array or their common ancestor"; "not needed for computing skinning transforms" | `KhronosGroup/glTF@9d23a5a specification/2.0/Specification.adoc:L2132-L2138` [S] |
| glTF-Validator 2.0.0-dev.3.10 | `SKIN_SKELETON_INVALID` "Skeleton node is not a common root." (severity Error) | `glTF-Validator@2.0.0-dev.3.10 lib/src/base/skin.dart:L130-L136`, `lib/src/errors.dart:L463-L464, L557-L558` [S] |
| three.js r183 GLTFLoader (skin) | `loadSkin` reads only `joints` and `inverseBindMatrices`, and binds with an identity bind matrix | `three.js r183 examples/jsm/loaders/GLTFLoader.js:L3981-L4040, L4226` [S] |
| Blender 5.2.3 glTF importer (skin) | Adds `skeleton` to the joint set when choosing the armature (deepest common ancestor). "Guess original bind pose" assumes the inverse bind matrices are relative to `skeleton`. | `blender@v5.2.3 scripts/addons_core/io_scene_gltf2/blender/imp/vnode.py:L266-L298, L500-L507` [S] |
| Babylon.js 9.33.2 (skin) | When absent, computes the nearest common ancestor. When it is not a common root, warns. Builds bones up to `skeleton` and parents the skinned mesh as a sibling of the skeleton root. | `Babylon.js@9.33.2 packages/dev/loaders/src/glTF/2.0/glTFLoader.pure.ts:L1567-L1594, L1645-L1651, L1046-L1056` [S] |
| glTFast v6.0.0-pre.2 (skin) | When absent, uses `GetLowestCommonAncestorNode`; the result becomes `SkinnedMeshRenderer.rootBone` | `com.unity.cloud.gltfast@v6.0.0-pre.2 Runtime/Scripts/GltfImport.cs:L2036-L2058, L2414-L2417`; `GameObjectInstantiator.cs:L263-L266` [S] |
| three.js r183 GLTFLoader (camera/light) | Camera: `PerspectiveCamera(deg(yfov), aspectRatio‖1, znear‖1, zfar‖2e6)`; ortho ±xmag/±ymag. Light: linear colour; intensity as is (three.js lights are in cd and lux); range → distance; spot angle = outer, penumbra = 1−inner/outer; decay 2 | `GLTFLoader.js:L3958-L3962, L706-L747`; `src/lights/PointLight.js:L25-L29` [S] |
| Blender 5.2.3 glTF IO | Camera: angle_y = yfov, vertical sensor fit; ortho_scale = 2·max(xmag,ymag). Lights in SPEC mode: W = 4π·cd/683 and sun W/m² = lux/683. spot_size = 2·outer, blend = 1−inner/outer. Range is not read ("TODO range"). | `.../io_scene_gltf2/blender/imp/camera.py:L30-L67`; `imp/light.py:L35, L58-L77, L137-L143`; `com/conversion.py:L10`; `exp/lights.py:L158-L178`; `__init__.py:L198-L207` [S] |
| Babylon.js 9.33.2 | fov = yfov; ortho ±xmag/±ymag. Spot angle = 2·outer, innerAngle = 2·inner; `FALLOFF_GLTF`; intensity as is; range read | `glTFLoader.pure.ts:L1713-L1756`; `Extensions/KHR_lights_punctual.pure.ts:L76-L100` [S] |
| glTFast v6.0.0-pre.2 | fieldOfView = deg(yfov). orthographicSize = ymag (xmag ignored). Camera child rotated 180° about Y. spotAngle = 2·deg(outer). Intensity divided by π in Built-in, as is in URP, Lux/Candela in HDRP | `GameObjectInstantiator.cs:L392, L420, L461-L462`; `LightPunctualExtension.cs:L51-L59, L103-L131` [S] |
| three.js r183 USDLoader | Camera and light prims fall into the generic branch and become an empty `Object3D` | `examples/jsm/loaders/usd/USDComposer.js:L621-L676` [S] |
| three.js r183 FBXLoader | Camera: FieldOfView, then `setFocalLength` with three.js's 35 mm film gauge, so yfov = 2·atan((35/max(aspect,1))/(2f)) and FilmWidth/FilmHeight are ignored. Near/FarPlane ÷1000. Orthographic unsupported. No +X axis fix. Light: Intensity/100; decay 1, not inverse square; spot angle = rad(InnerAngle); penumbra = max(rad(OuterAngle),1) | `FBXLoader.js:L1088-L1138, L1200, L1225, L1237-L1257`; `src/cameras/PerspectiveCamera.js:L121, L164-L170, L220-L224` [S] |
| three.js r183 ColladaLoader | Camera: yfov in degrees passed straight through; an xfov-only camera gets the default fov; ortho xmag/ymag halved. Light: colour converted sRGB→linear; no intensity; distance = √(1/quadratic_attenuation); angle = rad(falloff_angle) | `collada/ColladaParser.js:L1080-L1100, L1163-L1195`; `collada/ColladaComposer.js:L1410-L1446, L1466-L1494` [S] |
| ufbx v0.23.1 (Godot, and Blender 5.2's default FBX importer) | intensity = Intensity/100 ("FBX seems to store intensities 100x"). Reads InnerAngle or HotSpot, and OuterAngle or ConeAngle. Focal-length FOV: tan(fov/2) = aperture_in/(f_mm·0.03937)/2. "FBX saves cameras with +X forward and +Y up"; lights point "usually -Y" | `ufbx@v0.23.1 ufbx.c:L23047-L23065, L23240-L23245`; `ufbx.h:L1433-L1440, L1617-L1628` [S] |
| Blender 5.2.3 FBX IO | Export: Intensity = W·100·2^exposure, quadratic DecayType, OuterAngle = deg(spot_size) (the full cone), InnerAngle = deg(spot_size·(1−blend)). Camera: FocalLength in mm, film size in inches, ApertureMode 3. Axis-correction matrices for lights and cameras. Import is the inverse. | `scripts/addons_core/io_scene_fbx/export_fbx_bin.py:L605-L626, L655-L705`; `fbx_utils.py:L73-L74`; `import_fbx.py:L2196-L2251` [S] |
| assimp 6.0.5 | FBX: Intensity/100 multiplied into the colour; angles converted degrees→radians. COLLADA: falloff_angle becomes the inner cone; the outer cone is guessed from falloff_exponent | `assimp@v6.0.5 code/AssetLib/FBX/FBXConverter.cpp:L401-L428`; `code/AssetLib/Collada/ColladaLoader.cpp:L357-L375` [S] |
| Apple renderers | Camera: RealityKit ✗; Storm, SceneKit and Raytracer ✔. Distant and sphere lights: Storm and SceneKit ✔; RealityKit ✗ ("doesn't use any lights included in a USD file"). Light shaping: SceneKit ✔, Storm ✗ | developer.apple.com/documentation/usd/validating-usd-files [S] |

**How each construct maps**

| glTF | USD (OpenUSD v26.11.1) | FBX | COLLADA 1.4.1 |
|---|---|---|---|
| perspective: yfov (radians), aspectRatio, znear, zfar (absent = infinite) | `Camera` with `projection="perspective"`. yfov = 2·atan(verticalAperture/(2·focalLength)); aspect = h/v; clippingRange in scene units. Defaults 50, 20.955, 15.2908. The camera is "always Y up… looking down the -Z axis". (`usdGeom/schema.usda:L2279-L2304, L2352-L2373`; `gf/camera.cpp:L286-L295`) | NodeAttribute "Camera": FocalLength (mm), FilmWidth/FilmHeight (inches, default 0.816/0.612), ApertureMode (default eVertical), FieldOfView (degrees, default 40). yfov = 2·atan(25.4·FilmHeight/(2·FocalLength)). (a copy of the FBX SDK `fbxcamera.h` header) | `<perspective>`: yfov/xfov in degrees; aspect_ratio ("aspect_ratio = xfov / yfov"); znear/zfar in the `<unit>` (spec pp. 5-92 to 5-93) |
| orthographic: xmag/ymag are half the width and height (`Specification.adoc:L3057`) | `projection="orthographic"`; full width = aperture × 0.1, so horizontalAperture = 20·xmag (`gf/camera.cpp:L26-L30, L141-L161`) | CameraProjectionType 1, OrthoZoom | `<orthographic>` xmag/ymag. The spec contradicts itself: "[-xmag,xmag]" but also "width of xmag/2" (p. 5-90) |
| directional (lux) | `DistantLight` along −Z. Intensity = lux with normalize=1 ("intensity… illuminance, expressed in lux"); at angle 0 normalize makes no difference. Defaults: angle 0.53, intensity 50000 (`usdLux/schema.usda:L245-L340, L1085-L1123`) | LightType 1 | `<directional>`, direction [0,0,−1] (p. 5-30) |
| point (cd), range | `SphereLight` with treatAsPoint. My derivation, not tested: intensity = 4·cd normalised, or cd/(πr²) unnormalised (`schema.usda:L55-L73, L195-L215, L1196-L1227`) | LightType 0, DecayType 2 (quadratic) | `<point>` with constant, linear and quadratic attenuation |
| spot: inner/outer, radians from the axis (defaults 0 and π/4) | `ShapingAPI`: shaping:cone:angle = deg(outer) (measured "off the primary axis"); shaping:cone:softness = 1−inner/outer (`schema.usda:L735-L797`) | LightType 2. Blender writes OuterAngle = 2·deg(outer) and InnerAngle = 2·deg(inner). The SDK says only "(in degrees)", not whether it is the full or half cone | `<spot>`: falloff_angle in degrees (default 180) and falloff_exponent. The spec doesn't say half or full cone (p. 5-119) |

**Units**
- **Blender ↔ glTF uses 683 lm/W:** W = 4π·cd/683 for point and spot lights, and sun W/m² = lux/683.
- **Blender ↔ USD is not photometric:**
  - Intensity = W/π for point, spot and area lights.
  - The sun gets W/4 ("Unclear why, but approximately matches Karma").
  - It writes normalize=1, cone angle = spot_size/2, and DistantLight angle = sun_angle/2.
  - So a 1 cd glTF point light becomes 0.0184 W in Blender and an intensity of 0.00586 in USD.
  - Sources: `source/blender/io/usd/intern/usd_writer_light.cc:L90-L141, L171-L172`; the reverse is in `usd_reader_light.cc:L102-L146`.
- **USD camera units.** Lens and film values are in "tenths of a scene unit". With metersPerUnit=1, a 50 mm lens is focalLength=0.5. Blender writes the lens divided by 100·metersPerUnit (`usd_writer_camera.cc:L61-L94`). FOV is a ratio, so it survives wrong units.
- **FBX camera writer.** Use ApertureMode 3, FilmWidth = 35/25.4 inches, FilmHeight = FilmWidth/aspect, and AspectWidth:AspectHeight = aspect. Then three.js (35 mm film gauge) and ufbx compute the same yfov. I derived this from the cited code; I didn't test it.

**Not established**
- I had no device to test RealityKit or Quick Look with a UsdGeomCamera. Apple's table marks cameras as unsupported in RealityKit.
- I didn't check the SphereLight and DistantLight intensity formulas with a Storm render.
- I don't know whether other DCC writers mean COLLADA falloff_angle and FBX InnerAngle/OuterAngle as half or full cones. I read only Blender's code, which writes the full cone to FBX.

## Q9. Big-endian PLY

**Decides the design**
- **Big-endian PLY is common among the best-known public scans.** Armadillo, Lucy, the XYZ RGB Asian Dragon, Thai Statue and Manuscript, and the drill range scans are only offered as `binary_big_endian` [L]. The bunny, dragon and Happy Buddha reconstructions are ASCII.
- **So PLY readers must handle big-endian, and the oracles do.** three.js r183, assimp 6.0.5, SceneKit and Model I/O, and Blender 5.2.3 all read it. three.js's `examples/models/ply/binary/dolphins_be.ply` (855 vertices) and its `dolphins_le.ply` twin make a small, public test pair.
- **Writers emit big-endian only when asked, and none defaults to it.** MeshLab, Open3D, trimesh and Blender cannot write it at all. Writing only little-endian is correct, and litegen has no reason to offer big-endian output.

**Public files.** Headers were read from the first 64 to 128 KiB of each download, on 2026-11-04 [L].

| File | Header |
|---|---|
| `graphics.stanford.edu/pub/3Dscanrep/armadillo/Armadillo.ply.gz` | `binary_big_endian`, 172,974 vertices, 345,944 faces, "comment author: Paraform" |
| `graphics.stanford.edu/data/3Dscanrep/lucy.tar.gz` → `lucy.ply` | `binary_big_endian`, 14,027,872 vertices, 28,055,742 faces |
| `…/data/3Dscanrep/xyzrgb/xyzrgb_dragon.ply.gz` (Asian Dragon) | `binary_big_endian`, 3,609,600 vertices, 7,219,045 faces |
| `…/xyzrgb/xyzrgb_statuette.ply.gz` (Thai Statue) | `binary_big_endian`, 4,999,996 vertices, 10,000,000 faces |
| `…/xyzrgb/xyzrgb_manuscript.ply.gz` | `binary_big_endian`, 2,155,617 vertices, 4,305,818 faces |
| `…/pub/3Dscanrep/drill.tar.gz` → `drill/data/drill_1.6mm_*_cyb.ply` | `binary_big_endian` range scans: `vertex` plus a `range_grid` list element |
| `bunny.tar.gz` (scans and `bun_zipper.ply`), `dragon_recon.tar.gz` (`dragon_vrip.ply`), `happy_recon.tar.gz` (`happy_vrip.ply`) | `ascii`. The bunny scans also carry `range_grid`. |
| three.js r183 `examples/models/ply/binary/dolphins_be.ply` | `binary_big_endian`, 855 vertices, 1,689 faces. `dolphins_le.ply` is its little-endian twin; `Lucy100k.ply` is little-endian ("VCGLIB generated"). |

The repository's own page notes that ASCII "avoids the problem of using the correct big-endian vs. little-endian byte orders" (graphics.stanford.edu/data/3Dscanrep/).

**Writers**

| Writer | Big-endian output | Source |
|---|---|---|
| plyfile v1.1.5 (Python) | Yes: `PlyData(…, byte_order='>')`. The default `'='` means native, which is little-endian on x86 and ARM. | `dranjan/python-plyfile@v1.1.5 plyfile.py:L44-91` [S] |
| happly v1.1.0 (C++) | Yes: `DataFormat::BinaryBigEndian`. | `nmwsharp/happly@v1.1.0 happly.h:L69, L1136-1139` [S] |
| VTK 9.7.2 `vtkPLYWriter` | Yes: `SetDataByteOrderToBigEndian()`. The default is little-endian. | `Kitware/VTK@v9.7.2 IO/PLY/vtkPLYWriter.h:L67-70`, `vtkPLYWriter.cxx:L29, L142-150` [S] |
| CloudCompare 2.13.2 | Only from the command line: `-PLY_EXPORT_FMT BINARY_BE`. The GUI's "BINARY" is `PLY_DEFAULT`, which is native. | `CloudCompare@v2.13.2 qCC/ccCommandLineCommands.cpp:L6841-6858`; `libs/qCC_io/src/PlyFilter.cpp:L92-104` [S] |
| rply, and Greg Turk's original ply library | Yes: rply has the `PLY_BIG_ENDIAN` storage mode, and Turk's library has `PLY_BINARY_BE`; VTK's writer derives from it. | `vtkPLYWriter.cxx:L142-150`; CloudCompare's `ply_create` call [S] |
| PCL 1.15.1 | Only for a cloud whose `is_bigendian` flag is set. | `PointCloudLibrary/pcl@pcl-1.15.1 io/src/ply_io.cpp:L803-807, L1645` [S] |
| tinyply 3.0 | Only when re-writing a file it read as big-endian (`isBigEndian`). | `ddiakopoulos/tinyply@3.0 source/tinyply.h:L735-736, L1014` [S] |
| MeshLab 2025.07 (vcglib) | No: binary output is `"binary_little_endian"`. | `cnr-isti-vclab/vcglib@2025.07 wrap/io_trimesh/export_ply.h:L100, L113` [S] |
| Open3D v0.20.0 | No: `PLY_ASCII` or `PLY_LITTLE_ENDIAN`. | `isl-org/Open3D@v0.20.0 cpp/open3d/io/file_format/FilePLY.cpp:L532-533, L693-694` [S] |
| trimesh 5.2.0 | No: the encoding is "ascii" or "binary_little_endian". It reads both. | `mikedh/trimesh@5.2.0 trimesh/exchange/ply.py:L248-259, L488` [S] |
| Blender 5.2.3 | No: `ascii` or `binary_little_endian`. Its importer reads `binary_big_endian`. | `source/blender/io/ply/exporter/ply_export_header.cc:L24`; `importer/ply_import.cc:L120-121` [S] |

**Readers checked [L].** A three-vertex coloured face, written both ways, decoded identically in:
- three.js r183 PLYLoader, whose `little_endian` flag is true only for `binary_little_endian` (`PLYLoader.js:L679`);
- assimp 6.0.5 (`Ply/PlyLoader.cpp:L91-107, L246`);
- SceneKit and Model I/O.

**Not established**
- Other archives (Georgia Tech's large models, the Digital Michelangelo Project) were not checked.
- rply's source was not read at a tag. Its `PLY_BIG_ENDIAN` mode is taken from CloudCompare's use of it.

## Q10. glTF material extensions in other formats

**Decides the design**
- **UsdPreviewSurface (spec 2.6) carries only part of it:**
  - It carries clearcoat, clearcoatRoughness (default 0.01, so always write it), ior, and emissive strength folded into emissiveColor or the texture scale.
  - Transmission can only become opacity. With the default `opacityMode="transparent"`, a material keeps its specular ("models materials such as glass"), which is close to KHR transmission. `presence` is alpha coverage (`docs/spec_usdpreviewsurface.rst:L156-L178, L1112-L1119`).
  - There is no input for volume, sheen, specular in the metallic workflow, iridescence, anisotropy or dispersion.
  - specularColor only works when useSpecularWorkflow=1 (`L107-L135`), and RealityKit has no specular workflow.
- **MaterialX has an input for all ten extensions in `ND_gltf_pbr_surfaceshader`** (gltf_pbr 2.0.1, MaterialX 1.39.5). In my local tests, `usdchecker --arkit` 0.25.2 accepts it and usdrecord/Storm renders it [L]. But:
  - three.js r183 reads only `UsdPreviewSurface` and `arnold:openpbr_surface`.
  - SceneKit has no MaterialX.
  - RealityKit documents only its own surface nodes.
  - So write UsdPreviewSurface always, and gltf_pbr at most as an optional extra `outputs:mtlx:surface`.
- **FBX:**
  - No reader we test takes extension data from a Phong material.
  - ufbx sees Standard Surface only through Maya's Implementation and BindingTable objects; this is still open item #6 in research-fbx.md. Blender 5.2's default FBX importer is built on ufbx.
  - three.js FBXLoader reads only Phong and Lambert.
  - The one exception is emissive strength in `EmissiveFactor`: Blender writes it, and three.js, Blender and ufbx all read it.
- **three.js USDLoader reads USD colour constants as sRGB** (diffuse, emissive, specular). Emissive strength above 1 folded into emissiveColor will therefore look too bright in three.js.

**Mapping.** The MaterialX columns come from `libraries/bxdf/gltf_pbr.mtlx:L3-L34`, `open_pbr_surface.mtlx:L6-L95` and `standard_surface.mtlx:L14-L106` (SS = Standard Surface). The FBX names come from ufbx's tables. The Maya camelCase names follow ufbx's Arnold table, which agrees with research-fbx §4.2 for the base names. 3ds Max Physical uses the `3dsMax|Parameters|` prefix.

| Extension | UsdPreviewSurface | MaterialX: gltf_pbr / OpenPBR / SS | FBX: Maya SS `Maya\|…` / 3ds Max Physical |
|---|---|---|---|
| transmission | opacity = 1−t (an approximation; Blender does this) | transmission / transmission_weight / transmission | transmission / transparency |
| volume | none | thickness, attenuation_distance, attenuation_color / transmission_depth, transmission_color (Beer's law, OpenPBR `index.html:L778-L784`), geometry_thin_walled / transmission_depth, transmission_color, thin_walled | transmissionDepth, transmissionColor / trans_depth, trans_color |
| ior | ior | ior / specular_ior / specular_IOR | specularIOR / trans_ior |
| clearcoat | clearcoat (texture .r), clearcoatRoughness (.g); no coat normal | clearcoat, clearcoat_roughness, clearcoat_normal / coat_weight, coat_roughness, geometry_coat_normal / coat, coat_roughness, coat_normal | coat, coatRoughness, coatNormal / coating, coat_roughness, coat_ior |
| sheen | none | sheen_color, sheen_roughness / fuzz_* (a different model) / sheen, sheen_color, sheen_roughness | sheen, sheenColor, sheenRoughness / none |
| specular | none (specular workflow only) | specular, specular_color / specular_weight, specular_color / specular, specular_color | specular, specularColor / reflectivity, refl_color |
| iridescence | none | iridescence, iridescence_ior, iridescence_thickness (nm) / thin_film_weight, thin_film_ior, thin_film_thickness (**µm**, `index.html:L882`) / thin_film_IOR, thin_film_thickness (nm) | thinFilmThickness, thinFilmIOR / none |
| anisotropy | none | anisotropy_strength, anisotropy_rotation / specular_roughness_anisotropy plus geometry_tangent / specular_anisotropy, specular_rotation = θ/2π (the graph multiplies by 360°, `standard_surface.mtlx:L153-L156`) | specularAnisotropy, specularRotation / anisotropy, aniso_angle |
| dispersion (glTF stores 20/Abbe number) | none | dispersion / transmission_dispersion_scale = d with abbe_number 20 (`index.html:L823-L825`) / transmission_dispersion (an Abbe number, UNVERIFIED) | transmissionDispersion / none |
| emissive_strength | emissiveColor × strength, or the UsdUVTexture scale | emissive_strength / emission_luminance (**nits**, `index.html:L1150-L1159`) / emission | emission × emissionColor / emission, emit_color; legacy EmissiveFactor |

| Consumer | What it does | Source |
|---|---|---|
| three.js r183 USDLoader | Reads only shaders whose id is `UsdPreviewSurface` or `arnold:openpbr_surface`; `ND_*` surfaces are ignored. Preview Surface: ior; specularColor whatever the workflow; clearcoat and clearcoatRoughness; opacity as alpha blend or alphaTest; colours as sRGB. OpenPBR: transmission, coat, thin film (µm × 1000), specular weight and colour, anisotropy and emission_luminance, but not fuzz (sheen). | `USDComposer.js:L2435-L2441, L2511-L2523, L2539-L2570, L2626-L2671, L2675-L2708, L2803-L2932` [S] |
| three.js r183 FBXLoader | Phong or Lambert only. EmissiveFactor → emissiveIntensity, ReflectionFactor → reflectivity; texture slots include `Maya\|TEX_color/normal/ao_map`. No PBR properties. | `FBXLoader.js:L518-L546, L598-L629, L650-L725` [S] |
| three.js r183 USDZExporter (prior art) | Writes clearcoat (+ .r map), clearcoatRoughness (+ .g map) and ior. emissiveIntensity goes only into an emissive texture's scale, not into a constant colour. | `examples/jsm/exporters/USDZExporter.js:L936-L958, L1063-L1117` [S] |
| Blender 5.2.3 USD exporter (prior art) | Transmission Weight → opacity, inverted. Coat → clearcoat and clearcoatRoughness. IOR → ior. Emission Strength multiplied into emissiveColor. Specular IOR Level → a non-standard `inputs:specular`. | `source/blender/io/usd/intern/usd_writer_material.cc:L196-L211, L424-L426, L619-L633` [S] |
| Blender 5.2.3 FBX exporter | Phong only: EmissiveFactor = emission strength, ReflectionFactor = metallic. No coat, sheen, transmission or IOR. | `io_scene_fbx/export_fbx_bin.py:L1583-L1651` [S] |
| Blender 5.2.3 FBX importers | The default "FBX (.fbx)" importer is C++ on ufbx and maps IOR, coat, sheen, transmission, thin film, anisotropy and emission strength. "FBX (.fbx) (Legacy)" is the Python importer and reads Phong only. | `scripts/startup/bl_ui/space_topbar.py:L387-L388`; `io_scene_fbx/__init__.py:L716`; `source/blender/io/fbx/importer/fbx_import_material.cc:L132-L199`; `import_fbx.py:L2055-L2106` [S] |
| ufbx v0.23.1 | Has property tables for Standard Surface (OSL and Arnold), 3ds Max Physical, OpenPBR and 3ds Max's glTF material (`3dsMax\|extension\|clearcoat`, `…\|sheenColor`, `…\|specular`, `…\|transmission`, `…\|indexOfRefraction`). It picks a table only from the Implementation RenderAPI (ARNOLD_SHADER_ID or OSL) or the 3ds Max ClassIDs. | `ufbx.c:L19521-L19747, L22308-L22360` [S] |
| Apple renderers | MaterialX ✔ in RealityKit, Storm and Raytracer; ✗ in SceneKit. No specular workflow in RealityKit. RealityKit's own PBR surface (`ND_realitykit_pbr_surfaceshader`) adds only Specular and Clearcoat Normal to the Preview Surface inputs. | developer.apple.com/documentation/usd/validating-usd-files; developer.apple.com/documentation/shadergraph/realitykit/pbr-surface-(realitykit) [S] |
| usdchecker --arkit 0.25.2 | Passes the gltf_pbr, Standard Surface, OpenPBR and RealityKit PBR ids. A made-up id fails (MissingShaderIdInRegistry), a float3 base colour fails (MismatchedPropertyType), and unknown input names pass. | local test files [L] |
| usdrecord with Storm (Apple's build) | Renders the gltf_pbr and OpenPBR base colours (mean RGB 193,48,48 and 144,46,46). A made-up id falls back to grey (199,199,199). | local renders [L] |

**Not established**
- Whether RealityKit or Quick Look render the gltf_pbr, OpenPBR or Standard Surface nodes. That needs a device or Reality Composer Pro.
- Maya's full list of Standard Surface FBX property names; I inferred most of them from ufbx's Arnold table.
- What Standard Surface's transmission_dispersion means, and whether its anisotropy rotation turns the same way as glTF's.

---

## Appendix: local probes

These probes ran in a temporary local folder that wasn't kept, so this appendix records
how to redo each one. The paths below are relative to that folder.

**Q5, Q6, Q7 and Q9 (`q5q9/`)**
- **Inputs.** `scripts/make_inputs.py` (run with `python3 -I make_inputs.py <outdir>`; stdlib only) writes:
  - `pl.obj` (a `p` object, a 4-vertex `l`, a triangle), `mixed.obj` (a face and an `l` in one object), `vonly.obj`;
  - `pts.ply`, `edges.ply` (vertices plus `edge`), and `tri_be.ply` / `tri_le.ply` (one coloured face, big- and little-endian);
  - `modes.gltf` (modes 0–3, plus a quad with `TEXCOORD_0`–`_4`);
  - `lines.dae` (a `<lines>`, a two-`<p>` `<linestrips>` and a triangle) and `uv3.dae` (TEXCOORD sets 0–2);
  - `pc.usda` (Points, plus linear and cubic BasisCurves);
  - `uv3.usda` (`st`, `st1`, `st2`; the texture reads `st2`). The texture is red on the left and blue on the right, and `st`/`st1` sample the red half, so a blue render proves `st2` was used.
  - `uv10.*` adds `st3`–`st9` and TEXCOORD sets 3–9.
- **SceneKit and Model I/O.** `scripts/probe.swift`, run as `swift probe.swift <files…>`.
  - For each SceneKit node it prints the element primitive types and counts, the texcoord source count, `mappingChannel`, and whether there is a morpher or skinner.
  - For each Model I/O mesh it prints the submesh types and vertex attributes.
  - Load `.dae` with `SCNSceneSource(data:)`: `SCNScene(url:)` goes through an XPC service that fails in a sandbox.
  - `scripts/canimport.swift` prints `MDLAsset.canImportFileExtension` for each extension.
- **three.js r183 in Node.** `scripts/probe3.mjs <indir>` runs OBJLoader, PLYLoader and GLTFLoader; `scripts/probe_usd.mjs <file.usda>` runs USDLoader. GLTFLoader with data URIs needs `globalThis.ProgressEvent` stubbed first.
- **assimp.**
  - `assimp info <f>` prints per-mesh primitive types. Its default preset includes ValidateDataStructure, which rejects point clouds; `-fd` replaces the preset.
  - `assimp dump <f> out.assxml -s`, then count the `<TextureCoords` elements.
- **Storm.** `usdrecord --imageWidth 256 [--complexity veryhigh] in.usda out.png`.
- **PLY headers.** `curl -sL -r 0-65535 <url> | gzip -dc | head -c 2000`. Members of a `.tar.gz` are listed with `scripts/tarply.py`, run as `python3 -I tarply.py < stream`.

**The other probes' test files** were in `q1q2/`, `q3q4/` and `q8q10/`; their sections name them.
