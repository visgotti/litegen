# USD: USDC output, several clips, unlit — design

Part of the mesh-gaps set ([overview](2026-11-04-mesh-gaps-overview-design.md)). Size: M–L.
Three independent parts in one item, because all three live in the USD writer. Start
after items 11 (UV sets) and 4 (material layers), which also edit `usd_write.rs`.
Research: [`research-codecs.md`](../research/research-codecs.md) §4 (USDC) and
[`research-consumers.md`](../research/research-consumers.md) §1–2 (clips, unlit).

## Why

- **USDC.** Crates are read (`usdc.rs`: LZ4, integer and float compression, every
  section) but never written. `usdc` and `usd` stay unparsed because "a format name
  must mean both directions" (`mod.rs`). Pipelines that take only binary USD get
  nothing. Apple's own Quick Look gallery ships crate files (0.8.0, 0.9.0 and 0.10.0).
- **Several clips.** USD has one timeline, so `usd::prepare` keeps the first clip
  (`s.animations.truncate(1)`) and `usd::losses` reports the rest: "has one timeline;
  only the first clip is kept". An animated character with walk, run and idle reaches
  USDZ, and so AR Quick Look, with walk alone.
- **Unlit.** `KHR_materials_unlit` materials are "shaded as lit" and reported.
  UsdPreviewSurface has no unlit mode. A flat-colour UI element or a baked-lighting
  model gets lit twice.

## Decisions

### USDC (D4, settled: `usdc` is public; `usdz_root` is an opt-in)

- **A new container, `MeshFormat::Usdc`** (`"usdc"`). It's a standalone crate layer.
  Its textures are sidecars in package mode and absent from a single file, like
  `usda`. It joins `MeshFormat::ALL`, `Model3dFormat` (the OpenAPI enum) and
  `derivable_formats()`. `.usd` stays unparsed: it names either encoding, and a written
  format name must mean one.
- **The writer encodes the layer we already parse,** once the layer holds everything
  the crate needs. `usd::write_usdc` runs `usd_write::write_layer` (USDA text), then
  `usda::parse(text)` into a `Layer`, then the new `usdc::write(&Layer) ->
  Result<Vec<u8>, MeshError>`. Text and crate can't drift, because the crate is the
  parsed text.
  - Today's `Layer` loses three things the crate must carry (builder check). Fixing
    these is the first task:
    - variability: `usda.rs` skips `uniform`/`config`/`varying`, and `AttrSpec` has no
      field for it, yet `usd_write` authors `uniform token info:id`, `xformOpOrder`,
      `skel:blendShapes`, `orientation` and `subdivisionScheme`;
    - attribute metadata beyond `interpolation` and `elementSize`, which are the only
      two kept;
    - dictionary entry types: `customData` entries lose their declared types, and
      `string` and `token` both become `Value::Str`.
  - So `AttrSpec` gains `variability: Variability { Varying, Uniform, Config }` and
    `metadata: Vec<(String, Value)>`, and `Value` gains `Token(String)` and
    `Tokens(Vec<String>)` beside `Str`/`Strs`, which `as_str()` and `strs()` also
    accept. **Both parsers fill them,** `usda.rs` and `usdc.rs`, so the reader gets
    more faithful too, and every existing reader match on `Value::Str` is checked for
    `Value::Token`.
- **Crate version 0.8.0.** It's what `usdcat` and OpenUSD 25.05+ write by default.
  OpenUSD 26.11 reads it without a deprecation warning, USD ≥ 19.03 reads it, and Pixar
  intends it as the minimum. Nothing litegen writes needs a later version (timecode
  values 0.9, path expressions 0.10, relocates 0.11, splines 0.12+).
- **Compression, as cheap as the format allows.** From 0.4.0, structural sections must
  be compressed, so:
  - TOKENS: `TfFastCompression` with one chunk (byte 0 = 0) of **literal-only LZ4**,
    which is valid LZ4 and needs no match finder;
  - STRINGS: raw;
  - FIELDS, FIELDSETS, PATHS and SPECS: Sdf integer coding (deltas, 2-bit width codes,
    a common value), the inverse of `usdc.rs`'s `decode_ints`, with FIELDS' value
    reps LZ4'd;
  - arrays: uncompressed, with the compressed bit clear. That's allowed for every
    array, and the reader takes the uncompressed path.
