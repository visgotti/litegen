# Collada morph targets in output, on request — design

Part of the mesh-gaps set ([overview](2026-11-04-mesh-gaps-overview-design.md)). Size: S.
OpenAPI-visible: `dae_morphs` on the converter and on 3D requests (D6).

## Why

Collada can carry morph targets (`<controller><morph>`, COLLADA 1.4.1 §5-81), and
litegen's reader reads them. These are NORMALIZED and RELATIVE morphs, skin over morph,
and weight animation, checked against SceneKit on six files. The writer never writes
them. `dae::prepare` bakes every target at its node's effective weights, drops the
weight animation, and reports `morph_targets`. A character's face shapes, sent to a
Collada consumer that plays morphs (SceneKit and Xcode's scene editor, the main ones
left now that Blender 5.0 has dropped Collada), arrive frozen.

## Why it is off, and why it stays off by default

From `dae.rs`'s module doc:

- **three.js r183's `ColladaLoader` fails the whole file on any `<morph>`.**
  `parseController` stores a controller with no `skin`, and `buildNode` then reads
  `controller.skin.joints`. A skin over a morph fails in `buildController`. three.js is
  the dashboard's viewer and the most-used open reader.
  *Measured on 2026-11-09 by item 8's harness:* a file with a weight animation fails
  earlier, in `ColladaParser.parseAnimationChannel` (`sid.indexOf`), because the weight
  channel's target `…-weights(i)` has no `/`. The `joints` and `sources` failures above are
  what a static morph and a static skin over a morph meet.
- **assimp 6.0 fails too.** Its `<targets>` loop in `ColladaParser::ReadController`
  advances the wrong node, so the import fails with "Unable to resolve library
  reference ''".

The research ([`research-consumers.md`](../research/research-consumers.md) Q5) confirms this and widens it:
- only SceneKit reads Collada morph controllers (measured);
- three.js r183 and r187 fail the whole file, as do assimp 6.0.5 and assimp master;
- Godot reads morphs (from source);
- Blender 5.0 removed Collada;
- Unity and SketchUp weren't established.

So the default doesn't change: baking is what keeps a file openable everywhere. The
gap is that a caller who knows their reader plays morphs (SceneKit and Xcode, Godot)
can't ask for them.

## Decisions

- **Library (mine):** `WriteOptions` gains `dae_morphs: DaeMorphs`:
  - `Bake`, the default, is today's behaviour, byte for byte;
  - `Keep` writes morph controllers.

  `dae::capabilities(opts)` reports `morph_targets: true` under `Keep`, so `losses`
  and `prepare` follow the capability table as everywhere else. `dae::extra_losses`
  stops reporting the weight channels under `Keep`.
