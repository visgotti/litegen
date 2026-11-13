//! USDZ, USDA and USDC — Pixar's Universal Scene Description, packaged, as
//! text and as a binary crate.
//!
//! # Why USD, and why these three spellings
//!
//! USDZ is what iOS opens in AR Quick Look and what Apple's tools, Reality
//! Composer and a growing share of 3D vendors ship; a model that should "open on
//! an iPhone" has to be one. USDA is the same scene as plain text: the layer
//! inside a USDZ, what pipelines diff and patch, and the format most USD
//! tooling round-trips. USDC is the same layer as a binary crate: what Apple's
//! packagers and most DCC exporters write, and smaller and faster to open.
//!
//! - **Writing** produces USDA text first, in every case. A USDC is that
//!   text parsed back into a layer and encoded as a crate (`usdc::write`,
//!   crate version 0.8.0: uncompressed arrays and literal-only LZ4, see
//!   `usdc.rs`), so the two encodings hold one layer and read as one scene.
//!   A USDZ's root layer is USDA text unless `WriteOptions::usdz_root` asks
//!   for a crate (`model.usdc`, as Apple's own packagers write it). The
//!   research behind this module verified that
//!   `usdchecker --arkit` accepts a usda root layer (the rule demanding usdc
//!   was removed in OpenUSD 24.03) and that `<model-viewer>` hands exactly
//!   that to Quick Look. A USDZ is a ZIP whose
//!   entries are stored, 64-byte aligned, root layer first, sizes and CRC in
//!   the local headers (see `zip.rs`); textures go in as `textures/<name>`.
//! - **Reading** accepts text and binary crate layers (`usdc.rs`) in
//!   either container, composes them (`usd_stage.rs`) and maps the stage to
//!   the scene (`usd_read.rs`). The USDA and USDC readers are one reader:
//!   either takes either encoding, since a crate file is one layer like a
//!   text one (only the name a refusal gives differs).
//!
//! # What a USD file holds (the capabilities row)
//!
//! Hierarchy, normals, uv sets (eight in a USDA or USDC, two in a USDZ; see below),
//! vertex colours, UsdPreviewSurface (metallic-roughness PBR, with glTF's
//! clearcoat and ior layers; it has no input for the others), textures (USDZ
//! always; USDA and USDC only as a package, beside it), wrap modes, skins, morph
//! targets, animation (every clip of a USDA or USDC, one SkelAnimation each,
//! switched by Apple's variant set `Animation`; a USDZ keeps its first clip
//! alone unless `WriteOptions::usd_clips` asks for `All`; either way only the
//! first clip's rigid motion, its transform ops, is kept, because a variant
//! switches skeleton bindings and a transform op has one value; see
//! `usd_write.rs`, "Clips"), unlit materials (UsdPreviewSurface has no unlit
//! mode, so a recipe and a litegen marker; `usd_write.rs`, "Unlit"), and
//! glTF's punctual lights and cameras (UsdLux's
//! `DistantLight` and `SphereLight`, and `UsdGeomCamera`; `usd_write.rs`
//! gives the mapping and Ruling R6, and `usd_read.rs` how other writers'
//! are read; `WriteOptions::usd_light_units` and `ReadOptions::usd_light_units`
//! choose the schema's photometric units or Blender's `energy`, both ways).
//! Points (`Points` prims) and lines (linear `BasisCurves`, one
//! 2-vertex curve for each segment): `usd_write.rs` gives the shapes, widths
//! and Rulings I10-R7 to I10-R9 of mesh-gaps item 10, and `usd_read.rs` how
//! they are read. Not texture filters (UsdUVTexture has none) and
//! not texture transforms as such: UsdTransform2d exists, but three.js's USD
//! loader applies it in the wrong order and sense and RealityKit honours one
//! per material, so transforms are baked into the coordinates, which every
//! reader draws right.
//!
//! # UV sets
//!
//! - **Written** as `primvars:st`, `st1` … `st7` (`texCoord2f[]`, one value
//!   per vertex, V flipped to USD's bottom-left origin), `st` first:
//!   three.js's loader takes the first texture-coordinate primvar as its `uv`,
//!   and finds `st1` only by that name. A texture reads set N through a
//!   `UsdPrimvarReader_float2` whose varname is set N's.
//! - **A USDA or USDC holds eight**, as many as the reader keeps. three.js's loader
//!   builds only `uv` and `uv1`, so it cannot draw a texture on a later set,
//!   a reader limit; Storm, SceneKit and `usdchecker --arkit` handled ten
//!   (`docs/superpowers/research/research-consumers.md`, Q7).
//! - **A USDZ holds two,** because RealityKit draws it (AR Quick Look) and
//!   takes two: "RealityKit supports two UV Sets." They are the two its
//!   textures sample: where a material samples a set past the second,
//!   `pose::degrade` moves its sampled sets to the front, base colour's
//!   first, before it keeps two, so a texture on a later set still draws on
//!   a device.
//! - **Read by name.** A mesh's sets are `st`, then `st1` … `st7` as the mesh
//!   has them, in suffix order with gaps closed, then any other primvar its
//!   material's textures read (Blender's `UVMap`), in the order they are
//!   first read: eight at most. A texture's set is its primvar's place in
//!   that list, so this module's files read back as written. A mesh with no
//!   `st` takes its first `texCoord2*` primvar as set 0 instead: with two
//!   such primvars and a material reading the second, it has two sets, the
//!   texture on set 1, and a one-set target that keeps textures keeps that
//!   set.
//!
//! [`prepare`] reduces a scene to exactly what the writer encodes, beyond the
//! generic reduction: one timeline, its clips (the first alone, or with
//! `UsdClips::All` every one, the others keeping what a variant can switch,
//! their skeletons' and blend shapes' motion: a transform op has one value, so
//! only the first clip's rigid motion is kept),
//! linear keys (USD interpolates nothing else; STEP becomes a 1 ms ramp,
//! CUBICSPLINE is resampled), PNG/JPEG only, an unlit material kept as its base
//! colour and opacity alone (written as a recipe and a marker, `usd_write.rs`,
//! "Unlit"; metallic and roughness 1, and no maps or layers beside its base),
//! no tangents, the base normal map as
//! every clearcoat's (UsdPreviewSurface's clearcoat uses it), no vertex
//! colour under a base-colour texture (UsdPreviewSurface cannot multiply the
//! two), skeletons whose joints are parented to joints, joint scales at
//! half precision (UsdSkel's `half3[]`), each light and camera on a node of
//! its own (a USD prim is one thing), no range on a directional light, no
//! normals on a line (a curve's normals orient a ribbon), every point and
//! end of a segment a vertex of its own (a USD curve shares none), and a
//! primitive that names no material bound to one shared copy of the default
//! material the writer binds it to (so the scene has the material the file
//! reads back with, and the bytes written are what they were).
//! [`losses`] names those that lose something: the clips after the first under
//! `UsdClips::First`, or under `All` the other clips' rigid motion (and any clip
//! past `usd_write::MAX_CLIPS`); not an unlit material, which USD keeps. It
//! also names a camera's unset aspect ratio, infinite far plane or mirrored orthographic view,
//! which other readers see as the 16:9, the 1000 m and the unmirrored view
//! written for them, and a clearcoat (a factor above 0) over an ior other
//! than 1.5, which is written as it is: UsdPreviewSurface's ior sets the
//! clearcoat's reflectance too, where glTF's clearcoat always reflects with
//! an ior of 1.5.
//!
//! Two values read back unset, and so are prepared unset, silently: an
//! unconnected `clearcoat` of 0 and an `ior` of exactly 1.5, which draw as
//! their absence does. three.js's USDZExporter writes both on every physical
//! material, and the reader takes them as the OBJ, FBX and Collada readers
//! take their exporters' defaults (Ruling A41). So a glTF `{}` clearcoat or
//! `{}` ior does not survive USD.
//!
//! Ruling IDs ("Ruling A41") are mesh-gaps item 4's rulings, listed with
//! their costs in the status block of
//! `docs/superpowers/plans/2026-11-04-mesh-material-layers.md`. "Ruling R6"
//! and its kin are item 5's (lights and cameras), listed with their costs
//! in the "Rulings" section of
//! `docs/superpowers/plans/2026-11-04-mesh-lights-cameras.md`.
//!
//! # How it is verified
//!
//! Unit tests here round-trip every fixture through all three formats (and
//! hold a USDC to the scene its USDA twin reads as), and read
//! files other tools made from our own content (`testdata/usd/README.md`):
//! Pixar's crate writer (every value encoding, and composition arcs), Apple's
//! packager and glTF importer, and three.js's exporter. They fuzz both parsers
//! and the container. The render harness (`scripts/mesh-verify`) renders every
//! USD output with three.js's loader and with `usdrecord` — Pixar's own
//! renderer, the authority where three.js's loader is wrong (its USD bugs are
//! listed there with the source lines) — and runs `usdchecker --arkit` on each.

use super::package::{Package, PackageFile, Sidecars};
use super::scene::{AlphaMode, AnimPath, Capabilities, Interpolation, LayerSupport, Loss, LossKind, MaterialSupport, Scene, Topology, Transform};
use super::usd_stage::{Layer, Stage};
use super::zip;
use super::{math, pose, LightUnits, MeshError, MeshFormat, SceneRead, UsdClips, UsdzRoot, WriteOptions};

/// Name of the root layer inside a USDZ and of a USDA package's primary file.
const ROOT_USDA: &str = "model.usda";
/// Name of the root layer inside a USDZ written with `usdz_root: usdc`, and of
/// a USDC package's primary file.
const ROOT_USDC: &str = "model.usdc";
/// The STEP ramp: a key this long before each step holds the previous value.
const STEP_EPSILON: f32 = 1e-3;

/// The uv sets a USDA holds, `primvars:st`, `st1` … `st7`: as many as the
/// reader keeps, and the most the writer writes.
pub const UV_SETS: usize = 8;
/// The uv sets a USDZ holds: RealityKit's two (see the module docs).
pub const USDZ_UV_SETS: usize = 2;

/// The capabilities row for USDZ, USDA and USDC: a USDZ holds [`USDZ_UV_SETS`]
/// uv sets and a USDA or USDC [`UV_SETS`].
pub fn capabilities(format: MeshFormat, opts: WriteOptions) -> Capabilities {
    Capabilities {
        hierarchy: true,
        normals: true,
        uv_sets: if format == MeshFormat::Usdz { USDZ_UV_SETS } else { UV_SETS },
        vertex_colors: true,
        materials: MaterialSupport::Pbr,
        // USDZ embeds its textures; a USDA can only reference them, so one
        // self-contained file has none.
        textures: format == MeshFormat::Usdz || !opts.self_contained,
        texture_filters: false,
        texture_wrap: true,
        texture_transforms: false,
        skinning: true,
        morph_targets: true,
        animation: true,
        multiple_meshes: true,
        // UsdPreviewSurface's `clearcoat`, `clearcoatRoughness` and `ior`; it
        // has no input for the other layers.
        layers: LayerSupport { clearcoat: true, ior: true, ..LayerSupport::none() },
        // UsdLux's DistantLight and SphereLight, and UsdGeomCamera
        // (usd_write.rs, "Lights and cameras").
        lights: true,
        cameras: true,
        // `Points` prims and linear `BasisCurves` prims (usd_write.rs, "Points
        // and lines"): OpenUSD@v26.11.1 pxr/usd/usdGeom/schema.usda L1877
        // (Points) and L1594-1812 (BasisCurves).
        points: true,
        lines: true,
    }
}

// ─── reading ────────────────────────────────────────────────────────────────

/// Parse one layer, text or crate, by its magic.
fn parse_layer(bytes: &[u8], id: &str) -> Result<Layer, MeshError> {
    if super::usdc::looks_like_crate(bytes) {
        super::usdc::parse(bytes, id)
    } else if super::usda::looks_like_usda(bytes) {
        super::usda::parse(bytes, id)
    } else {
        Err(MeshError::Malformed(format!("usd: layer {:?} is neither USDA text nor a USDC crate", super::usd_stage::short(id))))
    }
}

fn read_stage(root: Layer, fetch: &mut dyn FnMut(&str) -> Option<Vec<u8>>, light_units: LightUnits) -> Result<SceneRead, MeshError> {
    let stage = Stage::compose(root, fetch, parse_layer)?;
    let (scene, warnings) = super::usd_read::read_stage(&stage, fetch, light_units)?;
    Ok(SceneRead { scene, warnings })
}

/// Read a USDZ package: its first entry is the root layer, and everything it
/// references (sublayers, references, textures) resolves inside the package.
/// Lights are read in the scene's own units ([`LightUnits::Photometric`]).
pub fn read_usdz(bytes: &[u8], sidecars: &dyn Sidecars) -> Result<SceneRead, MeshError> {
    read_usdz_with(bytes, sidecars, LightUnits::default())
}

/// [`read_usdz`], reading the strength of a light that has no litegen
/// `customData` in `light_units` (`usd_read.rs`, "Lights and cameras").
pub fn read_usdz_with(bytes: &[u8], _sidecars: &dyn Sidecars, light_units: LightUnits) -> Result<SceneRead, MeshError> {
    let not_usdz = |detail: String| MeshError::NotThisFormat { expected: MeshFormat::Usdz, detail };
    if !zip::looks_like_zip(bytes) {
        return Err(not_usdz("a USDZ package is a ZIP archive and these bytes are not one".into()));
    }
    let archive = zip::Archive::parse(bytes)?;
    let first = archive.entries.first().ok_or_else(|| not_usdz("the archive is empty".into()))?;
    let ext = first.name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    if !matches!(ext.as_str(), "usd" | "usda" | "usdc") {
        return Err(not_usdz(format!(
            "the first file in the archive is {:?}; a USDZ package starts with its root USD layer",
            super::usd_stage::short(&first.name)
        )));
    }
    let root_bytes = archive.read(first)?;
    let root = parse_layer(&root_bytes, &first.name)?;
    let compressed = archive.entries.iter().any(|e| !e.is_stored());
    let mut fetch = |path: &str| -> Option<Vec<u8>> { archive.find(path).and_then(|e| archive.read(e).ok()) };
    let mut r = read_stage(root, &mut fetch, light_units)?;
    if compressed {
        r.warnings.push("the package compresses its entries, which USD itself refuses to open; it was read anyway".into());
    }
    Ok(r)
}

/// Read a standalone USD layer — USDA text or a USDC crate — resolving the
/// files it references through `sidecars`. Lights are read in the scene's
/// own units ([`LightUnits::Photometric`]).
pub fn read_usda(bytes: &[u8], sidecars: &dyn Sidecars) -> Result<SceneRead, MeshError> {
    read_usda_with(bytes, sidecars, LightUnits::default())
}

/// [`read_usda`], reading the strength of a light that has no litegen
/// `customData` in `light_units` (`usd_read.rs`, "Lights and cameras").
pub fn read_usda_with(bytes: &[u8], sidecars: &dyn Sidecars, light_units: LightUnits) -> Result<SceneRead, MeshError> {
    read_layer_with(bytes, sidecars, light_units, MeshFormat::Usda)
}

/// Read a standalone USD layer as [`MeshFormat::Usdc`]: the same reader as
/// [`read_usda`], which takes either encoding, so a text layer reads under
/// this name too. Only the refusal of what is no USD layer names usdc.
pub fn read_usdc(bytes: &[u8], sidecars: &dyn Sidecars) -> Result<SceneRead, MeshError> {
    read_usdc_with(bytes, sidecars, LightUnits::default())
}

/// [`read_usdc`], reading the strength of a light that has no litegen
/// `customData` in `light_units` (`usd_read.rs`, "Lights and cameras").
pub fn read_usdc_with(bytes: &[u8], sidecars: &dyn Sidecars, light_units: LightUnits) -> Result<SceneRead, MeshError> {
    read_layer_with(bytes, sidecars, light_units, MeshFormat::Usdc)
}

/// A standalone layer, USDA or USDC by its magic, read for the caller that
/// asked for `expected` (the format a refusal names).
fn read_layer_with(bytes: &[u8], sidecars: &dyn Sidecars, light_units: LightUnits, expected: MeshFormat) -> Result<SceneRead, MeshError> {
    if zip::looks_like_zip(bytes) {
        return Err(MeshError::NotThisFormat { expected, detail: "these bytes are a ZIP archive (a USDZ package?), not a USD layer".into() });
    }
    let id = if super::usdc::looks_like_crate(bytes) { ROOT_USDC } else { ROOT_USDA };
    if !super::usdc::looks_like_crate(bytes) && !super::usda::looks_like_usda(bytes) {
        return Err(MeshError::NotThisFormat { expected, detail: "a USD layer starts with `#usda 1.0` or `PXR-USDC`".into() });
    }
    let root = parse_layer(bytes, id)?;
    let mut fetch = |path: &str| sidecars.get(path);
    read_stage(root, &mut fetch, light_units)
}

// ─── writing ────────────────────────────────────────────────────────────────

/// Write a prepared scene as a USDZ package: one file.
/// `opts.key_rate()`: the rate clips were keyed at, declared as
/// `framesPerSecond`. `opts.usd_light_units`: the units each light's
/// `inputs:intensity` is written in. `opts.usdz_root` is the root layer's
/// encoding.
pub fn write_usdz(scene: &Scene, opts: WriteOptions) -> Result<Package, MeshError> {
    if scene.drawable_count() == 0 {
        return Err(MeshError::Empty);
    }
    let (text, textures) = super::usd_write::write_layer(
        scene,
        super::usd_write::TexturePlacement::Files,
        opts.key_rate(),
        opts.usd_light_units,
        opts.clips_for(MeshFormat::Usdz),
    )?;
    let (root_name, root) = match opts.usdz_root {
        UsdzRoot::Usda => (ROOT_USDA, text.into_bytes()),
        UsdzRoot::Usdc => (ROOT_USDC, crate_of(&text)?),
    };
    let mut w = zip::Writer::new();
    // Root layer first, every entry stored and 64-byte aligned: the USDZ rules
    // that let USD map the package in place.
    w.add(root_name, &root, zip::Store::Stored { align: 64 })?;
    for t in &textures {
        w.add(&t.path, &t.bytes, zip::Store::Stored { align: 64 })?;
    }
    Ok(Package::single("model.usdz", w.finish()))
}

/// Write a prepared scene as a USDA layer; as a package, with its textures
/// beside it. The lights' units are `opts.usd_light_units`.
pub fn write_usda(scene: &Scene, opts: WriteOptions) -> Result<Package, MeshError> {
    if scene.drawable_count() == 0 {
        return Err(MeshError::Empty);
    }
    let placement = if opts.self_contained { super::usd_write::TexturePlacement::None } else { super::usd_write::TexturePlacement::Files };
    let (text, textures) = super::usd_write::write_layer(scene, placement, opts.key_rate(), opts.usd_light_units, opts.clips_for(MeshFormat::Usda))?;
    let mut files = vec![PackageFile { path: ROOT_USDA.into(), bytes: text.into_bytes() }];
    files.extend(textures.into_iter().map(|t| PackageFile { path: t.path, bytes: t.bytes }));
    Ok(Package { files })
}

/// Write a prepared scene as a USDC crate; as a package, with its textures
/// beside it. The lights' units are `opts.usd_light_units`.
pub fn write_usdc(scene: &Scene, opts: WriteOptions) -> Result<Package, MeshError> {
    if scene.drawable_count() == 0 {
        return Err(MeshError::Empty);
    }
    let placement = if opts.self_contained { super::usd_write::TexturePlacement::None } else { super::usd_write::TexturePlacement::Files };
    let (text, textures) = super::usd_write::write_layer(scene, placement, opts.key_rate(), opts.usd_light_units, opts.clips_for(MeshFormat::Usdc))?;
    let mut files = vec![PackageFile { path: ROOT_USDC.into(), bytes: crate_of(&text)? }];
    files.extend(textures.into_iter().map(|t| PackageFile { path: t.path, bytes: t.bytes }));
    Ok(Package { files })
}

/// The USDA text the writer produced, as a USDC crate: the text is parsed back
/// into a layer and that layer encoded, so a crate holds exactly the layer the
/// text does and the two encodings read as one scene.
///
/// The parse runs with the reader's limits (values, nesting), which a scene
/// inside `write_scene`'s ceilings can exceed. Its refusal is then the crate
/// write's: it names usdc and the re-parse, not the `usda` line it came
/// from, which reads like a failure to read the user's file.
fn crate_of(text: &str) -> Result<Vec<u8>, MeshError> {
    let layer = super::usda::parse(text.as_bytes(), ROOT_USDA)
        .map_err(|e| MeshError::Unsupported(format!("usdc: the layer is past what a crate write re-parses: {e}")))?;
    super::usdc::write(&layer)
}

// ─── lossiness ──────────────────────────────────────────────────────────────

/// The scene reduced to exactly what the USD writer encodes. See the module
/// docs for each step and why.
/// `fps` is the rate resampled channels are keyed at (`WriteOptions::fps`).
pub fn prepare(scene: &Scene, caps: &Capabilities, fps: f32, clips: UsdClips) -> Result<Scene, MeshError> {
    // The materials as USD draws them first, so the uv sets `degrade` plans
    // go to maps USD keeps, never to a clearcoat's own normal map or to the
    // maps of one that reads back as none; only the materials are copied
    // for it.
    let mut s = pose::degrade_with_materials(scene, usd_materials(&scene.materials), caps)?;
    // One clip, or every clip when the file can switch between them: when
    // `clips` says so, and some clip moves a skeleton or a blend shape (a
    // scene of rigid motion alone has nothing to switch, and keeps the first
    // clip as ever). At most `MAX_CLIPS`, the reader's own bound.
    let kept = if clips == UsdClips::All && super::usd_write::has_skeletal_clip(&s) { super::usd_write::MAX_CLIPS } else { 1 };
    s.animations.truncate(kept);
    // Only PNG and JPEG travel in a USD package.
    for m in s.materials.iter_mut() {
        let images = &s.images;
        let textures = &s.textures;
        let portable = |r: &Option<super::scene::TextureRef>| {
            r.as_ref().is_some_and(|r| textures.get(r.texture).and_then(|t| images.get(t.image)).is_some_and(|i| i.format.is_universal()))
        };
        for (_, slot) in m.texture_slots_mut() {
            if slot.is_some() && !portable(slot) {
                *slot = None;
            }
        }
        // A clearcoat of 0 whose map went with the textures is now unmapped.
        drop_unset_on_reading(m);
    }
    pose::prune_unused_textures(&mut s);
    let materials = s.materials.clone();
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
        p.tangents = None;
        if p.topology == Topology::Lines {
            // A curve's normals orient a ribbon; they are not shading normals,
            // and the reader does not read them (usd_write.rs, "Points and
            // lines"). A point keeps its own.
            p.normals = None;
            for t in p.targets.iter_mut() {
                t.normals = None;
            }
        }
        let mat = p.material.and_then(|i| materials.get(i));
        if mat.is_some_and(|m| m.base_color_texture.is_some()) {
            p.colors = None;
        }
    }
    // A primitive that names no material is written bound to the default one
    // (`usd_write::default_material`), so a file never holds an unbound gprim.
    // It is bound here, so the prepared scene has the material the file reads
    // back with. One shared copy serves every such primitive, coloured or not
    // (Ruling I10-R15): the writer keys a material by its index and by whether
    // the primitive has vertex colours, so the variants are named
    // (`DefaultMaterial_vertexColor`, `DefaultMaterial`) exactly as they were
    // when the primitives named none, and material-less triangles write the
    // same bytes as before, a mixed scene included. As with a scene material
    // used both ways, such a scene is prepared with one material and read back
    // with one for each prim the file holds.
    let mut default_at: Option<usize> = None;
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()).filter(|p| p.material.is_none()) {
        let at = *default_at.get_or_insert_with(|| {
            s.materials.push(super::usd_write::default_material());
            s.materials.len() - 1
        });
        p.material = Some(at);
    }
    // Alpha, as the writer spells it and a reader reads it back.
    let mut varies = vec![false; s.materials.len()];
    for p in s.meshes.iter().flat_map(|m| &m.primitives) {
        if let (Some(i), Some(cs)) = (p.material, &p.colors) {
            if cs.iter().any(|c| c[3] < 1.0) {
                varies[i] = true;
            }
        }
    }
    for (i, m) in s.materials.iter_mut().enumerate() {
        let constant_alpha = m.base_color_texture.is_none() && !varies[i];
        let opaque = match m.alpha_mode {
            AlphaMode::Opaque => true,
            AlphaMode::Mask => m.alpha_cutoff <= 0.0 || (constant_alpha && m.base_color[3] >= 1.0),
            AlphaMode::Blend => constant_alpha && m.base_color[3] >= 1.0,
        };
        if opaque {
            m.alpha_mode = AlphaMode::Opaque;
            m.base_color[3] = 1.0;
        }
    }
    let materials = s.materials.clone();
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
        let opaque = p.material.and_then(|i| materials.get(i)).is_none_or(|m| m.alpha_mode == AlphaMode::Opaque);
        if let (true, Some(cs)) = (opaque, p.colors.as_mut()) {
            for c in cs.iter_mut() {
                c[3] = 1.0;
            }
        }
    }
    drop_unused_skins(&mut s);
    // Each light and camera on a prim of its own, as the writer puts it: one
    // that shares its node with a mesh, a skeleton joint (no prim in USD) or
    // the other gets an untransformed child node of its own, named for its
    // node (`fbx::separate_lights_and_cameras`). A directional light keeps no
    // range, as glTF's writer keeps none: neither format applies one to it.
    super::fbx::separate_lights_and_cameras(&mut s);
    for l in s.lights.iter_mut().filter(|l| l.kind == super::scene::LightKind::Directional) {
        l.range = None;
    }
    pose::drop_constant_channels(&mut s);
    linearize_animation(&mut s, fps)?;
    normalize_skeletons(&mut s, fps)?;
    // A zero scale as a millionth, keys included, as Collada and FBX write
    // it: it draws the same and every transform stays invertible.
    pose::nonsingular_scales(&mut s, true);
    quantize_joint_scales(&mut s);
    keep_what_clips_switch(&mut s);
    // A segment's two ends and each point are vertices of their own, as the
    // reader gives them back (Ruling I10-R8).
    super::usd_write::deindex_scene(&mut s)?;
    s.validate()?;
    Ok(s)
}

/// The normal a clearcoat has in USD. UsdPreviewSurface's spec
/// (https://openusd.org/release/spec_usdpreviewsurface.html): "Clearcoat
/// results are calculated using the same normal map data used by the primary
/// specular lobe." So it is the base normal map at the base's scale, or no
/// map at the default scale 1: what the reader gives a clearcoat, and what
/// [`prepare`] makes it.
pub(super) fn usd_coat_normal(m: &super::scene::Material) -> (Option<&super::scene::TextureRef>, f32) {
    match &m.normal_texture {
        Some(r) => (Some(r), m.normal_scale),
        None => (None, 1.0),
    }
}

/// `materials` as UsdPreviewSurface draws them: each clearcoat's normal is
/// [`usd_coat_normal`], and what reads back unset is left out
/// ([`drop_unset_on_reading`]). [`losses`] names the clearcoats whose normal
/// changes.
fn usd_materials(materials: &[super::scene::Material]) -> Vec<super::scene::Material> {
    materials
        .iter()
        .map(|m| {
            // An unlit material is its base colour and opacity alone, as the
            // writer writes it (`usd_write::unlit_surface`): the maps only the
            // lit layers sampled go with them.
            let mut m = if m.unlit { super::usd_write::unlit_surface(m) } else { m.clone() };
            let (normal, scale) = usd_coat_normal(&m);
            let normal = normal.cloned();
            if let Some(c) = m.clearcoat.as_mut() {
                (c.normal_texture, c.normal_scale) = (normal, scale);
            }
            drop_unset_on_reading(&mut m);
            m
        })
        .collect()
}

/// An unmapped clearcoat of 0, written as an unconnected `clearcoat = 0`,
/// which the reader takes as no coat (Ruling A41).
fn reads_as_no_coat(c: &super::scene::Clearcoat) -> bool {
    c.factor == 0.0 && c.texture.is_none()
}

/// What USD's reader takes as unset, left out silently (Ruling A41): an
/// unmapped clearcoat of 0, which is written as an unconnected `clearcoat =
/// 0`, with its roughness and maps, and an ior of exactly 1.5. Each draws as
/// its absence does, as OBJ's off-value layers and Collada's 1.5 do.
fn drop_unset_on_reading(m: &mut super::scene::Material) {
    if m.clearcoat.as_ref().is_some_and(reads_as_no_coat) {
        m.clearcoat = None;
    }
    if m.ior == Some(super::scene::DEFAULT_IOR) {
        m.ior = None;
    }
}

/// Skins no mesh node binds: USD has nothing to write them as (a skeleton
/// exists for the meshes it deforms), and their joints stay plain nodes.
fn drop_unused_skins(s: &mut Scene) {
    let used = super::usd_write::used_skins(s);
    if used.iter().all(|u| *u) {
        return;
    }
    let mut map = vec![None; s.skins.len()];
    let mut kept = Vec::new();
    for (i, sk) in s.skins.drain(..).enumerate() {
        if used[i] {
            map[i] = Some(kept.len());
            kept.push(sk);
        }
    }
    s.skins = kept;
    for n in s.nodes.iter_mut() {
        n.skin = n.skin.and_then(|i| map[i]);
    }
}

/// STEP and CUBICSPLINE channels as the linear keys USD interpolates. Only
/// the cubic ones are resampled onto the grid, and they are budgeted
/// (`pose::MAX_RESAMPLED_VALUES`) before any is: LINEAR keys are written as
/// authored, however long the clip.
fn linearize_animation(s: &mut Scene, fps: f32) -> Result<(), MeshError> {
    if s.animations.is_empty() {
        return Ok(());
    }
    // Each clip's own grid; the cubic channels of every clip are budgeted
    // together before any is resampled.
    let grids: Vec<Vec<f32>> = (0..s.animations.len()).map(|ai| pose::resample_times(s, ai, fps)).collect();
    let targets: Vec<usize> = s.nodes.iter().map(|n| n.mesh.map_or(0, |m| s.meshes[m].target_count())).collect();
    let mut resampled = 0usize;
    for (anim, grid) in s.animations.iter().zip(&grids) {
        let cubic_width: usize = anim
            .channels
            .iter()
            .filter(|c| c.interpolation == Interpolation::CubicSpline)
            .map(|c| c.stride(targets[c.node]))
            .sum();
        resampled = resampled.saturating_add(grid.len().saturating_mul(cubic_width));
    }
    if resampled > pose::MAX_RESAMPLED_VALUES {
        return Err(MeshError::Unsupported(format!(
            "the animation's cubic channels would resample to more than {} keyed values",
            pose::MAX_RESAMPLED_VALUES
        )));
    }
    let joints: std::collections::HashSet<usize> = s.skins.iter().flat_map(|k| k.joints.iter().copied()).collect();
    for (ai, ch) in s.animations.iter_mut().enumerate().flat_map(|(ai, a)| a.channels.iter_mut().map(move |ch| (ai, ch))) {
        let grid = &grids[ai];
        let stride = ch.stride(targets[ch.node]);
        match ch.interpolation {
            Interpolation::Linear => {}
            Interpolation::Step => {
                let n = ch.times.len();
                let mut times = Vec::with_capacity(n * 2);
                let mut values = Vec::with_capacity(ch.values.len() * 2);
                for i in 0..n {
                    times.push(ch.times[i]);
                    values.extend_from_slice(&ch.values[i * stride..(i + 1) * stride]);
                    if i + 1 < n {
                        let (a, b) = (ch.times[i], ch.times[i + 1]);
                        let eps = STEP_EPSILON.min((b - a) / 4.0).max(f32::EPSILON * b.abs() * 8.0);
                        let hold = b - eps;
                        if hold > a && hold < b {
                            times.push(hold);
                            values.extend_from_slice(&ch.values[i * stride..(i + 1) * stride]);
                        }
                    }
                }
                ch.times = times;
                ch.values = values;
                ch.interpolation = Interpolation::Linear;
            }
            Interpolation::CubicSpline => {
                let (lo, hi) = (ch.times[0], ch.times[ch.times.len() - 1]);
                let mut times: Vec<f32> = grid.iter().copied().filter(|t| *t >= lo && *t <= hi).collect();
                if times.first() != Some(&lo) {
                    times.insert(0, lo);
                }
                if times.last() != Some(&hi) {
                    times.push(hi);
                }
                times.dedup();
                let values = times.iter().flat_map(|&t| pose::sample_channel(ch, t, stride)).collect();
                ch.times = times;
                ch.values = values;
                ch.interpolation = Interpolation::Linear;
            }
        }
        // What the SkelAnimation carries (joints, blend-shape weights) is
        // keyed from 0, holding the first value. USD holds a first sample
        // anyway, but SceneKit starts a skeleton's clip at its first sample:
        // CesiumMan, keyed from 1/24 s, drew 1/24 s ahead at every frame. An
        // xformOp must NOT get that key: SceneKit then stretches the op's
        // animation (Khronos BoxAnimated, whose rotation starts at 1.25 s,
        // held still until 1.6 s), and without it plays it exactly.
        let skel = joints.contains(&ch.node) || ch.path == AnimPath::Weights;
        if skel && ch.times.first().is_some_and(|t| *t > 0.0) && ch.values.len() >= stride {
            let first = ch.values[..stride].to_vec();
            ch.times.insert(0, 0.0);
            ch.values.splice(0..0, first);
        }
    }
    Ok(())
}

/// Parent every joint of a skeleton directly to its nearest joint ancestor
/// (or to the skeleton's anchor), folding the non-joint nodes between them.
///
/// A USD skeleton's joints are only joints: there is nowhere to put a group
/// node between a hip and a knee. Folding a static group is exact; folding an
/// animated one samples the relative motion at the resampling rate.
fn normalize_skeletons(s: &mut Scene, fps: f32) -> Result<(), MeshError> {
    if s.skins.is_empty() {
        return Ok(());
    }
    let sets = super::usd_write::skeleton_sets(s);
    let parents = s.parents();
    let mut moves: Vec<(usize, Option<usize>)> = Vec::new();
    for (_, anchor, members) in &sets {
        for &j in members {
            let mut cur = parents[j];
            let mut target = *anchor;
            let mut steps = 0;
            while let Some(p) = cur {
                if Some(p) == *anchor || members.contains(&p) {
                    target = Some(p);
                    break;
                }
                cur = parents[p];
                steps += 1;
                if steps > s.nodes.len() {
                    return Err(MeshError::Malformed("usd: the node graph is not a tree".into()));
                }
            }
            if parents[j] != target && !moves.iter().any(|(n, _)| *n == j) {
                moves.push((j, target));
            }
        }
    }
    if moves.is_empty() {
        return Ok(());
    }
    let rest_world = s.world_matrices();
    // A moved joint whose transform relative to its new parent is animated —
    // through its own channels or a folded node's — is resampled at the
    // resampling rate, before the graph changes, in each clip. Its own
    // channels were relative to its old parent and go either way.
    let mut resampled: Vec<Vec<Resampled>> = Vec::new();
    let mut budget = 0usize;
    for ai in 0..s.animations.len() {
        resampled.push(resample_moved(s, ai, fps, &moves, &parents, &mut budget)?);
    }
    for &(j, target) in &moves {
        // Detach, then attach under the target with the folded rest transform.
        for n in s.nodes.iter_mut() {
            n.children.retain(|c| *c != j);
        }
        s.roots.retain(|r| *r != j);
        match target {
            Some(t) => s.nodes[t].children.push(j),
            None => s.roots.push(j),
        }
        let base = target.map_or(math::IDENTITY, |t| rest_world[t]);
        let rel = math::invert(&base)
            .map(|inv| math::mat_mul(&inv, &rest_world[j]))
            .ok_or_else(|| MeshError::Malformed("usd: a skeleton ancestor has a singular transform".into()))?;
        s.nodes[j].transform = Transform::from_matrix(rel);
        for a in s.animations.iter_mut() {
            a.channels.retain(|c| c.node != j || c.path == AnimPath::Weights);
        }
    }
    for (a, moved) in s.animations.iter_mut().zip(resampled) {
        for r in moved {
            for (path, values) in [(AnimPath::Translation, r.translations), (AnimPath::Rotation, r.rotations), (AnimPath::Scale, r.scales)] {
                a.channels.push(super::scene::Channel { node: r.node, path, interpolation: Interpolation::Linear, times: r.times.clone(), values });
            }
        }
    }
    Ok(())
}

/// Clip `ai`'s moved joints, each resampled where its motion relative to its
/// new parent is animated (see [`normalize_skeletons`]). `budget` counts the
/// values the clips resampled so far, which share one budget.
fn resample_moved(
    s: &Scene,
    ai: usize,
    fps: f32,
    moves: &[(usize, Option<usize>)],
    parents: &[Option<usize>],
    budget: &mut usize,
) -> Result<Vec<Resampled>, MeshError> {
    // The nodes this clip animates (morph weights aside).
    let animated: Vec<usize> = s.animations[ai].channels.iter().filter(|c| c.path != AnimPath::Weights).map(|c| c.node).collect();
    let mut resampled: Vec<Resampled> = Vec::new();
    let anim_times = if animated.is_empty() { Vec::new() } else { pose::resample_times(s, ai, fps) };
    // Every node's world matrix at every grid time: budgeted as Collada's
    // matrices are, before any is computed.
    *budget = budget.saturating_add(anim_times.len().saturating_mul(s.nodes.len()).saturating_mul(16));
    if *budget > pose::MAX_RESAMPLED_VALUES {
        return Err(MeshError::Unsupported(format!(
            "re-parenting the skeleton's joints would resample more than {} keyed values",
            pose::MAX_RESAMPLED_VALUES
        )));
    }
    let worlds: Vec<Vec<math::Mat4>> = anim_times
        .iter()
        .map(|t| super::scene::world_from_locals(s, &pose::sample_pose(s, ai, *t).locals()))
        .collect();
    for &(j, target) in moves {
        let mut chain = vec![j];
        let mut cur = parents[j];
        while cur != target {
            let Some(p) = cur else { break };
            chain.push(p);
            cur = parents[p];
        }
        if !chain.iter().any(|n| animated.contains(n)) {
            continue;
        }
        let mut r = Resampled { node: j, times: anim_times.clone(), translations: Vec::new(), rotations: Vec::new(), scales: Vec::new() };
        let mut prev: Option<[f64; 4]> = None;
        for w in &worlds {
            let base = target.map_or(math::IDENTITY, |t| w[t]);
            let rel = math::invert(&base)
                .map(|inv| math::mat_mul(&inv, &w[j]))
                .ok_or_else(|| MeshError::Malformed("usd: a skeleton ancestor collapses to a singular transform".into()))?;
            // A key scaled to nothing (pop-in) decomposes; shear has no TRS,
            // and joints carry TRS only.
            let (t, mut q, sv) = math::decompose_near(&rel, prev).ok_or_else(|| {
                MeshError::Unsupported("usd: a folded skeleton group shears a joint, which joint TRS cannot hold".into())
            })?;
            if prev.is_some_and(|pq| math::quat_dot(pq, q) < 0.0) {
                q = [-q[0], -q[1], -q[2], -q[3]];
            }
            prev = Some(q);
            r.translations.extend(t.iter().map(|v| *v as f32));
            r.rotations.extend(q.iter().map(|v| *v as f32));
            r.scales.extend(sv.iter().map(|v| *v as f32));
        }
        resampled.push(r);
    }
    Ok(resampled)
}

/// A folded joint's motion relative to its new parent, flattened per key.
struct Resampled {
    node: usize,
    times: Vec<f32>,
    translations: Vec<f32>,
    rotations: Vec<f32>,
    scales: Vec<f32>,
}

/// The nearest half-precision value, as UsdSkel stores animated joint scales.
pub fn to_half_precision(x: f32) -> f32 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let a = x.abs().min(65504.0);
    // Halves have 11 significant bits; below 2^-14 they are subnormal with a
    // fixed step of 2^-24.
    let e = a.log2().floor().max(-14.0);
    let step = 2f32.powi(e as i32 - 10);
    let q = (a / step).round_ties_even() * step;
    q.copysign(x)
}

/// The joints whose scales USD holds as halves: every member of a skeleton
/// any clip animates keeps every clip's scales, and its rest scales, as
/// halves, since each clip's SkelAnimation lists the rest scales beside its
/// samples. Which nodes a clip moves is one pass over the channels, not a
/// scan of a skeleton's members for each channel.
fn half_scaled_joints(s: &Scene) -> Vec<bool> {
    half_scaled_joints_in(s, &s.animations)
}

/// [`half_scaled_joints`] as `animations`, some of `s`'s clips, animate it.
fn half_scaled_joints_in(s: &Scene, animations: &[super::scene::Animation]) -> Vec<bool> {
    let mut half = vec![false; s.nodes.len()];
    if animations.is_empty() {
        return half;
    }
    let mut moved = vec![false; s.nodes.len()];
    for c in animations.iter().flat_map(|a| &a.channels) {
        if c.path != AnimPath::Weights {
            if let Some(m) = moved.get_mut(c.node) {
                *m = true;
            }
        }
    }
    for (_, _, members) in &super::usd_write::skeleton_sets(s) {
        if members.iter().any(|&j| moved[j]) {
            for &j in members {
                half[j] = true;
            }
        }
    }
    half
}

/// The largest finite half, which [`to_half_precision`] writes for any
/// larger magnitude.
const HALF_MAX: f64 = 65_504.0;

/// Joint scales in animated skeletons as half floats: the SkelAnimation's
/// `half3[] scales` is what every USD reader poses them with.
fn quantize_joint_scales(s: &mut Scene) {
    let quantize = half_scaled_joints(s);
    for (j, q) in quantize.iter().enumerate() {
        if !q {
            continue;
        }
        if let Some((t, r, sc)) = s.nodes[j].transform.trs() {
            // Clamped in f64 first: a scale past f32's range (a matrix4d's
            // or an FBX double) would cast to an infinity.
            let h = sc.map(|v| to_half_precision(v.clamp(-HALF_MAX, HALF_MAX) as f32) as f64);
            if h != sc {
                s.nodes[j].transform = Transform::Trs { translation: t, rotation: r, scale: h };
            }
        }
    }
    for a in s.animations.iter_mut() {
        for c in a.channels.iter_mut() {
            if c.path == AnimPath::Scale && quantize[c.node] {
                for v in c.values.iter_mut() {
                    *v = to_half_precision(*v);
                }
            }
        }
    }
}

/// Of the clips a file of several holds, the first keeps all its channels
/// (its rigid motion stays outside the variants, as a one-clip file has it)
/// and the others keep what a variant can switch, the skeletons' and blend
/// shapes' ([`super::usd_write::is_skeletal`]); a clip left with nothing is
/// no alternative and goes ([`losses`] names what each lost). A scene of one
/// clip is as it was.
fn keep_what_clips_switch(s: &mut Scene) {
    if s.animations.len() < 2 {
        return;
    }
    let joints = super::usd_write::skeleton_joints(s);
    let mut kept = Vec::with_capacity(s.animations.len());
    for (i, mut a) in std::mem::take(&mut s.animations).into_iter().enumerate() {
        if i > 0 {
            a.channels.retain(|c| super::usd_write::is_skeletal(s, &joints, c));
            if a.channels.is_empty() {
                continue;
            }
        }
        kept.push(a);
    }
    s.animations = kept;
}

/// What USD loses beyond the generic capability table, named for the caller.
/// `textures`: whether this USD carries textures (a single-file USDA does
/// not, so the vertex colours under them survive).
pub fn losses(scene: &Scene, target: &str, textures: bool, clips: UsdClips) -> Vec<Loss> {
    let mut out = Vec::new();
    let mut push = |kind: LossKind, what: String, why: &str| out.push(Loss::new(kind, what, format!("{target} {why}")));
    if scene.animations.len() > 1 {
        let named = |(i, a): (usize, &super::scene::Animation)| a.name.clone().unwrap_or_else(|| format!("#{i}"));
        if clips == UsdClips::All && super::usd_write::has_skeletal_clip(scene) {
            // Every clip is kept, up to the bound, but a clip's rigid motion
            // is the first's alone: a variant switches a skeleton's
            // animation, and a transform op has one value.
            let past: Vec<String> = scene.animations.iter().enumerate().skip(super::usd_write::MAX_CLIPS).map(named).collect();
            if !past.is_empty() {
                push(
                    LossKind::Animation,
                    format!("{} animation clip(s) [{}]", past.len(), past.join(", ")),
                    &format!("keeps at most {} clips; the rest are left out", super::usd_write::MAX_CLIPS),
                );
            }
            let joints = super::usd_write::skeleton_joints(scene);
            let rigid: Vec<String> = scene
                .animations
                .iter()
                .enumerate()
                .take(super::usd_write::MAX_CLIPS)
                .skip(1)
                .filter(|(_, a)| a.channels.iter().any(|c| !super::usd_write::is_skeletal(scene, &joints, c)))
                .map(named)
                .collect();
            if !rigid.is_empty() {
                push(
                    LossKind::Animation,
                    format!("rigid motion of clips [{}]", rigid.join(", ")),
                    "switches skeleton bindings between clips, not transforms; only the first clip's rigid motion is kept",
                );
            }
        } else {
            let names: Vec<String> = scene.animations.iter().enumerate().skip(1).map(named).collect();
            push(LossKind::Animation, format!("{} animation clip(s) [{}]", names.len(), names.join(", ")), "has one timeline; only the first clip is kept");
        }
    }
    // An animated skeleton's joint scales are halves (`quantize_joint_scales`),
    // and a magnitude past the largest is written as it, in every encoding.
    // Judged over the clips the file keeps, as `prepare` truncates them.
    let kept = if clips == UsdClips::All && super::usd_write::has_skeletal_clip(scene) { super::usd_write::MAX_CLIPS } else { 1 };
    let kept = &scene.animations[..kept.min(scene.animations.len())];
    let half = half_scaled_joints_in(scene, kept);
    let past = |v: f64| v.is_finite() && v.abs() > HALF_MAX;
    let mut clamped = vec![false; scene.nodes.len()];
    for (j, n) in scene.nodes.iter().enumerate() {
        clamped[j] = half[j] && n.transform.trs().is_some_and(|(_, _, sc)| sc.iter().any(|v| past(*v)));
    }
    for c in kept.iter().flat_map(|a| &a.channels) {
        if c.path == AnimPath::Scale && half.get(c.node).copied().unwrap_or(false) && c.values.iter().any(|v| past(f64::from(*v))) {
            clamped[c.node] = true;
        }
    }
    let joints = clamped.iter().filter(|c| **c).count();
    if joints > 0 {
        push(
            LossKind::Skins,
            format!("joint scales past 65,504, on {joints} joint(s)"),
            "holds an animated skeleton's joint scales as halves, whose largest is 65,504; a larger one is written as 65,504",
        );
    }
    // A clip USD keeps is not reported for being resampled into its linear
    // interpolation (CUBICSPLINE at the key rate, a STEP as a 1 ms ramp): the
    // clip is delivered, and FBX and Collada resample alike without a report.
    // Unlit is not reported: it is kept, as a recipe and a marker
    // (`usd_write.rs`, "Unlit"). What an unlit material ignores is not named
    // either, below: its clearcoat, ior and maps are not written, and nothing
    // draws them (`usd_write::unlit_surface`).
    // A clearcoat whose own normal is not the base's (another map, a map over
    // an unmapped base, or none over a mapped one) is drawn with the base's
    // ([`usd_coat_normal`]): a map, so named only where textures survive.
    // Not "clearcoat on …", the loss of the layer itself, which USD keeps.
    // Not an unmapped clearcoat of 0, which draws nothing and goes silently
    // ([`drop_unset_on_reading`]).
    let coats = scene
        .materials
        .iter()
        .filter(|m| !m.unlit)
        .filter(|m| {
            let usd = usd_coat_normal(m);
            m.clearcoat.as_ref().is_some_and(|c| {
                !reads_as_no_coat(c) && (c.normal_texture.is_some() || usd.0.is_some()) && (c.normal_texture.as_ref(), c.normal_scale) != usd
            })
        })
        .count();
    if textures && coats > 0 {
        push(
            LossKind::Materials,
            format!("clearcoat normals of {coats} material(s)"),
            "has no separate clearcoat normal: UsdPreviewSurface's clearcoat uses the base normal map, so these clearcoats are drawn with the base layer's",
        );
    }
    // UsdPreviewSurface's ior is also its clearcoat's: "Index of Refraction to
    // be used for translucent objects and objects with specular components,
    // including the clearcoat if clearcoat > 0" (its spec), as Storm draws it
    // (previewSurface.glslfx:213, :241-246). glTF's clearcoat keeps 1.5: "The
    // index of refraction of clearcoat and base layer do not influence each
    // other" (KHR_materials_clearcoat). The ior is written for the base
    // layer, and a visible clearcoat over any other is named, not as "ior on
    // …" or "clearcoat on …", which USD keeps.
    let coupled = scene.materials.iter().filter(|m| !m.unlit && m.clearcoat.as_ref().is_some_and(|c| c.factor > 0.0) && m.ior.unwrap_or(1.5) != 1.5).count();
    if coupled > 0 {
        push(
            LossKind::Materials,
            format!("a clearcoat with an ior other than 1.5 on {coupled} material(s)"),
            "sets a clearcoat's reflectance from the material's ior, while glTF's clearcoat always uses 1.5, so these clearcoats reflect more or less strongly",
        );
    }
    // Tangents are not reported: the normals are kept, and readers derive
    // tangents from them and the uvs (glTF 2.0 §3.7.2.1, MikkTSpace), as no
    // container's report counts them either.
    let prims: Vec<&super::scene::Primitive> = scene.meshes.iter().flat_map(|m| &m.primitives).collect();
    let colored_textured = prims
        .iter()
        .filter(|p| textures && p.colors.is_some() && p.material.and_then(|m| scene.materials.get(m)).is_some_and(|m| m.base_color_texture.is_some()))
        .count();
    if colored_textured > 0 {
        push(
            LossKind::VertexColors,
            format!("vertex colours on {colored_textured} textured primitive(s)"),
            "cannot multiply a base-colour texture by a vertex colour (UsdPreviewSurface has no multiply); the vertex colours are dropped",
        );
    }
    // `prepare` leaves a line's normals out, and every morph target's with
    // them (a curve's normals orient a ribbon), where `scene::losses` says
    // nothing of normals as USD holds them. A point keeps its own.
    let curve_normals = prims
        .iter()
        .filter(|p| p.topology == Topology::Lines && (p.normals.is_some() || p.targets.iter().any(|t| t.normals.is_some())))
        .count();
    if curve_normals > 0 {
        push(
            LossKind::Normals,
            format!("normals of {curve_normals} line primitive(s)"),
            "writes a line as a `BasisCurves`, whose normals orient a ribbon, so they are left out",
        );
    }
    let clashing = scene.materials.iter().filter(|m| {
        // An unlit material samples its base colour alone.
        let refs: Vec<_> = if m.unlit { m.base_color_texture.iter().collect() } else { m.texture_refs().map(|(_, r)| r).collect() };
        refs.iter().any(|a| refs.iter().any(|b| a.uv_set == b.uv_set && a.transform != b.transform))
    }).count();
    if clashing > 0 {
        push(
            LossKind::TextureTransforms,
            format!("texture transforms of {clashing} material(s)"),
            "bakes texture transforms into texture coordinates, and these apply different transforms to one coordinate set; only the first is kept",
        );
    }
    // Group nodes between a skeleton's joints have no place in a USD
    // skeleton, which holds only joints.
    let parents = scene.parents();
    let folded = super::usd_write::skeleton_sets(scene).iter().any(|(_, anchor, members)| {
        members.iter().any(|&j| parents[j] != *anchor && parents[j].is_some_and(|p| !members.contains(&p)))
    });
    if folded {
        push(LossKind::Hierarchy, "group nodes between skeleton joints".into(), "skeletons hold only joints; those nodes' transforms are folded into the joints below them");
    }
    // glTF's `range` has no UsdLux form (Ruling R27): other readers light a
    // point or spot light past it, and only litegen's own customData keeps
    // it. A directional light's goes unnamed: `prepare` drops it, as glTF's
    // writer does, and no reader applies one.
    let ranged: Vec<String> = scene
        .lights
        .iter()
        .enumerate()
        .filter(|(_, l)| l.kind != super::scene::LightKind::Directional && l.range.is_some())
        .map(|(i, l)| l.name.clone().unwrap_or_else(|| format!("#{i}")))
        .collect();
    if !ranged.is_empty() {
        push(
            LossKind::Lights,
            format!("the range of {} light(s) [{}]", ranged.len(), ranged.join(", ")),
            "has no light range (UsdLux has none), so other readers light past it; only litegen's own customData keeps it",
        );
    }
    // What a camera holds that UsdGeomCamera cannot say to other readers
    // (`usd_write::write_camera`). litegen's own reader takes each back from
    // the camera's `litegen` customData, as Collada's takes its <extra>.
    use super::scene::Projection;
    let cameras = |pick: &dyn Fn(&Projection) -> bool| -> Vec<String> {
        let picked = scene.cameras.iter().enumerate().filter(|(_, c)| pick(&c.projection));
        picked.map(|(i, c)| c.name.clone().unwrap_or_else(|| format!("#{i}"))).collect()
    };
    let unset_aspect = cameras(&|p| matches!(p, Projection::Perspective { aspect: None, .. }));
    if !unset_aspect.is_empty() {
        push(
            LossKind::Cameras,
            format!("the viewport aspect ratio of {} camera(s) [{}]", unset_aspect.len(), unset_aspect.join(", ")),
            "always stores an aspect ratio (horizontalAperture over verticalAperture); 16:9 (1920 × 1080, Blender's default render size) \
             was written for other readers, and litegen's own customData keeps it unset",
        );
    }
    let infinite = cameras(&|p| matches!(p, Projection::Perspective { zfar: None, .. }));
    if !infinite.is_empty() {
        push(
            LossKind::Cameras,
            format!("the infinite far plane of {} camera(s) [{}]", infinite.len(), infinite.join(", ")),
            "always stores a far plane (clippingRange); 1000 m (Blender's default clip end), or 10,000 times a near plane at or past it, \
             was written for other readers, and litegen's own customData keeps it infinite",
        );
    }
    let mirrored = cameras(&|p| matches!(p, Projection::Orthographic { xmag, ymag, .. } if *xmag < 0.0 || *ymag < 0.0));
    if !mirrored.is_empty() {
        push(
            LossKind::Cameras,
            format!("the mirroring of {} orthographic camera(s) [{}]", mirrored.len(), mirrored.join(", ")),
            "stores an orthographic view's size (its apertures), not its sign; the view was written unmirrored for other readers, \
             and litegen's own customData keeps the sign",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::fixtures;
    use super::super::fixtures::lightcam;
    use super::super::fixtures::sweeps::{layer_sweep, lean_layered, span};
    use super::super::package::{MemorySidecars, NoSidecars, PackageFile};
    use super::super::scene::{
        Animation, Camera, Channel, Filter, Image, ImageFormat, Light, LightKind, Material, Node, Primitive, Projection, SceneMesh, Skin, UvTransform,
    };
    use super::super::usd_stage::Value;
    use super::super::{read_scene, read_scene_with, write_scene, ReadOptions, UsdzRoot};
    use super::*;

    const STRESS_USDA: &[u8] = include_bytes!("testdata/usd/stress.usda");
    const STRESS_USDC: &[u8] = include_bytes!("testdata/usd/stress.usdc");
    const COMP_USDA: &[u8] = include_bytes!("testdata/usd/comp.usda");
    const COMP_USDC: &[u8] = include_bytes!("testdata/usd/comp.usdc");
    const COMP_SUB: &[u8] = include_bytes!("testdata/usd/comp_sub.usda");
    const COMP_PAYLOAD: &[u8] = include_bytes!("testdata/usd/comp_payload.usda");
    const COMP_ANIM: &[u8] = include_bytes!("testdata/usd/comp_anim.usda");
    const CUBE_USDC: &[u8] = include_bytes!("testdata/usd/cube.usdc");
    const SKINNED_USDC: &[u8] = include_bytes!("testdata/usd/skinned.usdc");
    const MORPH_USDC: &[u8] = include_bytes!("testdata/usd/morph.usdc");
    const TEXTURED_ARKIT: &[u8] = include_bytes!("testdata/usd/textured_arkit.usdz");
    const TEXTURED_APPLE: &[u8] = include_bytes!("testdata/usd/textured_apple.usdz");
    const SKINNED_APPLE: &[u8] = include_bytes!("testdata/usd/skinned_apple.usdz");
    const TEXTURED_THREE: &[u8] = include_bytes!("testdata/usd/textured_three.usdz");
    const MULTIUV_USDC: &[u8] = include_bytes!("testdata/usd/multiuv.usdc");

    // ── evaluation-based comparison ─────────────────────────────────────────

    /// One drawn primitive in world space, with what it looks like.
    #[derive(Debug)]
    struct Drawn {
        positions: Vec<[f32; 3]>,
        normals: Option<Vec<[f32; 3]>>,
        uvs: Vec<Vec<[f32; 2]>>,
        colors: Option<Vec<[f32; 4]>>,
        triangles: Vec<[u32; 3]>,
        material: Option<Material>,
        base_image: Option<Vec<u8>>,
    }

    fn drawn(scene: &Scene, t: Option<f32>) -> Vec<Drawn> {
        let pose = match t {
            Some(t) if !scene.animations.is_empty() => pose::sample_pose(scene, 0, t),
            _ => pose::default_pose(scene),
        };
        pose::world_primitives(scene, &pose)
            .unwrap()
            .into_iter()
            .map(|wp| {
                let material = wp.material.and_then(|m| scene.materials.get(m)).cloned();
                let base_image = material
                    .as_ref()
                    .and_then(|m| m.base_color_texture.as_ref())
                    .and_then(|r| scene.textures.get(r.texture))
                    .and_then(|t| scene.images.get(t.image))
                    .map(|i| i.data.clone());
                Drawn {
                    positions: wp.positions,
                    normals: wp.normals,
                    uvs: wp.uvs,
                    colors: wp.colors,
                    triangles: wp.indices.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
                    material,
                    base_image,
                }
            })
            .collect()
    }

    fn close3(a: &[[f32; 3]], b: &[[f32; 3]], tol: f32) -> Option<String> {
        if a.len() != b.len() {
            return Some(format!("{} vs {} entries", a.len(), b.len()));
        }
        a.iter()
            .zip(b)
            .enumerate()
            .find(|(_, (x, y))| (0..3).any(|k| (x[k] - y[k]).abs() > tol * x[k].abs().max(1.0)))
            .map(|(i, (x, y))| format!("entry {i}: {x:?} vs {y:?}"))
    }

    #[derive(Clone, Copy)]
    struct Check {
        materials: bool,
        textures: bool,
        colors: bool,
        normals: bool,
        uvs: bool,
        /// Also compare coordinates no texture reads (which no renderer
        /// shows, and which a writer may encode any way it likes).
        unread_uvs: bool,
        tol: f32,
    }

    const FULL: Check =
        Check { materials: true, textures: true, colors: true, normals: true, uvs: true, unread_uvs: true, tol: 1e-5 };

    /// The two scenes draw the same thing: primitive by primitive in
    /// traversal order, at rest and at every sample time.
    fn assert_draws_alike(want: &Scene, got: &Scene, times: &[f32], check: Check, ctx: &str) {
        let mut moments: Vec<Option<f32>> = vec![None];
        moments.extend(times.iter().copied().map(Some));
        for t in moments {
            let (a, b) = (drawn(want, t), drawn(got, t));
            assert_eq!(a.len(), b.len(), "{ctx} @ {t:?}: drawn primitive count");
            for (i, (x, y)) in a.iter().zip(&b).enumerate() {
                let at = format!("{ctx} @ {t:?} primitive {i}");
                assert_eq!(x.triangles, y.triangles, "{at}: triangles");
                if let Some(e) = close3(&x.positions, &y.positions, check.tol) {
                    panic!("{at}: positions differ: {e}");
                }
                if check.normals {
                    match (&x.normals, &y.normals) {
                        (Some(p), Some(q)) => {
                            if let Some(e) = close3(p, q, 1e-4) {
                                panic!("{at}: normals differ: {e}");
                            }
                        }
                        (None, None) => {}
                        (p, q) => panic!("{at}: normals present {} vs {}", p.is_some(), q.is_some()),
                    }
                }
                let read = x.material.as_ref().is_some_and(|m| m.texture_refs().next().is_some());
                if check.uvs && (check.unread_uvs || read) {
                    assert_eq!(x.uvs.len(), y.uvs.len(), "{at}: uv set count");
                    for (u, v) in x.uvs.iter().zip(&y.uvs) {
                        let bad = u.iter().zip(v).position(|(p, q)| (p[0] - q[0]).abs() > 1e-6 || (p[1] - q[1]).abs() > 1e-6);
                        assert!(bad.is_none(), "{at}: uv {bad:?}: {:?} vs {:?}", bad.map(|i| u[i]), bad.map(|i| v[i]));
                    }
                }
                if check.colors {
                    match (&x.colors, &y.colors) {
                        (Some(p), Some(q)) => assert!(
                            p.iter().zip(q).all(|(c, d)| (0..4).all(|k| (c[k] - d[k]).abs() < 1e-5)),
                            "{at}: vertex colours differ"
                        ),
                        (None, None) => {}
                        (p, q) => panic!("{at}: vertex colours present {} vs {}", p.is_some(), q.is_some()),
                    }
                }
                if check.materials {
                    let (m, n) = (x.material.clone().unwrap_or_default(), y.material.clone().unwrap_or_default());
                    // A vertex-coloured primitive's base colour rides in its
                    // vertex colours in USD: compare the product.
                    let base = |m: &Material, d: &Drawn| -> [f32; 4] {
                        let c = d.colors.as_ref().and_then(|c| c.first()).copied().unwrap_or([1.0; 4]);
                        [m.base_color[0] * c[0], m.base_color[1] * c[1], m.base_color[2] * c[2], m.base_color[3] * c[3]]
                    };
                    let (bm, bn) = (base(&m, x), base(&n, y));
                    assert!((0..4).all(|k| (bm[k] - bn[k]).abs() < 1e-6), "{at}: base colour {bm:?} vs {bn:?}");
                    assert!((m.metallic - n.metallic).abs() < 1e-6 && (m.roughness - n.roughness).abs() < 1e-6, "{at}: metallic/roughness");
                    assert_eq!(m.alpha_mode, n.alpha_mode, "{at}: alpha mode");
                    assert_eq!(m.double_sided, n.double_sided, "{at}: double-sidedness");
                    assert!(m.emissive.iter().zip(&n.emissive).all(|(p, q)| (p - q).abs() < 1e-6), "{at}: emissive");
                }
                if check.textures {
                    assert_eq!(x.base_image, y.base_image, "{at}: base-colour image bytes");
                }
            }
        }
    }

    fn round(scene: &Scene, fmt: MeshFormat, self_contained: bool) -> (SceneRead, Vec<PackageFile>) {
        let w = write_scene(scene, fmt, WriteOptions { self_contained, ..Default::default() }).unwrap_or_else(|e| panic!("write {fmt}: {e}"));
        let files = w.package.files.clone();
        let side = MemorySidecars { files: &files };
        let r = read_scene(&files[0].bytes, fmt, &side).unwrap_or_else(|e| panic!("read {fmt}: {e}"));
        (r, files)
    }

    fn names(s: &Scene) -> Vec<Option<String>> {
        s.nodes.iter().map(|n| n.name.clone()).collect()
    }

    // ── our own files ───────────────────────────────────────────────────────

    /// `want`, an animated skeleton's scales as the file holds them. The
    /// writer samples every joint of a skeleton at the union of the key times
    /// of all that skeleton's joint channels, and prints each `half3[]` scale
    /// as a half (Ruling U13b); the reader poses the skin from those samples.
    /// So each scale channel of a skeleton joint becomes its own values at
    /// that union, rounded with [`to_half_precision`] as the writer rounds
    /// them. Nothing else changes: a translation or rotation channel is
    /// linear between its keys, so sampling it at more times loses nothing,
    /// and the scene's rest scales are rounded by `prepare` already.
    fn scales_as_the_writer_samples(want: &Scene) -> Scene {
        use super::super::scene::{AnimPath, Channel, Interpolation};
        let mut out = want.clone();
        // Each clip is sampled on its own: a skeleton's samples at the union
        // of the key times of that clip's joint channels.
        for (ai, anim) in want.animations.iter().enumerate() {
            for (_, _, joints) in super::super::usd_write::skeleton_sets(want) {
                let mut times: Vec<f32> = anim.channels.iter().filter(|c| joints.contains(&c.node) && c.path != AnimPath::Weights).flat_map(|c| c.times.iter().copied()).collect();
                times.sort_by(|a, b| a.partial_cmp(b).unwrap());
                times.dedup();
                for (i, c) in anim.channels.iter().enumerate().filter(|(_, c)| c.path == AnimPath::Scale && joints.contains(&c.node)) {
                    let values: Vec<f32> = times.iter().flat_map(|t| pose::sample_channel(c, *t, 3).into_iter().map(to_half_precision)).collect();
                    out.animations[ai].channels[i] = Channel { node: c.node, path: AnimPath::Scale, interpolation: Interpolation::Linear, times: times.clone(), values };
                }
            }
        }
        out
    }

    #[test]
    fn every_fixture_round_trips_through_usdz_and_usda() {
        for (name, fixture) in fixtures::all() {
            for (fmt, single) in [(MeshFormat::Usdz, true), (MeshFormat::Usda, false), (MeshFormat::Usda, true), (MeshFormat::Usdc, false), (MeshFormat::Usdc, true)] {
                let ctx = format!("{name} {fmt}{}", if single { " single" } else { " package" });
                let opts = WriteOptions { self_contained: single, ..Default::default() };
                let want = fmt.prepare(&fixture, opts).unwrap();
                let (r, _) = round(&fixture, fmt, single);
                let got = &r.scene;
                // Our own files come back node for node, name for name.
                assert_eq!(names(got), names(&want), "{ctx}: nodes");
                assert_eq!(got.roots, want.roots, "{ctx}: roots");
                assert_eq!(got.skins.len(), want.skins.len(), "{ctx}: skins");
                for (a, b) in got.skins.iter().zip(&want.skins) {
                    assert_eq!(a.joints, b.joints, "{ctx}: skin joints");
                    for (p, q) in a.inverse_bind_matrices.iter().zip(&b.inverse_bind_matrices) {
                        assert!(p.iter().zip(q).all(|(x, y)| (x - y).abs() < 1e-9), "{ctx}: inverse bind matrices");
                    }
                }
                for (gn, wn) in got.nodes.iter().zip(&want.nodes) {
                    if let (Some(gm), Some(wm)) = (gn.mesh, wn.mesh) {
                        assert_eq!(got.meshes[gm].target_names, want.meshes[wm].target_names, "{ctx}: morph target names");
                        assert_eq!(got.meshes[gm].weights, want.meshes[wm].weights, "{ctx}: default morph weights");
                    }
                }
                assert_eq!(got.animations.len(), want.animations.len(), "{ctx}: animations");
                if let (Some(a), Some(b)) = (got.animations.first(), want.animations.first()) {
                    assert_eq!(a.name, b.name, "{ctx}: animation name");
                    assert_eq!(a.time_range(), b.time_range(), "{ctx}: clip range");
                }
                let mut times = fixtures::sample_times(&fixture);
                // And between every pair of keys, where interpolation shows.
                if let Some(a) = want.animations.first() {
                    let mut keys: Vec<f32> = a.channels.iter().flat_map(|c| c.times.iter().copied()).collect();
                    keys.sort_by(|x, y| x.partial_cmp(y).unwrap());
                    keys.dedup();
                    times.extend(keys.windows(2).map(|w| (w[0] + w[1]) / 2.0));
                }
                let check = Check { textures: !(matches!(fmt, MeshFormat::Usda | MeshFormat::Usdc) && single), tol: 2e-5, ..FULL };
                // The scene as the file holds its skeleton's scales (Ruling U17).
                assert_draws_alike(&scales_as_the_writer_samples(&want), got, &times, check, &ctx);
            }
        }
    }

    #[test]
    fn writing_is_deterministic() {
        for (name, fixture) in fixtures::all() {
            for fmt in [MeshFormat::Usdz, MeshFormat::Usda, MeshFormat::Usdc] {
                let a = write_scene(&fixture, fmt, WriteOptions { self_contained: false, ..Default::default() }).unwrap();
                let b = write_scene(&fixture, fmt, WriteOptions { self_contained: false, ..Default::default() }).unwrap();
                assert_eq!(a.package, b.package, "{name} {fmt}");
            }
        }
    }

    #[test]
    fn usdz_follows_the_package_rules() {
        // Stored, data 64-byte aligned, root layer first, sizes and CRC in the
        // local headers with no data descriptors: what USD's own zip reader
        // (which walks local headers, not the central directory) requires.
        let pkg = write_scene(&fixtures::textured(), MeshFormat::Usdz, WriteOptions::default()).unwrap().package;
        assert_eq!(pkg.files.len(), 1);
        let bytes = &pkg.files[0].bytes;
        let mut at = 0usize;
        let mut names = Vec::new();
        while u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) == 0x0403_4b50 {
            let u16_at = |o: usize| u16::from_le_bytes(bytes[at + o..at + o + 2].try_into().unwrap()) as usize;
            let u32_at = |o: usize| u32::from_le_bytes(bytes[at + o..at + o + 4].try_into().unwrap()) as usize;
            assert_eq!(u16_at(6) & 0x0008, 0, "no data descriptor");
            assert_eq!(u16_at(8), 0, "stored");
            let (crc, csize, usize_) = (u32_at(14), u32_at(18), u32_at(22));
            assert_eq!(csize, usize_);
            let name = String::from_utf8(bytes[at + 30..at + 30 + u16_at(26)].to_vec()).unwrap();
            let data = at + 30 + u16_at(26) + u16_at(28);
            assert_eq!(data % 64, 0, "{name} data at {data}");
            assert_eq!(super::super::zip::crc32(&bytes[data..data + csize]) as usize, crc, "{name} crc");
            names.push(name);
            at = data + csize;
        }
        assert_eq!(names, vec!["model.usda".to_string(), "textures/quadrants.png".to_string()]);
    }

    #[test]
    fn a_usda_package_carries_its_textures_and_a_single_file_reports_them_lost() {
        let w = write_scene(&fixtures::skinned(), MeshFormat::Usda, WriteOptions { self_contained: false, ..Default::default() }).unwrap();
        let paths: Vec<&str> = w.package.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["model.usda", "textures/bands.png"]);
        assert!(String::from_utf8_lossy(&w.package.files[0].bytes).contains("@textures/bands.png@"));
        let one = write_scene(&fixtures::skinned(), MeshFormat::Usda, WriteOptions { self_contained: true, ..Default::default() }).unwrap();
        assert_eq!(one.package.files.len(), 1);
        assert!(!String::from_utf8_lossy(&one.package.files[0].bytes).contains("@textures"));
        assert!(one.losses.iter().any(|l| l.kind == LossKind::Textures), "{:?}", one.losses);
    }

    /// The scene read from USDA text, brought to the precision a crate holds
    /// its transforms at, to be compared with `==` against the same layer's
    /// crate reading, `crate_read`. Litegen writes its xformOps as
    /// `float3`/`quatf`, Sdf stores those in a crate as f32, and our text
    /// reader keeps the decimal by design (rulings U16, U18 and U18b), so:
    ///
    /// - every node's translation and scale are rounded through f32, which
    ///   recovers the crate's value exactly (the text is the f32's shortest
    ///   decimal, and nothing else touches them);
    /// - every node's rotation is held to within 2^-24 per component of the
    ///   crate's, asserted, naming the node and component, and then taken from
    ///   the crate. The bound is half an f32 ulp at 1, the declared precision
    ///   of `quatf`. Rotation needs a bound where the others do not because
    ///   the reader normalises the quaternion after reading it, from the
    ///   decimal on one side and from the f32 on the other, so no rounding of
    ///   the input can make the two equal.
    ///
    /// Everything else about the two scenes stays exact. (`tests/convert_api.rs`
    /// and `proxy/model3d_deliver.rs` keep a copy: they cannot see this one.)
    fn transforms_at_f32(mut text: Scene, crate_read: &Scene) -> Scene {
        const HALF_F32_ULP_AT_ONE: f64 = 1.0 / 16_777_216.0; // 2^-24
        let f32d = |x: f64| f64::from(x as f32);
        for (i, (node, other)) in text.nodes.iter_mut().zip(&crate_read.nodes).enumerate() {
            if let (Transform::Trs { translation, rotation, scale }, Transform::Trs { rotation: crate_rotation, .. }) = (&mut node.transform, &other.transform) {
                *translation = translation.map(f32d);
                *scale = scale.map(f32d);
                for (c, axis) in ["x", "y", "z", "w"].into_iter().enumerate() {
                    let apart = (rotation[c] - crate_rotation[c]).abs();
                    assert!(
                        apart <= HALF_F32_ULP_AT_ONE,
                        "node {i} {:?}: rotation {axis} is {} in the text and {} in the crate, {apart:e} apart, more than 2^-24",
                        node.name,
                        rotation[c],
                        crate_rotation[c]
                    );
                }
                *rotation = *crate_rotation;
            }
        }
        text
    }

    /// The crate is the layer the USDA text parses to, so what one scene
    /// writes as `usdc` reads as the scene what it writes as `usda` reads as,
    /// strictly (every field of the scene, equal, but for the transforms'
    /// precision: [`transforms_at_f32`]), and the files and losses
    /// beside it are the same: every fixture, as a package and as one file,
    /// read as `usdc` and, since a crate is a USD layer, as `usda` too (the
    /// text reader takes either encoding).
    #[test]
    fn usdc_scenes_equal_usda_scenes() {
        for (name, fixture) in fixtures::all() {
            for single in [false, true] {
                let ctx = format!("{name}{}", if single { " single" } else { " package" });
                let opts = WriteOptions { self_contained: single, ..Default::default() };
                let text = write_scene(&fixture, MeshFormat::Usda, opts).unwrap_or_else(|e| panic!("{ctx}: write usda: {e}"));
                let binary = write_scene(&fixture, MeshFormat::Usdc, opts).unwrap_or_else(|e| panic!("{ctx}: write usdc: {e}"));
                let layer = &binary.package.files[0].bytes;
                assert!(layer.starts_with(b"PXR-USDC"), "{ctx}: the first file is a crate, not {:?}", &layer[..layer.len().min(8)]);
                let paths = |p: &Package| p.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>();
                let mut want_paths = paths(&text.package);
                want_paths[0] = "model.usdc".into();
                assert_eq!(paths(&binary.package), want_paths, "{ctx}: the same files, the layer under its own name");
                assert_eq!(binary.package.files[1..], text.package.files[1..], "{ctx}: the sidecars are the same bytes");
                // A loss's `why` names the format written, so the text's names usda.
                let named_usdc: Vec<Loss> = text.losses.iter().map(|l| Loss { why: l.why.replace("usda", "usdc"), ..l.clone() }).collect();
                assert_eq!(binary.losses, named_usdc, "{ctx}: the same losses, each naming usdc where the text's name usda");
                let want = read_scene(&text.package.files[0].bytes, MeshFormat::Usda, &MemorySidecars { files: &text.package.files })
                    .unwrap_or_else(|e| panic!("{ctx}: read the usda: {e}"));
                for as_format in [MeshFormat::Usdc, MeshFormat::Usda] {
                    let got = read_scene(layer, as_format, &MemorySidecars { files: &binary.package.files })
                        .unwrap_or_else(|e| panic!("{ctx}: read the crate as {as_format}: {e}"));
                    assert!(got.scene == transforms_at_f32(want.scene.clone(), &got.scene), "{ctx}: the crate read as {as_format} is another scene than the text");
                    assert_eq!(got.warnings, want.warnings, "{ctx}: read as {as_format}: the same warnings");
                }
            }
        }
    }

    /// The writer's own options reach the crate as they reach the text: lights
    /// in `usd_light_units` (Ruling U11), and a clip keyed at `fps`. A crate
    /// written for Blender's units is not the photometric one, reads as the
    /// text for the same units does, and a crate keyed at 24 fps holds the
    /// samples the 24 fps text does.
    #[test]
    fn usd_light_units_and_fps_apply_to_a_usdc_as_to_a_usda() {
        let write = |scene: &Scene, fmt, opts| write_scene(scene, fmt, opts).unwrap_or_else(|e| panic!("write {fmt}: {e}")).package;
        let staged = fixtures::staged();
        let mut crates = Vec::new();
        for units in [LightUnits::Photometric, LightUnits::Blender] {
            let opts = WriteOptions { usd_light_units: units, ..Default::default() };
            let (text, binary) = (write(&staged, MeshFormat::Usda, opts), write(&staged, MeshFormat::Usdc, opts));
            let read_as = ReadOptions { usd_light_units: units, ..Default::default() };
            let want = read_scene_with(&text.files[0].bytes, MeshFormat::Usda, &NoSidecars, read_as).unwrap();
            let got = read_scene_with(&binary.files[0].bytes, MeshFormat::Usdc, &NoSidecars, read_as).unwrap();
            assert!(got.scene == transforms_at_f32(want.scene, &got.scene), "{units:?}: the crate reads as the text (transforms at f32)");
            crates.push(binary);
        }
        assert!(crates[0] != crates[1], "the Blender crate holds other UsdLux strengths than the photometric one");

        let skinned = fixtures::skinned();
        let at = |fps| write(&skinned, MeshFormat::Usdc, WriteOptions { fps: Some(fps), ..Default::default() });
        let (slow, fast) = (at(24), at(60));
        assert!(slow != fast, "the keys follow the requested fps");
        let text_at_24 = write(&skinned, MeshFormat::Usda, WriteOptions { fps: Some(24), ..Default::default() });
        let (a, b) = (read_scene(&text_at_24.files[0].bytes, MeshFormat::Usda, &NoSidecars).unwrap(), read_scene(&slow.files[0].bytes, MeshFormat::Usdc, &NoSidecars).unwrap());
        assert!(a.scene == b.scene, "a crate keyed at 24 fps reads as the 24 fps text");

        // The read end: a light with no litegen `customData` (every one
        // Blender wrote) reads by the units the caller names, in a crate as
        // in a text layer. Its UsdLux strengths are 3 lux and 20 cd to
        // Blender's importer (`usd_blender_read_follows_blenders_importer`).
        let blender_written = "#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n)\ndef Xform \"R\"\n{\n\
            def Mesh \"tri\" {\n int[] faceVertexCounts = [3]\n int[] faceVertexIndices = [0, 1, 2]\n point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]\n}\n\
            def DistantLight \"sun\" {\n float inputs:intensity = 0.0010980966\n bool inputs:normalize = 1\n float inputs:angle = 0\n}\n}\n";
        let layer = super::super::usda::parse(blender_written.as_bytes(), "model.usda").unwrap();
        let binary = super::super::usdc::write(&layer).unwrap();
        let sun = |bytes: &[u8], fmt, units| {
            let r = read_scene_with(bytes, fmt, &NoSidecars, ReadOptions { usd_light_units: units, ..Default::default() }).unwrap();
            r.scene.lights.iter().find(|l| l.name.as_deref() == Some("sun")).map(|l| l.intensity).expect("the sun")
        };
        for (units, want) in [(LightUnits::Blender, 3.0f32), (LightUnits::Photometric, 0.0010980966)] {
            let text = sun(blender_written.as_bytes(), MeshFormat::Usda, units);
            assert!((text - want).abs() <= 1e-4 * want, "{units:?}: the text layer reads {text}, not {want}");
            for fmt in [MeshFormat::Usdc, MeshFormat::Usda] {
                assert_eq!(sun(&binary, fmt, units), text, "{units:?}: the crate read as {fmt} reads as the text does");
            }
        }
    }

    /// One USDZ entry's name, where its data starts and how long it is.
    fn usdz_entries(bytes: &[u8]) -> Vec<(String, usize, usize)> {
        let (mut at, mut out) = (0usize, Vec::new());
        while bytes.len() >= at + 30 && u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) == 0x0403_4b50 {
            let u16_at = |o: usize| u16::from_le_bytes(bytes[at + o..at + o + 2].try_into().unwrap()) as usize;
            let size = u32::from_le_bytes(bytes[at + 18..at + 22].try_into().unwrap()) as usize;
            let name = String::from_utf8(bytes[at + 30..at + 30 + u16_at(26)].to_vec()).unwrap();
            let data = at + 30 + u16_at(26) + u16_at(28);
            out.push((name, data, size));
            at = data + size;
        }
        out
    }

    /// `usdz_root: usdc` roots the package in the crate: `model.usdc` is the
    /// first entry, as `usda` has `model.usda`; its data starts on a 64-byte
    /// boundary as every USDZ entry's does; it is a crate; and the package
    /// reads back to the scene the default (text-rooted) USDZ reads to. The
    /// default root stays the text one.
    #[test]
    fn usdz_root_usdc_writes_a_crate_root() {
        // A texture, so the entry after the root is aligned too.
        let scene = fixtures::textured();
        let written = |usdz_root| write_scene(&scene, MeshFormat::Usdz, WriteOptions { usdz_root, ..Default::default() }).unwrap().package;
        let (crated, texted) = (written(UsdzRoot::Usdc), written(UsdzRoot::Usda));
        assert!(texted == write_scene(&scene, MeshFormat::Usdz, WriteOptions::default()).unwrap().package, "the default root is usda, byte for byte");
        assert_eq!(crated.files.len(), 1);
        let bytes = &crated.files[0].bytes;
        let entries = usdz_entries(bytes);
        let names: Vec<&str> = entries.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(names, ["model.usdc", "textures/quadrants.png"], "the crate first, the texture after it");
        for (name, data, _) in &entries {
            assert_eq!(data % 64, 0, "{name}: data at {data}");
        }
        let (_, data, len) = &entries[0];
        assert_eq!(&bytes[*data..*data + 8], b"PXR-USDC", "the root is a crate");
        assert!(*len > 8);
        let read = |p: &Package| read_scene(&p.files[0].bytes, MeshFormat::Usdz, &NoSidecars).unwrap_or_else(|e| panic!("read the usdz: {e}"));
        let (a, b) = (read(&crated), read(&texted));
        assert!(a.scene == transforms_at_f32(b.scene, &a.scene), "either root reads back as the same scene (transforms at f32)");
        assert_eq!(a.scene.images.len(), 1, "the texture came with the crate root");
    }

    #[test]
    fn names_are_sanitised_and_restored() {
        let mut s = fixtures::cube();
        s.nodes[0].name = Some("my part #1 (ünï)".into());
        s.meshes[0].name = Some("my part #1 (ünï)".into());
        s.materials[0].name = Some("red / glossy".into());
        let (r, files) = round(&s, MeshFormat::Usda, true);
        let text = String::from_utf8_lossy(&files[0].bytes);
        assert!(text.contains("def Mesh \"my_part__1__"), "{text}");
        assert_eq!(r.scene.nodes[0].name.as_deref(), Some("my part #1 (ünï)"));
        assert_eq!(r.scene.materials[0].name.as_deref(), Some("red / glossy"));
    }

    // ── what prepare and losses promise ─────────────────────────────────────

    fn scene_with_everything_usd_reduces() -> Scene {
        let mut s = fixtures::textured();
        // A second clip, a step, a cubic, unlit, tangents, filters, and vertex
        // colours under a base-colour texture.
        let ch = |path, interpolation, values: Vec<f32>, times: Vec<f32>| Channel { node: 1, path, interpolation, times, values };
        let mut cubic = Vec::new();
        for v in [0.0f32, 1.0] {
            cubic.extend_from_slice(&[0.0, 0.0, 0.0, v, 0.0, 0.0, 0.0, 0.0, 0.0]);
        }
        s.animations.push(Animation {
            name: Some("First".into()),
            channels: vec![
                ch(AnimPath::Translation, Interpolation::Step, vec![-1.2, 0.0, 0.0, -1.2, 1.0, 0.0], vec![0.0, 1.0]),
                Channel { node: 2, ..ch(AnimPath::Translation, Interpolation::CubicSpline, cubic, vec![0.0, 2.0]) },
            ],
        });
        s.animations.push(Animation {
            name: Some("Second".into()),
            channels: vec![ch(AnimPath::Translation, Interpolation::Linear, vec![0.0; 6], vec![0.0, 1.0])],
        });
        s.materials[1].unlit = true;
        let n = s.meshes[0].primitives[0].positions.len();
        s.meshes[0].primitives[0].tangents = Some(vec![[1.0, 0.0, 0.0, 1.0]; n]);
        s.meshes[0].primitives[0].colors = Some(vec![[0.5, 0.5, 0.5, 1.0]; n]);
        s.textures[0].sampler.mag_filter = Some(Filter::Nearest);
        s
    }

    #[test]
    fn losses_name_everything_usd_reduces() {
        let s = scene_with_everything_usd_reduces();
        let w = write_scene(&s, MeshFormat::Usdz, WriteOptions::default()).unwrap();
        let text: Vec<String> = w.losses.iter().map(|l| format!("{}: {l}", l.kind)).collect();
        for needle in ["Second", "vertex colours on 1 textured", "filter modes"] {
            assert!(text.iter().any(|t| t.contains(needle)), "no loss mentions {needle:?}: {text:#?}");
        }
        // Unlit is kept, as a recipe and a marker (usd_write.rs, "Unlit").
        assert!(text.iter().all(|t| !t.contains("unlit")), "unlit reported: {text:#?}");
        // What USD keeps in its own form is not a loss: the resampled cubic
        // and step channels, and tangents its readers derive (D4).
        for needle in ["cubic-spline", "step channel", "tangents"] {
            assert!(text.iter().all(|t| !t.contains(needle)), "{needle:?} reported: {text:#?}");
        }
    }

    #[test]
    fn prepare_reduces_to_exactly_what_the_writer_encodes() {
        let s = scene_with_everything_usd_reduces();
        let p = MeshFormat::Usdz.prepare(&s, WriteOptions::default()).unwrap();
        assert_eq!(p.animations.len(), 1, "one timeline");
        let a = &p.animations[0];
        assert!(a.channels.iter().all(|c| c.interpolation == Interpolation::Linear));
        // The step became a hold until just before the key, then a 1 ms ramp.
        let step = a.channels.iter().find(|c| c.node == 1).unwrap();
        assert_eq!(step.times, vec![0.0, 1.0 - STEP_EPSILON, 1.0]);
        assert_eq!(&step.values[3..6], &[-1.2, 0.0, 0.0]);
        // The cubic was resampled through its own curve.
        let cubic = a.channels.iter().find(|c| c.node == 2).unwrap();
        assert!(cubic.times.len() > 30);
        let mid = pose::sample_channel(cubic, 1.0, 3);
        assert!((mid[0] - 0.5).abs() < 1e-6, "{mid:?}");
        // Unlit stays, with the metallic 1 and roughness 1 of its recipe.
        assert!(p.materials[1].unlit);
        assert_eq!((p.materials[1].metallic, p.materials[1].roughness), (1.0, 1.0));
        let prim = &p.meshes[0].primitives[0];
        assert!(prim.tangents.is_none() && prim.colors.is_none());
        assert!(p.textures.iter().all(|t| t.sampler.mag_filter.is_none()));
        // And what the writer encodes reads back as exactly that.
        let (r, _) = round(&s, MeshFormat::Usdz, true);
        let times: Vec<f32> = (0..=20).map(|i| i as f32 * 0.1).collect();
        assert_draws_alike(&p, &r.scene, &times, FULL, "reduced scene");
    }

    #[test]
    fn half_precision_reaches_animated_joint_scales_only() {
        assert_eq!(to_half_precision(1.25), 1.25);
        assert_eq!(to_half_precision(1.1), 1.099_609_4);
        assert_eq!(to_half_precision(-0.01), -0.010_002_136);
        let mut s = fixtures::skinned();
        if let Transform::Trs { scale, .. } = &mut s.nodes[1].transform {
            *scale = [1.1, 1.1, 1.1];
        }
        let p = MeshFormat::Usdz.prepare(&s, WriteOptions::default()).unwrap();
        let (_, _, sc) = p.nodes[1].transform.trs().unwrap();
        assert_eq!(sc[0] as f32, to_half_precision(1.1));
        let (r, _) = round(&s, MeshFormat::Usdz, true);
        let (_, _, sc) = r.scene.nodes[1].transform.trs().unwrap();
        assert_eq!(sc[0] as f32, to_half_precision(1.1), "the reader sees what USD stores");
    }

    #[test]
    fn texture_transforms_are_baked_into_the_coordinates() {
        let mut s = fixtures::textured();
        let tr = UvTransform { offset: [0.25, -0.5], rotation: 0.6, scale: [2.0, 0.5] };
        s.materials[0].base_color_texture.as_mut().unwrap().transform = Some(tr);
        let (r, _) = round(&s, MeshFormat::Usdz, true);
        let m = &r.scene.materials[0];
        assert!(m.base_color_texture.as_ref().unwrap().transform.is_none());
        let want: Vec<[f32; 2]> = s.meshes[0].primitives[0].uvs[0].iter().map(|uv| tr.apply(*uv)).collect();
        let got = &r.scene.meshes[0].primitives[0].uvs[0];
        assert!(want.iter().zip(got).all(|(a, b)| (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5));
    }

    #[test]
    fn webp_textures_are_dropped_and_reported() {
        let mut s = fixtures::textured();
        s.images[0] = Image { name: Some("w".into()), format: ImageFormat::Webp, data: b"RIFF\0\0\0\0WEBPVP8 ".to_vec() };
        let w = write_scene(&s, MeshFormat::Usdz, WriteOptions::default()).unwrap();
        assert!(w.losses.iter().any(|l| l.kind == LossKind::ImageFormats), "{:?}", w.losses);
        let r = read_scene(&w.package.files[0].bytes, MeshFormat::Usdz, &NoSidecars).unwrap();
        assert!(r.scene.images.is_empty() && r.scene.materials[0].base_color_texture.is_none());
    }

    #[test]
    fn usd_prepare_handles_layer_textures() {
        // A clearcoat map on a WebP beside the box's PNG base colour: USD
        // carries PNG and JPEG alone.
        use super::super::scene::{Clearcoat, LayerSupport, Texture, TextureRef};
        let mut s = fixtures::textured();
        s.images.push(Image { name: Some("coat".into()), format: ImageFormat::Webp, data: b"RIFF\0\0\0\0WEBPVP8 ".to_vec() });
        s.textures.push(Texture { name: None, image: 1, sampler: Default::default() });
        s.materials[0].clearcoat = Some(Clearcoat { factor: 1.0, texture: Some(TextureRef { texture: 1, uv_set: 0, transform: None }), ..Default::default() });
        let opts = WriteOptions::default();
        let usdz = MeshFormat::Usdz.capabilities(opts);
        let portable = |d: &Scene| {
            d.validate().unwrap();
            d.materials.iter().all(|m| m.texture_refs().all(|(_, r)| d.images[d.textures[r.texture].image].format.is_universal()))
                && d.images.iter().all(|i| i.format.is_universal())
        };
        // The drop itself, in a USD that keeps the layer: the layer stays, and
        // its WebP map goes as a base one does.
        let keeps = Capabilities { layers: LayerSupport { clearcoat: true, ..LayerSupport::none() }, ..usdz };
        let d = prepare(&s, &keeps, opts.key_rate(), opts.clips_for(MeshFormat::Usdz)).unwrap();
        assert_eq!(d.materials[0].clearcoat, Some(Clearcoat { factor: 1.0, ..Default::default() }));
        assert!(portable(&d), "{:?}", d.images);
        // USD as its capabilities stand: the layer goes, reported, or stays
        // untextured, and the WebP image is reported either way.
        let d = MeshFormat::Usdz.prepare(&s, opts).unwrap();
        assert!(portable(&d), "{:?}", d.images);
        assert_eq!(d.materials[0].clearcoat.is_some(), usdz.layers.clearcoat);
        let losses = write_scene(&s, MeshFormat::Usdz, opts).unwrap().losses;
        let named = losses.iter().filter(|l| l.kind == LossKind::Materials && l.what == "clearcoat on 1 material(s)").count();
        assert_eq!(named, usize::from(!usdz.layers.clearcoat), "{losses:?}");
        assert!(losses.iter().any(|l| l.kind == LossKind::ImageFormats), "{losses:?}");
    }

    // ── crate decoding ──────────────────────────────────────────────────────

    #[test]
    fn every_crate_encoding_decodes_to_its_text_original() {
        let text = super::super::usda::parse(STRESS_USDA, "stress.usda").unwrap();
        let crate_ = super::super::usdc::parse(STRESS_USDC, "stress.usdc").unwrap();
        assert_eq!(text.meta_str("defaultPrim"), crate_.meta_str("defaultPrim"));
        assert_eq!(text.meta("customLayerData"), crate_.meta("customLayerData"));
        assert_eq!(text.root_children, crate_.root_children);
        let (a, b) = (&text.specs["/S"], &crate_.specs["/S"]);
        assert_eq!(a.children, b.children);
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.custom_data, b.custom_data);
        let mut checked = 0;
        for (name, prop) in &a.props {
            let other = b.prop(name).unwrap_or_else(|| panic!("crate lacks {name}"));
            match (prop, other) {
                (super::super::usd_stage::Prop::Attr(x), super::super::usd_stage::Prop::Attr(y)) => {
                    assert_eq!(x.type_name, y.type_name, "{name}");
                    let same = |p: &Value, q: &Value| -> bool {
                        match (p.doubles(), q.doubles(), p.as_tuple(), q.as_tuple()) {
                            (Some(m), Some(n), _, _) => m.len() == n.len() && m.iter().zip(n.iter()).all(|(u, v)| (u - v).abs() <= 1e-3 * u.abs().max(1e-3)),
                            (_, _, Some(m), Some(n)) => m.len() == n.len() && m.iter().zip(n).all(|(u, v)| (u - v).abs() <= 1e-3 * u.abs().max(1e-3)),
                            _ => match (p.as_f64(), q.as_f64()) {
                                (Some(u), Some(v)) => (u - v).abs() <= 1e-3 * u.abs().max(1e-3),
                                _ => p == q,
                            },
                        }
                    };
                    match (&x.default, &y.default) {
                        (Some(p), Some(q)) => assert!(same(p, q), "{name}: {p:?} vs {q:?}"),
                        (None, None) => {}
                        (p, q) => panic!("{name}: {p:?} vs {q:?}"),
                    }
                    let (sx, sy) = (x.samples.clone().unwrap_or_default(), y.samples.clone().unwrap_or_default());
                    assert_eq!(sx.len(), sy.len(), "{name} samples");
                    for ((t, p), (u, q)) in sx.iter().zip(&sy) {
                        assert_eq!(t, u, "{name} sample time");
                        assert!(same(p, q), "{name} sample {p:?} vs {q:?}");
                    }
                    assert_eq!(x.connections, y.connections, "{name} connections");
                }
                (super::super::usd_stage::Prop::Rel(x), super::super::usd_stage::Prop::Rel(y)) => assert_eq!(x, y, "{name}"),
                _ => panic!("{name}: attribute vs relationship"),
            }
            checked += 1;
        }
        assert_eq!(checked, 29, "properties compared");
        // The exact values that matter most: integers survive their
        // compression, LUT floats their table, an int8-inlined double3 and a
        // diagonal-inlined matrix their shortcuts.
        let get = |n: &str| b.attr(n).unwrap().default.clone().unwrap();
        assert_eq!(get("ints").ints().unwrap()[20..], [-5, 1_000_000, -70_000, 3]);
        assert_eq!(get("wide").ints().unwrap()[3..5], [5_000_000_000, -5_000_000_000]);
        assert_eq!(get("small"), Value::Tuple(vec![1.0, -2.0, 3.0]));
        assert_eq!(get("ident").as_tuple().unwrap()[15], 1.0);
        assert_eq!(get("moved").as_tuple().unwrap()[12..15], [5.0, -6.0, 7.5]);
        assert_eq!(get("q"), Value::Tuple(vec![0.5, -0.5, 0.5, 0.5]), "quaternions are stored x, y, z, w");
        assert_eq!(get("hlut").floats().unwrap()[..2], [1.25, 0.5]);
        assert_eq!(get("str"), Value::Str("a string with \"quotes\"".into()));
    }

    // ── composition ─────────────────────────────────────────────────────────

    fn comp_sidecars() -> Vec<PackageFile> {
        vec![
            PackageFile { path: "comp_sub.usda".into(), bytes: COMP_SUB.to_vec() },
            PackageFile { path: "comp_payload.usda".into(), bytes: COMP_PAYLOAD.to_vec() },
            PackageFile { path: "comp_anim.usda".into(), bytes: COMP_ANIM.to_vec() },
        ]
    }

    #[test]
    fn composition_matches_what_usdcat_flatten_shows() {
        let side = comp_sidecars();
        for (what, bytes) in [("text", COMP_USDA), ("crate", COMP_USDC)] {
            let r = read_usda(bytes, &MemorySidecars { files: &side }).unwrap_or_else(|e| panic!("{what}: {e}"));
            let s = &r.scene;
            let find = |n: &str| s.nodes.iter().position(|x| x.name.as_deref() == Some(n));
            let node = |n: &str| &s.nodes[find(n).unwrap_or_else(|| panic!("{what}: no node {n}"))];
            // Root opinions beat the sublayer's.
            let (t, _, _) = node("Instance").transform.trs().unwrap();
            assert_eq!(t, [3.0, 0.0, 0.0], "{what}");
            // The reference brought the mesh, with its binding remapped.
            let inst = node("Instance");
            let m = &s.meshes[inst.mesh.unwrap()];
            let mat = &s.materials[m.primitives[0].material.unwrap()];
            assert_eq!(&mat.base_color[..3], &[0.0, 1.0, 0.0], "{what}: remapped material binding");
            let (t, _, _) = node("Lifted").transform.trs().unwrap();
            assert_eq!(t, [0.0, 5.0, 0.0], "{what}: inherited from the class");
            assert!(find("Quad").is_some() && find("Tri").map(|i| s.nodes[i].mesh.is_some()) == Some(true), "{what}: variant");
            let shapes = node("Shapes");
            assert_eq!(shapes.children.len(), 1, "{what}: only the selected variant");
            assert!(node("Unselected").children.is_empty(), "{what}");
            assert!(r.warnings.iter().any(|w| w.contains("\"look\"") && w.contains("no selection")), "{what}: {:?}", r.warnings);
            assert!(find("Inactive").is_none() && find("NotDrawn").is_none() && find("Detached").is_none(), "{what}");
            assert!(find("Payload").is_some() && find("FromSublayer").is_some(), "{what}");
            // The offset reference: keys 0 and 5 at offset 10, scale 2, at 10
            // time codes per second, are seconds 1 and 2.
            let timed = find("Timed").unwrap();
            let ch = r.scene.animations[0].channels.iter().find(|c| c.node == timed).unwrap();
            assert_eq!(ch.times, vec![1.0, 2.0], "{what}");
        }
    }

    #[test]
    fn composition_cycles_and_missing_layers_are_warnings_not_hangs() {
        let text = br#"#usda 1.0
(
    defaultPrim = "A"
    subLayers = [@self.usda@, @missing.usda@]
)
def Xform "A" (
    prepend references = [</A>, @missing.usda@</X>, @self.usda@</A>]
)
{
    def Mesh "M"
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
    }
}
"#;
        let side = vec![PackageFile { path: "self.usda".into(), bytes: text.to_vec() }];
        let started = std::time::Instant::now();
        let r = read_usda(text, &MemorySidecars { files: &side }).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(r.warnings.iter().any(|w| w.contains("missing.usda")), "{:?}", r.warnings);
        assert_eq!(r.scene.triangle_count(), 1);
    }

    #[test]
    fn instancing_cannot_amplify_a_small_file_past_the_vertex_ceiling() {
        // One 1000-point mesh referenced 5000 times is 5M vertices from a
        // ~300 KB file.
        let mut t = String::from("#usda 1.0\ndef Mesh \"Proto\" {\n    int[] faceVertexCounts = [3]\n    int[] faceVertexIndices = [0, 1, 2]\n    point3f[] points = [");
        for i in 0..1000 {
            t.push_str(&format!("({i}, 0, 0), "));
        }
        t.push_str("]\n}\n");
        for i in 0..5000 {
            t.push_str(&format!("def \"I{i}\" (references = </Proto>) {{}}\n"));
        }
        let started = std::time::Instant::now();
        let e = read_usda(t.as_bytes(), &NoSidecars).unwrap_err();
        assert!(matches!(e, MeshError::Unsupported(_)), "{e:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    }

    // ── files written by other tools ────────────────────────────────────────

    #[test]
    fn reads_the_crate_files_usdcat_wrote_from_our_fixtures() {
        for (name, bytes) in [("cube", CUBE_USDC), ("skinned", SKINNED_USDC), ("morph", MORPH_USDC)] {
            let fixture = fixtures::by_name(name).unwrap();
            let want = MeshFormat::Usda.prepare(&fixture, WriteOptions { self_contained: false, ..Default::default() }).unwrap();
            let r = read_scene(bytes, MeshFormat::Usda, &NoSidecars).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(names(&r.scene), names(&want), "{name}");
            // Textures are sidecars these tests do not supply.
            let check = Check { textures: false, materials: name != "skinned", ..FULL };
            assert_draws_alike(&want, &r.scene, &fixtures::sample_times(&fixture), check, name);
        }
    }

    #[test]
    fn reads_a_usdz_apples_packager_rewrote_with_a_crate_root() {
        let fixture = fixtures::textured();
        let want = MeshFormat::Usdz.prepare(&fixture, WriteOptions::default()).unwrap();
        let r = read_scene(TEXTURED_ARKIT, MeshFormat::Usdz, &NoSidecars).unwrap();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_draws_alike(&want, &r.scene, &[], FULL, "arkit");
    }

    #[test]
    fn reads_apples_gltf_importer_output() {
        // NodeGraph and Material-interface connections, unflipped st with a
        // UsdTransform2d V-flip per texture (baked back to plain UVs on read),
        // and the mirrored box instanced through an internal reference.
        let fixture = fixtures::textured();
        let want = MeshFormat::Usdz.prepare(&fixture, WriteOptions::default()).unwrap();
        let r = read_scene(TEXTURED_APPLE, MeshFormat::Usdz, &NoSidecars).unwrap();
        assert!(r.scene.materials.iter().all(|m| m.texture_refs().all(|(_, t)| t.transform.is_none())));
        // Apple's panel material never reads displayColor, so USD draws it
        // white and so do we; and with no texture to carry a V-flip, the
        // coordinates on the untextured parts stay as glTF had them, which
        // nothing draws. Otherwise the same picture.
        let check = Check { colors: false, materials: false, unread_uvs: false, ..FULL };
        assert_draws_alike(&want, &r.scene, &[], check, "apple");
        let box_mat = r.scene.materials.iter().find(|m| m.name.as_deref() == Some("box_mat")).unwrap();
        assert!(box_mat.base_color_texture.is_some());
    }

    #[test]
    fn reads_apples_skinned_import_with_its_quirks() {
        let r = read_scene(SKINNED_APPLE, MeshFormat::Usdz, &NoSidecars).unwrap();
        let s = &r.scene;
        // The invisible, proxy-purpose Skeleton still skins.
        assert_eq!(s.skins.len(), 1);
        let joint_names: Vec<_> = s.skins[0].joints.iter().map(|j| s.nodes[*j].name.clone().unwrap()).collect();
        assert_eq!(joint_names, ["hip", "knee", "ankle"], "jointNames name the joints");
        assert!(!s.animations.is_empty());
        // Its hip scale really is zero at t = 0 (the importer read glTF
        // in-tangents as values), and is carried as such, as TRS.
        let hip = s.skins[0].joints[0];
        let (_, _, sc) = s.nodes[hip].transform.trs().unwrap();
        assert_eq!(sc, [0.0; 3]);
        assert!(fixtures::bounds_over_animation(s).is_some());
    }

    #[test]
    fn reads_threejs_exporter_output() {
        // Meshes in external layers, the box referenced twice, materials in a
        // root prim outside the default prim.
        let fixture = fixtures::textured();
        let want = MeshFormat::Usdz.prepare(&fixture, WriteOptions::default()).unwrap();
        let r = read_scene(TEXTURED_THREE, MeshFormat::Usdz, &NoSidecars).unwrap();
        // three.js keeps only displayColor's first value and re-encodes the
        // PNG, so compare the geometry and the coordinates.
        let check = Check { colors: false, materials: false, textures: false, tol: 1e-5, ..FULL };
        assert_draws_alike(&want, &r.scene, &[], check, "three.js");
    }

    // ── uv sets ─────────────────────────────────────────────────────────────

    /// Each distinct vertex of `p` as its position and its uv sets, in
    /// position order: the same whatever vertex order a reader chooses.
    fn corner_sets(p: &Primitive) -> Vec<([f32; 3], Vec<[f32; 2]>)> {
        let mut out: Vec<([f32; 3], Vec<[f32; 2]>)> =
            (0..p.positions.len()).map(|v| (p.positions[v], p.uvs.iter().map(|set| set[v]).collect())).collect();
        out.sort_by_key(|(at, _)| at.map(f32::to_bits));
        out.dedup();
        out
    }

    /// `got` carries `want`'s uv sets `order`, in that order, corner for
    /// corner, each value within `tol`.
    fn assert_sets(got: &Primitive, want: &Primitive, order: &[usize], tol: f32, ctx: &str) {
        let picked = Primitive { uvs: order.iter().map(|&i| want.uvs[i].clone()).collect(), ..want.clone() };
        let (g, w) = (corner_sets(got), corner_sets(&picked));
        assert_eq!(g.len(), w.len(), "{ctx}: corners");
        for ((at, gs), (want_at, ws)) in g.iter().zip(&w) {
            assert_eq!(at, want_at, "{ctx}: corner positions");
            assert_eq!(gs.len(), ws.len(), "{ctx}: uv sets at {at:?}");
            for (k, (a, b)) in gs.iter().zip(ws).enumerate() {
                let close = (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol;
                assert!(close, "{ctx}: set {k} at {at:?} is {a:?}, not {b:?} (the source's set {})", order[k]);
            }
        }
    }

    /// A material's texture bindings, as (slot, uv set) in slot order.
    fn uv_bindings(m: &Material) -> Vec<(&'static str, u32)> {
        m.texture_refs().map(|(slot, r)| (slot.as_str(), r.uv_set)).collect()
    }

    /// The bindings of the material `p` draws with; none without one.
    fn bindings_of(s: &Scene, p: &Primitive) -> Vec<(&'static str, u32)> {
        p.material.and_then(|m| s.materials.get(m)).map(uv_bindings).unwrap_or_default()
    }

    /// The `texCoord2f[]` primvars a layer this module wrote declares, in order.
    fn uv_primvars(layer: &[u8]) -> Vec<String> {
        String::from_utf8_lossy(layer)
            .lines()
            .filter_map(|l| l.trim_start().strip_prefix("texCoord2f[] primvars:"))
            .filter_map(|rest| rest.split(' ').next())
            .map(str::to_string)
            .collect()
    }

    /// The `set` of every TEXCOORD input a written Collada document declares,
    /// in order.
    fn texcoord_sets(doc: &[u8]) -> Vec<String> {
        String::from_utf8_lossy(doc)
            .split(r#"<input semantic="TEXCOORD""#)
            .skip(1)
            .filter_map(|rest| rest.split(r#" set=""#).nth(1)?.split('"').next().map(str::to_string))
            .collect()
    }

    #[test]
    fn usda_writes_and_reads_every_uv_set() {
        // A USDA holds eight sets. multiuv's four are written as `st` … `st3`
        // in scene order (nothing is cut, so nothing moves) and read back by
        // name, the base colour still on set 3 and occlusion on set 1, with
        // no set reported lost. A single file carries no textures, but still
        // all four sets.
        let fixture = fixtures::multiuv();
        let source = &fixture.meshes[0].primitives[0];
        for single in [false, true] {
            let ctx = if single { "usda, one file" } else { "usda package" };
            let w = write_scene(&fixture, MeshFormat::Usda, WriteOptions { self_contained: single, ..Default::default() }).unwrap();
            assert!(!w.losses.iter().any(|l| l.kind == LossKind::Uvs), "{ctx}: {:?}", w.losses);
            assert_eq!(uv_primvars(&w.package.files[0].bytes), ["st", "st1", "st2", "st3"], "{ctx}");
            let side = MemorySidecars { files: &w.package.files };
            let r = read_scene(&w.package.files[0].bytes, MeshFormat::Usda, &side).unwrap();
            assert!(r.warnings.is_empty(), "{ctx}: {:?}", r.warnings);
            let p = &r.scene.meshes[0].primitives[0];
            assert_sets(p, source, &[0, 1, 2, 3], 1e-7, ctx);
            let want = if single { Vec::new() } else { uv_bindings(&fixture.materials[0]) };
            assert_eq!(bindings_of(&r.scene, p), want, "{ctx}: bindings");
        }
    }

    #[test]
    fn usdc_reads_every_uv_set() {
        // multiuv's USDA package root as Pixar's crate writer re-encodes it
        // (`testdata/usd/README.md`): the same four sets and bindings as the
        // text. Its textures are the package's own files.
        let fixture = fixtures::multiuv();
        let w = write_scene(&fixture, MeshFormat::Usda, WriteOptions { self_contained: false, ..Default::default() }).unwrap();
        let r = read_scene(MULTIUV_USDC, MeshFormat::Usda, &MemorySidecars { files: &w.package.files }).unwrap();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let p = &r.scene.meshes[0].primitives[0];
        assert_sets(p, &fixture.meshes[0].primitives[0], &[0, 1, 2, 3], 1e-7, "usdc");
        assert_eq!(bindings_of(&r.scene, p), uv_bindings(&fixture.materials[0]), "usdc: bindings");
    }

    #[test]
    fn usdz_keeps_the_two_sets_its_textures_use() {
        // A USDZ holds RealityKit's two sets: the two multiuv's textures
        // sample, the fixture's set 3 as set 0 under the base colour and its
        // set 1 as set 1 under occlusion. The two sets nothing samples are
        // the loss, and no texture is.
        let fixture = fixtures::multiuv();
        let w = write_scene(&fixture, MeshFormat::Usdz, WriteOptions::default()).unwrap();
        let reported: Vec<(LossKind, &str)> =
            w.losses.iter().filter(|l| matches!(l.kind, LossKind::Uvs | LossKind::Textures)).map(|l| (l.kind, l.what.as_str())).collect();
        assert_eq!(reported, [(LossKind::Uvs, "2 of 4 uv set(s)")], "{:?}", w.losses);
        let r = read_scene(&w.package.files[0].bytes, MeshFormat::Usdz, &NoSidecars).unwrap();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let p = &r.scene.meshes[0].primitives[0];
        assert_sets(p, &fixture.meshes[0].primitives[0], &[3, 1], 1e-7, "usdz");
        assert_eq!(bindings_of(&r.scene, p), [("baseColor", 0), ("occlusion", 1)], "usdz: bindings");
    }

    /// One quad with nine distinct uv sets and no material: set `i` maps the
    /// corners' `(u, v)` to `((u + i) / 8, (v + 7 − i) / 8)`, exact in every
    /// text format, V flip included.
    fn nine_set_quad() -> Scene {
        let corners: [([f32; 3], [f32; 2]); 4] = [
            ([-0.5, -0.5, 0.0], [0.0, 1.0]),
            ([0.5, -0.5, 0.0], [1.0, 1.0]),
            ([0.5, 0.5, 0.0], [1.0, 0.0]),
            ([-0.5, 0.5, 0.0], [0.0, 0.0]),
        ];
        let p = Primitive {
            positions: corners.iter().map(|(at, _)| *at).collect(),
            normals: Some(vec![[0.0, 0.0, 1.0]; 4]),
            uvs: (0..9).map(|i| corners.iter().map(|(_, [u, v])| [(u + i as f32) / 8.0, (v + 7.0 - i as f32) / 8.0]).collect()).collect(),
            indices: vec![0, 1, 2, 0, 2, 3],
            ..Default::default()
        };
        let mut s = Scene::default();
        s.meshes.push(SceneMesh { primitives: vec![p], ..Default::default() });
        s.add_root(Node { mesh: Some(0), ..Default::default() });
        s
    }

    #[test]
    fn nine_sets_still_report_one_lost() {
        // USDA and Collada hold eight sets. Of nine, the ninth is reported
        // lost, and the file carries the other eight (USDA's `st` … `st7`,
        // Collada's TEXCOORD sets 0 … 7), which read back as the source's
        // first eight.
        let s = nine_set_quad();
        let source = &s.meshes[0].primitives[0];
        for fmt in [MeshFormat::Usda, MeshFormat::Dae] {
            let w = write_scene(&s, fmt, WriteOptions::default()).unwrap();
            let uvs: Vec<&str> = w.losses.iter().filter(|l| l.kind == LossKind::Uvs).map(|l| l.what.as_str()).collect();
            assert_eq!(uvs, ["1 of 9 uv set(s)"], "{fmt}: {:?}", w.losses);
            let file = &w.package.files[0].bytes;
            let (written, want): (Vec<String>, Vec<String>) = if fmt == MeshFormat::Usda {
                (uv_primvars(file), (0..8).map(|i| if i == 0 { "st".to_string() } else { format!("st{i}") }).collect())
            } else {
                (texcoord_sets(file), (0..8).map(|i: u32| i.to_string()).collect())
            };
            assert_eq!(written, want, "{fmt}: the sets the file declares");
            let r = read_scene(file, fmt, &NoSidecars).unwrap();
            assert!(r.warnings.is_empty(), "{fmt}: {:?}", r.warnings);
            assert_sets(&r.scene.meshes[0].primitives[0], source, &[0, 1, 2, 3, 4, 5, 6, 7], 0.0, fmt.as_str());
        }
    }

    // ── material layers ─────────────────────────────────────────────────────

    /// One quad per material, side by side, each with the first `sets` of
    /// [`nine_set_quad`]'s uv sets. Texture 0 is the quadrants image and
    /// texture 1 the bands.
    fn quads(materials: Vec<Material>, sets: usize) -> Scene {
        let mut quad = nine_set_quad().meshes[0].primitives[0].clone();
        quad.uvs.truncate(sets);
        let mut s = Scene::default();
        for (i, (name, data)) in [("quadrants", fixtures::quadrant_png()), ("bands", fixtures::bands_png())].into_iter().enumerate() {
            s.images.push(Image { name: Some(name.into()), format: ImageFormat::Png, data });
            s.textures.push(super::super::scene::Texture { name: None, image: i, sampler: Default::default() });
        }
        for (i, m) in materials.into_iter().enumerate() {
            let name = m.name.clone().unwrap_or_default();
            s.meshes.push(SceneMesh { name: Some(name.clone()), primitives: vec![Primitive { material: Some(i), ..quad.clone() }], ..Default::default() });
            let transform = Transform::Trs { translation: [1.5 * i as f64, 0.0, 0.0], rotation: math::QUAT_IDENTITY, scale: [1.0; 3] };
            s.add_root(Node { name: Some(format!("{name}_node")), mesh: Some(i), transform, ..Default::default() });
            s.materials.push(m);
        }
        s
    }

    fn material_named<'a>(s: &'a Scene, name: &str) -> &'a Material {
        s.materials.iter().find(|m| m.name.as_deref() == Some(name)).unwrap_or_else(|| panic!("no material {name}"))
    }

    /// The primitive drawn with the material named `name`.
    fn prim_named<'a>(s: &'a Scene, name: &str) -> &'a Primitive {
        let drawn = |p: &&Primitive| p.material.and_then(|i| s.materials.get(i)).is_some_and(|m| m.name.as_deref() == Some(name));
        s.meshes.iter().flat_map(|m| &m.primitives).find(drawn).unwrap_or_else(|| panic!("no primitive drawn with {name}"))
    }

    /// Which fixture image `data` holds: "quadrants", "bands" or "another".
    fn which_image(data: &[u8]) -> &'static str {
        if data == fixtures::quadrant_png().as_slice() {
            "quadrants"
        } else if data == fixtures::bands_png().as_slice() {
            "bands"
        } else {
            "another"
        }
    }

    /// What a binding samples, its image ([`which_image`]) and its uv set,
    /// whatever index the reader gave its texture.
    fn sampled(s: &Scene, r: &Option<super::super::scene::TextureRef>) -> Option<(&'static str, u32)> {
        r.as_ref().map(|r| (which_image(&s.images[s.textures[r.texture].image].data), r.uv_set))
    }

    /// A layer's trimmed lines.
    fn layer_lines(layer: &[u8]) -> Vec<String> {
        String::from_utf8_lossy(layer).lines().map(|l| l.trim().to_string()).collect()
    }

    /// USD keeps the clearcoat and the ior. The clearcoat's factor and
    /// roughness ride in UsdPreviewSurface's `clearcoat` and
    /// `clearcoatRoughness`, through a UsdUVTexture's `r` and `g` when mapped
    /// (glTF's channels), the roughness written even at 0 (USD's default is
    /// 0.01); the ior in `ior`, here on `refractive`, which has no clearcoat
    /// (USD's ior also sets a clearcoat's reflectance, a loss of its own). The
    /// maps are on set 1: `coated` samples its base colour from set 0 too,
    /// and `coat_only`'s clearcoat map is its only one, so its reader meets
    /// `st1` first and must renumber the binding to the mesh's set 1. A
    /// single USDA keeps the numbers without the maps.
    #[test]
    fn clearcoat_and_ior_round_trip_through_usd() {
        use super::super::scene::{Clearcoat, TextureRef};
        let map = |texture: usize, uv_set: u32| Some(TextureRef { texture, uv_set, transform: None });
        let coated = Material {
            name: Some("coated".into()),
            base_color_texture: map(0, 0),
            clearcoat: Some(Clearcoat { factor: 0.8, texture: map(1, 1), roughness: 0.3, roughness_texture: map(1, 1), ..Default::default() }),
            ..Default::default()
        };
        let coat_only = Material {
            name: Some("coat_only".into()),
            clearcoat: Some(Clearcoat { factor: 1.0, texture: map(1, 1), roughness: 0.0, ..Default::default() }),
            ..Default::default()
        };
        let refractive = Material { name: Some("refractive".into()), ior: Some(1.45), ..Default::default() };
        let s = quads(vec![coated, coat_only, refractive], 2);
        for (fmt, single) in [(MeshFormat::Usdz, false), (MeshFormat::Usda, false), (MeshFormat::Usda, true)] {
            let ctx = format!("{fmt}{}", if single { " single" } else { "" });
            let w = write_scene(&s, fmt, WriteOptions { self_contained: single, ..Default::default() }).unwrap();
            let reported: Vec<(LossKind, &str)> = w.losses.iter().map(|l| (l.kind, l.what.as_str())).collect();
            let want: &[(LossKind, &str)] = if single { &[(LossKind::Textures, "4 texture binding(s)")] } else { &[] };
            assert_eq!(reported, want, "{ctx}");
            let r = read_scene(&w.package.files[0].bytes, fmt, &MemorySidecars { files: &w.package.files }).unwrap();
            assert!(r.warnings.is_empty(), "{ctx}: {:?}", r.warnings);
            let on = |image: &'static str, set: u32| (!single).then_some((image, set));
            let coated = material_named(&r.scene, "coated");
            let c = coated.clearcoat.as_ref().unwrap_or_else(|| panic!("{ctx}: coated has no clearcoat"));
            assert_eq!((c.factor, c.roughness, &c.normal_texture, c.normal_scale), (0.8, 0.3, &None, 1.0), "{ctx}: coated");
            assert_eq!(sampled(&r.scene, &c.texture), on("bands", 1), "{ctx}: coated's clearcoat map");
            assert_eq!(sampled(&r.scene, &c.roughness_texture), on("bands", 1), "{ctx}: coated's roughness map");
            assert_eq!(sampled(&r.scene, &coated.base_color_texture), on("quadrants", 0), "{ctx}: coated's base colour");
            let only = material_named(&r.scene, "coat_only");
            let c = only.clearcoat.as_ref().unwrap_or_else(|| panic!("{ctx}: coat_only has no clearcoat"));
            assert_eq!((c.factor, c.roughness, &c.roughness_texture), (1.0, 0.0, &None), "{ctx}: coat_only");
            assert_eq!(sampled(&r.scene, &c.texture), on("bands", 1), "{ctx}: coat_only's map, its first read, is on the mesh's set 1");
            let iors = ["coated", "coat_only", "refractive"].map(|name| material_named(&r.scene, name).ior);
            assert_eq!(iors, [None, None, Some(1.45)], "{ctx}: the iors");
            assert_eq!(material_named(&r.scene, "refractive").clearcoat, None, "{ctx}");
            for name in ["coated", "coat_only", "refractive"] {
                assert_sets(prim_named(&r.scene, name), &s.meshes[0].primitives[0], &[0, 1], 1e-7, &format!("{ctx}: {name}"));
            }
            if fmt == MeshFormat::Usda {
                let lines = layer_lines(&w.package.files[0].bytes);
                let text = lines.join("\n");
                let has = |l: &str| lines.iter().any(|x| x == l);
                let connects = |input: &str, source: &str| {
                    lines.iter().any(|l| l.starts_with(&format!("float inputs:{input}.connect = <")) && l.ends_with(&format!("/{source}>")))
                };
                if single {
                    for l in ["float inputs:clearcoat = 0.8", "float inputs:clearcoatRoughness = 0.3", "float inputs:clearcoat = 1"] {
                        assert!(has(l), "{ctx}: no {l:?} in\n{text}");
                    }
                } else {
                    assert!(connects("clearcoat", "ClearcoatTex.outputs:r") && has("float outputs:r"), "{ctx}: the clearcoat from a map's R\n{text}");
                    assert!(connects("clearcoatRoughness", "ClearcoatRoughnessTex.outputs:g") && has("float outputs:g"), "{ctx}: its roughness from G\n{text}");
                }
                assert!(has("float inputs:clearcoatRoughness = 0"), "{ctx}: a roughness of 0 is written\n{text}");
                assert!(has("float inputs:ior = 1.45"), "{ctx}\n{text}");
            }
        }
    }

    /// USD approximates no layer it lacks: `layered()`'s transmission,
    /// volume, specular, sheen, iridescence, anisotropy and dispersion are
    /// one materials loss each, its clearcoat and ior none, and the opacity
    /// written is the base alpha, whatever the transmission. The car paint's
    /// clearcoat normal map is named apart: USD's clearcoat takes the base
    /// normal map, and the car paint has none. A single USDA drops every map
    /// anyway, which is its texture loss. The lacquer's clearcoat sits on an
    /// ior of 1.45, which USD's clearcoat reflects with where glTF's keeps
    /// 1.5: named too.
    #[test]
    fn usd_reports_transmission_and_keeps_alpha() {
        let dropped = ["transmission", "volume", "specular", "sheen", "iridescence", "anisotropy", "dispersion"];
        let mut blended = fixtures::layered();
        let glass = blended.materials.iter().position(|m| m.name.as_deref() == Some("glass")).unwrap();
        blended.materials[glass].alpha_mode = AlphaMode::Blend;
        blended.materials[glass].base_color[3] = 0.6;
        for (fmt, single) in [(MeshFormat::Usdz, false), (MeshFormat::Usda, false), (MeshFormat::Usda, true)] {
            let ctx = format!("{fmt}{}", if single { " single" } else { "" });
            let opts = WriteOptions { self_contained: single, ..Default::default() };
            let w = write_scene(&fixtures::layered(), fmt, opts).unwrap();
            let mut named: Vec<&str> = w.losses.iter().filter(|l| l.kind == LossKind::Materials).map(|l| l.what.as_str()).collect();
            named.sort_unstable();
            let mut want: Vec<String> = dropped.iter().map(|l| format!("{l} on 1 material(s)")).collect();
            want.push("a clearcoat with an ior other than 1.5 on 1 material(s)".into());
            if !single {
                want.push("clearcoat normals of 1 material(s)".into());
            }
            want.sort_unstable();
            assert_eq!(named, want, "{ctx}: {:#?}", w.losses);
            for (scene, alpha) in [(fixtures::layered(), None), (blended.clone(), Some(0.6))] {
                let w = write_scene(&scene, fmt, opts).unwrap();
                let r = read_scene(&w.package.files[0].bytes, fmt, &MemorySidecars { files: &w.package.files }).unwrap();
                let g = material_named(&r.scene, "glass");
                assert_eq!(g.transmission, None, "{ctx}");
                let mode = if alpha.is_some() { AlphaMode::Blend } else { AlphaMode::Opaque };
                assert_eq!((g.alpha_mode, g.base_color[3]), (mode, alpha.unwrap_or(1.0)), "{ctx}: the glass's alpha");
                if fmt == MeshFormat::Usda {
                    let lines = layer_lines(&w.package.files[0].bytes);
                    let opacity: Vec<&String> = lines.iter().filter(|l| l.starts_with("float inputs:opacity ")).collect();
                    let want: Vec<String> = alpha.map(|a| format!("float inputs:opacity = {a}")).into_iter().collect();
                    assert_eq!(opacity, want.iter().collect::<Vec<_>>(), "{ctx}: the opacity written");
                }
            }
        }
    }

    // ── unlit ───────────────────────────────────────────────────────────────

    /// The trimmed lines of Material prim `name` in a USDA layer, from its
    /// `def` to its closing brace.
    fn material_block(layer: &[u8], name: &str) -> Vec<String> {
        let text = String::from_utf8_lossy(layer);
        let lines: Vec<&str> = text.lines().collect();
        let head = format!("def Material \"{name}\"");
        let start = lines.iter().position(|l| l.trim_start().starts_with(&head)).unwrap_or_else(|| panic!("no material {name} in\n{text}"));
        let indent = &lines[start][..lines[start].len() - lines[start].trim_start().len()];
        let close = format!("{indent}}}");
        let end = lines[start..].iter().position(|l| **l == *close).map(|i| start + i + 1).unwrap_or_else(|| panic!("material {name} never closes in\n{text}"));
        lines[start..end].iter().map(|l| l.trim().to_string()).collect()
    }

    /// `textured()` with two materials unlit and one lit: the box (a base
    /// colour texture under a tint, over a metallic 0 and a roughness of 0.6,
    /// which an unlit material has no use for) and the wedge (a flat colour)
    /// are unlit, and the panel is the lit control.
    fn unlit_scene() -> Scene {
        let mut s = fixtures::textured();
        s.materials[0].unlit = true;
        s.materials[0].base_color = [0.9, 0.7, 0.5, 1.0];
        s.materials[1].unlit = true;
        s
    }

    const EVERY_USD_FORM: [(MeshFormat, bool); 5] =
        [(MeshFormat::Usdz, false), (MeshFormat::Usda, false), (MeshFormat::Usda, true), (MeshFormat::Usdc, false), (MeshFormat::Usdc, true)];

    /// USD keeps an unlit material: UsdPreviewSurface has no unlit switch, so
    /// it is the recipe every converter uses (`usd_write.rs`, "Unlit"), `diffuseColor`
    /// black, `metallic` 1, `roughness` 1 and `emissiveColor` the base colour
    /// (a texture's `rgb` with `scale` the tint), and litegen's marker
    /// `customData.litegen.unlit`, which reads the material back unlit with
    /// its base colour and texture where the emission was.
    #[test]
    fn unlit_round_trips_through_usd() {
        let s = unlit_scene();
        for (fmt, single) in EVERY_USD_FORM {
            let ctx = format!("{fmt}{}", if single { " single" } else { " package" });
            let opts = WriteOptions { self_contained: single, ..Default::default() };
            let w = write_scene(&s, fmt, opts).unwrap();
            assert!(w.losses.iter().all(|l| !l.what.contains("unlit")), "{ctx}: unlit is kept, not reported: {:#?}", w.losses);
            let r = read_scene(&w.package.files[0].bytes, fmt, &MemorySidecars { files: &w.package.files }).unwrap();
            assert!(r.warnings.is_empty(), "{ctx}: {:?}", r.warnings);
            let want = fmt.prepare(&s, opts).unwrap();
            for name in ["box_mat", "wedge_mat", "panel_mat"] {
                let (w, g) = (material_named(&want, name), material_named(&r.scene, name));
                let unlit = name != "panel_mat";
                assert_eq!((w.unlit, g.unlit), (unlit, unlit), "{ctx}: {name}: prepare keeps unlit, and the file reads back as it was");
                assert_eq!(g.base_color, w.base_color, "{ctx}: {name}: base colour");
                assert_eq!(sampled(&r.scene, &g.base_color_texture), sampled(&want, &w.base_color_texture), "{ctx}: {name}: base colour texture");
                assert_eq!((g.emissive, g.emissive_texture.is_none()), ([0.0; 3], true), "{ctx}: {name}: the emission is the base colour's, not an emissive");
                assert_eq!((g.metallic, g.roughness), (w.metallic, w.roughness), "{ctx}: {name}: metallic and roughness");
            }
            // An unlit material is metallic 1 and roughness 1, the recipe's;
            // the lit control keeps its own.
            assert_eq!((material_named(&r.scene, "box_mat").metallic, material_named(&r.scene, "box_mat").roughness), (1.0, 1.0), "{ctx}");
            assert_eq!((material_named(&r.scene, "panel_mat").metallic, material_named(&r.scene, "panel_mat").roughness), (0.0, 1.0), "{ctx}");
            assert_draws_alike(&want, &r.scene, &[], FULL, &ctx);
        }
        // The recipe, as written, in a package whose textures are there.
        let w = write_scene(&s, MeshFormat::Usda, WriteOptions { self_contained: false, ..Default::default() }).unwrap();
        let layer = &w.package.files[0].bytes;
        let text = String::from_utf8_lossy(layer);
        let has = |b: &[String], l: &str| b.iter().any(|x| x == l);
        let connects = |b: &[String], input: &str, source: &str| {
            b.iter().any(|l| l.starts_with(&format!("color3f inputs:{input}.connect = <")) && l.ends_with(&format!("/{source}>")))
        };
        let marked = |b: &[String]| b.windows(2).any(|p| p[0] == "dictionary litegen = {" && p[1] == "bool unlit = 1");
        let box_mat = material_block(layer, "box_mat");
        assert!(has(&box_mat, "color3f inputs:diffuseColor = (0, 0, 0)"), "the diffuse is black\n{text}");
        assert!(box_mat.iter().all(|l| !l.starts_with("color3f inputs:diffuseColor.connect")), "the texture is not the diffuse\n{text}");
        assert!(has(&box_mat, "float inputs:metallic = 1") && has(&box_mat, "float inputs:roughness = 1"), "{text}");
        assert!(connects(&box_mat, "emissiveColor", "BaseColorTex.outputs:rgb"), "the texture is the emission\n{text}");
        assert!(has(&box_mat, "float4 inputs:scale = (0.9, 0.7, 0.5, 1)"), "the tint is the texture's scale\n{text}");
        assert!(has(&box_mat, "token inputs:sourceColorSpace = \"sRGB\""), "{text}");
        assert!(marked(&box_mat), "the marker\n{text}");
        let wedge = material_block(layer, "wedge_mat");
        assert!(has(&wedge, "color3f inputs:diffuseColor = (0, 0, 0)") && has(&wedge, "color3f inputs:emissiveColor = (0.9, 0.3, 0.1)"), "{text}");
        assert!(has(&wedge, "float inputs:metallic = 1") && has(&wedge, "float inputs:roughness = 1") && marked(&wedge), "{text}");
        for block in [&box_mat, &wedge] {
            assert!(block.iter().all(|l| !l.contains("mtlx")), "no MaterialX output\n{text}");
        }
        let panel = material_block(layer, "panel_mat");
        assert!(panel.iter().all(|l| !l.contains("unlit") && !l.contains("litegen")), "a lit material has no marker\n{text}");
        assert!(has(&panel, "color3f inputs:diffuseColor = (1, 1, 1)") && has(&panel, "float inputs:metallic = 0"), "{text}");
    }

    /// The recipe alone is not unlit: a black diffuse with an emission is a
    /// real design (a lamp, a screen), so only the marker reads back as
    /// unlit. The marker is `true` or nothing: a `false` one reads lit too.
    #[test]
    fn a_black_diffuse_emissive_material_without_the_marker_reads_lit() {
        let s = unlit_scene();
        for fmt in [MeshFormat::Usda, MeshFormat::Usdz] {
            let w = write_scene(&s, fmt, WriteOptions { self_contained: false, ..Default::default() }).unwrap();
            let layer = match fmt {
                MeshFormat::Usdz => String::from_utf8(zip::Archive::parse(&w.package.files[0].bytes).and_then(|a| a.read(&a.entries[0])).unwrap()).unwrap(),
                _ => String::from_utf8(w.package.files[0].bytes.clone()).unwrap(),
            };
            assert!(layer.contains("bool unlit = 1"), "{fmt}: the marker is written");
            let stripped = without_litegen_data(&layer);
            let false_marker = layer.replace("bool unlit = 1", "bool unlit = 0");
            for (how, text) in [("no marker", stripped), ("a false marker", false_marker)] {
                let ctx = format!("{fmt}, {how}");
                let files = with_root_layer(fmt, &w.package.files, &text);
                let r = read_scene(&files[0].bytes, fmt, &MemorySidecars { files: &files }).unwrap();
                assert!(r.warnings.is_empty(), "{ctx}: {:?}", r.warnings);
                for name in ["box_mat", "wedge_mat"] {
                    let m = material_named(&r.scene, name);
                    assert!(!m.unlit, "{ctx}: {name} reads lit");
                    assert_eq!(m.base_color[..3], [0.0, 0.0, 0.0], "{ctx}: {name} keeps its black diffuse");
                    assert_eq!((m.metallic, m.roughness), (1.0, 1.0), "{ctx}: {name}");
                }
                let wedge = material_named(&r.scene, "wedge_mat");
                assert_eq!(wedge.emissive, [0.9, 0.3, 0.1], "{ctx}: the wedge is an emissive material");
                let b = material_named(&r.scene, "box_mat");
                assert_eq!(b.emissive, [0.9, 0.7, 0.5], "{ctx}: the box's emissive tint");
                assert!(b.emissive_texture.is_some() && b.base_color_texture.is_none(), "{ctx}: the box's texture is its emissive map");
            }
        }
    }

    /// An unlit material keeps its opacity wired as a lit material's is (a
    /// constant, a base-colour texture's alpha, a vertex colour's alpha) and
    /// its vertex colours, which multiply through the `displayColor` reader
    /// that is now the emission's source (Review Focus 5).
    #[test]
    fn unlit_keeps_alpha_and_vertex_colour() {
        let unlit = |name: &str, base: [f32; 4], alpha_mode: AlphaMode, texture: bool| Material {
            name: Some(name.into()),
            base_color: base,
            base_color_texture: texture.then_some(super::super::scene::TextureRef { texture: 0, uv_set: 0, transform: None }),
            alpha_mode,
            unlit: true,
            ..Default::default()
        };
        let mut s = quads(
            vec![
                unlit("blend_flat", [0.6, 0.4, 0.2, 0.5], AlphaMode::Blend, false),
                unlit("alpha_map", [0.8, 0.8, 0.8, 0.5], AlphaMode::Blend, true),
                unlit("coloured", [0.5, 0.5, 1.0, 1.0], AlphaMode::Opaque, false),
                unlit("coloured_blend", [1.0, 0.5, 1.0, 0.5], AlphaMode::Blend, false),
            ],
            1,
        );
        s.meshes[2].primitives[0].colors = Some(vec![[1.0, 0.5, 0.25, 1.0], [0.25, 1.0, 0.5, 1.0], [0.5, 0.25, 1.0, 1.0], [1.0, 1.0, 0.0, 1.0]]);
        s.meshes[3].primitives[0].colors = Some(vec![[1.0, 0.5, 0.25, 1.0], [0.25, 1.0, 0.5, 0.5], [0.5, 0.25, 1.0, 0.75], [1.0, 1.0, 0.0, 0.25]]);
        for (fmt, single) in [(MeshFormat::Usdz, false), (MeshFormat::Usda, false), (MeshFormat::Usdc, false)] {
            let ctx = format!("{fmt}{}", if single { " single" } else { " package" });
            let opts = WriteOptions { self_contained: single, ..Default::default() };
            let w = write_scene(&s, fmt, opts).unwrap();
            assert!(w.losses.iter().all(|l| !l.what.contains("unlit")), "{ctx}: {:#?}", w.losses);
            let r = read_scene(&w.package.files[0].bytes, fmt, &MemorySidecars { files: &w.package.files }).unwrap();
            assert!(r.warnings.is_empty(), "{ctx}: {:?}", r.warnings);
            let want = fmt.prepare(&s, opts).unwrap();
            for name in ["blend_flat", "alpha_map", "coloured", "coloured_blend"] {
                let (w, g) = (material_named(&want, name), material_named(&r.scene, name));
                assert!(g.unlit && w.unlit, "{ctx}: {name} is unlit");
                assert_eq!(g.alpha_mode, w.alpha_mode, "{ctx}: {name}: alpha mode");
            }
            assert_eq!(material_named(&r.scene, "blend_flat").base_color, [0.6, 0.4, 0.2, 0.5], "{ctx}: a constant opacity");
            let map = material_named(&r.scene, "alpha_map");
            assert_eq!((map.base_color, sampled(&r.scene, &map.base_color_texture)), ([0.8, 0.8, 0.8, 0.5], Some(("quadrants", 0))), "{ctx}: the alpha map");
            // The base colour rides in the vertex colours, in USD: compare what
            // each vertex shows, the colour times the material's base.
            let shown = |s: &Scene, name: &str| -> Vec<[f32; 4]> {
                let b = material_named(s, name).base_color;
                prim_named(s, name).colors.as_ref().unwrap_or_else(|| panic!("{ctx}: {name} has no vertex colours")).iter().map(|c| [c[0] * b[0], c[1] * b[1], c[2] * b[2], c[3] * b[3]]).collect()
            };
            for name in ["coloured", "coloured_blend"] {
                let (a, b) = (shown(&want, name), shown(&r.scene, name));
                assert!(a.iter().zip(&b).all(|(p, q)| (0..4).all(|k| (p[k] - q[k]).abs() < 1e-6)) && a.len() == b.len(), "{ctx}: {name}: {a:?} vs {b:?}");
            }
            assert_draws_alike(&want, &r.scene, &[], Check { colors: false, ..FULL }, &ctx);
        }
        let w = write_scene(&s, MeshFormat::Usda, WriteOptions { self_contained: false, ..Default::default() }).unwrap();
        let layer = &w.package.files[0].bytes;
        let text = String::from_utf8_lossy(layer);
        let has = |b: &[String], l: &str| b.iter().any(|x| x == l);
        let connects = |b: &[String], ty: &str, input: &str, source: &str| {
            b.iter().any(|l| l.starts_with(&format!("{ty} inputs:{input}.connect = <")) && l.ends_with(&format!("/{source}>")))
        };
        let flat = material_block(layer, "blend_flat");
        assert!(has(&flat, "float inputs:opacity = 0.5") && has(&flat, "color3f inputs:emissiveColor = (0.6, 0.4, 0.2)"), "{text}");
        let map = material_block(layer, "alpha_map");
        assert!(connects(&map, "float", "opacity", "BaseColorTex.outputs:a") && has(&map, "float outputs:a"), "the alpha stays connected\n{text}");
        assert!(connects(&map, "color3f", "emissiveColor", "BaseColorTex.outputs:rgb"), "{text}");
        assert!(has(&map, "float4 inputs:scale = (0.8, 0.8, 0.8, 0.5)"), "{text}");
        let coloured = material_block(layer, "coloured");
        assert!(connects(&coloured, "color3f", "emissiveColor", "VertexColor.outputs:result"), "the vertex colour is the emission\n{text}");
        assert!(has(&coloured, "color3f inputs:emissiveColor = (0.5, 0.5, 1)"), "with the base colour beside it for readers that cannot follow a primvar\n{text}");
        assert!(has(&coloured, "color3f inputs:diffuseColor = (0, 0, 0)"), "{text}");
        assert!(coloured.iter().all(|l| !l.starts_with("color3f inputs:diffuseColor.connect")), "{text}");
        let blend = material_block(layer, "coloured_blend");
        assert!(connects(&blend, "float", "opacity", "VertexOpacity.outputs:result"), "the vertex alpha stays connected\n{text}");
    }

    /// An unlit material is its base colour and opacity and nothing else:
    /// glTF's KHR_materials_unlit says lighting-only properties are ignored.
    /// So its metallic-roughness map, normal map, occlusion, emissive,
    /// clearcoat and ior are not written (the recipe's own metallic, roughness
    /// and emission would be overridden by them), the maps only they used go
    /// out of the file, and none of it is reported: nothing is lost that
    /// anything draws.
    #[test]
    fn an_unlit_material_keeps_only_its_base() {
        use super::super::scene::{Clearcoat, TextureRef};
        let map = |texture: usize| Some(TextureRef { texture, uv_set: 0, transform: None });
        let busy = Material {
            name: Some("busy".into()),
            base_color: [0.9, 0.8, 0.7, 1.0],
            base_color_texture: map(0),
            metallic: 0.3,
            roughness: 0.2,
            metallic_roughness_texture: map(1),
            normal_texture: map(1),
            normal_scale: 2.0,
            occlusion_texture: map(1),
            occlusion_strength: 0.5,
            emissive: [0.1, 0.2, 0.3],
            emissive_texture: map(1),
            // A coat over another ior, and over a normal map that is not the
            // base's: a lit material would be reported for each.
            clearcoat: Some(Clearcoat { factor: 0.8, roughness: 0.1, normal_texture: map(1), normal_scale: 0.5, ..Default::default() }),
            ior: Some(1.2),
            unlit: true,
            ..Default::default()
        };
        let s = quads(vec![busy], 1);
        let w = write_scene(&s, MeshFormat::Usda, WriteOptions { self_contained: false, ..Default::default() }).unwrap();
        assert!(w.losses.is_empty(), "nothing an unlit material ignores is reported: {:#?}", w.losses);
        let layer = &w.package.files[0].bytes;
        let text = String::from_utf8_lossy(layer);
        for shader in ["MetallicRoughnessTex", "NormalTex", "OcclusionTex", "EmissiveTex", "ClearcoatTex", "ClearcoatRoughnessTex"] {
            assert!(!text.contains(shader), "{shader} is written for an unlit material\n{text}");
        }
        let block = material_block(layer, "busy");
        for input in ["normal", "occlusion", "clearcoat", "clearcoatRoughness", "ior"] {
            assert!(block.iter().all(|l| !l.contains(&format!("inputs:{input}"))), "{input} is written for an unlit material\n{text}");
        }
        assert!(block.iter().any(|l| l == "float inputs:metallic = 1") && block.iter().any(|l| l == "float inputs:roughness = 1"), "{text}");
        assert!(w.package.files.iter().all(|f| f.path != "textures/bands.png"), "the image only the lit layers sampled is not packaged: {:?}", w.package.files.iter().map(|f| &f.path).collect::<Vec<_>>());
        let r = read_scene(layer, MeshFormat::Usda, &MemorySidecars { files: &w.package.files }).unwrap();
        let m = material_named(&r.scene, "busy");
        assert!(m.unlit && m.clearcoat.is_none() && m.ior.is_none() && m.normal_texture.is_none() && m.occlusion_texture.is_none(), "{m:?}");
        assert_eq!((m.emissive, m.emissive_texture.is_none(), m.metallic_roughness_texture.is_none()), ([0.0; 3], true, true), "{m:?}");
        assert_eq!((m.metallic, m.roughness, m.base_color), (1.0, 1.0, [0.9, 0.8, 0.7, 1.0]), "{m:?}");
        assert_eq!(sampled(&r.scene, &m.base_color_texture), Some(("quadrants", 0)), "{m:?}");
        assert_eq!(r.scene.images.len(), 1, "one image is left");
    }

    /// The plan's Review Focus 5 asks for an ior that KHR_materials_ior's
    /// schema allows (0, or 1 and above) but UsdPreviewSurface cannot express,
    /// to be left out of USD and reported. For the base layer there is none.
    /// The schema's 0 means "The effective IOR becomes positive infinity and
    /// the Fresnel term MUST evaluate to 1.0", and in UsdPreviewSurface's
    /// metalness workflow "F0 is derived from *ior* via ((1-ior)/(1+ior))^2"
    /// (docs/spec_usdpreviewsurface.rst:132-133 in OpenUSD), which is 1 at 0.
    /// three.js r183 agrees: its USD loader takes `inputs:ior` as written
    /// (USDComposer.js:2626-2628), as its glTF loader takes
    /// KHR_materials_ior's (GLTFLoader.js:1270), and its shader's F0 is
    /// `pow2( ( material.ior - 1.0 ) / ( material.ior + 1.0 ) )`
    /// (lights_physical_fragment.glsl.js:45). So on a material without a
    /// clearcoat, 0, like the boundary 1, is written as it is, reads back, and
    /// is no loss. A clearcoat over an ior other than 1.5 is the case that
    /// remains (`a_clearcoat_over_an_ior_other_than_1_5_is_reported`).
    #[test]
    fn an_ior_of_zero_round_trips_through_usd() {
        let ior = |name: &str, ior: f32| Material { name: Some(name.into()), metallic: 0.0, ior: Some(ior), ..Default::default() };
        let s = quads(vec![ior("special", 0.0), ior("boundary", 1.0)], 1);
        for (fmt, single) in [(MeshFormat::Usdz, false), (MeshFormat::Usda, false), (MeshFormat::Usda, true)] {
            let ctx = format!("{fmt}{}", if single { " single" } else { "" });
            let opts = WriteOptions { self_contained: single, ..Default::default() };
            let w = write_scene(&s, fmt, opts).unwrap();
            assert!(w.losses.is_empty(), "{ctx}: {:?}", w.losses);
            let prepared = fmt.prepare(&s, opts).unwrap();
            let r = read_scene(&w.package.files[0].bytes, fmt, &MemorySidecars { files: &w.package.files }).unwrap();
            assert!(r.warnings.is_empty(), "{ctx}: {:?}", r.warnings);
            for (label, scene) in [("prepared", &prepared), ("read back", &r.scene)] {
                let iors = (material_named(scene, "special").ior, material_named(scene, "boundary").ior);
                assert_eq!(iors, (Some(0.0), Some(1.0)), "{ctx}: {label}");
            }
            if fmt == MeshFormat::Usda {
                let iors: Vec<String> = layer_lines(&w.package.files[0].bytes).into_iter().filter(|l| l.starts_with("float inputs:ior ")).collect();
                assert_eq!(iors, ["float inputs:ior = 0", "float inputs:ior = 1"], "{ctx}");
            }
        }
    }

    /// UsdPreviewSurface's ior is also its clearcoat's: "Index of Refraction
    /// to be used for translucent objects and objects with specular
    /// components, including the clearcoat if clearcoat > 0" (its spec), and
    /// Storm takes the coat's F0 from it (previewSurface.glslfx:213,
    /// :241-246). glTF's clearcoat keeps 1.5: "The index of refraction of
    /// clearcoat and base layer do not influence each other"
    /// (KHR_materials_clearcoat). So a clearcoat over an ior other than 1.5
    /// reflects otherwise in USD, and is named once per write: over 0 and
    /// 1.45 it is; over 1.5 or an unset ior, or at a clearcoat of 0, it is
    /// not. Each other ior is written all the same, for its base layer, and
    /// the reader warns about the two coats it reads back. The ior of 1.5
    /// and the unmapped clearcoat of 0 are left out, and read back unset
    /// (Ruling A41).
    #[test]
    fn a_clearcoat_over_an_ior_other_than_1_5_is_reported() {
        use super::super::scene::Clearcoat;
        let cases = [("zero", 1.0, Some(0.0)), ("low", 1.0, Some(1.45)), ("glass", 1.0, Some(1.5)), ("unset", 1.0, None), ("uncoated", 0.0, Some(1.45))];
        let back = [("zero", Some(1.0), Some(0.0)), ("low", Some(1.0), Some(1.45)), ("glass", Some(1.0), None), ("unset", Some(1.0), None), ("uncoated", None, Some(1.45))];
        let coat = |&(name, factor, ior): &(&str, f32, Option<f32>)| Material {
            name: Some(name.into()),
            clearcoat: Some(Clearcoat { factor, ..Default::default() }),
            ior,
            ..Default::default()
        };
        let s = quads(cases.iter().map(coat).collect(), 1);
        for (fmt, single) in [(MeshFormat::Usdz, false), (MeshFormat::Usda, false), (MeshFormat::Usda, true)] {
            let ctx = format!("{fmt}{}", if single { " single" } else { "" });
            let w = write_scene(&s, fmt, WriteOptions { self_contained: single, ..Default::default() }).unwrap();
            let reported: Vec<(LossKind, &str)> = w.losses.iter().map(|l| (l.kind, l.what.as_str())).collect();
            assert_eq!(reported, [(LossKind::Materials, "a clearcoat with an ior other than 1.5 on 2 material(s)")], "{ctx}");
            let r = read_scene(&w.package.files[0].bytes, fmt, &MemorySidecars { files: &w.package.files }).unwrap();
            for (name, factor, ior) in back {
                let m = material_named(&r.scene, name);
                assert_eq!((m.clearcoat.as_ref().map(|c| c.factor), m.ior), (factor, ior), "{ctx}: {name}");
            }
            assert_eq!(r.warnings.len(), 2, "{ctx}: {:?}", r.warnings);
            for ior in ["0", "1.45"] {
                assert!(r.warnings.iter().any(|w| w.contains(&format!("a clearcoat and an ior of {ior}:"))), "{ctx}: {ior}: {:?}", r.warnings);
            }
        }
    }

    /// three.js r183's USDZExporter writes `clearcoat`, `clearcoatRoughness`
    /// and `ior` on every MeshPhysicalMaterial
    /// (scripts/mesh-verify/node_modules/three/examples/jsm/exporters/USDZExporter.js:1063-1118),
    /// which its GLTFLoader builds for any glTF material with a physical
    /// extension. At MeshPhysicalMaterial's defaults they are 0, 0 and 1.5
    /// (src/materials/MeshPhysicalMaterial.js:316, :100 and :134). An
    /// unconnected clearcoat of 0 and an ior of 1.5 draw as their absence
    /// does, so they read as no layer and no ior, silently, as OBJ's, FBX's
    /// and Collada's exporter defaults do (Ruling A41). An FBX of such a
    /// material, otherwise plain, then names no materials loss at all: no
    /// "clearcoat on", no "ior on".
    #[test]
    fn threejs_physical_defaults_read_as_no_layers() {
        let body = format!(
            r#"def Mesh "Mesh_1" (prepend apiSchemas = ["MaterialBindingAPI"]) {{ {QUAD}
                    rel material:binding = </R/Materials/Material_7> }}
                def Scope "Materials" {{
                    def Material "Material_7" {{
                        def Shader "PreviewSurface" {{
                            uniform token info:id = "UsdPreviewSurface"
                            color3f inputs:diffuseColor = (0.8, 0.2, 0.2)
                            float inputs:roughness = 0.5
                            float inputs:metallic = 0
                            float inputs:opacity = 1
                            float inputs:clearcoat = 0
                            float inputs:clearcoatRoughness = 0
                            float inputs:ior = 1.5
                            int inputs:useSpecularWorkflow = 0
                            token outputs:surface
                        }}
                        token outputs:surface.connect = </R/Materials/Material_7/PreviewSurface.outputs:surface>
                    }}
                }}"#
        );
        let r = read_text(&body, &[]);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let m = &r.scene.materials[0];
        assert_eq!((m.clearcoat.is_some(), m.ior), (false, None), "three.js's defaults read as layers: {m:?}");
        let w = write_scene(&r.scene, MeshFormat::Fbx, WriteOptions::default()).unwrap();
        let named: Vec<&str> = w.losses.iter().filter(|l| l.kind == LossKind::Materials).map(|l| l.what.as_str()).collect();
        assert!(named.is_empty(), "an FBX of three.js's defaults names {named:?}");
    }

    /// What reads back unset is prepared unset (Ruling A41): an unmapped
    /// clearcoat of 0 goes, its roughness and maps with it (its own normal
    /// map named in no loss), and so does an ior of 1.5, both silently, as
    /// `obj::prepare` drops a layer at
    /// its off value and `dae::reduce_material` an ior of 1.5. A mapped
    /// clearcoat of 0 stays, map and all, where the map survives: only its
    /// map says where it is 0. A single USDA keeps no map, so there it goes
    /// too. The prepared scene is what reads back, in every USD form.
    #[test]
    fn usd_prepares_its_defaults_as_it_reads_them() {
        use super::super::scene::{Clearcoat, TextureRef};
        let map = |texture: usize| Some(TextureRef { texture, uv_set: 0, transform: None });
        let zero = Material {
            name: Some("zero".into()),
            clearcoat: Some(Clearcoat { factor: 0.0, roughness: 0.4, roughness_texture: map(1), normal_texture: map(1), ..Default::default() }),
            ..Default::default()
        };
        let mapped = Material { name: Some("mapped".into()), clearcoat: Some(Clearcoat { factor: 0.0, texture: map(0), ..Default::default() }), ..Default::default() };
        let glass = Material { name: Some("glass".into()), ior: Some(1.5), ..Default::default() };
        let s = quads(vec![zero, mapped, glass], 1);
        for (fmt, single) in [(MeshFormat::Usdz, false), (MeshFormat::Usda, false), (MeshFormat::Usda, true)] {
            let ctx = format!("{fmt}{}", if single { " single" } else { "" });
            let opts = WriteOptions { self_contained: single, ..Default::default() };
            let prepared = fmt.prepare(&s, opts).unwrap();
            let w = write_scene(&s, fmt, opts).unwrap();
            let layer_losses: Vec<&str> = w.losses.iter().filter(|l| l.kind == LossKind::Materials).map(|l| l.what.as_str()).collect();
            assert!(layer_losses.is_empty(), "{ctx}: {layer_losses:?}");
            let r = read_scene(&w.package.files[0].bytes, fmt, &MemorySidecars { files: &w.package.files }).unwrap();
            assert!(r.warnings.is_empty(), "{ctx}: {:?}", r.warnings);
            for (label, scene) in [("prepared", &prepared), ("read back", &r.scene)] {
                assert_eq!(material_named(scene, "zero").clearcoat, None, "{ctx}: {label}: the unmapped clearcoat of 0");
                assert_eq!(material_named(scene, "glass").ior, None, "{ctx}: {label}: the ior of 1.5");
                let kept = material_named(scene, "mapped").clearcoat.as_ref().map(|c| (c.factor, sampled(scene, &c.texture)));
                assert_eq!(kept, (!single).then_some((0.0, Some(("quadrants", 0)))), "{ctx}: {label}: the mapped clearcoat of 0");
            }
            assert_eq!(prepared.images.len(), r.scene.images.len(), "{ctx}: the images");
        }
    }

    /// The spec's site test for the uv-set plan: base colour on set 0, a
    /// clearcoat map on set 2 and a sheen colour map on set 1. USD keeps the
    /// clearcoat, so a USDZ keeps sets 0 and 2, with the clearcoat binding on
    /// set 1. USD has no sheen layer: its map claims no set, and it is
    /// reported once, with the layer.
    #[test]
    fn a_layer_texture_keeps_its_uv_set_in_usdz() {
        use super::super::scene::{Clearcoat, Sheen, TextureRef};
        let map = |texture: usize, uv_set: u32| Some(TextureRef { texture, uv_set, transform: None });
        let m = Material {
            name: Some("coated_velvet".into()),
            base_color_texture: map(0, 0),
            clearcoat: Some(Clearcoat { factor: 1.0, texture: map(1, 2), ..Default::default() }),
            sheen: Some(Sheen { color: [1.0; 3], color_texture: map(1, 1), ..Default::default() }),
            ..Default::default()
        };
        let s = quads(vec![m], 3);
        let w = write_scene(&s, MeshFormat::Usdz, WriteOptions::default()).unwrap();
        let reported: Vec<(LossKind, &str)> = w.losses.iter().map(|l| (l.kind, l.what.as_str())).collect();
        assert_eq!(reported, [(LossKind::Uvs, "1 of 3 uv set(s)"), (LossKind::Materials, "sheen on 1 material(s)")], "{:#?}", w.losses);
        let prepared = MeshFormat::Usdz.prepare(&s, WriteOptions::default()).unwrap();
        let r = read_scene(&w.package.files[0].bytes, MeshFormat::Usdz, &NoSidecars).unwrap();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        for (label, scene) in [("prepared", &prepared), ("read back", &r.scene)] {
            let p = &scene.meshes[0].primitives[0];
            assert_sets(p, &s.meshes[0].primitives[0], &[0, 2], 1e-7, label);
            assert_eq!(bindings_of(scene, p), [("baseColor", 0), ("clearcoat", 1)], "{label}");
        }
    }

    /// UsdPreviewSurface's spec: "Clearcoat results are calculated using the
    /// same normal map data used by the primary specular lobe." So in USD a
    /// clearcoat's normal is the base normal map, at the base's scale. A coat
    /// whose own normal differs is named, once per material: one with another
    /// map than the base's (`own_map`, whose map on set 2 then claims no USDZ
    /// set), one with none over a mapped base (`plain`), and one with a map
    /// over an unmapped base (`bare`); one that shares the base's (`shared`)
    /// is not.
    #[test]
    fn a_clearcoat_takes_the_base_normal_map_in_usd() {
        use super::super::scene::{Clearcoat, TextureRef};
        let map = |texture: usize, uv_set: u32| Some(TextureRef { texture, uv_set, transform: None });
        let coat = |name: &str, base: Option<TextureRef>, normal_texture: Option<TextureRef>, normal_scale: f32| Material {
            name: Some(name.into()),
            normal_texture: base,
            normal_scale: 0.5,
            clearcoat: Some(Clearcoat { factor: 1.0, normal_texture, normal_scale, ..Default::default() }),
            ..Default::default()
        };
        let s = quads(
            vec![
                coat("own_map", map(0, 0), map(1, 2), 0.75),
                coat("plain", map(0, 0), None, 1.0),
                coat("shared", map(0, 0), map(0, 0), 0.5),
                coat("bare", None, map(1, 0), 0.75),
            ],
            3,
        );
        for fmt in [MeshFormat::Usdz, MeshFormat::Usda] {
            let opts = WriteOptions { self_contained: false, ..Default::default() };
            let w = write_scene(&s, fmt, opts).unwrap();
            let reported: Vec<(LossKind, &str)> = w.losses.iter().map(|l| (l.kind, l.what.as_str())).collect();
            let mut want = vec![(LossKind::Materials, "clearcoat normals of 3 material(s)")];
            if fmt == MeshFormat::Usdz {
                want.insert(0, (LossKind::Uvs, "1 of 3 uv set(s)"));
            }
            assert_eq!(reported, want, "{fmt}");
            let prepared = fmt.prepare(&s, opts).unwrap();
            let r = read_scene(&w.package.files[0].bytes, fmt, &MemorySidecars { files: &w.package.files }).unwrap();
            assert!(r.warnings.is_empty(), "{fmt}: {:?}", r.warnings);
            let images: Vec<&str> = r.scene.images.iter().map(|i| which_image(&i.data)).collect();
            assert_eq!(images, ["quadrants"], "{fmt}: the coats' own maps are not shipped");
            for (label, scene) in [("prepared", &prepared), ("read back", &r.scene)] {
                for (name, base) in [("own_map", true), ("plain", true), ("shared", true), ("bare", false)] {
                    let m = material_named(scene, name);
                    let c = m.clearcoat.as_ref().unwrap_or_else(|| panic!("{fmt} {label}: {name} has no clearcoat"));
                    let want = base.then_some(("quadrants", 0));
                    assert_eq!((sampled(scene, &c.normal_texture), c.normal_scale), (want, if base { 0.5 } else { 1.0 }), "{fmt} {label}: {name}");
                    assert_eq!(c.normal_texture, m.normal_texture, "{fmt} {label}: {name}'s coat and base normals");
                }
                if fmt == MeshFormat::Usdz {
                    assert_sets(prim_named(scene, "own_map"), &s.meshes[0].primitives[0], &[0, 1], 1e-7, &format!("{fmt} {label}: own_map"));
                }
            }
        }
    }

    // ── hostile input ───────────────────────────────────────────────────────

    #[test]
    fn never_panics_on_truncated_or_corrupted_files() {
        let ours = write_scene(&fixtures::skinned(), MeshFormat::Usdz, WriteOptions::default()).unwrap().package.files[0].bytes.clone();
        let text = write_scene(&fixtures::morph(), MeshFormat::Usda, WriteOptions::default()).unwrap().package.files[0].bytes.clone();
        let cases: [(&str, &[u8], MeshFormat); 6] = [
            ("our usdz", &ours, MeshFormat::Usdz),
            ("our usda", &text, MeshFormat::Usda),
            ("stress crate", STRESS_USDC, MeshFormat::Usda),
            ("skinned crate", SKINNED_USDC, MeshFormat::Usda),
            ("composition crate", COMP_USDC, MeshFormat::Usda),
            ("apple usdz", TEXTURED_ARKIT, MeshFormat::Usdz),
        ];
        for (what, bytes, fmt) in cases {
            // Every byte of the small crate files (where the offsets, counts
            // and tables are dense); elsewhere a stride that still lands in
            // every header, table and count.
            let step = if bytes.len() <= 4096 { 1 } else { bytes.len() / 1000 };
            for cut in (0..bytes.len()).step_by(step) {
                let _ = read_scene(&bytes[..cut], fmt, &NoSidecars);
            }
            for i in (0..bytes.len()).step_by(step) {
                for flip in [0x01u8, 0x80, 0xFF] {
                    let mut v = bytes.to_vec();
                    v[i] ^= flip;
                    let r = read_scene(&v, fmt, &NoSidecars);
                    if let Ok(r) = r {
                        r.scene.validate().unwrap_or_else(|e| panic!("{what}: a corrupted file read as an invalid scene: {e}"));
                    }
                }
            }
        }
    }

    /// The USD files more than two uv sets are read from, multiuv as this
    /// module writes it and `testdata/usd/multiuv.usdc` (the crate `usdcat`
    /// made of that), through the reader: each cut at every length, with each
    /// bit of its first 4 KiB flipped in turn, and with 2–8 bits flipped at
    /// random in 10,000 seeded cases. Every reading is `Ok` or `Err`, never a
    /// panic. dae.rs's test of the same name runs the Collada input, and
    /// ply.rs's `big_endian_truncation_and_bit_flips_never_panic` is the
    /// pattern.
    #[test]
    fn uv_set_inputs_survive_truncation_and_bit_flips() {
        const FLIP_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
        let package = WriteOptions { self_contained: false, ..Default::default() };
        let usda = write_scene(&fixtures::multiuv(), MeshFormat::Usda, package).unwrap().package.files;
        let side = &MemorySidecars { files: &usda };
        for (name, file) in [("multiuv as USDA", &usda[0].bytes[..]), ("testdata/usd/multiuv.usdc", MULTIUV_USDC)] {
            let whole = read_scene(file, MeshFormat::Usda, side).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(whole.scene.meshes[0].primitives[0].uvs.len(), 4, "{name} itself carries the four sets");
            let survives = move |bytes: &[u8]| std::panic::catch_unwind(move || read_scene(bytes, MeshFormat::Usda, side)).is_ok();
            for cut in 0..file.len() {
                assert!(survives(&file[..cut]), "{name}: cut to {cut} bytes");
            }
            let mut flipped = file.to_vec();
            for bit in 0..file.len().min(4096) * 8 {
                flipped[bit / 8] ^= 1 << (bit % 8);
                assert!(survives(&flipped), "{name}: bit {bit} flipped");
                flipped[bit / 8] ^= 1 << (bit % 8);
            }
            // xorshift64 (Marsaglia, 2003): std only, and the same cases every
            // run, seeded again for each input.
            let mut state = FLIP_SEED;
            let mut next = move || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            };
            let bits = file.len() as u64 * 8;
            for case in 0..10_000 {
                let flips = 2 + next() % 7;
                let mut chosen: Vec<u64> = Vec::new();
                while (chosen.len() as u64) < flips {
                    let bit = next() % bits;
                    if !chosen.contains(&bit) {
                        chosen.push(bit);
                    }
                }
                let mut flipped = file.to_vec();
                for &bit in &chosen {
                    flipped[(bit / 8) as usize] ^= 1 << (bit % 8);
                }
                assert!(survives(&flipped), "{name}, case {case}: bits {chosen:?} flipped");
            }
        }
    }

    /// `scene`'s USDA package and its USDZ, each read whole, where it must
    /// carry two clearcoats (the car paint's from maps) and three iors, then
    /// swept by [`layer_sweep`]. The USDA's single flips also take every bit
    /// of its `Materials` scope, which in full-size layered runs past the
    /// first 4 KiB into the lacquer's clearcoat and ior. Every reading is
    /// `Ok` or `Err`. A USDZ's entries are stored and CRC-checked, so a flip
    /// inside one is refused before any layer is read, and so is every cut,
    /// which leaves no end-of-central-directory record.
    fn usd_layer_sweeps(label: &str, scene: &Scene) {
        use super::super::scene::MaterialLayer;
        let package = WriteOptions { self_contained: false, ..Default::default() };
        let usda = write_scene(scene, MeshFormat::Usda, package).unwrap().package.files;
        let usdz = write_scene(scene, MeshFormat::Usdz, WriteOptions::default()).unwrap().package.files;
        let beside = MemorySidecars { files: &usda };
        for (form, file, fmt) in [("USDA", &usda[0].bytes, MeshFormat::Usda), ("USDZ", &usdz[0].bytes, MeshFormat::Usdz)] {
            let name = &format!("{label} as {form}");
            let side: &dyn Sidecars = if fmt == MeshFormat::Usda { &beside } else { &NoSidecars };
            let whole = read_scene(file, fmt, side).unwrap_or_else(|e| panic!("{name}: {e}"));
            let has = |layer| whole.scene.materials.iter().filter(|m| m.has_layer(layer)).count();
            assert_eq!((has(MaterialLayer::Clearcoat), has(MaterialLayer::Ior)), (2, 3), "{name} itself carries two clearcoats and three iors");
            let coat = &material_named(&whole.scene, "car_paint").clearcoat;
            assert!(coat.as_ref().is_some_and(|c| c.texture.is_some() && c.roughness_texture.is_some()), "{name}: the car paint's coat maps: {coat:?}");
            let layers = if fmt == MeshFormat::Usda { span(name, file, r#"def Scope "Materials""#, "def Mesh") } else { 0..0 };
            layer_sweep(name, file, layers, |bytes| {
                let _ = read_scene(bytes, fmt, side);
            });
        }
    }

    /// USD's clearcoat, clearcoat roughness and ior, with the coat maps'
    /// channels and scale, through the reader ([`usd_layer_sweeps`]), over
    /// the four of layered's materials USD writes them for, lean
    /// ([`lean_layered`]): the car paint (a clearcoat from maps), the glass
    /// and the plastic (iors) and the lacquer (a clearcoat over an ior). The
    /// other three carry only layers the USD writer drops. Then what the
    /// writer never writes (Ruling A40c): a one-triangle USDA whose coat map
    /// scales its R and G and biases its G, on a surface with a normal map,
    /// read whole (the clearcoat comes from R at its scale, the biased G is
    /// left out with a warning, and the coat takes the base normal map) and
    /// swept by [`layer_sweep`].
    #[test]
    fn usd_layer_inputs_survive_truncation_and_bit_flips() {
        usd_layer_sweeps("lean layered", &lean_layered(&["car_paint", "glass", "plastic", "lacquer"]));
        let name = "a USDA whose coat map is scaled and biased, over a normal map";
        let pixel = fixtures::encode_png(1, 1, &[128, 128, 255, 255]);
        let files = [PackageFile { path: "coat.png".into(), bytes: pixel.clone() }, PackageFile { path: "normal.png".into(), bytes: pixel }];
        let side = MemorySidecars { files: &files };
        let text = usda(
            r#"def Mesh "M" (prepend apiSchemas = ["MaterialBindingAPI"]) {
                int[] faceVertexCounts = [3]
                int[] faceVertexIndices = [0, 1, 2]
                point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
                texCoord2f[] primvars:st = [(0, 0), (1, 0), (0, 1)] (interpolation = "vertex")
                rel material:binding = </R/Mat> }
            def Material "Mat" {
                token outputs:surface.connect = </R/Mat/PS.outputs:surface>
                def Shader "PS" {
                    uniform token info:id = "UsdPreviewSurface"
                    normal3f inputs:normal.connect = </R/Mat/N.outputs:rgb>
                    float inputs:clearcoat.connect = </R/Mat/Coat.outputs:r>
                    float inputs:clearcoatRoughness.connect = </R/Mat/Coat.outputs:g>
                    token outputs:surface }
                def Shader "St" {
                    uniform token info:id = "UsdPrimvarReader_float2"
                    string inputs:varname = "st"
                    float2 outputs:result }
                def Shader "Coat" {
                    uniform token info:id = "UsdUVTexture"
                    asset inputs:file = @coat.png@
                    float2 inputs:st.connect = </R/Mat/St.outputs:result>
                    token inputs:sourceColorSpace = "raw"
                    float4 inputs:scale = (0.8, 0.5, 1, 1)
                    float4 inputs:bias = (0, 0.25, 0, 0)
                    float outputs:r
                    float outputs:g }
                def Shader "N" {
                    uniform token info:id = "UsdUVTexture"
                    asset inputs:file = @normal.png@
                    float2 inputs:st.connect = </R/Mat/St.outputs:result>
                    token inputs:sourceColorSpace = "raw"
                    float4 inputs:scale = (1, 1, 2, 1)
                    float4 inputs:bias = (-0.5, -0.5, -1, 0)
                    float3 outputs:rgb }
            }"#,
        );
        let whole = read_scene(text.as_bytes(), MeshFormat::Usda, &side).unwrap_or_else(|e| panic!("{name}: {e}"));
        let m = &whole.scene.materials[0];
        assert!(m.normal_texture.is_some() && m.normal_scale == 0.5, "{name}: the base normal map");
        let c = m.clearcoat.as_ref().unwrap_or_else(|| panic!("{name}: no clearcoat"));
        assert_eq!((c.factor, c.texture.is_some(), c.roughness, &c.roughness_texture), (0.8, true, 0.01, &None), "{name}: R at its scale, the biased G left out");
        assert_eq!((&c.normal_texture, c.normal_scale), (&m.normal_texture, 0.5), "{name}: the coat takes the base normal map");
        assert!(whole.warnings.iter().any(|w| w.contains("biases its clearcoatRoughness map")), "{name}: {:?}", whole.warnings);
        layer_sweep(name, text.as_bytes(), 0..text.len(), |bytes| {
            let _ = read_scene(bytes, MeshFormat::Usda, &side);
        });
    }

    /// [`usd_layer_inputs_survive_truncation_and_bit_flips`] over the
    /// full-size `fixtures::layered()`: two minutes in a debug build, seconds
    /// released. Run it with the other full-size sweeps:
    /// `cargo test --release --lib -- --ignored layer_inputs_survive`.
    #[test]
    #[ignore = "full-size layered: minutes in a debug build; run it released (see the doc comment)"]
    fn usd_layer_inputs_survive_truncation_and_bit_flips_full() {
        usd_layer_sweeps("layered", &fixtures::layered());
    }

    /// What [`usdc::write`](super::usdc::write) makes of the named fixtures
    /// (each one's package-mode USDA, parsed, written as a crate), through the
    /// reader and [`layer_sweep`]: each cut at every length, each bit of its
    /// first 4 KiB and of its tables (the TOKENS section to the end, where a
    /// crate keeps its offsets, counts and path tree) flipped in turn, and 2-8
    /// bits flipped at random in 10,000 seeded cases. Every reading is `Ok` or
    /// `Err`, never a panic. `only` is the fixtures' names, or every fixture.
    fn usdc_we_write_sweeps(only: Option<&[&str]>) {
        let package = WriteOptions { self_contained: false, ..Default::default() };
        for (name, scene) in fixtures::all().into_iter().filter(|(name, _)| only.is_none_or(|names| names.contains(name))) {
            let usda = write_scene(&scene, MeshFormat::Usda, package).unwrap().package.files;
            let layer = super::super::usda::parse(&usda[0].bytes, "model.usda").unwrap_or_else(|e| panic!("{name}: {e}"));
            let usdc = super::super::usdc::write(&layer).unwrap_or_else(|e| panic!("{name}: {e}"));
            let side = MemorySidecars { files: &usda };
            // The sweep starts from a crate that reads whole.
            read_scene(&usdc, MeshFormat::Usda, &side).unwrap_or_else(|e| panic!("{name} as USDC: {e}"));
            // The tables: from the TOKENS section to the end. The table of
            // contents is the end of the file, and names where each starts.
            let toc = i64::from_le_bytes(usdc[16..24].try_into().unwrap()) as usize;
            let sections = u64::from_le_bytes(usdc[toc..toc + 8].try_into().unwrap()) as usize;
            let tokens = (0..sections)
                .map(|i| toc + 8 + i * 32)
                .find(|at| usdc[*at..*at + 6] == *b"TOKENS")
                .map(|at| i64::from_le_bytes(usdc[at + 16..at + 24].try_into().unwrap()) as usize)
                .unwrap_or_else(|| panic!("{name}: no TOKENS section"));
            layer_sweep(&format!("{name} as USDC"), &usdc, tokens..usdc.len(), |bytes| {
                let _ = read_scene(bytes, MeshFormat::Usda, &side);
            });
        }
    }

    /// [`usdc_we_write_sweeps`] over the three smallest fixtures, which take
    /// about twenty seconds in a debug build (the other seven are the full-size
    /// run below): the cube (a mesh with normals, uvs and a material), the four
    /// uv sets (primvars) and the wire (segments and points with colours, and a
    /// cube).
    #[test]
    fn usdc_we_write_survives_truncation_and_bit_flips() {
        usdc_we_write_sweeps(Some(&["cube", "multiuv", "wire"]));
    }

    /// [`usdc_we_write_sweeps`] over every fixture: about four minutes in a
    /// debug build (3 minutes 46 seconds on 2026-11-09, 705,892 readings, all
    /// `Ok` or `Err`). Run it with the other full-size sweeps:
    /// `cargo test --lib -- --ignored we_write_survives`.
    #[test]
    #[ignore = "every fixture's crate: four minutes in a debug build (see the doc comment)"]
    fn usdc_we_write_survives_truncation_and_bit_flips_full() {
        usdc_we_write_sweeps(None);
    }

    #[test]
    fn readers_reject_other_containers() {
        let mut w = super::super::zip::Writer::new();
        w.add("[Content_Types].xml", b"<Types/>", super::super::zip::Store::Deflated).unwrap();
        w.add("3D/3dmodel.model", b"<model/>", super::super::zip::Store::Deflated).unwrap();
        let three_mf = w.finish();
        assert!(matches!(read_usdz(&three_mf, &NoSidecars), Err(MeshError::NotThisFormat { .. })));
        assert!(matches!(read_usda(&three_mf, &NoSidecars), Err(MeshError::NotThisFormat { .. })));
        let glb = write_scene(&fixtures::cube(), MeshFormat::Glb, WriteOptions::default()).unwrap().package.files[0].bytes.clone();
        assert!(matches!(read_usdz(&glb, &NoSidecars), Err(MeshError::NotThisFormat { .. })));
        assert!(matches!(read_usda(&glb, &NoSidecars), Err(MeshError::NotThisFormat { .. })));
        assert!(read_usdz(TEXTURED_ARKIT, &NoSidecars).is_ok());
        assert!(matches!(read_usda(TEXTURED_ARKIT, &NoSidecars), Err(MeshError::NotThisFormat { .. })));
        // A refusal names the format the caller asked for (Ruling U10): `usdc`
        // reads a layer in either encoding, so what it refuses is no layer.
        for (bytes, what) in [(&three_mf, "a zip"), (&glb, "a glb")] {
            assert!(matches!(read_usdc(bytes, &NoSidecars), Err(MeshError::NotThisFormat { expected: MeshFormat::Usdc, .. })), "{what} read as usdc");
            assert!(matches!(read_usda(bytes, &NoSidecars), Err(MeshError::NotThisFormat { expected: MeshFormat::Usda, .. })), "{what} read as usda");
        }
        assert!(matches!(read_usdc(TEXTURED_ARKIT, &NoSidecars), Err(MeshError::NotThisFormat { expected: MeshFormat::Usdc, .. })), "a usdz read as usdc");
        let cube_text = write_scene(&fixtures::cube(), MeshFormat::Usda, WriteOptions::default()).unwrap().package.files[0].bytes.clone();
        assert!(read_usdc(&cube_text, &NoSidecars).is_ok(), "a text layer reads as usdc: the reader takes either encoding");
        assert!(read_usda(CUBE_USDC, &NoSidecars).is_ok() && read_usdc(CUBE_USDC, &NoSidecars).is_ok(), "a crate reads as either");
    }

    // ── what other writers put in USD ───────────────────────────────────────

    fn usda(body: &str) -> String {
        format!("#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n)\ndef Xform \"R\"\n{{\n{body}\n}}\n")
    }

    fn read_text(body: &str, files: &[PackageFile]) -> SceneRead {
        let text = usda(body);
        read_usda(text.as_bytes(), &MemorySidecars { files }).unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    const QUAD: &str = r#"
        int[] faceVertexCounts = [4]
        int[] faceVertexIndices = [0, 1, 2, 3]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)]
    "#;

    #[test]
    fn reads_face_varying_indexed_and_polygonal_meshes() {
        let r = read_text(
            r#"def Mesh "M" {
                int[] faceVertexCounts = [4, 5]
                int[] faceVertexIndices = [0, 1, 2, 3, 1, 4, 5, 6, 2]
                point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0), (2, 0, 0), (2.5, 0.5, 0), (2, 1, 0)]
                normal3f[] normals = [(0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1)] (interpolation = "faceVarying")
                texCoord2f[] primvars:st = [(0, 0), (1, 0), (1, 1), (0, 1)] (interpolation = "faceVarying")
                int[] primvars:st:indices = [0, 1, 2, 3, 0, 1, 1, 2, 3]
            }"#,
            &[],
        );
        let p = &r.scene.meshes[0].primitives[0];
        assert_eq!(p.triangle_count(), 2 + 3, "a quad and a pentagon, fanned");
        // Point 1 is used with uv (1,0) and (0,0): split into two vertices.
        let uses: Vec<[f32; 2]> = p.positions.iter().zip(&p.uvs[0]).filter(|(q, _)| **q == [1.0, 0.0, 0.0]).map(|(_, uv)| *uv).collect();
        assert_eq!(uses.len(), 2, "{uses:?}");
        assert!(uses.contains(&[1.0, 1.0]) && uses.contains(&[0.0, 1.0]), "st (1,0)/(0,0) flip to uv (1,1)/(0,1): {uses:?}");
    }

    #[test]
    fn left_handed_faces_and_holes_are_honoured() {
        let body = |o: &str| format!("def Mesh \"M\" {{ {QUAD} uniform token orientation = \"{o}\" }}");
        let right = read_text(&body("rightHanded"), &[]);
        let left = read_text(&body("leftHanded"), &[]);
        let facet = |s: &Scene| {
            let p = &s.meshes[0].primitives[0];
            let i = &p.indices;
            super::super::ir::Mesh::facet_normal(&[p.positions[i[0] as usize], p.positions[i[1] as usize], p.positions[i[2] as usize]])
        };
        assert!(facet(&right.scene)[2] > 0.0 && facet(&left.scene)[2] < 0.0);
        let holed = read_text(
            r#"def Mesh "M" {
                int[] faceVertexCounts = [3, 3]
                int[] faceVertexIndices = [0, 1, 2, 0, 2, 3]
                point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)]
                int[] holeIndices = [1]
            }"#,
            &[],
        );
        assert_eq!(holed.scene.triangle_count(), 1);
    }

    #[test]
    fn geom_subsets_split_by_material_and_keep_uncovered_faces() {
        let r = read_text(
            r#"def Mesh "M" (prepend apiSchemas = ["MaterialBindingAPI"]) {
                int[] faceVertexCounts = [3, 3, 3]
                int[] faceVertexIndices = [0, 1, 2, 0, 2, 3, 1, 4, 2]
                point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0), (2, 0, 0)]
                uniform token subsetFamily:materialBind:familyType = "nonOverlapping"
                rel material:binding = </R/Red>
                def GeomSubset "G" (prepend apiSchemas = ["MaterialBindingAPI"]) {
                    uniform token elementType = "face"
                    uniform token familyName = "materialBind"
                    int[] indices = [1]
                    rel material:binding = </R/Green>
                }
            }
            def Material "Red" { token outputs:surface.connect = </R/Red/S.outputs:surface>
                def Shader "S" { uniform token info:id = "UsdPreviewSurface"
                    color3f inputs:diffuseColor = (1, 0, 0)
                    token outputs:surface } }
            def Material "Green" { token outputs:surface.connect = </R/Green/S.outputs:surface>
                def Shader "S" { uniform token info:id = "UsdPreviewSurface"
                    color3f inputs:diffuseColor = (0, 1, 0)
                    token outputs:surface } }"#,
            &[],
        );
        let m = &r.scene.meshes[0];
        assert_eq!(m.primitives.len(), 2);
        let color = |p: &Primitive| r.scene.materials[p.material.unwrap()].base_color;
        assert_eq!((m.primitives[0].triangle_count(), color(&m.primitives[0])), (1, [0.0, 1.0, 0.0, 1.0]));
        assert_eq!((m.primitives[1].triangle_count(), color(&m.primitives[1])), (2, [1.0, 0.0, 0.0, 1.0]), "uncovered faces keep the mesh's binding");
    }

    #[test]
    fn reads_preview_surface_networks_as_glb_materials() {
        let png = fixtures::quadrant_png();
        let files = vec![
            PackageFile { path: "tex/c.png".into(), bytes: png.clone() },
            PackageFile { path: "tex/mr.png".into(), bytes: png.clone() },
            PackageFile { path: "tex/n.png".into(), bytes: png.clone() },
        ];
        let r = read_text(
            &format!(
                r#"def Mesh "M" (prepend apiSchemas = ["MaterialBindingAPI"]) {{ {QUAD}
                    texCoord2f[] primvars:st = [(0, 0), (1, 0), (1, 1), (0, 1)] (interpolation = "vertex")
                    rel material:binding = </R/Mat> }}
                def Material "Mat" {{
                    asset inputs:colorTex = @tex/c.png@
                    token outputs:surface.connect = </R/Mat/NG/PS.outputs:surface>
                    def NodeGraph "NG" {{
                        def Shader "PS" {{
                            uniform token info:id = "UsdPreviewSurface"
                            color3f inputs:diffuseColor.connect = </R/Mat/NG/C.outputs:rgb>
                            float inputs:opacity.connect = </R/Mat/NG/C.outputs:a>
                            float inputs:opacityThreshold = 0.4
                            float inputs:roughness.connect = </R/Mat/NG/MR.outputs:g>
                            float inputs:metallic.connect = </R/Mat/NG/MR.outputs:b>
                            normal3f inputs:normal.connect = </R/Mat/NG/N.outputs:rgb>
                            color3f inputs:emissiveColor = (0.5, 0.25, 0)
                            token outputs:surface }}
                        def Shader "R" {{ uniform token info:id = "UsdPrimvarReader_float2"
                            string inputs:varname = "st"
                            float2 outputs:result }}
                        def Shader "C" {{ uniform token info:id = "UsdUVTexture"
                            asset inputs:file.connect = </R/Mat.inputs:colorTex>
                            float2 inputs:st.connect = </R/Mat/NG/R.outputs:result>
                            float4 inputs:scale = (0.5, 0.5, 0.5, 1)
                            token inputs:wrapS = "mirror"
                            token inputs:wrapT = "clamp"
                            float3 outputs:rgb
                            float outputs:a }}
                        def Shader "MR" {{ uniform token info:id = "UsdUVTexture"
                            asset inputs:file = @tex/mr.png@
                            float2 inputs:st.connect = </R/Mat/NG/R.outputs:result>
                            token inputs:sourceColorSpace = "raw"
                            float4 inputs:scale = (1, 0.8, 0.3, 1)
                            float outputs:g
                            float outputs:b }}
                        def Shader "N" {{ uniform token info:id = "UsdUVTexture"
                            asset inputs:file = @tex/n.png@
                            float2 inputs:st.connect = </R/Mat/NG/R.outputs:result>
                            token inputs:sourceColorSpace = "raw"
                            float4 inputs:scale = (1.5, 1.5, 2, 1)
                            float4 inputs:bias = (-0.75, -0.75, -1, 0)
                            float3 outputs:rgb }}
                    }}
                }}"#
            ),
            &files,
        );
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let m = &r.scene.materials[0];
        assert_eq!(m.base_color, [0.5, 0.5, 0.5, 1.0]);
        assert!(m.base_color_texture.is_some());
        assert_eq!(m.alpha_mode, super::super::scene::AlphaMode::Mask);
        assert!((m.alpha_cutoff - 0.4).abs() < 1e-6);
        assert!(m.metallic_roughness_texture.is_some());
        assert!((m.roughness - 0.8).abs() < 1e-6 && (m.metallic - 0.3).abs() < 1e-6);
        assert!(m.normal_texture.is_some() && (m.normal_scale - 0.75).abs() < 1e-6);
        assert_eq!(m.emissive, [0.5, 0.25, 0.0]);
        let t = &r.scene.textures[m.base_color_texture.as_ref().unwrap().texture];
        assert_eq!((t.sampler.wrap_s, t.sampler.wrap_t), (super::super::scene::Wrap::MirroredRepeat, super::super::scene::Wrap::ClampToEdge));
        assert_eq!(r.scene.images.len(), 3, "one image per file");
    }

    /// 2026-10-12 review, M3: metallic and roughness in two separate images
    /// read as glTF's one packed map (metallic in B, roughness in G, each
    /// baked through its texture's scale and bias), as Blender's glTF
    /// exporter merges them. The map's scale used to become the factor, so
    /// a scale of 1 made the material polished metal.
    #[test]
    fn separate_metallic_and_roughness_maps_pack_into_one() {
        // Metallic: R is 255 in the left half, 0 in the right (mean 0.5).
        let (mw, mh) = (4u32, 4u32);
        let metal: Vec<u8> = (0..mw * mh).flat_map(|i| if i % mw < mw / 2 { [255, 9, 9, 255] } else { [0, 9, 9, 255] }).collect();
        // Roughness: G is 0.4 everywhere, at another size; scale 0.5 → 0.2.
        let rough: Vec<u8> = (0..8 * 8).flat_map(|_| [9u8, 102, 9, 255]).collect();
        let files = vec![
            PackageFile { path: "m.png".into(), bytes: fixtures::encode_png(mw, mh, &metal) },
            PackageFile { path: "r.png".into(), bytes: fixtures::encode_png(8, 8, &rough) },
        ];
        let network = |metal_file: &str| {
            format!(
                r#"def Mesh "M" (prepend apiSchemas = ["MaterialBindingAPI"]) {{ {QUAD}
                    texCoord2f[] primvars:st = [(0, 0), (1, 0), (1, 1), (0, 1)] (interpolation = "vertex")
                    rel material:binding = </R/Mat> }}
                def Material "Mat" {{
                    token outputs:surface.connect = </R/Mat/PS.outputs:surface>
                    def Shader "PS" {{ uniform token info:id = "UsdPreviewSurface"
                        float inputs:metallic.connect = </R/Mat/MT.outputs:r>
                        float inputs:roughness.connect = </R/Mat/RT.outputs:g>
                        token outputs:surface }}
                    def Shader "St" {{ uniform token info:id = "UsdPrimvarReader_float2"
                        string inputs:varname = "st"
                        float2 outputs:result }}
                    def Shader "MT" {{ uniform token info:id = "UsdUVTexture"
                        asset inputs:file = @{metal_file}@
                        float2 inputs:st.connect = </R/Mat/St.outputs:result>
                        token inputs:sourceColorSpace = "raw"
                        float outputs:r }}
                    def Shader "RT" {{ uniform token info:id = "UsdUVTexture"
                        asset inputs:file = @r.png@
                        float2 inputs:st.connect = </R/Mat/St.outputs:result>
                        token inputs:sourceColorSpace = "raw"
                        float4 inputs:scale = (1, 0.5, 1, 1)
                        float outputs:g }}
                }}"#
            )
        };
        let r = read_text(&network("m.png"), &files);
        let m = &r.scene.materials[0];
        let packed = m.metallic_roughness_texture.as_ref().expect("one packed map");
        assert_eq!((m.metallic, m.roughness), (1.0, 1.0), "the maps carry the values");
        let img = &r.scene.images[r.scene.textures[packed.texture].image];
        let means = super::super::normalize::channel_means(&img.data, img.format).unwrap();
        assert!((means[2] - 0.5).abs() < 0.02, "metallic (B) {means:?}");
        assert!((means[1] - 0.2).abs() < 0.01, "roughness (G) {means:?}");
        assert_eq!(r.scene.images.len(), 1, "the two source images are not carried as well");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);

        // A metallic map that cannot be had leaves metallic at USD's 0, not 1.
        let r = read_text(&network("missing.png"), &files);
        let m = &r.scene.materials[0];
        let packed = m.metallic_roughness_texture.as_ref().expect("roughness still packed");
        let img = &r.scene.images[r.scene.textures[packed.texture].image];
        let means = super::super::normalize::channel_means(&img.data, img.format).unwrap();
        assert!(m.metallic * means[2] < 0.01, "metallic {} × {means:?}", m.metallic);
        assert!((m.roughness * means[1] - 0.2).abs() < 0.01, "roughness {} × {means:?}", m.roughness);
    }

    /// 2026-10-12 review, findings 23.1 and N6: a shader input connected to
    /// the material's interface — `inputs:varname.connect =
    /// </Mat.inputs:frame:stPrimvarName>` as Blender and Reality Composer
    /// write it, a connected `wrapS` — reads through the connection, as every
    /// other input does. A literal was all that was read: the UV set, the
    /// colour primvar and the wrap were lost.
    #[test]
    fn connected_varnames_and_texture_settings_are_followed() {
        let r = read_text(
            &format!(
                r#"def Mesh "M" (prepend apiSchemas = ["MaterialBindingAPI"]) {{ {QUAD}
                    color3f[] primvars:displayColor = [(1, 0, 0)] (interpolation = "constant")
                    rel material:binding = </R/Mat> }}
                def Material "Mat" {{ token inputs:colorVar = "displayColor"
                    token outputs:surface.connect = </R/Mat/S.outputs:surface>
                    def Shader "S" {{ uniform token info:id = "UsdPreviewSurface"
                        color3f inputs:diffuseColor.connect = </R/Mat/Rd.outputs:result>
                        token outputs:surface }}
                    def Shader "Rd" {{ uniform token info:id = "UsdPrimvarReader_float3"
                        token inputs:varname.connect = </R/Mat.inputs:colorVar>
                        float3 outputs:result }} }}"#
            ),
            &[],
        );
        let p = &r.scene.meshes[0].primitives[0];
        assert!(p.colors.as_ref().is_some_and(|c| c.iter().all(|c| c[0] > 0.99 && c[1] < 0.01)), "{:?} {:?}", p.colors, r.warnings);

        let files = vec![PackageFile { path: "c.png".into(), bytes: fixtures::quadrant_png() }];
        let r = read_text(
            &format!(
                r#"def Mesh "M" (prepend apiSchemas = ["MaterialBindingAPI"]) {{ {QUAD}
                    texCoord2f[] primvars:uv = [(0, 0), (1, 0), (1, 1), (0, 1)] (interpolation = "vertex")
                    rel material:binding = </R/Mat> }}
                def Material "Mat" {{
                    token inputs:frame:stPrimvarName = "uv"
                    token inputs:wrap = "mirror"
                    token outputs:surface.connect = </R/Mat/S.outputs:surface>
                    def Shader "S" {{ uniform token info:id = "UsdPreviewSurface"
                        color3f inputs:diffuseColor.connect = </R/Mat/C.outputs:rgb>
                        token outputs:surface }}
                    def Shader "St" {{ uniform token info:id = "UsdPrimvarReader_float2"
                        token inputs:varname.connect = </R/Mat.inputs:frame:stPrimvarName>
                        float2 outputs:result }}
                    def Shader "C" {{ uniform token info:id = "UsdUVTexture"
                        asset inputs:file = @c.png@
                        float2 inputs:st.connect = </R/Mat/St.outputs:result>
                        token inputs:wrapS.connect = </R/Mat.inputs:wrap>
                        float3 outputs:rgb }} }}"#
            ),
            &files,
        );
        let m = &r.scene.materials[0];
        let tex = m.base_color_texture.as_ref().unwrap_or_else(|| panic!("{:?}", r.warnings));
        assert!(!r.scene.meshes[0].primitives[0].uvs.is_empty(), "the texture's UV set (`uv`) was read: {:?}", r.warnings);
        assert_eq!(r.scene.textures[tex.texture].sampler.wrap_s, super::super::scene::Wrap::MirroredRepeat);
    }

    /// 2026-10-12 review, findings 23.11 and N7: `\xNN` and octal `\NNN`
    /// escapes are bytes of the UTF-8 string (OpenUSD's TfEscapeString), so
    /// `Caf\xc3\xa9` is "Café", not one character per byte ("CafÃ©").
    #[test]
    fn escaped_bytes_decode_as_utf8() {
        for spelled in [r"Caf\xc3\xa9", r"Caf\303\251"] {
            let r = read_text(&format!(r#"def Mesh "M" ( customData = {{ string originalName = "{spelled}" }} ) {{ {QUAD} }}"#), &[]);
            let name = r.scene.nodes.iter().find_map(|n| n.name.clone());
            assert_eq!(name.as_deref(), Some("Café"), "{spelled}");
        }
    }

    /// 2026-10-12 review, finding 23.13: a zero scale (a part hidden by
    /// scaling it away) is written as a millionth, as the Collada and FBX
    /// writers write it (`math::nonsingular_scale`): it draws the same and
    /// leaves every transform invertible, keys included.
    #[test]
    fn a_zero_scale_is_written_as_a_millionth() {
        use super::super::scene::{AnimPath, Transform};
        let zero = Transform::Trs { translation: [0.0; 3], rotation: super::super::math::QUAT_IDENTITY, scale: [0.0; 3] };
        // A plain node's xformOp:scale, and a joint's animated scales.
        let mut cube = fixtures::cube();
        let n = cube.roots[0];
        cube.nodes[n].transform = zero.clone();
        let mut rig = fixtures::skinned();
        let j = rig.roots[0];
        rig.nodes[j].transform = zero;
        for ch in rig.animations[0].channels.iter_mut().filter(|c| c.path == AnimPath::Scale) {
            ch.values.iter_mut().for_each(|v| *v = 0.0);
        }
        for s in [cube, rig] {
            let w = write_scene(&s, MeshFormat::Usda, WriteOptions::default()).unwrap();
            let text = String::from_utf8(w.package.files[0].bytes.clone()).unwrap();
            let zeros: Vec<&str> = text.lines().filter(|l| l.contains("scale") && l.contains("(0, 0, 0)")).collect();
            assert!(zeros.is_empty(), "{zeros:?}");
        }
    }

    /// 2026-10-12 review, finding 23.12: a morph target named what a prim
    /// cannot be (`smile:L`) keeps its name, as nodes and materials do, in
    /// the BlendShape's customData.
    #[test]
    fn blend_shape_names_survive_sanitising() {
        let mut s = fixtures::morph();
        s.meshes[0].target_names[0] = "smile:L".into();
        for to in [MeshFormat::Usda, MeshFormat::Usdz] {
            let w = write_scene(&s, to, WriteOptions::default()).unwrap();
            let back = super::super::read_scene(&w.package.files[0].bytes, to, &NoSidecars).unwrap().scene;
            assert_eq!(back.meshes[0].target_names.first().map(String::as_str), Some("smile:L"), "{to}");
        }
    }

    #[test]
    fn display_color_counts_only_where_usd_draws_it() {
        let colored = r#"color3f[] primvars:displayColor = [(1, 0, 0), (0, 1, 0), (0, 0, 1), (1, 1, 1)] (interpolation = "vertex")"#;
        // Unbound: Hydra draws displayColor.
        let r = read_text(&format!("def Mesh \"M\" {{ {QUAD} {colored} }}"), &[]);
        assert!(r.scene.meshes[0].primitives[0].colors.is_some());
        // Bound to a material that ignores it: the material is drawn.
        let r = read_text(
            &format!(
                r#"def Mesh "M" (prepend apiSchemas = ["MaterialBindingAPI"]) {{ {QUAD} {colored} rel material:binding = </R/W> }}
                def Material "W" {{ token outputs:surface.connect = </R/W/S.outputs:surface>
                    def Shader "S" {{ uniform token info:id = "UsdPreviewSurface"
                        color3f inputs:diffuseColor = (1, 1, 1)
                        token outputs:surface }} }}"#
            ),
            &[],
        );
        assert!(r.scene.meshes[0].primitives[0].colors.is_none());
    }

    #[test]
    fn reads_skinning_as_usdskel_defines_it() {
        // Joints listed child-first, a mesh-level joint order, a non-identity
        // geomBindTransform and a rigid (constant) binding.
        let r = read_text(
            r#"def SkelRoot "Rig" {
                def Skeleton "Skel" (prepend apiSchemas = ["SkelBindingAPI"]) {
                    uniform token[] joints = ["a/b", "a"]
                    uniform matrix4d[] bindTransforms = [((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 1, 0, 1)), ((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))]
                    uniform matrix4d[] restTransforms = [((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 2, 0, 1)), ((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))]
                }
                def Mesh "Rigid" (prepend apiSchemas = ["SkelBindingAPI"]) {
                    int[] faceVertexCounts = [3]
                    int[] faceVertexIndices = [0, 1, 2]
                    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
                    uniform token[] skel:joints = ["a", "a/b"]
                    int[] primvars:skel:jointIndices = [1] (elementSize = 1
                        interpolation = "constant")
                    float[] primvars:skel:jointWeights = [1] (elementSize = 1
                        interpolation = "constant")
                    matrix4d primvars:skel:geomBindTransform = ((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (10, 0, 0, 1))
                    rel skel:skeleton = </R/Rig/Skel>
                }
            }"#,
            &[],
        );
        let s = &r.scene;
        assert_eq!(s.skins.len(), 1);
        // USD: world = J(a/b) · bind(a/b)⁻¹ · geomBind · p. J(a/b) = rest(a) ·
        // rest(a/b) = translate(0, 2, 0); bind(a/b) = translate(0, 1, 0); so
        // every point moves by (10, 1, 0).
        let d = drawn(s, None);
        assert_eq!(d[0].positions, vec![[10.0, 1.0, 0.0], [11.0, 1.0, 0.0], [10.0, 2.0, 0.0]]);
    }

    #[test]
    fn a_skeleton_outside_a_skelroot_skins_nothing() {
        let r = read_text(
            r#"def Skeleton "Skel" {
                uniform token[] joints = ["a"]
                uniform matrix4d[] bindTransforms = [((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))]
                uniform matrix4d[] restTransforms = [((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (5, 0, 0, 1))]
            }
            def Mesh "M" {
                int[] faceVertexCounts = [3]
                int[] faceVertexIndices = [0, 1, 2]
                point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
                int[] primvars:skel:jointIndices = [0, 0, 0] (elementSize = 1
                    interpolation = "vertex")
                float[] primvars:skel:jointWeights = [1, 1, 1] (elementSize = 1
                    interpolation = "vertex")
                rel skel:skeleton = </R/Skel>
            }"#,
            &[],
        );
        assert!(r.scene.skins.is_empty());
        assert!(r.warnings.iter().any(|w| w.contains("SkelRoot")), "{:?}", r.warnings);
        assert_eq!(drawn(&r.scene, None)[0].positions[0], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn reads_every_xform_op_and_converts_up_axis_and_units() {
        let r = read_usda(
            br#"#usda 1.0
(
    metersPerUnit = 0.01
    upAxis = "Z"
)
def Xform "A"
{
    float3 xformOp:translate = (100, 0, 0)
    float3 xformOp:rotateXYZ = (0, 0, 90)
    float3 xformOp:translate:pivot = (0, 100, 0)
    float xformOp:scale = 2
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:translate:pivot", "xformOp:scale", "!invert!xformOp:translate:pivot"]
    def Mesh "M"
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 100, 0), (100, 100, 0), (0, 100, 100)]
    }
}
"#,
            &NoSidecars,
        )
        .unwrap();
        // In stage space: p → T(100,0,0)·Rz(90)·T(pivot)·S(2)·T(-pivot)·p.
        // (0,100,0) stays at the pivot: Rz(90)(0,100,0) = (-100,0,0), + (100,0,0)
        // = (0,0,0). Then Z-up → Y-up and centimetres → metres.
        let d = drawn(&r.scene, None);
        let p = d[0].positions[0];
        assert!(p.iter().all(|v| v.abs() < 1e-5), "{p:?}");
        // (0,100,100): 2·(0,0,100) about the pivot is (0,100,200); rotated,
        // (-100,0,200); moved, (0,0,200) cm, Z up: (0, 2, 0) m.
        let q = d[0].positions[2];
        assert!((q[0]).abs() < 1e-5 && (q[1] - 2.0).abs() < 1e-5 && q[2].abs() < 1e-5, "{q:?}");
    }

    #[test]
    fn euler_animation_is_sampled_the_way_usd_interpolates_it() {
        let r = read_text(
            r#"def Xform "Spin" {
                float3 xformOp:rotateXYZ.timeSamples = {
                    0: (0, 0, 0),
                    24: (0, 0, 170),
                }
                uniform token[] xformOpOrder = ["xformOp:rotateXYZ"]
                def Mesh "M" { int[] faceVertexCounts = [3]
                    int[] faceVertexIndices = [0, 1, 2]
                    point3f[] points = [(1, 0, 0), (2, 0, 0), (1, 1, 0)] }
            }"#,
            &[],
        );
        // USD lerps the angle: at code 12, 85°. A slerp between the two keys
        // would agree here, but the resampling is what makes any stack right.
        let s = &r.scene;
        let at = |t: f32| drawn(s, Some(t))[0].positions[0];
        let p = at(0.5); // 24 tcps default: code 12
        let want = 85f32.to_radians();
        assert!((p[0] - want.cos()).abs() < 2e-3 && (p[1] - want.sin()).abs() < 2e-3, "{p:?}");
    }

    #[test]
    fn a_zero_scale_key_stays_trs() {
        let r = read_text(
            r#"def Xform "Pop" {
                float3 xformOp:scale.timeSamples = {
                    0: (0, 0, 0),
                    10: (1, 1, 1),
                }
                uniform token[] xformOpOrder = ["xformOp:scale"]
                def Mesh "M" { int[] faceVertexCounts = [3]
                    int[] faceVertexIndices = [0, 1, 2]
                    point3f[] points = [(1, 0, 0), (2, 0, 0), (1, 1, 0)] }
            }"#,
            &[],
        );
        assert!(matches!(r.scene.nodes[0].transform, Transform::Trs { scale: [0.0, 0.0, 0.0], .. }));
        let glb = write_scene(&r.scene, MeshFormat::Glb, WriteOptions::default()).unwrap();
        assert!(!String::from_utf8_lossy(&glb.package.files[0].bytes).contains("\"matrix\""));
    }

    #[test]
    fn a_stack_animated_from_zero_scale_is_animation_not_shear() {
        // A part that pops in: its op stack (translate, rotate, scale) is
        // sampled as one matrix, which is singular where the scale is 0. That
        // is a degenerate scale, not shear, and the animation must survive.
        let r = read_text(
            r#"def Xform "Pop" {
                double3 xformOp:translate = (1, 0, 0)
                float3 xformOp:rotateXYZ = (0, 0, 90)
                float3 xformOp:scale.timeSamples = {
                    0: (0, 0, 0),
                    24: (2, 2, 2),
                }
                uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"]
                def Mesh "M" { int[] faceVertexCounts = [3]
                    int[] faceVertexIndices = [0, 1, 2]
                    point3f[] points = [(1, 0, 0), (2, 0, 0), (1, 1, 0)] }
            }"#,
            &[],
        );
        assert!(r.warnings.iter().all(|w| !w.contains("shear")), "{:?}", r.warnings);
        assert_eq!(r.scene.animations.len(), 1, "the pop-in animation was dropped");
        // Halfway (code 12 at 24 tcps): scale 1, so (1,0,0) turns to (0,1,0)
        // and moves to (1,1,0).
        let p = drawn(&r.scene, Some(0.5))[0].positions[0];
        assert!((p[0] - 1.0).abs() < 1e-4 && (p[1] - 1.0).abs() < 1e-4 && p[2].abs() < 1e-4, "{p:?}");
    }

    #[test]
    fn skins_meshes_and_morphs_survive_together() {
        // A skinned mesh that also has morph targets: one skeleton drives both.
        let mut s = fixtures::skinned();
        let n = s.meshes[0].primitives[0].positions.len();
        s.meshes[0].primitives[0].targets.push(super::super::scene::MorphTarget { positions: vec![[0.1, 0.0, 0.0]; n], normals: None });
        s.meshes[0].weights = vec![0.5];
        s.meshes[0].target_names = vec!["fat".into()];
        let want = MeshFormat::Usdz.prepare(&s, WriteOptions::default()).unwrap();
        let (r, _) = round(&s, MeshFormat::Usdz, true);
        assert_eq!(r.scene.meshes[0].target_names, vec!["fat".to_string()]);
        assert_draws_alike(&want, &r.scene, &fixtures::sample_times(&s), Check { tol: 2e-5, ..FULL }, "skinned+morph");
    }

    #[test]
    fn an_animated_group_between_joints_is_folded_into_the_joint() {
        // A USD skeleton has no place for a non-joint node between two joints:
        // prepare folds it, resampling its motion into the child joint.
        let mut s = fixtures::skinned();
        let group = s.nodes.len();
        let q10 = math::quat_from_axis_angle([0.0, 0.0, 1.0], 10f64.to_radians());
        s.nodes.push(Node {
            name: Some("shin_group".into()),
            transform: Transform::Trs { translation: [0.0, 0.2, 0.0], rotation: q10, scale: [1.0; 3] },
            children: vec![2],
            ..Default::default()
        });
        s.nodes[1].children = vec![group];
        let q30 = math::quat_from_axis_angle([0.0, 0.0, 1.0], 30f64.to_radians());
        let values: Vec<f32> = q10.iter().chain(q30.iter()).map(|v| *v as f32).collect();
        s.animations[0].channels.push(Channel { node: group, path: AnimPath::Rotation, interpolation: Interpolation::Linear, times: vec![0.0, 2.0], values });
        // Without the hip's scale keys, which USD stores as halves: what is
        // left to differ is the folding alone.
        s.animations[0].channels.retain(|c| !(c.node == 0 && c.path == AnimPath::Scale));
        s.validate().unwrap();
        let want = MeshFormat::Usdz.prepare(&s, WriteOptions::default()).unwrap();
        assert_eq!(want.parents()[2], Some(1), "the ankle hangs from the knee");
        // Exact on the resampling grid.
        let grid = [0.0, 0.5, 1.0, 1.5, 2.0];
        assert_draws_alike(&s, &want, &grid, Check { tol: 1e-5, ..FULL }, "folded vs original");
        let (r, _) = round(&s, MeshFormat::Usdz, true);
        assert_draws_alike(&want, &r.scene, &grid, Check { tol: 2e-5, ..FULL }, "folded round trip");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn unused_skins_become_plain_nodes() {
        let mut s = fixtures::textured();
        s.skins.push(Skin { name: None, joints: vec![1, 2], inverse_bind_matrices: vec![super::super::math::IDENTITY; 2], skeleton: None });
        let p = MeshFormat::Usdz.prepare(&s, WriteOptions::default()).unwrap();
        assert!(p.skins.is_empty());
        let (r, _) = round(&s, MeshFormat::Usdz, true);
        assert_eq!(names(&r.scene), names(&p));
    }

    /// 2026-10-12 review, finding 11: three reader paths that nothing charged.
    /// Each blend shape is a points-length array per primitive however
    /// sparse its offsets (16k points × 1,024 one-offset shapes are 49M
    /// values, ~400 MB); every distinct `geomBindTransform` copies the whole
    /// skeleton into its own skin, with a joint-by-joint root search
    /// (2,000 joints × 2,000 meshes were 4M entries, 533 MB).
    #[test]
    fn blend_shapes_and_skins_are_charged_as_they_are_built() {
        let points: String = (0..16_002).map(|i| format!("({}, {}, 0)", i % 200, i / 200)).collect::<Vec<_>>().join(", ");
        let indices: String = (0..16_002).map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
        let shapes: String = (0..1024).map(|k| format!("def BlendShape \"s{k}\" {{\n uniform vector3f[] offsets = [(0, 0, 1)]\n uniform int[] pointIndices = [{k}]\n}}\n")).collect();
        let names: String = (0..1024).map(|k| format!("\"s{k}\"")).collect::<Vec<_>>().join(", ");
        let targets: String = (0..1024).map(|k| format!("</R/M/s{k}>")).collect::<Vec<_>>().join(", ");
        let body = format!(
            "def Mesh \"M\" (prepend apiSchemas = [\"SkelBindingAPI\"]) {{\n int[] faceVertexCounts = [{}]\n int[] faceVertexIndices = [{indices}]\n point3f[] points = [{points}]\n uniform token[] skel:blendShapes = [{names}]\n rel skel:blendShapeTargets = [{targets}]\n{shapes}}}\n",
            vec!["3"; 5_334].join(", ")
        );
        let e = read_usda(usda(&body).as_bytes(), &NoSidecars).map(|_| ()).unwrap_err();
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("blend shape")), "{e:?}");

        let joints = 1001;
        let names: String = (0..joints).map(|j| format!("\"j{j}\"")).collect::<Vec<_>>().join(", ");
        let identity = vec!["((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))"; joints].join(", ");
        let meshes: String = (0..1000)
            .map(|m| format!("def Mesh \"m{m}\" (prepend apiSchemas = [\"SkelBindingAPI\"]) {{\n int[] faceVertexCounts = [3]\n int[] faceVertexIndices = [0, 1, 2]\n point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]\n int[] primvars:skel:jointIndices = [0] (elementSize = 1\n interpolation = \"constant\")\n float[] primvars:skel:jointWeights = [1] (elementSize = 1\n interpolation = \"constant\")\n matrix4d primvars:skel:geomBindTransform = ((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), ({m}, 0, 0, 1))\n rel skel:skeleton = </R/Rig/Skel>\n}}\n"))
            .collect();
        let body = format!("def SkelRoot \"Rig\" {{\n def Skeleton \"Skel\" (prepend apiSchemas = [\"SkelBindingAPI\"]) {{\n uniform token[] joints = [{names}]\n uniform matrix4d[] bindTransforms = [{identity}]\n uniform matrix4d[] restTransforms = [{identity}]\n }}\n{meshes}}}\n");
        let started = std::time::Instant::now();
        let e = read_usda(usda(&body).as_bytes(), &NoSidecars).map(|_| ()).unwrap_err();
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("skin")), "{e:?}");
        assert!(started.elapsed().as_secs() < 10, "refused only after {:?}", started.elapsed());
    }

    /// 2026-10-12 review, finding 17: a prim's property names were gathered
    /// with a list scan per name, quadratic in the property count (a 193 KB
    /// USDA took 25 s).
    #[test]
    fn a_prim_with_many_properties_reads_in_linear_time() {
        let extra: String = (0..20_000).map(|i| format!("    float p{i} = 0\n")).collect();
        let body = format!("def Mesh \"M\" {{\n{QUAD}\n{extra}}}\n");
        let started = std::time::Instant::now();
        let r = read_usda(usda(&body).as_bytes(), &NoSidecars).unwrap();
        assert_eq!(r.scene.triangle_count(), 2);
        assert!(started.elapsed().as_millis() < 2000, "{:?}", started.elapsed());
    }

    // ── USD rules the reader missed (2026-10-12 review #17, #23, N5) ────────
    // Each rule is OpenUSD's own, pinned to v26.11.1 (96c1989), with paths
    // under https://github.com/PixarAnimationStudios/OpenUSD/blob/v26.11.1/pxr/.

    const TRI: &str = r#"def Mesh "M" { int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(1, 0, 0), (2, 0, 0), (1, 1, 0)] }"#;

    /// The stage at the first file, the rest its package.
    fn read_package(files: &[(&str, &str)]) -> SceneRead {
        let pkg: Vec<PackageFile> = files.iter().map(|(p, t)| PackageFile { path: p.to_string(), bytes: t.as_bytes().to_vec() }).collect();
        read_usda(&pkg[0].bytes, &MemorySidecars { files: &pkg[1..] }).unwrap_or_else(|e| panic!("{e}"))
    }

    /// `prim` translated by samples 0 → (0, 0, 0) and 48 → (10, 0, 0), a
    /// triangle under it: at the authoring layer's 48 tcps, one second.
    fn moved(prim: &str) -> String {
        format!(
            r#"def Xform "{prim}" {{
                double3 xformOp:translate.timeSamples = {{ 0: (0, 0, 0), 48: (10, 0, 0) }}
                uniform token[] xformOpOrder = ["xformOp:translate"]
                {TRI} }}"#
        )
    }

    /// Where the triangle's first point is at `secs` stage seconds.
    fn first_point_at(r: &SceneRead, secs: f32) -> [f32; 3] {
        drawn(&r.scene, Some(secs))[0].positions[0]
    }

    /// #23.7: within one spec, time samples beat the default at any numeric
    /// time (usd/usd/stage.cpp#L8996-L9069), one sample is held at every time
    /// (usd/sdf/data.cpp#L338-L365), and usdrecord renders the start time
    /// code, not `Default()` (usdImaging/usdAppUtils/framesArgs.py#L249-L252).
    #[test]
    fn a_time_sample_beats_the_default_authored_beside_it() {
        let r = read_text(
            &format!(
                r#"def Xform "Moved" {{
                    double3 xformOp:translate = (0, 0, 0)
                    double3 xformOp:translate.timeSamples = {{ 0: (5, 0, 0) }}
                    uniform token[] xformOpOrder = ["xformOp:translate"]
                    {TRI} }}"#
            ),
            &[],
        );
        assert_eq!(drawn(&r.scene, None)[0].positions[0], [6.0, 0.0, 0.0]);
    }

    /// #23.7: the strongest spec with any value opinion decides, so a weaker
    /// layer's default never beats a stronger layer's samples
    /// (usd/usd/stage.cpp#L9383-L9400).
    #[test]
    fn a_stronger_layers_samples_beat_a_weaker_layers_default() {
        let weak = format!(
            "#usda 1.0\n(\n    defaultPrim = \"R\"\n)\ndef Xform \"R\" {{\n def Xform \"Moved\" {{\n double3 xformOp:translate = (0, 0, 0)\n uniform token[] xformOpOrder = [\"xformOp:translate\"]\n {TRI} }}\n}}\n"
        );
        let root = "#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n    subLayers = [@weak.usda@]\n)\nover \"R\" {\n over \"Moved\" {\n double3 xformOp:translate.timeSamples = { 0: (5, 0, 0) }\n }\n}\n";
        let r = read_package(&[("root.usda", root), ("weak.usda", &weak)]);
        assert_eq!(drawn(&r.scene, None)[0].positions[0], [6.0, 0.0, 0.0], "{:?}", r.warnings);
    }

    /// #23.9: a sublayer's time codes are scaled by the listing layer's tcps
    /// over its own (usd/pcp/layerStack.cpp#L1878-L1892): a 48 tcps sublayer's
    /// code 48 is one second under a 24 tcps root, not two.
    #[test]
    fn a_sublayers_own_time_codes_per_second_are_honoured() {
        let sub = format!("#usda 1.0\n(\n    timeCodesPerSecond = 48\n)\ndef Xform \"R\" {{\n{}\n}}\n", moved("Moved"));
        let root = "#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n    timeCodesPerSecond = 24\n    subLayers = [@sub.usda@]\n)\n";
        let r = read_package(&[("root.usda", root), ("sub.usda", &sub)]);
        let p = first_point_at(&r, 0.5);
        assert!((p[0] - 6.0).abs() < 1e-3, "halfway through the second: {p:?} {:?}", r.warnings);
    }

    /// #23.8: a reference's time scale is the tcps of the layer that authors
    /// it over the referenced layer's (usd/pcp/primIndex.cpp#L2381-L2395,
    /// srcLayer per usd/pcp/composeSite.cpp#L128-L132), composed under the
    /// authoring layer's own mapping (primIndex.cpp#L2251-L2257). A reference
    /// authored in a 48 tcps layer to another 48 tcps layer, under a 24 tcps
    /// root, played twice as fast.
    #[test]
    fn a_nested_reference_is_scaled_by_the_layer_that_authors_it() {
        let root = "#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n    timeCodesPerSecond = 24\n)\ndef Xform \"R\" (\n    prepend references = @a.usda@\n)\n{\n}\n";
        let a = "#usda 1.0\n(\n    defaultPrim = \"A\"\n    timeCodesPerSecond = 48\n)\ndef Xform \"A\" (\n    prepend references = @b.usda@\n)\n{\n}\n";
        let b = format!("#usda 1.0\n(\n    defaultPrim = \"B\"\n    timeCodesPerSecond = 48\n)\ndef Xform \"B\" {{\n{}\n}}\n", moved("Moved"));
        let r = read_package(&[("root.usda", root), ("a.usda", a), ("b.usda", &b)]);
        let p = first_point_at(&r, 0.5);
        assert!((p[0] - 6.0).abs() < 1e-3, "halfway through the second: {p:?} {:?}", r.warnings);
    }

    /// N5: a layer that authors framesPerSecond and no timeCodesPerSecond has
    /// that rate as its tcps (usd/sdf/layer.cpp#L1842-L1860), for the scale of
    /// a reference into it as for the stage's own rate.
    #[test]
    fn a_referenced_layers_frames_per_second_is_its_time_codes_per_second() {
        let root = "#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n    timeCodesPerSecond = 24\n)\ndef Xform \"R\" (\n    prepend references = @b.usda@\n)\n{\n}\n";
        let b = format!("#usda 1.0\n(\n    defaultPrim = \"B\"\n    framesPerSecond = 48\n)\ndef Xform \"B\" {{\n{}\n}}\n", moved("Moved"));
        let r = read_package(&[("root.usda", root), ("b.usda", &b)]);
        let p = first_point_at(&r, 0.5);
        assert!((p[0] - 6.0).abs() < 1e-3, "halfway through the second: {p:?} {:?}", r.warnings);
    }

    /// An internal reference takes its authored offset alone, with no tcps
    /// factor (primIndex.cpp#L2251-L2257): its target's opinions are scaled
    /// once, by their own layer's place in the stack, even when the arc is
    /// authored in a 48 tcps sublayer.
    #[test]
    fn an_internal_reference_in_a_sublayer_is_scaled_once() {
        let sub = format!(
            "#usda 1.0\n(\n    timeCodesPerSecond = 48\n)\ndef Xform \"R\" {{\n def Xform \"Moved\" (\n prepend references = </Anim>\n )\n {{\n }}\n}}\nclass Xform \"Anim\" {{\n double3 xformOp:translate.timeSamples = {{ 0: (0, 0, 0), 48: (10, 0, 0) }}\n uniform token[] xformOpOrder = [\"xformOp:translate\"]\n {TRI}\n}}\n"
        );
        let root = "#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n    timeCodesPerSecond = 24\n    subLayers = [@sub.usda@]\n)\n";
        let r = read_package(&[("root.usda", root), ("sub.usda", &sub)]);
        let p = first_point_at(&r, 0.5);
        assert!((p[0] - 6.0).abs() < 1e-3, "halfway through the second: {p:?} {:?}", r.warnings);
    }

    /// #23.6: a SkelAnimation poses its joints only when translations,
    /// rotations and scales all resolve (usd/usdSkel/animQueryImpl.cpp#L118-L171);
    /// otherwise USD shows the rest pose (usd/usdSkel/skeletonQuery.cpp#L92-L128).
    #[test]
    fn an_incomplete_skel_animation_leaves_the_rest_pose() {
        let rig = |scales: &str| {
            format!(
                r#"def SkelRoot "Rig" {{
                    def Skeleton "Skel" (prepend apiSchemas = ["SkelBindingAPI"]) {{
                        uniform token[] joints = ["a"]
                        uniform matrix4d[] bindTransforms = [((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))]
                        uniform matrix4d[] restTransforms = [((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))]
                        rel skel:animationSource = </R/Rig/Anim>
                    }}
                    def SkelAnimation "Anim" {{
                        uniform token[] joints = ["a"]
                        float3[] translations.timeSamples = {{ 0: [(0, 0, 0)], 24: [(0, 5, 0)] }}
                        quatf[] rotations = [(1, 0, 0, 0)]
                        {scales}
                    }}
                    def Mesh "M" (prepend apiSchemas = ["SkelBindingAPI"]) {{
                        int[] faceVertexCounts = [3]
                        int[] faceVertexIndices = [0, 1, 2]
                        point3f[] points = [(1, 0, 0), (2, 0, 0), (1, 1, 0)]
                        int[] primvars:skel:jointIndices = [0] (elementSize = 1
                            interpolation = "constant")
                        float[] primvars:skel:jointWeights = [1] (elementSize = 1
                            interpolation = "constant")
                        rel skel:skeleton = </R/Rig/Skel>
                    }}
                }}"#
            )
        };
        let complete = read_text(&rig("half3[] scales = [(1, 1, 1)]"), &[]);
        assert!((first_point_at(&complete, 1.0)[1] - 5.0).abs() < 1e-4, "the control animates");

        let r = read_text(&rig(""), &[]);
        assert!(r.scene.animations.is_empty(), "{:?}", r.scene.animations.len());
        assert_eq!(first_point_at(&r, 1.0), [1.0, 0.0, 0.0], "the rest pose");
        assert!(r.warnings.iter().any(|w| w.contains("scales")), "{:?}", r.warnings);
    }

    /// #23.2-23.4: a mesh inherits skel:joints, joint influences and its
    /// geomBindTransform from ancestors inside its SkelRoot
    /// (usd/usdSkel/cacheImpl.cpp#L241-L328); the imaging path passes joint
    /// influences down only when constant
    /// (usdImaging/usdSkelImaging/jointInfluencesData.cpp#L28-L41). Here the
    /// rig of `reads_skinning_as_usdskel_defines_it`, authored on a group.
    #[test]
    fn skel_bindings_are_inherited_from_ancestors_in_the_skel_root() {
        let r = read_text(
            r#"def SkelRoot "Rig" {
                def Skeleton "Skel" (prepend apiSchemas = ["SkelBindingAPI"]) {
                    uniform token[] joints = ["a/b", "a"]
                    uniform matrix4d[] bindTransforms = [((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 1, 0, 1)), ((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))]
                    uniform matrix4d[] restTransforms = [((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 2, 0, 1)), ((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))]
                }
                def Xform "Group" (prepend apiSchemas = ["SkelBindingAPI"]) {
                    uniform token[] skel:joints = ["a", "a/b"]
                    int[] primvars:skel:jointIndices = [1] (elementSize = 1
                        interpolation = "constant")
                    float[] primvars:skel:jointWeights = [1] (elementSize = 1
                        interpolation = "constant")
                    matrix4d primvars:skel:geomBindTransform = ((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (10, 0, 0, 1))
                    rel skel:skeleton = </R/Rig/Skel>
                    def Mesh "Rigid" (prepend apiSchemas = ["SkelBindingAPI"]) {
                        int[] faceVertexCounts = [3]
                        int[] faceVertexIndices = [0, 1, 2]
                        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
                    }
                }
            }"#,
            &[],
        );
        assert_eq!(r.scene.skins.len(), 1, "{:?}", r.warnings);
        let d = drawn(&r.scene, None);
        assert_eq!(d[0].positions, vec![[10.0, 1.0, 0.0], [11.0, 1.0, 0.0], [10.0, 2.0, 0.0]]);
    }

    /// #23.5: a gprim draws the constant primvars of its ancestors that it
    /// does not author itself (usd/usdGeom/primvarsAPI.h#L331-L333; Hydra
    /// imaging/hd/flattenedPrimvarsDataSourceProvider.cpp#L168-L190). Its own
    /// primvar of any interpolation wins, a blocked one lets the ancestor's
    /// show through, and a non-constant ancestor primvar is not inherited.
    #[test]
    fn constant_primvars_are_inherited_from_ancestors() {
        let r = read_text(
            &format!(
                r#"def Xform "Painted" {{
                    color3f[] primvars:displayColor = [(1, 0, 0)] (interpolation = "constant")
                    def Mesh "Inherits" {{ {QUAD} }}
                    def Mesh "Own" {{ {QUAD} color3f[] primvars:displayColor = [(0, 0, 1)] (interpolation = "constant") }}
                    def Mesh "Blocked" {{ {QUAD} color3f[] primvars:displayColor = None }}
                }}
                def Xform "Varying" {{
                    color3f[] primvars:displayColor = [(1, 0, 0), (1, 0, 0), (1, 0, 0), (1, 0, 0)] (interpolation = "vertex")
                    def Mesh "Not" {{ {QUAD} }}
                }}"#
            ),
            &[],
        );
        let color = |name: &str| {
            let m = r.scene.meshes.iter().find(|m| m.name.as_deref() == Some(name)).unwrap_or_else(|| panic!("{name}"));
            m.primitives[0].colors.as_ref().map(|c| c[0])
        };
        let red = |c: Option<[f32; 4]>| c.is_some_and(|c| c[0] > 0.99 && c[1] < 0.01 && c[2] < 0.01);
        assert!(red(color("Inherits")), "{:?}", color("Inherits"));
        assert!(color("Own").is_some_and(|c| c[2] > 0.99 && c[0] < 0.01), "{:?}", color("Own"));
        assert!(red(color("Blocked")), "{:?}", color("Blocked"));
        assert_eq!(color("Not"), None);
    }

    /// A lattice of internal references: each of a level's `width` prims
    /// references every prim of the next, so a prim reaches the last level
    /// along width^depth paths while only width·depth prims exist.
    fn reference_lattice(depth: usize, width: usize) -> String {
        let names = |level: usize| (0..width).map(|i| format!("</L{level}_{i}>")).collect::<Vec<_>>().join(", ");
        let mut s = String::from("#usda 1.0\n(\n    defaultPrim = \"R\"\n)\n");
        for level in 0..depth {
            for i in 0..width {
                let arcs = if level + 1 < depth { format!("(\n    prepend references = [{}]\n)", names(level + 1)) } else { String::new() };
                s.push_str(&format!("over \"L{level}_{i}\" {arcs}\n{{\n    custom int depth = {level}\n}}\n"));
            }
        }
        s.push_str(&format!("def Xform \"R\" (\n    prepend references = [{}]\n)\n{{\n{TRI}\n}}\n", names(0)));
        s
    }

    /// #17: composition expanded every path through the arcs, so a 4 KB
    /// lattice six wide and six deep took 86 ms and each further level six
    /// times longer. A site joins a prim's composition once, at its strongest
    /// occurrence, as Pcp builds a prim index: linear in the sites.
    #[test]
    fn a_lattice_of_references_composes_each_site_once() {
        let r = read_usda(reference_lattice(6, 6).as_bytes(), &NoSidecars).unwrap();
        assert!(r.warnings.iter().all(|w| !w.contains("opinions")), "{:?}", r.warnings);
        let started = std::time::Instant::now();
        let r = read_usda(reference_lattice(16, 6).as_bytes(), &NoSidecars).unwrap();
        assert_eq!(r.scene.meshes.len(), 1);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    }

    /// D4 (2026-10-12 whole-branch review): resampling a clip into USD's
    /// linear interpolation was reported as `animation`, whose meaning is "the
    /// model is delivered at rest", though the clip is kept; and dropped
    /// tangents as `normals`, though the normals are kept and readers derive
    /// tangents (glTF 2.0 §3.7.2.1: MikkTSpace). FBX and Collada resample the
    /// same way and report nothing; no container reports tangents.
    #[test]
    fn resampling_and_derived_tangents_are_not_reported_as_losses() {
        let mut scene = fixtures::skinned();
        let p = &mut scene.meshes[0].primitives[0];
        p.tangents = Some(vec![[1.0, 0.0, 0.0, 1.0]; p.positions.len()]);
        for to in [MeshFormat::Usdz, MeshFormat::Usda, MeshFormat::Usdc] {
            let w = write_scene(&scene, to, WriteOptions::default()).unwrap();
            let kinds: Vec<LossKind> = w.losses.iter().map(|l| l.kind).collect();
            assert!(!kinds.contains(&LossKind::Animation), "{to:?}: {:?}", w.losses);
            assert!(!kinds.contains(&LossKind::Normals), "{to:?}: {:?}", w.losses);
        }
    }

    // ── lights and cameras ──────────────────────────────────────────────────

    /// The ways USD is written: a USDZ, and a USDA as a package and as one
    /// file.
    const USD_WAYS: [(MeshFormat, bool); 3] = [(MeshFormat::Usdz, true), (MeshFormat::Usda, false), (MeshFormat::Usda, true)];

    /// A way of [`USD_WAYS`] named for failures, with its write options.
    fn way(fmt: MeshFormat, single: bool) -> (String, WriteOptions) {
        (format!("{fmt}{}", if single { " single" } else { " package" }), WriteOptions { self_contained: single, ..Default::default() })
    }

    /// The root layer of a written USD package as text: a USDA's first
    /// file, or a USDZ's first entry.
    fn root_layer(fmt: MeshFormat, files: &[PackageFile]) -> String {
        let bytes = &files[0].bytes;
        let layer = if fmt == MeshFormat::Usdz {
            let archive = zip::Archive::parse(bytes).unwrap();
            archive.read(&archive.entries[0]).unwrap()
        } else {
            bytes.clone()
        };
        String::from_utf8(layer).unwrap()
    }

    /// `layer` without the `dictionary litegen = { … }` entries of its prims'
    /// customData: what a tool that does not know them leaves, and what a
    /// reader of the UsdLux and UsdGeomCamera attributes alone sees.
    fn without_litegen_data(layer: &str) -> String {
        let (mut out, mut rest) = (String::new(), layer);
        while let Some(start) = rest.find("dictionary litegen = {") {
            let open = start + rest[start..].find('{').unwrap();
            let (mut depth, mut end) = (0usize, rest.len());
            for (i, ch) in rest[open..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            out.push_str(&rest[..start]);
            rest = &rest[end..];
        }
        out.push_str(rest);
        out
    }

    /// `files`, a written USD package, with its root layer replaced by
    /// `layer`: a USDZ stored again, its other entries after the layer.
    fn with_root_layer(fmt: MeshFormat, files: &[PackageFile], layer: &str) -> Vec<PackageFile> {
        if fmt != MeshFormat::Usdz {
            let mut out = files.to_vec();
            out[0].bytes = layer.as_bytes().to_vec();
            return out;
        }
        let archive = zip::Archive::parse(&files[0].bytes).unwrap();
        let mut w = zip::Writer::new();
        w.add(&archive.entries[0].name, layer.as_bytes(), zip::Store::Stored { align: 64 }).unwrap();
        for e in &archive.entries[1..] {
            w.add(&e.name, &archive.read(e).unwrap(), zip::Store::Stored { align: 64 }).unwrap();
        }
        vec![PackageFile { path: files[0].path.clone(), bytes: w.finish() }]
    }

    /// `files` read as `fmt`, with its sidecars beside it.
    fn read_files(fmt: MeshFormat, files: &[PackageFile]) -> SceneRead {
        read_scene(&files[0].bytes, fmt, &MemorySidecars { files }).unwrap_or_else(|e| panic!("read {fmt}: {e}"))
    }

    /// The lights and cameras each node of `s` places, by node name: what a
    /// round trip must give back value for value.
    fn placed(s: &Scene) -> Vec<(String, Option<Light>, Option<Camera>)> {
        let mut v: Vec<(String, Option<Light>, Option<Camera>)> = s
            .nodes
            .iter()
            .filter(|n| n.light.is_some() || n.camera.is_some())
            .map(|n| (n.name.clone().unwrap_or_default(), n.light.map(|l| s.lights[l].clone()), n.camera.map(|c| s.cameras[c].clone())))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// The light the node named `node` places, if it places one.
    fn light_on<'s>(s: &'s Scene, node: &str) -> Option<&'s Light> {
        let n = s.nodes.iter().find(|n| n.name.as_deref() == Some(node))?;
        n.light.map(|l| &s.lights[l])
    }

    /// The camera the node named `node` places, if it places one.
    fn camera_on<'s>(s: &'s Scene, node: &str) -> Option<&'s Camera> {
        let n = s.nodes.iter().find(|n| n.name.as_deref() == Some(node))?;
        n.camera.map(|c| &s.cameras[c])
    }

    fn near3(a: [f64; 3], b: [f64; 3], eps: f64) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() <= eps)
    }

    /// The warnings of `r` that are about a light or a camera.
    fn about_lights_and_cameras(r: &SceneRead) -> Vec<&String> {
        r.warnings.iter().filter(|w| ["light", "Light", "camera", "Camera"].iter().any(|k| w.contains(k))).collect()
    }

    /// `staged()` through USD comes back exactly, through the `litegen`
    /// dictionary in each light and camera prim's customData: every light
    /// and camera at rest and at every sample time (`lightcam`, 1e-5), and
    /// each value as the prepared scene has it, names included, in a USDZ
    /// and in a USDA package and single file, with nothing about one warned
    /// on the way back. Only the bulb's range is reported lost (Ruling R27):
    /// UsdLux has no range, so other readers light past it, and only the
    /// customData keeps it.
    #[test]
    fn every_light_and_camera_round_trips_through_usd() {
        let fixture = fixtures::staged();
        for (fmt, single) in USD_WAYS {
            let (ctx, opts) = way(fmt, single);
            let w = write_scene(&fixture, fmt, opts).unwrap();
            let lost: Vec<&Loss> = w.losses.iter().filter(|l| matches!(l.kind, LossKind::Lights | LossKind::Cameras)).collect();
            let named: Vec<(LossKind, &str)> = lost.iter().map(|l| (l.kind, l.what.as_str())).collect();
            assert_eq!(named, [(LossKind::Lights, "the range of 1 light(s) [bulb]")], "{ctx}: {lost:?}");
            let why = format!("{fmt} has no light range");
            assert!(lost[0].why.starts_with(&why) && lost[0].why.contains("customData"), "{ctx}: {lost:?}");
            let layer = root_layer(fmt, &w.package.files);
            assert_eq!(layer.matches("dictionary litegen = {").count(), 5, "{ctx}: the litegen customData of each of the three lights and two cameras");
            let r = read_files(fmt, &w.package.files);
            assert!(about_lights_and_cameras(&r).is_empty(), "{ctx}: {:?}", r.warnings);
            let want = fmt.prepare(&fixture, opts).unwrap();
            lightcam::assert_same_lights_and_cameras(&want, &r.scene, 1e-5, &ctx);
            assert_eq!(placed(&r.scene), placed(&want), "{ctx}: every value exactly");
        }
    }

    /// litegen's own numbers in a light's or camera's customData (a `litegen`
    /// dictionary) are declared `double`/`double3`, never `float`, `float3` or
    /// `color3f`: a `Layer`'s dictionary value cannot tell a float from a
    /// double, so a crate holds them as doubles, and the text has to say the
    /// same for `usdcat` to print the same for both (Ruling U15). Readers take
    /// them as f64 either way, so no reading changes. `staged` has 17: a
    /// perspective camera's four, an orthographic one's four, the sun's two,
    /// the bulb's three and the spot's four.
    #[test]
    fn litegens_own_numbers_are_written_as_doubles() {
        let staged = fixtures::staged();
        for (fmt, single) in USD_WAYS {
            let (ctx, opts) = way(fmt, single);
            let layer = root_layer(fmt, &write_scene(&staged, fmt, opts).unwrap().package.files);
            let mut numeric: Vec<String> = Vec::new();
            let mut rest = layer.as_str();
            while let Some(start) = rest.find("dictionary litegen = {") {
                // The entries are one line each: no braces inside the dictionary.
                let open = start + rest[start..].find('{').unwrap();
                let close = open + rest[open..].find('}').unwrap();
                for line in rest[open + 1..close].lines().map(str::trim).filter(|l| !l.is_empty()) {
                    let mut words = line.split_whitespace();
                    let (declared, key) = (words.next().unwrap(), words.next().unwrap_or_default());
                    if !["token", "string"].contains(&declared) {
                        numeric.push(format!("{declared} {key}"));
                    }
                }
                rest = &rest[close + 1..];
            }
            let not_doubles: Vec<&String> = numeric.iter().filter(|e| !(e.starts_with("double ") || e.starts_with("double3 "))).collect();
            assert!(not_doubles.is_empty(), "{ctx}: litegen entries not declared double or double3: {not_doubles:?}");
            assert_eq!(numeric.len(), 17, "{ctx}: the numeric entries of the five dictionaries: {numeric:?}");
        }
    }

    /// The SkelAnimation's `half3[] scales`, its rest line and every sample,
    /// are half-exact when written: Sdf stores a `half3[]` as halves, so any
    /// other value is one no reader sees (Ruling U13b). `prepare` rounds an
    /// animated skeleton's scale keys (`quantize_joint_scales`), but the
    /// writer samples every joint at the union of all channels' key times and
    /// interpolates between those keys, which gives values between two
    /// halves; it rounds again as it prints. The skinned fixture had three
    /// such values (1.2495898 where a half holds 1.25).
    #[test]
    fn skeletal_scales_are_written_half_exact() {
        use super::super::usd_stage::Prop;
        let skinned = fixtures::skinned();
        for (fmt, single) in USD_WAYS {
            let (ctx, opts) = way(fmt, single);
            let text = root_layer(fmt, &write_scene(&skinned, fmt, opts).unwrap().package.files);
            let layer = super::super::usda::parse(text.as_bytes(), "model.usda").unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let (mut seen, mut off) = (0usize, Vec::new());
            for (path, spec) in &layer.specs {
                for (name, prop) in &spec.props {
                    let Prop::Attr(a) = prop else { continue };
                    if name != "scales" || a.type_name != "half3[]" {
                        continue;
                    }
                    let values = a.default.iter().map(|v| (None, v)).chain(a.samples.iter().flatten().map(|(t, v)| (Some(*t), v)));
                    for (time, v) in values {
                        let Value::Floats(floats) = v else { panic!("{ctx}: {path}.{name} holds {v:?}") };
                        seen += floats.len();
                        off.extend(floats.iter().filter(|x| to_half_precision(**x) != **x).map(|x| format!("{path}.{name} at {time:?}: {x}")));
                    }
                }
            }
            assert!(seen > 90, "{ctx}: the skeleton animation's scales were found ({seen} values)");
            assert!(off.is_empty(), "{ctx}: {} of {seen} scale values are not half precision: {off:?}", off.len());
        }
    }

    /// The same files without their litegen customData read by the
    /// documented mapping (usd_read.rs, "Lights and cameras"): each light's
    /// `inputs:color` and `inputs:intensity`, a normalised DistantLight's in
    /// lux and a normalised SphereLight's as four times its candela (Ruling
    /// R6), so staged's intensities come back within 1e-3 relatively (here
    /// exactly: ×4 and ÷4 are exact in binary); a spot's cones from its cone
    /// angle and softness; no range, which UsdLux has none of; and every
    /// camera from its focal length, apertures and clipping range. The world
    /// frames are untouched (`lightcam`, 1e-5, against the fixture with no
    /// ranges), and nothing about a light or camera is warned.
    #[test]
    fn without_the_litegen_data_lights_and_cameras_follow_the_documented_mapping() {
        let fixture = fixtures::staged();
        for (fmt, single) in USD_WAYS {
            let (ctx, opts) = way(fmt, single);
            let w = write_scene(&fixture, fmt, opts).unwrap();
            let full = root_layer(fmt, &w.package.files);
            assert_eq!(full.matches("dictionary litegen = {").count(), 5, "{ctx}: the written file carries the five dictionaries");
            let bare = without_litegen_data(&full);
            let left: Vec<&str> = bare.lines().filter(|l| l.contains("dictionary litegen")).collect();
            assert!(left.is_empty(), "{ctx}: all stripped: {left:?}");
            let r = read_files(fmt, &with_root_layer(fmt, &w.package.files, &bare));
            assert!(about_lights_and_cameras(&r).is_empty(), "{ctx}: {:?}", r.warnings);
            let mut want = fmt.prepare(&fixture, opts).unwrap();
            for l in want.lights.iter_mut() {
                l.range = None;
            }
            lightcam::assert_same_lights_and_cameras(&want, &r.scene, 1e-5, &format!("{ctx} without the litegen data"));
            for l in &fixture.lights {
                let node = format!("{}_node", l.name.as_deref().unwrap());
                let got = light_on(&r.scene, &node).unwrap_or_else(|| panic!("{ctx}: no light on {node}"));
                assert!(((got.intensity - l.intensity) / l.intensity).abs() <= 1e-3, "{ctx} {node}: intensity {} against {}", got.intensity, l.intensity);
            }
        }
    }

    // ── light units (`usd_light_units`) ─────────────────────────────────────

    /// FNV-1a 64, to pin a whole file's bytes.
    fn fnv1a64(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf29ce484222325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100000001b3))
    }

    /// `staged`'s three light prims as `usd_photometric_bytes_are_unchanged`
    /// pins them, written with the default options: each prim's name, its
    /// customData and its UsdLux attributes, without its transform ops
    /// ([`light_prims`]). The values are the ones the USD writer's docs
    /// give: normalised, radius 0, intensity 3 (lux), 4 × 20 and 4 × 40. The
    /// litegen dictionaries' numbers are `double`s (Ruling U15, 2026-11-10).
    const STAGED_LIGHT_PRIMS: &str = r#"def DistantLight "sun_node" (
    customData = {
        dictionary litegen = {
            token type = "directional"
            string name = "sun"
            double3 color = (1, 0.875, 0.75)
            double intensity = 3
        }
    }
)
{
    color3f inputs:color = (1, 0.875, 0.75)
    float inputs:intensity = 3
    bool inputs:normalize = 1
    float inputs:angle = 0
}
def SphereLight "bulb_node" (
    customData = {
        dictionary litegen = {
            token type = "point"
            string name = "bulb"
            double3 color = (1, 0.75, 0.5)
            double intensity = 20
            double range = 10
        }
    }
)
{
    color3f inputs:color = (1, 0.75, 0.5)
    float inputs:intensity = 80
    bool inputs:normalize = 1
    float inputs:radius = 0
    bool treatAsPoint = 1
}
def SphereLight "spot_node" (
    prepend apiSchemas = ["ShapingAPI"]
    customData = {
        dictionary litegen = {
            token type = "spot"
            string name = "spot"
            double3 color = (0.5, 0.75, 1)
            double intensity = 40
            double innerConeAngle = 0.2
            double outerConeAngle = 0.6
        }
    }
)
{
    color3f inputs:color = (0.5, 0.75, 1)
    float inputs:intensity = 160
    bool inputs:normalize = 1
    float inputs:radius = 0
    bool treatAsPoint = 1
    float inputs:shaping:cone:angle = 34.37747
    float inputs:shaping:cone:softness = 0.6666667
}"#;

    /// Each light prim of `layer` (a `DistantLight` or a `SphereLight`) as
    /// its name and its text, one line per line, dedented, without the
    /// lines of its transform ops: those go through libm trig (a node's
    /// orient is built with `atan2` and `asin`), which rounds the last bit
    /// differently on macOS and on Linux, so a pin of them would break on
    /// CI (Ruling L7).
    fn light_prims(layer: &str) -> Vec<(String, String)> {
        let lines: Vec<&str> = layer.lines().collect();
        let mut out = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let def = line.trim_start();
            if !(def.starts_with("def DistantLight ") || def.starts_with("def SphereLight ")) {
                continue;
            }
            let indent = &line[..line.len() - def.len()];
            let close = format!("{indent}}}");
            let end = (i + 1..lines.len()).find(|j| lines[*j] == close).unwrap_or_else(|| panic!("the prim at line {i} never closes"));
            let name = def.split('"').nth(1).unwrap().to_string();
            let text: Vec<&str> = lines[i..=end].iter().filter(|l| !l.contains("xformOp")).map(|l| &l[indent.len()..]).collect();
            out.push((name, text.join("\n")));
        }
        out
    }

    /// The `inputs:intensity` each light prim of `layer` is written with,
    /// by prim name, as the f32 the file holds.
    fn light_prim_intensities(layer: &str) -> Vec<(String, f32)> {
        light_prims(layer)
            .into_iter()
            .map(|(name, text)| {
                let line = text.lines().find_map(|l| l.trim().strip_prefix("float inputs:intensity = ")).unwrap_or_else(|| panic!("{name}: no inputs:intensity"));
                (name, line.parse::<f32>().unwrap_or_else(|e| panic!("{line}: {e}")))
            })
            .collect()
    }

    /// `scene` written as `fmt` (a USDZ, or a USDA as a package or one file)
    /// with its USD lights in `units`, and everything else the default.
    fn write_in(scene: &Scene, fmt: MeshFormat, single: bool, units: LightUnits) -> Vec<PackageFile> {
        let (_, opts) = way(fmt, single);
        write_scene(scene, fmt, WriteOptions { usd_light_units: units, ..opts }).unwrap_or_else(|e| panic!("write: {e}")).package.files
    }

    /// `files`, a written USD package, read with its USD lights in `units`.
    fn read_in(files: &[PackageFile], fmt: MeshFormat, units: LightUnits) -> SceneRead {
        let opts = ReadOptions { usd_light_units: units, ..Default::default() };
        read_scene_with(&files[0].bytes, fmt, &MemorySidecars { files }, opts).unwrap_or_else(|e| panic!("read {fmt}: {e}"))
    }

    /// The stage `body` (see [`usda`]) with a triangle to draw, read as USDA
    /// with its lights in `units`.
    fn read_stage_in(body: &str, units: LightUnits) -> SceneRead {
        let text = usda(&format!("def Mesh \"tri\" {{ {QUAD} }}\n{body}"));
        let opts = ReadOptions { usd_light_units: units, ..Default::default() };
        read_scene_with(text.as_bytes(), MeshFormat::Usda, &NoSidecars, opts).unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    /// `got` against `want`, within `rel` of `want`.
    fn assert_close_rel(got: f64, want: f64, rel: f64, ctx: &str) {
        assert!((got - want).abs() <= rel * want.abs(), "{ctx}: {got} against {want}");
    }

    /// The light-units option's default is today's output, byte for byte,
    /// pinned as far as it can hold on every platform (Ruling L7):
    /// - `fixtures::cube()` (no lights, no rotation, and normals made with
    ///   `sqrt` and division only, which IEEE rounds the same everywhere;
    ///   no number in its file comes from libm trig): its whole USDA, and
    ///   its whole USDZ (every zip entry is stored with the fixed 1980
    ///   timestamp, `zip::DOS_DATE`, so the package is deterministic),
    ///   written with the default options and with an explicit
    ///   `LightUnits::Photometric`, hash to the FNV-1a 64 pinned here;
    /// - `fixtures::staged()` (three lights): its nodes' orients go through
    ///   libm trig (`atan2` and `asin`), and its cameras through `tan` and
    ///   `atan`, which round the last bit differently on macOS and on
    ///   Linux (CI), so its whole file is not pinned. Each light prim's
    ///   customData and attributes are ([`STAGED_LIGHT_PRIMS`]), as text:
    ///   `inputs:intensity`, `inputs:color`, the cone and its softness,
    ///   `inputs:normalize`, the radius, and the litegen dictionary. The
    ///   whole files written by the default options and by an explicit
    ///   `Photometric` are compared with each other in the same run.
    ///
    /// The constants were computed on 2026-11-08, before the option had any
    /// effect, from the writer as item 10 left it.
    #[test]
    fn usd_photometric_bytes_are_unchanged() {
        const CUBE_USDA_FNV1A: u64 = 0x01ff8132d578084c;
        const CUBE_USDZ_FNV1A: u64 = 0xa1f73d9b227c2212;
        let cube = fixtures::cube();
        assert!(cube.lights.is_empty());
        for (fmt, single) in USD_WAYS {
            let (ctx, opts) = way(fmt, single);
            let pinned = if fmt == MeshFormat::Usdz { CUBE_USDZ_FNV1A } else { CUBE_USDA_FNV1A };
            let default = write_scene(&cube, fmt, opts).unwrap().package.files;
            let explicit = write_in(&cube, fmt, single, LightUnits::Photometric);
            for (which, files) in [("the default options", &default), ("an explicit Photometric", &explicit)] {
                assert_eq!(files.len(), 1, "{ctx}: one file");
                assert_eq!(format!("{ctx}: {:#018x}", fnv1a64(&files[0].bytes)), format!("{ctx}: {pinned:#018x}"), "cube with {which}");
            }
        }
        let staged = fixtures::staged();
        for (fmt, single) in USD_WAYS {
            let (ctx, opts) = way(fmt, single);
            let default = write_scene(&staged, fmt, opts).unwrap().package.files;
            let explicit = write_in(&staged, fmt, single, LightUnits::Photometric);
            assert!(default == explicit, "{ctx}: staged with the default options is staged with an explicit Photometric, byte for byte");
            let prims: Vec<String> = light_prims(&root_layer(fmt, &default)).into_iter().map(|(_, text)| text).collect();
            assert_eq!(prims.join("\n"), STAGED_LIGHT_PRIMS, "{ctx}: staged's light prims");
        }
    }

    // ── several clips (`usd_clips`) ─────────────────────────────────────────

    /// The ways `fixtures::clips()` is written: a USDZ, and a USDA and a USDC
    /// each as a package and as one file.
    const CLIP_WAYS: [(MeshFormat, bool); 5] = [
        (MeshFormat::Usdz, true),
        (MeshFormat::Usda, false),
        (MeshFormat::Usda, true),
        (MeshFormat::Usdc, false),
        (MeshFormat::Usdc, true),
    ];

    fn clip_opts(single: bool, clips: Option<UsdClips>) -> WriteOptions {
        WriteOptions { self_contained: single, usd_clips: clips, ..Default::default() }
    }

    fn clip_ctx(fmt: MeshFormat, single: bool) -> String {
        format!("{fmt}{}", if single { " single" } else { " package" })
    }

    /// The root layer of a written USD package, parsed: text or crate, in a
    /// USDZ or beside its textures.
    fn layer_of(fmt: MeshFormat, files: &[PackageFile]) -> Layer {
        let bytes = if fmt == MeshFormat::Usdz {
            let archive = zip::Archive::parse(&files[0].bytes).unwrap();
            archive.read(&archive.entries[0]).unwrap()
        } else {
            files[0].bytes.clone()
        };
        parse_layer(&bytes, "model").unwrap_or_else(|e| panic!("{fmt}: {e}"))
    }

    /// `scene` with its first clip alone: what a USD file held before it
    /// could hold several.
    fn first_clip_only(scene: &Scene) -> Scene {
        let mut s = scene.clone();
        s.animations.truncate(1);
        s
    }

    /// The world-space positions of everything `scene` draws, with clip
    /// `clip` playing at `t` seconds.
    fn posed(scene: &Scene, clip: usize, t: f32) -> Vec<Vec<[f32; 3]>> {
        pose::world_primitives(scene, &pose::sample_pose(scene, clip, t)).unwrap().into_iter().map(|wp| wp.positions).collect()
    }

    fn clip_names(s: &Scene) -> Vec<Option<String>> {
        s.animations.iter().map(|a| a.name.clone()).collect()
    }

    /// The `litegen.clipEnd` of a SkelAnimation spec, in seconds.
    fn clip_end_of(spec: &super::super::usd_stage::PrimSpec) -> Option<f64> {
        let Some(Value::Dict(entries)) = spec.custom_data.as_ref()?.iter().find(|(k, _)| k == "litegen").map(|(_, v)| v) else { return None };
        entries.iter().find(|(k, _)| k == "clipEnd").and_then(|(_, v)| v.as_f64())
    }

    /// `usd_clips=first` writes the file this writer wrote before it could
    /// hold several clips. The FNV-1a 64 of each file `fixtures::clips()`
    /// wrote then are pinned here, as `bake_is_todays_output` pins Collada.
    /// They were first recorded on 2026-11-10 from the writer as it stood
    /// before the clip layout (the first clip, the rest reported), with the
    /// fixture's prop last in the mesh and material lists; the fixture then
    /// moved the prop to the front of every list (Collada and 3MF number
    /// meshes and materials by first use), and these are the pins of that
    /// order, taken from `first`, which the assertions below show is the
    /// one-clip path (byte for byte the old writer's: the hashes of every
    /// other fixture's files, and of this fixture in its first order, were the
    /// same before and after the clip layout). They run on macOS only: the
    /// fixture's quaternions go through libm's `sin_cos`, which glibc may
    /// round differently in the last bit (Ruling DM2a). Everywhere, the same
    /// files are compared with what the first clip alone writes by default,
    /// which does not depend on the platform's libm; the USDZ default is
    /// `first`; and every fixture that has one clip or none is written
    /// byte for byte the same under any word (none of them has more, so no
    /// existing fixture's output moved).
    #[test]
    fn usd_clips_first_is_todays_output() {
        const TODAYS: [(MeshFormat, bool, &str, u64); 7] = [
            (MeshFormat::Usdz, true, "model.usdz", 0x3fc97e7d5fbc0079),
            (MeshFormat::Usda, false, "model.usda", 0x7336ebbe99c58d22),
            (MeshFormat::Usda, false, "textures/bands.png", 0x7bf71dc9bf715a1b),
            (MeshFormat::Usda, true, "model.usda", 0xae0998e23aa04de7),
            (MeshFormat::Usdc, false, "model.usdc", 0xec2019b830ded461),
            (MeshFormat::Usdc, false, "textures/bands.png", 0x7bf71dc9bf715a1b),
            (MeshFormat::Usdc, true, "model.usdc", 0x1f90235dea5234df),
        ];
        let write = |scene: &Scene, fmt: MeshFormat, single: bool, clips: Option<UsdClips>| -> Vec<PackageFile> {
            write_scene(scene, fmt, clip_opts(single, clips)).unwrap_or_else(|e| panic!("write {fmt}: {e}")).package.files
        };
        let fixture = fixtures::clips();
        assert_eq!(fixture.animations.len(), 3);
        let mut got: Vec<(MeshFormat, bool, String, u64)> = Vec::new();
        for (fmt, single) in CLIP_WAYS {
            let ctx = clip_ctx(fmt, single);
            let first = write(&fixture, fmt, single, Some(UsdClips::First));
            assert_eq!(first, write(&first_clip_only(&fixture), fmt, single, None), "{ctx}: `first` is what the first clip alone writes");
            if fmt == MeshFormat::Usdz {
                assert_eq!(first, write(&fixture, fmt, single, None), "{ctx}: a USDZ writes `first` unless asked for more");
            }
            got.extend(first.iter().map(|f| (fmt, single, f.path.clone(), fnv1a64(&f.bytes))));
        }
        #[cfg(target_os = "macos")]
        {
            let want: Vec<(MeshFormat, bool, String, u64)> = TODAYS.iter().map(|(f, s, p, h)| (*f, *s, p.to_string(), *h)).collect();
            assert_eq!(got, want);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (&got, TODAYS);
        for (name, scene) in fixtures::all().into_iter().filter(|(_, s)| s.animations.len() <= 1) {
            for (fmt, single) in CLIP_WAYS {
                let default = write(&scene, fmt, single, None);
                for word in [UsdClips::First, UsdClips::All] {
                    assert!(default == write(&scene, fmt, single, Some(word)), "{name} {}: `{}` writes what the default does", clip_ctx(fmt, single), word.as_str());
                }
            }
        }
    }

    /// Every clip of `fixtures::clips()` comes back from a USDZ, a USDA and
    /// a USDC written with `all`: three clips, named and ordered as they
    /// were, as long as they were (from `clipEnd`), whose skeletal keys are
    /// equal at every key, between them and past the end of a short clip.
    /// walk's prop motion, outside the variants, is kept; run's is reported
    /// lost by one `animation` loss that names run alone.
    #[test]
    fn usd_keeps_every_clip() {
        let fixture = fixtures::clips();
        for (fmt, single) in CLIP_WAYS {
            let ctx = clip_ctx(fmt, single);
            let opts = clip_opts(single, Some(UsdClips::All));
            let w = write_scene(&fixture, fmt, opts).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let want = fmt.prepare(&fixture, opts).unwrap();
            let r = read_files(fmt, &w.package.files);
            let got = &r.scene;
            assert_eq!(
                clip_names(got),
                ["walk", "run", "wave"].map(|n| Some(n.to_string())),
                "{ctx}: the clips, named and in the order they were"
            );
            assert_eq!(clip_names(&want), clip_names(got), "{ctx}: prepared names");
            let ends = |s: &Scene| -> Vec<Option<(f32, f32)>> { s.animations.iter().map(|a| a.time_range()).collect() };
            assert_eq!(ends(got), [Some((0.0, 1.0)), Some((0.0, 0.6)), Some((0.0, 2.0))], "{ctx}: durations");
            assert_eq!(ends(&want), ends(got), "{ctx}: prepared durations");
            assert_eq!(names(got), names(&want), "{ctx}: nodes");
            let want = scales_as_the_writer_samples(&want);
            for clip in 0..3 {
                let mut keys: Vec<f32> = want.animations[clip].channels.iter().flat_map(|c| c.times.iter().copied()).collect();
                keys.sort_by(|a, b| a.partial_cmp(b).unwrap());
                keys.dedup();
                let mut times = keys.clone();
                times.extend(keys.windows(2).map(|k| (k[0] + k[1]) / 2.0));
                // A short clip holds its last pose.
                times.push(keys[keys.len() - 1] + 0.5);
                for t in times {
                    let (a, b) = (posed(&want, clip, t), posed(got, clip, t));
                    assert_eq!(a.len(), b.len(), "{ctx}: clip {clip} @ {t}: drawn primitives");
                    for (x, y) in a.iter().zip(&b) {
                        if let Some(e) = close3(x, y, 2e-5) {
                            panic!("{ctx}: clip {clip} @ {t}: positions differ: {e}");
                        }
                    }
                }
            }
            // Rigid motion: walk's prop is kept, run's is not (and was not
            // asked to be: wave has none).
            let prop = got.nodes.iter().position(|n| n.name.as_deref() == Some("prop_node")).unwrap();
            let on_prop = |clip: usize| got.animations[clip].channels.iter().filter(|c| c.node == prop).count();
            assert_eq!((on_prop(0), on_prop(1), on_prop(2)), (1, 0, 0), "{ctx}: the prop moves in walk only");
            let lost: Vec<String> = w.losses.iter().filter(|l| l.kind == LossKind::Animation).map(|l| l.to_string()).collect();
            assert_eq!(lost.len(), 1, "{ctx}: {lost:?}");
            assert!(lost[0].contains("run") && !lost[0].contains("walk") && !lost[0].contains("wave"), "{ctx}: {lost:?}");
            assert!(lost[0].contains("switches skeleton bindings between clips, not transforms"), "{ctx}: {lost:?}");
        }
    }

    /// The written layout is Apple's (`variant_combiner.py`): three
    /// SkelAnimation prims outside the variant set, the variant set
    /// `Animation` on the default prim with the first clip selected, and in
    /// each variant `skel:animationSource` and nothing else, with
    /// `skel:animationSource` nowhere outside them (a local opinion would beat
    /// every variant's). walk's rigid xformOps stay where a one-clip file has
    /// them, `endTimeCode` is the longest clip's end, and each SkelAnimation
    /// records its own end.
    #[test]
    fn the_layout_is_apples() {
        use super::super::usd_stage::Prop;
        let fixture = fixtures::clips();
        for (fmt, single) in CLIP_WAYS {
            let ctx = clip_ctx(fmt, single);
            let files = write_scene(&fixture, fmt, clip_opts(single, Some(UsdClips::All))).unwrap().package.files;
            let layer = layer_of(fmt, &files);
            let one_clip = layer_of(fmt, &write_scene(&fixture, fmt, clip_opts(single, Some(UsdClips::First))).unwrap().package.files);
            if fmt != MeshFormat::Usdc {
                let text = root_layer(fmt, &files);
                assert!(text.contains("prepend variantSets = \"Animation\""), "{ctx}: {text}");
                assert!(text.contains("string Animation = \"walk\""), "{ctx}: the selection");
            }
            assert_eq!(layer.meta_str("defaultPrim"), Some("clips"), "{ctx}");
            let root = &layer.specs["/clips"];
            assert_eq!(root.variant_set_names.as_ref().map(|op| op.apply(&[])), Some(vec!["Animation".to_string()]), "{ctx}");
            assert_eq!(root.variant_selection, [("Animation".to_string(), "walk".to_string())], "{ctx}: the first clip is selected");
            let variants = vec!["walk".to_string(), "run".to_string(), "wave".to_string()];
            assert_eq!(root.variant_sets, [("Animation".to_string(), variants)], "{ctx}: one variant a clip, in order");
            // One SkelAnimation a clip, none inside a variant.
            let mut anims: Vec<&str> = layer.specs.iter().filter(|(_, s)| s.type_name == "SkelAnimation").map(|(p, _)| p.as_str()).collect();
            anims.sort();
            assert_eq!(anims, ["/clips/leg/run", "/clips/leg/walk", "/clips/leg/wave"], "{ctx}");
            // The variants author skel:animationSource and nothing else; nothing
            // else does.
            for (path, spec) in &layer.specs {
                if path.contains('{') {
                    let props: Vec<&str> = spec.props.iter().map(|(n, _)| n.as_str()).collect();
                    assert!(props.iter().all(|n| *n == "skel:animationSource"), "{ctx}: {path} authors {props:?}");
                    assert!(spec.type_name.is_empty(), "{ctx}: {path} is a {}", spec.type_name);
                } else {
                    assert!(spec.prop("skel:animationSource").is_none(), "{ctx}: {path} binds an animation outside the variants");
                }
            }
            for clip in ["walk", "run", "wave"] {
                let path = format!("/clips{{Animation={clip}}}leg");
                let Some(Prop::Rel(rel)) = layer.specs.get(&path).and_then(|s| s.prop("skel:animationSource")) else {
                    panic!("{ctx}: variant {clip} does not bind the skeleton ({path})");
                };
                assert_eq!(rel.targets.as_ref().map(|t| t.apply(&[])), Some(vec![format!("/clips/leg/{clip}")]), "{ctx}: variant {clip}");
            }
            // Rigid motion stays as a one-clip file has it.
            let prop = &layer.specs["/clips/prop_node"];
            assert_eq!(prop, &one_clip.specs["/clips/prop_node"], "{ctx}: walk's prop motion is written as ever");
            assert!(prop.attr("xformOp:translate").is_some_and(|a| a.samples.is_some()), "{ctx}: and it is walk's");
            // One timeline, as long as the longest clip, and each clip's own end.
            assert_eq!(layer.meta_f64("endTimeCode"), Some(60.0), "{ctx}: endTimeCode is wave's 2 s");
            let end = |clip: &str| clip_end_of(&layer.specs[&format!("/clips/leg/{clip}")]).map(|v| v as f32);
            assert_eq!([end("walk"), end("run"), end("wave")], [Some(1.0), Some(0.6), Some(2.0)], "{ctx}: clipEnd");
        }
    }

    /// Clips with rigid motion alone have no skeleton to switch, so a scene
    /// of them writes as ever: the first clip, the other reported, and no
    /// variant set. It guards the layout from growing variants nothing
    /// switches.
    #[test]
    fn an_all_rigid_multi_clip_scene_writes_as_today() {
        let mut scene = fixtures::wire();
        scene.animations.push(Animation {
            name: Some("Spin".into()),
            channels: vec![Channel {
                node: 0,
                path: AnimPath::Rotation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.5],
                values: vec![0.0, 0.0, 0.0, 1.0, 0.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, std::f32::consts::FRAC_1_SQRT_2],
            }],
        });
        for (fmt, single) in CLIP_WAYS {
            let ctx = clip_ctx(fmt, single);
            let all = write_scene(&scene, fmt, clip_opts(single, Some(UsdClips::All))).unwrap();
            let first = write_scene(&scene, fmt, clip_opts(single, Some(UsdClips::First))).unwrap();
            assert!(all.package.files == first.package.files, "{ctx}: `all` writes what `first` does");
            assert_eq!(all.losses, first.losses, "{ctx}: and reports what it does");
            let layer = layer_of(fmt, &all.package.files);
            assert!(layer.specs.values().all(|s| s.variant_sets.is_empty() && s.variant_set_names.is_none()), "{ctx}: no variant set");
            let lost: Vec<String> = all.losses.iter().filter(|l| l.kind == LossKind::Animation).map(|l| l.to_string()).collect();
            assert_eq!(lost.len(), 1, "{ctx}: {lost:?}");
            assert!(lost[0].contains("Spin"), "{ctx}: the other clip is named: {lost:?}");
        }
    }

    /// A layer whose default prim is a SkelRoot with one skinned joint and `n`
    /// SkelAnimations `a0` … `a{n-1}`; `a{k}` lifts the joint by `k + 1` over
    /// a second (24 time codes). With `Some(s)` the SkelRoot has the variant
    /// set `Animation` of Apple's layout: a variant `c{k}` for each, switching
    /// the skeleton to `a{k}`, with `c{s}` selected. With `None` the skeleton
    /// binds `a0` itself and there is no variant set. `extra` is more of the
    /// SkelRoot's body.
    fn animation_rig(n: usize, selected: Option<usize>, extra: &str) -> String {
        let id = "((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))";
        let mut t = String::from("#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    timeCodesPerSecond = 24\n    upAxis = \"Y\"\n)\n");
        t.push_str("def SkelRoot \"R\"");
        if let Some(s) = selected {
            t.push_str(&format!(" (\n    prepend variantSets = \"Animation\"\n    variants = {{ string Animation = \"c{s}\" }}\n)"));
        }
        t.push_str("\n{\n");
        if selected.is_some() {
            t.push_str("    variantSet \"Animation\" = {\n");
            for k in 0..n {
                t.push_str(&format!("        \"c{k}\" {{ over \"Skel\" {{ rel skel:animationSource = </R/Skel/a{k}> }} }}\n"));
            }
            t.push_str("    }\n");
        }
        t.push_str("    def Skeleton \"Skel\" (prepend apiSchemas = [\"SkelBindingAPI\"])\n    {\n        uniform token[] joints = [\"a\"]\n");
        t.push_str(&format!("        uniform matrix4d[] bindTransforms = [{id}]\n        uniform matrix4d[] restTransforms = [{id}]\n"));
        if selected.is_none() {
            t.push_str("        rel skel:animationSource = </R/Skel/a0>\n");
        }
        for k in 0..n {
            t.push_str(&format!(
                "        def SkelAnimation \"a{k}\"\n        {{\n            uniform token[] joints = [\"a\"]\n            float3[] translations.timeSamples = {{ 0: [(0, 0, 0)], 24: [(0, {}, 0)] }}\n            quatf[] rotations = [(1, 0, 0, 0)]\n            half3[] scales = [(1, 1, 1)]\n        }}\n",
                k + 1
            ));
        }
        t.push_str("    }\n    def Mesh \"M\" (prepend apiSchemas = [\"SkelBindingAPI\"])\n    {\n");
        t.push_str("        int[] faceVertexCounts = [3]\n        int[] faceVertexIndices = [0, 1, 2]\n        point3f[] points = [(1, 0, 0), (2, 0, 0), (1, 1, 0)]\n");
        t.push_str("        int[] primvars:skel:jointIndices = [0] (elementSize = 1\n            interpolation = \"constant\")\n");
        t.push_str("        float[] primvars:skel:jointWeights = [1] (elementSize = 1\n            interpolation = \"constant\")\n");
        t.push_str("        rel skel:skeleton = </R/Skel>\n    }\n");
        t.push_str(extra);
        t.push_str("\n}\n");
        t
    }

    fn read_rig(text: &str) -> SceneRead {
        read_usda(text.as_bytes(), &NoSidecars).unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    /// Where the rig's mesh has its first point in clip `clip` after `secs`.
    fn rig_point(r: &SceneRead, clip: usize, secs: f32) -> [f32; 3] {
        posed(&r.scene, clip, secs)[0][0]
    }

    /// The reader recognises Apple's convention, the default prim's variant
    /// set `Animation` whose variants switch `skel:animationSource`, and only
    /// that. A `look` variant set, an `Animation` set that switches something
    /// else, and several SkelAnimations of which one is bound all read as one
    /// clip: the one the stage shows.
    #[test]
    fn other_variant_sets_read_as_today() {
        let moving = |set: &str, selected: &str, a: &str, b: &str| {
            format!(
                "#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    timeCodesPerSecond = 24\n    upAxis = \"Y\"\n)\n\
                 def Xform \"R\" (\n    prepend variantSets = \"{set}\"\n    variants = {{ string {set} = \"{selected}\" }}\n)\n{{\n    variantSet \"{set}\" = {{\n\
                 \"{a}\" {{ double3 xformOp:translate.timeSamples = {{ 0: (0, 0, 0), 24: (10, 0, 0) }}\n uniform token[] xformOpOrder = [\"xformOp:translate\"] }}\n\
                 \"{b}\" {{ double3 xformOp:translate.timeSamples = {{ 0: (0, 0, 0), 24: (0, 10, 0) }}\n uniform token[] xformOpOrder = [\"xformOp:translate\"] }}\n    }}\n    {TRI}\n}}\n"
            )
        };
        // A set of another name, and one called Animation that switches no skeleton.
        for (set, a, b) in [("look", "red", "blue"), ("Animation", "slow", "fast")] {
            let r = read_rig(&moving(set, a, a, b));
            assert_eq!(r.scene.animations.len(), 1, "{set}: one clip, the selection's");
            assert_eq!(first_point_at(&r, 1.0), [11.0, 0.0, 0.0], "{set}: the selected variant moves it");
            let r = read_rig(&moving(set, b, a, b));
            assert_eq!(first_point_at(&r, 1.0), [1.0, 10.0, 0.0], "{set}: and the other selection moves it the other way");
        }
        // Two SkelAnimations, one bound: the bound one is the clip.
        let r = read_rig(&animation_rig(2, None, ""));
        assert_eq!(clip_names(&r.scene), [Some("a0".to_string())]);
        assert_eq!(rig_point(&r, 0, 1.0), [1.0, 1.0, 0.0]);
    }

    /// Apple's layout is read as one clip a variant: the authored selection
    /// first, then the rest in variant order, each named by the SkelAnimation
    /// its variant binds.
    #[test]
    fn the_reader_takes_the_authored_selection_first_then_the_rest_in_variant_order() {
        let r = read_rig(&animation_rig(3, Some(1), ""));
        assert_eq!(clip_names(&r.scene), ["a1", "a0", "a2"].map(|n| Some(n.to_string())));
        assert_eq!(
            [rig_point(&r, 0, 1.0), rig_point(&r, 1, 1.0), rig_point(&r, 2, 1.0)],
            [[1.0, 2.0, 0.0], [1.0, 1.0, 0.0], [1.0, 3.0, 0.0]],
            "each clip moves the joint as its SkelAnimation does"
        );
    }

    /// A name a list holds twice goes to its first place, in every clip: a
    /// skeleton listing the joint "a" twice is posed through the first, by the
    /// selected clip and by one composed after it, and so is a blend shape
    /// "s" a SkelAnimation lists twice. The reader looks the names up in a
    /// table built once for each clip; the table keeps the first of two alike,
    /// as a scan of the list does.
    #[test]
    fn a_name_a_list_holds_twice_goes_to_its_first_place_in_every_clip() {
        let id = "((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))";
        let mut text = animation_rig(2, Some(0), "");
        let mut swap = |from: String, to: String| {
            assert!(text.matches(&from).count() == 1, "one place to change: {from:?}");
            text = text.replace(&from, &to);
        };
        // Two joints named "a", so the animations' "a" has two places.
        swap(
            format!("uniform token[] joints = [\"a\"]\n        uniform matrix4d[] bindTransforms = [{id}]\n        uniform matrix4d[] restTransforms = [{id}]"),
            format!("uniform token[] joints = [\"a\", \"a\"]\n        uniform matrix4d[] bindTransforms = [{id}, {id}]\n        uniform matrix4d[] restTransforms = [{id}, {id}]"),
        );
        // A blend shape "s" on the mesh, and in each clip a list naming it twice
        // with a weight for each: the selected clip's at rest, the other's keyed.
        swap(
            "        rel skel:skeleton = </R/Skel>\n    }\n".into(),
            "        uniform token[] skel:blendShapes = [\"s\"]\n        rel skel:blendShapeTargets = [</R/M/s>]\n        rel skel:skeleton = </R/Skel>\n        def BlendShape \"s\"\n        {\n            uniform vector3f[] offsets = [(0, 0, 1), (0, 0, 1), (0, 0, 1)]\n            uniform int[] pointIndices = [0, 1, 2]\n        }\n    }\n".into(),
        );
        swap(
            "def SkelAnimation \"a0\"\n        {\n".into(),
            "def SkelAnimation \"a0\"\n        {\n            uniform token[] blendShapes = [\"s\", \"s\"]\n            float[] blendShapeWeights = [0.25, 0.75]\n".into(),
        );
        swap(
            "def SkelAnimation \"a1\"\n        {\n".into(),
            "def SkelAnimation \"a1\"\n        {\n            uniform token[] blendShapes = [\"s\", \"s\"]\n            float[] blendShapeWeights.timeSamples = { 0: [0.1, 0.9], 24: [0.5, 0.6] }\n".into(),
        );
        let r = read_rig(&text);
        let scene = &r.scene;
        let joints: Vec<usize> = scene.nodes.iter().enumerate().filter(|(_, n)| n.name.as_deref() == Some("a")).map(|(i, _)| i).collect();
        assert_eq!(joints.len(), 2, "two joints called a");
        assert_eq!(scene.animations.len(), 2);
        for (k, clip) in scene.animations.iter().enumerate() {
            let posed: Vec<usize> = clip.channels.iter().filter(|c| c.path == AnimPath::Translation).map(|c| c.node).collect();
            assert_eq!(posed, [joints[0]], "clip {k}: the animation's joint a is the skeleton's first");
        }
        assert_eq!(
            [rig_point(&r, 0, 1.0), rig_point(&r, 1, 1.0)],
            [[1.0, 1.0, 0.25], [1.0, 2.0, 0.5]],
            "the mesh rides the first joint, which both clips lift, and the first s, which they weigh at 0.25 and (a second in) 0.5"
        );
        assert_eq!(scene.meshes[0].weights, [0.25], "the selected clip's rest weight of s is its first");
        let keyed: Vec<&Channel> = scene.animations[1].channels.iter().filter(|c| c.path == AnimPath::Weights).collect();
        assert_eq!(keyed.len(), 1, "the other clip keys the weight");
        assert_eq!(keyed[0].values, [0.1, 0.5], "and keys the first s's");
    }

    /// A rigid xformOp is authored outside the variants, so it belongs to the
    /// authored selection's clip alone.
    #[test]
    fn rigid_samples_belong_to_the_selected_clip() {
        let box_prim = "    def Xform \"Box\" {\n        double3 xformOp:translate.timeSamples = { 0: (0, 0, 0), 24: (5, 0, 0) }\n        uniform token[] xformOpOrder = [\"xformOp:translate\"]\n    }";
        let r = read_rig(&animation_rig(2, Some(1), box_prim));
        let node = r.scene.nodes.iter().position(|n| n.name.as_deref() == Some("Box")).unwrap();
        let on_box: Vec<usize> = r.scene.animations.iter().map(|a| a.channels.iter().filter(|c| c.node == node).count()).collect();
        assert_eq!(on_box, [1, 0], "the box moves in the selected clip (a1) and not in the other (a0)");
        assert_eq!(clip_names(&r.scene), ["a1", "a0"].map(|n| Some(n.to_string())));
    }

    /// `customData.litegen.clipEnd` is where a clip ends, even past its last
    /// key; a clip is not shortened to fit it.
    #[test]
    fn a_clip_ends_where_the_file_says() {
        let mut text = animation_rig(2, Some(0), "");
        text = text.replace(
            "def SkelAnimation \"a0\"",
            "def SkelAnimation \"a0\" (\n            customData = { dictionary litegen = { double clipEnd = 3 } }\n        )",
        );
        text = text.replace(
            "def SkelAnimation \"a1\"",
            "def SkelAnimation \"a1\" (\n            customData = { dictionary litegen = { double clipEnd = 0.25 } }\n        )",
        );
        let r = read_rig(&text);
        let ends: Vec<Option<(f32, f32)>> = r.scene.animations.iter().map(|a| a.time_range()).collect();
        assert_eq!(ends, [Some((0.0, 3.0)), Some((0.0, 1.0))], "a0 is held to 3 s; a1 keeps its keys when clipEnd is before the last");
        assert_eq!(rig_point(&r, 0, 3.0), [1.0, 1.0, 0.0], "and holds its last pose");
    }

    /// The reader bounds the variants it composes at 64, and a hostile 65 is
    /// refused by name.
    #[test]
    fn a_hostile_animation_variant_count_is_capped() {
        let r = read_rig(&animation_rig(64, Some(0), ""));
        assert_eq!(r.scene.animations.len(), 64, "the cap itself reads");
        assert_eq!(r.scene.animations[63].name.as_deref(), Some("a63"));
        let text = animation_rig(65, Some(0), "");
        let e = read_usda(text.as_bytes(), &NoSidecars).map(|_| ()).unwrap_err();
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("64") && m.contains("Animation")), "{e:?}");
    }

    /// A clip with a blend-shape weight animation and no joint motion binds a
    /// SkelAnimation that lists no joints, so its variant poses the skeleton
    /// at rest and not in the pose the clip before it ended in.
    #[test]
    fn a_weights_only_clip_keeps_the_rest_pose() {
        use super::super::scene::MorphTarget;
        use super::super::usd_stage::Prop;
        let mut s = fixtures::skinned();
        s.animations.clear();
        let tube = &mut s.meshes[0].primitives[0];
        let n = tube.positions.len();
        tube.targets.push(MorphTarget { positions: vec![[0.0, 0.0, 0.05]; n], normals: None });
        s.meshes[0].weights = vec![0.0];
        s.meshes[0].target_names = vec!["swell".into()];
        s.animations.push(Animation {
            name: Some("bend".into()),
            channels: vec![Channel {
                node: 1,
                path: AnimPath::Rotation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.5, 0.866_025_4],
            }],
        });
        s.animations.push(Animation {
            name: Some("swell".into()),
            channels: vec![Channel { node: 3, path: AnimPath::Weights, interpolation: Interpolation::Linear, times: vec![0.0, 0.5], values: vec![0.0, 1.0] }],
        });
        s.validate().unwrap();
        for (fmt, single) in [(MeshFormat::Usdz, true), (MeshFormat::Usda, true), (MeshFormat::Usdc, true)] {
            let ctx = clip_ctx(fmt, single);
            let w = write_scene(&s, fmt, clip_opts(single, Some(UsdClips::All))).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let got = read_files(fmt, &w.package.files).scene;
            assert_eq!(clip_names(&got), ["bend", "swell"].map(|n| Some(n.to_string())), "{ctx}");
            let moves = |clip: usize, weights: bool| got.animations[clip].channels.iter().filter(|c| (c.path == AnimPath::Weights) == weights).count();
            assert_eq!((moves(0, false), moves(0, true)), (1, 0), "{ctx}: bend moves a joint and no weight");
            assert_eq!((moves(1, false), moves(1, true)), (0, 1), "{ctx}: swell moves a weight and no joint");
            let rest = pose::default_pose(&got);
            for t in [0.0, 0.25, 0.5, 1.0] {
                assert_eq!(pose::sample_pose(&got, 1, t).trs[..3], rest.trs[..3], "{ctx}: swell @ {t}: the joints stay at rest");
            }
            // In the file: swell's SkelAnimation lists no joints.
            let layer = layer_of(fmt, &w.package.files);
            let spec = &layer.specs["/skinned/leg/swell"];
            assert_eq!(spec.type_name, "SkelAnimation", "{ctx}");
            let Some(Prop::Attr(joints)) = spec.prop("joints") else { panic!("{ctx}: swell has no joints attribute") };
            assert_eq!(joints.default, Some(Value::Tokens(Vec::new())), "{ctx}: swell lists no joints");
            assert!(spec.prop("rotations").is_none() && spec.prop("translations").is_none(), "{ctx}: and poses none");
        }
    }

    /// Clips whose names make one prim name are written under distinct prim
    /// and variant names, and read back under their own: "run fast", "run-fast"
    /// and the already taken "hip" (a joint's name).
    #[test]
    fn clip_names_that_sanitise_alike_stay_distinct() {
        let mut fixture = fixtures::clips();
        let wanted = ["run fast", "run-fast", "hip"];
        for (a, n) in fixture.animations.iter_mut().zip(wanted) {
            a.name = Some(n.to_string());
        }
        for (fmt, single) in CLIP_WAYS {
            let ctx = clip_ctx(fmt, single);
            let w = write_scene(&fixture, fmt, clip_opts(single, Some(UsdClips::All))).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let layer = layer_of(fmt, &w.package.files);
            let sets = &layer.specs["/clips"].variant_sets;
            assert_eq!(sets.len(), 1, "{ctx}: one variant set");
            let variants = &sets[0].1;
            let distinct: std::collections::HashSet<&String> = variants.iter().collect();
            assert_eq!(distinct.len(), 3, "{ctx}: three variants: {variants:?}");
            let anims: std::collections::HashSet<&String> = layer.specs.iter().filter(|(_, s)| s.type_name == "SkelAnimation").map(|(p, _)| p).collect();
            assert_eq!(anims.len(), 3, "{ctx}: three SkelAnimation prims: {anims:?}");
            let got = read_files(fmt, &w.package.files).scene;
            assert_eq!(clip_names(&got), wanted.map(|n| Some(n.to_string())), "{ctx}: the originals come back");
        }
    }

    /// A rig built to cost the reader the square of `n`, wherever it looks
    /// something up by scanning a list instead of a table: a skeleton `Big`
    /// of `n` joints whose paths have no listed parent (`x/j0` … with no
    /// `x`), listed after `n` one-joint skeletons, and `n` meshes bound to
    /// `Big`, each remapping its joint through `skel:joints` and weighting
    /// one of the `n` blend shapes `Big`'s animations list, in two clips.
    fn wide_rig(n: usize) -> String {
        let list = |f: &dyn Fn(usize) -> String| (0..n).map(f).collect::<Vec<_>>().join(", ");
        let mut t = String::from("#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    timeCodesPerSecond = 24\n    upAxis = \"Y\"\n)\n");
        t.push_str("def SkelRoot \"R\" (\n    prepend variantSets = \"Animation\"\n    variants = { string Animation = \"c0\" }\n)\n{\n");
        t.push_str("    variantSet \"Animation\" = {\n");
        for k in 0..2 {
            t.push_str(&format!("        \"c{k}\" {{ over \"Big\" {{ rel skel:animationSource = </R/Big/a{k}> }} }}\n"));
        }
        t.push_str("    }\n");
        let id = "((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))";
        for i in 0..n {
            t.push_str(&format!(
                "    def Skeleton \"T{i}\" (prepend apiSchemas = [\"SkelBindingAPI\"])\n    {{\n        uniform token[] joints = [\"t\"]\n        uniform matrix4d[] bindTransforms = [{id}]\n        uniform matrix4d[] restTransforms = [{id}]\n    }}\n"
            ));
        }
        let shapes = list(&|i| format!("\"s{i}\""));
        let (half, one) = (list(&|_| "0.5".into()), list(&|_| "1".into()));
        t.push_str(&format!("    def Skeleton \"Big\" (prepend apiSchemas = [\"SkelBindingAPI\"])\n    {{\n        uniform token[] joints = [{}]\n", list(&|i| format!("\"x/j{i}\""))));
        for (k, (from, to)) in [(&half, &one), (&one, &half)].into_iter().enumerate() {
            t.push_str(&format!(
                "        def SkelAnimation \"a{k}\"\n        {{\n            uniform token[] blendShapes = [{shapes}]\n            float[] blendShapeWeights.timeSamples = {{ 0: [{from}], 24: [{to}] }}\n        }}\n"
            ));
        }
        t.push_str("    }\n");
        for i in 0..n {
            t.push_str(&format!("    def Mesh \"M{i}\" (prepend apiSchemas = [\"SkelBindingAPI\"])\n    {{\n"));
            t.push_str("        int[] faceVertexCounts = [3]\n        int[] faceVertexIndices = [0, 1, 2]\n        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]\n");
            t.push_str("        int[] primvars:skel:jointIndices = [0] (elementSize = 1\n            interpolation = \"constant\")\n");
            t.push_str("        float[] primvars:skel:jointWeights = [1] (elementSize = 1\n            interpolation = \"constant\")\n");
            t.push_str(&format!("        uniform token[] skel:joints = [\"x/j{i}\"]\n        uniform token[] skel:blendShapes = [\"s{i}\"]\n"));
            t.push_str(&format!("        rel skel:blendShapeTargets = [</R/M{i}/s>]\n        rel skel:skeleton = </R/Big>\n"));
            t.push_str("        def BlendShape \"s\"\n        {\n            uniform vector3f[] offsets = [(0, 0, 1), (0, 0, 1), (0, 0, 1)]\n            uniform int[] pointIndices = [0, 1, 2]\n        }\n    }\n");
        }
        t.push_str("}\n");
        t
    }

    /// The reader finds a joint's parent, a mesh's skeleton, the skeleton's
    /// joints a mesh names and the blend shapes an animation weighs through
    /// tables built once (per skeleton, per animation), not by scanning a
    /// list for each joint, mesh or clip: six times the rig takes about six
    /// times as long, where the scans took the square (the 2026-11-10
    /// follow-up of item 6's final review: a hostile upload could take
    /// minutes). Timed against a sixth of the rig, the best of two runs each.
    #[test]
    fn a_wide_skeleton_and_its_meshes_read_in_linear_time() {
        let time = |n: usize| {
            let text = wide_rig(n);
            (0..2)
                .map(|_| {
                    let started = std::time::Instant::now();
                    let r = read_usda(text.as_bytes(), &NoSidecars).unwrap_or_else(|e| panic!("{e}"));
                    let took = started.elapsed();
                    assert_eq!(r.scene.meshes.len(), n);
                    assert_eq!(r.scene.animations.len(), 2, "{:?}", r.warnings);
                    took
                })
                .min()
                .unwrap()
        };
        let (small, large) = (time(1_000), time(6_000));
        eprintln!("wide rig: 1,000 in {small:?}, 6,000 in {large:?}");
        assert!(large < small * 12 + std::time::Duration::from_millis(500), "6,000 took {large:?} against 1,000 in {small:?}");
    }

    /// Skeletons that share one animation (here inherited from their
    /// SkelRoot) read it once, not once each: `n` one-joint skeletons bound
    /// to an animation of `n` joints, none of them theirs, take about six
    /// times as long as a sixth of them, where copying and scanning the
    /// animation for every skeleton took the square (the 2026-11-10
    /// follow-up). Timed as `a_wide_skeleton_and_its_meshes_read_in_linear_time`.
    #[test]
    fn skeletons_sharing_one_animation_read_in_linear_time() {
        let rig = |n: usize| {
            let each = |v: &str| vec![v; n].join(", ");
            let joints = (0..n).map(|i| format!("\"x/j{i}\"")).collect::<Vec<_>>().join(", ");
            let id = "((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))";
            let mut t = String::from("#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    timeCodesPerSecond = 24\n    upAxis = \"Y\"\n)\n");
            t.push_str("def SkelRoot \"R\" (prepend apiSchemas = [\"SkelBindingAPI\"])\n{\n    rel skel:animationSource = </R/Moves>\n");
            t.push_str(&format!(
                "    def SkelAnimation \"Moves\"\n    {{\n        uniform token[] joints = [{joints}]\n        float3[] translations.timeSamples = {{ 0: [{}], 24: [{}] }}\n        quatf[] rotations = [{}]\n        half3[] scales = [{}]\n    }}\n",
                each("(0, 0, 0)"),
                each("(0, 1, 0)"),
                each("(1, 0, 0, 0)"),
                each("(1, 1, 1)")
            ));
            for i in 0..n {
                t.push_str(&format!(
                    "    def Skeleton \"T{i}\" (prepend apiSchemas = [\"SkelBindingAPI\"])\n    {{\n        uniform token[] joints = [\"t\"]\n        uniform matrix4d[] bindTransforms = [{id}]\n        uniform matrix4d[] restTransforms = [{id}]\n    }}\n"
                ));
            }
            t.push_str("    def Mesh \"M\"\n    {\n        int[] faceVertexCounts = [3]\n        int[] faceVertexIndices = [0, 1, 2]\n        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]\n    }\n}\n");
            t
        };
        let time = |n: usize| {
            let text = rig(n);
            (0..2)
                .map(|_| {
                    let started = std::time::Instant::now();
                    let r = read_usda(text.as_bytes(), &NoSidecars).unwrap_or_else(|e| panic!("{e}"));
                    let took = started.elapsed();
                    assert!(r.warnings.iter().any(|w| w.contains("Moves") && w.contains("lacks")), "{:?}", r.warnings);
                    took
                })
                .min()
                .unwrap()
        };
        let (small, large) = (time(1_000), time(6_000));
        eprintln!("shared animation: 1,000 skeletons in {small:?}, 6,000 in {large:?}");
        assert!(large < small * 12 + std::time::Duration::from_millis(500), "6,000 took {large:?} against 1,000 in {small:?}");
    }

    /// Skeletons that share one animation each get a copy of the channels
    /// of the joints they list, and each copy is charged to the key budget
    /// before it is made: past `MAX_ANIMATION_KEYS` the read is refused as it
    /// goes, where it used to build every copy first (gigabytes, for a
    /// crafted file of 100,000 skeletons), and refuse at the end (the
    /// 2026-11-10 follow-up review, I2). Here 2,100 skeletons each copy a
    /// translation of 10,000 keys: 21,000,000 keys.
    #[test]
    fn copies_of_a_shared_animation_are_charged_before_they_are_made() {
        let keys = 10_000;
        let rig = |n: usize| {
            let id = "((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))";
            let samples = (0..keys).map(|i| format!("{i}: [(0, {i}, 0)]")).collect::<Vec<_>>().join(", ");
            let mut t = String::from("#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    timeCodesPerSecond = 24\n    upAxis = \"Y\"\n)\n");
            t.push_str("def SkelRoot \"R\" (prepend apiSchemas = [\"SkelBindingAPI\"])\n{\n    rel skel:animationSource = </R/Moves>\n");
            t.push_str(&format!(
                "    def SkelAnimation \"Moves\"\n    {{\n        uniform token[] joints = [\"t\"]\n        float3[] translations.timeSamples = {{ {samples} }}\n        quatf[] rotations = [(1, 0, 0, 0)]\n        half3[] scales = [(1, 1, 1)]\n    }}\n"
            ));
            for i in 0..n {
                t.push_str(&format!(
                    "    def Skeleton \"T{i}\" (prepend apiSchemas = [\"SkelBindingAPI\"])\n    {{\n        uniform token[] joints = [\"t\"]\n        uniform matrix4d[] bindTransforms = [{id}]\n        uniform matrix4d[] restTransforms = [{id}]\n    }}\n"
                ));
            }
            t.push_str("    def Mesh \"M\"\n    {\n        int[] faceVertexCounts = [3]\n        int[] faceVertexIndices = [0, 1, 2]\n        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]\n    }\n}\n");
            t
        };
        assert!(2_100 * keys > super::super::scene::MAX_ANIMATION_KEYS);
        let e = read_usda(rig(2_100).as_bytes(), &NoSidecars).map(|_| ()).unwrap_err();
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("joint and blend-shape channels")), "{e:?}");
        // Within the budget the same rig reads, every skeleton with its copy.
        let r = read_usda(rig(10).as_bytes(), &NoSidecars).unwrap();
        assert_eq!(r.scene.animations.len(), 1, "{:?}", r.warnings);
        assert_eq!(r.scene.animations[0].channels.len(), 10, "one translation for each skeleton");
    }

    /// The scale loss judges the clips the file keeps: a USDZ keeps its first
    /// clip by default, so a scale only the second clip reaches is no loss
    /// there, and is one where `usd_clips=all` keeps that clip (the
    /// 2026-11-10 follow-up review, M1).
    #[test]
    fn a_joint_scale_only_a_dropped_clip_reaches_is_no_loss() {
        let mut s = fixtures::skinned();
        let j = s.skins[0].joints[0];
        let mut second = s.animations[0].clone();
        second.name = Some("second".into());
        second.channels =
            vec![Channel { node: j, path: AnimPath::Scale, interpolation: Interpolation::Linear, times: vec![0.0, 1.0], values: vec![1.0, 1.0, 1.0, 1.0, 70_000.0, 1.0] }];
        s.animations.push(second);
        s.validate().unwrap();
        let named = |losses: &[Loss]| losses.iter().any(|l| l.what.contains("joint scale"));
        let first = write_scene(&s, MeshFormat::Usdz, WriteOptions::default()).unwrap();
        assert!(!named(&first.losses), "the first clip alone: {:#?}", first.losses);
        let all = write_scene(&s, MeshFormat::Usdz, WriteOptions { usd_clips: Some(UsdClips::All), ..Default::default() }).unwrap();
        assert!(named(&all.losses), "every clip: {:#?}", all.losses);
    }

    /// A rest scale past f32's range (a `matrix4d`, or an FBX double, can
    /// hold one) is written as 65,504 too: the clamp is taken in f64, where
    /// the cast to f32 made it an infinity that `validate` then refused (the
    /// 2026-11-10 follow-up review, M2).
    #[test]
    fn a_joint_scale_past_f32_is_clamped_not_made_infinite() {
        let mut s = fixtures::skinned();
        let j = s.skins[0].joints[0];
        let (t, r, _) = s.nodes[j].transform.trs().unwrap();
        s.nodes[j].transform = Transform::Trs { translation: t, rotation: r, scale: [1.0, 1e39, -1e39] };
        quantize_joint_scales(&mut s);
        assert_eq!(s.nodes[j].transform.trs().unwrap().2, [1.0, 65_504.0, -65_504.0]);
    }

    /// UsdSkel holds an animated skeleton's joint scales as halves, whose
    /// largest finite value is 65,504, so the writer writes a larger one as
    /// 65,504 (`to_half_precision`), in every USD encoding. That changes the
    /// model, so it is named for the caller, as other losses are. (The
    /// 2026-11-10 follow-up of Task 1's review, which found that the crate
    /// would write such a half as an infinity: the clamp comes first.)
    #[test]
    fn joint_scales_past_a_half_are_named_as_a_loss() {
        let mut s = fixtures::skinned();
        assert!(!s.animations.is_empty(), "the skeleton is animated");
        let j = s.skins[0].joints[0];
        let (t, r, _) = s.nodes[j].transform.trs().unwrap();
        s.nodes[j].transform = Transform::Trs { translation: t, rotation: r, scale: [1.0, 70_000.0, 1.0] };
        let named = |l: &Loss| l.what.contains("joint scale") && l.why.contains("65,504");
        for fmt in [MeshFormat::Usda, MeshFormat::Usdc, MeshFormat::Usdz] {
            let w = write_scene(&s, fmt, WriteOptions::default()).unwrap_or_else(|e| panic!("{fmt}: {e}"));
            assert_eq!(w.losses.iter().filter(|&l| named(l)).count(), 1, "{fmt}: {:#?}", w.losses);
        }
        // A scale a half holds, and a skeleton no clip animates, lose nothing.
        let w = write_scene(&fixtures::skinned(), MeshFormat::Usda, WriteOptions::default()).unwrap();
        assert!(!w.losses.iter().any(named), "{:#?}", w.losses);
        s.animations.clear();
        let w = write_scene(&s, MeshFormat::Usda, WriteOptions::default()).unwrap();
        assert!(!w.losses.iter().any(named), "at rest the scale is a matrix4d's, which holds it: {:#?}", w.losses);
    }

    /// A marker that is not a boolean (`int unlit = 1`, a string, a double)
    /// is no marker, as before, but no longer silently: the material reads
    /// lit, and the reader names it (the 2026-11-10 follow-up of Task 4a's
    /// review).
    #[test]
    fn a_mistyped_unlit_marker_reads_lit_and_is_named() {
        let s = unlit_scene();
        let w = write_scene(&s, MeshFormat::Usda, WriteOptions { self_contained: false, ..Default::default() }).unwrap();
        let layer = String::from_utf8(w.package.files[0].bytes.clone()).unwrap();
        for wrong in ["int unlit = 1", "string unlit = \"true\"", "double unlit = 1"] {
            let text = layer.replace("bool unlit = 1", wrong);
            let files = with_root_layer(MeshFormat::Usda, &w.package.files, &text);
            let r = read_scene(&files[0].bytes, MeshFormat::Usda, &MemorySidecars { files: &files }).unwrap();
            for name in ["box_mat", "wedge_mat"] {
                assert!(!material_named(&r.scene, name).unlit, "{wrong}: {name} reads lit");
                assert!(
                    r.warnings.iter().any(|x| x.contains(name) && x.contains("unlit") && x.contains("not a bool")),
                    "{wrong}: {name} is named: {:?}",
                    r.warnings
                );
            }
            assert_eq!(r.warnings.len(), 2, "{wrong}: {:?}", r.warnings);
        }
    }

    /// A SkelAnimation that lists no joints poses none, and USD shows its
    /// skeleton at rest. One that weighs blend shapes, or is a clip of
    /// litegen's own that leaves this skeleton alone (`customData.litegen`),
    /// is what it means to be and reads silently. Any other animates nothing,
    /// and the reader names it, as it did before item 6 (Task 3a's report,
    /// concern 5).
    #[test]
    fn an_animation_that_animates_nothing_is_named() {
        let base = animation_rig(1, None, "");
        let a0 = "        def SkelAnimation \"a0\"\n        {\n            uniform token[] joints = [\"a\"]\n            float3[] translations.timeSamples = { 0: [(0, 0, 0)], 24: [(0, 1, 0)] }\n            quatf[] rotations = [(1, 0, 0, 0)]\n            half3[] scales = [(1, 1, 1)]\n        }\n";
        assert_eq!(base.matches(a0).count(), 1, "{base}");
        let rig = |head: &str, inner: &str| read_rig(&base.replace(a0, &format!("        def SkelAnimation \"a0\"{head}\n        {{\n{inner}        }}\n")));
        let named = |r: &SceneRead| r.warnings.iter().any(|w| w.contains("a0") && w.contains("animates nothing"));
        let ours = " (\n            customData = { dictionary litegen = { double clipEnd = 1 } }\n        )";
        for (what, r) in [("an empty animation", rig("", "")), ("one that lists no joints", rig("", "            uniform token[] joints = []\n"))] {
            assert!(named(&r), "{what} is named: {:?}", r.warnings);
            assert!(r.scene.animations.is_empty(), "{what}: nothing moves");
        }
        for (what, r) in [
            ("a weights-only animation", rig("", "            uniform token[] blendShapes = [\"s\"]\n            float[] blendShapeWeights = [0.5]\n")),
            ("litegen's own clip that leaves the skeleton alone", rig(ours, "            uniform token[] joints = []\n")),
        ] {
            assert!(!named(&r), "{what} reads silently: {:?}", r.warnings);
        }
    }

    /// A crate is encoded from the writer's own text, parsed back with the
    /// reader's limits. A scene that `write_scene` accepts can nest deeper
    /// than the reader's 1,024 open prims: its USDA writes, and its crate
    /// refuses, with an error that names usdc and the write, not a `usda`
    /// line that reads like a failure to read the user's input.
    #[test]
    fn a_crate_write_the_reader_would_not_re_parse_is_a_usdc_refusal() {
        let mut s = fixtures::cube();
        let mut top = s.roots[0];
        for _ in 0..1100 {
            s.nodes.push(Node { name: Some("g".into()), children: vec![top], ..Default::default() });
            top = s.nodes.len() - 1;
        }
        s.roots = vec![top];
        s.validate().unwrap();
        write_scene(&s, MeshFormat::Usda, WriteOptions::default()).expect("the same scene writes as text");
        let usdz_crate = WriteOptions { usdz_root: UsdzRoot::Usdc, ..Default::default() };
        for (fmt, opts) in [(MeshFormat::Usdc, WriteOptions::default()), (MeshFormat::Usdz, usdz_crate)] {
            let e = write_scene(&s, fmt, opts).map(|_| ()).unwrap_err();
            let MeshError::Unsupported(m) = &e else { panic!("{fmt}: not a refusal of the scene: {e:?}") };
            assert!(m.starts_with("usdc: "), "{fmt}: the error names the format it was writing: {m}");
            assert!(m.contains("re-parses"), "{fmt}: and says where it failed: {m}");
            assert!(m.contains("nest deeper"), "{fmt}: and why: {m}");
        }
    }

    /// `prepare` keeps every clip it is asked to keep at the half precision
    /// UsdSkel stores a joint scale in, not the first alone: the scene it
    /// returns is then the scene the file reads back as.
    #[test]
    fn prepare_rounds_every_kept_clips_joint_scales() {
        let p = MeshFormat::Usda.prepare(&fixtures::clips(), clip_opts(true, Some(UsdClips::All))).unwrap();
        assert_eq!(p.animations.len(), 3);
        let scales: Vec<f32> = p
            .animations
            .iter()
            .flat_map(|a| a.channels.iter().filter(|c| c.path == AnimPath::Scale))
            .flat_map(|c| c.values.iter().copied())
            .collect();
        assert!(!scales.is_empty());
        assert!(scales.iter().all(|v| to_half_precision(*v) == *v), "{scales:?}");
        assert!(scales.contains(&to_half_precision(1.3)), "wave's 1.3 is a half now: {scales:?}");
    }

    /// `LightUnits::Blender` writes each light the way Blender's own USD
    /// exporter writes a light of the `energy` Blender's glTF importer gives
    /// the scene's light (`scene::blender_energy`): a point or spot light
    /// is a normalised SphereLight of intensity E / π
    /// (usd_writer_light.cc L138-L141 and L171-L174, Blender v5.2.0 and
    /// v5.2.3), and a sun a normalised DistantLight of E / 4 (L134-L137).
    /// `staged`'s sun (3 lux), bulb (20 cd) and spot (40 cd) get Task 1's
    /// 0.001098096633, 0.117130307467 (= 80/683) and 0.234260614934
    /// (= 160/683), against 3, 80 and 160 photometric. The two layers differ
    /// in those three lines only: colour, radius, the cone, `customData`
    /// (which keeps the scene's exact values) and everything else are
    /// unchanged. The other format's option does not move a USD file.
    #[test]
    fn usd_blender_units_write_blenders_values() {
        let staged = fixtures::staged();
        let pinned = [("sun_node", 0.001098096633), ("bulb_node", 0.117130307467), ("spot_node", 0.234260614934)];
        for (fmt, single) in USD_WAYS {
            let (ctx, _) = way(fmt, single);
            let photometric = root_layer(fmt, &write_in(&staged, fmt, single, LightUnits::Photometric));
            let blender = root_layer(fmt, &write_in(&staged, fmt, single, LightUnits::Blender));
            let photo = light_prim_intensities(&photometric);
            assert_eq!(photo.iter().map(|p| p.1).collect::<Vec<_>>(), [3.0, 80.0, 160.0], "{ctx}: photometric stays lux and 4 × candela");
            let got = light_prim_intensities(&blender);
            assert_eq!(got.len(), 3, "{ctx}: {got:?}");
            for light in &staged.lights {
                let node = format!("{}_node", light.name.as_deref().unwrap());
                let written = f64::from(got.iter().find(|g| g.0 == node).unwrap_or_else(|| panic!("{ctx}: no {node}: {got:?}")).1);
                let energy = super::super::scene::blender_energy(&light.kind, f64::from(light.intensity));
                let per_energy = if light.kind == LightKind::Directional { 4.0 } else { std::f64::consts::PI };
                assert_close_rel(written, energy / per_energy, 1e-7, &format!("{ctx} {node}: E over the exporter's divisor"));
                assert_close_rel(written, pinned.iter().find(|p| p.0 == node).unwrap().1, 1e-7, &format!("{ctx} {node}: Task 1's number"));
            }
            let (before, after): (Vec<&str>, Vec<&str>) = (photometric.lines().collect(), blender.lines().collect());
            assert_eq!(before.len(), after.len(), "{ctx}");
            let changed: Vec<&str> = before.iter().zip(&after).filter(|(a, b)| a != b).map(|(_, b)| b.trim_start()).collect();
            assert_eq!(changed.len(), 3, "{ctx}: only the three intensities differ: {changed:?}");
            assert!(changed.iter().all(|l| l.starts_with("float inputs:intensity = ")), "{ctx}: {changed:?}");
            // FBX's option is not USD's.
            let other = WriteOptions { fbx_light_units: LightUnits::Blender, ..way(fmt, single).1 };
            let files = write_scene(&staged, fmt, other).unwrap().package.files;
            assert!(files == write_in(&staged, fmt, single, LightUnits::Photometric), "{ctx}: fbx_light_units leaves a USD file alone");
        }
    }

    /// Blender's USD importer takes a light's `energy` from the file as
    /// `inputs:intensity` × π for a sphere and × 4 for a sun
    /// (usd_reader_light.cc L134-L148, v5.2.0 and v5.2.3), so the scene's
    /// light is `scene::from_blender_energy` of that: cd = 170.75 ×
    /// intensity for a point or spot light and lux = 2732 × intensity for a
    /// sun. Exactly three things enter `energy`: the intensity (or the
    /// schema's fallback, 1 for a sphere and 50000 for a distant light), the
    /// light's kind, and (Ruling L3) 2^`inputs:exposure`, which the
    /// photometric reader folds in too; Blender keeps the exposure in its
    /// own field, and the glTF exporter multiplies it back (lights.py L179).
    /// `normalize`, `radius` and `angle` go to other fields of Blender's
    /// light (Review Focus 3), so they change nothing here, where the
    /// photometric reading scales by them.
    ///
    /// Read, each worked by hand:
    /// - a USDA as Blender writes `staged` (normalised, radius 0): the sun
    ///   0.0010980966 is 2.99999 lux, the bulb 0.11713031 is 20 cd and the
    ///   spot 0.23426062 is 40 cd, with its cones;
    /// - an unnormalised sphere of intensity 4 and radius 0.5 is 4 × 170.75 =
    ///   683 cd (photometric: π), and an unnormalised sun of intensity 2 and
    ///   angle 3° is 2 × 2732 = 5464 lux (photometric: 2 × `sizeFactor`);
    /// - exposure 2 on a sun of intensity 1 is 4 × 2732 = 10928 lux, and
    ///   exposure −1 on a sphere of 0.5 is 0.25 × 170.75 cd;
    /// - the fallbacks, 170.75 cd for a bare SphereLight and 50000 × 2732 =
    ///   1.366e8 lux for a bare DistantLight;
    /// - a colour of (2, 1, 0.5) on a sun of intensity 0.001 moves its peak
    ///   into the intensity: 0.001 × 2 × 2732 = 5.464 lux at (1, 0.5, 0.25).
    ///
    /// The photometric reading of the same file differs (the bulb is
    /// 0.11713031 / 4 cd), and so does neither option's effect on the
    /// other format: `fbx_light_units` does not move a USD read.
    #[test]
    fn usd_blender_read_follows_blenders_importer() {
        let body = r#"
            def DistantLight "sun" {
                float inputs:intensity = 0.0010980966
                bool inputs:normalize = 1
                float inputs:angle = 0
            }
            def SphereLight "bulb" {
                float inputs:intensity = 0.11713031
                bool inputs:normalize = 1
                float inputs:radius = 0
                bool treatAsPoint = 1
            }
            def SphereLight "spot" (
                prepend apiSchemas = ["ShapingAPI"]
            ) {
                float inputs:intensity = 0.23426062
                bool inputs:normalize = 1
                float inputs:radius = 0
                bool treatAsPoint = 1
                float inputs:shaping:cone:angle = 34.37747
                float inputs:shaping:cone:softness = 0.6666667
            }
            def SphereLight "globe" {
                float inputs:intensity = 4
                float inputs:radius = 0.5
            }
            def DistantLight "wide" {
                float inputs:intensity = 2
                float inputs:angle = 3
            }
            def DistantLight "exposed" {
                float inputs:intensity = 1
                float inputs:exposure = 2
                bool inputs:normalize = 1
            }
            def SphereLight "dimmed" {
                float inputs:intensity = 0.5
                float inputs:exposure = -1
                bool inputs:normalize = 1
            }
            def SphereLight "plain" {
            }
            def DistantLight "bare" {
            }
            def DistantLight "hot" {
                color3f inputs:color = (2, 1, 0.5)
                float inputs:intensity = 0.001
                bool inputs:normalize = 1
            }"#;
        let r = read_stage_in(body, LightUnits::Blender);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let (sphere, sun) = (170.75, 2732.0);
        let from_energy = |kind, energy| super::super::scene::from_blender_energy(&kind, energy);
        assert_close_rel(from_energy(LightKind::Point, std::f64::consts::PI), sphere, 1e-12, "Task 1's sphere factor: E = π per unit of intensity");
        assert_close_rel(from_energy(LightKind::Directional, 4.0), sun, 1e-12, "Task 1's sun factor: E = 4 per unit of intensity");
        let cases = [
            ("sun", LightKind::Directional, 3.0, [1.0; 3]),
            ("bulb", LightKind::Point, 20.0, [1.0; 3]),
            ("spot", LightKind::Spot { inner_cone: 0.2, outer_cone: 0.6 }, 40.0, [1.0; 3]),
            ("globe", LightKind::Point, 4.0 * sphere, [1.0; 3]),
            ("wide", LightKind::Directional, 2.0 * sun, [1.0; 3]),
            ("exposed", LightKind::Directional, 4.0 * sun, [1.0; 3]),
            ("dimmed", LightKind::Point, 0.25 * sphere, [1.0; 3]),
            ("plain", LightKind::Point, sphere, [1.0; 3]),
            ("bare", LightKind::Directional, 50000.0 * sun, [1.0; 3]),
            ("hot", LightKind::Directional, 0.001 * 2.0 * sun, [1.0, 0.5, 0.25]),
        ];
        assert_eq!(r.scene.lights.len(), cases.len());
        for (node, kind, want, colour) in cases {
            let l = light_on(&r.scene, node).unwrap_or_else(|| panic!("no light on {node}"));
            assert_close_rel(f64::from(l.intensity), want, 1e-6, node);
            assert!((0..3).all(|k| (f64::from(l.color[k]) - colour[k]).abs() < 1e-7), "{node}: colour {:?} against {colour:?}", l.color);
            match (l.kind, kind) {
                (LightKind::Spot { inner_cone: i, outer_cone: o }, LightKind::Spot { inner_cone: wi, outer_cone: wo }) => {
                    assert!((i - wi).abs() < 1e-5 && (o - wo).abs() < 1e-5, "{node}: cones {i} and {o} against {wi} and {wo}");
                }
                (got, want) => assert_eq!(got, want, "{node}"),
            }
            assert_eq!(l.range, None, "{node}: UsdLux has no range");
        }
        // Photometric reads the same file as the schema says, and FBX's
        // option does not move it.
        let photometric = read_stage_in(body, LightUnits::Photometric);
        assert_close_rel(f64::from(light_on(&photometric.scene, "bulb").unwrap().intensity), 0.11713031 / 4.0, 1e-6, "photometric: a normalised sphere's cd is its intensity / 4");
        assert_close_rel(f64::from(light_on(&photometric.scene, "globe").unwrap().intensity), std::f64::consts::PI, 1e-6, "photometric: the radius is folded in");
        let text = usda(&format!("def Mesh \"tri\" {{ {QUAD} }}\n{body}"));
        let fbx_only = ReadOptions { fbx_light_units: LightUnits::Blender, ..Default::default() };
        let read = read_scene_with(text.as_bytes(), MeshFormat::Usda, &NoSidecars, fbx_only).unwrap();
        assert_eq!(read.scene.lights, photometric.scene.lights, "fbx_light_units is not usd_light_units");
    }

    /// A litegen file keeps the scene's own values in `customData.litegen`,
    /// and the reader prefers them whatever the units: a file written with
    /// `Blender` reads exactly as `Photometric` and as `Blender` (every
    /// light and camera, value for value), and so does one written with
    /// `Photometric`. Only a file without them (stripped here) reads by
    /// the UsdLux attributes, and then the units matter: staged's sun, in
    /// Blender's units, is 0.0044 lux read as photometric.
    #[test]
    fn usd_custom_data_still_wins() {
        let staged = fixtures::staged();
        for (fmt, single) in USD_WAYS {
            let (ctx, opts) = way(fmt, single);
            let want = placed(&fmt.prepare(&staged, opts).unwrap());
            for written in [LightUnits::Blender, LightUnits::Photometric] {
                let files = write_in(&staged, fmt, single, written);
                for read in [LightUnits::Photometric, LightUnits::Blender] {
                    let r = read_in(&files, fmt, read);
                    assert!(about_lights_and_cameras(&r).is_empty(), "{ctx}: {:?}", r.warnings);
                    assert_eq!(placed(&r.scene), want, "{ctx}: written {}, read {}: every value exactly", written.as_str(), read.as_str());
                }
            }
            let files = write_in(&staged, fmt, single, LightUnits::Blender);
            let bare = with_root_layer(fmt, &files, &without_litegen_data(&root_layer(fmt, &files)));
            let sun = |units| f64::from(light_on(&read_in(&bare, fmt, units).scene, "sun_node").unwrap().intensity);
            assert_close_rel(sun(LightUnits::Blender), 3.0, 1e-6, &format!("{ctx}: without customData, Blender reads its own file"));
            assert!(sun(LightUnits::Photometric) < 0.01, "{ctx}: without it, photometric reads Blender's 0.0011: {}", sun(LightUnits::Photometric));
        }
    }

    /// Writing and reading with `Blender` returns each light of `staged`,
    /// the spot included, to 1e-6 relative in intensity and to 1e-5 in
    /// everything else (`lightcam`), from a file that has lost its
    /// customData, so the UsdLux attributes alone carry it: the writer's
    /// divisors and the reader's multipliers are one factor and its inverse.
    /// No light is warned about on the way back.
    #[test]
    fn usd_blender_units_round_trip() {
        let staged = fixtures::staged();
        for (fmt, single) in USD_WAYS {
            let (ctx, opts) = way(fmt, single);
            let files = write_in(&staged, fmt, single, LightUnits::Blender);
            // A photometric file would round-trip through photometric
            // reading just as well, so first see that this one is not.
            assert!(files != write_in(&staged, fmt, single, LightUnits::Photometric), "{ctx}: the file is in Blender's units");
            let bare = with_root_layer(fmt, &files, &without_litegen_data(&root_layer(fmt, &files)));
            let r = read_in(&bare, fmt, LightUnits::Blender);
            assert!(about_lights_and_cameras(&r).is_empty(), "{ctx}: {:?}", r.warnings);
            assert_eq!(r.scene.lights.len(), 3, "{ctx}");
            let mut kinds = Vec::new();
            for want in &staged.lights {
                let node = format!("{}_node", want.name.as_deref().unwrap());
                let got = light_on(&r.scene, &node).unwrap_or_else(|| panic!("{ctx}: no light on {node}"));
                assert_close_rel(f64::from(got.intensity), f64::from(want.intensity), 1e-6, &format!("{ctx} {node}"));
                assert_eq!(std::mem::discriminant(&got.kind), std::mem::discriminant(&want.kind), "{ctx} {node}");
                kinds.push(got.kind);
            }
            assert!(matches!(kinds[2], LightKind::Spot { .. }), "{ctx}: the spot reads as a spot");
            let mut prepared = fmt.prepare(&staged, opts).unwrap();
            for l in prepared.lights.iter_mut() {
                l.range = None;
            }
            lightcam::assert_same_lights_and_cameras(&prepared, &r.scene, 1e-5, &format!("{ctx} staged through USD in Blender units"));
        }
    }

    /// Review Focus 1, with Ruling L5: no clamp on read, a drop. Written, a
    /// light at the f32 edge (the largest candela and lux, 1e38, zero and
    /// 1e-30) is finite: E / π and E / 4 are below the candela or lux they
    /// come from (and USD's existing `.min(f32::MAX)` still holds the
    /// photometric values), and 1e38 and 1e-30 read back to 1e-6 relative.
    /// The smallest subnormal and the largest float are written finite too
    /// (the f32 file value rounds; they are not read back here). Read, an
    /// intensity whose cd or lux is past f32 is dropped, never clamped,
    /// with the warning photometric units give an infinite one: 1e37 is
    /// 2.5e36 cd or 1e37 lux photometric but 1.7e39 cd or 2.7e40 lux
    /// Blender's, so only Blender drops it; 1e40 overflows in both; 1e36
    /// and 1e35 fit in both; and `inf` is dropped by both.
    #[test]
    fn usd_blender_units_clamp_at_the_f32_edge() {
        let fmt = MeshFormat::Usda;
        let blender_intensity = |l: &Light| {
            let divisor = if l.kind == LightKind::Directional { 4.0 } else { std::f64::consts::PI };
            super::super::scene::blender_energy(&l.kind, f64::from(l.intensity)) / divisor
        };
        for edge in [[f32::MAX, 1e38, f32::MAX], [0.0, f32::from_bits(1), 1e-30], [0.0, 1e-30, 1e38]] {
            let mut scene = fixtures::staged();
            for (light, intensity) in scene.lights.iter_mut().zip(edge) {
                light.intensity = intensity;
            }
            for units in [LightUnits::Photometric, LightUnits::Blender] {
                let layer = root_layer(fmt, &write_in(&scene, fmt, true, units));
                for (node, written) in light_prim_intensities(&layer) {
                    assert!(written.is_finite(), "{node} at {edge:?} in {}: {written}", units.as_str());
                    if units == LightUnits::Blender {
                        let light = scene.lights.iter().find(|l| node == format!("{}_node", l.name.as_deref().unwrap())).unwrap();
                        // The file holds an f32: a value under half its
                        // smallest subnormal rounds to zero.
                        let (written, want) = (f64::from(written), blender_intensity(light));
                        assert!((written - want).abs() <= 1e-6 * want + f64::from(f32::from_bits(1)), "{node} at {}: {written} against {want}", light.intensity);
                    }
                }
            }
            if edge.iter().any(|v| *v == f32::MAX || *v == f32::from_bits(1)) {
                continue;
            }
            let files = write_in(&scene, fmt, true, LightUnits::Blender);
            let bare = with_root_layer(fmt, &files, &without_litegen_data(&root_layer(fmt, &files)));
            let r = read_in(&bare, fmt, LightUnits::Blender);
            assert!(r.warnings.is_empty(), "{edge:?}: {:?}", r.warnings);
            assert_eq!(r.scene.lights.len(), 3, "{edge:?}: no light is dropped");
            for want in &scene.lights {
                let got = light_on(&r.scene, &format!("{}_node", want.name.as_deref().unwrap())).unwrap().intensity;
                assert!(got.is_finite(), "{:?} read as {got}", want.name);
                assert_close_rel(f64::from(got), f64::from(want.intensity), 1e-6, &format!("{:?} comes back", want.name));
            }
        }
        let far = |kind: &str, intensity: &str, units: LightUnits| {
            read_stage_in(&format!("def {kind} \"far\" {{\n float inputs:intensity = {intensity}\n bool inputs:normalize = 1\n }}"), units)
        };
        let dropped = "light /R/far is dropped: its intensity inf is not finite";
        for kind in ["SphereLight", "DistantLight"] {
            let fits = |intensity: &str, units: LightUnits, cd_or_lux: f64| {
                let r = far(kind, intensity, units);
                assert!(r.warnings.is_empty(), "{kind} {intensity} in {}: {:?}", units.as_str(), r.warnings);
                assert_eq!(r.scene.lights.len(), 1, "{kind} {intensity} in {}", units.as_str());
                assert_close_rel(f64::from(r.scene.lights[0].intensity), cd_or_lux, 1e-6, &format!("{kind} {intensity} in {}", units.as_str()));
            };
            let (photometric, blender) = if kind == "SphereLight" { (0.25, 170.75) } else { (1.0, 2732.0) };
            // 1e37 fits an f32 as photometric cd or lux, and overflows as
            // Blender's.
            fits("1e37", LightUnits::Photometric, 1e37 * photometric);
            let r = far(kind, "1e37", LightUnits::Blender);
            assert!(r.scene.lights.is_empty(), "{kind}: Blender drops what overflows f32, it does not clamp: {:?}", r.scene.lights);
            assert_eq!(r.warnings, [dropped], "{kind}: {:?}", r.warnings);
            // Fits in both.
            let (text, value) = if kind == "SphereLight" { ("1e36", 1e36) } else { ("1e35", 1e35) };
            fits(text, LightUnits::Blender, value * blender);
            fits(text, LightUnits::Photometric, value * photometric);
            // Past f32 in either units, and an infinite one: the same drop.
            for intensity in ["1e40", "inf"] {
                for units in [LightUnits::Photometric, LightUnits::Blender] {
                    let r = far(kind, intensity, units);
                    assert!(r.scene.lights.is_empty(), "{kind} {intensity} in {}: {:?}", units.as_str(), r.scene.lights);
                    assert_eq!(r.warnings, [dropped], "{kind} {intensity} in {}: {:?}", units.as_str(), r.warnings);
                }
            }
        }
    }

    /// Review Focus 4: a scene with no lights writes the same USDA and USDZ
    /// bytes with either units. `staged`'s do differ, so the comparison has
    /// something to catch.
    #[test]
    fn usd_blender_units_leave_a_lightless_scene_alone() {
        let cube = fixtures::cube();
        assert!(cube.lights.is_empty());
        let staged = fixtures::staged();
        for (fmt, single) in USD_WAYS {
            let (ctx, opts) = way(fmt, single);
            let blender = write_in(&cube, fmt, single, LightUnits::Blender);
            assert!(blender == write_in(&cube, fmt, single, LightUnits::Photometric), "{ctx}");
            assert!(blender == write_scene(&cube, fmt, opts).unwrap().package.files, "{ctx}: and the default's");
            assert!(write_in(&staged, fmt, single, LightUnits::Blender) != write_in(&staged, fmt, single, LightUnits::Photometric), "{ctx}: staged's differ");
        }
    }

    /// A light whose `inputs:normalize` is not a bool is dropped, with the
    /// warning naming the attribute, in either units: the reader drops a
    /// light on any mistyped attribute, and `Blender` reads `normalize`
    /// ahead of ignoring it, as `Photometric` reads it ahead of using it.
    #[test]
    fn a_mistyped_normalize_drops_the_light_in_either_units() {
        let body = r#"
            def SphereLight "lamp" {
                float inputs:intensity = 5
                string inputs:normalize = "yes"
            }
            def DistantLight "sun" {
                float inputs:intensity = 5
                string inputs:normalize = "yes"
            }"#;
        for units in [LightUnits::Photometric, LightUnits::Blender] {
            let r = read_stage_in(body, units);
            assert!(r.scene.lights.is_empty(), "{}: {:?}", units.as_str(), r.scene.lights);
            assert_eq!(
                r.warnings,
                ["light /R/lamp is dropped: its inputs:normalize is not a bool", "light /R/sun is dropped: its inputs:normalize is not a bool"],
                "{}",
                units.as_str()
            );
        }
    }

    /// The USD read end of `convert_scene` (`mod.rs`, which passes
    /// `WriteOptions::usd_light_units` into the `ReadOptions` of the read):
    /// `staged` written in Blender's units, its customData stripped (so only
    /// the UsdLux attributes say anything), converted to a GLB with
    /// `usd_light_units` = `Blender`, has the sun at 3 lux, the bulb at 20 cd
    /// and the spot at 40 cd, which is each file intensity × 2732 or ×
    /// 170.75 (Blender's importer, Task 1's rows 9 and 10). With the default
    /// the same file reads 0.0011 lux, and FBX's option does not move it.
    #[test]
    fn convert_scene_reads_a_blender_usd_in_blenders_units() {
        let staged = fixtures::staged();
        for (fmt, single) in USD_WAYS {
            let (ctx, _) = way(fmt, single);
            let files = write_in(&staged, fmt, single, LightUnits::Blender);
            let layer = root_layer(fmt, &files);
            let bare = with_root_layer(fmt, &files, &without_litegen_data(&layer));
            let intensity = |name: &str| f64::from(light_prim_intensities(&layer).into_iter().find(|p| p.0 == name).unwrap().1);
            let lights = |opts: WriteOptions| {
                let w = super::super::convert_scene(&bare[0].bytes, fmt, &MemorySidecars { files: &bare }, MeshFormat::Glb, opts).unwrap_or_else(|e| panic!("{ctx}: {e}"));
                read_scene(&w.package.files[0].bytes, MeshFormat::Glb, &NoSidecars).unwrap().scene.lights
            };
            let named = |lights: &[Light], name: &str| f64::from(lights.iter().find(|l| l.name.as_deref() == Some(name)).unwrap_or_else(|| panic!("{ctx}: no light {name}")).intensity);
            let blender = lights(WriteOptions { usd_light_units: LightUnits::Blender, ..Default::default() });
            assert_eq!(blender.len(), 3, "{ctx}");
            // The stripped file names each light after its prim.
            for (node, factor, want) in [("sun_node", 2732.0, 3.0), ("bulb_node", 170.75, 20.0), ("spot_node", 170.75, 40.0)] {
                assert_close_rel(named(&blender, node), intensity(node) * factor, 1e-6, &format!("{ctx} {node}: the file's intensity × Blender's factor"));
                assert_close_rel(named(&blender, node), want, 1e-6, &format!("{ctx} {node}: the scene's value"));
            }
            for opts in [WriteOptions::default(), WriteOptions { fbx_light_units: LightUnits::Blender, ..Default::default() }] {
                let got = lights(opts);
                assert_close_rel(named(&got, "sun_node"), intensity("sun_node"), 1e-6, &format!("{ctx}: the default reads the file's intensity as lux"));
                assert!(named(&got, "sun_node") < 0.01, "{ctx}: {}", named(&got, "sun_node"));
            }
        }
    }

    /// UsdGeomCamera's frustum (gf/camera.cpp L285-L298 and L310-L337,
    /// OpenUSD v26.11.1): a perspective camera's vertical field of view is
    /// 2·atan(verticalAperture / (2·focalLength)) and its aspect ratio
    /// horizontalAperture / verticalAperture; an orthographic one's window is
    /// its apertures times 0.1 (`GfCamera::APERTURE_UNIT`, L24-L26) wide, so
    /// xmag, half the width, is horizontalAperture / 20. Lens and film values
    /// are in tenths of a scene unit and the clipping range in scene units
    /// (usdGeom/schema.usda L2288-L2300, L2354-L2373). Written: staged's
    /// perspective camera (yfov 0.8, aspect 1.5) goes on a 24 mm film back,
    /// 0.24 at metersPerUnit 1, so its focal length is 0.12 / tan 0.4 =
    /// 0.28382668 (a 28.4 mm lens) and its horizontal aperture 0.36; its
    /// orthographic one (xmag 3, ymag 2) has apertures of 60 and 40. Read,
    /// each worked by hand (Python): a 50 mm lens on a 36 × 24 mm back (0.5,
    /// 0.36, 0.24) is a yfov of 0.47108996 at 3:2; a camera that authors
    /// nothing takes the schema's fallbacks (50, 20.955, 15.2908, a clipping
    /// range of 1 to 1e6), a yfov of 0.30346548 at 1.3704319; apertures of
    /// 60 and 40 are an orthographic view 3 by 2 half-extents; and in a
    /// centimetre stage the clipping range and the orthographic window come
    /// out in metres.
    #[test]
    fn usd_camera_fov_matches_the_aperture_formula() {
        let w = write_scene(&fixtures::staged(), MeshFormat::Usda, WriteOptions { self_contained: true, ..Default::default() }).unwrap();
        let layer = super::super::usda::parse(&w.package.files[0].bytes, "model.usda").unwrap();
        let attr = |prim: &str, name: &str| -> f64 {
            let v = layer.specs.get(prim).and_then(|s| s.attr(name)).and_then(|a| a.default.as_ref()).and_then(Value::as_f64);
            v.unwrap_or_else(|| panic!("{prim} has no {name}"))
        };
        let persp = "/staged/rig/persp_camera_node";
        let (f, h, v) = (attr(persp, "focalLength"), attr(persp, "horizontalAperture"), attr(persp, "verticalAperture"));
        assert_eq!((f as f32, h as f32, v as f32), (0.28382668, 0.36, 0.24), "a 28.4 mm lens on a 36 × 24 mm film back, in tenths of a metre");
        let yfov = 2.0 * (v / (2.0 * f)).atan();
        assert!((yfov - f64::from(0.8f32)).abs() < 1e-6 && (h / v - 1.5).abs() < 1e-6, "yfov {yfov}, aspect {}", h / v);
        let ortho = "/staged/rig/ortho_camera_node";
        assert_eq!((attr(ortho, "horizontalAperture"), attr(ortho, "verticalAperture")), (60.0, 40.0), "20 × xmag and 20 × ymag");

        let r = read_text(
            &format!(
                r#"def Mesh "tri" {{ {QUAD} }}
                def Camera "lens" {{
                    float focalLength = 0.5
                    float horizontalAperture = 0.36
                    float verticalAperture = 0.24
                    float2 clippingRange = (0.1, 100)
                }}
                def Camera "fallback" {{
                }}
                def Camera "plan" {{
                    token projection = "orthographic"
                    float horizontalAperture = 60
                    float verticalAperture = 40
                    float2 clippingRange = (0, 20)
                }}"#
            ),
            &[],
        );
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let near = |a: f32, b: f64| (f64::from(a) - b).abs() < 1e-6;
        for (node, want_yfov, want_aspect, want_near, want_far) in [("lens", 0.47108996144172666, 1.5, 0.1, 100.0), ("fallback", 0.30346547582902544, 1.370431880219285, 1.0, 1e6)] {
            let p = camera_on(&r.scene, node).map(|c| c.projection);
            let Some(Projection::Perspective { yfov, aspect: Some(aspect), znear, zfar: Some(zfar) }) = p else { panic!("{node}: {p:?}") };
            assert!(near(yfov, want_yfov) && near(aspect, want_aspect) && near(znear, want_near) && near(zfar, want_far), "{node}: {p:?}");
        }
        assert_eq!(camera_on(&r.scene, "plan").map(|c| c.projection), Some(Projection::Orthographic { xmag: 3.0, ymag: 2.0, znear: 0.0, zfar: 20.0 }));

        let cm = format!(
            "#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 0.01\n    upAxis = \"Y\"\n)\ndef Xform \"R\"\n{{\n\
             def Mesh \"tri\" {{ {QUAD} }}\n\
             def Camera \"cm_p\" {{ float focalLength = 50\n float horizontalAperture = 36\n float verticalAperture = 24\n float2 clippingRange = (10, 5000) }}\n\
             def Camera \"cm_o\" {{ token projection = \"orthographic\"\n float horizontalAperture = 6000\n float verticalAperture = 4000\n float2 clippingRange = (0, 2000) }}\n}}\n"
        );
        let r = read_usda(cm.as_bytes(), &NoSidecars).unwrap();
        let p = camera_on(&r.scene, "cm_p").map(|c| c.projection);
        let Some(Projection::Perspective { yfov, aspect: Some(aspect), znear, zfar: Some(zfar) }) = p else { panic!("cm_p: {p:?}") };
        assert!(near(yfov, 0.47108996144172666) && near(aspect, 1.5) && near(znear, 0.1) && near(zfar, 50.0), "cm_p: {p:?}");
        let p = camera_on(&r.scene, "cm_o").map(|c| c.projection);
        let Some(Projection::Orthographic { xmag, ymag, znear, zfar }) = p else { panic!("cm_o: {p:?}") };
        assert!(near(xmag, 3.0) && near(ymag, 2.0) && znear == 0.0 && near(zfar, 20.0), "cm_o: {p:?}");
    }

    /// Ruling R6: a light's `inputs:intensity` is photometric, as the UsdLux
    /// schema defines it (usd_write.rs, `SPHERE_INTENSITY_PER_CANDELA`).
    /// Written: staged's sun, 3 lux, is a normalised DistantLight of angle 0
    /// with an intensity of 3; its bulb (20 cd) and spot (40 cd) are
    /// normalised SphereLights treated as points, of radius 0, with
    /// intensities of 80 and 160. Read, each worked by hand (Python): a
    /// normalised DistantLight of intensity 5 is 5 lux; one that authors
    /// nothing is unnormalised, of the schema's 50000 nits and 0.53°, so
    /// 50000·π·sin²(0.265°) = 3.3601852 lux; a normalised SphereLight of
    /// intensity 8 and exposure 1 is 8·2/4 = 4 cd; an unnormalised one of
    /// intensity 4 and radius 0.5 is 4·π·0.5² = π cd; and a colour of
    /// (2, 1, 0.5) at 3 lux moves its peak into the intensity, (1, 0.5, 0.25)
    /// at 6 lux, as glTF's colour stays within 0 to 1.
    #[test]
    fn usd_light_intensity_follows_the_usdlux_schema() {
        let w = write_scene(&fixtures::staged(), MeshFormat::Usda, WriteOptions { self_contained: true, ..Default::default() }).unwrap();
        let layer = super::super::usda::parse(&w.package.files[0].bytes, "model.usda").unwrap();
        let get = |prim: &str, name: &str| -> Option<f64> {
            let p = format!("/staged/rig/{prim}");
            layer.specs.get(&p).and_then(|s| s.attr(name)).and_then(|a| a.default.as_ref()).and_then(Value::as_f64)
        };
        for (prim, intensity) in [("sun_node", 3.0), ("bulb_node", 80.0), ("spot_node", 160.0)] {
            assert_eq!(get(prim, "inputs:intensity"), Some(intensity), "{prim}");
            assert_eq!(get(prim, "inputs:normalize"), Some(1.0), "{prim}: normalised");
        }
        assert_eq!(get("sun_node", "inputs:angle"), Some(0.0), "the sun's rays are parallel");
        for prim in ["bulb_node", "spot_node"] {
            assert_eq!((get(prim, "inputs:radius"), get(prim, "treatAsPoint")), (Some(0.0), Some(1.0)), "{prim}: a point");
        }

        let r = read_text(
            &format!(
                r#"def Mesh "tri" {{ {QUAD} }}
                def DistantLight "sun" {{
                    float inputs:intensity = 5
                    bool inputs:normalize = 1
                }}
                def DistantLight "dim" {{
                }}
                def SphereLight "bulb" {{
                    float inputs:intensity = 8
                    float inputs:exposure = 1
                    bool inputs:normalize = 1
                    bool treatAsPoint = 1
                    float inputs:radius = 0
                }}
                def SphereLight "globe" {{
                    float inputs:intensity = 4
                    float inputs:radius = 0.5
                }}
                def DistantLight "hot" {{
                    color3f inputs:color = (2, 1, 0.5)
                    float inputs:intensity = 3
                    bool inputs:normalize = 1
                }}"#
            ),
            &[],
        );
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let cases = [
            ("sun", LightKind::Directional, 5.0, [1.0; 3]),
            ("dim", LightKind::Directional, 3.3601852138740393, [1.0; 3]),
            ("bulb", LightKind::Point, 4.0, [1.0; 3]),
            ("globe", LightKind::Point, std::f64::consts::PI, [1.0; 3]),
            ("hot", LightKind::Directional, 6.0, [1.0, 0.5, 0.25]),
        ];
        for (node, kind, intensity, colour) in cases {
            let l = light_on(&r.scene, node).unwrap_or_else(|| panic!("no light on {node}"));
            assert_eq!(l.kind, kind, "{node}");
            assert!(((f64::from(l.intensity) - intensity) / intensity).abs() < 1e-6, "{node}: intensity {} against {intensity}", l.intensity);
            assert!((0..3).all(|k| (f64::from(l.color[k]) - colour[k]).abs() < 1e-7), "{node}: colour {:?} against {colour:?}", l.color);
            assert_eq!(l.range, None, "{node}: UsdLux has no range");
        }
    }

    /// UsdLux lights shine, and UsdGeomCamera cameras look, along their
    /// prim's −Z, with +Y up ("By convention, lights with a primary axis emit
    /// along -Z", usdLux/overview.dox L96; DistantLight's "Light emitted
    /// from a distant source along the -Z axis", usdLux/schema.usda L1087; a
    /// camera is "looking down the -Z axis, with +Y as the up axis",
    /// usdGeom/schema.usda L2280-L2284), as glTF's do: the boundary turns
    /// nothing. Read: a camera turned a quarter about +Y looks along −X with
    /// +Y up, and a sun and a spot turned −90° about X shine straight down.
    /// Written: each node goes out with the scene's own translate and orient,
    /// so staged's lights and cameras, composed from the written ops alone,
    /// face as in the scene.
    #[test]
    fn usd_cameras_and_lights_point_along_minus_z_on_the_wire() {
        let r = read_text(
            &format!(
                r#"def Mesh "tri" {{ {QUAD} }}
                def Camera "eye" {{
                    float xformOp:rotateY = 90
                    uniform token[] xformOpOrder = ["xformOp:rotateY"]
                }}
                def DistantLight "sun" {{
                    float xformOp:rotateX = -90
                    uniform token[] xformOpOrder = ["xformOp:rotateX"]
                }}
                def SphereLight "spot" (
                    prepend apiSchemas = ["ShapingAPI"]
                ) {{
                    float inputs:shaping:cone:angle = 30
                    float inputs:shaping:cone:softness = 0.5
                    float xformOp:rotateX = -90
                    uniform token[] xformOpOrder = ["xformOp:rotateX"]
                }}"#
            ),
            &[],
        );
        let (lights, cameras) = lightcam::frames(&r.scene, None);
        assert_eq!((lights.len(), cameras.len()), (2, 1), "{:?}", r.warnings);
        assert!(near3(cameras[0].forward, [-1.0, 0.0, 0.0], 1e-12) && near3(cameras[0].up, [0.0, 1.0, 0.0], 1e-12), "{:?}", cameras[0]);
        for l in &lights {
            assert!(near3(l.direction, [0.0, -1.0, 0.0], 1e-12), "{l:?}");
        }

        let fixture = fixtures::staged();
        let w = write_scene(&fixture, MeshFormat::Usda, WriteOptions { self_contained: true, ..Default::default() }).unwrap();
        let layer = super::super::usda::parse(&w.package.files[0].bytes, "model.usda").unwrap();
        let op = |prim: &str, name: &str| -> Option<Vec<f64>> { layer.specs.get(prim)?.attr(name)?.default.as_ref()?.as_tuple().map(<[f64]>::to_vec) };
        let local = |prim: &str| -> math::Mat4 {
            let t = op(prim, "xformOp:translate").unwrap_or_else(|| vec![0.0; 3]);
            let q = op(prim, "xformOp:orient").unwrap_or_else(|| math::QUAT_IDENTITY.to_vec());
            math::compose([t[0], t[1], t[2]], [q[0], q[1], q[2], q[3]], [1.0; 3])
        };
        let rig = local("/staged/rig");
        let world = |node: &str| math::mat_mul(&rig, &local(&format!("/staged/rig/{node}")));
        let unit = |v: [f64; 3]| {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / l, v[1] / l, v[2] / l]
        };
        let (want_lights, want_cameras) = lightcam::frames(&fixture, None);
        for f in &want_lights {
            let m = world(&f.node);
            let facing = unit([-m[8], -m[9], -m[10]]);
            assert!(near3(facing, f.direction, 1e-6), "{}: {facing:?} against {:?}", f.node, f.direction);
        }
        for f in &want_cameras {
            let m = world(&f.node);
            let (facing, up) = (unit([-m[8], -m[9], -m[10]]), unit([m[4], m[5], m[6]]));
            assert!(near3(facing, f.forward, 1e-6) && near3(up, f.up, 1e-6), "{}: {facing:?} {up:?} against {f:?}", f.node);
        }
        let written = ["def Camera \"persp_camera_node\"", "def Camera \"ortho_camera_node\"", "def DistantLight \"sun_node\"", "def SphereLight \"bulb_node\"", "def SphereLight \"spot_node\""];
        let text = String::from_utf8_lossy(&w.package.files[0].bytes);
        assert!(written.iter().all(|p| text.contains(p)), "each light and camera node is its own prim: {text}");
    }

    /// Ruling R3: a light or camera on a skeleton joint, or on a group node
    /// that `normalize_skeletons` folds out of the chain between two joints,
    /// keeps its world frame through USD, at rest and at every sample time.
    /// A USD skeleton's joints are not prims, so the knee's lamp goes on a
    /// child node of its own (`knee_light`, as FBX gives one), which the
    /// writer places under an Xform mirroring the knee; the folded group,
    /// animated between the knee and the ankle, keeps its own prim, its
    /// camera's. And a camera on the skinned mesh's node, which the writer
    /// puts beside its skeleton, keeps its place too. The hip's scale keys
    /// go first: USD stores joint scales as halves, which is another test's.
    #[test]
    fn a_light_and_camera_on_a_joint_survive_usd_joint_folding() {
        let mut s = fixtures::skinned();
        let group = s.nodes.len();
        let q10 = math::quat_from_axis_angle([0.0, 0.0, 1.0], 10f64.to_radians());
        let q30 = math::quat_from_axis_angle([0.0, 0.0, 1.0], 30f64.to_radians());
        s.nodes.push(Node {
            name: Some("shin_group".into()),
            transform: Transform::Trs { translation: [0.0, 0.2, 0.0], rotation: q10, scale: [1.0; 3] },
            children: vec![2],
            camera: Some(0),
            ..Default::default()
        });
        s.nodes[1].children = vec![group];
        let values: Vec<f32> = q10.iter().chain(q30.iter()).map(|v| *v as f32).collect();
        s.animations[0].channels.push(Channel { node: group, path: AnimPath::Rotation, interpolation: Interpolation::Linear, times: vec![0.0, 2.0], values });
        s.animations[0].channels.retain(|c| !(c.node == 0 && c.path == AnimPath::Scale));
        s.lights.push(Light { name: Some("knee_lamp".into()), kind: LightKind::Spot { inner_cone: 0.2, outer_cone: 0.6 }, color: [1.0, 0.5, 0.25], intensity: 9.0, range: Some(4.0) });
        s.nodes[1].light = Some(0);
        s.cameras.push(Camera { name: Some("shin_eye".into()), projection: Projection::Perspective { yfov: 0.8, aspect: Some(1.5), znear: 0.125, zfar: Some(64.0) } });
        s.cameras.push(Camera { name: Some("tube_eye".into()), projection: Projection::Orthographic { xmag: 3.0, ymag: 2.0, znear: 0.0625, zfar: 20.0 } });
        s.nodes[3].camera = Some(1);
        s.validate().unwrap();
        for (fmt, single) in USD_WAYS {
            let (ctx, opts) = way(fmt, single);
            let want = fmt.prepare(&s, opts).unwrap();
            let (r, _) = round(&s, fmt, single);
            assert_eq!((r.scene.lights.len(), r.scene.cameras.len()), (1, 2), "{ctx}: the knee's lamp and both cameras: {:?}", r.warnings);
            lightcam::assert_same_lights_and_cameras(&want, &r.scene, 1e-5, &ctx);
            for t in std::iter::once(None).chain(fixtures::sample_times(&s).into_iter().map(Some)) {
                let ((sl, sc), (gl, gc)) = (lightcam::frames(&s, t), lightcam::frames(&r.scene, t));
                for w in &sl {
                    let g = gl.iter().find(|g| g.light.name == w.light.name).unwrap_or_else(|| panic!("{ctx} {t:?}: no light {:?}", w.light.name));
                    assert!(near3(g.position, w.position, 1e-5) && near3(g.direction, w.direction, 1e-5), "{ctx} {t:?}: {g:?} against {w:?}");
                }
                for w in &sc {
                    let g = gc.iter().find(|g| g.camera.name == w.camera.name).unwrap_or_else(|| panic!("{ctx} {t:?}: no camera {:?}", w.camera.name));
                    assert!(near3(g.eye, w.eye, 1e-5) && near3(g.forward, w.forward, 1e-5) && near3(g.up, w.up, 1e-5), "{ctx} {t:?}: {g:?} against {w:?}");
                }
            }
        }
    }

    /// Review Focus 5: two camera nodes with one name (three.js binds by
    /// name) both survive USD, whose prim names must be unique: the writer
    /// gives the second prim a name of its own and keeps "cam" in its
    /// customData, the reader names both nodes "cam" again, and
    /// `normalize::unique_node_names` tells them apart. Each camera then
    /// stands, faces and projects as in the fixture renamed the same way
    /// (`lightcam`).
    #[test]
    fn two_cameras_with_one_name_both_survive() {
        let mut s = fixtures::staged();
        for n in [3, 4] {
            assert!(s.nodes[n].camera.is_some(), "staged's node {n} places a camera");
            s.nodes[n].name = Some("cam".into());
        }
        for (fmt, single) in USD_WAYS {
            let (ctx, opts) = way(fmt, single);
            let w = write_scene(&s, fmt, opts).unwrap();
            let layer = root_layer(fmt, &w.package.files);
            assert!(layer.contains("def Camera \"cam\"") && layer.contains("def Camera \"cam_1\""), "{ctx}: a prim each, named apart");
            let mut got = read_files(fmt, &w.package.files).scene;
            let named: Vec<Option<&str>> = got.nodes.iter().filter(|n| n.camera.is_some()).map(|n| n.name.as_deref()).collect();
            assert_eq!(named, [Some("cam"), Some("cam")], "{ctx}: both read back, each named as written");
            assert!(super::super::normalize::unique_node_names(&mut got), "{ctx}: renamed apart");
            let mut want = fmt.prepare(&s, opts).unwrap();
            super::super::normalize::unique_node_names(&mut want);
            lightcam::assert_same_lights_and_cameras(&want, &got, 1e-5, &ctx);
        }
    }

    /// Review Focus 6 in USD: a light or camera that breaks one of glTF's
    /// bounds is dropped with a warning naming its prim and the bound, the
    /// stage still reads, and every node stays: a spot whose litegen
    /// customData puts its cones out of order, a light whose intensity is
    /// infinite, and a perspective camera whose clipping range starts at 0.
    #[test]
    fn a_bad_light_or_camera_is_dropped_not_refused() {
        let r = read_text(
            &format!(
                r#"def Mesh "tri" {{ {QUAD} }}
                def SphereLight "lamp" (
                    prepend apiSchemas = ["ShapingAPI"]
                    customData = {{
                        dictionary litegen = {{
                            token type = "spot"
                            color3f color = (1, 1, 1)
                            float intensity = 5
                            float innerConeAngle = 0.7
                            float outerConeAngle = 0.6
                        }}
                    }}
                ) {{
                    float inputs:shaping:cone:angle = 34.377468
                }}
                def SphereLight "flare" {{
                    float inputs:intensity = inf
                }}
                def Camera "shot" {{
                    float2 clippingRange = (0, 10)
                }}"#
            ),
            &[],
        );
        assert_eq!(
            r.warnings,
            [
                "light /R/lamp is dropped: its inner cone 0.7 is not below its outer cone 0.6",
                "light /R/flare is dropped: its intensity inf is not finite",
                "camera /R/shot is dropped: its znear 0 is not positive",
            ]
        );
        assert!(r.scene.lights.is_empty() && r.scene.cameras.is_empty());
        for node in ["lamp", "flare", "shot"] {
            assert!(r.scene.nodes.iter().any(|n| n.name.as_deref() == Some(node) && n.light.is_none() && n.camera.is_none()), "{node} stays, empty");
        }
    }

    /// Ruling R21: a reader never refuses a stage over a light or camera, so
    /// a light or camera prim whose transform does not read is dropped, with
    /// one warning naming it and why, and the stage reads without it. Here
    /// a sun whose orient's second sample is no quaternion, after a translate
    /// whose channel reading it already recorded and which goes with it (the
    /// sun comes first, so a leftover channel would animate the mesh), and a
    /// camera whose `xformOpOrder` names an op it lacks. What is under such a
    /// prim goes unread with it, as under every light and camera prim before
    /// the scene carried them. An Xform whose transform does not read still
    /// refuses the stage, as it did.
    #[test]
    fn a_light_or_camera_whose_transform_does_not_read_is_dropped_not_refused() {
        let body = format!(
            r#"def DistantLight "sun" {{
                float3 xformOp:translate.timeSamples = {{
                    0: (0, 0, 0),
                    24: (1, 0, 0),
                }}
                quatf xformOp:orient.timeSamples = {{
                    0: (1, 0, 0, 0),
                    24: None,
                }}
                uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:orient"]
            }}
            def Camera "eye" {{
                uniform token[] xformOpOrder = ["xformOp:rotateY"]
                def Mesh "lens" {{ {QUAD} }}
            }}
            def Mesh "tri" {{ {QUAD} }}"#
        );
        let r = read_usda(usda(&body).as_bytes(), &NoSidecars).unwrap_or_else(|e| panic!("the stage is refused: {e}"));
        assert_eq!(r.warnings.len(), 2, "one warning each: {:?}", r.warnings);
        let (sun, eye) = (&r.warnings[0], &r.warnings[1]);
        assert!(sun.starts_with("light /R/sun is dropped: its transform does not read") && sun.contains("xformOp:orient"), "{sun}");
        assert!(eye.starts_with("camera /R/eye is dropped: its transform does not read") && eye.contains("xformOp:rotateY") && eye.contains("under it"), "{eye}");
        assert!(r.scene.lights.is_empty() && r.scene.cameras.is_empty(), "{:?} {:?}", r.scene.lights, r.scene.cameras);
        let nodes: Vec<Option<&str>> = r.scene.nodes.iter().map(|n| n.name.as_deref()).collect();
        assert_eq!(nodes, [Some("tri")], "the mesh alone: the sun, the camera and the lens under it are not read");
        assert!(r.scene.animations.is_empty(), "the sun's translate goes with it: {:?}", r.scene.animations);
        let group = read_usda(usda(r#"def Xform "group" { uniform token[] xformOpOrder = ["xformOp:rotateY"] }"#).as_bytes(), &NoSidecars);
        assert!(matches!(&group, Err(MeshError::Malformed(m)) if m.contains("lists \"xformOp:rotateY\"")), "{:?}", group.map(|r| r.warnings));
    }

    /// Ruling R22: a light or camera whose transform fails keeps the
    /// animation keys its attempt charged, so a stage pays for one key budget
    /// however many of them fail. Each light here inherits one op stack that
    /// is not TRS (an inverted scaleX), 335 keys 4000 time codes apart, which
    /// densifies to 3,340,001 samples, past half the budget
    /// (`scene::MAX_ANIMATION_KEYS`, three keys a sample), and fails only at
    /// its last key, whose scale of 0 cannot be inverted. The first light
    /// computes every sample and is dropped for its last; each one after it
    /// is refused by `densify` before anything is allocated, and dropped with
    /// that reason. So eight lights cost what one does. Giving the charge back
    /// made every failing light pay a whole budget again, which is the
    /// amplification the 2026-10-12 review's finding 11 closed for one prim
    /// (`densified_samples_count_against_the_key_budget`).
    #[test]
    fn failed_lights_keep_the_animation_keys_they_charged() {
        let keys: String = (0..335).map(|k| format!("            {}: {},\n", k * 4000, if k == 334 { 0 } else { 1 })).collect();
        let stage = |lights: usize| {
            let body: String = (0..lights).map(|i| format!("    def DistantLight \"l{i}\" (inherits = </Spin>) {{\n    }}\n")).collect();
            format!(
                "#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n)\n\
                 class \"Spin\"\n{{\n    float xformOp:scaleX = 1\n    float xformOp:scaleX.timeSamples = {{\n{keys}    }}\n    \
                 uniform token[] xformOpOrder = [\"!invert!xformOp:scaleX\"]\n}}\n\
                 def Xform \"R\"\n{{\n    def Mesh \"tri\" {{ {QUAD} }}\n{body}}}\n"
            )
        };
        let read = |lights: usize| {
            let started = std::time::Instant::now();
            let r = read_usda(stage(lights).as_bytes(), &NoSidecars).unwrap_or_else(|e| panic!("{lights} light(s) refused: {e}"));
            (r, started.elapsed())
        };
        let (one, alone) = read(1);
        assert_eq!(one.warnings.len(), 1, "{:?}", one.warnings);
        assert!(one.warnings[0].starts_with("light /R/l0 is dropped") && one.warnings[0].contains("cannot be inverted"), "{:?}", one.warnings);
        let (eight, together) = read(8);
        eprintln!("failed lights: one {alone:?}, eight {together:?}");
        assert_eq!(eight.warnings.len(), 8, "one warning a light: {:?}", eight.warnings);
        assert!(eight.warnings[0].contains("cannot be inverted"), "the first light computes every sample: {:?}", eight.warnings[0]);
        for w in &eight.warnings[1..] {
            assert!(w.contains("densifies to 3340001 keys, more than this converter will hold"), "refused before allocating: {w}");
        }
        let nodes: Vec<Option<&str>> = eight.scene.nodes.iter().map(|n| n.name.as_deref()).collect();
        assert_eq!(nodes, [Some("tri")], "every light is dropped, the stage read");
        // A ratio of two reads in this test, so the machine's load falls on
        // both: about 1 kept, about 9.5 given back (54 s against 5.6 s in a
        // debug build on a loaded machine). The bound is 6, not 4, so that a
        // load rising between the two reads cannot fail it, while 9.5 still
        // does; the warnings above catch that regression on their own.
        assert!(together < alone * 6, "eight failing lights took {together:?}, one {alone:?}: the work grows with them");
    }

    /// Review Focus 4 in USD: a hard-edged spot, a SphereLight whose cone
    /// softness is ShapingAPI's fallback of 0, is softened and warned once
    /// (`Light::soften_hard_edge`), not dropped; written back and read
    /// again, with or without its litegen customData, it comes back
    /// unchanged and unwarned.
    #[test]
    fn a_hard_edged_usd_spot_is_softened_once() {
        let r = read_text(
            &format!(
                r#"def Mesh "tri" {{ {QUAD} }}
                def SphereLight "spot" (
                    prepend apiSchemas = ["ShapingAPI"]
                ) {{
                    bool inputs:normalize = 1
                    float inputs:shaping:cone:angle = 30
                }}"#
            ),
            &[],
        );
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert!(r.warnings[0].starts_with("light /R/spot had a hard edge"), "{:?}", r.warnings);
        let soft = light_on(&r.scene, "spot").cloned().expect("softened, not dropped");
        let LightKind::Spot { inner_cone, outer_cone } = soft.kind else { panic!("{soft:?}") };
        assert!(inner_cone < outer_cone && (f64::from(outer_cone) - std::f64::consts::FRAC_PI_6).abs() < 1e-7, "{soft:?}");
        let w = write_scene(&r.scene, MeshFormat::Usda, WriteOptions { self_contained: true, ..Default::default() }).unwrap();
        let again = String::from_utf8(w.package.files[0].bytes.clone()).unwrap();
        for (label, text) in [("with the litegen data", again.clone()), ("without it", without_litegen_data(&again))] {
            let back = read_usda(text.as_bytes(), &NoSidecars).unwrap();
            assert!(back.warnings.is_empty(), "{label}: {:?}", back.warnings);
            let kind = light_on(&back.scene, "spot").map(|l| l.kind);
            let Some(LightKind::Spot { inner_cone: i, outer_cone: o }) = kind else { panic!("{label}: {kind:?}") };
            assert!(i < o && (i - inner_cone).abs() <= 1e-6 && (o - outer_cone).abs() <= 1e-6, "{label}: ({i}, {o}) against ({inner_cone}, {outer_cone})");
        }
    }

    /// ShapingAPI's `inputs:shaping:cone:angle` falls back to 90°
    /// (usdLux/schema.usda L735, OpenUSD v26.11.1), and an unauthored
    /// attribute takes the schema's fallback (module docs). So a SphereLight
    /// that applies the API and authors no cone angle is a spot of 90°, a
    /// hemisphere, where it was read silently as a point light. With the
    /// softness's fallback of 0 its edge is hard ("the coneAngle functions
    /// as a hard binary "off" toggle", L756-L759), so it is softened, with
    /// the usual warning (Ruling R5). A SphereLight without the API, or one
    /// whose `apiSchemas` only deletes it, stays a point light, unwarned.
    #[test]
    fn shaping_api_without_a_cone_angle_takes_its_fallback_of_90_degrees() {
        let r = read_text(
            &format!(
                r#"def Mesh "tri" {{ {QUAD} }}
                def SphereLight "shaped" (
                    prepend apiSchemas = ["ShapingAPI"]
                ) {{
                    bool inputs:normalize = 1
                }}
                def SphereLight "plain" {{
                    bool inputs:normalize = 1
                }}
                def SphereLight "unshaped" (
                    delete apiSchemas = ["ShapingAPI"]
                ) {{
                    bool inputs:normalize = 1
                }}"#
            ),
            &[],
        );
        let shaped = light_on(&r.scene, "shaped").map(|l| l.kind);
        let Some(LightKind::Spot { inner_cone, outer_cone }) = shaped else { panic!("{shaped:?}: {:?}", r.warnings) };
        assert!((f64::from(outer_cone) - std::f64::consts::FRAC_PI_2).abs() < 1e-7 && inner_cone < outer_cone, "({inner_cone}, {outer_cone})");
        for prim in ["plain", "unshaped"] {
            assert_eq!(light_on(&r.scene, prim).map(|l| l.kind), Some(LightKind::Point), "{prim}");
        }
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert!(r.warnings[0].starts_with("light /R/shaped had a hard edge"), "{:?}", r.warnings);
    }

    /// What a UsdLux light or a UsdGeomCamera holds that the scene cannot is
    /// warned, naming the prim, and the rest is read: a cone wider than a
    /// spot's 90° reads as a point light; a colour temperature other than
    /// 6500 K, which is white, is not applied; an animated intensity or focal
    /// length is read at its first sample (the scene animates no light or
    /// camera property); an aperture offset is not applied, the view read
    /// centred; litegen customData that does not read (a colour that is a
    /// string) is passed over for the attributes; and an IES profile (the
    /// spec's non-goals warn IES lights by name), a shaping focus and a
    /// diffuse multiplier other than 1 are not applied. The IES and focus
    /// lights apply ShapingAPI with no cone angle, so each is also a spot of
    /// the schema's fallback 90°, with a hard edge softened and warned
    /// (`shaping_api_without_a_cone_angle_takes_its_fallback_of_90_degrees`).
    #[test]
    fn what_the_scene_cannot_hold_of_a_usd_light_or_camera_is_warned() {
        let r = read_text(
            &format!(
                r#"def Mesh "tri" {{ {QUAD} }}
                def SphereLight "wide" (
                    prepend apiSchemas = ["ShapingAPI"]
                ) {{
                    float inputs:shaping:cone:angle = 120
                }}
                def DistantLight "warm" {{
                    bool inputs:enableColorTemperature = 1
                    float inputs:colorTemperature = 3000
                    bool inputs:normalize = 1
                }}
                def DistantLight "white" {{
                    bool inputs:enableColorTemperature = 1
                    bool inputs:normalize = 1
                }}
                def SphereLight "pulse" {{
                    float inputs:intensity.timeSamples = {{
                        0: 4,
                        24: 8,
                    }}
                    bool inputs:normalize = 1
                }}
                def Camera "zoom" {{
                    float focalLength.timeSamples = {{
                        0: 0.5,
                        24: 1,
                    }}
                    float horizontalAperture = 0.36
                    float verticalAperture = 0.24
                }}
                def Camera "shifted" {{
                    float horizontalApertureOffset = 0.05
                }}
                def DistantLight "odd" (
                    customData = {{
                        dictionary litegen = {{
                            token type = "directional"
                            string color = "white"
                            float intensity = 3
                        }}
                    }}
                ) {{
                    float inputs:intensity = 2
                    bool inputs:normalize = 1
                }}
                def SphereLight "ies" (
                    prepend apiSchemas = ["ShapingAPI"]
                ) {{
                    asset inputs:shaping:ies:file = @beam.ies@
                }}
                def SphereLight "focused" (
                    prepend apiSchemas = ["ShapingAPI"]
                ) {{
                    float inputs:shaping:focus = 2
                }}
                def DistantLight "dull" {{
                    float inputs:diffuse = 0.5
                    bool inputs:normalize = 1
                }}"#
            ),
            &[],
        );
        let of = |prim: &str| -> Vec<&String> { r.warnings.iter().filter(|w| w.starts_with(&format!("light /R/{prim} ")) || w.starts_with(&format!("camera /R/{prim} "))).collect() };
        let says = |prim: &str, words: &[&str]| {
            let w = of(prim);
            assert!(w.len() == 1 && words.iter().all(|k| w[0].contains(k)), "{prim}: {w:?}");
        };
        says("wide", &["120", "point light"]);
        says("warm", &["3000 K"]);
        says("pulse", &["inputs:intensity", "first sample"]);
        says("zoom", &["focalLength", "first sample"]);
        says("shifted", &["aperture offset"]);
        says("odd", &["litegen", "color"]);
        for (prim, words) in [("ies", ["IES profile", "beam.ies"]), ("focused", ["focus", "2"])] {
            let w = of(prim);
            let told = w.iter().any(|w| words.iter().all(|k| w.contains(k)));
            assert!(w.len() == 2 && told && w.iter().any(|w| w.contains("hard edge")), "{prim}: {w:?}");
        }
        says("dull", &["diffuse", "0.5"]);
        assert!(of("white").is_empty(), "6500 K is white: {:?}", of("white"));
        assert_eq!(r.warnings.len(), 11, "{:?}", r.warnings);
        for node in ["ies", "focused", "dull"] {
            assert!(light_on(&r.scene, node).is_some(), "{node} is read without what it cannot carry");
        }
        assert_eq!(light_on(&r.scene, "wide").map(|l| l.kind), Some(LightKind::Point));
        assert_eq!(light_on(&r.scene, "warm").map(|l| l.color), Some([1.0; 3]));
        assert_eq!(light_on(&r.scene, "pulse").map(|l| l.intensity), Some(1.0), "4 at its first sample, normalised: 4 / 4 cd");
        let zoom = camera_on(&r.scene, "zoom").map(|c| c.projection);
        assert!(matches!(zoom, Some(Projection::Perspective { yfov, .. }) if (f64::from(yfov) - 0.47108996144172666).abs() < 1e-6), "{zoom:?}");
        assert!(camera_on(&r.scene, "shifted").is_some(), "read, centred");
        assert_eq!(light_on(&r.scene, "odd").map(|l| l.intensity), Some(2.0), "from inputs:intensity");
    }

    /// What a camera holds that USD cannot say to other readers: a
    /// perspective camera always has a horizontal aperture and a far
    /// clipping distance, so an unset aspect ratio is written as 16:9 and an
    /// infinite far plane as 1000 m (`fbx::UNSET_ASPECT`, `fbx::unset_far`,
    /// as FBX and Collada write them), and an orthographic view's apertures
    /// are sizes, so a mirrored one is written unmirrored. Each is reported,
    /// and litegen's own customData brings the scene's values back. A
    /// directional light never carries a range: `prepare` drops one, as
    /// glTF's writer does, unreported; only the bulb's range, which UsdLux
    /// cannot hold either, is reported (Ruling R27).
    #[test]
    fn what_usd_cannot_hold_of_a_camera_is_written_and_reported() {
        let mut s = fixtures::staged();
        let open = Projection::Perspective { yfov: 0.8, aspect: None, znear: 0.125, zfar: None };
        let mirrored = Projection::Orthographic { xmag: -3.0, ymag: 2.0, znear: 0.0625, zfar: 20.0 };
        s.cameras[0].projection = open;
        s.cameras[1].projection = mirrored;
        s.lights[0].range = Some(5.0);
        let w = write_scene(&s, MeshFormat::Usda, WriteOptions { self_contained: true, ..Default::default() }).unwrap();
        let named: Vec<&str> = w.losses.iter().filter(|l| l.kind == LossKind::Cameras).map(|l| l.what.as_str()).collect();
        assert_eq!(
            named,
            [
                "the viewport aspect ratio of 1 camera(s) [persp_camera]",
                "the infinite far plane of 1 camera(s) [persp_camera]",
                "the mirroring of 1 orthographic camera(s) [ortho_camera]",
            ],
            "{:?}",
            w.losses
        );
        let lit: Vec<&str> = w.losses.iter().filter(|l| l.kind == LossKind::Lights).map(|l| l.what.as_str()).collect();
        assert_eq!(lit, ["the range of 1 light(s) [bulb]"], "{:?}", w.losses);
        let opts = WriteOptions { self_contained: true, ..Default::default() };
        assert_eq!(MeshFormat::Usda.prepare(&s, opts).unwrap().lights[0].range, None, "prepare models the sun as it reads back");
        let text = String::from_utf8(w.package.files[0].bytes.clone()).unwrap();
        let layer = super::super::usda::parse(text.as_bytes(), "model.usda").unwrap();
        let attr = |prim: &str, name: &str| layer.specs.get(prim).and_then(|s| s.attr(name)).and_then(|a| a.default.clone());
        let persp = "/staged/rig/persp_camera_node";
        assert_eq!(attr(persp, "horizontalAperture").and_then(|v| v.as_f64()).map(|v| v as f32), Some(0.42666668), "16:9 of the 0.24 film back");
        assert_eq!(attr(persp, "clippingRange"), Some(Value::Tuple(vec![0.125, 1000.0])), "1000 m");
        assert_eq!(attr("/staged/rig/ortho_camera_node", "horizontalAperture").and_then(|v| v.as_f64()), Some(60.0), "unmirrored");
        let sun = &text[text.find("def DistantLight \"sun_node\"").expect("the sun is written")..];
        let sun = &sun[..sun.find("inputs:").unwrap()];
        assert!(sun.contains("dictionary litegen") && !sun.contains("range"), "{sun}");
        let back = read_usda(text.as_bytes(), &NoSidecars).unwrap().scene;
        assert_eq!(camera_on(&back, "persp_camera_node").map(|c| c.projection), Some(open));
        assert_eq!(camera_on(&back, "ortho_camera_node").map(|c| c.projection), Some(mirrored));
        assert_eq!(light_on(&back, "sun_node").map(|l| l.range), Some(None));
        let bare = read_usda(without_litegen_data(&text).as_bytes(), &NoSidecars).unwrap().scene;
        let p = camera_on(&bare, "persp_camera_node").map(|c| c.projection);
        let Some(Projection::Perspective { aspect, zfar, .. }) = p else { panic!("{p:?}") };
        assert!(aspect.is_some_and(|a| (a - 16.0 / 9.0).abs() < 1e-6) && zfar == Some(1000.0), "{p:?}");
        assert_eq!(camera_on(&bare, "ortho_camera_node").map(|c| c.projection), Some(Projection::Orthographic { xmag: 3.0, ymag: 2.0, znear: 0.0625, zfar: 20.0 }));
    }

    /// `staged()`'s lights and cameras, lean, for the sweep: each on a root
    /// node at its place in staged, translated only, and one bare triangle
    /// of the ground, which a USD file needs to be written at all. No rig,
    /// clip, cube, normals or materials: the paths that read those have
    /// sweeps of their own, and a sweep reads its file nine times per byte,
    /// plus 10,000 (dae.rs's `lean_staged` is the pattern).
    fn lean_staged() -> Scene {
        let full = fixtures::staged();
        let mut ground = full.meshes[0].primitives[0].clone();
        ground.positions.truncate(3);
        ground.normals = None;
        ground.indices = vec![0, 1, 2];
        ground.material = None;
        let mut s = Scene { name: full.name.clone(), lights: full.lights.clone(), cameras: full.cameras.clone(), ..Default::default() };
        s.meshes.push(SceneMesh { primitives: vec![ground], ..Default::default() });
        s.nodes.push(Node { name: Some("ground_node".into()), mesh: Some(0), ..Default::default() });
        for n in &full.nodes[3..8] {
            let translation = n.transform.trs().expect("staged's nodes are TRS").0;
            s.nodes.push(Node { transform: Transform::Trs { translation, rotation: math::QUAT_IDENTITY, scale: [1.0; 3] }, children: Vec::new(), ..n.clone() });
        }
        s.roots = (0..6).collect();
        s
    }

    /// Lean staged as one USDA file ([`lean_staged`]) through the reader
    /// under `layer_sweep`: cut at every length, each bit of its first 4 KiB
    /// and of everything from its first camera prim on flipped in turn, and
    /// 2–8 bits flipped at random in 10,000 seeded cases. Every reading
    /// returns. The USDZ is not swept again: its layer is this one, and its
    /// entries are stored and CRC-checked, so a flip inside the layer is
    /// refused before it is read ([`usd_layer_sweeps`]).
    #[test]
    fn usd_light_and_camera_inputs_survive_truncation_and_bit_flips() {
        let name = "lean staged as USDA";
        let file = write_scene(&lean_staged(), MeshFormat::Usda, WriteOptions { self_contained: true, ..Default::default() }).unwrap().package.files[0].bytes.clone();
        let whole = read_scene(&file, MeshFormat::Usda, &NoSidecars).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((whole.scene.lights.len(), whole.scene.cameras.len()), (3, 2), "{name} carries every light and camera");
        let from = file.windows(10).position(|w| w == b"def Camera").unwrap_or_else(|| panic!("{name} has no camera prim"));
        layer_sweep(name, &file, from..file.len(), |bytes| {
            let _ = read_scene(bytes, MeshFormat::Usda, &NoSidecars);
        });
    }

    /// The same sweep over lean staged with its litegen customData stripped
    /// ([`without_litegen_data`]). The reader then takes every light and
    /// camera from its UsdLux and UsdGeomCamera attributes
    /// (`Reader::light_of`, `Reader::camera_of`), which the sweep above
    /// never reaches: there, every dictionary that reads returns first.
    /// Each bit from the first camera prim on, the light and camera prims
    /// and their attributes, is flipped in turn among the single flips.
    /// Every reading returns.
    #[test]
    fn usd_lights_and_cameras_without_the_litegen_data_survive_truncation_and_bit_flips() {
        let name = "lean staged as USDA, without its litegen data";
        let full = write_scene(&lean_staged(), MeshFormat::Usda, WriteOptions { self_contained: true, ..Default::default() }).unwrap().package.files[0].bytes.clone();
        let file = without_litegen_data(&String::from_utf8(full).unwrap()).into_bytes();
        assert!(!String::from_utf8_lossy(&file).contains("dictionary litegen"), "{name}: every dictionary stripped");
        let whole = read_scene(&file, MeshFormat::Usda, &NoSidecars).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((whole.scene.lights.len(), whole.scene.cameras.len()), (3, 2), "{name} carries every light and camera");
        let from = file.windows(10).position(|w| w == b"def Camera").unwrap_or_else(|| panic!("{name} has no camera prim"));
        layer_sweep(name, &file, from..file.len(), |bytes| {
            let _ = read_scene(bytes, MeshFormat::Usda, &NoSidecars);
        });
    }

    // ── points and lines (mesh-gaps item 10, Task 4) ────────────────────────

    /// `Points` and linear `BasisCurves`: read, written, and swept.
    mod points_and_lines {
        use super::super::super::scene::{LossKind, Topology};
        use super::*;

        /// The primitive of the mesh node `node` draws.
        fn prim_of<'a>(s: &'a Scene, node: &str) -> &'a Primitive {
            let n = s.nodes.iter().find(|n| n.name.as_deref() == Some(node)).unwrap_or_else(|| panic!("no node {node}: {:?}", names(s)));
            &s.meshes[n.mesh.unwrap_or_else(|| panic!("node {node} draws nothing"))].primitives[0]
        }

        fn draws_nothing(s: &Scene, node: &str) -> bool {
            s.nodes.iter().find(|n| n.name.as_deref() == Some(node)).unwrap_or_else(|| panic!("no node {node}: {:?}", names(s))).mesh.is_none()
        }

        const LINEAR: &str = "uniform token type = \"linear\"";

        /// A `BasisCurves` prim: `attrs`, then `counts` over `n` points on a zigzag.
        fn curves(name: &str, attrs: &str, counts: &str, n: usize) -> String {
            let points: Vec<String> = (0..n).map(|i| format!("({i}, {}, 0)", i % 2)).collect();
            format!("def BasisCurves \"{name}\" {{\n {attrs}\n int[] curveVertexCounts = [{counts}]\n point3f[] points = [{}]\n}}\n", points.join(", "))
        }

        const CLOUD: &str = r#"def Points "Cloud" {
            point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
            normal3f[] normals = [(0, 0, 1), (0, 1, 0), (1, 0, 0)] (interpolation = "vertex")
            color3f[] primvars:displayColor = [(1, 0, 0), (0, 1, 0), (0, 0, 1)] (interpolation = "vertex")
            float[] widths = [0.5] (interpolation = "constant")
        }
        "#;

        #[test]
        fn usda_points_and_linear_curves_read() {
            let ribbon = "normal3f[] normals = [(0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1)] (interpolation = \"vertex\")\n \
                          float[] widths = [0.1, 0.1, 0.1, 0.1] (interpolation = \"vertex\")";
            let body = [
                CLOUD.to_string(),
                curves("Loop", &format!("{LINEAR}\n uniform token wrap = \"periodic\""), "4", 4),
                curves("Open", &format!("{LINEAR}\n uniform token wrap = \"nonperiodic\"\n {ribbon}"), "4", 4),
                curves("Pinned", &format!("{LINEAR}\n uniform token wrap = \"pinned\""), "4", 4),
                curves("Cubic", "uniform token type = \"cubic\"\n uniform token basis = \"bspline\"", "4", 4),
                curves("Untyped", "", "4", 4),
            ]
            .concat();
            let r = read_text(&body, &[]);
            let s = &r.scene;

            let cloud = prim_of(s, "Cloud");
            assert_eq!(cloud.topology, Topology::Points);
            assert_eq!(cloud.indices, [0, 1, 2]);
            assert_eq!(cloud.positions, [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
            assert_eq!(cloud.normals, Some(vec![[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]]), "points carry their normals");
            assert_eq!(cloud.colors, Some(vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]]), "and their colours");

            let segments = |name: &str| {
                let p = prim_of(s, name);
                assert_eq!(p.topology, Topology::Lines, "{name}");
                p.indices.clone()
            };
            assert_eq!(segments("Loop"), [0, 1, 1, 2, 2, 3, 3, 0], "a periodic curve of 4 vertices has 4 segments, the closing one included");
            assert_eq!(segments("Open"), [0, 1, 1, 2, 2, 3], "a nonperiodic one has 3");
            assert_eq!(segments("Pinned"), [0, 1, 1, 2, 2, 3], "pinned is a cubic basis's notion: a linear curve reads it as nonperiodic");
            assert_eq!(prim_of(s, "Open").normals, None, "a curve's normals orient a ribbon; they are not shading normals");

            for name in ["Cubic", "Untyped"] {
                assert!(draws_nothing(s, name), "{name} is cubic: not a polyline");
                let path = format!("/R/{name}");
                assert!(r.warnings.iter().any(|w| w.contains(&path) && w.contains("cubic")), "{name} is warned about, as cubic: {:?}", r.warnings);
            }
            assert_eq!(s.drawable_count(), 3 + 4 + 3 + 3);
        }

        #[test]
        fn a_linear_prims_vertex_counts_split_it_into_curves() {
            let open = format!("{LINEAR}\n uniform token wrap = \"nonperiodic\"");
            let closed = format!("{LINEAR}\n uniform token wrap = \"periodic\"");
            let r = read_text(&[curves("Open", &open, "3, 2, 1", 6), curves("Closed", &closed, "3, 2, 1", 6)].concat(), &[]);
            assert_eq!(prim_of(&r.scene, "Open").indices, [0, 1, 1, 2, 3, 4], "n - 1 segments a curve, none between two curves");
            assert_eq!(prim_of(&r.scene, "Closed").indices, [0, 1, 1, 2, 2, 0, 3, 4, 4, 3], "n segments a curve, a loop of two being its segment both ways");
            assert_eq!(prim_of(&r.scene, "Open").positions.len(), 6, "the vertices are the points themselves");
            for name in ["Open", "Closed"] {
                let path = format!("/R/{name}");
                let said: Vec<&String> = r.warnings.iter().filter(|w| w.contains(&path) && w.contains("fewer than 2")).collect();
                assert_eq!(said.len(), 1, "the curve of one vertex is skipped, and said once for the prim: {:?}", r.warnings);
            }
        }

        #[test]
        fn counts_that_do_not_add_up_are_refused_without_panicking() {
            for (counts, n, what) in [
                ("3", 4, "a sum short of the points"),
                ("5", 4, "a sum past the points"),
                ("-1, 5", 4, "a negative count"),
                ("9223372036854775807, 9223372036854775807", 3, "counts that would overflow a sum"),
            ] {
                let text = usda(&curves("C", LINEAR, counts, n));
                let e = read_usda(text.as_bytes(), &NoSidecars).map(|_| ()).unwrap_err();
                assert!(matches!(&e, MeshError::Malformed(m) if m.contains("/R/C")), "{what}: {e:?}");
            }
        }

        /// Ruling I10-R7: on a `Points` prim each point is an element, so a
        /// `uniform` primvar holds one value per point; on curves it holds
        /// one per curve, and `varying`, `vertex` and `faceVarying` one per
        /// vertex. A count that is not that is ignored, and said.
        #[test]
        fn uniform_primvars_are_per_point_on_points_and_per_curve_on_curves() {
            let red = "(1, 0, 0)";
            let body = format!(
                r#"def Points "P" {{
                    point3f[] points = [(0, 0, 0), (1, 0, 0), (2, 0, 0)]
                    color3f[] primvars:displayColor = [(1, 0, 0), (0, 1, 0), (0, 0, 1)] (interpolation = "uniform")
                }}
                def Points "PShort" {{
                    point3f[] points = [(0, 0, 0), (1, 0, 0)]
                    color3f[] primvars:displayColor = [{red}] (interpolation = "uniform")
                }}
                def BasisCurves "C" {{
                    {LINEAR}
                    int[] curveVertexCounts = [2, 2]
                    point3f[] points = [(0, 0, 0), (1, 0, 0), (2, 0, 0), (3, 0, 0)]
                    color3f[] primvars:displayColor = [{red}, (0, 0, 1)] (interpolation = "uniform")
                }}
                def BasisCurves "CShort" {{
                    {LINEAR}
                    int[] curveVertexCounts = [2, 2]
                    point3f[] points = [(0, 0, 0), (1, 0, 0), (2, 0, 0), (3, 0, 0)]
                    color3f[] primvars:displayColor = [{red}] (interpolation = "uniform")
                }}
                def BasisCurves "CVarying" {{
                    {LINEAR}
                    int[] curveVertexCounts = [2, 2]
                    point3f[] points = [(0, 0, 0), (1, 0, 0), (2, 0, 0), (3, 0, 0)]
                    color3f[] primvars:displayColor = [(1, 0, 0), (0, 1, 0), (0, 0, 1), (1, 1, 1)] (interpolation = "varying")
                }}"#
            );
            let r = read_text(&body, &[]);
            let s = &r.scene;
            assert_eq!(prim_of(s, "P").colors, Some(vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]]), "one per point");
            let c = prim_of(s, "C");
            assert_eq!(c.indices, [0, 1, 2, 3]);
            let colors = c.colors.clone().expect("colours");
            assert_eq!(colors, [[1.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0], [0.0, 0.0, 1.0, 1.0]], "one per curve, on both its vertices");
            assert_eq!(prim_of(s, "CVarying").colors.as_ref().map(|c| c[3]), Some([1.0, 1.0, 1.0, 1.0]), "varying is one per vertex");
            for (name, needs) in [("PShort", 2), ("CShort", 2)] {
                assert_eq!(prim_of(s, name).colors, None, "{name}");
                let path = format!("/R/{name}");
                let said = format!("has 1 values where its interpolation needs {needs}");
                assert!(r.warnings.iter().any(|w| w.contains(&path) && w.contains(&said)), "{name}: {:?}", r.warnings);
            }
        }

        // ── writing ─────────────────────────────────────────────────────────

        /// A segment's or point's ends: position and colour, as bits.
        type End = ([u32; 3], Option<[u32; 4]>);

        fn end(p: &Primitive, i: u32) -> End {
            (p.positions[i as usize].map(f32::to_bits), p.colors.as_ref().map(|c| c[i as usize].map(f32::to_bits)))
        }

        /// Each segment as its two ends, in order. The vertex count is no
        /// part of it: a USD curve has no shared vertices (Ruling I10-R8).
        fn segments(p: &Primitive) -> Vec<[End; 2]> {
            p.indices.chunks_exact(2).map(|c| [end(p, c[0]), end(p, c[1])]).collect()
        }

        fn dots(p: &Primitive) -> Vec<End> {
            p.indices.iter().map(|&i| end(p, i)).collect()
        }

        /// The lines of the block of the prim whose header starts with `header`.
        fn block(layer: &str, header: &str) -> Vec<String> {
            let lines: Vec<&str> = layer.lines().collect();
            let start = lines.iter().position(|l| l.trim_start().starts_with(header)).unwrap_or_else(|| panic!("no {header} in\n{layer}"));
            let indent = lines[start].len() - lines[start].trim_start().len();
            let close = lines[start..].iter().position(|l| l.len() - l.trim_start().len() == indent && l.trim() == "}").expect("a closing brace");
            lines[start..start + close].iter().map(|l| l.trim().to_string()).collect()
        }

        #[test]
        fn wire_round_trips_through_usda_and_usdz() {
            let wire = fixtures::wire();
            for (fmt, single) in [(MeshFormat::Usda, false), (MeshFormat::Usda, true), (MeshFormat::Usdz, true)] {
                let (name, opts) = way(fmt, single);
                let w = write_scene(&wire, fmt, opts).unwrap_or_else(|e| panic!("{name}: {e}"));
                assert!(w.losses.iter().all(|l| !matches!(l.kind, LossKind::Points | LossKind::Lines)), "{name} holds both: {:?}", w.losses);
                let layer = root_layer(fmt, &w.package.files);
                let prim = block(&layer, "def BasisCurves \"wire_loop_node\"");
                for line in ["uniform token type = \"linear\"", "uniform token wrap = \"nonperiodic\"", "int[] curveVertexCounts = [2, 2, 2, 2]"] {
                    assert!(prim.iter().any(|l| l == line), "{name}: {line} in {prim:?}");
                }
                block(&layer, "def Points \"wire_points_node\"");

                let side = MemorySidecars { files: &w.package.files };
                let back = read_scene(&w.package.files[0].bytes, fmt, &side).unwrap_or_else(|e| panic!("{name}: {e}")).scene;
                let (a, b) = (prim_of(&wire, "wire_loop_node"), prim_of(&back, "wire_loop_node"));
                assert_eq!((b.topology, b.indices.len() / 2), (Topology::Lines, 4), "{name}");
                assert_eq!(segments(b), segments(a), "{name}: each segment's ends and their colours, bit for bit");
                assert_eq!(b.positions.len(), 8, "{name}: a vertex for each end of each segment (Ruling I10-R8)");
                let (a, b) = (prim_of(&wire, "wire_points_node"), prim_of(&back, "wire_points_node"));
                assert_eq!((b.topology, b.indices.len()), (Topology::Points, 9), "{name}");
                assert_eq!(dots(b), dots(a), "{name}: each point and its colour, bit for bit");
                assert_eq!(back.triangle_count(), wire.triangle_count(), "{name}");
                assert_eq!(back.drawable_count(), wire.drawable_count(), "{name}");
            }
        }

        /// (the prim's header, its extent, its widths and the line after them),
        /// for every prim of `layer` that has either.
        #[allow(clippy::type_complexity)]
        fn widths_of(layer: &str) -> Vec<(String, Option<Vec<f64>>, Option<(Vec<f64>, String)>)> {
            let numbers = |line: &str| -> Vec<f64> {
                let values = line.split_once('=').expect("an assignment").1;
                values.split(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | 'e' | '+'))).filter_map(|t| t.parse().ok()).collect()
            };
            let lines: Vec<&str> = layer.lines().map(str::trim).collect();
            let mut out: Vec<(String, Option<Vec<f64>>, Option<(Vec<f64>, String)>)> = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                if line.starts_with("def ") {
                    out.push((line.to_string(), None, None));
                } else if line.starts_with("float3[] extent") {
                    out.last_mut().unwrap().1 = Some(numbers(line));
                } else if line.starts_with("float[] widths") {
                    out.last_mut().unwrap().2 = Some((numbers(line), lines[i + 1].to_string()));
                }
            }
            out
        }

        fn near(a: f64, b: f64) -> bool {
            (a - b).abs() <= 1e-5 * b.abs().max(1.0)
        }

        /// The width of a point or a line is one constant, 0.25 % of the
        /// diagonal of the scene's bounds at rest (Ruling I10-R9: a
        /// centimetre where there is no diagonal), and the prim's extent is
        /// padded by half of it on every side; a Mesh has neither.
        #[test]
        fn usd_widths_follow_the_rule() {
            let wire = fixtures::wire();
            let (lo, hi) = pose::world_bounds(&wire, &pose::default_pose(&wire)).unwrap();
            let w = 0.0025 * (0..3).map(|k| (hi[k] - lo[k]).powi(2)).sum::<f64>().sqrt();
            assert!(w > 0.008 && w < 0.009, "a 2 m cube's diagonal is 3.46 m: {w}");
            for (fmt, single) in [(MeshFormat::Usda, true), (MeshFormat::Usdz, true)] {
                let (name, opts) = way(fmt, single);
                let layer = root_layer(fmt, &write_scene(&wire, fmt, opts).unwrap().package.files);
                let mut seen = 0;
                for (header, extent, widths) in widths_of(&layer) {
                    let drawn_as = |kind: &str| header.starts_with(&format!("def {kind} "));
                    let Some((widths, next)) = widths else {
                        assert!(!drawn_as("Points") && !drawn_as("BasisCurves"), "{name}: {header} has no widths");
                        continue;
                    };
                    seen += 1;
                    assert!(drawn_as("Points") || drawn_as("BasisCurves"), "{name}: {header} has widths");
                    assert!(widths.len() == 1 && near(widths[0], w), "{name}: {header}: {widths:?} is not {w}");
                    assert_eq!(next, "interpolation = \"constant\"", "{name}: {header}: one constant for the prim");
                    // The prim's own positions, padded.
                    let (own_lo, own_hi) = if drawn_as("Points") { ([-1.0, -1.0, -1.0], [1.0, -1.0, 1.0]) } else { ([-0.75, 1.0, -0.75], [0.75, 1.0, 0.75]) };
                    let extent = extent.unwrap_or_else(|| panic!("{name}: {header} has no extent"));
                    let want: Vec<f64> = (0..3).map(|k| own_lo[k] - w / 2.0).chain((0..3).map(|k| own_hi[k] + w / 2.0)).collect();
                    assert!(extent.len() == 6 && extent.iter().zip(&want).all(|(a, b)| near(*a, *b)), "{name}: {header}: extent {extent:?} is not {want:?}");
                }
                assert_eq!(seen, 2, "{name}: the loop and the grid");
            }

            // One point has no diagonal: a centimetre.
            let mut lone = Scene::default();
            lone.meshes.push(SceneMesh {
                primitives: vec![Primitive { positions: vec![[3.0, 4.0, 5.0]], indices: vec![0], topology: Topology::Points, ..Default::default() }],
                ..Default::default()
            });
            lone.nodes.push(Node { name: Some("lone".into()), mesh: Some(0), ..Default::default() });
            lone.roots = vec![0];
            let layer = root_layer(MeshFormat::Usda, &write_scene(&lone, MeshFormat::Usda, WriteOptions::default()).unwrap().package.files);
            let found = widths_of(&layer);
            let (_, extent, widths) = found.iter().find(|(h, ..)| h.starts_with("def Points ")).expect("the point");
            assert_eq!(widths.as_ref().map(|w| w.0.clone()), Some(vec![0.01]));
            let extent = extent.clone().unwrap();
            assert!(extent.iter().zip([2.995, 3.995, 4.995, 3.005, 4.005, 5.005]).all(|(a, b)| near(*a, b)), "{extent:?}");
        }

        /// What a points and a lines primitive need the layer to say, and
        /// what they never say: no faces, no subdivision, no normals on a
        /// curve (they would make it a ribbon).
        #[test]
        fn usd_writes_points_and_curves_without_faces() {
            let layer = String::from_utf8(write_scene(&tangled(), MeshFormat::Usda, WriteOptions::default()).unwrap().package.files[0].bytes.clone()).unwrap();
            let points = block(&layer, "def Points \"points_node\"");
            let curves = block(&layer, "def BasisCurves \"lines_node\"");
            for (name, prim) in [("Points", &points), ("BasisCurves", &curves)] {
                for never in ["int[] faceVertexCounts", "int[] faceVertexIndices", "uniform token subdivisionScheme", "uniform token orientation"] {
                    assert!(!prim.iter().any(|l| l.starts_with(never)), "{name} has {never}");
                }
                assert!(prim.iter().any(|l| l.starts_with("float[] widths")), "{name} has widths");
            }
            assert!(points.iter().any(|l| l == "point3f[] points = [(3, 0, 0), (1, 0, 0), (3, 0, 0), (0, 0, 0)]"), "{points:?}");
            assert!(points.iter().any(|l| l.starts_with("normal3f[] normals")), "points keep their normals: {points:?}");
            assert!(!points.iter().any(|l| l.starts_with("uniform token type")), "{points:?}");
            let line_points = "point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 0, 0), (1, 1, 0), (1, 1, 0), (0, 1, 0)]";
            assert!(curves.iter().any(|l| l == line_points), "{curves:?}");
            assert!(curves.iter().any(|l| l == "int[] curveVertexCounts = [2, 2, 2]"), "{curves:?}");
            assert!(!curves.iter().any(|l| l.starts_with("normal3f[] normals")), "a curve's normals would make it a ribbon: {curves:?}");
        }

        /// `prepare` leaves the normals of a line out (a curve's normals orient
        /// a ribbon), so the write says so: `losses` never says less than
        /// `prepare` does. A morph target's normals on a line go the same way,
        /// by themselves; the normals of a point are kept and unnamed, and a
        /// line with none has none to lose.
        #[test]
        fn usd_names_the_line_normals_it_leaves_out() {
            let normals_named = |scene: &Scene, fmt: MeshFormat, single: bool| -> Vec<String> {
                let (name, opts) = way(fmt, single);
                let w = write_scene(scene, fmt, opts).unwrap_or_else(|e| panic!("{name}: {e}"));
                w.losses.iter().filter(|l| l.kind == LossKind::Normals).map(|l| l.what.clone()).collect()
            };
            let mut targeted = tangled();
            {
                let line = &mut targeted.meshes[0].primitives[0];
                line.normals = None;
                line.targets = vec![super::super::super::scene::MorphTarget { positions: line.positions.clone(), normals: Some(vec![[0.0, 0.0, 1.0]; 4]) }];
                targeted.meshes[0].weights = vec![0.5];
            }
            let mut bare = tangled();
            bare.meshes[0].primitives[0].normals = None;
            let mut lone_points = tangled();
            lone_points.meshes[0].primitives[0].normals = None;
            lone_points.meshes.truncate(2);
            for (fmt, single) in USD_WAYS {
                let (name, _) = way(fmt, single);
                let one = vec!["normals of 1 line primitive(s)".to_string()];
                assert_eq!(normals_named(&tangled(), fmt, single), one, "{name}: a line's normals");
                assert_eq!(normals_named(&targeted, fmt, single), one, "{name}: a line's morph target's normals");
                assert_eq!(normals_named(&bare, fmt, single), Vec::<String>::new(), "{name}: the points' normals are kept, the line has none");
                assert_eq!(normals_named(&lone_points, fmt, single), Vec::<String>::new(), "{name}: points alone");
            }
            // The loss is the whole of what `prepare` does to them.
            for (fmt, single) in USD_WAYS {
                let (name, opts) = way(fmt, single);
                let prepared = fmt.prepare(&tangled(), opts).unwrap();
                assert!(prepared.meshes.iter().flat_map(|m| &m.primitives).filter(|p| p.topology == Topology::Lines).all(|p| p.normals.is_none()), "{name}");
            }
        }

        /// Lines over shared vertices with normals, uvs and colours, and
        /// points that repeat one vertex and leave two unnamed.
        fn tangled() -> Scene {
            let line = Primitive {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
                normals: Some(vec![[0.0, 0.0, 1.0]; 4]),
                uvs: vec![vec![[0.0, 0.0], [0.5, 0.0], [0.5, 0.5], [0.0, 0.5]]],
                colors: Some(vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0], [1.0, 1.0, 0.0, 1.0]]),
                indices: vec![0, 1, 1, 2, 2, 3],
                topology: Topology::Lines,
                ..Default::default()
            };
            let dots = Primitive {
                positions: (0..5).map(|i| [i as f32, 0.0, 0.0]).collect(),
                normals: Some(vec![[0.0, 1.0, 0.0]; 5]),
                uvs: vec![(0..5).map(|i| [i as f32 / 4.0, 0.25]).collect()],
                colors: Some((0..5).map(|i| [i as f32 / 4.0, 0.0, 0.5, 1.0]).collect()),
                indices: vec![3, 1, 3, 0],
                topology: Topology::Points,
                ..Default::default()
            };
            let mut s = Scene { name: Some("tangled".into()), ..Default::default() };
            s.meshes = vec![
                SceneMesh { name: Some("tangled_lines".into()), primitives: vec![line], ..Default::default() },
                SceneMesh { name: Some("tangled_points".into()), primitives: vec![dots], ..Default::default() },
            ];
            s.nodes = vec![
                Node { name: Some("lines_node".into()), mesh: Some(0), ..Default::default() },
                Node { name: Some("points_node".into()), mesh: Some(1), ..Default::default() },
            ];
            s.roots = vec![0, 1];
            s
        }

        /// `prepare` is what the reader gives back (the harness draws
        /// `to.prepare(from.prepare(x))`): each index a vertex of its own,
        /// line normals gone.
        #[test]
        fn usd_prepare_deindexes_as_the_reader_reads() {
            let scene = tangled();
            for (fmt, single) in [(MeshFormat::Usda, true), (MeshFormat::Usda, false), (MeshFormat::Usdz, true)] {
                let (name, opts) = way(fmt, single);
                let want = fmt.prepare(&scene, opts).unwrap_or_else(|e| panic!("{name}: {e}"));
                let (back, _) = round(&scene, fmt, single);
                let (lines, dots) = (prim_of(&want, "lines_node"), prim_of(&want, "points_node"));
                let at = |i: u32| scene.meshes[0].primitives[0].positions[i as usize];
                assert_eq!(lines.positions, [0, 1, 1, 2, 2, 3].map(at), "{name}: a segment's two ends, each a vertex");
                assert_eq!(lines.indices, [0, 1, 2, 3, 4, 5], "{name}");
                assert_eq!(lines.normals, None, "{name}: a curve's normals would make a ribbon");
                assert_eq!(dots.positions, [3.0, 1.0, 3.0, 0.0].map(|x| [x, 0.0, 0.0]), "{name}: a vertex a point, in index order");
                assert_eq!(dots.indices, [0, 1, 2, 3], "{name}");
                assert_eq!(dots.normals, Some(vec![[0.0, 1.0, 0.0]; 4]), "{name}: points keep their normals");
                for node in ["lines_node", "points_node"] {
                    let (a, b) = (prim_of(&want, node), prim_of(&back.scene, node));
                    assert_eq!(b.topology, a.topology, "{name} {node}");
                    assert_eq!(b.positions, a.positions, "{name} {node}: positions");
                    assert_eq!(b.indices, a.indices, "{name} {node}: indices");
                    assert_eq!(b.normals, a.normals, "{name} {node}: normals");
                    assert_eq!(b.colors, a.colors, "{name} {node}: colours");
                    assert_eq!(b.uvs, a.uvs, "{name} {node}: uvs");
                    assert_eq!(b.uvs.len(), 1, "{name} {node}: the uv set is there to be compared");
                }
            }
        }

        /// A primitive that names no material is written bound to the default
        /// one (`usd_write::default_material`: "DefaultMaterial", white,
        /// metallic 1, roughness 1), so a file never holds an unbound gprim and
        /// the reader reads that material back. `prepare` is what reads back,
        /// so it binds the same one first: the same material on each
        /// primitive, and the same materials in the same number when the
        /// scene's material-less primitives all have vertex colours or all
        /// lack them (the mixed case is below).
        /// (`zip::tests::a_package_zip_reads_back_as_its_container` counts them.)
        ///
        /// One shared copy is bound to every such primitive (Ruling I10-R15),
        /// so the writer names the variants as it always has. A scene that
        /// mixes a coloured primitive and a bare one is therefore prepared with
        /// one material and read back with two (`DefaultMaterial_vertexColor`
        /// and `DefaultMaterial`, one prim for each), as a scene material used
        /// both ways already is: the reader makes a material of each material
        /// prim.
        #[test]
        fn a_primitive_with_no_material_is_prepared_with_the_default_one_the_writer_binds() {
            let wire = fixtures::wire();
            // The same two, bare: no colours.
            let mut bare = wire.clone();
            for p in bare.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()).filter(|p| p.topology != Topology::Triangles) {
                p.colors = None;
            }
            // One coloured and one bare: the writer writes each its own variant.
            let mut mixed = wire.clone();
            mixed.meshes[2].primitives[0].colors = None;
            for (label, scene) in [("coloured", &wire), ("bare", &bare), ("mixed", &mixed)] {
                for (fmt, single) in [(MeshFormat::Usda, false), (MeshFormat::Usda, true), (MeshFormat::Usdz, true)] {
                    let (name, opts) = way(fmt, single);
                    let want = fmt.prepare(scene, opts).unwrap_or_else(|e| panic!("{label} {name}: {e}"));
                    let (back, files) = round(scene, fmt, single);
                    // The materials of the prepared scene: the cube's, and one default.
                    assert_eq!(want.materials.len(), 2, "{label} {name}: the cube's material and one default");
                    let split = label == "mixed";
                    assert_eq!(back.scene.materials.len(), want.materials.len() + usize::from(split), "{label} {name}: materials");
                    let layer = root_layer(fmt, &files);
                    assert_eq!(layer.contains("def Material \"DefaultMaterial_vertexColor\""), split, "{label} {name}: the colour variant is a prim of its own");
                    for node in ["wire_loop_node", "wire_points_node"] {
                        let material = |s: &Scene| prim_of(s, node).material.map(|m| s.materials[m].clone());
                        let prepared = material(&want).unwrap_or_else(|| panic!("{label} {name} {node}: prepared with a material"));
                        assert_eq!(prepared.name.as_deref(), Some("DefaultMaterial"), "{label} {name} {node}: the writer's default");
                        assert_eq!((prepared.metallic, prepared.roughness), (1.0, 1.0), "{label} {name} {node}: glTF's default");
                        assert_eq!(material(&back.scene), Some(prepared), "{label} {name} {node}: the material it reads back with");
                    }
                    let at = |s: &Scene, node: &str| prim_of(s, node).material;
                    assert_eq!(at(&want, "wire_loop_node"), at(&want, "wire_points_node"), "{label} {name}: prepared, one shared copy");
                    assert_eq!(
                        at(&back.scene, "wire_loop_node") != at(&back.scene, "wire_points_node"),
                        split,
                        "{label} {name}: read back, a material for each variant in use"
                    );
                    let cube = |s: &Scene| prim_of(s, "wire_cube_node").material;
                    assert_eq!(cube(&back.scene), cube(&want), "{label} {name}: the cube keeps its own");
                }
            }
        }

        /// Material-less triangles are written as they always were: bound to a
        /// `DefaultMaterial` that is white, metallic 1 and roughness 1, the
        /// vertex-colour variant (`DefaultMaterial_vertexColor`) a prim of its
        /// own when the scene has both kinds. `prepare` binds one shared copy
        /// of that material itself now (Ruling I10-R15), and the bytes do not
        /// change, for a scene of bare triangles, of coloured ones and of both:
        /// the writer called on the scene as it stands (no binding from
        /// `prepare`) writes the same file as on the prepared scene, as a USDA
        /// package, a single USDA and a USDZ.
        #[test]
        fn a_triangle_only_scene_with_no_material_writes_the_same_default_material_prepared_or_not() {
            let tri = |colored: bool| Primitive {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                colors: colored.then(|| vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]]),
                indices: vec![0, 1, 2],
                ..Default::default()
            };
            let scene_of = |name: &str, colored: &[bool]| {
                let mut scene = Scene { name: Some(name.into()), ..Default::default() };
                for (i, c) in colored.iter().enumerate() {
                    scene.meshes.push(SceneMesh { name: Some(format!("{name}_mesh{i}")), primitives: vec![tri(*c)], ..Default::default() });
                    scene.nodes.push(Node { name: Some(format!("{name}_node{i}")), mesh: Some(i), ..Default::default() });
                }
                scene.roots = (0..colored.len()).collect();
                scene
            };
            for (label, scene) in [("bare", scene_of("bare", &[false, false])), ("coloured", scene_of("coloured", &[true, true])), ("mixed", scene_of("mixed", &[true, false]))] {
                for (fmt, single) in [(MeshFormat::Usda, true), (MeshFormat::Usda, false), (MeshFormat::Usdz, true)] {
                    let (name, opts) = way(fmt, single);
                    let write = |s: &Scene| if fmt == MeshFormat::Usdz { write_usdz(s, opts) } else { write_usda(s, opts) }.unwrap();
                    let prepared = fmt.prepare(&scene, opts).unwrap();
                    assert_eq!(prepared.materials.len(), 1, "{label} {name}: prepare binds one shared default");
                    let (as_it_stands, with_prepare) = (write(&scene), write(&prepared));
                    assert_eq!(as_it_stands.files.len(), with_prepare.files.len(), "{label} {name}: files");
                    for (a, b) in as_it_stands.files.iter().zip(&with_prepare.files) {
                        assert_eq!(a.path, b.path, "{label} {name}: path");
                        assert!(a.bytes == b.bytes, "{label} {name} {}: the bytes the writer wrote do not depend on prepare's binding", a.path);
                    }
                    let layer = root_layer(fmt, &with_prepare.files);
                    for prim in ["DefaultMaterial"].into_iter().chain((label == "mixed").then_some("DefaultMaterial_vertexColor")) {
                        let material = block(&layer, &format!("def Material \"{prim}\""));
                        assert!(material.iter().any(|l| l == "float inputs:metallic = 1"), "{label} {name} {prim}: metallic 1: {material:?}");
                        assert!(material.iter().any(|l| l == "float inputs:roughness = 1"), "{label} {name} {prim}: roughness 1: {material:?}");
                    }
                    assert_eq!(layer.contains("def Material \"DefaultMaterial_vertexColor\""), label == "mixed", "{label} {name}: the variant is there for a mixed scene only");
                    assert!(!layer.contains("DefaultMaterial_1"), "{label} {name}: no variant is renamed");
                    assert!(layer.contains("def Material \"DefaultMaterial\""), "{label} {name}: DefaultMaterial");
                    assert!(!layer.contains("def Material \"displayColor\""), "{label} {name}: the reader's unbound fallback is never written");
                    let (back, _) = round(&scene, fmt, single);
                    if label == "mixed" {
                        assert_eq!(back.scene.materials.len(), 2, "{label} {name}: read back, a material for each variant");
                    } else {
                        assert_eq!(back.scene.materials, prepared.materials, "{label} {name}: prepare is what reads back");
                    }
                }
            }
        }

        /// USD's own write and read gates are of what is drawn: a file of
        /// points alone, or of lines alone, writes and reads back, as USDA
        /// (a package and a single file) and as USDZ.
        #[test]
        fn a_scene_of_points_or_lines_alone_writes_and_reads_back() {
            let wire = fixtures::wire();
            for (what, mesh, topology, count) in [("points", 2, Topology::Points, 9), ("lines", 1, Topology::Lines, 4)] {
                let mut alone = Scene { name: Some(what.into()), ..Default::default() };
                alone.meshes.push(wire.meshes[mesh].clone());
                alone.nodes.push(Node { name: Some(format!("{what}_node")), mesh: Some(0), ..Default::default() });
                alone.roots = vec![0];
                for (fmt, single) in [(MeshFormat::Usda, false), (MeshFormat::Usda, true), (MeshFormat::Usdz, true)] {
                    let (name, opts) = way(fmt, single);
                    let w = write_scene(&alone, fmt, opts).unwrap_or_else(|e| panic!("{what} as {name}: {e}"));
                    assert!(w.losses.is_empty(), "{what} as {name}: {:?}", w.losses);
                    let side = MemorySidecars { files: &w.package.files };
                    let back = read_scene(&w.package.files[0].bytes, fmt, &side).unwrap_or_else(|e| panic!("{what} as {name}: {e}")).scene;
                    let p = prim_of(&back, &format!("{what}_node"));
                    assert_eq!((p.topology, p.indices.len() / p.topology.indices_per_element()), (topology, count), "{what} as {name}");
                    assert_eq!(back.triangle_count(), 0);
                }
            }
        }

        /// The layer a points prim and a curves prim are read from,
        /// lean: one `Points` and one periodic linear `BasisCurves` of two
        /// curves, with the primvars the reader takes for them.
        const LEAN: &str = r#"def Points "Cloud" {
            point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
            normal3f[] normals = [(0, 0, 1), (0, 0, 1), (0, 0, 1)] (interpolation = "vertex")
            color3f[] primvars:displayColor = [(1, 0, 0), (0, 1, 0), (0, 0, 1)] (interpolation = "vertex")
            float[] widths = [0.1] (interpolation = "constant")
        }
        def BasisCurves "Loop" {
            uniform token type = "linear"
            uniform token wrap = "periodic"
            int[] curveVertexCounts = [4, 3]
            point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0), (2, 0, 0), (3, 0, 0), (2, 1, 0)]
            color3f[] primvars:displayColor = [(1, 0, 0), (0, 0, 1)] (interpolation = "uniform")
            float[] widths = [0.1] (interpolation = "constant")
        }"#;

        /// [`LEAN`] read whole (3 points and 4 + 3 segments), then through
        /// `layer_sweep` over everything from its first prim on: cut at every
        /// length, each bit of its first 4 KiB and of that span flipped in
        /// turn, and 2–8 bits flipped at random in 10,000 seeded cases. Every
        /// reading returns.
        #[test]
        fn usd_points_and_curves_survive_truncation_and_bit_flips() {
            let name = "lean points and curves as USDA";
            let file = usda(LEAN).into_bytes();
            let whole = read_scene(&file, MeshFormat::Usda, &NoSidecars).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!((whole.scene.drawable_count(), whole.scene.triangle_count()), (3 + 4 + 3, 0), "{name} itself carries them");
            let from = file.windows(8).position(|w| w == b"def Poin").expect("the points prim");
            layer_sweep(name, &file, from..file.len(), |bytes| {
                let _ = read_scene(bytes, MeshFormat::Usda, &NoSidecars);
            });
        }

        // ── budgets ─────────────────────────────────────────────────────────

        /// A prototype prim `proto` (its `def` line and body) referenced
        /// `refs` times from a small file, then `tail`.
        fn lattice(proto: &str, refs: usize, tail: &str) -> String {
            let mut t = format!("#usda 1.0\n{proto}\n");
            for i in 0..refs {
                t.push_str(&format!("def \"I{i}\" (references = </Proto>) {{}}\n"));
            }
            t.push_str(tail);
            t
        }

        fn thousand_points() -> String {
            (0..1000).map(|i| format!("({i}, 0, 0)")).collect::<Vec<_>>().join(", ")
        }

        /// The vertex ceiling holds for points and curves as for meshes:
        /// one 1,000-point prim referenced 4,100 times is 4.1M vertices from a
        /// small file.
        #[test]
        fn instancing_cannot_amplify_points_or_curves_past_the_vertex_ceiling() {
            let points = thousand_points();
            for (what, proto) in [
                ("points", format!("def Points \"Proto\" {{\n    point3f[] points = [{points}]\n}}")),
                (
                    "curves",
                    format!("def BasisCurves \"Proto\" {{\n    uniform token type = \"linear\"\n    int[] curveVertexCounts = [1000]\n    point3f[] points = [{points}]\n}}"),
                ),
            ] {
                let started = std::time::Instant::now();
                let text = lattice(&proto, 4100, "");
                let e = read_usda(text.as_bytes(), &NoSidecars).map(|_| ()).unwrap_err();
                assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("vertices")), "{what}: {e:?}");
                eprintln!("{what} vertex ceiling: {:?}", started.elapsed());
            }
        }

        /// Segments are charged against the index budget before any is made.
        /// A curve cannot reach it alone (its segments are at most its
        /// points, so its indices are at most twice them, and the vertex
        /// ceiling is a third of the index one), so it is filled first: a
        /// polygon of 100,002 corners on three points is 300,000 indices, and
        /// forty references to it are exactly the budget. The next segment is
        /// the one too many.
        #[test]
        fn a_curve_past_the_index_budget_is_refused_before_its_segments_are_made() {
            let corners: Vec<String> = (0..100_002).map(|i| (i % 3).to_string()).collect();
            let polygon = format!(
                "def Mesh \"Proto\" {{\n    int[] faceVertexCounts = [100002]\n    int[] faceVertexIndices = [{}]\n    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]\n}}",
                corners.join(", ")
            );
            let curve = |n: usize| -> String {
                let points: Vec<String> = (0..n).map(|i| format!("({i}, 0, 0)")).collect();
                format!("def BasisCurves \"C\" {{\n    uniform token type = \"linear\"\n    int[] curveVertexCounts = [{n}]\n    point3f[] points = [{}]\n}}\n", points.join(", "))
            };
            let started = std::time::Instant::now();
            // The proto and 39 references: 12,000,000 indices, which is the budget.
            let full = lattice(&polygon, 39, "");
            let r = read_usda(full.as_bytes(), &NoSidecars).unwrap_or_else(|e| panic!("exactly the budget: {e}"));
            assert_eq!(r.scene.triangle_count(), 40 * 100_000);
            // And one segment more.
            let over = lattice(&polygon, 39, &curve(2));
            let e = read_usda(over.as_bytes(), &NoSidecars).map(|_| ()).unwrap_err();
            assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("line indices")), "{e:?}");
            eprintln!("index budget: {:?}", started.elapsed());
        }

        /// The number of segments is a saturating sum: counts as large as
        /// `i64` allows are many segments, not a wrapped few.
        #[test]
        fn segment_counts_add_without_overflowing() {
            use super::super::super::usd_read::segment_count;
            assert_eq!(segment_count(&[4], false), 3);
            assert_eq!(segment_count(&[4], true), 4);
            assert_eq!(segment_count(&[3, 2, 1, 0, -5], false), 2 + 1, "a curve of fewer than 2 vertices has none");
            assert_eq!(segment_count(&[3, 2, 1, 0, -5], true), 3 + 2, "a loop of two goes there and back");
            assert_eq!(segment_count(&[i64::MAX, i64::MAX, i64::MAX], true), usize::MAX);
            assert_eq!(segment_count(&[i64::MAX, i64::MAX, i64::MAX], false), usize::MAX);
            assert_eq!(segment_count(&[], true), 0);
        }
    }
}