- **Types come from `AttrSpec.type_name`.** The crate needs exact types (`point3f[]`,
  `half3[]`, `quatf[]`, `token` versus `string`), and the parsed text keeps every
  declared type name. A test enumerates every `type_name` that `usd_write` emits
  across the fixtures, and asserts `usdc::write` encodes each with no fallback.
- **Output is deterministic,** byte-identical between runs.
- **USDZ's root is the caller's choice: `usdz_root=usda|usdc`** (the overview's
  options table).
  - `usda` stays the default. `usdchecker --arkit` has accepted it since OpenUSD 24.03,
    and model-viewer ships the same to Quick Look.
  - `usdc` writes the crate as the root (`model.usdc` first), as Apple's own packagers
    and most of Apple's Quick Look gallery do.
  - It's a converter query parameter and a 3D request field, recorded in the
    `DeliveryPlan`. With keep-nothing it's 400 `option_needs_keeping`.
  - `usdchecker --arkit` must accept both roots. The AR Quick Look device check at the
    end of the plan opens one of each.

### Several clips (following Apple's layout; [`research-consumers.md`](../research/research-consumers.md) Q1), and the `usd_clips` option

**`usd_clips=all|first`** (the overview's options table):
- `first` is today's output, byte for byte: the first clip, the rest reported.
- `all` is the layout below.
- The default is `all` for USDA and USDC. For USDZ it's `first` until the AR Quick Look
  check passes, then `all`, because on-device behaviour of variant-switched animation is
  unestablished.
- It's a converter query parameter and a 3D request field, recorded in the
  `DeliveryPlan`.

The layout `all` writes:

Apple documents one way to hold alternative clips in one USDZ, and builds it with its
own `variant_combiner.py`. Defaults:

- **One SkelAnimation prim per clip,** under the Skeleton, named by the clip's
  sanitised name, with the original in `customData` as prim names already are. These
  sit **outside** the variant set.
- **A variant set named `Animation` on the default prim,** one variant per clip.
  Each variant authors only `rel skel:animationSource = </…/Skel/<clip>>` on every
  Skeleton. The first clip is the authored selection: USD applies no variant without
  one.
- **Nothing a variant varies is authored outside the variant set.** A local opinion is
  stronger than a variant's, and a stronger default beats a weaker animated value
  (measured in the research). So `skel:animationSource` appears only inside the
  variants.
- **Rigid (xformOp) animation stays as today: the first clip's only** (revised after
  the builder check). A variant can vary an xformOp only when nothing outside the
  variants authors it, and then readers that skip variants lose the node's rest
  transform entirely. three.js's USDA parser doesn't parse variant bodies, and its USDC
  path adds variant children as extra prims. So:
  - the first clip's rigid channels are authored outside the variants, exactly as
    today, and every reader shows them;
  - the other clips' rigid channels are reported lost (`animation`, "rigid motion of
    clips [run, wave]: USD switches skeleton bindings between clips, not
    transforms").

  Every clip's skeletal and blend-shape animation, the common case for characters, goes
  through the SkelAnimations, which every reader sees. A scene whose clips are all rigid
  writes as today: the first clip, the rest reported. Turning rigid animation into
  rigid skinning (UsdSkel's constant joint bindings), so every clip can switch, is
  listed under non-goals as the follow-up.
- **One timeline:** `endTimeCode` is the longest clip (Apple's tool's "max" mode), and a
  shorter clip holds its last pose. Each SkelAnimation's `customData.litegen.clipEnd`
  records its own end, so litegen reads exact durations.
- **What each consumer then does:**
  - three.js r183 returns one `AnimationClip` per SkelAnimation prim, so every clip,
    named after the prim;
  - Storm (usdview, usdrecord) and Blender play the selected variant;
  - `usdchecker --arkit` validates each variant;
  - Apple says Quick Look shows default-prim variants as configuration options;
  - RealityKit apps see the active variant's timeline.

  The research couldn't establish device behaviour (switching, looping), and it's
  listed under "not yet known" in `3d-format-conversion.md` §5.
- **The reader recognises this convention, and only this one.** A default prim whose
  variant set `Animation` switches `skel:animationSource` is composed once per variant
  (a selection override) and read as one clip per variant, with the authored selection
  first and the rest in variant order. Rigid xformOp samples, authored outside the
  variants, belong to the authored selection's clip only.
  - It's bounded at 64 variants, with the composition budget per variant.
  - Other variant sets, and several SkelAnimations with only one bound (some
    exporters write that), read as today: the bound or selected one only.
- Under `all`, `usd::prepare` stops truncating, and `usd::losses` stops reporting the
  skeletal clips. `usd::capabilities().animation` is already true. Under `first`,
  both behave as today.

### Unlit (mine; [`research-consumers.md`](../research/research-consumers.md) Q2)

- **google/usd_from_gltf's recipe, measured exact in Storm:**
  - `diffuseColor` (0, 0, 0), **`metallic` 1**, `roughness` 1;
  - `emissiveColor` = the base colour factor, or the base-colour texture's `outputs:rgb`
    (sRGB) with `scale` = the factor;
  - opacity as for a lit material.

  A metal with a black base has an F0 of black, so no specular remains. Storm drew the
  pixel equal to the linear base colour at 0°, 70° and 85°, where `metallic` 0 left 1–2
  levels of specular. three.js builds it as a black `MeshPhysicalMaterial` with
  `emissive` = base. RealityKit and Quick Look draw it close to unlit: the research
  cites a rim-light factor in Quick Look, not measured on a device.
- **`customData = { litegen = { unlit = true } }` on the Material prim,** so litegen's
  reader restores `unlit: true` and the base colour from the emissive connection. No
  standard marker exists. Without ours, the recipe reads as a lit black emissive
  material, as today, because guessing intent from the recipe could misread a real
  emissive design.
- `usd::losses` stops reporting unlit.
- **Known device risk, recorded in "not yet known":** usd_from_gltf writes emissive
  textures linear because Quick Look treated the emissive texture as linear. Whether
  that still holds is unestablished, so an unlit textured USDZ may look brighter in
  Quick Look than in Storm until a device check settles it.
- **Not built: the RealityKit MaterialX unlit node** (`ND_realitykit_unlit_surfaceshader`).
  It's exact in RealityKit, but Storm renders it white, Storm prefers
  `outputs:mtlx:surface` when both outputs exist, and three.js ignores it. It belongs
  in a later opt-in "RealityKit profile", not the default.

## Non-goals

- USDC with compressed arrays or a real LZ4 match finder. Both are optional, and can
  come later as a size optimisation.
- Value clips, splines, or the 0.9+ crate features.
- Any variant set litegen didn't write.
- Switching rigid (xformOp) animation between clips. The follow-up would rig
  rigidly animated nodes as joints with constant bindings, so every clip becomes a
  SkelAnimation.
- RealityKit's MaterialX unlit surface. It isn't UsdPreviewSurface, and Storm,
  three.js and SceneKit don't draw it.

## What proves it is done

### USDC: unit tests, RED first

- `usda_keeps_variability_metadata_and_token_types`: a hand-written USDA with
  `uniform` attributes, extra attribute metadata (`customData`, `doc`), and a dictionary
  mixing `string`, `token`, `int`, `double` and nested dicts parses into a `Layer` that
  holds all of it.
- `usdc_round_trips_every_layer_we_write`: for every fixture, in both write modes,
  `usdc::parse(usdc::write(usda::parse(text))) == usda::parse(text)` (Layer equality:
  specs, fields, values, variability, attribute metadata, dictionary types, time
  samples, list ops, layer metadata), with no warnings.
- `usdc_scenes_equal_usda_scenes`: `read_scene(usdc) == read_scene(usda)` for every
  fixture.
- `usdc_encodes_every_type_we_write` (the enumeration test above).
- `usdc_output_is_deterministic`.
- **Codec pieces:**
  - `integer_coding_round_trips` (random and edge arrays: empty, one value, all equal,
    i64 extremes, each width code);
  - `literal_only_lz4_round_trips_through_our_decoder`;
  - `the_bootstrap_says_0_8_0`.
- The existing crate-reader sweeps (truncation and bit flips) also run on our writer's
  output for every fixture.

### USDC: independent tools

These are the harness's installed Apple USD Tools 0.25.2 (`/usr/bin`; USD 25.02, which
reads crates up to 0.12.0). `usddiff` isn't installed, and installing OpenUSD isn't
part of this item.

