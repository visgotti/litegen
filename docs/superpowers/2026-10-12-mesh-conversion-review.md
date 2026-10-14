# Mesh conversion code review, 2026-10-12

Scope: everything under `litegen-core/src/mesh/`, plus its integration points:
`proxy/model3d_convert.rs`, `proxy/router.rs`, `proxy/poller.rs`, `proxy/storage.rs`,
`api/handlers/convert.rs`, `providers/model3d/{fal,mock}.rs` and `scripts/mesh-verify/`.
The question was whether each conversion step is correct against the official format specs
and stable reference implementations, which edge cases are not covered, and whether each step
is documented and referenced.

**Result.** On ordinary files the converters are sound: every pair of the 10 formats
round-trips, and independent renderers and validators agree with our output
(`3d-format-conversion.md` §8). The review still found 32 problems, grouped from 107
candidates. 105 candidates were confirmed, 1 is plausible but not measured, and 1 was refuted.
The problems fall into four groups:

- hostile or unusual input that can take down the whole server process;
- the proxy silently delivering degraded files;
- legitimate files converted wrong with no error;
- gaps in loss reporting, the test harness and the documentation.

**Method.** Eleven finders each reviewed one area, and an independent verifier then re-checked
every candidate. Nearly all candidates were reproduced on release builds of a standalone copy of
the mesh module. Abort-sized allocations were measured to a safe size and extrapolated. The
oracles were:

- three.js r183 loaders;
- assimp 6.0 and ufbx;
- Apple USD Tools 0.25.2 (`usdcat`, `usdrecord`, `usdchecker`);
- the Khronos glTF validator;
- xmllint and expat.

The probe programs lived in a scratch directory and were not kept. Each finding below says what
input reproduces it, which is what a regression test needs.

**Runtime facts that set the severity.**

- **Allocation failure** aborts the whole process. A stack overflow aborts it too.
- **The panic catch** around the conversion only covers panics. Conversions run in
  `spawn_blocking`, and that catch stops neither an abort nor a long-running loop.
- **Integer overflow** is unchecked in release builds.
- **Exposure:** untrusted bytes reach these readers from two places. One is the public
  `POST /v1/utils/models3d/convert` endpoint (128 MB body cap, 64 shared in-flight permits, no
  timeout). The other is vendor output on the poll path.

Findings are numbered most severe first.

---

## A. Input that can take down the server (abort or OOM)

### 1. Nested USDA dictionaries overflow the stack
- **Where:** `mesh/usda.rs:996`. `typed()` sends an unknown type to `generic(0)`, which resets
  the depth. `generic` then calls `dictionary(depth + 1)` (1191), `dictionary` calls `typed()`
  (1225), and the cycle repeats, so `MAX_VALUE_DEPTH` is never reached.
- **Reproduce:** `#usda 1.0` followed by `( customLayerData = ` + `{a b=` ×3000 + `1` + `}` ×3000
  + `)` and one triangle mesh. That is 18 KB, as USDA or wrapped in a USDZ, and it overflows a
  2 MiB thread stack (SIGABRT). The existing test `deep_nesting_is_refused_not_overflowed` only
  nests `[`.
- **Fix:** carry the depth through `typed()`, or count depth in the parser state on every `{` and
  `[`.

### 2. FBX morph-weight channel sized from the file
- **Where:** `mesh/fbx_read.rs:2825`, `vec![0.0f32; times.len() * ntargets]`. No budget charges
  it. Then `finish_channel(…, values.clone())` (2837) clones it once per node.
- **Reproduce:** a 276 KB ASCII FBX with 1,024 one-shape blend targets and one 160-key cubic
  `DeformPercent` curve. The process aborted on a single 651 MB `alloc_zeroed`. At the caps
  (20M times × 1,024 targets) the allocation would be about 82 GB.
- **Fix:** charge `times × targets` to `MAX_MORPH_VALUES` before allocating, using `checked_mul`.

### 3. Writer-side resampling has no total memory budget
- **Where:** `pose.rs:584` caps one grid at 100,001 times, but every writer multiplies that grid
  by the number of nodes.
  - DAE: `mesh/dae.rs:4424` pre-sizes `times × 16` f64 for every node, with no writer cap.
  - USD: `usd.rs:252` linearizes cubic channels before `validate()` (255).
  - FBX: `MAX_RESAMPLE_WORK` (`fbx_write.rs:87, 1000-1053`) bounds pose evaluations, not
    memory, and ignores morph targets. The Euler refinement shares one list of up to 20M times
    across every curve (1024, 1148-1212).
- **Reproduce:**
  - Each GLB node carrying a 2-key translation at 0 s and 3400 s costs 9.9 MB of DAE output
    and 35 MB of heap. A 265 KB GLB with 1,000 such nodes needs about 35 GB.
  - Cubic channels into USDZ: a 3.2 MB GLB needs about 26 GB inside `usd::prepare`.
  - FBX: a 270 KB GLB (1,000 nodes, 333 s) becomes a 191 MB FBX and 689 MB of heap. A 282 KB
    GLB with 1,024 targets becomes a 437 MB FBX and 1.26 GB of heap. A 6 MB GLB with 300k
    rotation keys becomes a 141 MB FBX that our own reader refuses.
