# More than two UV sets in USD and Collada output — design

Part of the mesh-gaps set ([overview](2026-11-04-mesh-gaps-overview-design.md)). Size: S–M.
Revised 2026-11-04 after the builder check. The USD reader reads sets differently from
what the first draft assumed, and that changes the scope. Amended the same day during
the build: decisions 2 and 6 and the Collada test follow litegen-37's rulings P3a–P3c
and P4 (`.superpowers/sdd/2026-11-04-mesh-uv-sets-usd-dae/progress.md`).

## Why

USD and Collada can carry any number of texture coordinate sets, but the writers stop
at two:

- **USD:** `usd::capabilities` sets `uv_sets: 2`, and `usd_write.rs` loops
  `p.uvs.iter().enumerate().take(2)`. The module doc gives the reason as "two uv sets
  (RealityKit's limit)".
- **Collada:** `dae::UV_SETS = 2`. Its doc gives the reason as three.js reading set 0 as
  `uv` and set 1 as `uv1`, "and nothing beyond" (`parseGeometryPrimitive`,
  `buildGeometryType`).

A GLB with five sets loses three in USD and Collada, and `losses` reports "3 of 5 uv
set(s)".

There's a second, older problem that this item has to fix too.
**`pose::degrade` keeps the first N sets, not the ones textures use.** A GLB whose base
colour samples `TEXCOORD_1`, converted to OBJ (one set), loses its texture, reported as
"texture binding(s) on uv set 1 or later". Set 0, which nothing samples, is what's kept.
Collada and USDZ hit the same wall once a texture sits past their cap.

## Where it stands today: the USD reader

`usd_read.rs` doesn't read sets by index:

- When the bound material's textures name primvars (`UsdPrimvarReader_float2`
  `inputs:varname`), it reads exactly those, numbered in the order they are first
  referenced (`uv_names`, near `usd_read.rs:726`; `TexRead`, near `:1594`).
- Otherwise it reads `st` (or the first `texCoord2*` primvar) and `st1`
  (`default_uv_names`, near `:1016`).

So a mesh with `st`…`st3` whose material reads `st3` and `st1` reads back as two sets
(`st3` → 0, `st1` → 1). Writing more sets alone can't round-trip.

## Decisions (mine)