- `usdcat model.usdc` exits 0, with no warning;
- **`usdcat model.usda` and `usdcat model.usdc` print identical text.** usdcat prints
  both encodings through one canonical printer, so any difference is a value, type,
  variability or metadata the crate lost. This replaces `usddiff`, and it's what
  catches the `uniform` and token issues;
- `usdchecker model.usdc` reports 0 errors;
- usdrecord renders the `.usdc` and the `.usda` identically (0.00 %);
- three.js `USDLoader` renders the `.usdc`, which joins the harness's formats with its
  own row and column.

Negative controls:
- write a wrong width code in one integer-coded section: `usdcat` fails, and
  `usdc_round_trips_every_layer_we_write` fails;
- drop `uniform` from the crate's variability field: the `usdcat` text comparison
  fails.

Restore each by rewriting.

### Several clips

- `fixtures::clips()`: one skinned rig, three named clips of different lengths ("walk"
  1 s, "run" 0.6 s, "wave" 2 s). "walk" and "run" also animate a non-joint Xform (a
  prop), so the rigid rule is tested both ways.
- `usd_keeps_every_clip`: `clips()` → USDZ, USDA and USDC → read. Three clips, with
  names, order and durations (from `clipEnd`) equal. Their skeletal keys are equal at
  every sample time within the existing USD resampling tolerance. walk's rigid prop
  animation is kept. run's is reported lost, by exactly one `animation` loss naming
  `run`.
