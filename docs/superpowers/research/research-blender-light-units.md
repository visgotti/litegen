# Blender 5.2's light-strength factors for FBX and USD

Research date: 2026-11-08, Task 1 of the light-units plan
(`docs/superpowers/specs/2026-11-08-light-units-option-design.md`). No product code was touched.

Every factor below was read from Blender's source, derived from readings out of Blender itself, and checked against
the spec's E definition. The probe wins any disagreement; each one is named in "Where source, probe and spec disagree".

## What was probed

- **Blender:** `bpy.app.version_string` = `5.2.0 LTS`, build hash `fbe6228777e7`, built 2026-07-14. That hash is the
  commit of the GitHub tag `v5.2.0` (`fbe6228777e7d9afefcd61a413844e790ae75db7`, checked through the GitHub API).
  The bundle is a local `Blender.app` (`$BLENDER_APP` below), run headless with
  `PYTHONDONTWRITEBYTECODE=1 … -b --factory-startup --python-use-system-env`.
- **Bundle writes:** `find <bundle> -newer <marker> -type f` printed **nothing** after every run: the version check,
  the operator introspection, the three probe runs and the legacy-importer object check. The marker is
  `.superpowers/sdd/2026-11-08-light-units-option/probe/marker`, touched before the first run. Nothing was written inside
  the bundle.
- **Source:** C++ at the GitHub tag `v5.2.0`, `https://raw.githubusercontent.com/blender/blender/v5.2.0/source/blender/…`.
  The spec cites v5.2.3 for the USD files; the eight C++ files fetched at both tags (the FBX importer, the USD light
  writer and reader and the dome and convert files) are **byte-identical** at v5.2.0 and v5.2.3, so the line numbers
  hold for both. The Python add-ons were read from the bundle itself (`Contents/Resources/5.2/scripts/addons_core/`;
  `io_scene_fbx` bl_info 5.15.0, `io_scene_gltf2` 5.2.39). Copies of the C++ files are in `probe/src/v5.2.0/`.
- **Inputs:** item 5's `staged` fixture in photometric units, from
  `.superpowers/sdd/2026-11-04-mesh-lights-cameras/gate/m-all/staged/src/{glb,fbx,usda,usdz}/model.*`, and Blender's own
  earlier exports `import/BlenderFBX/staged_blender.fbx` and `import/BlenderUSD/staged_blender.usda`. The glTF JSON's
  `KHR_lights_punctual` gives the reference lights: `sun` directional 3 lux, `bulb` point 20 cd, `spot` spot 40 cd
  (cone 0.2 to 0.6 rad).
- **Probe:** `scripts/blender-light-units/probe.py`. Output: `probe/out/readings.json` (every reading), `probe/runs/probe-run.log`
  (raw) and `probe/runs/probe-run.clean.txt` (without the legacy importer's tracebacks), `probe/out/exports/` (the
  files Blender and the probe wrote). Rerun:

  ```
  P=.superpowers/sdd/2026-11-08-light-units-option/probe; touch $P/marker
  PYTHONDONTWRITEBYTECODE=1 $BLENDER_APP/Contents/MacOS/Blender \
    -b --factory-startup --python-use-system-env --python scripts/blender-light-units/probe.py -- \
    --out $P/out --glb <ref.glb> --fbx <a.fbx> … --usd <a.usda|b.usdz> … --synthetic --predict
  find $BLENDER_APP -newer $P/marker -type f   # must print nothing
  ```

  The probe copies every input under `out/inputs/` before importing it, parses the raw `Intensity` / UsdLux values out of
  the files itself (so a factor is "file value / energy Blender reports"), and prints energies next to the glTF
  reference E. `--predict` patches the first FBX and first USDA with the values a `blender`-units litegen would write and
  imports them, so Task 5 can rerun it on its real output.

## The factors (the table Tasks 2 and 3 implement)