- **Fix:** add one writer budget, the samples × (nodes + targets) the output will hold, checked
  before allocating. Check `MAX_ANIMATION_KEYS` while linearizing. Keep refined times per curve
  rather than in one global list.

### 4. Instancing bypasses the merged-geometry ceiling
- **Where:** each of these makes one copy per node that draws a skinned or morph-weighted mesh,
  with no `MAX_SCENE_*` check before the copies exist:
  - `mesh/usd_write.rs:806`;
  - `fbx_write.rs:1279`;
  - `dae.rs:377/404` (`bake_morph_targets`);
  - `threemf.rs:475-597`.
- **Why it slipped:** only `pose::world_primitives` (`pose.rs:191-207`) checks before copying.
  fa3bdde's per-instance ceiling used to cover every conversion through the flat read, so this
  is a regression.
- **Reproduce:** fa3bdde's own regression file (1.2 MB: a 99,999-vertex mesh × 45 nodes) is
  refused at HEAD, but now becomes a 114 MB USDZ. With 450 nodes it gives 1.14 GB of output and
  2.3 GB of RSS. 3MF and DAE `prepare` cost about 0.5 MB and 1.45 MB per instance of a
  30k-vertex mesh, so about 100k instances (a GLB of a few MB) need 50–145 GB.
- **Fix:** run the same running-total check before each per-instance copy, or count instanced
  vertices in `validate` before any writer runs.

## B. The proxy delivers degraded files without saying so