- `the_layout_is_apples`: in the written text:
  - three SkelAnimation prims outside the variant set;
  - `variantSets = "Animation"` with selection `Animation = "walk"`;
  - each variant authors only `skel:animationSource`;
  - no `skel:animationSource` outside the variant set;
  - walk's rigid xformOps outside the variants, as today.
- `an_all_rigid_multi_clip_scene_writes_as_today`: two rigid-only clips give the
  first clip, the other reported, and no variant set. The output is byte-identical to
  today's for that scene.
- `three_js_lists_every_clip` (harness, Node): three.js r183 `USDLoader` on the `.usdc`
  returns three clips named walk, run and wave, with matching durations. Use USDC,
  because three.js assumes 30 fps for USDA.
- `other_variant_sets_read_as_today`: a hand-written USDA with a `look` variant set
  and time samples reads one clip (the selection), as today.
- `a_hostile_animation_variant_count_is_capped`: 65 variants gives `Unsupported`,
  naming the cap.
- **Harness, per variant:**
  - usdrecord renders each variant (`--sessionLayer` with `over "<root>" (variants =
    { string Animation = "<clip>" }) {}`) at rest and at every sample time, compared
    with that clip's expected picture;
  - `usdchecker --arkit` accepts every USDZ and reports each variant;
  - three.js renders each SkelAnimation clip, and walk's rigid motion (outside the
    variants) as its `TransformAnimation` clip;
  - SceneKit plays the selection (Apple rates its variant support "partial"), and the
    other variants are cited skips.
- **The device check sets USDZ's default.** Device behaviour of variant-switched
  animation is unestablished (research Q1). So USDZ ships with `usd_clips` defaulting
  to `first`, today's behaviour, and callers can choose `all`. The plan's last task has
  the user open three USDZs in AR Quick Look on an iPhone:
  - a multi-clip `all` (does the default clip play, and does the picker switch?);
  - a `usdz_root=usdc`;
  - a plain one.

  If the multi-clip file passes, USDZ's default flips to `all`, with one constant and its
  test. If it doesn't, the default stays `first` and the result is recorded in
  `3d-format-conversion.md` §5.
