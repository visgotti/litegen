# FBX: animation layers after the first, and ASCII output — design

Part of the mesh-gaps set ([overview](2026-11-04-mesh-gaps-overview-design.md)). Size: S–M.
Two independent parts. Research: [`research-consumers.md`](../research/research-consumers.md) §3 (layer semantics) and §4
(who reads ASCII FBX).

## Why

- **Layers.** An FBX animation stack can blend several `AnimationLayer`s. Animators use
  them for additive breathing over a walk, or an override fix on top of mocap. The
  reader takes the first layer with curves and warns "{n} animation layers would blend;
  only the first was carried" (`fbx_read.rs`, `stack_animation`). The clip plays
  without the other layers' contribution, so a DCC file's motion differs from what its
  author saw.
- **ASCII.** FBX 7.x has a text encoding. litegen reads it (`fbx_dom::parse_ascii`,
  including the 7.2+ `KeyAttrDataFloat` bit-pattern rule) but writes binary 7400 only.
  Nothing has asked for ASCII output. It's useful for diffing and debugging, and for
  pipelines that check FBX into text-diffed repositories.

## Decisions

### Layers (mine)

- **Every layer of a stack is evaluated and blended into one clip, by ufbx v0.23.1's
  formulas exactly.** ufbx is the reader's existing authority for curves, which already
  match Maya and MotionBuilder within 1.1e-3 on 37 files. It's also what Blender 5.x's
  default importer and Godot users see, and the only oracle that blends layers at all:
  - three.js plays layer 0;
  - assimp lets the last curve win;
  - Blender makes one Action per layer and plays the first.

  The formulas (`ufbx.c` L22221–22251 and L25711–25817; [research-consumers.md](../research/research-consumers.md) Q3):
  - layers fold in stack order (the file's connection order), and layer 0 replaces the
    property;
  - `Weight` is divided by 100 and clamped to [0, 1]. It can be animated (a
    `d|Weight` curve node), and is read for blended layers;
  - `BlendMode` 0 additive: `r += w·v`. 1 override: `r = v`. 2 override-passthrough:
    `r = (1−w)·r + w·v`. Unknown values: `r = v`;
  - `RotationAccumulationMode` 0 composes quaternions in the node's rotation order
    (additive `q·slerp(1, q_layer, w)`, passthrough `slerp(q, q_layer, w)`), and 1 adds
    or lerps Euler components;
  - `ScaleAccumulationMode` 0 composes (additive `s·|v|^w`, passthrough
    `|s|^(1−w)·|v|^w`), and 1 adds or lerps.
- **Where ufbx and the SDK docs disagree, litegen follows ufbx and says so.** A
  warning names the layer and the property when a file has:
  - a base layer whose `Weight` isn't 100, or whose `BlendMode` isn't additive (the
    SDK multiplies the base layer by its weight; ufbx ignores both);
  - a pure Override layer with `Weight` below 100 (MotionBuilder says the weight still
    scales it; ufbx ignores it);
  - `Mute`, `Solo` or `BlendModeBypass` set (the SDK honours them; ufbx never reads
    them).

  No SDK was run in the research, so ufbx's reading is the one litegen can verify.
- **The result is resampled** at the union of every layer's key times plus the 30 fps
  grid, the same as the writer's resampling (`pose::resample_times`, with its
  100,000-step cap). The blended keys are **charged to the reader's own key budget**
  (`fbx_read.rs`'s `MAX_ANIMATION_KEYS` accounting) before they're allocated, so a
  many-layer stack is refused like any file past the scene caps. Rotation is
  blended as the SDK does, then stored as quaternions. Near gimbal lock, the reader
  refines between keys, as the writer does (`refine_euler_times`).
- **Blend-shape weights on layers** follow the same blend modes. `DeformPercent`
  curves on later layers add or override as their layer says.
- **One layer is unchanged:** a stack with one layer takes today's path, bit for bit.
  The blend runs only for more than one contributing layer.
- The warning goes. A layer property litegen can't evaluate (an animated `Weight` with
  a curve type the reader lacks, if any) keeps a specific warning naming the layer.