1. **The USD reader numbers sets by name.** The sets of a mesh are:
   - first, `primvars:st`, `st1`, …, `st7` that exist, in suffix order, with gaps
     closed;
   - then, any other primvar a bound texture reads (for example Blender's `UVMap`), in
     first-reference order;
   - up to 8 in all.

   A texture's `uv_set` is its varname's position in that list. When the mesh has no
   `st` at all, today's fallback is kept: the first `texCoord2*` primvar is set 0. For
   litegen's own files this is the identity, so round trips are exact. For third-party
   files it changes numbering only where the file has `stN` primvars no material reads
   (those are now kept), or a material that reads `st1` alone (its texture is now on
   set 1, beside `st` as set 0). Decision 2 keeps that case working in one-set targets.
2. **`pose::degrade` keeps the sets textures use.** When a target holds k sets and a
   primitive has more, the sets its material samples **through slots the target keeps**
   come first, in first-use order, then the rest by index, and the first k are kept.
   The kept slots follow the target's material model, in this order:
   - PBR targets: base colour, metallic-roughness, normal, occlusion, emissive;
   - Phong targets: base colour, normal, emissive;
   - colour-only targets: base colour.

   One plan helper derives them from the target's `Capabilities`, and `degrade` and
   `scene::losses` share it. A target that keeps no textures (PLY, STL, and single-file
   OBJ, Collada and USDA) keeps the first k, as today. The material's
   `TextureRef.uv_set`s are remapped to match. Because the remap is a function of the
   material, primitives that share a material share the remap. Textures are dropped only
   when a material samples more distinct sets than k through kept slots, and they're
   reported as today. This fixes the `TEXCOORD_1` → OBJ case for every format.

   The first draft ordered all five slots for every target (amended per rulings P3a and
   P3c). A map the target deletes uses no set in its output, yet it could take the only
   kept set from one the target keeps. For example, an OBJ with emissive on set 0 and
   occlusion on set 1 would keep set 1 and lose the emissive map, which today's code
   keeps.
3. **USDA, and USDC from item 6, write up to eight sets:** `primvars:st`, `st1` …
   `st7`, as `texCoord2f[]`, with the same interpolation and V flip as `st` today.
4. **USDZ keeps two.** USDZ is AR Quick Look's and RealityKit's format, and Apple
   documents two UV sets ("Multiple UV sets: partial — two UV sets",
   `research-usd-3mf-dae.md` §A2). With decision 2, a USDZ keeps the two sets its
   textures use, so the case that matters on device, a texture on a later set, now
   works there too. `usd::capabilities` already takes the format: USDZ 2, USDA and
   USDC 8. If [`research-consumers.md`](../research/research-consumers.md) §7 shows RealityKit reads more, a later change
   can raise it.
5. **Collada writes up to eight sets,** `TEXCOORD set="0"…"7"`, each its own `<source>`
   in f64, in order. `bind_vertex_input` names whichever set a texture uses.
   three.js's `ColladaLoader` reads two (cited in `dae.rs`), and that is written into
   the module doc as a reader limit. Its decision 2 counterpart doesn't apply: Collada
   now holds eight, so nothing is degraded.
6. **Collada and FBX put the sampled sets first** (added from [`research-consumers.md`](../research/research-consumers.md)
   Q7). three.js r183's `ColladaLoader` and `FBXLoader` sample every texture from set
   0, whatever the file binds. So `dae::prepare` and `fbx::prepare` order each
   primitive's sets by decision 2's sampled-set order (base colour's set first) even
   when nothing is dropped, and remap the bindings. Each passes the slots it keeps:
   Collada base colour and emissive (`dae::reduce_material` also drops normal maps),
   FBX base colour, normal and emissive. SceneKit, assimp and the Autodesk SDK follow
   the remapped bindings, so they draw the same. three.js now draws a base colour that
   sat on a later set correctly. A scene whose textures already sit on set 0 writes
   byte-identically. The research found nothing that warns on a third set (usdchecker,
   SceneKit and Storm handled 10; assimp and Blender cap at 8). Only textures on later
   sets break, and this decision and decision 2 cover them.

   **3MF does the same, with base colour as its one kept slot** (added per ruling P4).
   It's a one-set textured container whose own `prepare` bypasses `degrade`, and it
   textures only set 0. Without the reorder it would lose a base colour on a later set.
7. **The ARKit checker runs on USDZ only.** `usdchecker --arkit` judges files for AR,
   which means USDZ. USDA and USDC outputs get plain `usdchecker`. The harness changes
   to match, and the ledger records the change.

## Non-goals

- More than eight sets anywhere.
- Raising USDZ above two without device evidence.
- Remapping sets for readers that have no limit (glTF, USDA, Collada): their order is
  the scene's.

## What proves it is done

### Unit tests, RED first

- `fixtures::multiuv()`: a quad with four distinct uv sets (each a different affine map
  of the corners, so swapped or shifted sets show), a base-colour texture on set 3 and
  an occlusion texture on set 1, registered in `fixtures::all()`. Add a second variant,
  `multiuv_shared()`, where two primitives share the material.
- **USD reader:**
  - `usd_sets_are_numbered_by_name`: a hand-written USDA with `st`, `st1`, `st2` and
    `st3` and a material reading `st3` and `st1` reads four sets in suffix order, the
    base-colour texture with `uv_set == 3` and occlusion with `uv_set == 1`;
  - `a_gap_in_the_suffixes_closes`: `st`, `st2` read as sets 0 and 1, and the texture
    on `st2` gets `uv_set == 1`;
  - `uvmap_still_reads`: a Blender-style mesh with only `primvars:UVMap` read by its
    material gives one set, with the texture on it (today's behaviour kept);
  - `a_material_reading_only_st1_keeps_both`: `st` and `st1` with a texture on `st1`
    give two sets and `uv_set == 1`.
- **USD writer:** `usda_writes_and_reads_every_uv_set`: `multiuv` → USDA → read. Four
  sets equal within 1e-7 (float text, V flip), bindings equal, no `uvs` loss. Also
  `usdc_reads_every_uv_set` on a `usdcat` crate copy committed in `testdata/usd/` (our
  content).
- `usdz_keeps_the_two_sets_its_textures_use`: `multiuv` → USDZ → read. Two sets: the
  fixture's set 3 as set 0 and set 1 as set 1, with base colour on 0 and occlusion on 1.
  The `uvs` loss reads "2 of 4 uv set(s)", and there is no texture loss.
- `degrade_keeps_sampled_sets`: a GLB with base colour on `TEXCOORD_1` → OBJ (package)
  → the texture survives, on the OBJ's one set, which equals the GLB's set 1. Today
  this test fails with the texture loss.
- `a_phong_target_keeps_the_set_of_a_map_it_keeps`: an OBJ package (one set) of a
  material with emissive on set 0, occlusion on set 1 and no base colour keeps set 0
  and writes `map_Ke` (ruling P3a).
- `threemf_keeps_the_base_colour_set`: a GLB with base colour on `TEXCOORD_1` → 3MF
  keeps the texture on its one set, which equals the GLB's set 1 (ruling P4).
- `shared_materials_share_the_remap`: `multiuv_shared()` → USDZ. Both primitives get
  the same two sets.
- `a_material_sampling_three_sets_loses_one_in_usdz`: base colour on 0, occlusion on 1,
  emissive on 2 → USDZ keeps 0 and 1 (first use), drops 2, and reports the emissive
  texture lost.
- **Collada:** `dae_writes_and_reads_every_uv_set`. Four sets bit-exact, in the
  sampled-first order [3, 0, 1, 2]: the fixture's set 3 (base colour) is written as set
  0, then the rest by index. Collada keeps no occlusion map, so the fixture's set 1
  isn't sampled there (ruling P3b). The bindings name the new indices.
- `fbx_and_dae_put_the_base_colour_set_first`: a GLB with base colour on `TEXCOORD_1`
  and nothing on `TEXCOORD_0` → FBX and DAE → three.js's loaders (harness) draw the
  texture correctly, which fails today. Our reader reads the remapped indices.
- `a_scene_textured_on_set_zero_is_unchanged`: `textured()` → FBX and DAE gives
  byte-identical output to before. Check against the recorded hashes.
- `nine_sets_still_report_one_lost` for USDA and Collada.
- `scene_matrix_tests`: `multiuv` in every pair. Set counts after each pair follow the
  capabilities, texture bindings survive wherever their sets do, and bindings are
  dropped only past a material's k distinct sets.

### Render harness

- `multiuv` in every direction. The `uv` mode gains a set selector, and each surviving
  set is compared.
- three.js `ColladaLoader` and `USDLoader` render sets 0 and 1, and later sets are cited
  skips. usdrecord judges USDA sets 2–3 (texture on `st3` in the albedo mode). SceneKit
  judges Collada and USD sets 2–3: it handled 10 sets in the research's probe.
- USDZ's two kept sets render textures correctly in three.js, SceneKit and usdrecord.
  This is the case that failed before.
- `usdchecker --arkit` on every USDZ, and plain `usdchecker` on USDA and USDC: 0 errors.
- Negative controls:
  - write set 3's data into set 2 in the Collada writer: the Collada test fails;
  - make `degrade` keep the first k sets again: `degrade_keeps_sampled_sets` and the
    USDZ test fail.

  Restore each by rewriting.

### Third-party files

`--import` the Khronos `MultiUVTest` (two sets, local only). It must pass as before.

### Docs

- `usd.rs`'s module doc (the reader's numbering rule; USDZ 2 with sampled sets kept;
  USDA 8);
- `dae.rs`'s module doc;
- `pose.rs`'s `degrade` doc;
- `mesh/mod.rs`'s table rows;
- `3d-format-conversion.md` §5 and §7;
- `scripts/mesh-verify/README.md` (`--arkit` on USDZ only);
- the matrix page's "UV sets" row (via litegen-a3).

### Gate

The overview's full gate, the harness with `multiuv`, and the `--import` above.

## Left to the builder

The harness's set selector for the `uv` mode.
