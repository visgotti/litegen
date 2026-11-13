# Blender Light Units Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two opt-in options, `fbx_light_units` and `usd_light_units` (`photometric`, the
default, or `blender`). With `blender`, litegen's FBX and USD lights open in Blender at the
strength Blender's own glTF import gives them, and Blender's lights read back as Blender
reads them.

**Architecture:**
- The mesh library gets `LightUnits` and four option fields beside `FbxColorSpace`. The FBX
  and USD light writers and readers apply Blender's factors only when the option is
  `blender`.
- The converter endpoint and the 3D generation request carry the option exactly as they
  carry `fbx_color_space`. The SDKs, MCP's tool list and litegen-d0's converter package
  follow from that.

**Tech Stack:** Rust (`mesh/`, `types/`, `proxy/`, `api/`). Blender 5.2's headless bundle
for the probe and the confirmation. The SDK generators (`sdks/scripts/regen-all.sh`).

**Spec:** `docs/superpowers/specs/2026-11-08-light-units-option-design.md`

**Status: done on 2026-11-09 by session litegen-fc, not committed.** The ledger,
`.superpowers/sdd/2026-11-08-light-units-option/progress.md`, records each task's RED and
GREEN, every review and fix round, and the gate. It isn't committed; the rulings L1–L11
that change what the steps below say are listed here.

The proof:
- **Rust gate,** batched with item 3's (`2026-11-04-mesh-ktx2-textures.md`), on tree
  e8270b9 with cargo 1.99.1 and clippy 0.1.99, which holds every edit of this plan:
  - lib 2,802 passed, 0 failed, 18 ignored;
  - the three load-sensitive tests alone, 3/3;
  - `--tests --no-fail-fast` with Postgres: 54 integration suites, 879 passed, 0 failed;
  - clippy `--all-targets --all-features -D warnings` clean.
- **The default is unchanged.** `photometric` writes today's bytes (an in-run whole-file
  comparison, and a pin of `staged()`'s light records), and the release harness passed every
  fixture at the default, 1,140/1,140.
- **Real Blender 5.2.0 LTS** (Task 5a), 31 checks:
  - litegen's `blender` FBX, USDA and USDZ of the `staged` scene import with exactly the
    strengths Blender's own glTF import gives (worst relative difference 0);
  - Blender's own FBX and USD exports read back through litegen as 3 lux, 20 cd and
    40 cd (worst relative difference 2.64e-8);
  - `photometric` files differ by the known factors.
  The two ignored tests in `mesh/tests.rs`, `write_the_light_units_probe_files` and
  `read_the_light_units_probe_files`, and `scripts/blender-light-units/probe.py` rerun it.
- **SDKs and the converter package:** litegen-d0 regenerated after Task 4 and after the
  final fix wave. The package takes both options, and `light_units_equal_the_hosted_pipeline`
  shows its output equals the hosted converter's in six directions with both options.

The build differs from the steps below in these places:

- **Task split.** Task 5 ran as 5a (real Blender and the docs); its gate ran with item 3's.
  Implementers and reviewers ran on sonnet (L2); the final review ran on fable.
- **Choices the plan left open:**
  - L3: a USD read folds `2^inputs:exposure` into the strength (Global Constraints).
  - L4: FBX gets no write-side clamp under `blender`: `Intensity` is f64, and a clamp
    would break the exact round trip.
  - L5: on read, an `Intensity` whose candela or lux overflows f32 drops the light with the
    existing warning, as `photometric` does (item 5's R5, R8).
  - L7, L7a: the byte pin covers `staged()`'s light records only, since its transforms go
    through libm and CI is x86_64 Linux. If a Linux run ever differs in the last bit, hash
    the records without `Color`.
  - L9: on the generation request, a bad word is refused as `fbx_color_space`'s is (400
    `malformed_request`, exact lowercase). On the converter endpoint it is 400
    `param_invalid`, case- and space-insensitive.
