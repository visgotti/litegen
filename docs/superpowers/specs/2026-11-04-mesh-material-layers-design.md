# Glass, clearcoat and the other material layers — design

Part of the mesh-gaps set ([overview](2026-11-04-mesh-gaps-overview-design.md)). Size: M.
Changes the scene model, and comes first of items 4, 5 and 10, which all add to
`scene.rs`, `Capabilities` and the codecs.

## Why

`Material` holds glTF's core metallic-roughness model plus emissive strength and unlit.
Everything glTF's PBR extensions add, and everything USD's `UsdPreviewSurface` adds
beyond the core, is dropped on read with a warning:

- **glTF:** `KHR_materials_transmission`, `volume`, `dispersion`, `ior`, `clearcoat`,
  `sheen`, `specular`, `iridescence` and `anisotropy` are on
  `IGNORABLE_REQUIRED_EXTENSIONS` but not `CARRIED_EXTENSIONS`, so `uncarried_extensions`
  warns "…: transmission: glass and other see-through materials render opaque".
  GLB → GLB loses them too, though glTF holds every one.
- **USD:** `inputs:clearcoat` → "material … has clearcoat, which is not carried"
  (`usd_read.rs`). `ior` is ignored.
- **OBJ:** `Pc`, `Pcr`, `Ps`, `aniso` and `anisor` are dropped with a warning, and `Ni`
  is ignored silently (`obj.rs`, beside `ka`, `ks` and `illum`).
- **FBX:** the Arnold, Stingray, 3ds Max Physical and OpenPBR families are read for
  their base layer only.

Vendors already emit some of these. Rodin and Hunyuan PBR outputs carry `specular`, and
glass and car paint are clearcoat and transmission in every DCC export. A GLB that
round-trips through litegen comes back flatter than it went in.

## Goals

1. The scene carries every glTF PBR extension listed above, with its textures. GLB →
   GLB and glTF → glTF are lossless for all of them.
2. Each other container keeps the layers it has a slot for, and reports the rest by
   name:
   - USD keeps clearcoat, clearcoat roughness and ior, and reports transmission and the
     rest;
   - OBJ+MTL keeps `Pc`, `Pcr`, `Ps`, `Ni`, `aniso` and `anisor`;
   - Collada keeps `index_of_refraction`;
   - FBX reads the DCC families' layers.
3. No silent drop anywhere: every layer a target can't hold is a `materials` loss
   naming the layer and the material count.

## Non-goals

- `KHR_materials_variants`. It holds alternative material bindings, not a layer, and
  stays warned.
- MaterialX, OpenPBR or MDL networks in USD output. UsdPreviewSurface stays the one USD
  shading model, because it's what RealityKit and every USD reader draws. The
  research (Q10) found MaterialX `gltf_pbr` carries all ten layers and passes
  `usdchecker`, but three.js and SceneKit ignore it, so it's a later opt-in.
- Writing FBX layer properties. FBX output stays Phong with Blender's PBR mapping, the
  form every FBX reader understands, and layers are reported lost.
- `KHR_materials_pbrSpecularGlossiness` (archived by Khronos). It's refused as today if
  required.

## Decisions (mine)

### The scene

`Material` gains these optional layers, each with glTF's field names, defaults and
units, so that the glTF reader and writer are a direct mapping:

```rust
pub clearcoat: Option<Clearcoat>,       // factor, texture (R), roughness, roughness_texture (G), normal_texture, normal_scale
pub transmission: Option<Transmission>, // factor, texture (R)
pub volume: Option<Volume>,             // thickness, thickness_texture (G), attenuation_distance (f32::INFINITY = none), attenuation_color
pub ior: Option<f32>,                   // KHR_materials_ior, default 1.5 when absent
pub specular: Option<Specular>,         // factor, texture (A), color, color_texture (RGB, sRGB)
pub sheen: Option<Sheen>,               // color, color_texture (sRGB), roughness, roughness_texture (A)
pub iridescence: Option<Iridescence>,   // factor, texture (R), ior, thickness_min, thickness_max, thickness_texture (G)
pub anisotropy: Option<Anisotropy>,     // strength, rotation, texture (RG direction, B strength)
pub dispersion: Option<f32>,
```

- `TextureSlot` gains one slot per layer texture, and `texture_refs()` yields them.
  `is_color()` is true for `SpecularColor` and `SheenColor`.
