# Material Layers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The scene carries glTF's nine PBR layer extensions. glTF keeps them all, USD, OBJ and Collada keep what they can, FBX reads them, and every drop is a named loss.

**Architecture:** Optional layer structs on `Material`, one `TextureSlot` per layer texture (so existing texture passes cover them), and `Capabilities.layers: LayerSupport`, which `losses` and `prepare` follow.

**Tech Stack:** Rust (`mesh/`), the Node render harness (a new `lit` mode with three.js `MeshPhysicalMaterial`, usdrecord, the Khronos validator).

**Spec:** `docs/superpowers/specs/2026-11-04-mesh-material-layers-design.md`

**Status: done on 2026-11-05 by session litegen-b2, not committed.** The ledger,
`.superpowers/sdd/2026-11-04-mesh-material-layers/progress.md`, records every ruling with
its cost if wrong and its evidence. The ledger isn't committed, so the rulings are also
listed at the end of this block, one line each. Code comments and docs cite them by ID.
The build differs from the steps below in these places:

- **Task split.**
  - Task 4 ran as 4a (OBJ and Collada) and 4b (FBX) (A23).
  - Task 5 ran as 5a (the lit harness) and 5b (the imports and the native check) (A26).
  - Task 6b, the hostile-input sweeps that the overview's done-bar line 7 requires, was
    added (A40).
- **Interfaces.**
  - `Material::texture_slots_mut()` walks every slot, base and layer. Item 11's
    `texture_refs_mut()` stays for remaps (A1).
  - Every layer slot has a snake_case name in `inspect::slot_name`, so the delivered
    manifest's `maps` lists layer maps (A10, OpenAPI-visible).
  - `scene` has one `KHR_materials_ior` schema check, which every reader uses.
  - `pose::degrade_with_materials` lets OBJ and USD prepare without a second scene copy.
- **Reader value rules** (A16 and its sub-rulings):
  - A malformed layer is dropped with a warning, and the file converts. So is one whose
    texture is broken and used only by layers (A16a–A16d, A16c revised).
  - An ior outside `KHR_materials_ior`'s schema is dropped with a warning (A16b).
  - Every reader treats its format's exporter defaults as unset:
    - OBJ: A21b, A21f;
    - Collada: A22a, plus A42 for ≤ 0;
    - FBX: A24b;
    - USD: A41, a clearcoat of 0 and an ior of 1.5.
- **USD:**
  - The coat normal is the base normal map, as UsdPreviewSurface's spec says (A8a).
  - A textured coat keeps the base-map pattern (A25).
  - An ior of 0 is written like any other (A16g, revised).
  - A coat over an ior other than 1.5 is reported (A16h).
- **OBJ:**
  - It reads and writes Blender's PBR MTL extension (A21–A21j).
  - `obj::prepare` models what an MTL holds of sidedness and alpha (A36).
- **Collada:** reads `<index_of_refraction>`. 1, 1.45 and 1.5 read as unset (A22a). A
  value ≤ 0 reads as unset with a warning, and an ior of 0 is reported on write (A42).
- **FBX:** reads Arnold/Standard Surface, 3ds Max Physical and OpenPBR layers. It maps
  only what a cited source establishes and warns the rest (A24–A24h).
- **Warnings are budgeted** (A45):
  - USD clips each message;
  - OBJ has `MAX_WARNINGS`, and clips the names and paths it quotes;
  - glTF no longer serializes a bad value whole;
  - FBX clips the names it quotes.
- **The harness:**
  - `lit` and `lit_coat` run at a 1/255 tolerance, with per-layer visibility measured.
  - Every judge is shown failing under a control (A27).
  - usdrecord is a recorded non-judge (A32).
  - It uses one camera per fixture, not the spec's four poses (A34).
  - The native check has a lit pass (A35), and its rotation excuse is bounded to 1e-6.
  - Alpha skips are cited (A37–A37b), and a USD case left with no geometry judge fails.
  - SceneKit albedo is judged on 4×4 blocks (A38), and one ToyCar case is a cited skip
    (A39).
  - `layered()` gained tangents in glTF's handedness and a lacquer material (A29, A31,
    A31a).
  - The `--import` run compares each glTF output's layer extensions with its source's,
    field by field as f32: `scripts/mesh-verify/layer-extensions.mjs` (A20, A20a, A43).