### 5. Derived files lose textures, and the losses are thrown away
- **Where:** `proxy/model3d_convert.rs:186` calls `mesh::convert`, which drops the write losses
  and the reader warnings (`mesh/mod.rs:513-514, 571-573`). `SynthesisReport` has no field for
  them. This contradicts `model3d_convert.rs:17` and `3d-format-conversion.md` ("the losses say
  what that costs").
- **Reproduce:** with every textured vendor GLB tested (Meshy, Rodin, Tripo, Hunyuan3D), derived
  obj, stl, ply, dae and usda come back with 0 textures, and the generation completes with
  nothing recorded.
- **Fix:** carry the losses for each format through `SynthesisReport` and log them. Surfacing
  them on the asset or in the response is an API change, so tell the frontend session first.

### 6. Rodin's fbx/usdz-first routing degrades the delivered GLB
- **Where:**
  - `providers/model3d/fal.rs:239` asks for `prefers_vendor_native(r) && offers(r)`.
  - `proxy/model3d_convert.rs:43` holds that rule.
  - The FBX reader has no metallic-roughness slot (`fbx_read.rs:2303-2318`).
  - Rodin's separate texture files are never passed as sidecars.
- **Reproduce:** `fal/hyper3d-rodin` with `output_formats: [glb, fbx]` or `[glb, usdz]` derives
  `model.glb` from the vendor's FBX or USDZ. Every case below reports its losses as `[]`:
  - The GLB always loses the metallic-roughness map and renders uniformly metallic 1, rough 1.
  - If the FBX references external PNGs rather than embedding them, the GLB has no textures at
    all. One real Rodin FBX would settle which happens.
  - Meshy's real FBX carries only the base map.

  A request for `[glb]` alone gets Rodin's own full-PBR GLB. Nothing backs the comment at
  `model3d_convert.rs:33-41` that vendor FBX and USDZ carry more.
- **Fix (your decision):** two options:
  - always take the vendor GLB as the source and derive fbx/usdz from it;
  - spend a second job on the native container when `max_items` allows.

### 7. Panics discard every vendor file on the poll path
- **Where:** two panics:
  - `mesh/glb.rs:714` (also `glb.rs:1642` and `dae.rs:1818`): `uri[..5]` byte-slices a URI and
    panics when byte 5 falls inside a multibyte character.
  - `pose.rs:586` and `pose.rs:127`: a key span beyond f32 range gives a NaN grid time, then an
    out-of-bounds `ch.times[k + 1]`.

  On a panic, the fallback replaces all of the vendor's files with an empty Vec
  (`poller.rs:232-247`, `router.rs:838`).
- **Reproduce:** a texture or buffer named `Textür.png` or `モデル.bin`, or a Collada
  `<init_from>` holding CJK text. A 1,100-byte GLB with rotation keys at [−2e38, 0, 2e38] panics
  glb→fbx and glb→dae. The convert endpoint returns 500. On the poll path the native mesh,
  previews and textures are all dropped, and the generation fails blaming the provider.
- **Fix:** use `uri.get(..5)` or a byte-wise prefix test. Refuse or shift negative and
  non-finite time ranges in the glTF reader, since the spec requires `time[0] >= 0`. Make the
  panic fallback keep the vendor's own files.

## C. More hostile-input memory and CPU problems

### 8. A 2 KB USDC allocates 1.6 GB
- **Where:** `mesh/usdc.rs:841`, `vec![None; total]`. `total` is bounded only by `MAX_TABLE`
  (64Mi), not by the bytes present. FIELDS (~524) and `decode_ints` (~224) have the same shape.
- **Reproduce:** `testdata/usd/cube.usdc` with numPaths rewritten to 67,108,864 reads OK, at
  1.61 GB of RSS.
- **Fix:** bound counts by the remaining bytes, as `Cursor::count` already does for STRINGS.

### 9. A tiny GLB expands strips into hundreds of MB
- **Where:** `mesh/glb.rs:1374`. View-less sparse accessors are now accepted. Strip and fan
  expansion and chunk copies are never charged, and the index ceiling is checked per primitive,
  not cumulatively. HEAD refused these, so this is a regression.
- **Reproduce:** a 668-byte GLB with a TRIANGLE_STRIP whose indices are a 4M-count view-less
  sparse accessor gives a 48 MB GLB and a 228 MB peak. Twelve such primitives (3.2 KB) reach
  518 MB before the budget refuses.
- **Fix:** charge the expansion to the Budget, and make the index ceiling cumulative.

### 10. The PLY and OBJ readers check caps only after building the scene
- **Where:** `mesh/ply.rs:538` (also 903, 913-917, 994-1011), and `obj.rs:577`, 461 and 449.
  Nothing counts while reading. Lists become a `Vec<f64>` per item, n-gons fan into 3(n−2)
  indices, and every `o` or `usemtl` allocates.
- **Reproduce:**
  - A 127 MiB PLY with one 133M-corner uchar face reaches 3.29 GB of RSS before Unsupported.
  - A 48 MB OBJ of `o a / f 1 1 1` ×4M reaches about 5 GB.
  - A 120 MB OBJ with one 60M-corner face reaches 1.57 GB.
- **Fix:** keep running vertex, index, node and primitive counts while parsing.

### 11. The USD reader amplifies blend shapes, keys and skins
- **Where:** three uncharged paths in `mesh/usd_read.rs`:
  - Blend shapes: 874 (also 883, 894, 981) allocate a full points-length array per target, with
    no morph budget.
  - Keys: 2133 `densify` caps samples per key window (10,000) but not in total, and
    timeCodesPerSecond comes from the file.
  - Skins: 1743 creates one Skin per distinct `geomBindTransform`, each copying every joint,
    with an O(J²) root search.
- **Reproduce:**
  - A 3.8 MB USDA (150k points × 200 targets) reads at 387 MB. At the caps that is about
    49 GB.
  - A 12 KB USDA with tcps 1e-9 is accepted with 20M keys; a 29 KB one reaches 1.24 GB.
  - An 841 KB USDA (2,000 joints, 2,000 meshes) builds 4M joint entries, 533 MB.
- **Fix:**
  - Charge targets to a morph budget.
  - Count densified samples against `MAX_ANIMATION_KEYS` as they are made.
  - Key skins by skeleton and bake `geomBindTransform` into the points. The scene ignores the
    holder's transform anyway.

### 12. The 3MF reader leaves components, materials and multiproperties uncharged
- **Where:** `mesh/threemf.rs:1999`: components and items are never charged, and the cap is per
  object. 2635 makes one primitive and one material per distinct base index. 2426 and 2451
  re-resolve multiproperties and clone composites per triangle, so the work is O(P·T).
- **Reproduce:**
  - A 609 KB ZIP of 100 objects × 100k components reads at 1.86 GB.
  - A 3.5 MB ZIP gives 1.3M primitives at 1.3 GB.
  - A 4.8 KB ZIP of 50k pids × 50k triangles takes 16.5 s.
  - A 13.5 KB file of composites takes 86 s.
- **Fix:** charge components and items. Resolve each group's properties once and cache them, and
  share composites by reference.

### 13. XML attributes: quadratic dedup and unbounded DOM memory
- **Where:** `mesh/xml.rs:124`. quick-xml's default duplicate-attribute check is linear per
  attribute, and nothing caps attributes per element. `MAX_DOM_ELEMENTS` does not bound
  attribute memory, so every Collada document and the 3MF rels are affected.
- **Reproduce:**
  - 40k attributes on one tag take 1.5 s; about 13M in a 128 MB DAE is estimated at ~45 h.
  - An 8 KB 3MF whose `_rels/.rels` holds 2M `<a/>` reads at 461 MB.
- **Fix:** cap attributes per element, turn off quick-xml's check, and use a HashSet. Charge
  attributes to the DOM budget.

### 14. The Collada reader copies curves, skins and morph bases
- **Where:** `mesh/dae.rs`:
  - 2791: every `<channel>` builds its own copy of its sampler's curve.
  - 2570: `infl.truncate(MAX_INFLUENCES)` keeps the full capacity.
  - 2628-2647: each `<instance_controller>` clones and hex-keys every inverse bind matrix.
  - 2507: a geometry listed as MORPH_TARGET up to 1,024 times is copied whole each time.
- **Reproduce:**
  - A ~1 MB DAE with one 200k-key sampler costs ~6 MB per extra `<channel>`.
  - A skin with 20k influences on one vertex, reused by 3,000 vertices, reaches 751 MB.
  - A 200k-vertex base costs ~2.3 MB per MORPH_TARGET listing.
- **Fix:** share one `Rc` curve per sampler. Cap influences while collecting them. Look up the
  skin cache before cloning. Dedupe and charge morph targets.

### 15. Deep hierarchies give quadratic output size
- **Where:** three writers grow with depth²:
  - `mesh/dae.rs:3772`: the indent loop has no cap (USD caps its indent at 48).
  - `usd_write.rs:347`: every joint token is its full path.
  - `fbx_write.rs:938-948`: bind poses push joints × non-joint ancestors before dedup.
- **Reproduce:**
  - An 8k-node chain (0.53 MB GLB) becomes a 184 MB DAE, which our own reader refuses because
    its nesting is deeper than 512.
  - An 8k-joint chain (1.06 MB) becomes a 329 MB USDA at 1.65 GB of heap.
  - 2000 joints × 2000 ancestors (250 KB) reaches 566 MB in the FBX writer.
- **Fix:** cap DAE indentation, bound total output, and dedupe bind poses with a set as they
  are added.

### 16. The FBX `KeyAttrRefCount` loop spins 2^31 times per run
- **Where:** `mesh/fbx_read.rs:2537-2545`. The inner loop runs the file's declared count even
  after every key is assigned. ufbx consumes runs per key (`ufbx.c:14311`).
- **Reproduce:** with 2 keys and a count of 2,147,483,647, each run costs 0.75 s. A ~150 KB file
  with 10k runs takes about 2 h.
- **Fix:** stop once `k == n`.

### 17. Superlinear CPU on hostile input
- **Where:**
  - Name uniquifiers: cubic in `obj.rs:1011`, quadratic in `fbx_write.rs:198`,
    `dae.rs:3709` and `usd_write.rs:75`.
  - `Scene::validate`: walks every node to its root (`scene.rs:619`), 2–4 times per conversion.
  - Linear scans per item: the OBJ `usemtl` lookup (`obj.rs:458`), the PLY duplicate-element
    check (`ply.rs:403`), the FBX ancestor walk (`fbx_read.rs:2756`), USD `prop_names`
    (`usd_stage.rs:1172`), the FBX bind-pose `joints.contains` and per-skinned-node
    `world_matrices` (`fbx_write.rs:939, 887`), and the GLB time-accessor dedup
    (`glb.rs:2564`).
  - Plausible, not measured: USD composition expands every arc with no memo, and truncates to
    `MAX_SOURCES` only after recursing (`usd_stage.rs:899-965`).
- **Reproduce:**
  - 8,000 same-named nodes (a 96 KB OBJ) take 179 s to write; 100k would take about 4 days.
    FBX, DAE and USDA at 100k take 6–10 min.
  - A 2 MB GLB with a 99,999-deep chain takes 23–38 s.
  - 80k `usemtl` lines (1.75 MB) take 13–30 s.
  - A 1 MiB PLY header takes 11.5 s.
  - A 12 MB FBX chain takes 45 s.
  - A 193 KB USDA takes 25 s.
- **Fix:** each has a linear form:
  - uniquifiers: a HashSet plus a counter per base name;
  - `validate`: one DFS from the roots;
  - the scans: HashMap lookups;
  - FBX: compute world matrices once per write;
  - USD composition: memoize `(stack, path)`.

## D. Legitimate files converted wrong

### 18. STEP keys fire a frame late in FBX and DAE output
- **Where:** `mesh/pose.rs:593`, `times.dedup_by(|a, b| |a−b| < 1e-6)`, keeps the earlier element.
  A grid time one ULP before a source key therefore replaces the exact key, and STEP sampled
  there returns the previous value.
- **Reproduce:** a 10 s, 30 fps STEP clip loses 151 of its 300 exact keys. In FBX the step lands
  ~33 ms late; in DAE, and when three.js reads the FBX, it becomes a ramp. Checked with assimp,
  ufbx and three.js.
- **Fix:** merge the source key times into the grid, and prefer them when deduping.

### 19. InheritType 0/2 under a sheared parent gives the wrong skeleton
- **Where:** `mesh/fbx_read.rs:279` (and near 1234) inverts a column-normalized,
  non-orthogonal rotation by transposing it. three.js `FBXLoader.js:4538-4540` uses a true
  inverse.
- **Reproduce:** grandparent scale (1,2,1), parent rotated 45°, identity child with InheritType
  2. The child's world differs from its parent's by up to 0.849, squashed about 1.44×. Mixamo
  writes InheritType 2 on every joint.
- **Fix:** use a real matrix inverse.

### 20. Concave n-gons fan-triangulated into the wrong shape
- **Where:** fan triangulation from corner 0 in `mesh/fbx_read.rs:1680`, `usd_read.rs:743`,
  `obj.rs:561` and `ply.rs:913`. ufbx and three.js FBXLoader/USDComposer ear-clip instead.
  `dae.rs:3599` ear-clips, but counts points on an edge as inside, so a repeated corner or
  keyhole falls back to a fan.
- **Reproduce:** the pentagon (0,0)(4,0)(4,2)(2,0.5)(0,2), area 5, reads as triangles totalling
  area 7, one of them back-facing, from OBJ, PLY, USDA and FBX. A Collada C-shape with a
  duplicated corner fills its notch.
- **Fix:** move `dae.rs`'s ear clipper into a shared helper, exclude coincident points from its
  inside test, and use it in every reader. OBJ fanning matches three.js OBJLoader, so OBJ is
  optional.

### 21. FBX colours are sRGB-encoded against the ecosystem convention
- **Where:** `mesh/fbx.rs:112-135` (`srgb_encode`/`srgb_decode`), `fbx_write.rs:739`,
  `fbx_read.rs:2194-2208`. The stated reason is the product's three.js viewer, but the dashboard
  previews only glTF (`dashboard/src/components/model3d-assets.ts:29-39`). ufbx, assimp,
  Blender and Unity URP treat FBX colours as linear, and `research/research-fbx.md:799-801`
  recommended linear.
- **Reproduce:**
  - Linear (0.2, 0.55, 0.85) is written as (0.48, 0.77, 0.93); ufbx and assimp read it that
    light.
  - A Blender-authored FBX, such as Meshy's, with DiffuseColor 0.8 reads as 0.604.
- **Fix (your decision):** write linear, or decode by `Creator`. Either way, update the
  rationale in `fbx.rs`.

### 22. A dropped metallic-roughness map is written as fully metallic
- **Where:** after degrade drops the MR texture (`pose.rs:768`), `mesh/fbx_write.rs:751-777`
  writes the bare factors as absolute values. glTF's defaults (1, 1) become fully metallic and
  fully rough. `obj.rs:1198` does the same with `Pm`/`Pr`. Separately, `SpecularColor` is
  written white with `Shininess` 0 (765-768), which gives a uniform white specular wash in
  three.js.
- **Reproduce:** `meshy.glb` → FBX writes `ReflectionFactor 1.0` and `Maya|metallic 1.0`.
  Blender, ufbx and assimp show it fully metallic. The map's mean metalness is 0.29, and Meshy's
  own FBX writes 0.0.
- **Fix:** when an MR map is dropped, write factor × the map's mean, or metallic 0. Scale or
  darken the specular.

### 23. USD schema rules the reader misses
- **Where (mostly `mesh/usd_read.rs`):**
  - 1159 (also 1190, 1437, 1440) reads `inputs:varname` without following `.connect`. That is
    the UsdPreviewSurface spec's own example pattern.
  - 1714, 1735, 661 and 675 ignore `skel:joints`, `geomBindTransform`, constant
    jointIndices/jointWeights and constant primvars authored on ancestors. UsdSkel and primvars
    inherit these.
  - 1600-1635 still animates an incomplete SkelAnimation; USD shows the rest pose.
  - `usd_stage.rs:561-571`: a default plus one time sample uses the default. usdrecord shows the
    sample.
  - `usd_stage.rs:1064` scales references and payloads by the stage root's tcps instead of the
    authoring layer's (Pcp `primIndex.cpp:2387`).
  - `usd_stage.rs:661` never applies a sublayer's tcps (Pcp `layerStack.cpp:1831`).
  - `usdc.rs:1084` fails the whole layer on crate types 57 (pathExpression) and 59 (splines).
  - `usda.rs:241` turns `\xNN` escapes into mojibake.
- **Where (writer, `mesh/usd_write.rs`):**
  - 345 renames joints and blend shapes (`mixamorig:Hips` → `mixamorig_Hips`) without writing
    `jointNames`.
  - 914 writes a zero scale, which three.js decomposes as unit scale; FBX and DAE write 1e-6.
- **Reproduce (against usdrecord and usdcat):**
  - A spec-pattern material renders red in usdrecord and white in litegen, and its UVs collapse
    to (0,0).
  - With the joints bound on an ancestor, the mesh comes out unskinned.
  - A reference authored in a 48 tcps layer under a 24 tcps root plays 2× fast; a 48 tcps
    sublayer plays at half speed.
  - Any USDC with a collection `membershipExpression` is refused.
  - Mixamo names come back renamed, with losses `[]`.
  - A hidden zero-scale part shows full size in three.js.
- **Fix:** each item has its own fix:
  - Resolve `.connect` for `varname`.
  - Walk ancestors for the inherited properties.
  - Check that a SkelAnimation is complete before animating it.
  - Prefer samples over the default.
  - Use the authoring layer's tcps, and apply sublayer tcps.
  - Decode crate values lazily, or skip unknown types with a warning.
  - Decode `\xNN` as bytes.
  - Write `jointNames` and originalName customData.
  - Use a nonsingular scale.

### 24. OBJ, PLY and STL text-format edge cases
- **Where:**
  - **BOM:** `mesh/obj.rs:331` (also 616, 287) keeps a UTF-8 BOM, so the first statement is
    ignored. `stl.rs:133` refuses a BOM'd ASCII STL.
  - **Lines and points:** `obj.rs:598`: one `l` or `p` line refuses the whole file.
  - **Groups:** `g` groups are merged.
  - **mtllib:** 584-593 splits on spaces, and 17 or more tokens refuse the file.
  - **Map options:** 663-685: `-s u` also scales V, and an unknown map option silently drops
    the texture.
  - **PLY attributes:** face colours, `diffuse_*` and `s,t` UVs are dropped silently, and a
    file with only edges or strips reads as Empty (`ply.rs:723-1024`).
  - **STL:** written and read Y-up in metres, with no note and no loss, and facet colours are
    dropped silently (`stl.rs:222, 421`).
- **Reproduce:**
  - **BOM'd OBJ:** the first `v` is lost and every positive index shifts, so the triangles are
    wrong; if line 1 is `mtllib`, the materials go to defaults.
  - **Blender OBJ:** one loose edge makes the file Unsupported.
  - **STL in a slicer:** a 1.8 m GLB exported to STL arrives as a 1.8 mm figure lying on its
    back.
  - **three.js:** handles the BOM, `l` and STL colour cases.
- **Fix:**
  - Strip a leading BOM.
  - Skip `l`/`p` with a warning.
  - Read the mtllib name to the end of the line.
  - Default `v` to 1 for `-s`, and warn on unknown map options.
  - Read the PLY aliases.
  - Document the STL convention, or offer Z-up millimetres.

### 25. FBX reader defaults that differ from the SDK and ufbx
- **Where (`mesh/fbx_read.rs`):**
  - 2223: `TransparentColor` falls back to white; the SDK default is black.
  - 2207-2208: the legacy `Emissive`, already colour × factor, is multiplied by
    `EmissiveFactor` again.
  - 1289-1292: a missing `RotationActive` defaults to 0. `ufbx.c:22964` defaults it to 1, and
    the comment claiming ufbx agrees is wrong.
  - 1395, `fbx_dom.rs:356/805`: any non-finite layer value, or an MSVC `1.#IND` token, refuses
    the whole file.
  - `fbx_dom.rs:650`: ASCII FBX must be valid UTF-8 as a whole, while binary decodes lossily.
- **Reproduce:**
  - `TransparencyFactor 1` with no `Opacity` and no template gives alpha 0, an invisible part;
    three.js guards against exactly this.
  - `EmissiveFactor` 0.5 beside a legacy `Emissive` gives a quarter of the brightness.
  - `PreRotation` with no `RotationActive` and no template is dropped.
  - One `1.#IND00` normal, or one CP1252 byte in an ASCII comment, makes the file Malformed;
    ufbx, assimp and three.js all load it.
- **Fix:**
  - Use the SDK defaults, plus three.js's opacity guard.
  - Apply `EmissiveFactor` once.
  - Default `RotationActive` as ufbx does.
  - Replace non-finite values with a warning.
  - Decode ASCII lossily.

### 26. Collada and XML semantics
- **Where:**
  - **Text fusion:** `mesh/xml.rs:160` `trim_text(true)` plus concatenation (249) fuses tokens
    around comments, PIs and CDATA. quick-xml's docs warn about this, and it breaks XML 1.0
    §2.10.
  - **Clips:** `dae.rs:2757` ignores `<animation_clip>` start and end, and drops unreferenced
    animations (spec 1.4.1 p.5-15).
  - **Ids:** `dae.rs:3698` keeps `.` in written ids, so assimp drops channels whose target holds
    two or more dots (`ColladaLoader.cpp:1140`).
  - **newparam:** `dae.rs:1609` never searches a technique-level `<newparam>`.
  - **`<constant>`:** an emission texture becomes unlit white.
  - **3MF names:** `threemf.rs:221` and `xml.rs:270` let U+FFFE and U+FFFF through.
- **Reproduce:**
  - `<color>1 0 <!-- tint --> 0 1</color>` reads as magenta.
  - A commented `<polygons><p>` silently loses a triangle; counted lists are refused instead.
  - A rig named `upper_arm.L.001`, written to DAE, loses all its animation in assimp.
  - Two clips each get the full 0–4 s curve.
  - A material named with U+FFFF produces a 3MF that expat rejects.
- **Fix:**
  - Keep text events separate, joined by whitespace.
  - Honour clip ranges, and keep unreferenced animations as a default clip.
  - Map `.` to `_` in ids.
  - Search the technique's newparams.
  - Use the emission texture as the base colour for `<constant>`.
  - Filter non-characters on write.

### 27. glTF: valid input refused, invalid output written
- **Where (refusals):**
  - `mesh/glb.rs:879` refuses an accessor with neither bufferView nor sparse, though the spec
    says it reads as zeros. HEAD read it, so this is a regression.
  - 1233 refuses a primitive with no POSITION; the spec says SHOULD skip.
  - 1223 and 753 refuse optional Draco or meshopt that comes with a fallback.
  - `scene.rs:829` and `glb.rs:1487-1797` make five non-geometry quirks fail even glb→stl/obj,
    which HEAD converted. Two of the five pass the validator.
- **Where (invalid output):**
  - 2567 writes negative key times.
  - 2309 writes `primitives: []` for a mesh that held only points or lines.
  - f32 overflow is written as `null` (1179, 1925, 1537).
  - A sheared matrix is written as a non-TRS glTF matrix, with losses `[]`; FBX errors instead
    (`fbx_write.rs:1622`).
- **Where (other):**
  - The hoist moves rigid primitives on a skinned node to the origin (2199).
  - `math.rs:101` uses an absolute determinant threshold.
- **Reproduce (Khronos validator):**
  - Files with a Blender empty shape key, `alphaMode "opaque"` or `magFilter 9987` have 0 errors
    and still fail every conversion.
  - FBX pre-roll converted to GLB gives ACCESSOR_ANIMATION_INPUT_NEGATIVE.
  - An edges-only CAD mesh gives EMPTY_ENTITY.
  - A sheared DAE gives NODE_MATRIX_NON_TRS in GLB and fails a billed FBX.
  - Scale (1e-5, 1e-5, 1e-3) gives wrong normals.
- **Fix:**
  - Zero-fill accessors under the budget.
  - Skip primitives with no position, with a warning.
  - Demote the non-geometry quirks to warnings.
  - Shift or clamp negative times.
  - Drop empty meshes.
  - Check f32 range on read.
  - Bake shear the way 3MF does.
  - Hoist only the skinned primitives.
  - Use a relative determinant threshold.

### 28. Alpha mode mishandled in 3MF, DAE and OBJ
- **Where:**
  - `mesh/threemf.rs:1005` writes displaycolor alpha whatever the alpha mode, and the reader
    returns AA < FF as OPAQUE.
  - `dae.rs:483` turns BLEND with vertex-colour alpha into OPAQUE, with no loss.
  - `obj.rs:1199` writes `d` for OPAQUE materials. OBJ also drops vertex and BLEND alpha, MASK,
    double-sided and unlit, and `scene::losses` reports none of it.
- **Reproduce:**
  - glTF OPAQUE with alpha 0.6 becomes `d 0.6`, which renders 60% transparent.
  - A 3MF with AA < FF becomes an OPAQUE glTF.
  - Vertex alpha 0.25 written to DAE or single-file OBJ comes out opaque, with losses `[]`.
- **Fix:** write alpha only for BLEND or MASK, and read AA < FF as BLEND. Report the alpha, mask,
  double-sided and unlit losses.

## E. Loss reporting, the proxy's resources, the harness and the docs

### 29. The loss header misses real losses and reports false ones
- **Where:** `api/handlers/convert.rs:175` builds `X-Litegen-Conversion-Losses` from the write
  losses only, and `convert_scene` throws away the reader's warnings.
- **Missed losses:**
  - The header is absent for a GLB with a LINES primitive, Meshy's own `meshy.obj` (whose MTL
    isn't supplied), and an FBX with external textures.
  - `KHR_materials_*` (transmission, clearcoat, sheen, …) and `KHR_lights_punctual` are dropped
    silently even when listed in `extensionsRequired` (`glb.rs:110`), so glass turns opaque.
  - A texture on a UV set that gets dropped is reported only as `uvs` (`pose.rs:704-716`).
  - Reading a multi-clip FBX keeps one clip and reports nothing.
  - MTL `Ke` clamps HDR emission.
- **False losses:**
  - The default LINEAR/mipmap filters are reported as `texture_sampling` (`scene.rs:1234`);
    usdz reports it twice. 3MF reports NEAREST as lost, though it round-trips.
  - Single-file usda reports `vertex_colors` dropped while delivering them (`mod.rs:471-485`).
  - An OBJ package reports WebP dropped while shipping it.
- **Fix:**
  - Add the reader's warnings to the header, or send them in their own header.
  - Report losses for the KHR extensions.
  - Compare filters against the effective default.
  - Compute losses on the prepared scene.

### 30. The poll path repeats work and never frees memory
- **Where:**
  - **Repeated work:** client GETs and the poller are not single-flight
    (`proxy/router.rs:803-868`), and the poller converts rows one at a time (`poller.rs:110`).
  - **Memory:** `LocalModel3dStorage`, the default, never evicts (`storage.rs:318-349`).
  - **Permit:** the convert endpoint releases its permit before writing a body up to ~160× the
    input (`convert.rs:156`).
  - **Synthesis:** it re-parses the source once per format and deep-copies every vendor file
    (`model3d_convert.rs:179-186`).
  - **Peak memory:** 8–15× the input, or 31× with all ten outputs held, against "several times"
    in `mod.rs:370`.
  - **Polycount:** copied from the source onto derived files even when instancing or
    degenerate-triangle removal changed it (`model3d_convert.rs:196-199`).
- **Reproduce:**
  - A 216k-triangle Tripo model costs 1.2 s and ~205 MB per observation.
  - A ten-format generation stays in memory (~117 MB) until restart.
  - Our own reader refuses our textured FBX above ~1.92M triangles (per-corner f64 normals
    exceed `MAX_DECODED_BYTES`), and our own textured 3MF above ~4M.
- **Fix:**
  - Claim a generation before converting it.
  - Evict stored files, or require object storage.
  - Hold the permit until the body is sent.
  - Parse once and write N formats.
  - Write FBX normals as f32, or size the budget to fit our own output.

### 31. The harness passes regressions, and production's form goes unverified
- **Where:**
  - `scripts/mesh-verify/verify.mjs:718`: `failures` ignores native DIFFs, any pair of errors
    counts as "refused" without comparing them, and refused imports drop out
    (`examples/mesh_matrix.rs:179`).
  - `mesh_matrix.rs:146` renders package mode only. Production ships `self_contained`: the
    single-file OBJ colour bake, data-URI glTF, and DAE/USDA without textures. No independent
    loader ever checks those.
  - The uv mode never compares on the textured fixture (`render.js:470/547`).
  - Every fixture has metallic 0, which is how #22 went untested.
  - `matrix_tests` exercise the flat API, which production does not use and which diverges:
    `mesh::write(Ply)` emits RGBA (`mod.rs:517-560`).
  - The mock provider cannot deliver derived-only requests such as `["fbx"]`
    (`providers/model3d/mock.rs:147`).
  - Dead or duplicated code: `capsFromKey`, the two copies of the usdrecord loop, and pose.rs's
    duplicated colour bake.
- **Fix:**
  - Fail on native DIFFs, on mismatched errors and on refused imports.
  - Render the `self_contained` mode too.
  - Add fixtures with UV-mapped textures and with metallic materials.
  - Point `matrix_tests` at the scene API.
  - Make the mock derive from its GLB.

### 32. Documentation and references
- **Fixed on 2026-10-12, alongside this review:**
  - The format research notes and their scripts moved from a temporary directory into
    [`research/`](research/README.md).
  - The Collada and FBX testdata generators are now in the repo, and each reproduced its
    committed files byte for byte:
    - `docs/superpowers/research/usd3mfdae/scripts/make_ref_dae.py`;
    - `litegen-core/examples/fbx_testdata.rs`.
- **Still open:**
  - Only 207 of 1,003 codec functions (21%) cite a spec or a reference implementation.
    Uncited core steps include:
    - Collada `primitive`, `skin` and `curve`;
    - USD `read_mesh`, `bind_skeletons` and `texture`;
    - FBX `build_mesh` and the animation channels;
    - the usdc and usda parsers.
  - `3d-format-conversion.md` cites no URLs.
  - Comments that are wrong today:
    - `fbx_read.rs:1289` claims ufbx defaults RotationActive off;
    - `fbx.rs:122` justifies sRGB by an FBX viewer the dashboard does not have;
    - `usd_stage.rs` `rest()` says usdrecord shows the default;
    - `mod.rs:48` says fbx keeps only the first animation and reports the rest, but the writer
      keeps every clip and the reader drops them silently;
    - `mod.rs:267` puts the 3MF description on the Dae arm.

---

## Checked and found correct

Recorded so nobody re-flags them:

- **FBX:**
  - Binary layout: header, 32-bit end offsets, NULL records, zlib arrays, and a footer identical
    to Assimp's. Our files parse in Assimp 6 and ufbx with 0 warnings.
  - Rotations and keys: Euler XYZ extraction and its gimbal branch, the unwrap, and the KeyAttr
    arrays.
  - Skins: cluster `Transform` and `TransformLink`, and the bind pose.
  - Blend shapes: relative shapes, and `DeformPercent` written both ways.
  - Settings and UVs: Y-up GlobalSettings, unit scale 100, the V flip, and the binormal sign.
- **glTF:** accessor bounds, sparse, normalized integers, strip/fan winding, samplers,
  quantization, `KHR_texture_transform`, and the hoist semantics.
- **Collada:** transparency modes, matrix transposes, rotate in degrees, `up_axis`,
  BEZIER/HERMITE layouts, tristrip winding, RELATIVE morph, and XSD-valid writer output.
- **3MF:** transforms, Y-up→Z-up, tex2coord `1−v`, the `p1` default, ZIP and ZIP64, OPC rels, and
  the unit default.
- **USD:** Z-up rotation, tcps fallbacks, xformOp order, quaternion order, texture scale, bias and
  channels, skin formulas, `usdchecker --arkit`, `subdivisionScheme none`, extent, and the API
  schemas.
- **Shared:**
  - The flat API equals the scene path byte for byte, apart from the documented OBJ/PLY
    `base_color`.
  - Sidecar resolution never leaves the package.
  - Output is deterministic: three processes produced identical bytes.
  - 1.77M mutation-fuzz iterations found no panics or hangs beyond those listed above.
- **Proxy:**
  - Output formats are trimmed, lowercased and deduped.
  - The deliverable-format sets agree everywhere.
  - Permits are taken before buffering.
  - `NotThisFormat` maps to 400.
- **Refuted:** OBJ multiplying `Kd` into `map_Kd` is what the MTL spec says ("the map_Kd value is
  multiplied by the Kd value"), and three.js does the same.