E is the energy Blender's glTF importer gives a scene light, as the spec defines it. The default lighting mode is
`SPEC` (rna default of `export_import_convert_lighting_mode`; `io_scene_gltf2/__init__.py` L198-L206).
Directional E = lux / 683 W/m². Point and spot E = cd × 4π / 683 W
(`io_scene_gltf2/blender/imp/light.py` L58-L66 and L69-L77; `PBR_WATTS_TO_LUMENS = 683` at `io_scene_gltf2/blender/com/conversion.py` L10).

| # | Factor | Value | Derived from the readings | Source (tag `v5.2.0` unless noted) | Spec E check |
|---|---|---|---|---|---|
| 1 | glTF import, sun | `energy` = lux / 683 | 3 lux gives 0.0043923864; ratio to spec E 0.99999997 (f32) | `light.py` L58-L60 | defines E |
| 2 | glTF import, point and spot | `energy` = cd × 4π / 683 | 20 cd gives 0.36797571, 40 cd gives 0.73595142; ratio 1.0000000 | `light.py` L69-L71 | defines E |
| 3 | **FBX import, default `wm.fbx_import`** | `energy` = `Intensity` / **100**, all kinds | litegen FBX `Intensity` 300 / 2000 / 4000 gave energy 3 / 20 / 40; Blender's own export of E gave `Intensity` / energy = 100.000 | `io/fbx/importer/fbx_import.cc` L231 `lamp->energy = flight->intensity`; ufbx `extern/ufbx/ufbx.c` L22934-L22937 `light->intensity = Intensity / 100` (default 100 gives 1.0) | write `Intensity` = 100 E; read E = `Intensity` / 100 |
| 4 | **FBX import, legacy `import_scene.fbx`** | the same, `energy` = `Intensity` / 100 | energy 3 / 20 / 40, identical to row 3, but only through a stub (row 5) | `io_scene_fbx/import_fbx.py` L2251 | same as row 3 |
| 5 | Legacy importer as shipped | **raises on every light**, imports nothing | `AttributeError: 'CyclesLightSettings' object has no attribute 'cast_shadow'`, no objects created, one orphan Light datablock whose `energy` was already set | `import_fbx.py` L2254-L2255 (`lamp.cycles.cast_shadow = lamp.use_shadow`, after L2251) | n/a |
| 6 | FBX export (`export_scene.fbx`) | `Intensity` = `energy` × 100 × 2^`exposure` | `Intensity` / E = 100.000 at exposure 0; 2000 for energy 10, exposure 1 (ratio 100 after 2^x) | `export_fbx_bin.py` L605, written at L618; no `Exposure` property is written | the Blender FBX of E: `Intensity` = 100 E |
| 7 | USD export, point and spot | `inputs:intensity` = `energy` / π, normalised SphereLight | E / intensity = 3.1415927 in every row; the glTF scene gave 0.11713031 (bulb) and 0.23426062 (spot) | `io/usd/intern/usd_writer_light.cc` L138-L141; `normalize` L171-L174; radius and `treatAsPoint` L93-L100 | intensity = 4 cd / 683 |
| 8 | USD export, sun | `inputs:intensity` = `energy` / **4**, normalised DistantLight | E / intensity = 4.0000 in every row; the glTF scene gave 0.0010980966 | `usd_writer_light.cc` L134-L137 (comment "Unclear why, but approximately matches Karma") | intensity = lux / (4 × 683) = lux / 2732 |
| 9 | USD import, point and spot | `energy` = `inputs:intensity` × π × `light_intensity_scale` (import option, default 1.0) | energy / intensity = 3.1415926; with `light_intensity_scale` 2 it doubled (6.2831852) | `io/usd/intern/usd_reader_light.cc` L142-L146 | cd = 683 energy / 4π = intensity × 170.75 |
| 10 | USD import, sun | `energy` = `inputs:intensity` × **4** × `light_intensity_scale` | energy / intensity = 4.0000 (8.0000 with scale 2) | `usd_reader_light.cc` L138-L141, L146 | lux = 683 energy = intensity × 2732 |
| 11 | USD export then import (Blender's own), per kind | **identity** for point, spot and sun, to f32 | glTF scene: re-import / E = 1, 1, 1 (sun 1.0000000). Synthetic: 18 lights, worst relative error 6e-8 | writer ÷π and ÷4, reader ×π and ×4 | litegen with `blender` returns the scene's values |
| 12 | FBX export then import (Blender's own), per kind | **identity** at exposure 0, both importers (at exposure x the export bakes 2^x into `Intensity`, so the re-import has energy × 2^x and exposure 0) | re-import / E = 1 in all three kinds | ×100 (L605) then ÷100 (row 3) | same |
| 13 | Collada | **no operator in 5.2.0** | `dir(bpy.ops.wm)` has no `collada_*`; `get_rna_type` for `WM_OT_collada_import` raises KeyError; `import_scene` and `export_scene` list only `fbx` and `gltf` | n/a | no `dae_light_units` follow-up |

In litegen terms, with the scene's lux or cd and E from rows 1-2:

| Light | FBX `Intensity`, `photometric` (today) | FBX `Intensity`, `blender` | USD `intensity`, `photometric` (today) | USD `intensity`, `blender` |
|---|---|---|---|---|
| sun, L lux | 100 L | 100 E = L × 0.146412884 | L | E / 4 = L / 2732 = L × 0.000366032 |
| point or spot, I cd | 100 I | 100 E = I × 1.839878567 | 4 I | E / π = 4 I / 683 = I × 0.005856515 |
| read back, from the file value | n/a | lux = 6.83 × `Intensity`; cd = 0.543514131 × `Intensity` | n/a | lux = 2732 × intensity; cd = 170.75 × intensity |

`staged` with `blender` units (what Task 2 and 3's tests should expect, f64): E = 0.004392386530, 0.367975713451 and
0.735951426902 for the sun, bulb and spot. FBX `Intensity` = 0.439238653001, 36.797571345122, 73.595142690244. USD
`inputs:intensity` = 0.001098096633, 0.117130307467 (= 80/683), 0.234260614934 (= 160/683).

### Checked three ways

- **Derived from the readings.** Every row above has its reading in the probe's "STEP 5" block.
- **Against the source.** Each row cites the line. Blender's writers and readers are exact inverses (rows 11 and 12), as
  the source predicts: ÷π against ×π, ÷4 against ×4, ×100 against ÷100.
- **Against E.** The probe wrote the `blender`-units values (STEP 4c, `--predict`) into copies of litegen's own `staged`
  FBX and USDA, using the formulas in the second table, and imported them. Every light's `energy` equals E:

  | File, importer | sun / E | bulb / E | spot / E |
  |---|---|---|---|
  | FBX, `wm.fbx_import` (default) | 0.99999997 | 1.0000000 | 1.0000000 |
  | FBX, legacy with the Cycles stub | 0.99999997 | 1.0000000 | 1.0000000 |
  | USDA, `wm.usd_import` | 0.99999997 | 1.0000000 | 1.0000000 |

  The 0.99999997 is float32 rounding of the sun's tiny E (0.0043923864 against 0.0043923865).

## Probe readings

All energies are `bpy.data.lights[].energy`. "x E_spec" is energy divided by the spec's E.

### The `staged` scene, one light per kind (probe STEP 1 and STEP 5)

| File and importer | sun energy | bulb energy | spot energy | x E_spec (sun / bulb / spot) |
|---|---|---|---|---|
| `staged.glb`, `import_scene.gltf` (`SPEC`) | 0.0043923864 | 0.36797571 | 0.73595142 | 1 / 1 / 1 |
| litegen FBX (`Intensity` 300 / 2000 / 4000), `wm.fbx_import` | 3 | 20 | 40 | 683 / 54.351413 / 54.351413 |
| litegen FBX, legacy with Cycles stub | 3 | 20 | 40 | 683 / 54.351413 / 54.351413 |
| litegen USDA and USDZ (`intensity` 3 / 80 / 160), `wm.usd_import` | 12 | 251.32741 | 502.65482 | 2732 / 683 / 683 |
| Blender's earlier FBX (`Intensity` 0.4392 / 36.80 / 73.60), `wm.fbx_import` | 0.0043923864 | 0.36797571 | 0.73595142 | 1 / 1 / 1 |
| Blender's earlier USDA (`intensity` 0.0010981 / 0.1171 / 0.2343), `wm.usd_import` | 0.0043923864 | 0.36797571 | 0.73595142 | 1 / 1 / 1 |

Spot cones read the same through every importer and match the glTF import: `spot_size` 1.2 rad, `spot_blend` 0.66666669.
Radius 0 and `normalize` True match as well. litegen's `customData` on the USD prims is ignored by Blender.

### Blender's export of the glTF-imported scene, then re-import (STEP 3)

| Kind | E | FBX `Intensity` | FBX re-import (default / legacy+stub) | USD `intensity` | USD re-import |
|---|---|---|---|---|---|
| sun | 0.0043923864 | 0.43923864 | 0.0043923864 / 0.0043923864 | 0.0010980966 | 0.0043923864 |
| bulb | 0.36797571 | 36.797571 | 0.36797571 / 0.36797571 | 0.11713031 | 0.36797571 |
| spot | 0.73595142 | 73.595142 | 0.73595142 / 0.73595142 | 0.23426062 | 0.73595142 |

The USD export also writes a `DomeLight` named `env_light` (intensity 1) for the world; its re-import made no light object.

### Blender-authored lights of known energy (STEP 4a, 18 lights)

- **FBX export.** `Intensity` / (energy × 2^exposure) = 100.000 for every light: energies 0.5, 1, 10, π, 200000,
  exposure 0 and 1, radius 0.5, non-normalised, sun angle 0.5, spot cone 0.8. Radius, normalize and angle do not touch
  `Intensity`. Re-import with exposure 1 and energy 10 gave energy **20** and `exposure` 0 through the default importer and the
  stubbed legacy one: the exporter bakes 2^exposure into `Intensity` and writes no `Exposure` property. Re-import energy
  against energy × 2^exposure agreed within 3e-8 for all 18 lights and both working importers.
- **USD export.** Point and spot: energy / `intensity` = 3.1415927 for every light. Sun: 4.0000. Exposure is written
  **separately** as `inputs:exposure`, not folded into the intensity: energy 10 with exposure 1 wrote intensity
  3.1830988 (= 10/π) and exposure 1, and re-imported as energy 10 with `light.exposure` 1. Radius 0.5, non-normalised
  and sun angle 0.5 left the intensity alone. The writer is sparse, so a value equal to the schema fallback is omitted:
  energy π (intensity 1, the SphereLight fallback) and energy 200000 (intensity 50000, the DistantLight fallback) wrote
  **no `inputs:intensity`** and re-imported exactly as π and 200000.
- USD re-import energy against the original agreed within 6e-8 for all 18 lights, and `light.exposure` came back equal.

### Hand-written UsdLux files (STEP 4b)

`wm.usd_import` of lights that vary one input at a time, `light_intensity_scale` 1 (default) then 2:

| Input varied | Effect on `energy` | Where it went |
|---|---|---|
| `inputs:intensity` 1 and 5 | ×π (sphere), ×4 (distant) | `energy` |
| `inputs:exposure` 1 | none | `light.exposure` = 1 |
| `inputs:normalize` 0 | none | `light.normalize` False. **Not authored also gives False**: the UsdLux fallback is false, which Blender reads as unnormalised. litegen writes `inputs:normalize = 1`, so its files import as normalised |
| `inputs:radius` 0.5 | none | `light.shadow_soft_size` 0.5 (not authored also gives 0.5, the schema fallback) |
| `treatAsPoint` 1 with radius 0.5 | none | `light.shadow_soft_size` 0 |
| `inputs:angle` 3 | none | `light.angle` = 2 × 3° in radians (0.10471976); not authored gives the UsdLux fallback 0.53°, so 0.0185 rad |
| `inputs:intensity` not authored | sphere: π (fallback 1); distant: **200000** (fallback 50000, ×4) | `energy` |
| import option `light_intensity_scale` 2 | ×2 on every `energy` | `energy`; the option's default is 1.0 |
| `ShapingAPI` with `inputs:shaping:cone:angle` authored | none | type becomes SPOT, `spot_size` = 2 × cone angle, `spot_blend` = softness |

So `energy` folds exactly three things: `inputs:intensity` (or its schema fallback), the light's kind, and the import
option `light_intensity_scale`. `normalize`, `exposure`, `radius`, `angle` and the cone are stored in other Blender
fields (`usd_reader_light.cc`: radius L88-L98, cone L101-L118, angle L125-L130, exposure L150-L156, normalize
L200-L208). The importer's colour (L158-L167), temperature, diffuse and specular do not touch `energy` either.

### Predicted `blender`-units files (STEP 4c)

See "Checked three ways": energy / E_spec = 1 for all three kinds through the default FBX importer, the stubbed legacy
importer and `wm.usd_import`.

## Citations in one place

- `source/blender/io/fbx/importer/fbx_import.cc` (v5.2.0): `import_lights` L200-L250; types L212-L226; spot angles
  L221-L222; **`lamp->energy = flight->intensity` L231**; `lamp->exposure` from the `Exposure` property L232. The File >
  Import > FBX menu entry is `wm.fbx_import` (`startup/bl_ui/space_topbar.py` L388 in the bundle).
- `extern/ufbx/ufbx.c` (v5.2.0, the ufbx Blender ships): `ufbxi_update_light` L22932; the intensity line L22937 with the
  comment at L22934-L22936 ("FBX seems to store intensities 100x of what's specified in at least Maya and Blender").
  An excerpt is in `probe/src/v5.2.0/ufbx_update_light_excerpt.txt`. v5.2.3 has the same line at L22937.
- `io_scene_fbx/import_fbx.py` (bundle): `blen_read_light` L2222; `lamp.energy` L2251; `lamp.exposure` L2252; the failing
  `lamp.cycles.cast_shadow` L2254-L2255. `io_scene_fbx/export_fbx_bin.py`: `intensity = lamp.energy * 100.0 * pow(2.0,
  lamp.exposure)` L605; the `Intensity` property L618. The legacy menu entry is `io_scene_fbx/__init__.py` L716.
- `source/blender/io/usd/intern/usd_writer_light.cc` (v5.2.0 and v5.2.3): sphere and spot L90-L118; sun L119-L127;
  **intensity L133-L141** (sun `energy / 4.0f` L136; others `energy / M_PI` L140); exposure L145-L148; normalize
  L171-L174.
- `source/blender/io/usd/intern/usd_reader_light.cc` (v5.2.0 and v5.2.3): sphere L82-L119; distant L120-L132;
  **intensity L134-L148** (sun `* 4.0f` L140; others `* M_PI` L144; `* light_intensity_scale` L146); exposure L150-L156;
  normalize L200-L208.
- `io_scene_gltf2/blender/imp/light.py` L58-L77 and `io_scene_gltf2/blender/com/conversion.py` L10 (E); the glTF exporter's inverse, for
  reference, is `blender/exp/lights.py` L158-L179 (it also multiplies by 2^exposure, and by the area when a light is not
  normalised).

## Where source, probe and spec disagree

The probe wins every one. Source and probe agree on all thirteen rows of the factor table. These are the disagreements
with the spec text or with a first reading of the source:

1. **The legacy FBX importer does not work in Blender 5.2.0.** The source reads `energy = Intensity / 100` at L2251, which
   suggests an importer one could compare. The probe: the operator raises at L2255 on any file with a light and creates no
   objects. The energy is only readable from the orphan Light datablock (the first light only). The probe runs it both
   ways: as shipped (one orphan light) and with a stub `cast_shadow` property added to Cycles' light settings (all lights,
   then removed). The probe's reading is `Intensity / 100`, so the spec's choice stands (the default `wm.fbx_import` wins
   and the two importers agree on the factor), but a "both importers" check in Task 5 has to use the stub. This is
   reproduced in `--factory-startup` background mode. The cause is a property missing from the bundled Cycles add-on, so
   the GUI should fail the same way; that was not tried.
2. **Blender's own USD round trip is an identity for every kind.** The spec expected it might not be ("the 0.0011 sun
   suggests it isn't"). The 0.0011 is only what litegen's current photometric reader makes of Blender's `intensity` =
   E / 4 (it reads the value as lux). Blender's reader multiplies it by 4 again. So no criterion gives way: reading with
   `blender` returns the glTF scene's values for FBX and for USD, and the spec's "Which criterion gives way" clause is moot.
3. **The brightness ratios in "Why" were right for FBX only.** The spec first said litegen's FBX and USD lights open
   about 54× (point and spot) and 683× (sun) brighter than the glTF scene. It was corrected from this note the same day,
   and now gives the USD figures below. Measured: FBX is 54.35× and 683×. **USD is 683× for
   point and spot and 2732× for the sun** (litegen writes 4 cd and Blender multiplies by π, so 4π cd against cd × 4π / 683;
   litegen writes lux and Blender multiplies by 4, so 4 lux against lux / 683). The same factors apply the other way
   when litegen reads Blender's USD today. This changes no formula.
4. **Blender's importers do read `Exposure`.** The spec says the importer "ignores" it. Both FBX importers copy the
   `Exposure` property into `light.exposure` (C++ L232, Python L2252) and the USD reader copies `inputs:exposure` the same
   way (L150-L156); neither adds it to `energy`. So "ignored" is true of `energy`, which is what the spec compares, but
   Blender's `Light.exposure` ("multiplying the intensity by 2^exposure") still brightens the render. See the concern
   below.
5. **ufbx line numbers.** `light_attribute` cites ufbx.c L23049-L23052; the ufbx that Blender 5.2.0 ships has the same
   expression at L22937. Different ufbx revisions, same behaviour.
6. **No Collada.** The spec's "(my call, to confirm)" is confirmed: 5.2.0 has no Collada operator.

## Things the controller should know for Tasks 2 and 3

- **Exposure on read.** The `blender` read compares `energy`, so it ignores `inputs:exposure` and FBX `Exposure`. A Blender
  USD with a non-zero exposure keeps it separate from the intensity (probe), and Blender renders it brighter by 2^exposure.
  litegen's existing photometric USD reader folds `exposure.exp2()` into the value (`usd_read.rs` ~L866). The spec says to
  follow the importer, so no fold; folding would instead follow Blender's own glTF exporter (`lights.py` L179). It is a
  one-line choice for Task 3, and Blender's FBX export bakes the exposure into `Intensity` either way.
- **Absent `inputs:intensity`.** Blender's writer omits it whenever it equals the schema fallback (1 for a sphere, 50000
  for a distant light), and Blender's reader applies those fallbacks (energy π and 200000). `usd_read.rs` already uses the
  same fallbacks (`unwrap_or(if distant { 50000.0 } else { 1.0 })`), so the `blender` branch must keep them.
- **Other inputs that stay out of `energy`:** sun angle (litegen writes 0; Blender's glTF import gives 0.00918 rad),
  radius and `normalize`. Not a unit factor, and litegen's `treatAsPoint` radius 0 matches the glTF import's radius 0.
  One trap on read: a hand-written light with no `inputs:normalize` imports as non-normalised.
- **Float precision.** Blender stores `energy` as f32, so the readings agree to about 3e-8 relative. The spec's 1e-4
  tolerance is comfortable.
- **Not covered:** the GUI importer menus, `wm.usd_import`'s `scale` and `apply_unit_conversion_scale` options (the light
  path in `usd_reader_light.cc` does not use them), and lights beyond point, spot and sun (area lights have their own
  branches, out of scope per the spec).
