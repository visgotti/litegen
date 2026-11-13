# Blender light units for FBX and USD — design

Status: design, 2026-11-08, builder-checked the same day. The check's four questions are
settled below as my calls, and its MCP correction is applied. The user delegated every call
here ("idk u decide … do it urself"; "always make it opt in if its an optional thing",
LiteLLM-style), so each decision below is the controller's (litegen-fc), marked
**(my call)**. Build it after item 10 (points and lines) closes, because both edit
`fbx_read.rs`, `fbx_write.rs`, `usd_read.rs` and `usd_write.rs`.

## Why

litegen writes and reads lights in glTF's photometric units: candela for a point or spot
light, lux for a directional one (KHR_lights_punctual).
- **FBX:** `Intensity` = 100 × the candela or lux (`fbx_write.rs` `light_attribute`). That's
  what ufbx and three.js's FBXLoader divide back out.
- **USD:**
  - a point or spot light is a normalised `SphereLight` treated as a point, with intensity
    = 4 × the candela (`usd_write.rs` `SPHERE_INTENSITY_PER_CANDELA`, Ruling R6);
  - a directional light is a normalised `DistantLight` of angle 0, with intensity = the lux;
  - litegen also writes the scene's exact values in each prim's `customData`, so its own
    files read back exactly.

Blender doesn't read either format that way. Item 5's fix wave cited all of the following
from the Blender 5.2 bundle and io_scene_gltf2 v5.2.39:
- **Blender's units.** It keeps a light's strength in its own radiometric watts: W/m² for a
  sun.
- **FBX import:** `energy = Intensity / 100` (import_fbx.py L2251). That's the Python
  importer, now the "Legacy" one. Blender 5.2's default importer is the ufbx-based
  `wm.fbx_import` (see "The mapping").
- **FBX export:** `Intensity = energy × 100 × 2^exposure` (export_fbx_bin.py L605).
- **glTF import,** in the default `SPEC` lighting mode (`blender/imp/light.py` L58-L71,
  `PBR_WATTS_TO_LUMENS = 683` in `io_scene_gltf2/blender/com/conversion.py` L10):
  - a sun gets lux / 683 W/m²;
  - a point or spot light gets candela × 4π / 683 W.
- **USD export:** a P-watt point light becomes a normalised `SphereLight` of intensity P/π
  (`usd_writer_light.cc` L133-L141 and L171-L174, v5.2.3).

The result:
- litegen's FBX lights open in Blender about 54× (683/4π, point and spot) and 683× (sun)
  brighter than the same scene's glTF does. Its USD lights open 683× (point and spot) and
  2732× (sun) brighter, because Blender's USD importer reads energy = intensity·π for a
  sphere and intensity·4 for a sun (Task 1's note,
  `docs/superpowers/research/research-blender-light-units.md`);
- Blender's own FBX and USD lights read that much dimmer in litegen. `staged`'s 3 lux sun
  came back from Blender's FBX export as 0.0044 and from its USD export as 0.0011 (item 5,
  Ruling R28).

Item 5 documented this and left an opt-in option as a follow-up for the user to decide. The
user has now delegated it: build it, opt-in.

## Goals

1. **Two opt-in options.** `fbx_light_units` and `usd_light_units` each take `photometric`
   or `blender`.
   - `photometric` is the default and writes today's bytes, byte for byte.
   - **(my call)** Each format gets its own option, as `fbx_color_space` does.
     Format-specific parameters are the user's LiteLLM-style ask, and a caller converting FBX
     → USD may want Blender units at one end only.
2. **What `blender` means.**
   - *Write:* each light opens in Blender 5.2 with the `energy` Blender's own glTF importer
     gives the same scene's glTF light.
   - *Read:* a light Blender wrote reads as the glTF light whose Blender glTF import gives
     that `energy`.
   - So "same picture in Blender" holds both ways, and litegen → Blender → litegen returns
     the scene's values.
3. **Read and write.** Each option applies at whichever end of a conversion is its format,
   as `fbx_color_space` does at the FBX end.
4. **Exposed everywhere `fbx_color_space` is:**
   - the mesh library's `ReadOptions` and `WriteOptions`;
   - the converter endpoint's query (`POST /v1/utils/models3d/convert`);
   - the 3D generation request's delivery options. The value is applied to every FBX or USD
     litegen reads or delivers for the request, and recorded in the job's `meta.delivery`.