### ASCII output (D5, settled: built, as an opt-in)

The research found every oracle reads ASCII 7.x except Blender's Python importer. That
importer was Blender's only importer through 4.4 and the default in 4.5, and it's
"FBX (.fbx) (Legacy)" in 5.x, where the default ufbx importer reads ASCII. ASCII is
1.3–2× larger, can't deflate arrays, and base64-inflates textures by a third. Its one
real advantage, no `FileId`/`CreationTime`/footer to get wrong, doesn't apply, because
litegen already writes the known-good triple. So binary stays the default, and ASCII is a choice callers can make (the user: "make sure users can decide and have options"):

- `WriteOptions` gains `fbx_encoding: FbxEncoding { #[default] Binary, Ascii }`.
  `fbx_dom` gains `write_records_ascii(top: &[WRec], out: &mut Vec<u8>)`, serialising
  the same record tree the binary writer serialises. There's one writer, with two
  encodings.
- **Text rules, in the SDK's 7.4 ASCII form:**
  - the `; FBX 7.4.0 project file` header, and `FBXHeaderExtension { FBXVersion: 7400 }`,
    which three.js reads the version from (FBXLoader rejects ASCII below 7000);
  - `Name: values` records with `{ }` children;
  - arrays as `*N { a: v,v,… }`;
  - object names as `"Class::Name"` (binary's `\x00\x01` separator reversed);
  - numbers in shortest round-trip form;
  - embedded texture `Content` as base64 in a quoted string;
  - `KeyAttrDataFloat` values as the integers of their float bit patterns, which the
    SDK writes from 7.2 and ufbx and our reader read.
- **Where it's offered:** `fbx_encoding=binary|ascii` as a converter query parameter
  (only with `to=fbx`, else `param_unsupported`), and as a 3D request field that every
  3D model accepts. It applies to every FBX litegen writes for that request, as
  `fbx_color_space` does, and it's recorded in the `DeliveryPlan`. Keep-nothing with
  `ascii` gives 400 `option_needs_keeping`.
- **Who reads it,** written into the parameter's OpenAPI text: the Autodesk SDK (Maya,
  3ds Max, MotionBuilder, Unreal through it), ufbx (and so Godot and Blender ≥ 5.0's
  default importer), assimp and three.js `FBXLoader`. **Blender's Python importer
  refuses it** (`import_fbx.py` L3079–3094: "ASCII FBX files are not supported"). That
  was the default through 4.5 and is "Legacy" in 5.x, and the text says so.

## Non-goals

- Writing several layers. The writer keeps one layer per stack, with blended
  animation. It's what every reader plays, and it's equivalent.
- FBX 6.x ASCII.

## What proves it is done

### Layers: unit tests, RED first

- **Hand-built stacks** (through the test DOM helpers), each against values computed by
  hand from ufbx's formulas:
  - `two_additive_layers_add` (translation and Euler rotation by channel);
  - `an_override_layer_replaces` (weight ignored, plus the warning);
  - `override_passthrough_lerps_by_weight`;
  - `rotation_by_layer_composes_quaternions` (in a non-XYZ rotation order too);
  - `scale_composes_and_adds_by_mode`;
  - `mute_solo_and_bypass_are_warned_not_applied`;
  - `a_base_layer_weight_is_warned_and_ignored`;
  - `an_animated_layer_weight_is_evaluated_per_key`;
  - `blend_shape_weights_blend_by_layer`.
- `a_single_layer_stack_is_byte_identical`: every existing FBX test file reads to the
  same scene as before. Hash the read scenes before the change.
- `layer_resampling_respects_the_budget`: 50 layers × 1,000 channels over a long take
  is refused by the reader's `MAX_ANIMATION_KEYS` charge (the same error kind and
  message family as today's key-budget refusal), before the blended keys are
  allocated.