- **A new `Material::texture_refs_mut()`** yields every slot mutably, base and layers.
  These sites list the five base slots by name today, so they don't follow
  `texture_refs()` (builder check, 2026-11-04), and each moves to it. The line numbers
  predate item 11, which grew `pose.rs`.
  - `pose.rs`'s texture-transform baking (two places, near lines 1133 and 1155);
  - `pose::prune_unused_textures`' index remap (near 1206): it marks through
    `texture_refs` but remaps the five, so a layer-only texture would keep a stale
    index;
  - `pose::degrade`'s material reduction (near 1064–1086);
  - `normalize::transcode_textures`' KTX2 drop (near 516);
  - `usd::prepare` (near 198);
  - `dae::prepare` (near 380);
  - item 11's UV-set plan in `pose.rs`: `sampled_set_order`, the `TextureRef.uv_set`
    remap and `order_sampled_sets_first`. `degrade`, `scene::losses` and the Collada,
    FBX and 3MF `prepare`s share it. It orders a primitive's sets by the slots the
    target keeps (item 11's ruling P3a), so each layer texture slot joins that order,
    after emissive, wherever the target's `LayerSupport` keeps the layer. Of the
    containers with a set limit, only USD keeps layer textures (clearcoat and clearcoat
    roughness), so USDZ's two sets can now go to a clearcoat map. Collada, FBX and 3MF
    keep no layer textures, so the slot lists they pass don't change.

  Each gets a test that a texture used only by a layer slot is handled there: baked,
  remapped, kept, dropped or reported. After this item, a slot added later can't be
  missed, because no site lists every slot by name. A container's list of the slots it
  keeps is a statement of its capabilities, not such a site.
- `validate` checks every new factor is finite, and that `ior` is within
  `KHR_materials_ior`'s schema (its minimum, plus any special value the schema allows;
  the builder pins the bounds from the schema file and cites it).
- `Capabilities` gains `layers: LayerSupport`, a struct of nine bools
  (`clearcoat … dispersion`). `full()` sets all of them, and `intersect` ANDs them.
  `losses` reports each layer present in the scene that the target lacks: kind
  `materials`, what "clearcoat on 2 material(s)", why "{target} has no clearcoat layer;
  the base layer is kept".

### Per container