- `usd_clips_first_is_todays_output`: `clips()` with `usd_clips=first` gives output
  byte-identical to today's.
- `scene_matrix_tests`: `clips()` in every pair. The clip count follows each
  container's capability (FBX, glTF, Collada and USD all keep three; static containers
  none).
- Negative controls:
  - bind every variant's `skel:animationSource` to the first SkelAnimation: the
    per-variant usdrecord cases and `usd_keeps_every_clip` fail;
  - author `skel:animationSource` outside the variant set as well: the per-variant
    usdrecord cases fail, because the local opinion wins, which proves the writer rule
    matters.

  Restore each by rewriting.

### Unlit

- `unlit_round_trips_through_usd`: a `textured()` variant with `unlit: true` and a
  base-colour factor → USD → read. `unlit` is true, and the base colour and texture are
  equal. The recipe is asserted in the written text: `diffuseColor` black, `metallic`
  1, `roughness` 1, the emissive connection with `scale` = factor, and the
  `customData` marker.
- `a_black_diffuse_emissive_material_without_the_marker_reads_lit`.
- **Harness:** usdrecord renders the unlit material **lit, twice, with two different
  light intensities.** The unlit surface's pixels must be identical between the two
  renders, within 1 %, and equal the base colour within the albedo threshold. A lit
  control material in the same frame must change. This proves "unlit" independently of
  how it's encoded. SceneKit and three.js get the same two-light check, with any
  residual highlight measured and recorded in the module doc.
- Negative control: write `diffuseColor` = base colour (lit) instead of black. The
  two-light invariance check fails. Restore by rewriting.
- If the recipe itself fails the two-light check in usdrecord (the research measured
  it exact in Storm, so this would be a writer bug or a tool difference), the item
  stops there. It's reported to the user with the measurement, and no tolerance is
  widened.

### API, SDK, MCP (`usdc`, `usdz_root`, `usd_clips`)

- Send the frontend/SDK session the shapes:
  - `Model3dFormat` adds `"usdc"`. The converter's `from`/`to`, `output_formats` and
    `derivable_values` follow from `derivable_formats()`;
  - the converter query and 3D request field `usdz_root: "usda" | "usdc"` and
    `usd_clips: "all" | "first"`;
  - `option_needs_keeping`.
- Option tests:
  - `usdz_root_usdc_writes_a_crate_root`: the first zip entry is `model.usdc`, it's
    still 64-byte aligned, `usdchecker --arkit` passes, and it reads back equal;
  - each option's bad value gives 400 `param_invalid`;
  - keep-nothing with a non-default value gives 400 `option_needs_keeping`;
  - the poller path honours the recorded plan.
- openapi tests; the SDK suites after regeneration; `docs/mcp/tools.md` regenerated;
  `apps/landing`'s derived capabilities (`sync-openapi.mjs`) and any price-api enum
  that lists formats (`apps/price-api/src/common/enums.ts`): check with grep, and
  coordinate with the owning session.
- `tests/model3d_api.rs`: `output_formats: [usdc]` on a mock row delivers a crate that
  `read_scene` reads.

### Docs

- `usd.rs` and `usdc.rs` module docs;
- `mesh/mod.rs` (`MeshFormat` docs, the table's USD rows);
- `3d-format-conversion.md` §5 and §9 ("USDC writing" goes);
- the README's format list;
- the matrix page (a USDC column; the USD "Animation" and "Materials" rows; via
  litegen-a3).

### Gate

The overview's full gate, and the harness with `clips` and the unlit variant across
every format, including the new `usdc` row and column.

## Left to the builder

- The literal-only LZ4 framing details. Mirror `usdc.rs`'s decoder.
- Per-variant composition in the reader: a selection override on a cloned stage, or a
  stage-level selection map.