- **Tests beyond the plan:**
  - the hostile-input sweeps: six suite tests over lean inputs, and four `#[ignore]`d
    `…_full` tests over full-size ones. They share `fixtures::sweeps` (A40a–A40e);
  - `scene_matrix_tests` compares every material's layer values, f32-exact, in every pair
    and both write modes;
  - the per-task tests the reports list.
- **The final review** (0 Critical, 3 Important, 8 Minor) found three problems:
  - USD read its exporters' defaults as layers;
  - Collada's ior 0 drew a mirror;
  - the A20 check lived only in the ledger.

  One fix wave fixed all three, the eight Minors, and the cheap deferred ones (A46). Its
  scoped re-review found one evidence gap: the `--import` layer-extension pass had never
  been seen failing. The controller closed it with two altered outputs, each failing its
  case. One was visible to `lit`, and one, an added dispersion layer, was caught by the
  pass alone. The residuals are ruled as A47a and A49.

Proof, on tree 6879167 (the final tree):

- **Full gate:**
  - lib: 2,366 passed, 0 failed, with the three load-sensitive tests skipped;
  - those three alone: 3/3;
  - `--tests --no-fail-fast`, with the three skipped (A48) and a real Postgres: 56
    suites, 3,218 passed, 0 failed;
  - clippy `--all-targets --all-features -D warnings`: clean;
  - `mesh::isolation_tests`: 3/3.
- **Every-fixture harness:** 912/912 on the release binary `69fd9869…99cb8`. All 912
  verdicts are the same as in the run before the fix wave. The final tree's rebuilt binary
  writes byte-identical outputs: all 1,415 files.
- **Khronos `--import`:** the sixteen models pass 1,824/1,824, and all 16 native checks
  pass. The layer-extension pass is 112/112.
- **Controls:** each of these fails when it should:
  - every lit judge (A27);
  - the native rotation bound (a 1° turn);
  - the no-geometry-judge guard;
  - the A5 validator rule;
  - five mesh mutants in the fix wave.
- **Default output:** byte-identical for every fixture × container × mode (the P6
  listing, 160 of 160 lines, `layered` included).
- **Hostile input:** about 1.17 million readings, with no panic. The slowest single case
  in release is 1.34 ms.

To commit on the other laptop (item 11's files overlap, so commit item 11 first or both
together):

- `litegen-core/src/mesh/`:
  - `dae.rs`, `fbx.rs`, `fbx_read.rs`, `fixtures.rs`, `glb.rs`, `inspect.rs`,
    `normalize.rs`, `obj.rs`, `pose.rs`, `scene.rs`, `scene_matrix_tests.rs`, `usd.rs`,
    `usd_read.rs` and `usd_write.rs`;
  - `mod.rs` (item 4's lines are the layers column, the OBJ prepare hook and the layer
    rows; other items edit the same file);
- `litegen-core/src/types/mod.rs` (the `SceneMaterial.maps` doc, A10);
- `litegen-core/src/proxy/model3d_deliver.rs` (three test lines, A19);
- `litegen-core/src/api/handlers/convert.rs` (one OpenAPI phrase, A18);
- `scripts/mesh-verify/`: `verify.mjs`, `page/render.js` and `README.md`, plus the new
  `layer-extensions.mjs` and `layer-extensions.test.mjs`;
- `docs/superpowers/3d-format-conversion.md` (item 4's lines are in §1 and §3–§9; other
  items edit the same file);
- this plan, its spec, and the overview's row 4.

After the commit, the frontend/SDK owner regenerates for A10's and A18's OpenAPI texts:
`sdks/scripts/regen-all.sh`, then `node apps/landing/scripts/sync-openapi.mjs`.

**Rulings.** Code comments and docs cite these IDs ("Ruling A24h", "(A36)"). Each line gives
the decision, then what it costs if it's wrong. The ledger,
`.superpowers/sdd/2026-11-04-mesh-material-layers/progress.md`, isn't committed. It has
each ruling in full, with its evidence.