| Container | Keeps | Mapping |
|---|---|---|
| glTF | everything | the nine extensions, read and written, each added to `extensionsUsed` (never `extensionsRequired`: a reader without them draws the base layer, which is the spec's intent) |
| USD | clearcoat, clearcoat roughness, ior | `inputs:clearcoat`, `inputs:clearcoatRoughness` (with textures through `UsdUVTexture` channels `r` and `g`), `inputs:ior`. Transmission, volume and the other layers are reported. An opacity approximation of transmission was considered and rejected (builder check): `opacity = 1 − t` makes full glass invisible in every USD reader, and it can't be inverted when the base alpha is textured, MASK or BLEND |
| OBJ + MTL (package) | clearcoat, sheen (weight), ior, anisotropy | `Pc`, `Pcr`, `Ps`, `Ni`, `aniso` and `anisor`, the PBR extension to MTL that Blender reads and writes. A sheen colour becomes `Ps` = its max channel, reported |
| Collada | ior | `<index_of_refraction>` in the profile_COMMON effect |
| FBX read | from Arnold `aiStandardSurface` (`Maya\|coat`, `Maya\|coatRoughness`, `Maya\|transmission`, `Maya\|sheen`, `Maya\|sheenColor`, `Maya\|specularIOR`, …), 3ds Max Physical, OpenPBR (`coat_weight`, `transmission_weight`, `fuzz_weight`, `specular_ior`, `thin_film_*`, `specular_roughness_anisotropy`) | each property name is pinned by a test from a real file or the vendor's documentation |
| FBX write, 3MF, PLY, STL | — | reported |

### Readers that see layers they can't render

The dashboard's three.js `GLTFLoader` renders all nine as `MeshPhysicalMaterial`, so
keeping them changes how the dashboard draws such files. That's the point. A GLB that
needed transmission rendered opaque before.

## What proves it is done

### Unit tests, RED first

- **Every `texture_refs_mut` site:** one test each, with a texture used only by
  `sheen.color_texture` (or `clearcoat.texture`):
  - `a_layer_texture_transform_is_baked`;
  - `pruning_remaps_a_layer_texture` (the index stays valid after an earlier texture is
    pruned);
  - `degrade_reduces_layer_textures`;
  - `a_ktx2_layer_texture_is_dropped_and_its_layer_kept`;
  - `usd_prepare_handles_layer_textures`;
  - `dae_prepare_handles_layer_textures`;
  - `a_layer_texture_keeps_its_uv_set_in_usdz`: base colour on set 0, a clearcoat
    texture on set 2 and a sheen colour texture on set 1 → USDZ keeps sets 0 and 2, and
    the clearcoat binding names set 1. The sheen texture is reported once, with the
    sheen layer (a `materials` loss).
- **scene.rs:** `layer_textures_are_texture_refs`; `layer_factors_must_be_finite`;
  `ior_outside_the_schema_is_refused` (the schema's bounds, cited);
  `losses_name_each_missing_layer`.
- **glb.rs:**
  - `every_pbr_extension_round_trips`: `fixtures::layered()` → GLB → read gives
    `Material` `==` in every field. The written JSON's `extensions` objects equal a
    hand-written expected JSON per extension, key for key, with the defaults omitted
    as glTF writers do;
  - `layered_glb_validates`: the Khronos validator, run in the harness, gives 0 errors
    and 0 warnings on `layered()`'s GLB and glTF;
  - `layers_are_never_required`: no layer appears in `extensionsRequired`.
- **usd:**
  - `clearcoat_and_ior_round_trip_through_usd`: factors and textures equal, with the
    textures' channel outputs `r`/`g` asserted in the written USDA text;
  - `usd_reports_transmission_and_keeps_alpha`: `layered()` → USD. The loss names
    transmission, and the written `opacity` is the base alpha, unchanged by
    transmission.
- **obj:** `mtl_pbr_extension_round_trips` (`Pc`, `Pcr`, `Ps`, `Ni`, `aniso`, `anisor`
  written and read in package mode); `a_sheen_colour_is_reduced_and_reported`.
- **dae:** `index_of_refraction_round_trips`.
- **fbx:** one test per family, `arnold_layers_read`, `physical_layers_read` and
  `openpbr_layers_read`, each from a hand-built FBX DOM with the exact property names.
  Each names the source of those names (vendor docs URL or the real file it was taken
  from) in a doc comment.
- **Matrix:** `scene_matrix_tests` gains `layered()`. For every pair, the layers
  survive exactly where `LayerSupport` says they do, and the loss list names exactly
  the rest.

### Render harness

- **New `lit` mode.** three.js renders with `MeshPhysicalMaterial` under a fixed
  `RoomEnvironment` PMREM, ACES tone mapping off and a fixed exposure, in the four
  existing camera poses. The expected picture is GLTFLoader's rendering of
  `to.prepare(from.prepare(layered()))`. Thresholds are set from a calibration run of
  the expected file against itself (0.00 %) and of GLB → GLB (must be 0.00 %).
  - glTF outputs must match at 0.00 %.
  - USD outputs are judged by three.js `USDLoader`, which reads `clearcoat` and `ior`
    (cite the line), and by `usdrecord` with a dome light.
  - OBJ outputs are judged by `MTLLoader` for the properties it reads (cite which).
  - Where a loader ignores a property, that comparison is skipped with the citation.
- **Negative controls:**
  - write `clearcoatRoughness` into `clearcoat` in the USD writer: the USD round trip
    test and the lit USD cases fail;
  - drop `KHR_materials_sheen` from the glTF writer: `every_pbr_extension_round_trips`
    and the glTF lit cases fail.

  Restore each by rewriting.

### Third-party files

`--import` (local only, not committed) runs these Khronos glTF-Sample-Assets models:

- ClearCoatTest, ClearcoatWicker, TransmissionTest, TransmissionRoughnessTest,
  AttenuationTest, IORTestGrid, SpecularTest, SheenChair, SheenCloth, IridescenceLamp,
  IridescenceMetallicSpheres, AnisotropyBarnLamp, AnisotropyRotationTest,
  DispersionTest, GlassVaseFlowers and ToyCar.

For each: GLB → GLB keeps every extension object (JSON-equal after key sort), the
validator gives 0 errors, the native check passes in `lit` mode, and every direction
passes.

### Docs

- `3d-format-conversion.md`: §1's materials bullet, and each codec's section;
- `mesh/mod.rs`'s table, with a layers column;
- `glb.rs`'s module doc (moves the extensions from "dropped" to "read");
- `docs/superpowers/research/research-usd-3mf-dae.md` stays as research;
- the matrix page's "Glass, clearcoat, sheen" row (via litegen-a3).

### Gate

The overview's full gate, the harness fixture set with `layered` in all modes including
`lit`, and the Khronos `--import` set above.

## Left to the builder

- The exact `RoomEnvironment` settings and the `lit` thresholds, from the calibration
  run, recorded in `scripts/mesh-verify/README.md`.
- The FBX property names, each pinned by its test.