- **Reference values,** from ufbx v0.23.1's corpus (`data/`; each file committed only
  if the corpus licence allows, else local):
  - `maya_anim_layers_7500_{ascii,binary}.fbx` and the `_acc`, `_over` and `_over_acc`
    variants (layers X and Y; ufbx's own expectations in `test/test_animation.h`
    L1224–1265);
  - `maya_anim_layer_anim_7700_ascii.fbx` (animated weight; ufbx expects pCube1
    translate.y / rotate.x of −1.0 / 22.5° at frame 6 and 0.0 / 45° at frame 12, where
    three.js gives −2.0 / 0 and assimp +2.0 / 45);
  - `maya_anim_layers_over_acc_orders_7500_*` (six rotation orders);
  - `synthetic_rotation_order_layers_7700_ascii.fbx`.
- `layered_files_match_ufbx`: at ufbx's own expectations, matched exactly as its tests
  compare, and at 30 more sample times computed once by a small C program over ufbx
  (`ufbx_evaluate_transform`; offline, outputs committed as numbers), our world
  transforms are within 1.1e-3 of model size. Frame 18 of the orders file isn't used:
  its static pose equals the export pose, so it can't catch a reader that drops layers
  (research Q3).
- Negative control: treat passthrough as additive. `override_passthrough_lerps_by_weight`
  and `layered_files_match_ufbx` (the `_over` files) fail. Restore by rewriting.

### ASCII: tests

- `ascii_reads_back_as_binary_does`: for every fixture, `read(write_ascii(s)) ==
  read(write_binary(s))`, the whole scene equal. Floats are exact: the reader parses
  shortest-repr text back to the same bits.
- `ascii_key_attr_floats_are_bit_patterns`: the written `KeyAttrDataFloat` tokens are
  integers, and reading them gives the original floats' bits.
- `ascii_names_use_class_separator`, `ascii_embedded_textures_are_base64`.
- **Converter:** `fbx_encoding=ascii` with `to=fbx` gives a body starting
  `; FBX 7.4.0 project file`. With `to=glb` it's 400 `param_unsupported`, and a bad
  value is 400 `param_invalid`.
- **Generations** (tests/model3d_api.rs):
  - `fbx_encoding: "ascii"` with `output_formats: [glb, fbx]` gives an ASCII FBX and a
    GLB unchanged;
  - the default is byte-identical to today;
  - keep-nothing with `ascii` gives 400 `option_needs_keeping`;
  - the poller path honours the recorded plan.
- **Harness:** an `fbx:ascii` variant of every direction into FBX. three.js
  `FBXLoader` renders it at 0.00 % against the binary FBX's picture. `assimp info`
  imports it. If ufbx's CLI or a ufbx-based check is available, it loads with no
  errors.
- **Third-party:** our reader reads the two committed assimp ASCII files as today. An
  ASCII file written by the Autodesk SDK (FBX Converter or FBX Review), if one can be
  produced locally from our content, reads equal to its binary twin.
- Negative control: print `KeyAttrDataFloat` as decimal floats. Our reader still reads
  the tokens (fractions are accepted), but `ascii_key_attr_floats_are_bit_patterns`
  fails, and so do the tangent-weighted curve comparisons against three.js. Restore by
  rewriting.

### Docs

- `fbx.rs` and `fbx_read.rs` module docs (layers are carried, with the rules and
  their citations; ASCII output);
- `mesh/mod.rs`'s FBX row ("every animation clip" now includes layered stacks);
- `3d-format-conversion.md` §4 ("every animation layer after the first" leaves the
  not-carried list);
- the converter's and the request's OpenAPI text, the SDK types regenerated, and
  `docs/mcp/tools.md`;
- the matrix page (via litegen-a3).

### Gate

The overview's full gate, and the harness with the FBX rows and the `fbx:ascii` variant.

## Left to the builder

- The C program that runs ufbx's evaluation for the reference values. It's offline,
  in `docs/superpowers/research/fbx/scripts/`, standard library plus `ufbx.c`.
- How the reader groups curves across layers before blending.