- **Known limits, documented** (3d-format-conversion.md §4 and §5):
  - Blender 5.2.0's legacy Python FBX importer raises on any file with a light. It reads the
    same factor only with a stub for the setting it lacks; the default importer reads it as
    shipped;
  - Storm (usdview, usdrecord) lights a normalised sphere 4× brighter than Blender does;
    the two agree only on the sun.
- **Open, for the user:**
  - L11: an operation's `source_model` is still read with the default units and colour
    space (`model3d_source::prepare`). Threading `fbx_light_units`, `usd_light_units` and
    `fbx_color_space` into it is a follow-up;
  - the first x86_64 Linux CI run checks L7a's pin.
- **Parked minors,** left as they are:
  - the generation request accepts an option for a format it doesn't request, as
    `fbx_color_space` does;
  - null light-units fields in `request.params`;
  - `write_usdz` and `write_usda` take the option differently, on purpose;
  - a guard test the final review suggested.

Files to commit (36). † marks a file that also holds edits from item 3, item 5's
follow-ups or item 10, all of them done, so they can share a commit. ‡ marks a file that
also holds another session's uncommitted work (the 2026-11-09 provider audit), so check
with it first.

```
docs/mcp/tools.md †
docs/superpowers/3d-format-conversion.md †
docs/superpowers/plans/2026-11-04-mesh-gaps-handoff.md †
docs/superpowers/plans/2026-11-04-mesh-lights-cameras.md †
docs/superpowers/plans/2026-11-08-light-units-option.md
docs/superpowers/research/research-blender-light-units.md
docs/superpowers/specs/2026-11-04-mesh-gaps-overview-design.md †
docs/superpowers/specs/2026-11-08-light-units-option-design.md
litegen-core/src/api/handlers/convert.rs †
litegen-core/src/api/handlers/mod.rs †
litegen-core/src/mesh/fbx.rs †
litegen-core/src/mesh/fbx_read.rs †
litegen-core/src/mesh/fbx_write.rs †
litegen-core/src/mesh/mod.rs †
litegen-core/src/mesh/scene.rs †
litegen-core/src/mesh/scene_matrix_tests.rs †
litegen-core/src/mesh/tests.rs †
litegen-core/src/mesh/usd.rs †
litegen-core/src/mesh/usd_read.rs †
litegen-core/src/mesh/usd_write.rs †
litegen-core/src/providers/mod.rs †
litegen-core/src/providers/model3d/fal.rs † ‡
litegen-core/src/providers/model3d/meshy.rs †
litegen-core/src/providers/model3d/mock.rs †
litegen-core/src/providers/model3d/tripo.rs †
litegen-core/src/proxy/config_conformance.rs †
litegen-core/src/proxy/model3d_convert.rs
litegen-core/src/proxy/model3d_deliver.rs †
litegen-core/src/proxy/poller.rs †
litegen-core/src/proxy/router.rs †
litegen-core/src/types/mod.rs †
litegen-core/tests/convert_api.rs †
litegen-core/tests/live_model3d.rs †
litegen-core/tests/model3d_api.rs †
litegen-core/tests/model3d_validation.rs †
scripts/blender-light-units/probe.py
```

litegen-d0 lists the regenerated SDK, landing and converter-package files with its own
work.

## Global Constraints

- **Start after item 10 (points and lines) closes.** Both edit `fbx_read.rs`, `fbx_write.rs`,
  `usd_read.rs` and `usd_write.rs`.
- **Names, exact:**
  - the options `fbx_light_units` and `usd_light_units`;
  - the values `photometric` (the default) and `blender`;
  - the types `mesh::LightUnits { Photometric, Blender }` and
    `types::Model3dLightUnits { Photometric, Blender }`.