## Non-goals

- **Other applications' conventions** (Maya, 3ds Max, Unity, Unreal, Houdini). A later value
  is one more enum variant, additive on the wire.
- **glTF and Collada.**
  - glTF's units are photometric by its specification, and Blender's glTF IO converts them
    itself.
  - Blender 5.x has no Collada IO. **(my call, to confirm)** The builder checks the 5.2 bundle
    for a `wm.collada_import` operator. If one exists, a `dae_light_units` is a follow-up,
    not part of this design.
- **Blender's `exposure`.** Its FBX exporter multiplies `Intensity` by 2^exposure, and its
  importer ignores it. litegen writes no exposure and reads none.
- **Light types the scene doesn't model** (area, dome, …).
- **Any change to the default** or to the `customData` exact round trip.

## Anti-goals

- `photometric` changes a byte of today's output.
- The two directions disagree. Writing with `blender` and reading back with `blender` must
  return the scene's values to f32 rounding. That's one multiply and its inverse.
- A factor that is guessed instead of read from Blender's source and confirmed by Blender
  itself (see USD below).

## The mapping

Let E(light) be the energy Blender's glTF importer gives a scene light:
- directional: lux / 683, in W/m²;
- point or spot: candela × 4π / 683, in W.

Blender's spot energy is the power of the equivalent point light, so the spot cone leaves it
unchanged. Write E⁻¹ for the inverse (lux = 683 E; candela = 683 E / 4π).

### FBX, `fbx_light_units = blender`