*The pre-flight scan and Task 1 (the scene types):*
- **A1.** Item 11's `texture_refs_mut()` stays for remaps. `Material::texture_slots_mut()`
  walks every slot, base and layer, for the sites that drop or replace a binding.
  *If wrong:* one method name differs from the plan.
- **A2.** Every site that listed the base slots now passes over all of them (dae.rs,
  normalize.rs, pose.rs, usd.rs, usd_read.rs, and fbx.rs's portability drop). The
  per-format mappings that name slots stay. *If wrong:* a mapping might read better
  rewritten.
- **A3.** The transform-claim pass keeps its priority order. *If wrong:* a layer texture
  can't claim a uv set ahead of a base texture.
- **A4.** In Task 1, every format's `layers` was `none()`, glTF included, until each
  codec's task. *If wrong:* nothing.
- **A5.** `layered_glb_validates` is a harness check: gltf-validator over layered's GLB and
  glTF, with 0 errors and 0 warnings. *If wrong:* nothing; the spec says "run in the
  harness".
- **A6.** This one is provisional until item 3's session agrees. The KTX2 layer-texture
  test uses a KTX2 whose header names zlib supercompression. *If wrong:* item 3 changes
  one assertion.
- **A7.** Item 4 changed only one slot list in normalize.rs (`transcode_textures`'
  dead-texture drop). *If wrong:* one hand merge in one loop.
- **A8.** `LayerSupport` stays nine bools. When a container keeps a layer but can't hold
  one of its parts (USD's coat normal map), the codec's `prepare` removes that part before
  `degrade` and reports it. *If wrong:* one more USD loss line.
- **A8a.** UsdPreviewSurface's coat uses the base normal map (its spec: "Clearcoat results
  are calculated using the same normal map data used by the primary specular lobe").
  *If wrong:* a glTF coat with its own normal map loses it in USD, reported.
- **A9.** `scene_matrix_tests` runs `layered()` in every pair and both write modes. Each
  layer survives exactly where the pair's `LayerSupport` says, and the losses name the
  rest. *If wrong:* nothing.
- **A10.** Every layer slot has a snake_case name in `inspect::slot_name`, so the
  manifest's `maps` lists layer maps. The `SceneMaterial.maps` doc names them
  (OpenAPI-visible). *If wrong:* one description line to revert.
- **A11.** Task 1 started during item 11's closing harness. *If wrong:* a pause and one
  rebase.
- **A12.** Between Tasks 1 and 2, glTF's `prepare` ran `pose::degrade`. *If wrong:* GLB →
  GLB output for layer-only textures differed between Tasks 1 and 2. Nothing was committed
  in between.
- **A13.** The full `Capabilities` literals in usd.rs, dae.rs and fbx.rs gain
  `layers: LayerSupport::none()`. *If wrong:* nothing.
- **A14.** A layer loss reads "{t} has no {layer} layer; the base layer is kept" when the
  target has materials, and "{t} has no {layer} layer" otherwise. *If wrong:* one clause
  of loss text.
- **A15.** The conflicting-transform loss counts every binding, as before. So a map the
  target drops anyway is named twice, or three times on USD. *If wrong:* extra lines in the
  losses header until a follow-up counts only kept slots.

*Reader value rules:*
- **A16.** Layer values follow the base factors' rule: `validate` checks finiteness and
  ior's schema only. *If wrong:* a third-party glTF's out-of-range layer values round-trip
  unchanged.
- **A16a.** A layer extension with a malformed field or textureInfo is dropped whole, with
  a warning naming the extension, and the file converts. *If wrong:* a malformed optional
  layer converts as base-only instead of refusing the file.
- **A16b.** An ior outside `KHR_materials_ior`'s schema (neither 0 nor at least 1) is
  dropped with a warning, not refused. *If wrong:* such a file converts without its ior.
- **A16c** (revised in the final fix wave). A16a covers the layer's own JSON. A layer whose
  texture resolves to a broken texture-table entry that no base slot uses is also dropped
  with a warning, so no glTF that converted before item 4 is refused now. *If wrong:* such
  a layer converts as base-only.
- **A16d.** A layer whose own JSON fails is a gap under A16c, and it's closed. *If
  wrong:* nothing.
- **A16e.** A non-finite USD clearcoat, clearcoatRoughness or ior is dropped with a
  warning, and the file converts. *If wrong:* such a file converts without that input.
- **A16f.** "Dropped" means read as unauthored, with a warning. *If wrong:* a USD coat
  taken from a channel other than r is lost, with a warning.
- **A16g** (revised). An authored USD `inputs:ior = 0` reads as glTF's 0, since both give
  F0 = 1, and USD writes an ior of 0 like any other. *If wrong:* a USD reader that gives
  ior 0 another meaning draws such a material wrong.
- **A16h.** The USD writer keeps writing the ior. A clearcoat over an ior other than 1.5 is
  reported, because UsdPreviewSurface's coat uses the base ior. *If wrong:* one more loss
  line for coated materials with an ior other than 1.5.
- **A16i.** A non-finite or non-numeric Collada `<index_of_refraction>` or MTL layer value
  is dropped with a warning, and the file converts. *If wrong:* such a file converts
  without the value.

*Tasks 2–4b (the codecs):*
- **A17.** Task 3 went ahead on today's USD value types, without item 6's `Value::Token`
  change. *If wrong:* item 6 redoes a few usd_read.rs matches by hand, which litegen-fc
  accepted.
- **A18.** convert.rs's OpenAPI example for `X-Litegen-Conversion-Warnings` changed from
  "(transmission, lights, …)" to "(lights, material variants, …)", now that transmission
  is carried. *If wrong:* one stale example.
- **A19.** Three model3d_deliver.rs test lines now name `KHR_materials_variants`, which is
  still warned. *If wrong:* nothing; it's test data.
- **A20.** The Khronos import check compares each material's layer extensions by meaning,
  not as raw JSON. Since the final fix wave it's in `scripts/mesh-verify` (A43). *If
  wrong:* an extras or nested-extension drop inside a layer object is listed, not failed.
- **A20a.** A20 compares float fields as f32. *If wrong:* nothing for rendering.
- **A21.** OBJ's `Ni` maps to an ior only in a PBR MTL (one with any of Pr, Pm, Ps, Pc,
  Pcr, aniso, anisor, map_Pr or map_Pm). *If wrong:* a classic MTL's deliberate Ni isn't
  carried.
- **A21a.** Blender writes `Pc` as 4 × Coat Weight and reads it back divided by 4.
  litegen does the same. *If wrong:* an MTL from another writer that meant `Pc` as a 0–1
  factor reads at a quarter of its strength.
- **A21b.** A layer at its off value isn't written or read. Blender writes Pc, Pcr, Ps,
  aniso and anisor on every material, zeros included. *If wrong:* a glTF layer explicitly
  present at zero weight doesn't survive OBJ, though it's invisible either way.
- **A21c.** `Ni` ≤ 0 in a PBR MTL is left out with a warning, as Blender does. *If wrong:*
  KHR's ior 0 doesn't survive OBJ (it's reported).
- **A21d.** An absent PBR value takes Blender's Principled BSDF default. *If wrong:* a
  sheen read from MTL draws rougher than glTF's default of 0 would.
- **A21e.** OBJ gets `obj::prepare`, through a three-line hook in mod.rs. *If wrong:*
  nothing.
- **A21f.** OBJ's `Ni` follows Collada's exporter-default rule, so the two agree: 1, 1.45
  and 1.5 read as unset. *If wrong:* a deliberate Ni of exactly 1, 1.45 or 1.5 isn't
  carried, though 1.5 draws the same.
- **A21g.** A three-value `Ps r g b` is the sheen colour, as assimp reads it. *If wrong:*
  nothing for one-value files.
- **A21h.** OBJ writes sheen as `Ps w w w`. *If wrong:* nothing.
- **A21i.** Transmission isn't kept in OBJ. A PBR MTL's `Tf` weight above 0 is warned on
  read. *If wrong:* a Blender PBR MTL's glass converts opaque, with a warning.
- **A21j.** A PBR MTL's `Kt` above 0 is warned the same way as `Tf`. *If wrong:* nothing.
- **A22.** Collada's `<index_of_refraction>` maps to an ior, unless it's a common
  exporter's cited default. *If wrong:* a deliberate ior that equals an exporter default
  isn't carried.
- **A22a.** Collada exporters write `<index_of_refraction>` by default, so 1, 1.45 and 1.5
  read as unset:
  - SceneKit writes 1;
  - Blender writes its Principled IOR, 1.45 in 2.8–3.6 and 1.5 since 4.0;
  - OpenCOLLADA writes it whenever it's ≥ 0.

  *If wrong:* a deliberate 1, 1.45 or 1.5 isn't carried.
- **A23.** Task 4 ran as two dispatches: 4a (OBJ and Collada) and 4b (FBX). *If wrong:* one
  more review seat.
- **A24.** The FBX reader maps a layer property only where a cited source establishes its
  meaning and units, and warns the rest. The sources are ufbx's tables, the Standard
  Surface and OpenPBR specs, and Autodesk's Physical docs. *If wrong:* an unmapped layer is
  lost with a warning instead of approximated.
- **A24a** (revised). FBX dispersion isn't mapped. It's warned only when transmission is
  above 0 and the surface isn't thin-walled. *If wrong:* a source's dispersion is lost,
  with a warning.
- **A24b.** An FBX ior equal to the family's documented default is unset, such as
  Standard Surface's 1.52 and Physical's 1.52. *If wrong:* a deliberate ior equal to a
  default isn't carried.
- **A24c.** FBX volume isn't mapped. *If wrong:* the refraction and absorption of FBX glass
  are lost, with a warning.
- **A24d.** The transmission factor is the weight, and the base colour is unchanged, as
  Blender does. *If wrong:* a default Arnold glass draws 20 % grey in glTF, with a warning.
- **A24e.** FBX anisotropy isn't mapped. *If wrong:* brushed-metal highlights are lost,
  with a warning.
- **A24f.** Standard Surface's specular and Physical's reflectivity map to the specular
  factor, when it's not 1 and lies within [0, 1]. *If wrong:* a tinted specular is lost,
  with a warning.
- **A24g.** The coat's base-adjusting parameters warn only when they differ from their
  family's default. *If wrong:* a default Physical or OpenPBR coat's base adjustment goes
  unwarned.
- **A24h.** glTF's transmission uses the base roughness, so an FBX transmission roughness
  of its own is warned, not carried. *If wrong:* frosted FBX glass reads as clear, with a
  warning.
- **A25.** The USD writer keeps the base-map pattern for a textured coat: a connection,
  with the factor in the texture's scale. *If wrong:* three.js-based USD viewers draw a
  textured coat as no coat. That's a reader limit, cited.

*Tasks 5a and 5b (the harness):*
- **A26.** Task 5 ran as two dispatches: 5a (the lit harness) and 5b (the imports and the
  native check). *If wrong:* one more review seat.
- **A27.** A lit comparison is a judge only if a negative control makes it fail, and its
  separation is recorded. *If wrong:* a reader that could have judged loosely is recorded
  as a non-judge.
- **A28, A28a, A28b.** While opus and sonnet subagents were over their weekly limits,
  seats moved to fable. They went back to opus from Task 5a's review. *If wrong:* some work
  ran on a less-proven tier, though it was still reviewed.
- **A29.** layered()'s car paint gets a TANGENT set, since the validator error was the
  fixture's. Generating tangents in the writer is a follow-up. *If wrong:* an invalid
  source's missing tangent space passes through unrepaired.
- **A30.** The cited assimp specularSlot exemption stays. It's a reader bug, and it applies
  only when the expected file is refused alike. *If wrong:* a litegen specular regression
  that only assimp would catch is masked, when the expected file is refused too.
- **A31, A31a.** layered() gains a lacquer material (a black base, a coat and an ior), so
  three.js's USDLoader judges a USD coat. *If wrong:* one more material in layered().
- **A32.** usdrecord's dome-lit render stays a non-judge, because it separates by about 2
  points, at the tolerance's edge. *If wrong:* a weak but real judge goes unused.
- **A33.** The harness doesn't change for a glTF writer that drops a layer. That's pinned
  by `every_pbr_extension_round_trips` and by A20. *If wrong:* a writer drop that escapes
  both goes unseen by the harness.
- **A34.** The harness has one camera per fixture, not the spec's four poses. *If wrong:*
  nothing; there are fewer views than the spec imagined.
- **A35.** The native check gains a lit pass through one shared camera, at threshold 0.
  *If wrong:* nothing.
- **A36.** `obj::prepare` models what an MTL holds of sidedness and alpha. *If wrong:* an
  OBJ consumer that reads map_d gets no alpha from litegen.
- **A37, A37a, A37b.** SceneKit's and usdrecord's albedo and silhouette are cited skips,
  gated per file, where the expected scene has a translucent material, texture alpha or
  vertex alpha. *If wrong:* 298 SceneKit and usdrecord views of translucent files go
  unjudged.
- **A38.** SceneKit albedo is judged on 4×4 block means (tolerance 24), beside the
  per-pixel record. *If wrong:* SceneKit misses a texture error smaller than a block.
- **A39.** ToyCar single → usda's lit_coat is a cited, measured skip. *If wrong:* one
  curved-coat case goes unjudged.

*Task 6b (hostile input) and the final fix wave:*
- **A40.** Task 6b adds the hostile-input sweeps that done-bar line 7 requires. *If
  wrong:* nothing; without it line 7 is unproven.
- **A40a.** The suite sweeps lean inputs built from layered(). Each keeps its codec's layer
  materials, textures and values, on one triangle with 1×1 images. The full-size sweeps
  are `#[ignore]`d `…_full` tests, run once in release. *If wrong:* an interaction between
  layer data and large geometry goes unrun in the normal suite.
- **A40b.** glTF's lean sweep splits into a GLB half and a one-file half, to keep the
  suite fast. *If wrong:* one more test name.
- **A40c.** Small extra inputs reach three more paths: USD's coat-map bias, the coat normal
  over a base normal map, and a glTF layer map's `KHR_texture_transform`. *If wrong:*
  nothing.
- **A40d.** The sweeps share one helper and lean scene, `fixtures::sweeps`. *If wrong:*
  nothing.
- **A40e.** The seeded sample is drawn from the whole file, as item 11's sweeps do, and
  every layer span also gets exhaustive single flips. *If wrong:* read literally, line 7's
  "10,000 elsewhere" isn't met for inputs over 4 KiB.
- **A41.** USD reads an unconnected `inputs:clearcoat` of 0 as no layer, and an ior of
  exactly 1.5 as unset, silently. This matches how the other readers treat exporter
  defaults, and `usd::prepare` drops both the same way. *If wrong:* a glTF `{}` clearcoat
  or ior doesn't survive USD and isn't reported, though it's invisible either way.
- **A42.** A Collada `<index_of_refraction>` ≤ 0 reads as unset with a warning. An ior of 0
  is left out of Collada output and reported. *If wrong:* litegen's own glTF ior 0 doesn't
  survive Collada (it's reported).
- **A43.** The A20 check lives in `scripts/mesh-verify` and runs per case under
  `--import`. *If wrong:* one more harness pass to maintain.
- **A44.** Ruling IDs in code and docs resolve to this list. *If wrong:* a reader follows
  a pointer instead of reading an inline reason.
- **A45.** Item 4's warnings are budgeted:
  - USD clips each message at 1,000 characters;
  - OBJ has a `MAX_WARNINGS` budget, charged before each push, and clips the names and paths
    it quotes;
  - glTF no longer serializes a bad value whole before cutting it;
  - FBX clips the material names its notes quote.

  *If wrong:* long names in warnings are shortened, and an OBJ past the budget reports a
  count instead of the rest.
- **A46.** The final fix wave carried the review's issues, its value-level matrix check
  and the cheap deferred minors. The rest are follow-ups. *If wrong:* the wave was bigger
  than it needed to be.
- **A47.** A file whose layer-only images push it over the image budget stays refused. The
  budget is the bound against oversized inputs. *If wrong:* such a file, which converted
  before item 4 read layers, is refused because it needs more image memory than the budget
  allows.

## Global Constraints

- The overview's "Rules every plan carries" apply.
- Field names, defaults and units are glTF's, from each KHR extension's spec: `ior` 1.5, `attenuation_distance` +∞, and `specular.factor` 1 with `color` [1, 1, 1].
- Layers go in `extensionsUsed`, never `extensionsRequired`.
- USD approximates nothing: transmission, volume, specular, sheen, iridescence, anisotropy and dispersion are reported lost, and the written `opacity` stays the base alpha.
- No site may list texture slots by name after this item: every one goes through `Material::texture_refs()` or `texture_refs_mut()`. A container's list of the slots it keeps (item 11's kept-slot lists) is a statement of its capabilities, not such a site. The list derived from `Capabilities` gains each layer slot the target's `LayerSupport` keeps, after emissive.
- Item 4 lands before items 5 and 10. Keep `LayerSupport` additive so their `Capabilities` fields merge cleanly.

## Review Focus

1. A layer texture on a uv set the target drops: the existing "texture on uv set N or later" loss must count layer textures too.
2. A layer texture with a `KHR_texture_transform` different from the base colour's on the same uv set: the conflicting-transform loss must count it.
3. A KTX2 or WebP layer texture: WebP transcodes; a KTX2 one is dropped with the material's layer kept but untextured, and reported.
4. A texture used only by a layer slot, beside base textures that get pruned: after pruning, its index still points at the same image (the remap covers layer slots).
5. An `ior` at a special or boundary value that `KHR_materials_ior`'s schema allows but `UsdPreviewSurface` can't express: it round-trips in glTF, is left out of USD, and is reported.

---

### Task 1: Scene types and losses

**Files:** `litegen-core/src/mesh/scene.rs` (the layer structs, `Material` fields, `TextureSlot` variants and `texture_refs`, `validate`, `LayerSupport`, `Capabilities.layers`, `losses`), `litegen-core/src/mesh/mod.rs` (each format's `layers`), `litegen-core/src/mesh/fixtures.rs` (`layered()`, registered in `all()`).

**Interfaces:**
- Produces: `pub struct Clearcoat { pub factor: f32, pub texture: Option<TextureRef>, pub roughness: f32, pub roughness_texture: Option<TextureRef>, pub normal_texture: Option<TextureRef>, pub normal_scale: f32 }`, and likewise `Transmission`, `Volume`, `Specular`, `Sheen`, `Iridescence` and `Anisotropy`, field for field with the KHR specs.
- Produces: `Material { clearcoat, transmission, volume, ior: Option<f32>, specular, sheen, iridescence, anisotropy, dispersion: Option<f32>, .. }`, all `None` by default.
- Produces: `pub struct LayerSupport { pub clearcoat: bool, pub transmission: bool, pub volume: bool, pub ior: bool, pub specular: bool, pub sheen: bool, pub iridescence: bool, pub anisotropy: bool, pub dispersion: bool }`, with `LayerSupport::all()` and `LayerSupport::none()`.
- Produces: `pub fn texture_refs_mut(&mut self) -> impl Iterator<Item = (TextureSlot, &mut Option<TextureRef>)>` on `Material`, every slot, base and layers.

- [ ] Write `layer_textures_are_texture_refs`, `layer_factors_must_be_finite`, `ior_outside_the_schema_is_refused` (bounds pinned from the KHR_materials_ior schema, cited), `losses_name_each_missing_layer`, and Review Focus 1–2 as `layer_textures_on_dropped_uv_sets_are_lost` and `layer_transforms_conflict_like_base_ones`.
- [ ] Write the six site tests from the spec (the seventh, `a_layer_texture_keeps_its_uv_set_in_usdz`, is Task 3's): `a_layer_texture_transform_is_baked`, `pruning_remaps_a_layer_texture` (Review Focus 4), `degrade_reduces_layer_textures`, `a_ktx2_layer_texture_is_dropped_and_its_layer_kept`, `usd_prepare_handles_layer_textures` and `dae_prepare_handles_layer_textures`.
- [ ] Run `cargo test --lib -- mesh::scene mesh::pose mesh::normalize`. Expected: FAIL (undefined types).
- [ ] Implement the types, then convert the sites to `texture_refs_mut`: `pose.rs` transform baking (two places), `prune_unused_textures`' remap, `degrade`'s material reduction, `normalize::transcode_textures`, `usd::prepare`, `dae::prepare`, and item 11's UV-set remap in `pose.rs` (its slot order gains the layer slots after emissive). Check with `grep -n "base_color_texture,\|&mut m.emissive_texture" litegen-core/src/mesh/*.rs`: it must show no slot lists outside `scene.rs`. Every format's `layers` is `none()` except glTF (`all()`), so this task changes no output.
- [ ] Run `cargo test --lib mesh::`. Expected: pass. `layered()` reports its layers lost in every non-glTF container, and glTF also reports them until Task 2.

### Task 2: glTF reads and writes all nine

**Files:** `litegen-core/src/mesh/glb.rs` (material reading near the emissive-strength read, material writing near `KHR_materials_unlit`, `CARRIED_EXTENSIONS`, `extension_loss`).

- [ ] Write `every_pbr_extension_round_trips` (with the hand-written expected JSON per extension) and `layers_are_never_required`.
- [ ] Run `cargo test --lib mesh::glb`. Expected: FAIL.
- [ ] Implement. The nine move onto `CARRIED_EXTENSIONS`.
- [ ] Run `cargo test --lib -- mesh::glb mesh::scene_matrix_tests`. Expected: pass.

### Task 3: USD

**Files:** `litegen-core/src/mesh/usd_write.rs` (`write_material`), `usd_read.rs` (the clearcoat warning becomes a read; ior), `usd.rs` (capabilities: `LayerSupport { clearcoat, ior, .. none }`).

- [ ] Write `clearcoat_and_ior_round_trip_through_usd`, `usd_reports_transmission_and_keeps_alpha`, `an_ior_usd_cannot_express_is_reported` (Review Focus 5), and `a_layer_texture_keeps_its_uv_set_in_usdz` (the spec's site test for item 11's UV-set plan).
- [ ] Run `cargo test --lib mesh::usd`. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test --lib -- mesh::usd mesh::usda mesh::usdc mesh::scene_matrix_tests`. Expected: pass.

### Task 4: OBJ, Collada and FBX

**Files:** `litegen-core/src/mesh/obj.rs` (MTL read and write of `Pc`, `Pcr`, `Ps`, `Ni`, `aniso`, `anisor`), `dae.rs` (`index_of_refraction`), `fbx_read.rs` (the Arnold, Physical and OpenPBR layer properties).

- [ ] Write `mtl_pbr_extension_round_trips`, `a_sheen_colour_is_reduced_and_reported`, `index_of_refraction_round_trips`, `arnold_layers_read`, `physical_layers_read` and `openpbr_layers_read`. Each FBX test cites the source of its property names.
- [ ] Run them. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test --lib mesh::`. Expected: pass.

### Task 5: The `lit` harness mode, and Khronos samples

**Files:** `scripts/mesh-verify/page/render.js` (the `lit` mode), `scripts/mesh-verify/verify.mjs` (mode selection; usdrecord with a dome light), `scripts/mesh-verify/README.md` (thresholds and calibration). Review Focus 3's KTX2 test is written in Task 1.

- [ ] Implement the `lit` mode, then calibrate: the expected file against itself, and GLB → GLB. Both must be 0.00 %. Record the thresholds.
- [ ] Run `cargo run --release --example mesh_matrix -- <dir>` (all fixtures, `layered` included) and `node verify.mjs <dir> <report>`. Expected: 0 failures, and skips cite loader lines.
- [ ] Fetch the sixteen Khronos models from the spec into a local folder outside the repo. Run `--import`. Expected:
  - GLB → GLB keeps every extension object (JSON-equal after key sort);
  - the validator reports 0 errors;
  - the native checks pass.
- [ ] Negative controls: (a) `clearcoatRoughness` written into `clearcoat`; (b) sheen dropped from the glTF writer. Each fails its named tests and the lit cases. Restore by rewriting.

### Task 6: Docs and gate

- [ ] Update the docs listed in the spec. Send litegen-a3 the matrix row changes.
- [ ] Run the overview's full gate. Write the ledger: RED→GREEN names, both negative controls, the calibration numbers and the harness summaries.