- **The default is today's output:** `photometric` writes today's bytes, byte for byte.
- **Converter rules:**
  - `fbx_light_units` applies when `from` or `to` is `fbx`;
  - `usd_light_units` applies when `from` or `to` is `usdz` or `usda`;
  - anywhere else, the option is 400 `param_unsupported`;
  - an unknown value is 400 `param_invalid`;
  - values are case- and space-insensitive, as `fbx_color_space`'s are.
- **Blender's glTF import (SPEC mode), the definition of E:**
  - directional: E = lux / 683 (W/m²);
  - point or spot: E = cd × 4π / 683 (W).
- **Blender 5.2.0's factors** (Task 1, `docs/superpowers/research/research-blender-light-units.md`,
  each cited at the v5.2.0 tag and probed):
  - **FBX:** F_fbx = 100. With `blender`, write `Intensity` = 100 × E, and read
    E = `Intensity` / 100.
  - **USD write:** a point or spot gets `inputs:intensity` = E / π, and a sun gets E / 4,
    normalised as today.
  - **USD read** (no litegen `customData`): E = intensity × π for a point or spot, and
    intensity × 4 for a sun. `normalize`, `radius` and `angle` don't change E: Blender's
    importer sends them to other fields.
  - **Ruling L3: exposure folds in.** E is multiplied by 2^`inputs:exposure`. Blender keeps
    exposure in its own field and renders with it, and glTF has none, so folding keeps the
    picture the same. That's also what the photometric reader does — cost if wrong: a
    Blender light with exposure ≠ 0 reads 2^exposure off.
  - The UsdLux intensity fallbacks (1 for a sphere, 50000 for a distant light) apply
    before the factor, as Blender's reader applies them.
  - Blender's own FBX and USD round trips are identities (under 6e-8), so the spec's
    "which criterion gives way" doesn't arise.
  - The legacy FBX importer raises on any file with a light in 5.2.0
    (`import_fbx.py` L2255, `cast_shadow`), so Task 5 reads its factor through the probe's
    stub.
- **litegen-d0 lands its parity in the same window as the mesh fields.** Its literals are a
  deliberate tripwire, and its CI compiles the whole workspace. The controller sends d0 the
  final names before Task 2 adds the fields, and tells d0 when they land.
- **Blender runs:** set `sys.dont_write_bytecode = True` first, and write nothing inside
  the Blender bundle. Run its
  `Blender.app/Contents/MacOS/Blender -b`.
- **Repo rules:**
  - the overview's "Rules every plan carries";
  - `mesh-standalone.md` (no serde in `mesh/`);
  - `engine-boundary.md`;
  - nothing committed or staged;
  - sonnet subagents until 2026-11-12 02:00 ET (the opus limit), with fable as the
    fallback.

## Review Focus

1. **A light at the f32 edge** (1e38 cd, or 0): the `blender` factors write finite values,
   clamped as today's `.min(f32::MAX)`, and never inf or NaN.
2. **A Blender FBX with a non-zero `exposure`:** Blender folds 2^exposure into `Intensity` and
   its importer ignores it, so litegen reads `Intensity` / F_fbx as the energy and reads no
   exposure.
3. **A USD light Blender didn't shape** (unnormalised, radius > 0, angle > 0), read with
   `blender`: it follows Blender's importer's formula for each of those inputs (Task 1's
   note), not the schema path.
4. **A scene with no lights** written with `blender`: the bytes equal the default's.
5. **A spot light:** its cone changes nothing in E, and it round-trips like a point light.

---

### Task 1: Blender's factors, probed and cited

**Files:**
- Create: `docs/superpowers/research/research-blender-light-units.md`
- Create: `scripts/blender-light-units/probe.py`, the Blender script, kept with its output
  in the note.