- **Write:** `Intensity` = 100 × E(light).
- **Read:** E = `Intensity` / 100, then the scene light is E⁻¹(E) for its `LightType`.
- **Which importer the write must satisfy (my call, after the builder check):**
  - Blender 5.2's File > Import > FBX is the C++ ufbx-based `wm.fbx_import`
    (`startup/bl_ui/space_topbar.py` L388; `fbx_read.rs`'s module doc says so too). The
    Python importer cited above is the "FBX (.fbx) (Legacy)" one (`io_scene_fbx/__init__.py`
    L716).
  - The write targets the **default** `wm.fbx_import`, because that's what users open. ufbx
    divides `Intensity` by 100 (ufbx.c L23049-L23052, as `light_attribute` cites). The builder
    reads how Blender's C++ importer turns ufbx's intensity into `energy` (Blender's source at
    the v5.2 tag, `source/blender/io/fbx/…`, cited file:line).
  - The real-Blender check runs **both** importers. If they disagree, the default wins:
    the factor follows `wm.fbx_import`, and the legacy importer's reading is documented beside
    it.
  - The read side has one source: Blender has no C++ FBX exporter, so the Python exporter's
    `Intensity = energy × 100 × 2^exposure` is the only one.

Everything else is unchanged:
- colour, still in `fbx_color_space`;
- `DecayType`;
- cones;
- the range's far attenuation.

### USD, `usd_light_units = blender`

- **Write:** each `DistantLight` and `SphereLight` gets the UsdLux values Blender's USD
  exporter writes for a Blender light of energy E(light):
  - a point or spot light: E/π on a normalised sphere (`usd_writer_light.cc` L133-L141,
    L171-L174);
  - a sun: whatever that file's sun branch writes. Blender's USD export of `staged`'s sun
    wrote 0.0011, a quarter of the 0.0044 W/m² its FBX export wrote, so the sun's factor
    isn't 1 and must come from the source.
- **Read:** applies to a light prim that has no litegen `customData`. Its energy is what
  Blender's USD importer (`usd_reader_light.cc`, v5.2.3) takes from it, and the scene light
  is E⁻¹ of that energy.
- **`customData`.** It keeps the scene's own values whichever option wrote the file, and on
  read it still wins over the UsdLux attributes. So litegen's own files read back exactly
  under either option.
- **(my call)** The builder reads every USD factor (writer and reader, point, spot and sun)
  from Blender's source at the v5.2.3 tag, cites file:line, and confirms each one with the
  real-Blender check below. If Blender's importer isn't the exact inverse of its exporter,
  then:
  - the read side follows the **importer**, because that's what reading a Blender file
    means;
  - the write side follows the **exporter-inverse of the importer**: litegen writes what
    Blender's importer turns into E(light);
  - the discrepancy is documented.
- **"Follows the importer" means all of it (my call, after the builder check).**
  - With `blender`, a light prim without litegen `customData` reads by Blender's importer's
    formula (`usd_reader_light.cc`) for every input that formula folds in (`normalize`,
    `exposure`, `radius`, `angle`, whatever it uses), each cited.
  - What the importer ignores is ignored.
  - The schema reading `light_of` does today stays the `photometric` path, unchanged.
  - So `blender` is one more branch in `light_of`, not a rewrite of it.
- **Which criterion gives way:** when Blender's own USD export → import isn't an identity
  (the 0.0011 sun suggests it isn't), litegen matches **what Blender's importer reads**, not
  the original scene. The read criterion below is stated that way.

## Interface

- **mesh (`mod.rs`, beside `FbxColorSpace`):**
  - `pub enum LightUnits { #[default] Photometric, Blender }` (`Debug`, `Clone`, `Copy`,
    `PartialEq`, `Eq`, `Default`; no serde, per `mesh-standalone.md`);
  - `ReadOptions.fbx_light_units` and `.usd_light_units`, and `WriteOptions.fbx_light_units`
    and `.usd_light_units`, documented as above;
  - every literal of those structs updated: in `src/mesh/` tests, `proxy/`, `api/` and
    `tests/`;
  - the FBX and USD readers and writers take the value where they take
    `fbx_color_space` today, or the options struct.
- **Converter:**
  - query parameters `fbx_light_units` and `usd_light_units` (`PARAMS` grows to 10), values
    `photometric` and `blender`, case- and space-insensitive as `fbx_color_space`'s are;
  - `fbx_light_units` applies when `from` or `to` is `fbx`, and `usd_light_units` when
    `from` or `to` is `usdz` or `usda`; otherwise 400 `param_unsupported`;
  - an unknown value is 400 `param_invalid`;
  - OpenAPI descriptions as `fbx_color_space`'s: what each value means, who reads which, and
    that `photometric` is three.js's, ufbx's and Storm's reading.
- **Generation:**
  - `pub enum Model3dLightUnits { #[default] Photometric, Blender }` in `types/mod.rs`
    (serde lowercase, `ToSchema`), beside `Model3dFbxColorSpace`;
  - `fbx_light_units` and `usd_light_units` (`Option<Model3dLightUnits>`) on the 3D
    generation request, documented as `fbx_color_space` is there;
  - carried through `providers::…` and the delivery plan, mapped in `proxy/model3d_convert.rs`,
    applied by delivery, and recorded in `meta.delivery`;
  - every struct literal site updated (router, poller, providers, `config_conformance`, tests).
- **Clients stay thin** (`.claude/rules/working-in-this-repo.md`):
  - the SDKs regenerate from the OpenAPI document (`sdks/scripts/regen-all.sh`, then
    `node apps/landing/scripts/sync-openapi.mjs`);
  - hand-written SDK doc comments that list `fbx_color_space` (TS `client.ts`, Python
    `types.py` and `client.py`) list the two new options the same way;
  - MCP's tools are derived from the OpenAPI document (`docs/mcp/tools.md` lists
    `fbx_color_space` at L446, L466 and L506), so they gain the two options with no code.
    `docs/mcp/tools.md` is regenerated (`cargo run --example mcp_tools_doc >
    ../docs/mcp/tools.md` from `litegen-core`), and `mcp/tests.rs` (~L713) fails while it's
    stale;
  - the dashboard's source doesn't expose `fbx_color_space`, so it gets nothing.
- **litegen-d0's converter package (my call, after the builder check).**
  - `packages/3d-converter/` (Rust `core/`, the PyO3 `python/` binding and its `.pyi`, the
    wasm `js/` binding, `scripts/parity.mjs`) and `apps/3d-converter/` (the UI:
    `OptionsPanel.tsx`, `ConverterPage.tsx`) expose `fbx_color_space`, so they get parity:
    both options, the same values, the same per-end rules.
  - litegen-d0 owns those trees, a separate cargo workspace with its own toolchain, so
    **d0 does that part**, from the exact shapes the controller sends it before the mesh
    change lands.
  - The mesh change must never leave d0's workspace uncompilable. Its option literals
    (`core/src/format.rs` ~L96, `core/src/model.rs` ~L75 and ~L162) build the structs field
    by field. So the controller asks d0 either to switch them to `..Default::default()`
    first, or to land its parity in the same window, and waits for its answer before adding
    the fields.
- **Same-format conversions (my call).** With one option per format, applied at each end,
  fbx → fbx (or usdz → usda) with `blender` reads Blender units and writes them back, which
  is a no-op. That follows `fbx_color_space`'s precedent, and the OpenAPI text says so. To
  retarget a Blender file to photometric, a caller converts twice (fbx → glb with
  `blender`, then glb → fbx). Separate read-end controls, like `from_units`/`from_up`, are a
  non-goal; they're additive later if asked for.
- **Engine boundary:** `mesh/`, `types/`, `proxy/model3d_convert.rs` and
  `proxy/model3d_deliver.rs` are engine; `api/handlers/` is server. Nothing new crosses.

## What proves it is done

- **Unit (mesh):**
  - for each light kind and each format, `blender` writes exactly 100 × E (FBX) and the cited
    USD values;
  - reading inverts them;
  - with `photometric`, writing `fixtures::staged()` (and the other fixtures with lights) to
    FBX, USDA and USDZ is byte-identical to the default options' bytes;
  - write with `blender` then read with `blender`: each light's intensity is within 1e-6
    relative;
  - USD `customData` still wins on read under either option;
  - the sweeps aren't needed: no new parsing.
- **Real Blender** (the local bundle, headless; `sys.dont_write_bytecode = True` first, and
  nothing written inside that project), with `staged`:
  - **Write:**
    - its FBX written with `blender` opens through `wm.fbx_import` (the default) with each
      light's `energy` within 1e-4 relative of what Blender's glTF import of `staged`'s GLB
      gives;
    - the legacy importer's reading is recorded beside it. In 5.2.0 that importer raises
      on any file with a light (`import_fbx.py` L2255, `cast_shadow`), so its factor is
      read through Task 1's stub;
    - its USDZ (or USDA) written with `blender` does the same through Blender's USD importer.
  - **Read:** Blender's own FBX and USD exports of that imported scene, read by litegen with
    `blender`, give each light within 1e-4 relative of E⁻¹ of the `energy` Blender's own
    default importer reads from that same file. Blender re-imports its export, and that's
    the comparison.
    - For FBX, that equals `staged`'s glTF values: the exporter's ×100 and the importer's
      /100 cancel, which the check confirms.
    - For USD, it equals them only if Blender's own USD round trip is an identity. Where it
      isn't, the check records the factor, and the docs name it.

  This proves the factors, the USD sun's included. Keep the Blender script and its output
  in the ledger.
- **Converter API:**
  - each option is accepted at its end and refused elsewhere (400 `param_unsupported`);
  - a bad value is 400 `param_invalid`, and case and spaces don't matter;
  - with the option, the FBX and USD bytes differ from the default's only in the lights.
- **Generation API:** the per-request override lands in `meta.delivery`, and a bad value is
  refused, as `fbx_color_space_and_format_source_are_per_request_overrides` does.
- **OpenAPI, SDKs and MCP:** the OpenAPI document, the SDKs and `docs/mcp/tools.md` are
  regenerated, with the SDK tests and `mcp::tests` green.
- **litegen-d0's converter package:** at parity, with d0's own suites green, as d0 reports
  them.
- **The overview's full gate** passes.

## Docs

- `3d-format-conversion.md` §4 (FBX lights) and §5 (USD lights): replace "an opt-in
  light-units option … is a possible follow-up, the user's decision" with the options and
  their factors.
- The code docs that say the same:
  - `fbx_write.rs` `light_attribute`;
  - `fbx_read.rs`'s module doc;
  - `usd_write.rs` `SPHERE_INTENSITY_PER_CANDELA`;
  - `usd_read.rs`'s module doc.
- The converter's OpenAPI text, and the generation request's field docs.
- The matrix page's FBX and USD lights cells, through the page's steward.

## Left to the builder

- The helpers' names, and how the readers and writers receive the value: the options
  struct, or one more argument beside `FbxColorSpace`.
- Blender's USD factors, read and cited as above.
- Whether `LightUnits` gets a `pub fn as_str` for messages and docs.