- **The form written under `Keep` (mine, settled by the oracle):**
  - one `<controller id="…-morph">` per morphed mesh, with `<morph method="NORMALIZED"
    source="#<base geometry>">`;
  - an `IDREF_array` of target geometries, each a full `<geometry>` holding the
    target's absolute positions (base + offset) and normals;
  - a `float_array` of default weights;
  - `<targets>` with the `MORPH_TARGET` and `MORPH_WEIGHT` inputs.

  NORMALIZED with absolute targets is the reading every implementation agrees on
  (`base·(1−Σw) + Σ wᵢ·targetᵢ`). If SceneKit reads it otherwise in the oracle run, the
  builder switches to `RELATIVE` with offset targets, which the reader also takes.
  Whichever form SceneKit plays correctly is the one shipped.
  - A skinned and morphed mesh is `<skin source="#…-morph">`: skin over morph, as the
    reader reads it.
  - Weight animation is one `<channel>` per animated weight, addressed as the reader
    resolves it (the morph's weight array element, `…-morph-weights(i)`). A source key
    at 0, and STEP emulation, follow the existing matrix-channel rules.
- **Where it's offered (D6, settled: opt-in on both paths):** `dae_morphs=bake|keep`
  - as a converter query parameter, only when `to` is `dae`. Anything else gives
    `param_unsupported`, and a bad value gives `param_invalid`;
  - as a 3D request field that every 3D model accepts, applied to the request's Collada
    and recorded in the `DeliveryPlan`. Keep-nothing with `keep` gives 400
    `option_needs_keeping`.

  The parameter's text says three.js and assimp refuse `keep` files, and SceneKit and
  Godot read them. (An earlier draft kept generations out because "the dashboard's
  viewer opens their dae". That was wrong: the dashboard previews glTF only,
  `canPreviewMesh`.)
- **The reader is untouched.** It's the round-trip oracle. Two consequences, both from
  the builder check:
  - **Normals go inside `<vertices>` in morphed geometries.** The reader takes normal
    offsets only from a `NORMAL` input inside `<vertices>`, in both the base and the
    target geometries (`dae.rs`, `morph_offsets`). Today's writer puts `NORMAL` on
    `<triangles>`. Under `Keep`, a morphed mesh's base and target geometries carry
    `POSITION` and `NORMAL` inside `<vertices>`. Unmorphed geometries keep today's
    layout, so `Bake` stays byte-identical.
  - **A morphed mesh with several primitives comes back as several meshes.** The writer
    emits one `<geometry>` per primitive (three.js needs that), SceneKit keeps one
    controller per node, and the reader gives each morph controller its own mesh and
    node. So a three-primitive face round-trips as three morphed meshes. Each has the
    same target names and default weights, and the posed geometry is identical. That's
    accepted and documented. Merging them back in the reader is a reader change, out of
    scope.
- **SceneKit is the oracle, and what it plays is established first.** The only
  SceneKit morph check recorded in the tree is a static RELATIVE morph, with the
  weight animation removed and no skin (`testdata/dae/README.md`). Weight animation
  and skin over morph have never been checked in SceneKit. So the plan opens with a
  spike: hand-written files rendered by SceneKit at sample times. Its result decides
  what ships:
  - SceneKit plays morphs, weight animation and skin over morph → `Keep` as designed;
  - it plays the targets but not the weight animation → `Keep` still writes the
    animation, which Collada holds. SceneKit judges the rest pose and default weights
    only, with the citation, and the round trip through our reader judges the
    animation;
  - it doesn't skin over morph → a skinned and morphed mesh under `Keep` bakes its
    morphs (today's behaviour for that mesh only), and that's reported;
  - it reads no morph at all → `Keep` doesn't ship. The item stops, and the user is
    told, since then no reader that matters plays litegen's morphs.

## Non-goals

- Changing the default.
- Morph targets on UV or colour attributes. The scene has position and normal offsets
  only.
- Collada output for three.js with morphs. That needs a three.js change, and it's
  recorded in the reader table.

## What proves it is done

### Unit tests (dae.rs, RED first)

- `kept_morphs_round_trip_through_our_reader`: the `morph` fixture written with `Keep`
  and read back gives:
  - equal target count and names;
  - every target's position and normal offsets within 1e-6 of the fixture's;
  - equal default weights;
  - the weight animation's keys equal at every `fixtures::sample_times`;
  - no `morph_targets` loss.
- `kept_morphs_under_a_skin_round_trip`: a skinned and morphed scene (the `skinned`
  fixture plus one target on its tube) gives `<skin source="#…-morph">`, and after the
  round trip the posed geometry at every sample time is within 1e-5 of the fixture's
  (`pose::world_primitives`). This applies only if the spike shows SceneKit skins over
  morphs; otherwise the test asserts the documented bake and loss.
- `kept_morph_normals_sit_in_vertices`: under `Keep`, the base and every target
  geometry of a morphed mesh carry `NORMAL` inside `<vertices>`, and the round trip
  restores normal offsets within 1e-6.
- `a_multi_primitive_morph_comes_back_as_meshes`: a three-primitive morphed mesh
  round-trips as three meshes. Each has the fixture's target names and default
  weights, and the posed geometry at every sample time equals the fixture's within
  1e-5.
- `bake_is_todays_output`: with `Bake`, the output is byte-equal to today's for every
  fixture. Record the hashes before the change.
- `kept_output_validates_against_the_xsd`: the harness runs `xmllint --noout --schema
  collada_schema_1_4_1.xsd` on every `Keep` file (the schema is fetched by the harness,
  not committed). 0 errors.
- The scene matrix: `scene_matrix_tests` adds a `dae:keep` write mode for every
  fixture. Morph-weight animation survives exactly where the capability says it does.

### Converter API

- `dae_morphs_keep_writes_controllers`: `from=glb&to=dae&dae_morphs=keep` on the
  `morph` fixture gives a body containing `<morph `, and the losses header lacks
  `morph_targets`.
- `dae_morphs_only_for_dae`: `to=obj&dae_morphs=keep` gives 400 `param_unsupported`.
  `dae_morphs=yes` gives 400 `param_invalid`.
- The OpenAPI test documents the parameter and the field.
- **Generations** (tests/model3d_api.rs):
  - `dae_morphs: "keep"` on a mock morph model with `output_formats: [glb, dae]` gives a
    Collada with `<morph>` and a GLB unchanged;
  - the default is byte-identical to today;
  - keep-nothing with `keep` gives 400 `option_needs_keeping`;
  - the poller path honours the recorded plan.

### Render harness and oracles

- A `dae:keep` variant of every direction into Collada:
  - **SceneKit** renders it at rest and at every sample time, against the expected
    picture (the fixture with its morphs), within today's thresholds;
  - **three.js** `ColladaLoader` must fail to load it (the harness records it as
    refused, citing `ColladaComposer.buildNode`). If it ever loads, the harness fails the
    case with "documented reader limitation no longer holds", so the docs get updated;
  - **assimp**: the same rule, citing `ColladaParser::ReadController`. The research
    found assimp master still fails, so no later release is used.
- Negative controls:
  - negate one target's offsets in the writer: SceneKit's comparison at animated times
    fails, as the USD negative control failed by 7–29 %, and
    `kept_morphs_round_trip_through_our_reader` fails;
  - write RELATIVE with absolute targets (the ambiguous form): the round-trip test fails.

  Restore each by rewriting.

### Docs

- `dae.rs` module doc: "Morph targets are baked unless asked for".
- `mesh/mod.rs` table, Collada row: "morph targets (applied at their weights unless
  written with `dae_morphs: keep`…)".
- `3d-format-conversion.md` §7.
- The converter's and the request's OpenAPI text, the SDK types (regenerated), and
  `docs/mcp/tools.md`.

### Gate

`cargo test --lib -- mesh::dae mesh::scene_matrix_tests`, the convert tests, then the
overview's full gate, and the harness with `--formats dae` plus SceneKit.

## Left to the builder

- The exact SID and id scheme of the controllers, within the writer's existing
  NCName rules.
- Whether animated weights share one `<animation>` per controller or one per weight.