**Interfaces:**
- Produces: the factors Tasks 2 and 3 implement, one table, each with file:line in
  Blender's source at the v5.2 tag and its probe reading:
  - F_fbx: the default `wm.fbx_import` reads `Intensity` / F_fbx as `energy`. Also give the
    legacy `import_fbx.py` reading, and the Python exporter's `Intensity = energy × 100 ×
    2^exposure`.
  - USD export: a Blender light of energy E becomes UsdLux values for a point, a spot and a
    sun (`usd_writer_light.cc`).
  - USD import: UsdLux values become `energy` for each kind, including `normalize`,
    `exposure`, `radius` and `angle` as `usd_reader_light.cc` folds them.
  - Whether Blender's own USD export → import is an identity for each kind, and the factor
    where it isn't.
  - Whether the 5.2 bundle has a Collada operator (`wm.collada_import`).

- [ ] Write the probe:
  - import `staged`'s photometric FBX, USDA and USDZ (from `cargo run --example mesh_matrix
    -- <dir> --fixtures staged`, debug build) through both FBX importers and the USD
    importer;
  - import its GLB through the glTF importer;
  - print every light's type and `energy`;
  - export that glTF-imported scene as FBX and USD, then print the exported files' light
    values and their re-import energies.
- [ ] Run it. Expected: one table of readings per light. Derive each factor from the
  readings, and check each against the cited source line.
- [ ] Write the note: the factor table, the citations, the probe's output, and each place
  where source and probe disagree (the probe wins, and the disagreement is named).

### Task 2: The option in the mesh library, and FBX

**Files:**
- `litegen-core/src/mesh/mod.rs`: `LightUnits`, the option fields, and the plumbing to `fbx`.
- `litegen-core/src/mesh/fbx.rs`: `read_scene_with` and `write_scene` receive the units.
- `litegen-core/src/mesh/fbx_write.rs`: `light_attribute`.
- `litegen-core/src/mesh/fbx_read.rs`: the light `Intensity` read, ~L1537.
- `litegen-core/src/mesh/scene.rs`: the shared E helpers.
- Every `ReadOptions` and `WriteOptions` literal that breaks: in `src/mesh/` tests,
  `src/proxy/model3d_deliver.rs` ~L125 and ~L669, `src/api/handlers/convert.rs` ~L369 and
  ~L380, and `tests/`.

**Interfaces:**
- Consumes: Task 1's F_fbx.
- Produces:
  - `pub enum LightUnits { #[default] Photometric, Blender }` with `pub fn as_str(self) ->
    &'static str` ("photometric" / "blender");
  - `ReadOptions { fbx_color_space, fbx_light_units, usd_light_units }` and
    `WriteOptions { self_contained, fps, fbx_color_space, fbx_light_units, usd_light_units }`;
  - in `scene.rs`, `pub fn blender_energy(kind: &LightKind, intensity: f64) -> f64` and
    `pub fn from_blender_energy(kind: &LightKind, energy: f64) -> f64`, with
    `pub const BLENDER_LUMENS_PER_WATT: f64 = 683.0`;
  - `mod.rs`'s `convert_scene` builds its `ReadOptions` from `WriteOptions` with both
    unit fields, as it does `fbx_color_space`.

- [ ] Before any change, compute and pin an FNV-1a 64 of `staged()`'s FBX and of `cube()`'s
  FBX written with `WriteOptions::default()`. These are the "today's bytes" constants.
- [ ] Write the tests:
  - `fbx_photometric_bytes_are_unchanged`: both hashes, with the default options and with
    an explicit `Photometric`;
  - `blender_energy_is_blenders_gltf_import`: a 3 lux sun gives 3/683, and a 100 cd point
    or spot gives 100·4π/683; `from_blender_energy` inverts each to 1e-12 relative;
  - `fbx_blender_units_write_blender_energy`: each `staged` light's `Intensity` is
    F_fbx × E;
  - `fbx_blender_units_round_trip`: write and read with `blender` gives each intensity to
    1e-6 relative;
  - `fbx_blender_reads_intensity_as_blender_does` (Review Focus 2): a hand-built ASCII
    FBX light with `Intensity` 2000 reads as cd = (2000/F_fbx)·683/4π;
  - `fbx_blender_units_clamp_at_the_f32_edge` (Review Focus 1);
  - `fbx_blender_units_leave_a_lightless_scene_alone` (Review Focus 4): `cube()`'s bytes
    with `blender` equal the default's;
  - `fbx_spot_energy_ignores_the_cone` (Review Focus 5).
- [ ] Run them: `cargo test --lib -- mesh::fbx mesh::scene`. Expected: FAIL on assertions,
  against stubs.
- [ ] Implement. The FBX path's other light properties (colour, decay, cones, range) are
  unchanged.
- [ ] Run `cargo test --lib -- mesh::`, skipping the three load-sensitive tests, then
  `cargo test --lib --no-run` and clippy. Expected: pass.

### Task 3: USD

**Files:**
- `litegen-core/src/mesh/usd.rs`: `read_usdz`, `read_usda`, `write_usdz` and `write_usda`
  receive the units.
- `litegen-core/src/mesh/usd_write.rs`: `write_light` intensity, ~L808.
- `litegen-core/src/mesh/usd_read.rs`: a `blender` branch in `light_of`.
- `litegen-core/src/mesh/mod.rs`: the plumbing only.

**Interfaces:**
- Consumes: Task 2's `LightUnits`, the option fields and the E helpers; Task 1's USD factors.
- Produces: nothing new for later tasks.

- [ ] Write the tests:
  - `usd_photometric_bytes_are_unchanged`: `staged()` and `cube()` as USDA and USDZ,
    hashes pinned before the change;
  - `usd_blender_units_write_blenders_values`: each light kind gets Task 1's exporter
    values;
  - `usd_blender_read_follows_blenders_importer`: a USDA as Blender writes it (normalised,
    radius 0, plus Review Focus 3's unnormalised sphere with radius and a sun with an
    angle) reads per Task 1's importer formulas;
  - `usd_custom_data_still_wins`: a litegen file written with `blender` reads exactly with
    `photometric`, and the reverse;
  - `usd_blender_units_round_trip`: each `staged` light, the spot included, to 1e-6
    relative;
  - `usd_blender_units_clamp_at_the_f32_edge` (Review Focus 1);
  - `usd_blender_units_leave_a_lightless_scene_alone` (Review Focus 4): `cube()`'s USDA and
    USDZ bytes with `blender` equal the default's.
- [ ] Run `cargo test --lib -- mesh::usd mesh::usda mesh::usdc`. Expected: FAIL on
  assertions, against stubs.
- [ ] Implement. `photometric` takes today's path in `light_of` unchanged.
- [ ] Run the mesh suite, then clippy. Expected: pass.

### Task 4: The converter endpoint and the generation request

**Files:**
- `litegen-core/src/api/handlers/convert.rs`: `PARAMS`, parsing, the two `utoipa` params, and
  the 400 texts.
- `litegen-core/src/types/mod.rs`: `Model3dLightUnits`, and the two request fields beside
  `fbx_color_space` (~L1025).
- `litegen-core/src/providers/mod.rs` ~L755.
- `litegen-core/src/proxy/model3d_convert.rs`: a `light_units(choice)` mapping beside
  `fbx_color_space` (~L57).
- `litegen-core/src/proxy/model3d_deliver.rs`: the delivery plan, the options, and
  `meta.delivery`.
- `litegen-core/src/api/handlers/mod.rs` ~L1929.
- Every literal with `fbx_color_space: None` (router, poller, providers' mock, fal, meshy and
  tripo, `config_conformance`, `tests/live_model3d.rs`, `tests/model3d_validation.rs`):
  mechanical.
- `tests/convert_api.rs`, `tests/model3d_api.rs`.

**Interfaces:**
- Consumes: Task 2's mesh fields. Reuse item 1's option plumbing; never copy it (the
  overview's "Rules every plan carries"). The path runs: the request field →
  `DeliveryPlan` (a serde default, skipped when off) → `write_keyed(…, plan)` in
  `proxy/model3d_deliver.rs`, which is where a new `WriteOptions` field is threaded.
  - **Sequence with item 3's Task 4** (`ktx2_textures`): it edits the same converter
    `PARAMS`, request type, `DeliveryPlan` and `docs/mcp/tools.md`. Whichever task runs
    second rebases onto the first.
- Produces: the OpenAPI shapes Task 5 regenerates from. Query `fbx_light_units` and
  `usd_light_units` (string); request fields `fbx_light_units` and `usd_light_units`
  (`Model3dLightUnits`, lowercase).

- [ ] Write the tests:
  - `light_units_apply_at_their_end` in `convert_api.rs`:
    - `?from=glb&to=fbx&fbx_light_units=blender` is 200, and its lights are F_fbx × E;
    - `?from=fbx&to=glb&fbx_light_units=blender` is 200;
    - `?from=glb&to=usdz&usd_light_units=blender` is 200;
    - `?from=glb&to=obj&fbx_light_units=blender` and `?from=glb&to=fbx&usd_light_units=blender`
      are 400 `param_unsupported`;
    - `fbx_light_units=watts` is 400 `param_invalid`;
    - `fbx_light_units=%20Blender%20` is 200;
    - `?from=fbx&to=fbx&fbx_light_units=blender` on a litegen `staged` FBX gives back the
      same light intensities to 1e-6 relative: the same-format no-op the spec documents.
  - `light_units_are_per_request_overrides` in `model3d_api.rs`: `meta["delivery"]` records
    both values, and a bad value is refused, as
    `fbx_color_space_and_format_source_are_per_request_overrides` does.
- [ ] Run `cargo test --test convert_api --test model3d_api`. Expected: FAIL.
- [ ] Implement, with descriptions modelled on `fbx_color_space`'s (convert.rs ~L158):
  - each value's meaning;
  - that `photometric` is three.js's, ufbx's and Storm's reading;
  - that a same-format conversion is a no-op (spec);
  - the 400 lists gain the two names.
- [ ] Regenerate MCP's tool list. Its tools derive from the OpenAPI document, and
  `mcp/tests.rs` (~L713) fails while `docs/mcp/tools.md` is stale. From `litegen-core`, run
  `cargo run --example mcp_tools_doc > ../docs/mcp/tools.md`.
- [ ] Run both suites, `cargo test --lib` (`mcp::` included) and clippy. Expected: pass.

### Task 5: Confirmation, docs, regeneration and gate (the controller's)

**Files:**
- `docs/superpowers/3d-format-conversion.md` §4 and §5.
- The code docs the spec lists:
  - `fbx_write.rs` `light_attribute`;
  - `fbx_read.rs`'s module doc;
  - `usd_write.rs` `SPHERE_INTENSITY_PER_CANDELA`;
  - `usd_read.rs`'s module doc.
- The SDKs (regenerated), and the hand-written comments in TS `client.ts` and Python
  `types.py`/`client.py`.
- `apps/landing/public/openapi.json` (synced).

- [ ] **Real Blender, on the new code:** Task 1's probe, run with `staged` written with
  `blender`, gives each light's `energy` within 1e-4 relative of Blender's glTF import.
  Blender's own exports, read by litegen with `blender`, give each light within 1e-4
  relative of E⁻¹ of Blender's re-import. Keep the outputs in the ledger.
- [ ] Update the docs. Replace each "possible follow-up" sentence with the options and the
  factors.
- [ ] After a green round:
  - `sdks/scripts/regen-all.sh`, then `node apps/landing/scripts/sync-openapi.mjs`;
  - the SDK tests.
- [ ] litegen-d0's parity: it lands in the same window as Task 2, and its suites are green
  as d0 reports them.
- [ ] Run the overview's full gate. Write the ledger. Send the matrix page's FBX and USD
  lights cells to its steward.
