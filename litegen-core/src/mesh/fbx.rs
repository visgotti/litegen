//! Autodesk FBX — binary 7100–7700 and ASCII in, binary 7400 out.
//!
//! # Why FBX is here at all
//!
//! FBX is what Unity, Unreal, Maya and 3ds Max import without a plugin, and
//! what Mixamo, Meshy and Tripo hand back for rigged and animated characters.
//! There is no specification: Autodesk's own guide says "the FBX file format
//! is not documented. Applications should use the FBX SDK", and the SDK's
//! licence forbids running it as a hosted service. So this codec is written
//! from the bytes of real files — SDK 2014 and 2020 output (Mixamo, 3ds Max,
//! Maya), Blender's and Assimp's — and from what the readers that matter
//! (three.js, the engine behind the dashboard's viewer; Assimp; ufbx, behind
//! Godot and Blender 4.5) demonstrably accept. Every fact it relies on is in
//! the research notes (`docs/superpowers/research/research-fbx.md`), with the
//! file it was measured in.
//!
//! The files are split by layer: [`super::fbx_dom`] is the container (records
//! and typed properties, binary and ASCII, plus the binary record writer),
//! [`super::fbx_read`] turns a document into a [`Scene`], [`super::fbx_write`]
//! turns a scene into a document. This file holds what they share and the
//! format's lossiness rules.
//!
//! # Conventions at the boundary
//!
//! - **Axes and units.** `GlobalSettings` names the file's up, front and
//!   right axes and its unit (`UnitScaleFactor`, centimetres per file unit).
//!   The reader converts to the scene's Y-up metres with ONE root node holding
//!   the axis change and the `UnitScaleFactor / 100` scale, so skins (whose
//!   inverse binds live in file units) and animation (whose keys do) stay
//!   consistent under it. The writer emits glTF's frame unchanged — it IS the
//!   FBX "Maya Y-up" frame — with metre data and `UnitScaleFactor = 100`, so
//!   readers that ignore the unit (three.js, Assimp) see the same numbers
//!   the glTF had and readers that honour it (Unity, Maya, Blender) see metres.
//! - **UVs.** FBX's V runs up (OpenGL); the scene's runs down. `v' = 1 - v`
//!   both ways. [`prepare`] puts the sets the kept maps sample first (base
//!   colour's, then the normal map's and emission's) and renumbers the
//!   bindings, because three.js's `FBXLoader` samples every texture from the
//!   first set.
//! - **Colours.** See [`srgb_encode`]: material and light colours are
//!   written sRGB and read back to linear, unless the caller asks for
//!   linear ([`FbxColorSpace`]).
//! - **Transforms.** A node's FBX local transform is
//!   `T·Roff·Rp·Rpre·R·Rpost⁻¹·Rp⁻¹·Soff·Sp·S·Sp⁻¹`; the reader evaluates all of
//!   it (and `InheritType`) and decomposes the result, the writer only ever
//!   writes `T·R·S` with Euler XYZ rotations.
//! - **Skins.** A cluster's `Transform` is the joint's inverse bind matrix
//!   (it maps mesh space to bone space, exactly glTF's `inverseBindMatrix`);
//!   `TransformLink` is the joint's world matrix at bind time.
//! - **Animation.** Keys are int64 ticks, 46,186,158,000 per second (FBX 2019.5
//!   opted some files into 141,120,000, flagged in the header). Rotations are
//!   Euler degrees, which do not interpolate like quaternions — so both
//!   directions resample: the reader wherever Euler and quaternion paths can
//!   part, the writer at 30 fps plus every source key.
//! - **Lights and cameras.** Each is a `NodeAttribute` of class `Light` or
//!   `Camera` on a model of the same class. FBX aims a light along its
//!   model's −Y and a camera along its +X with +Y up (ufbx v0.23.1, ufbx.h
//!   L1439 and L1617-L1618), where the scene aims both along −Z with +Y up.
//!   So a fixed quarter turn ([`Turn`]: Blender 5.2.3's `MAT_CONVERT_LIGHT`
//!   and `MAT_CONVERT_CAMERA`, fbx_utils.py L73-L74) goes on the model's own
//!   rotation, its inverse on each child's, and both on every key
//!   ([`reframe`]), so that nothing else moves. A model holds one thing, so
//!   [`prepare`] gives a light or camera that shares its node a child of its
//!   own. The values are in FBX's units; `fbx_write` and `fbx_read` cite
//!   the source of each rule, which is ufbx's or Blender's reading, never
//!   three.js's `FBXLoader`. Among the ways it departs from them at r183
//!   (`examples/jsm/loaders/FBXLoader.js`), with lights and cameras:
//!   - it turns no camera, so a camera looks along FBX's +X, 90° off
//!     (`createCamera`, L1064-L1153);
//!   - it builds no orthographic camera (L1137-L1140);
//!   - it takes yfov from `FieldOfView`, then from `FocalLength` against
//!     its own 35 mm film gauge across the wider side, ignoring `FilmWidth`,
//!     `FilmHeight` and `ApertureMode` (L1121-L1134; `PerspectiveCamera.js`
//!     L121, L164-L172, L219-L224). The writer's 35 mm film across the wider
//!     side makes three.js's yfov match ufbx's and Blender's;
//!   - it divides `NearPlane` and `FarPlane` by 1000 (L1097, L1104);
//!   - it aims a directional or spot light only at a `LookAtProperty`
//!     target (`setLookAtProperties`, L1423-L1459; the light branch,
//!     L1439-L1443). This writer writes none, so three.js aims such a light
//!     at the world origin, not along its axis;
//!   - it takes a spot's `InnerAngle`, a full cone, as its half-angle
//!     (L1242);
//!   - its penumbra is max(rad(`OuterAngle`), 1) (L1246-L1253);
//!   - it hard-codes `decay = 1` (L1225) where the file says the inverse
//!     square;
//!   - it reads a light's `Color` as sRGB (L1196), where ufbx and Blender
//!     read it as linear. `FbxColorSpace` chooses which reading a file is
//!     written for, as for material colours (Ruling R14).
//!
//!   A light's strength has the same choice, `LightUnits`: Blender reads
//!   `Intensity` / 100 as its own watts, where its glTF importer converts
//!   candela and lux at 683 lm/W. `Photometric` (the default) stores 100 ×
//!   the candela or lux, and `Blender` 100 × the energy Blender's glTF
//!   import gives.
//!
//! "Ruling R14" and its kin are mesh-gaps item 5's rulings (lights and
//! cameras), listed with their costs in the "Rulings" section of
//! `docs/superpowers/plans/2026-11-04-mesh-lights-cameras.md`.
//!
//! # What is refused
//!
//! FBX 6.x and older (a different object model: string names, `Properties60`),
//! encrypted files, and the bytes of any other container.

use super::ir::Mesh;
use super::math::{self, Mat4};
use super::package::Sidecars;
use super::scene::{AlphaMode, Capabilities, ImageFormat, LayerSupport, Loss, LossKind, MaterialSupport, Scene, TextureSlot, Wrap};
use super::scene::{AnimPath, LightKind, Node, Projection, Transform};
use super::{FbxColorSpace, LightUnits};
use super::{MeshError, MeshFormat, SceneRead, WriteOptions};

/// Ticks per second in every FBX key time, FBX 7.x's `KTime` unit.
pub(super) const TICKS_PER_SECOND: f64 = 46_186_158_000.0;
/// The unit SDK 2019.5 lets a writer opt into, signalled by
/// `FBXHeaderExtension.OtherFlags.TCDefinition` other than 127.
pub(super) const TICKS_PER_SECOND_2019: f64 = 141_120_000.0;
/// The version this module writes: the most widely importable binary layout
/// (Autodesk calls 7.5 "only compatible with the 2016 or later product
/// versions"), and the one Blender writes.
pub(super) const WRITE_VERSION: u32 = 7400;
/// Oldest version read. FBX 6.x names objects by string and stores
/// properties as `Properties60`; it is a different document model.
pub(super) const MIN_READ_VERSION: u32 = 7000;

pub(super) fn malformed(detail: impl Into<String>) -> MeshError {
    MeshError::Malformed(format!("fbx: {}", detail.into()))
}

pub(super) fn unsupported(detail: impl Into<String>) -> MeshError {
    MeshError::Unsupported(format!("fbx: {}", detail.into()))
}

fn not_fbx(detail: impl Into<String>) -> MeshError {
    MeshError::NotThisFormat { expected: MeshFormat::Fbx, detail: detail.into() }
}

// ─── zlib ───────────────────────────────────────────────────────────────────
//
// Kept in this file: `miniz_oxide` is allowed in exactly one FBX file (see
// `isolation_tests.rs`), and these two are all the format needs of it.

/// Inflate a zlib (RFC 1950) stream that must produce exactly `expected`
/// bytes. The limit is the declared size, so a stream that would inflate
/// further stops there instead of allocating whatever it describes.
pub(super) fn inflate_zlib(data: &[u8], expected: usize, what: &str) -> Result<Vec<u8>, MeshError> {
    let out = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, expected)
        .map_err(|e| malformed(format!("{what}: compressed array does not inflate to its declared size ({:?})", e.status)))?;
    if out.len() != expected {
        return Err(malformed(format!(
            "{what}: compressed array inflates to {} bytes, not the declared {expected}",
            out.len()
        )));
    }
    Ok(out)
}

/// zlib-compress an array payload. Level 1 is what makes the header `78 01`,
/// the header every one of the 1,617 compressed arrays in four SDK files
/// carries; it is also the fastest level, and mesh doubles barely compress at
/// any level.
pub(super) fn deflate_zlib(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 1)
}

// ─── colour ─────────────────────────────────────────────────────────────────

/// Linear → sRGB for a colour written to FBX.
///
/// FBX does not say what space its colours are in, and its readers disagree:
/// three.js's FBXLoader decodes `DiffuseColor`, `EmissiveColor`, `Specular`,
/// a light's `Color` (L1196 at r183) and every vertex colour as sRGB
/// (`ColorManagement.colorSpaceToWorking(…, SRGBColorSpace)`), and Blender
/// writes and reads vertex colours as sRGB by default, while Blender's
/// material and light import, Unity URP's MaterialDescription path, ufbx and
/// assimp take material and light colours as linear (research notes §4.2,
/// item 5). The scene is linear, so whichever space is written, some reader
/// is off. The default is sRGB: encoded here and decoded on read
/// ([`srgb_decode`]), so three.js draws a converted file exactly as its glTF
/// source, and a linear reader sees the colour slightly lighter. A caller
/// whose pipeline reads FBX material and light colours as linear asks for
/// [`FbxColorSpace::Linear`] ([`encode_color`]); vertex colours stay sRGB, as
/// every reader above takes them. A light's colour is read back past 1
/// unclamped (`fbx_read::light_color`, Ruling R14).
pub(super) fn srgb_encode(linear: f32) -> f64 {
    super::math::linear_to_srgb(linear) as f64
}

/// sRGB → linear for a colour read from FBX. See [`srgb_encode`].
pub(super) fn srgb_decode(encoded: f64) -> f32 {
    super::math::srgb_to_linear(encoded as f32)
}

/// A material or light colour as stored in `space`: sRGB by default
/// ([`srgb_encode`]), or the linear value itself when the caller's pipeline
/// reads FBX colours as linear.
pub(super) fn encode_color(linear: f32, space: FbxColorSpace) -> f64 {
    match space {
        FbxColorSpace::Srgb => srgb_encode(linear),
        FbxColorSpace::Linear => linear as f64,
    }
}

/// [`encode_color`]'s inverse.
pub(super) fn decode_color(stored: f64, space: FbxColorSpace) -> f32 {
    match space {
        FbxColorSpace::Srgb => srgb_decode(stored),
        FbxColorSpace::Linear => stored as f32,
    }
}

// ─── API ────────────────────────────────────────────────────────────────────

/// Parse the container (binary or ASCII) into a document.
pub(super) fn parse(bytes: &[u8]) -> Result<super::fbx_dom::Dom<'_>, MeshError> {
    use super::fbx_dom;
    let dom = if bytes.starts_with(fbx_dom::BINARY_MAGIC) {
        fbx_dom::parse_binary(bytes)?
    } else if bytes.starts_with(&fbx_dom::BINARY_MAGIC[..18]) {
        // "Kaydara FBX Binary" followed by something other than two spaces and
        // a NUL: seen when a binary FBX went through a text-mode transfer that
        // replaced its NUL bytes — the document is destroyed, and saying so is
        // more useful than "not an FBX".
        return Err(malformed(
            "binary header magic is damaged (its NUL byte was replaced); the file was corrupted in transfer",
        ));
    } else if fbx_dom::looks_like_ascii(bytes) {
        fbx_dom::parse_ascii(bytes)?
    } else {
        return Err(not_fbx(
            "neither the binary magic `Kaydara FBX Binary` nor an ASCII FBX header (`; FBX` / `FBXHeaderExtension:`)",
        ));
    };
    if dom.version < MIN_READ_VERSION {
        return Err(unsupported(format!(
            "FBX version {} is FBX 6.x or older, a different document model (string object names, Properties60) \
             that this converter does not read",
            dom.version
        )));
    }
    Ok(dom)
}

/// Read an FBX (binary or ASCII) into a scene. Textures the file references
/// rather than embeds are looked up in `sidecars`.
pub fn read_scene(bytes: &[u8], sidecars: &dyn Sidecars) -> Result<SceneRead, MeshError> {
    read_scene_with(bytes, sidecars, FbxColorSpace::default(), LightUnits::default())
}

/// [`read_scene`], with its material and light colours stored in `colors`
/// and its light strengths in `light_units`.
pub fn read_scene_with(bytes: &[u8], sidecars: &dyn Sidecars, colors: FbxColorSpace, light_units: LightUnits) -> Result<SceneRead, MeshError> {
    // An ASCII document holding bytes that are not UTF-8 (a name or comment
    // in a legacy code page, from an older tool) is read with U+FFFD for
    // each, as binary FBX strings are, rather than refused.
    let recoded;
    let (bytes, lossy) = if !bytes.starts_with(&super::fbx_dom::BINARY_MAGIC[..18])
        && super::fbx_dom::looks_like_ascii(bytes)
        && std::str::from_utf8(bytes).is_err()
    {
        recoded = String::from_utf8_lossy(bytes).into_owned();
        (recoded.as_bytes(), true)
    } else {
        (bytes, false)
    };
    let dom = parse(bytes)?;
    let mut read = super::fbx_read::read(&dom, sidecars, colors, light_units)?;
    if lossy {
        read.warnings.push("the ASCII document is not UTF-8; each byte that is not was read as U+FFFD".into());
    }
    Ok(read)
}

/// Read an FBX into the flat, world-space view.
pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    let r = read_scene(bytes, &super::NoSidecars)?;
    let mesh = super::pose::flatten(&r.scene)?;
    if mesh.indices.is_empty() {
        return Err(MeshError::Empty);
    }
    Ok(mesh)
}

/// Write a scene as a binary FBX 7400 file. The scene should already be
/// [`prepare`]d (`super::write_scene` does that).
/// `fps` is the rate clips are keyed at and declared at (`TimeMode`);
/// `colors`, how material and light colours are stored; `light_units`, the
/// units of the light strengths.
pub fn write_scene(scene: &Scene, fps: f32, colors: FbxColorSpace, light_units: LightUnits) -> Result<Vec<u8>, MeshError> {
    super::fbx_write::write(scene, fps, colors, light_units)
}

/// What an FBX file holds, as this module writes and reads it.
///
/// Everything but PBR: FBX's only standard materials are Lambert and Phong.
/// The writer additionally records metallic and roughness factors (Blender's
/// encoding plus Stingray properties) so a round trip keeps them, but texture
/// slots for metallic-roughness and occlusion have no Phong equivalent.
/// Sampler filters have no FBX field at all.
pub fn capabilities() -> Capabilities {
    Capabilities {
        hierarchy: true,
        normals: true,
        uv_sets: 8,
        vertex_colors: true,
        materials: MaterialSupport::Phong,
        textures: true,
        skinning: true,
        morph_targets: true,
        animation: true,
        multiple_meshes: true,
        // No filter fields in FBX; WrapModeU/V (repeat or clamp) exist and
        // three.js honours them; texture transforms are baked into the UVs
        // by `pose::degrade`, which renders identically and keeps the
        // rotation FBX's Translation/Scaling cannot express.
        texture_filters: false,
        texture_wrap: true,
        texture_transforms: false,
        layers: LayerSupport::none(),
        // `NodeAttribute`s of class `Light` and `Camera` (see the module
        // doc); what they cannot say of a camera is `losses`'.
        lights: true,
        cameras: true,
        // Point and line primitives are not written: `prepare` drops them,
        // and `scene::losses` names them.
        points: false,
        lines: false,
    }
}

/// The texture slots FBX keeps, the ones its Phong reduction keeps
/// (`pose::kept_texture_slots`): base colour, normal map and emission.
/// [`prepare`] orders uv sets by these alone, once the others are gone.
const TEXTURED_SLOTS: &[TextureSlot] = &[TextureSlot::BaseColor, TextureSlot::Normal, TextureSlot::Emissive];

/// The scene reduced to exactly what [`write_scene`] encodes: the generic
/// Phong reduction, then what FBX's material and sampler model cannot say.
///
/// Once every binding FBX drops is gone (the Phong reduction's, and those
/// whose image is not PNG or JPEG), and before a mesh's primitives are lined
/// up into its one geometry, each primitive's uv sets are put in the order its
/// material's remaining textures sample them: base colour's set first, then
/// the normal map's and emission's, then the rest by index, with every
/// binding renumbered ([`super::pose::order_sampled_sets_first`] over
/// [`TEXTURED_SLOTS`]). three.js r183's `FBXLoader` samples every texture
/// from the first `LayerElementUV`: its `parseTexture` never sets a
/// texture's `channel`, and nothing reads the `UVSet` this writer names
/// (`docs/superpowers/research/research-consumers.md` Q7). Readers that
/// follow `UVSet` draw the same either way. A map or an image FBX drops never
/// takes set 0, and a scene textured on set 0 writes unchanged.
pub fn prepare(scene: &Scene, opts: WriteOptions) -> Result<Scene, MeshError> {
    let _ = opts;
    // Skinned meshes split as the writer splits them first, so a rigid part
    // under a shear is baked with the rest; then FBX's TRS transforms: a
    // sheared one is baked or approximated (`pose::bake_shear`), and
    // `losses` says which.
    let mut s = super::glb::hoist_skinned_meshes(&super::pose::degrade(scene, &capabilities())?);
    super::pose::bake_shear(&mut s);
    // The node scale only: three.js decomposes node transforms, but applies
    // scale keys directly.
    super::pose::nonsingular_scales(&mut s, false);
    // Only PNG and JPEG travel (every FBX consumer decodes those; three.js
    // picks the decoder from the file extension and knows no others that the
    // SDK tools share). `scene::losses` names the dropped images.
    let images: Vec<ImageFormat> = s.images.iter().map(|i| i.format).collect();
    let textures: Vec<usize> = s.textures.iter().map(|t| t.image).collect();
    let portable = |t: usize| textures.get(t).and_then(|i| images.get(*i)).is_some_and(|f| f.is_universal());
    for m in s.materials.iter_mut() {
        for (_, r) in m.texture_slots_mut() {
            if r.as_ref().is_some_and(|r| !portable(r.texture)) {
                *r = None;
            }
        }
        // FBX has no per-material double-sidedness, alpha test or unlit flag
        // (three.js builds every FBX material single-sided, opaque unless its
        // opacity is below one, and lit): `losses` names each.
        m.double_sided = false;
        m.unlit = false;
        match m.alpha_mode {
            AlphaMode::Mask => m.alpha_mode = AlphaMode::Opaque,
            AlphaMode::Blend if m.base_color[3] >= 1.0 => m.alpha_mode = AlphaMode::Opaque,
            _ => {}
        }
        m.alpha_cutoff = 0.5;
        if m.alpha_mode == AlphaMode::Opaque {
            // An opaque material's alpha is ignored by definition; FBX stores
            // opacity, so keeping a meaningless value would only make the
            // round trip disagree with itself.
            m.base_color[3] = 1.0;
        }
        if m.occlusion_texture.is_none() {
            m.occlusion_strength = 1.0;
        }
        // Under a map, only a factor scales it (`fbx_write::material_record`):
        // the base colour is the map alone, emission the map times one grey.
        if m.base_color_texture.is_some() {
            m.base_color = [1.0, 1.0, 1.0, m.base_color[3]];
        }
        if m.emissive_texture.is_some() {
            let g = m.emissive.iter().cloned().fold(0.0f32, f32::max);
            m.emissive = [g; 3];
        }
    }
    // Now that only the bindings FBX writes are left, and before a mesh's
    // primitives are lined up below.
    super::pose::order_sampled_sets_first(&mut s, TEXTURED_SLOTS);
    // One FBX geometry holds every primitive of a mesh, and its layer
    // elements cover all of its polygons: where primitives disagree about an
    // attribute, the writer fills the gaps the way the flat view does
    // (derived normals, zero UVs, white colours, zero normal offsets) and
    // drops tangents unless every primitive has them.
    for mesh in s.meshes.iter_mut() {
        let any_normals = mesh.primitives.iter().any(|p| p.normals.is_some());
        let all_tangents = any_normals && mesh.primitives.iter().all(|p| p.tangents.is_some());
        let sets = mesh.primitives.iter().map(|p| p.uvs.len()).max().unwrap_or(0);
        let any_colors = mesh.primitives.iter().any(|p| p.colors.is_some());
        let targets = mesh.target_count();
        let target_normals: Vec<bool> =
            (0..targets).map(|t| mesh.primitives.iter().any(|p| p.targets[t].normals.is_some())).collect();
        for p in mesh.primitives.iter_mut() {
            let n = p.positions.len();
            if any_normals && p.normals.is_none() {
                p.normals = Some(super::pose::derived_normals(&p.positions, &p.indices));
            }
            if !all_tangents {
                p.tangents = None;
            }
            while p.uvs.len() < sets {
                p.uvs.push(vec![[0.0; 2]; n]);
            }
            if any_colors && p.colors.is_none() {
                p.colors = Some(vec![[1.0; 4]; n]);
            }
            for (t, has) in target_normals.iter().enumerate() {
                if *has && p.targets[t].normals.is_none() {
                    p.targets[t].normals = Some(vec![[0.0; 3]; n]);
                }
            }
        }
    }
    for t in s.textures.iter_mut() {
        // FBX wraps are repeat (0) or clamp (1); there is no mirror.
        for w in [&mut t.sampler.wrap_s, &mut t.sampler.wrap_t] {
            if *w == Wrap::MirroredRepeat {
                *w = Wrap::Repeat;
            }
        }
    }
    super::pose::prune_unused_textures(&mut s);
    // Lights and cameras as the writer writes them and the reader reads
    // them back: each on a model of its own, a directional light without
    // the range glTF ignores too (`glb::light_json` writes none), and each
    // camera with what FBX always stores (`losses` names what that sets).
    separate_lights_and_cameras(&mut s);
    for l in s.lights.iter_mut().filter(|l| l.kind == LightKind::Directional) {
        l.range = None;
    }
    for c in s.cameras.iter_mut() {
        c.projection = held_projection(c.projection);
    }
    s.validate()?;
    Ok(s)
}

/// What FBX loses beyond the generic table (`scene::losses`), named.
///
/// Texture filter modes are not repeated here: `capabilities()` says FBX has
/// none, so the generic table already names them.
pub fn losses(scene: &Scene) -> Vec<Loss> {
    let mut out = Vec::new();
    let mut push = |kind: LossKind, what: String, why: &str| out.push(Loss::new(kind, what, format!("fbx {why}")));
    let count = |f: &dyn Fn(&super::scene::Material) -> bool| scene.materials.iter().filter(|m| f(m)).count();
    let n = count(&|m| m.double_sided);
    if n > 0 {
        push(
            LossKind::Materials,
            format!("double-sidedness of {n} material(s)"),
            "has no per-material double-sided flag; faces render one-sided",
        );
    }
    let n = count(&|m| m.alpha_mode == AlphaMode::Mask);
    if n > 0 {
        push(
            LossKind::Alpha,
            format!("alpha cutoff of {n} material(s)"),
            "has no alpha test; masked materials become opaque",
        );
    }
    let n = count(&|m| m.alpha_mode == AlphaMode::Blend && m.base_color[3] >= 1.0);
    if n > 0 {
        push(
            LossKind::Alpha,
            format!("texture-alpha blending of {n} material(s)"),
            "stores transparency as an opacity factor only; materials blended by texture alpha alone become opaque",
        );
    }
    let n = count(&|m| m.unlit);
    if n > 0 {
        push(LossKind::Materials, format!("unlit shading of {n} material(s)"), "has no unlit material; they become lit Phong");
    }
    // A colour beside a map: readers disagree whether it replaces or tints
    // the map, so the writer leaves only what both read the same.
    let portable = |r: &Option<super::scene::TextureRef>| {
        r.as_ref().is_some_and(|r| {
            scene.textures.get(r.texture).and_then(|t| scene.images.get(t.image)).is_some_and(|i| i.format.is_universal())
        })
    };
    let n = count(&|m| portable(&m.base_color_texture) && m.base_color[..3].iter().any(|c| *c != 1.0));
    if n > 0 {
        push(
            LossKind::Materials,
            format!("the base-colour tint of {n} textured material(s)"),
            "readers disagree about a colour beside a map (the SDK, Maya and Blender use the map alone; three.js and Unity multiply), so the map is written untinted",
        );
    }
    let n = count(&|m| portable(&m.emissive_texture) && (m.emissive[0] != m.emissive[1] || m.emissive[1] != m.emissive[2]));
    if n > 0 {
        push(
            LossKind::Materials,
            format!("the emissive tint of {n} textured material(s)"),
            "scales an emissive map by one factor; the tint's brightest channel is kept",
        );
    }
    let n = scene
        .textures
        .iter()
        .filter(|t| t.sampler.wrap_s == Wrap::MirroredRepeat || t.sampler.wrap_t == Wrap::MirroredRepeat)
        .count();
    if n > 0 {
        push(
            LossKind::TextureSampling,
            format!("mirrored wrapping of {n} texture(s)"),
            "wraps textures by repeat or clamp only; mirrored repeat becomes repeat",
        );
    }
    // What a camera holds that FBX cannot say ([`held_projection`]).
    let cameras = |pick: &dyn Fn(&Projection) -> bool| -> Vec<String> {
        let picked = scene.cameras.iter().enumerate().filter(|(_, c)| pick(&c.projection));
        picked.map(|(i, c)| c.name.clone().unwrap_or_else(|| format!("#{i}"))).collect()
    };
    let unset_aspect = cameras(&|p| matches!(p, Projection::Perspective { aspect: None, .. }));
    if !unset_aspect.is_empty() {
        push(
            LossKind::Cameras,
            format!("the viewport aspect ratio of {} camera(s) [{}]", unset_aspect.len(), unset_aspect.join(", ")),
            "always stores an aspect ratio; 16:9 (1920 × 1080, Blender's default render size) was written",
        );
    }
    let infinite = cameras(&|p| matches!(p, Projection::Perspective { zfar: None, .. }));
    if !infinite.is_empty() {
        push(
            LossKind::Cameras,
            format!("the infinite far plane of {} camera(s) [{}]", infinite.len(), infinite.join(", ")),
            "always stores a far plane; 1000 m (Blender's default clip end), or 10,000 times a near plane at or past it, was written",
        );
    }
    let mirrored = cameras(&|p| matches!(p, Projection::Orthographic { xmag, ymag, .. } if *xmag < 0.0 || *ymag < 0.0));
    if !mirrored.is_empty() {
        push(
            LossKind::Cameras,
            format!("the mirroring of {} orthographic camera(s) [{}]", mirrored.len(), mirrored.join(", ")),
            "stores an orthographic view's size, not its sign; the view was written unmirrored",
        );
    }
    out
}

// ─── lights and cameras ─────────────────────────────────────────────────────

/// A quarter turn about X or Y: the fixed rotation between a light's or a
/// camera's frame in the scene, which aims both along −Z with +Y up, and in
/// FBX, which aims a light along −Y (ufbx v0.23.1, ufbx.h L1439, "usually
/// -Y"; ufbx.c L23726-L23731 gives every light that `local_direction`) and
/// a camera along +X with +Y up (ufbx.h L1617-L1618, "FBX saves cameras
/// with +X forward and +Y up"). [`Turn::LIGHT`] and [`Turn::CAMERA`] are
/// Blender 5.2.3's `MAT_CONVERT_LIGHT` and `MAT_CONVERT_CAMERA`
/// (fbx_utils.py L73-L74), which its exporter multiplies onto a light's or
/// camera's local matrix (fbx_utils.py L1786-L1789) and its importer takes
/// off again (import_fbx.py L66-L67, L2462-L2470). Each takes FBX's axis
/// to the scene's −Z: an FBX model's local transform is the scene node's
/// times the turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Turn {
    /// The axis turned about: 0 for X, 1 for Y.
    axis: usize,
    /// +90° when true, −90° when false.
    positive: bool,
}

impl Turn {
    /// R_x(+90°), which takes −Y to −Z.
    pub(super) const LIGHT: Turn = Turn { axis: 0, positive: true };
    /// R_y(+90°), which takes +X to −Z and keeps +Y.
    pub(super) const CAMERA: Turn = Turn { axis: 1, positive: true };

    pub(super) fn inverse(self) -> Turn {
        Turn { positive: !self.positive, ..self }
    }

    /// As a quaternion.
    pub(super) fn quat(self) -> [f64; 4] {
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let mut q = [0.0, 0.0, 0.0, h];
        q[self.axis] = if self.positive { h } else { -h };
        q
    }

    /// `v` turned, exactly: a quarter turn swaps two components and negates
    /// one of them.
    pub(super) fn rotate(self, v: [f64; 3]) -> [f64; 3] {
        let [x, y, z] = v;
        match (self.axis, self.positive) {
            (0, true) => [x, -z, y],
            (0, false) => [x, z, -y],
            (_, true) => [z, y, -x],
            (_, false) => [-z, y, x],
        }
    }

    /// The scale `s` seen through the turn, `turn⁻¹ · diag(s) · turn` (or
    /// the other way round, which is the same): the two axes the turn
    /// swaps trade their factors, so `T·R·S·turn` is `T·(R·turn)·S'`, a TRS
    /// again.
    pub(super) fn swap_scale(self, s: [f64; 3]) -> [f64; 3] {
        let [x, y, z] = s;
        if self.axis == 0 {
            [x, z, y]
        } else {
            [z, y, x]
        }
    }

    /// As a matrix, exactly.
    pub(super) fn matrix(self) -> Mat4 {
        let mut m = math::IDENTITY;
        for c in 0..3 {
            let mut e = [0.0; 3];
            e[c] = 1.0;
            m[c * 4..c * 4 + 3].copy_from_slice(&self.rotate(e));
        }
        m
    }
}

/// The turn a node's light or camera gives its model in FBX: a node holds
/// one of them at most once [`separate_lights_and_cameras`] has run.
pub(super) fn turn_of(node: &Node) -> Option<Turn> {
    if node.camera.is_some() {
        Some(Turn::CAMERA)
    } else if node.light.is_some() {
        Some(Turn::LIGHT)
    } else {
        None
    }
}

/// Turns every node's local transform and animation so that each node with
/// a turn in `post` has its world frame turned by it, on the right, and
/// every other node keeps its world frame: node `n`'s local becomes
/// `post[parent]⁻¹ · local · post[n]`. The writer passes each light's and
/// camera's [`Turn`], the reader each one's inverse, so a light or camera
/// faces FBX's way in the file and the scene's way in the scene while its
/// children, and every other node, stay where they were, at rest and at
/// every key. Blender's importer does the same with a light or camera
/// node's `post_matrix` and the inverse as each child's `pre_matrix`
/// (import_fbx.py L2372-L2380, L2462-L2478). Translation keys turn with
/// the parent's inverse, rotation keys with both, scale keys swap with the
/// node's own turn ([`Turn::swap_scale`]); every map is linear, so a
/// CUBICSPLINE channel's tangents turn with its values.
pub(super) fn reframe(scene: &mut Scene, post: &[Option<Turn>]) {
    let own = |n: usize| post.get(n).copied().flatten();
    let pre: Vec<Option<Turn>> = scene.parents().iter().map(|p| p.and_then(own).map(Turn::inverse)).collect();
    let turned = |a: Option<Turn>, q: [f64; 4], b: Option<Turn>| {
        let q = a.map_or(q, |a| math::quat_mul(a.quat(), q));
        b.map_or(q, |b| math::quat_mul(q, b.quat()))
    };
    for (n, node) in scene.nodes.iter_mut().enumerate() {
        let (a, b) = (pre[n], own(n));
        if a.is_none() && b.is_none() {
            continue;
        }
        node.transform = match node.transform {
            Transform::Trs { translation, rotation, scale } => Transform::Trs {
                translation: a.map_or(translation, |a| a.rotate(translation)),
                rotation: math::quat_normalize(turned(a, rotation, b)),
                scale: b.map_or(scale, |b| b.swap_scale(scale)),
            },
            Transform::Matrix(m) => {
                let m = a.map_or(m, |a| math::mat_mul(&a.matrix(), &m));
                Transform::Matrix(b.map_or(m, |b| math::mat_mul(&m, &b.matrix())))
            }
        };
    }
    for ch in scene.animations.iter_mut().flat_map(|a| a.channels.iter_mut()) {
        let (a, b) = (pre.get(ch.node).copied().flatten(), own(ch.node));
        match ch.path {
            AnimPath::Translation => {
                let Some(a) = a else { continue };
                for v in ch.values.chunks_exact_mut(3) {
                    let r = a.rotate([v[0].into(), v[1].into(), v[2].into()]);
                    v.copy_from_slice(&r.map(|x| x as f32));
                }
            }
            AnimPath::Rotation if a.is_some() || b.is_some() => {
                for v in ch.values.chunks_exact_mut(4) {
                    let q = turned(a, [v[0].into(), v[1].into(), v[2].into(), v[3].into()], b);
                    v.copy_from_slice(&q.map(|x| x as f32));
                }
            }
            AnimPath::Scale => {
                let Some(b) = b else { continue };
                for v in ch.values.chunks_exact_mut(3) {
                    let s = b.swap_scale([v[0].into(), v[1].into(), v[2].into()]);
                    v.copy_from_slice(&s.map(|x| x as f32));
                }
            }
            AnimPath::Rotation | AnimPath::Weights => {}
        }
    }
}

/// Gives each light and camera that shares its node with a mesh, a skin
/// joint or the other one an untransformed child node of its own, named
/// for its node (`{name}_light`, `{name}_camera`), so that every FBX model
/// holds one thing. A model's class is what it holds (`Mesh`, `LimbNode`,
/// `Light`, `Camera`; Blender's importer keys the light and camera turns on
/// it, import_fbx.py L2280, L2465-L2468), and the turn its light or camera
/// needs would swing its mesh, its joint or its other attribute. A node
/// with a light and a camera keeps the light. Nothing moves: the children
/// stand where their nodes do.
pub(super) fn separate_lights_and_cameras(s: &mut Scene) {
    let joints = s.joint_nodes();
    for n in 0..s.nodes.len() {
        let node = &s.nodes[n];
        let crowded = node.mesh.is_some() || joints.get(n).copied().unwrap_or(false);
        let light = node.light.filter(|_| crowded);
        let camera = node.camera.filter(|_| crowded || node.light.is_some());
        let base = node.name.clone();
        for (light, camera, suffix) in [(light, None, "light"), (None, camera, "camera")] {
            if light.is_none() && camera.is_none() {
                continue;
            }
            let name = base.as_ref().map(|b| format!("{b}_{suffix}"));
            s.nodes.push(Node { name, light, camera, ..Default::default() });
            let child = s.nodes.len() - 1;
            let node = &mut s.nodes[n];
            node.children.push(child);
            if light.is_some() {
                node.light = None;
            } else {
                node.camera = None;
            }
        }
    }
}

/// The aspect ratio written for a camera with none (glTF's "the
/// viewport's"): 16:9, Blender's default render size, 1920 × 1080 (Blender
/// 5.2.3, `DNA_scene_types.h` L871 and L875). FBX always stores one.
pub(super) const UNSET_ASPECT: f32 = 16.0 / 9.0;

/// The far plane written for a perspective camera with none (glTF's
/// infinite): 1000 m, Blender's default clip end (Blender 5.2.3,
/// `DNA_camera_types.h` L218), or, for a near plane already at or past it,
/// 10,000 × the near plane, the ratio of Blender's defaults (0.1 m to
/// 1000 m). FBX always stores one.
pub(super) fn unset_far(znear: f32) -> f32 {
    if znear < 1000.0 {
        1000.0
    } else {
        let far = znear * 10_000.0;
        if far.is_finite() {
            far
        } else {
            f32::MAX
        }
    }
}

/// What FBX holds of a projection, so what reads back: always an aspect
/// ratio ([`UNSET_ASPECT`]) and a far plane ([`unset_far`]), and an
/// orthographic view's size, never its sign (`OrthoZoom` and the aspect
/// ratio are sizes).
pub(super) fn held_projection(p: Projection) -> Projection {
    match p {
        Projection::Perspective { yfov, aspect, znear, zfar } => Projection::Perspective {
            yfov,
            aspect: Some(aspect.unwrap_or(UNSET_ASPECT)),
            znear,
            zfar: Some(zfar.unwrap_or_else(|| unset_far(znear))),
        },
        Projection::Orthographic { xmag, ymag, znear, zfar } => Projection::Orthographic { xmag: xmag.abs(), ymag: ymag.abs(), znear, zfar },
    }
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::fbx_dom::{split_object_name, Dom, RecId};
    use super::super::fixtures;
    use super::super::fixtures::lightcam;
    use super::super::fixtures::sweeps::{layer_sweep, span};
    use super::super::package::{MemorySidecars, NoSidecars, PackageFile};
    use super::super::pose;
    use super::super::scene::{
        AnimPath, Animation, Camera, Channel, Interpolation, Light, LightKind, Material, MorphTarget, Node, Primitive,
        Projection, SceneMesh, Skin, SkinWeights, Transform,
    };
    use super::super::math;
    use super::*;

    fn write_prepared(scene: &Scene) -> Vec<u8> {
        super::super::write_scene(scene, MeshFormat::Fbx, WriteOptions::default())
            .unwrap_or_else(|e| panic!("write: {e}"))
            .package
            .files
            .remove(0)
            .bytes
    }

    fn read_back(bytes: &[u8]) -> SceneRead {
        super::super::read_scene(bytes, MeshFormat::Fbx, &NoSidecars).unwrap_or_else(|e| panic!("read: {e}"))
    }

    /// World-space triangles of a posed scene.
    fn posed_triangles(scene: &Scene, pose: &pose::Pose) -> Vec<[[f64; 3]; 3]> {
        let mut out = Vec::new();
        for wp in pose::world_primitives(scene, pose).unwrap() {
            for t in wp.indices.chunks_exact(3) {
                let p = |i: u32| {
                    let v = wp.positions[i as usize];
                    [v[0] as f64, v[1] as f64, v[2] as f64]
                };
                out.push([p(t[0]), p(t[1]), p(t[2])]);
            }
        }
        out
    }

    /// Every triangle of `a` has a counterpart in `b` (same corners, same
    /// winding, any starting corner) within `eps`, and the counts agree.
    /// Matching rather than sorting: a sort reorders under rounding noise.
    fn assert_same_geometry(a: &Scene, pa: &pose::Pose, b: &Scene, pb: &pose::Pose, eps: f64, ctx: &str) {
        let ta = posed_triangles(a, pa);
        let tb = posed_triangles(b, pb);
        assert_eq!(ta.len(), tb.len(), "{ctx}: triangle count");
        let close = |x: &[[f64; 3]; 3], y: &[[f64; 3]; 3], r: usize| {
            (0..3).all(|k| (0..3).all(|c| (x[k][c] - y[(k + r) % 3][c]).abs() <= eps))
        };
        for (i, x) in ta.iter().enumerate() {
            let found = tb.iter().any(|y| (0..3).any(|r| close(x, y, r)));
            assert!(found, "{ctx}: triangle {i} {x:?} has no counterpart within {eps}");
        }
    }

    #[test]
    fn every_fixture_round_trips_through_fbx() {
        for (name, fixture) in fixtures::all() {
            let prepared = prepare(&fixture, WriteOptions::default()).unwrap();
            let bytes = write_prepared(&fixture);
            let back = read_back(&bytes);
            assert!(back.warnings.is_empty(), "{name}: unexpected warnings {:?}", back.warnings);
            let got = &back.scene;
            // Default pose.
            assert_same_geometry(&prepared, &pose::default_pose(&prepared), got, &pose::default_pose(got), 1e-4, name);
            // Animation, sampled between keys where interpolation matters.
            // Every clip: the writer gives each its own AnimationStack and the
            // reader returns each (the `clips` fixture has three).
            assert_eq!(got.animations.len(), prepared.animations.len(), "{name}: animations");
            for t in fixtures::sample_times(&fixture) {
                assert_same_geometry(
                    &prepared,
                    &pose::sample_pose(&prepared, 0, t),
                    got,
                    &pose::sample_pose(got, 0, t),
                    2e-3,
                    &format!("{name} @ {t}"),
                );
            }
            // Structure that must survive exactly.
            assert_eq!(got.skins.len(), prepared.skins.len(), "{name}: skins");
            let targets = |s: &Scene| s.meshes.iter().map(|m| m.target_count()).sum::<usize>();
            assert_eq!(targets(got), targets(&prepared), "{name}: morph targets");
            assert_eq!(got.images.len(), prepared.images.len(), "{name}: images");
            for (a, b) in got.images.iter().zip(&prepared.images) {
                assert_eq!(a.data, b.data, "{name}: image bytes must pass through untouched");
            }
            // Materials: colour, metallic and roughness come back exactly.
            for m in &prepared.materials {
                let other = got
                    .materials
                    .iter()
                    .find(|g| g.name == m.name)
                    .unwrap_or_else(|| panic!("{name}: material {:?} lost", m.name));
                for k in 0..4 {
                    assert!((other.base_color[k] - m.base_color[k]).abs() < 1e-6, "{name}: {:?} colour", m.name);
                }
                assert!((other.metallic - m.metallic).abs() < 1e-6, "{name}: metallic");
                assert!((other.roughness - m.roughness).abs() < 1e-6, "{name}: roughness");
                assert_eq!(other.base_color_texture.is_some(), m.base_color_texture.is_some(), "{name}: texture");
            }
        }
    }

    #[test]
    fn uvs_vertex_colours_and_normals_come_back_per_vertex() {
        let fixture = fixtures::textured();
        let back = read_back(&write_prepared(&fixture)).scene;
        // Every source vertex (position, normal, uv, colour) exists in the
        // read-back mesh of the same name.
        for m in &fixture.meshes {
            let got = back.meshes.iter().find(|g| g.name == m.name).expect("mesh by name");
            let key = |p: &Primitive, i: usize| -> Vec<i64> {
                let mut k: Vec<f32> = p.positions[i].to_vec();
                if let Some(n) = &p.normals {
                    k.extend(n[i]);
                }
                for set in &p.uvs {
                    k.extend(set[i]);
                }
                if let Some(c) = &p.colors {
                    k.extend(c[i]);
                }
                k.iter().map(|v| (*v as f64 * 1e5).round() as i64).collect()
            };
            let mut want: Vec<Vec<i64>> = m.primitives.iter().flat_map(|p| (0..p.positions.len()).map(move |i| key(p, i))).collect();
            let mut have: Vec<Vec<i64>> = got.primitives.iter().flat_map(|p| (0..p.positions.len()).map(move |i| key(p, i))).collect();
            want.sort();
            want.dedup();
            have.sort();
            have.dedup();
            assert_eq!(want, have, "{:?}", m.name);
        }
    }

    #[test]
    fn the_writer_emits_what_three_js_and_assimp_require() {
        let bytes = write_prepared(&fixtures::skinned());
        assert!(bytes.starts_with(b"Kaydara FBX Binary  \0\x1a\x00"));
        assert_eq!(u32::from_le_bytes(bytes[23..27].try_into().unwrap()), 7400);
        // The footer: fixed tail magic, version at 16-byte-aligned offset + 4.
        assert!(bytes.ends_with(&[0xF8, 0x5A, 0x8C, 0x6A, 0xDE, 0xF5, 0xD9, 0x7E, 0xEC, 0xE9, 0x0C, 0xE3, 0x75, 0x8F, 0x29, 0x0B]));
        assert_eq!(bytes.len() % 16, 0);
        let dom = parse(&bytes).unwrap();
        use super::super::fbx_dom::ROOT;
        for top in ["FBXHeaderExtension", "FileId", "CreationTime", "Creator", "GlobalSettings", "Documents", "References", "Definitions", "Objects", "Connections", "Takes"] {
            assert!(dom.child(ROOT, top).is_some(), "missing {top}");
        }
        let objects = dom.child(ROOT, "Objects").unwrap();
        // BindPose is mandatory for three.js to place a skinned mesh.
        assert!(dom.children_named(objects, "Pose").count() == 1);
        // Every material has a ShadingModel, every texture a FileName, every
        // blend shape channel FullWeights: three.js throws without them.
        for m in dom.children_named(objects, "Material") {
            assert!(dom.child(m, "ShadingModel").is_some());
        }
        for t in dom.children_named(objects, "Texture") {
            assert!(dom.child(t, "FileName").is_some());
        }
        // IDs below 2^53, unique names.
        let mut names = Vec::new();
        for o in dom.children(objects) {
            let p = dom.props(o).unwrap();
            let id = p[0].as_i64().unwrap();
            assert!(id > 0 && id < (1i64 << 53));
            if dom.name(o) == "Model" {
                names.push(p[1].as_bytes().unwrap().to_vec());
            }
        }
        let n = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), n, "model names must be unique: three.js binds tracks by name");
    }

    #[test]
    fn writing_is_deterministic() {
        for (name, f) in fixtures::all() {
            assert_eq!(write_prepared(&f), write_prepared(&f), "{name}");
        }
    }

    #[test]
    fn names_are_made_unique_the_way_three_js_compares_them() {
        // "a.b" and "ab" sanitise to the same three.js node name.
        let mut s = fixtures::cube();
        let n = s.nodes[0].clone();
        s.nodes[0].name = Some("a.b".into());
        s.add_root(Node { name: Some("ab".into()), ..n.clone() });
        s.add_root(Node { name: None, ..n });
        let back = read_back(&write_prepared(&s)).scene;
        let mut names: Vec<String> = back.nodes.iter().filter_map(|n| n.name.clone()).collect();
        let sanitized: Vec<String> = names.iter().map(|n| n.replace(['.', ':', '/', '[', ']'], "")).collect();
        let mut dedup = sanitized.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(dedup.len(), sanitized.len(), "{names:?}");
        names.sort();
        assert!(names.iter().all(|n| !n.is_empty()));
    }

    fn quad_scene() -> Scene {
        let mut s = Scene::default();
        s.materials.push(Material { name: Some("m".into()), base_color: [0.5, 0.25, 0.125, 1.0], metallic: 0.0, roughness: 0.5, ..Default::default() });
        s.meshes.push(SceneMesh {
            name: Some("quad".into()),
            primitives: vec![Primitive {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
                normals: Some(vec![[0.0, 0.0, 1.0]; 4]),
                uvs: vec![vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]],
                indices: vec![0, 1, 2, 0, 2, 3],
                material: Some(0),
                ..Default::default()
            }],
            ..Default::default()
        });
        s.add_root(Node { name: Some("quad".into()), mesh: Some(0), ..Default::default() });
        s
    }

    #[test]
    fn colours_are_written_srgb_and_read_back_linear() {
        let bytes = write_prepared(&quad_scene());
        let dom = parse(&bytes).unwrap();
        use super::super::fbx_dom::ROOT;
        let objects = dom.child(ROOT, "Objects").unwrap();
        let mat = dom.child(objects, "Material").unwrap();
        let p70 = dom.child(mat, "Properties70").unwrap();
        let diffuse = dom
            .children_named(p70, "P")
            .map(|p| dom.props(p).unwrap())
            .find(|p| p[0].as_str().as_deref() == Some("DiffuseColor"))
            .unwrap();
        // linear 0.5 is sRGB 0.7354.
        assert!((diffuse[4].as_f64().unwrap() - 0.735_356_983_052).abs() < 1e-6);
        let back = read_back(&bytes).scene;
        assert!((back.materials[0].base_color[0] - 0.5).abs() < 1e-6);
    }

    /// 2026-10-12 review #30: normals, tangents and binormals were written per
    /// corner as doubles, 72 bytes a corner each, and our own reader's decode
    /// budget refused our own textured FBX above ~1.92M triangles. Each
    /// distinct value is now stored once and indexed per corner, as UVs and
    /// colours already were, and reads back exactly.
    #[test]
    fn normals_tangents_and_binormals_store_each_value_once() {
        let mut s = quad_scene();
        let flat = &mut s.meshes[0].primitives[0];
        flat.tangents = Some(vec![[1.0, 0.0, 0.0, -1.0]; 4]);
        let corners = flat.indices.len();
        let bytes = write_prepared(&s);
        let dom = parse(&bytes).unwrap();
        use super::super::fbx_dom::ROOT;
        let objects = dom.child(ROOT, "Objects").unwrap();
        let geo = dom.child(objects, "Geometry").unwrap();
        for (element, values, index) in [
            ("LayerElementNormal", "Normals", "NormalsIndex"),
            ("LayerElementTangent", "Tangents", "TangentsIndex"),
            ("LayerElementBinormal", "Binormals", "BinormalsIndex"),
        ] {
            let el = dom.child(geo, element).unwrap_or_else(|| panic!("{element}"));
            let reference = dom.child_prop(el, "ReferenceInformationType").unwrap().unwrap();
            assert_eq!(reference.as_str().as_deref(), Some("IndexToDirect"), "{element}");
            let v = dom.f64s(&dom.child_array(el, values).unwrap().unwrap(), values).unwrap();
            let i = dom.i32s(&dom.child_array(el, index).unwrap().unwrap(), index).unwrap();
            assert_eq!(i.len(), corners, "{index}: one per corner");
            assert_eq!(v.len(), 3, "{values}: the flat quad's one value, once");
        }
        let back = read_back(&bytes).scene;
        let p = &back.meshes[0].primitives[0];
        assert!(p.normals.as_ref().unwrap().iter().all(|n| *n == [0.0, 0.0, 1.0]), "{:?}", p.normals);
        assert!(p.tangents.as_ref().unwrap().iter().all(|t| *t == [1.0, 0.0, 0.0, -1.0]), "{:?}", p.tangents);
    }

    /// 2026-10-12 review #22: `SpecularColor` was white for every material,
    /// with `Shininess` 0 when fully rough, and three.js's FBXLoader (which
    /// reads the colour and not `SpecularFactor`) drew a uniform white
    /// specular wash. The colour is glTF's F0 faded by roughness, factor 1.
    #[test]
    fn the_phong_specular_follows_the_material_so_rough_surfaces_do_not_wash() {
        let specular = |metallic: f32, roughness: f32| -> (Vec<f64>, f64) {
            let mut s = quad_scene();
            s.materials[0].metallic = metallic;
            s.materials[0].roughness = roughness;
            let bytes = write_prepared(&s);
            let dom = parse(&bytes).unwrap();
            use super::super::fbx_dom::ROOT;
            let objects = dom.child(ROOT, "Objects").unwrap();
            let p70 = dom.child(dom.child(objects, "Material").unwrap(), "Properties70").unwrap();
            let prop = |name: &str| dom.children_named(p70, "P").map(|p| dom.props(p).unwrap()).find(|p| p[0].as_str().as_deref() == Some(name)).unwrap();
            let c = prop("SpecularColor");
            ((4..7).map(|i| c[i].as_f64().unwrap()).collect(), prop("SpecularFactor")[4].as_f64().unwrap())
        };
        let (rough, factor) = specular(0.0, 1.0);
        assert_eq!(rough, vec![0.0; 3], "no highlight on a fully rough surface");
        assert_eq!(factor, 1.0, "the colour carries it all");
        let (glossy, _) = specular(0.0, 0.0);
        assert!(glossy.iter().all(|c| (srgb_decode(*c) as f64 - 0.04).abs() < 1e-3), "a dielectric's F0: {glossy:?}");
        // quad_scene's base colour, as a metal's F0.
        let (metal, _) = specular(1.0, 0.0);
        assert!((srgb_decode(metal[0]) as f64 - 0.5).abs() < 1e-3 && (srgb_decode(metal[1]) as f64 - 0.25).abs() < 1e-3, "{metal:?}");
    }

    #[test]
    fn uv_v_is_flipped_in_the_file() {
        let bytes = write_prepared(&quad_scene());
        let dom = parse(&bytes).unwrap();
        use super::super::fbx_dom::ROOT;
        let objects = dom.child(ROOT, "Objects").unwrap();
        let geo = dom.child(objects, "Geometry").unwrap();
        let uv = dom.child(geo, "LayerElementUV").unwrap();
        let arr = dom.child_array(uv, "UV").unwrap().unwrap();
        let v = dom.f64s(&arr, "UV").unwrap();
        // The IR's (0, 1) — bottom-left of the image — is FBX's (0, 0).
        assert!(v.chunks(2).any(|c| c == [0.0, 0.0]));
        let back = read_back(&bytes).scene;
        let p = &back.meshes[0].primitives[0];
        for (pos, uv) in p.positions.iter().zip(&p.uvs[0]) {
            assert_eq!(uv[1], 1.0 - pos[1], "{pos:?} {uv:?}");
        }
    }

    #[test]
    fn a_step_channel_survives_and_three_js_sees_an_emulated_step() {
        let s = fixtures::skinned();
        let bytes = write_prepared(&s);
        let back = read_back(&bytes).scene;
        let hip_t = back.animations[0]
            .channels
            .iter()
            .find(|c| c.path == AnimPath::Translation && back.nodes[c.node].name.as_deref() == Some("hip"))
            .expect("hip translation");
        assert_eq!(hip_t.interpolation, Interpolation::Step);
        assert_eq!(hip_t.times, vec![0.0, 1.0], "redundant step keys are dropped on read");
        assert!((hip_t.values[3] - 0.4).abs() < 1e-6);
        // In the file, the key just before the step holds the old value, so a
        // reader that ignores the constant flag (three.js) still steps.
        let dom = parse(&bytes).unwrap();
        use super::super::fbx_dom::ROOT;
        let objects = dom.child(ROOT, "Objects").unwrap();
        let mut found = false;
        for c in dom.children_named(objects, "AnimationCurve") {
            let flags = dom.i32s(&dom.child_array(c, "KeyAttrFlags").unwrap().unwrap(), "f").unwrap();
            if flags[0] & 0x2 != 0 {
                let t = dom.i64s(&dom.child_array(c, "KeyTime").unwrap().unwrap(), "t").unwrap();
                let v = dom.f32s(&dom.child_array(c, "KeyValueFloat").unwrap().unwrap(), "v").unwrap();
                let one = TICKS_PER_SECOND as i64;
                let k = t.iter().position(|x| *x == one).unwrap();
                assert!(t[k - 1] > one - one / 100, "the emulation key sits just before the step");
                assert_eq!(v[k - 1], v[0]);
                found = true;
            }
        }
        assert!(found, "no constant-interpolated curve was written");
    }

    #[test]
    fn rotations_are_written_as_unwrapped_euler_with_shared_key_times() {
        // A full turn about Y in four steps: Euler must not jump by 360.
        let mut s = quad_scene();
        let q = |deg: f64| math::quat_from_axis_angle([0.0, 1.0, 0.0], deg.to_radians());
        let mut values = Vec::new();
        for d in [0.0, 120.0, 240.0, 359.0] {
            values.extend(q(d).iter().map(|v| *v as f32));
        }
        s.animations.push(Animation {
            name: Some("Spin".into()),
            channels: vec![Channel { node: 0, path: AnimPath::Rotation, interpolation: Interpolation::Linear, times: vec![0.0, 1.0, 2.0, 3.0], values }],
        });
        let bytes = write_prepared(&s);
        let dom = parse(&bytes).unwrap();
        use super::super::fbx_dom::ROOT;
        let objects = dom.child(ROOT, "Objects").unwrap();
        let curves: Vec<_> = dom.children_named(objects, "AnimationCurve").collect();
        assert_eq!(curves.len(), 3);
        let times: Vec<Vec<i64>> = curves.iter().map(|c| dom.i64s(&dom.child_array(*c, "KeyTime").unwrap().unwrap(), "t").unwrap()).collect();
        assert!(times.windows(2).all(|w| w[0] == w[1]), "X/Y/Z must share key times");
        for c in &curves {
            let v = dom.f32s(&dom.child_array(*c, "KeyValueFloat").unwrap().unwrap(), "v").unwrap();
            assert!(v.windows(2).all(|w| (w[1] - w[0]).abs() < 90.0), "jump in {v:?}");
        }
        // And the read-back rotation matches at off-key times.
        let back = read_back(&bytes).scene;
        for t in [0.5f32, 1.3, 2.7] {
            let a = pose::sample_pose(&s, 0, t).trs[0].unwrap().1;
            let b = pose::sample_pose(&back, 0, t).trs[0].unwrap().1;
            assert!(math::quat_dot(a, b).abs() > 1.0 - 1e-5, "at {t}: {a:?} vs {b:?}");
        }
    }

    #[test]
    fn a_rotation_through_gimbal_lock_reads_back_on_its_arc() {
        // XYZ Euler angles are singular at Y = ±90°: near it the X and Z
        // angles swing fast, and interpolating each on its own (what FBX
        // readers do between keys) leaves the arc the scene's quaternions
        // follow by degrees within one 30 fps step. These are four keys of
        // CesiumMan's right leg (Khronos sample, 24 fps), whose written curve
        // read back 2.5° off at t = 1.178 before the writer refined it;
        // three.js, which slerps between keys, never showed it.
        let mut s = quad_scene();
        let keys: [[f32; 4]; 4] = [
            [0.0291048, -0.81100506, 0.018963795, -0.584007],
            [0.029462837, -0.7438936, 0.016568525, -0.6674428],
            [0.025692973, -0.67182314, 0.013065219, -0.74015063],
            [0.02188712, -0.6047138, 0.008674434, -0.7960949],
        ];
        s.animations.push(Animation {
            name: Some("Stride".into()),
            channels: vec![Channel {
                node: 0,
                path: AnimPath::Rotation,
                interpolation: Interpolation::Linear,
                times: (0..4).map(|k| k as f32 / 24.0).collect(),
                values: keys.iter().flatten().copied().collect(),
            }],
        });
        let back = read_back(&write_prepared(&s)).scene;
        let mut worst = (0.0f64, 0.0f32);
        for k in 0..=1000 {
            let t = 0.125 * k as f32 / 1000.0;
            let a = pose::sample_pose(&s, 0, t).trs[0].unwrap().1;
            let b = pose::sample_pose(&back, 0, t).trs[0].unwrap().1;
            let deg = 2.0 * math::quat_dot(a, b).abs().min(1.0).acos().to_degrees();
            if deg > worst.0 {
                worst = (deg, t);
            }
        }
        // The writer's tolerance (0.05°), plus the reader's own resampling
        // of the Euler curves it reads.
        assert!(worst.0 < 0.1, "{:.3}° off the arc at t = {}", worst.0, worst.1);
    }

    #[test]
    fn skin_inverse_binds_survive_exactly() {
        let s = fixtures::skinned();
        let back = read_back(&write_prepared(&s)).scene;
        assert_eq!(back.skins.len(), 1);
        let (a, b) = (&s.skins[0], &back.skins[0]);
        assert_eq!(a.joints.len(), b.joints.len());
        for (x, y) in a.inverse_bind_matrices.iter().zip(&b.inverse_bind_matrices) {
            for k in 0..16 {
                assert!((x[k] - y[k]).abs() < 1e-9, "{x:?} vs {y:?}");
            }
        }
        for (ja, jb) in a.joints.iter().zip(&b.joints) {
            assert_eq!(s.nodes[*ja].name, back.nodes[*jb].name);
        }
    }

    #[test]
    fn morph_targets_keep_names_defaults_and_normals() {
        let s = fixtures::morph();
        let back = read_back(&write_prepared(&s)).scene;
        let m = &back.meshes[0];
        assert_eq!(m.target_names, vec!["bulge".to_string(), "twist".to_string()]);
        assert!((m.weights[0] - 0.25).abs() < 1e-6 && m.weights[1] == 0.0, "{:?}", m.weights);
        // Every vertex's position and normal deltas come back, matched by the
        // vertex they move (the writer welds and reorders vertices).
        let prepared = prepare(&s, WriteOptions::default()).unwrap();
        let q = |v: [f32; 3]| v.map(|x| (x as f64 * 1e5).round() as i64);
        let deltas = |p: &Primitive| -> Vec<([i64; 3], [i64; 3], Vec<[i64; 3]>)> {
            let mut out: Vec<_> = (0..p.positions.len())
                .map(|i| {
                    let n = p.normals.as_ref().map_or([0.0; 3], |n| n[i]);
                    let mut d = Vec::new();
                    for t in &p.targets {
                        d.push(q(t.positions[i]));
                        d.push(q(t.normals.as_ref().map_or([0.0; 3], |n| n[i])));
                    }
                    (q(p.positions[i]), q(n), d)
                })
                .collect();
            out.sort();
            out.dedup();
            out
        };
        assert_eq!(deltas(&m.primitives[0]), deltas(&prepared.meshes[0].primitives[0]));
        assert!(m.primitives[0].targets[0].normals.as_ref().is_some_and(|n| n.iter().any(|d| d[0] != 0.0)), "bulge moves normals");
        let ch = &back.animations[0].channels[0];
        assert_eq!(ch.path, AnimPath::Weights);
    }

    #[test]
    fn a_geometric_transform_is_baked_into_its_own_geometry_only() {
        // Hand-written: a parent at x=1 whose geometry is offset by +10 in y by
        // GeometricTranslation, and a child at x=2 under it. The child must
        // NOT inherit the +10.
        let src = r#"; FBX 7.4.0 project file
FBXHeaderExtension:  {
	FBXHeaderVersion: 1003
	FBXVersion: 7400
}
GlobalSettings:  {
	Properties70:  {
		P: "UnitScaleFactor", "double", "Number", "", 100
	}
}
Objects:  {
	Model: 10, "Model::parent", "Mesh" {
		Properties70:  {
			P: "Lcl Translation", "Lcl Translation", "", "A",1,0,0
			P: "GeometricTranslation", "Vector3D", "Vector", "",0,10,0
		}
	}
	Model: 11, "Model::child", "Mesh" {
		Properties70:  {
			P: "Lcl Translation", "Lcl Translation", "", "A",2,0,0
		}
	}
	Geometry: 20, "Geometry::tri", "Mesh" {
		Vertices: *9 {
			a: 0,0,0,1,0,0,0,1,0
		}
		PolygonVertexIndex: *3 {
			a: 0,1,-3
		}
	}
}
Connections:  {
	C: "OO",10,0
	C: "OO",11,10
	C: "OO",20,10
	C: "OO",20,11
}
"#;
        let r = super::super::read_scene(src.as_bytes(), MeshFormat::Fbx, &NoSidecars).unwrap();
        let m = pose::flatten(&r.scene).unwrap();
        let ys: Vec<f32> = m.positions.iter().map(|p| p[1]).collect();
        let xs: Vec<f32> = m.positions.iter().map(|p| p[0]).collect();
        // Parent's copy: x in [1, 2], y in [10, 11]. Child's: x in [3, 4], y in [0, 1].
        assert!(m.positions.iter().any(|p| p[0] == 1.0 && p[1] == 10.0), "{xs:?} {ys:?}");
        assert!(m.positions.iter().any(|p| p[0] == 3.0 && p[1] == 0.0), "{xs:?} {ys:?}");
        assert!(!m.positions.iter().any(|p| p[0] >= 3.0 && p[1] >= 10.0), "the child inherited the geometric offset");
    }

    #[test]
    fn z_up_centimetre_files_get_one_root_correction() {
        let src = r#"; FBX 7.4.0 project file
FBXHeaderExtension:  {
	FBXVersion: 7400
}
GlobalSettings:  {
	Properties70:  {
		P: "UpAxis", "int", "Integer", "",2
		P: "UpAxisSign", "int", "Integer", "",1
		P: "FrontAxis", "int", "Integer", "",1
		P: "FrontAxisSign", "int", "Integer", "",-1
		P: "CoordAxis", "int", "Integer", "",0
		P: "CoordAxisSign", "int", "Integer", "",1
		P: "UnitScaleFactor", "double", "Number", "",1
	}
}
Objects:  {
	Model: 10, "Model::m", "Mesh" {
	}
	Geometry: 20, "Geometry::tri", "Mesh" {
		Vertices: *9 {
			a: 0,0,0,100,0,0,0,0,100
		}
		PolygonVertexIndex: *3 {
			a: 0,1,-3
		}
	}
}
Connections:  {
	C: "OO",10,0
	C: "OO",20,10
}
"#;
        let r = super::super::read_scene(src.as_bytes(), MeshFormat::Fbx, &NoSidecars).unwrap();
        assert_eq!(r.scene.roots.len(), 1, "one correction root");
        let m = pose::flatten(&r.scene).unwrap();
        // 100 cm up (file +Z) is 1 m up (+Y).
        assert!(m.positions.iter().any(|p| (p[1] - 1.0).abs() < 1e-6 && p[0].abs() < 1e-6 && p[2].abs() < 1e-6), "{:?}", m.positions);
        assert!(m.positions.iter().any(|p| (p[0] - 1.0).abs() < 1e-6), "{:?}", m.positions);
    }

    #[test]
    fn ngons_are_fanned_with_their_winding() {
        let src = r#"; FBX 7.4.0 project file
FBXHeaderExtension:  {
	FBXVersion: 7400
}
GlobalSettings:  {
	Properties70:  {
		P: "UnitScaleFactor", "double", "Number", "",100
	}
}
Objects:  {
	Model: 10, "Model::m", "Mesh" {
	}
	Geometry: 20, "Geometry::pent", "Mesh" {
		Vertices: *15 {
			a: 0,0,0,2,0,0,3,1,0,1,2,0,-1,1,0
		}
		PolygonVertexIndex: *5 {
			a: 0,1,2,3,-5
		}
	}
}
Connections:  {
	C: "OO",10,0
	C: "OO",20,10
}
"#;
        let r = super::super::read_scene(src.as_bytes(), MeshFormat::Fbx, &NoSidecars).unwrap();
        let m = pose::flatten(&r.scene).unwrap();
        assert_eq!(m.triangle_count(), 3);
        for t in m.indices.chunks(3) {
            let tri = [m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]];
            assert!(super::super::Mesh::facet_normal(&tri)[2] > 0.0, "fan reversed the winding");
        }
    }

    #[test]
    fn per_polygon_materials_split_into_primitives() {
        let mut s = quad_scene();
        s.materials.push(Material { name: Some("other".into()), base_color: [0.1, 0.9, 0.1, 1.0], ..Default::default() });
        let mut p2 = s.meshes[0].primitives[0].clone();
        for p in p2.positions.iter_mut() {
            p[2] = 1.0;
        }
        p2.material = Some(1);
        s.meshes[0].primitives.push(p2);
        let back = read_back(&write_prepared(&s)).scene;
        assert_eq!(back.meshes[0].primitives.len(), 2);
        let mats: Vec<Option<String>> = back.meshes[0].primitives.iter().map(|p| p.material.and_then(|m| back.materials[m].name.clone())).collect();
        assert_eq!(mats, vec![Some("m".to_string()), Some("other".to_string())]);
    }

    #[test]
    fn a_skinned_mesh_away_from_the_origin_is_hoisted_and_binds_correctly() {
        let mut s = fixtures::skinned();
        // Put the mesh node at x=5: glTF ignores it, so the posed shape must
        // not move.
        s.nodes[3].transform = Transform::Trs { translation: [5.0, 0.0, 0.0], rotation: math::QUAT_IDENTITY, scale: [1.0; 3] };
        let back = read_back(&write_prepared(&s)).scene;
        assert_same_geometry(&s, &pose::default_pose(&s), &back, &pose::default_pose(&back), 1e-4, "hoisted");
    }

    #[test]
    fn influences_are_padded_to_four_and_extra_ones_survive() {
        let mut s = fixtures::skinned();
        // Six influences on vertex 0 across three joints (duplicates allowed).
        let p = &mut s.meshes[0].primitives[0];
        let n = p.positions.len();
        let mut sw = SkinWeights { influences: 8, joints: vec![0; n * 8], weights: vec![0.0; n * 8] };
        for v in 0..n {
            let (j, w) = p.skin.as_ref().unwrap().vertex(v);
            for k in 0..4 {
                sw.joints[v * 8 + k] = j[k];
                sw.weights[v * 8 + k] = w[k];
            }
        }
        sw.joints[4] = 2;
        sw.weights[4] = 0.25;
        p.skin = Some(sw);
        let back = read_back(&write_prepared(&s)).scene;
        let bp = &back.meshes[0].primitives[0];
        let sk = bp.skin.as_ref().unwrap();
        assert_eq!(sk.influences % 4, 0);
        assert!(sk.influences >= 4);
        assert_same_geometry(&s, &pose::default_pose(&s), &back, &pose::default_pose(&back), 1e-4, "extra influence");
    }

    #[test]
    fn a_mesh_without_material_gets_gltfs_default_rather_than_three_js_grey() {
        let mut s = quad_scene();
        s.meshes[0].primitives[0].material = None;
        s.materials.clear();
        let back = read_back(&write_prepared(&s)).scene;
        let m = back.meshes[0].primitives[0].material.map(|m| &back.materials[m]).expect("a material");
        assert_eq!(m.base_color, [1.0; 4]);
    }

    #[test]
    fn textures_embed_and_also_resolve_from_sidecars() {
        let s = fixtures::textured();
        let bytes = write_prepared(&s);
        let back = read_back(&bytes).scene;
        assert_eq!(back.images.len(), 1);
        assert_eq!(back.images[0].data, s.images[0].data);
        // The same file with the Content stripped must find the image by its
        // RelativeFilename among the sidecars.
        let dom = parse(&bytes).unwrap();
        use super::super::fbx_dom::ROOT;
        let objects = dom.child(ROOT, "Objects").unwrap();
        let video = dom.child(objects, "Video").unwrap();
        let rel = dom.child_prop(video, "RelativeFilename").unwrap().unwrap();
        let rel = rel.as_str().unwrap().into_owned();
        let stripped = super::super::fbx_write::tests_support::strip_video_content(&s).unwrap();
        let files = vec![PackageFile { path: rel.clone(), bytes: s.images[0].data.clone() }];
        let r = super::super::read_scene(&stripped, MeshFormat::Fbx, &MemorySidecars { files: &files }).unwrap();
        assert_eq!(r.scene.images.len(), 1, "{rel}");
        let r = super::super::read_scene(&stripped, MeshFormat::Fbx, &NoSidecars).unwrap();
        assert!(r.scene.images.is_empty());
        assert!(r.warnings.iter().any(|w| w.contains(&rel)), "{:?}", r.warnings);
    }

    /// A classic Phong material, as Blender's exporter writes one: the
    /// colour beside a diffuse map is its socket's default, 0.8.
    fn phong_doc(diffuse_factor: f64, textured: bool) -> String {
        let texture = if textured {
            "\tTexture: 30, \"Texture::t\", \"\" {\n\t\tType: \"TextureVideoClip\"\n\t\tFileName: \"t.png\"\n\t\tRelativeFilename: \"t.png\"\n\t}\n"
        } else {
            ""
        };
        let link = if textured { "\tC: \"OP\",30,20, \"DiffuseColor\"\n" } else { "" };
        format!(
            "; FBX 7.4.0 project file\nFBXHeaderExtension:  {{\n\tFBXVersion: 7400\n\tCreator: \"Blender (stable FBX IO) - 4.4.0\"\n}}\nObjects:  {{\n\
             \tModel: 10, \"Model::m\", \"Mesh\" {{\n\t}}\n\
             \tGeometry: 99, \"Geometry::tri\", \"Mesh\" {{\n\t\tVertices: *9 {{\n\t\t\ta: 0,0,0,1,0,0,0,1,0\n\t\t}}\n\t\tPolygonVertexIndex: *3 {{\n\t\t\ta: 0,1,-3\n\t\t}}\n\
             \t\tLayerElementUV: 0 {{\n\t\t\tName: \"uv\"\n\t\t\tMappingInformationType: \"ByPolygonVertex\"\n\t\t\tReferenceInformationType: \"Direct\"\n\t\t\tUV: *6 {{\n\t\t\t\ta: 0,0,1,0,0,1\n\t\t\t}}\n\t\t}}\n\
             \t\tLayerElementMaterial: 0 {{\n\t\t\tMappingInformationType: \"AllSame\"\n\t\t\tReferenceInformationType: \"IndexToDirect\"\n\t\t\tMaterials: *1 {{\n\t\t\t\ta: 0\n\t\t\t}}\n\t\t}}\n\
             \t\tLayer: 0 {{\n\t\t\tLayerElement:  {{\n\t\t\t\tType: \"LayerElementUV\"\n\t\t\t\tTypedIndex: 0\n\t\t\t}}\n\t\t\tLayerElement:  {{\n\t\t\t\tType: \"LayerElementMaterial\"\n\t\t\t\tTypedIndex: 0\n\t\t\t}}\n\t\t}}\n\t}}\n\
             \tMaterial: 20, \"Material::m\", \"\" {{\n\t\tShadingModel: \"phong\"\n\t\tProperties70:  {{\n\
             \t\t\tP: \"DiffuseColor\", \"Color\", \"\", \"A\",0.8,0.8,0.8\n\t\t\tP: \"DiffuseFactor\", \"Number\", \"\", \"A\",{diffuse_factor}\n\t\t}}\n\t}}\n\
             {texture}}}\nConnections:  {{\n\tC: \"OO\",10,0\n\tC: \"OO\",99,10\n\tC: \"OO\",20,10\n{link}}}\n"
        )
    }

    /// 2026-10-12 review, finding 25.4: a non-finite number — MSVC prints NaN
    /// as `1.#IND00` and infinity as `1.#INF00` — in one layer element or one
    /// vertex no longer refuses the file: the element reads as zeros, a
    /// polygon on the vertex is skipped, and each says so.
    #[test]
    fn non_finite_numbers_are_replaced_with_a_warning_not_refused() {
        let cube = super::super::fbx_write::tests_support::ascii_cube();
        let src = cube.replace("a: 0,0,1,0,0,1,0,0,1", "a: 0,0,1,0,0,1,0,0,1.#IND00");
        let r = super::super::read_scene(src.as_bytes(), MeshFormat::Fbx, &NoSidecars).unwrap_or_else(|e| panic!("{e}"));
        assert!(r.warnings.iter().any(|w| w.contains("non-finite")), "{:?}", r.warnings);
        let n = r.scene.meshes[0].primitives[0].normals.as_ref().unwrap();
        assert!(n.iter().flatten().all(|v| v.is_finite()), "{n:?}");
        let src = cube
            .replace("Vertices: *9 {\n\t\t\ta: 0,0,0,1,0,0,0,1,0", "Vertices: *12 {\n\t\t\ta: 0,0,0,1,0,0,0,1,0,-1.#INF00,0,0")
            .replace("PolygonVertexIndex: *3 {\n\t\t\ta: 0,1,-3", "PolygonVertexIndex: *6 {\n\t\t\ta: 0,1,-3,0,2,-4")
            .replace("Normals: *9 {\n\t\t\t\ta: 0,0,1,0,0,1,0,0,1", "Normals: *18 {\n\t\t\t\ta: 0,0,1,0,0,1,0,0,1,0,0,1,0,0,1,0,0,1")
            .replace("UVIndex: *3 {\n\t\t\t\ta: 0,1,2", "UVIndex: *6 {\n\t\t\t\ta: 0,1,2,0,2,1");
        assert!(src.contains("*12") && src.contains("*18") && src.contains("UVIndex: *6"), "the fixture changed");
        let r = super::super::read_scene(src.as_bytes(), MeshFormat::Fbx, &NoSidecars).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.scene.triangle_count(), 1);
        assert!(r.warnings.iter().any(|w| w.contains("non-finite")), "{:?}", r.warnings);
    }

    /// Final review, finding 7 (2026-11-01): a non-finite colour element reads
    /// as opaque white, a colour's neutral, not as (0, 0, 0, 0), which made the
    /// corner transparent and every OBJ/Collada write report vertex alpha.
    #[test]
    fn a_non_finite_colour_reads_as_opaque_white() {
        let colours = "\t\tLayerElementColor: 0 {\n\t\t\tVersion: 101\n\t\t\tName: \"col\"\n\t\t\tMappingInformationType: \"ByPolygonVertex\"\n\t\t\tReferenceInformationType: \"Direct\"\n\t\t\tColors: *12 {\n\t\t\t\ta: 1,0,0,1,1,0,0,1,1,0,0,1.#IND\n\t\t\t}\n\t\t}\n";
        let src = super::super::fbx_write::tests_support::ascii_cube().replacen("\t\tLayerElementUV: 0 {", &format!("{colours}\t\tLayerElementUV: 0 {{"), 1);
        assert!(src.contains("LayerElementColor"), "the fixture changed");
        let r = super::super::read_scene(src.as_bytes(), MeshFormat::Fbx, &NoSidecars).unwrap_or_else(|e| panic!("{e}"));
        let c = r.scene.meshes[0].primitives[0].colors.clone().expect("colours");
        assert!(c.iter().all(|c| c[3] == 1.0), "{c:?}");
    }

    /// 2026-10-12 review, finding 25.5: an ASCII FBX holding a byte that is not
    /// UTF-8 (a CP1252 é in a comment or a name, from an older tool) is read,
    /// the byte as U+FFFD, with a warning, as binary FBX strings already are.
    #[test]
    fn an_ascii_fbx_in_a_legacy_code_page_is_read() {
        let mut b = super::super::fbx_write::tests_support::ascii_cube().into_bytes();
        let nl = b.iter().position(|&c| c == b'\n').unwrap();
        b.insert(nl, 0xE9);
        let r = super::super::read_scene(&b, MeshFormat::Fbx, &NoSidecars).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.scene.triangle_count(), 1);
        assert!(r.warnings.iter().any(|w| w.contains("UTF-8")), "{:?}", r.warnings);
    }

    /// The caller's choice of FBX material colour space (review finding 21,
    /// left to the client): `Linear` stores DiffuseColor and EmissiveColor as
    /// the scene's linear values (Blender, Unity URP, ufbx and assimp read them
    /// so) and reads them back so; the default stays sRGB (three.js's
    /// FBXLoader). Vertex colours are sRGB either way, Blender's default.
    #[test]
    fn material_colours_follow_the_requested_colour_space() {
        use super::super::{read_scene_with, FbxColorSpace, ReadOptions};
        // A stored classic colour, as the file holds it.
        fn stored(bytes: &[u8], name: &str) -> Vec<f64> {
            let dom = super::parse(bytes).unwrap();
            let objects = dom.child(super::super::fbx_dom::ROOT, "Objects").unwrap();
            let material = dom.children_named(objects, "Material").next().unwrap();
            let p70 = dom.child(material, "Properties70").unwrap();
            let found = dom.children_named(p70, "P").find_map(|p| {
                let props = dom.props(p).ok()?;
                (props.first()?.as_str()?.as_ref() == name).then(|| props[4..7].iter().map(|v| v.as_f64().unwrap()).collect::<Vec<f64>>())
            });
            found.unwrap()
        }
        let close = |a: &[f64], b: &[f64]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);
        let mut s = fixtures::cube();
        s.materials[0].base_color = [0.2, 0.55, 0.85, 1.0];
        s.materials[0].emissive = [0.25, 0.0, 0.0];
        let n = s.meshes[0].primitives[0].positions.len();
        s.meshes[0].primitives[0].colors = Some(vec![[0.5, 0.25, 0.75, 1.0]; n]);
        let write = |space: FbxColorSpace| {
            super::super::write_scene(&s, MeshFormat::Fbx, WriteOptions { fbx_color_space: space, ..Default::default() }).unwrap().package.files[0].bytes.clone()
        };
        // Written: linear as is; by default (unchanged) sRGB-encoded.
        let linear = write(FbxColorSpace::Linear);
        assert!(close(&stored(&linear, "DiffuseColor"), &[0.2, 0.55, 0.85]), "{:?}", stored(&linear, "DiffuseColor"));
        assert!(close(&stored(&linear, "EmissiveColor"), &[0.25, 0.0, 0.0]));
        let srgb = write(FbxColorSpace::Srgb);
        let enc = |c: f32| f64::from(math::linear_to_srgb(c));
        assert!(close(&stored(&srgb, "DiffuseColor"), &[enc(0.2), enc(0.55), enc(0.85)]));
        // Either way our own file reads back exactly (its Stingray colours are
        // linear in both), and vertex colours are sRGB in both.
        for (bytes, space) in [(&linear, FbxColorSpace::Linear), (&srgb, FbxColorSpace::Srgb)] {
            let back = read_scene_with(bytes, MeshFormat::Fbx, &NoSidecars, ReadOptions { fbx_color_space: space, ..Default::default() }).unwrap().scene;
            let m = &back.materials[0];
            assert!(close(&m.base_color[..3].iter().map(|v| f64::from(*v)).collect::<Vec<_>>(), &[0.2, 0.55, 0.85]), "{m:?}");
            let vc = back.meshes[0].primitives[0].colors.clone().unwrap();
            assert!(vc.iter().all(|c| (c[0] - 0.5).abs() < 1e-3 && (c[2] - 0.75).abs() < 1e-3), "{:?}", vc[0]);
        }
        // Read: a classic file (Blender's, Meshy's) with DiffuseColor 0.8 is
        // 0.8 linear when the caller says linear, 0.604 decoded as sRGB.
        let classic = phong_doc(1.0, false);
        let read = |space| read_scene_with(classic.as_bytes(), MeshFormat::Fbx, &NoSidecars, ReadOptions { fbx_color_space: space, ..Default::default() }).unwrap().scene.materials[0].base_color[0];
        assert!((read(FbxColorSpace::Linear) - 0.8).abs() < 1e-6, "{}", read(FbxColorSpace::Linear));
        assert!((read(FbxColorSpace::Srgb) - math::srgb_to_linear(0.8)).abs() < 1e-6);
    }

    fn phong_with(props: &str) -> super::super::scene::Material {
        let doc = phong_doc(1.0, false).replace("\t\t\tP: \"DiffuseFactor\"", &format!("{props}\t\t\tP: \"DiffuseFactor\""));
        super::super::read_scene(doc.as_bytes(), MeshFormat::Fbx, &NoSidecars).unwrap().scene.materials[0].clone()
    }

    /// 2026-10-12 review, finding 25.1: the SDK's TransparentColor default is
    /// black (the FbxSurfacePhong template: TransparentColor 0,0,0,
    /// TransparencyFactor 0; research-fbx.md), so a TransparencyFactor of 1
    /// with no colour and no Opacity is opaque. Reading white made the part
    /// invisible.
    #[test]
    fn a_transparency_factor_with_no_transparent_colour_is_opaque() {
        let m = phong_with("\t\t\tP: \"TransparencyFactor\", \"Number\", \"\", \"A\",1\n");
        assert_eq!((m.base_color[3], m.alpha_mode), (1.0, AlphaMode::Opaque), "{m:?}");
        // Stated, the SDK's product still holds: white × 0.25 is 75 % opaque.
        let m = phong_with("\t\t\tP: \"TransparencyFactor\", \"Number\", \"\", \"A\",0.25\n\t\t\tP: \"TransparentColor\", \"Color\", \"\", \"A\",1,1,1\n");
        assert!((m.base_color[3] - 0.75).abs() < 1e-6 && m.alpha_mode == AlphaMode::Blend, "{m:?}");
    }

    /// 2026-10-12 review, finding 25.2: the legacy `Emissive` is the SDK's
    /// computed EmissiveColor × EmissiveFactor (research-fbx.md, as the legacy
    /// `Diffuse` is), so the factor applies once. Applying it again gave a
    /// quarter of the brightness for a factor of 0.5.
    #[test]
    fn the_legacy_emissive_already_holds_its_factor() {
        let m = phong_with(
            "\t\t\tP: \"EmissiveColor\", \"Color\", \"\", \"A\",1,1,1\n\t\t\tP: \"EmissiveFactor\", \"Number\", \"\", \"A\",0.5\n\t\t\tP: \"Emissive\", \"Vector3D\", \"Vector\", \"\",0.5,0.5,0.5\n",
        );
        let want = math::srgb_to_linear(0.5);
        assert!(m.emissive.iter().all(|e| (e - want).abs() < 1e-4), "{:?} vs {want}", m.emissive);
        // With no legacy value the factor scales the colour.
        let m = phong_with("\t\t\tP: \"EmissiveColor\", \"Color\", \"\", \"A\",1,1,1\n\t\t\tP: \"EmissiveFactor\", \"Number\", \"\", \"A\",0.5\n");
        assert!(m.emissive.iter().all(|e| (e - 0.5).abs() < 1e-4), "{:?}", m.emissive);
    }

    #[test]
    fn a_classic_phong_map_replaces_the_colour_beside_it() {
        // Meshy's FBX says DiffuseColor 0.8 beside its texture, and Meshy's
        // GLB of the same model says baseColorFactor 1.0: the map replaces
        // the colour, as the SDK, Maya and Blender read it. Multiplying it in
        // (three.js, Unity) drew the model 40 % darker than its author did.
        let files = vec![PackageFile { path: "t.png".into(), bytes: fixtures::quadrant_png() }];
        let side = MemorySidecars { files: &files };
        let read = |src: String| super::super::read_scene(src.as_bytes(), MeshFormat::Fbx, &side).unwrap_or_else(|e| panic!("{e}")).scene;
        let s = read(phong_doc(1.0, true));
        let m = &s.materials[0];
        assert!(m.base_color_texture.is_some(), "the map was found");
        assert_eq!(m.base_color, [1.0; 4], "the map alone");
        // Only the factor still scales it (Maya writes 0.8 there).
        let s = read(phong_doc(0.8, true));
        assert!(s.materials[0].base_color[..3].iter().all(|c| (c - 0.8).abs() < 1e-6), "{:?}", s.materials[0].base_color);
        // Without a map, the colour is the colour: sRGB 0.8 is linear 0.6.
        let s = read(phong_doc(1.0, false));
        let want = super::super::math::srgb_to_linear(0.8);
        assert!(s.materials[0].base_color[..3].iter().all(|c| (c - want).abs() < 1e-5), "{:?}", s.materials[0].base_color);
    }

    #[test]
    fn a_tint_beside_a_map_is_written_white_and_reported() {
        // Readers disagree about a colour beside a map, so the writer leaves
        // only what both readings agree on, and says what that cost.
        let mut s = fixtures::textured();
        s.materials[0].base_color = [0.5, 0.25, 1.0, 1.0];
        let w = super::super::write_scene(&s, MeshFormat::Fbx, WriteOptions::default()).unwrap();
        assert!(w.losses.iter().any(|l| l.kind == LossKind::Materials && l.what.contains("tint")), "{:?}", w.losses);
        let back = read_back(&w.package.files[0].bytes).scene;
        let m = back.materials.iter().find(|m| m.base_color_texture.is_some()).unwrap();
        assert_eq!(m.base_color, [1.0; 4]);
        assert_eq!(prepare(&s, WriteOptions::default()).unwrap().materials[0].base_color, [1.0; 4]);
    }

    #[test]
    fn a_texture_transform_is_baked_into_the_uvs_it_transforms() {
        let mut s = fixtures::textured();
        let t = super::super::scene::UvTransform { offset: [0.25, 0.5], rotation: 0.3, scale: [2.0, 0.5] };
        s.materials[0].base_color_texture.as_mut().unwrap().transform = Some(t);
        let back = read_back(&write_prepared(&s)).scene;
        let src = s.meshes[0].primitives[0].uvs[0].iter().map(|uv| t.apply(*uv)).collect::<Vec<_>>();
        let got = &back.meshes.iter().find(|m| m.name.as_deref() == Some("box")).unwrap().primitives[0];
        assert!(back.materials.iter().all(|m| m.base_color_texture.as_ref().is_none_or(|r| r.transform.is_none())));
        for uv in &got.uvs[0] {
            assert!(src.iter().any(|w| (w[0] - uv[0]).abs() < 1e-5 && (w[1] - uv[1]).abs() < 1e-5), "{uv:?}");
        }
    }

    #[test]
    fn prepare_says_what_fbx_cannot_hold() {
        let mut s = fixtures::textured();
        s.materials[2].double_sided = true;
        s.materials[1].alpha_mode = AlphaMode::Mask;
        s.textures[0].sampler.wrap_s = Wrap::MirroredRepeat;
        let p = prepare(&s, WriteOptions::default()).unwrap();
        assert!(!p.materials[2].double_sided);
        assert_eq!(p.materials[1].alpha_mode, AlphaMode::Opaque);
        assert_eq!(p.textures[0].sampler.wrap_s, Wrap::Repeat);
        let l: Vec<String> = losses(&s).iter().map(|l| l.to_string()).collect();
        assert!(l.iter().any(|x| x.contains("double-sided")), "{l:?}");
        assert!(l.iter().any(|x| x.contains("alpha cutoff")), "{l:?}");
        assert!(l.iter().any(|x| x.contains("mirrored")), "{l:?}");
        let kinds: Vec<LossKind> = losses(&s).iter().map(|l| l.kind).collect();
        assert_eq!(kinds, [LossKind::Materials, LossKind::Alpha, LossKind::TextureSampling]);
        // And write_scene reports them.
        let w = super::super::write_scene(&s, MeshFormat::Fbx, WriteOptions::default()).unwrap();
        assert!(w.losses.iter().any(|x| x.to_string().contains("double-sided")));
    }

    #[test]
    fn fbx_puts_an_emissive_or_normal_set_first() {
        // three.js r183's FBXLoader samples every texture from the first set
        // (`prepare`'s doc says why), and FBX keeps normal and emissive maps
        // as well as base colour (`TEXTURED_SLOTS`). So an emissive map alone
        // on set 1 of two, with nothing on set 0, is written on set 0 from set
        // 1's coordinates, and so is a normal map alone there; the other set
        // follows at index 1.
        let mut quad = fixtures::multiuv();
        quad.meshes[0].primitives[0].uvs.truncate(2);
        quad.materials[0].base_color_texture = None;
        quad.materials[0].occlusion_texture = None;
        // Each distinct vertex as its position and its uv sets, in position
        // order: the reader welds the corners of its polygons.
        let corners = |p: &Primitive| {
            let mut out: Vec<([f32; 3], Vec<[f32; 2]>)> =
                (0..p.positions.len()).map(|v| (p.positions[v], p.uvs.iter().map(|set| set[v]).collect())).collect();
            out.sort_by_key(|(at, _)| at.map(f32::to_bits));
            out.dedup();
            out
        };
        let source = quad.meshes[0].primitives[0].clone();
        let swapped = Primitive { uvs: vec![source.uvs[1].clone(), source.uvs[0].clone()], ..source };
        let on_set_one = Some(super::super::scene::TextureRef { texture: 0, uv_set: 1, transform: None });
        let mut emissive = quad.clone();
        emissive.materials[0].emissive_texture = on_set_one.clone();
        emissive.materials[0].emissive = [1.0; 3];
        let mut normal = quad;
        normal.materials[0].normal_texture = on_set_one;
        let got = [("emissive", emissive), ("normal", normal)].map(|(slot, s)| {
            let back = read_back(&write_prepared(&s)).scene;
            let p = &back.meshes[0].primitives[0];
            let m = p.material.map(|m| &back.materials[m]);
            let map = m.and_then(|m| if slot == "emissive" { m.emissive_texture.as_ref() } else { m.normal_texture.as_ref() });
            (slot, map.map(|t| t.uv_set), corners(p))
        });
        assert_eq!(got, [("emissive", Some(0), corners(&swapped)), ("normal", Some(0), corners(&swapped))]);
    }

    // ── the FBX transform stack ─────────────────────────────────────────────

    /// A document with one triangle under the given models. `models` is
    /// `(id, parent id, Properties70 lines)`; the triangle hangs off the last.
    fn transform_doc(models: &[(i64, i64, &str)], curves: &str) -> String {
        let mut src = String::from(
            "; FBX 7.4.0 project file\nFBXHeaderExtension:  {\n\tFBXVersion: 7400\n}\nGlobalSettings:  {\n\tProperties70:  {\n\t\tP: \"UnitScaleFactor\", \"double\", \"Number\", \"\",100\n\t}\n}\nObjects:  {\n",
        );
        for (id, _, props) in models {
            src.push_str(&format!("\tModel: {id}, \"Model::m{id}\", \"Mesh\" {{\n\t\tProperties70:  {{\n{props}\n\t\t}}\n\t}}\n"));
        }
        src.push_str("\tGeometry: 99, \"Geometry::tri\", \"Mesh\" {\n\t\tVertices: *9 {\n\t\t\ta: 0,0,0,1,0,0,0,1,0\n\t\t}\n\t\tPolygonVertexIndex: *3 {\n\t\t\ta: 0,1,-3\n\t\t}\n\t}\n");
        src.push_str(curves);
        src.push_str("}\nConnections:  {\n");
        for (id, parent, _) in models {
            src.push_str(&format!("\tC: \"OO\",{id},{parent}\n"));
        }
        let last = models.last().map_or(0, |m| m.0);
        src.push_str(&format!("\tC: \"OO\",99,{last}\n}}\n"));
        src
    }

    fn world_triangle(src: &str, t: Option<f32>) -> Vec<[f64; 3]> {
        let r = super::super::read_scene(src.as_bytes(), MeshFormat::Fbx, &NoSidecars).unwrap_or_else(|e| panic!("{e}"));
        let pose = match t {
            Some(t) => pose::sample_pose(&r.scene, 0, t),
            None => pose::default_pose(&r.scene),
        };
        let wp = pose::world_primitives(&r.scene, &pose).unwrap();
        wp[0].positions.iter().map(|p| [p[0] as f64, p[1] as f64, p[2] as f64]).collect()
    }

    fn deg(v: [f64; 3]) -> [f64; 3] {
        [v[0].to_radians(), v[1].to_radians(), v[2].to_radians()]
    }

    fn assert_points(got: &[[f64; 3]], m: &math::Mat4, ctx: &str) {
        for (i, p) in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].iter().enumerate() {
            let want = math::transform_point_f64(m, *p);
            for k in 0..3 {
                assert!((got[i][k] - want[k]).abs() < 1e-5, "{ctx}: corner {i} is {:?}, want {want:?}", got[i]);
            }
        }
    }

    #[test]
    fn pre_rotation_rotation_order_and_post_rotation_compose_as_the_sdk_defines() {
        let props = "\t\t\tP: \"RotationActive\", \"bool\", \"\", \"\",1\n\t\t\tP: \"RotationOrder\", \"enum\", \"\", \"\",4\n\t\t\tP: \"PreRotation\", \"Vector3D\", \"Vector\", \"\",90,0,0\n\t\t\tP: \"PostRotation\", \"Vector3D\", \"Vector\", \"\",0,0,30\n\t\t\tP: \"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\",10,20,30\n\t\t\tP: \"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",1,2,3";
        let got = world_triangle(&transform_doc(&[(10, 0, props)], ""), None);
        // T · Rpre(XYZ) · R(ZXY order: Z first, then X, then Y) · Rpost⁻¹.
        let pre = math::quat_to_mat(math::euler_to_quat(deg([90.0, 0.0, 0.0]), math::EulerOrder::Xyz));
        let r = math::quat_to_mat(math::euler_to_quat(deg([10.0, 20.0, 30.0]), math::EulerOrder::Zxy));
        let post = math::invert(&math::quat_to_mat(math::euler_to_quat(deg([0.0, 0.0, 30.0]), math::EulerOrder::Xyz))).unwrap();
        let m = math::mat_mul(&math::translation([1.0, 2.0, 3.0]), &math::mat_mul(&pre, &math::mat_mul(&r, &post)));
        assert_points(&got, &m, "pre/post rotation");
    }

    /// 2026-10-12 review, finding 25.3: the SDK's template default for
    /// RotationActive is 0, but a file that states it nowhere (not on the
    /// model, not in a template) is active to ufbx —
    /// `ufbxi_find_int(&node->props, ufbxi_RotationActive, 1)` (ufbx.c:22964
    /// at 5955c5c) — and to three.js and Assimp, which ignore the flag.
    /// Stated off, on the model or by its template, rotation order and
    /// pre/post rotation are ignored, as the SDK defines.
    #[test]
    fn rotation_active_reads_as_ufbx_reads_it() {
        let rest = "\t\t\tP: \"RotationOrder\", \"enum\", \"\", \"\",5\n\t\t\tP: \"PreRotation\", \"Vector3D\", \"Vector\", \"\",90,0,0\n\t\t\tP: \"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\",10,20,30";
        let pre = math::quat_to_mat(math::euler_to_quat(deg([90.0, 0.0, 0.0]), math::EulerOrder::Xyz));
        let on = math::mat_mul(&pre, &math::quat_to_mat(math::euler_to_quat(deg([10.0, 20.0, 30.0]), math::EulerOrder::Zyx)));
        let off = math::quat_to_mat(math::euler_to_quat(deg([10.0, 20.0, 30.0]), math::EulerOrder::Xyz));
        assert_points(&world_triangle(&transform_doc(&[(10, 0, rest)], ""), None), &on, "stated nowhere");
        let props = format!("\t\t\tP: \"RotationActive\", \"bool\", \"\", \"\",0\n{rest}");
        assert_points(&world_triangle(&transform_doc(&[(10, 0, &props)], ""), None), &off, "RotationActive 0");
        let template = "Definitions:  {\n\tObjectType: \"Model\" {\n\t\tPropertyTemplate: \"FbxNode\" {\n\t\t\tProperties70:  {\n\t\t\t\tP: \"RotationActive\", \"bool\", \"\", \"\",0\n\t\t\t}\n\t\t}\n\t}\n}\nObjects:  {\n";
        let doc = transform_doc(&[(10, 0, rest)], "").replacen("Objects:  {\n", template, 1);
        assert_points(&world_triangle(&doc, None), &off, "the template's 0");
    }

    #[test]
    fn rotation_and_scaling_pivots_keep_their_point_fixed() {
        let props = "\t\t\tP: \"RotationPivot\", \"Vector3D\", \"Vector\", \"\",1,0,0\n\t\t\tP: \"ScalingPivot\", \"Vector3D\", \"Vector\", \"\",1,0,0\n\t\t\tP: \"RotationOffset\", \"Vector3D\", \"Vector\", \"\",0,0,5\n\t\t\tP: \"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\",0,0,90\n\t\t\tP: \"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\",2,2,2";
        let got = world_triangle(&transform_doc(&[(10, 0, props)], ""), None);
        // T·Roff·Rp·R·Rp⁻¹·Sp·S·Sp⁻¹ with Rp = Sp = (1,0,0).
        let rp = math::translation([1.0, 0.0, 0.0]);
        let rp_inv = math::translation([-1.0, 0.0, 0.0]);
        let r = math::quat_to_mat(math::euler_to_quat(deg([0.0, 0.0, 90.0]), math::EulerOrder::Xyz));
        let mut m = math::translation([0.0, 0.0, 5.0]);
        for x in [rp, r, rp_inv, rp, math::scaling([2.0; 3]), rp_inv] {
            m = math::mat_mul(&m, &x);
        }
        assert_points(&got, &m, "pivots");
        // The pivot itself (corner 1, at x = 1) only moves by the offset.
        assert!((got[1][0] - 1.0).abs() < 1e-6 && got[1][1].abs() < 1e-6 && (got[1][2] - 5.0).abs() < 1e-6, "{:?}", got[1]);
    }

    /// 2026-10-12 review, finding 19: under a sheared parent (a grandparent
    /// scaled (1,2,1), the parent turned 45°) the parent's column-normalised
    /// rotation is not orthogonal, and its transpose is not its inverse. An
    /// identity InheritType 2 child must sit exactly where its parent is (the
    /// SDK's formula; three.js FBXLoader inverts, `FBXLoader.js`
    /// `lParentGSM.copy(lParentGRM).invert().multiply(lParentGRSM)`); it came
    /// out squashed about 1.44× (up to 0.849 off).
    #[test]
    fn inherit_types_zero_and_two_under_a_sheared_parent_use_a_true_inverse() {
        let grand = "\t\t\tP: \"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\",1,2,1";
        // InheritType 1 on the parent: ordinary composition, which is what
        // shears it (the default, 0, would apply the scale after its turn).
        let parent = "\t\t\tP: \"InheritType\", \"enum\", \"\", \"\",1\n\t\t\tP: \"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\",0,0,45";
        let m = math::mat_mul(&math::scaling([1.0, 2.0, 1.0]), &math::quat_to_mat(math::euler_to_quat(deg([0.0, 0.0, 45.0]), math::EulerOrder::Xyz)));
        for inherit in [0, 2] {
            let child = format!("\t\t\tP: \"InheritType\", \"enum\", \"\", \"\",{inherit}");
            let got = world_triangle(&transform_doc(&[(10, 0, grand), (11, 10, parent), (12, 11, &child)], ""), None);
            assert_points(&got, &m, &format!("identity InheritType {inherit} child under a sheared parent"));
        }
    }

    #[test]
    fn inherit_type_zero_applies_the_parents_scale_after_the_childs_rotation() {
        let parent = "\t\t\tP: \"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\",3,1,1";
        let child = "\t\t\tP: \"InheritType\", \"enum\", \"\", \"\",0\n\t\t\tP: \"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\",0,0,90";
        let got = world_triangle(&transform_doc(&[(10, 0, parent), (11, 10, child)], ""), None);
        // RrSs: world = Rparent·Rchild·Sparent·Schild (no shear under the
        // rotated child, unlike ordinary composition).
        let r = math::quat_to_mat(math::euler_to_quat(deg([0.0, 0.0, 90.0]), math::EulerOrder::Xyz));
        let m = math::mat_mul(&r, &math::scaling([3.0, 1.0, 1.0]));
        assert_points(&got, &m, "InheritType 0");
        // And it differs from glTF composition, so the test means something.
        let plain = math::mat_mul(&math::scaling([3.0, 1.0, 1.0]), &r);
        assert!((math::transform_point_f64(&plain, [1.0, 0.0, 0.0])[1] - got[1][1]).abs() > 0.5);
    }

    #[test]
    fn inherit_type_two_ignores_the_parents_own_scale() {
        let parent = "\t\t\tP: \"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\",2,2,2";
        let child = "\t\t\tP: \"InheritType\", \"enum\", \"\", \"\",2\n\t\t\tP: \"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",1,0,0";
        let got = world_triangle(&transform_doc(&[(10, 0, parent), (11, 10, child)], ""), None);
        // Rrs: the child's translation is still scaled by the parent (it is a
        // position in the parent's space); its geometry is not.
        let m = math::translation([2.0, 0.0, 0.0]);
        assert_points(&got, &m, "InheritType 2");
    }

    #[test]
    fn an_animated_rotation_about_a_pivot_is_resampled_on_its_arc() {
        let props = "\t\t\tP: \"RotationPivot\", \"Vector3D\", \"Vector\", \"\",0,1,0\n\t\t\tP: \"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A+\",0,0,0";
        let curves = "\tAnimationStack: 1, \"AnimStack::spin\", \"\" {\n\t}\n\tAnimationLayer: 2, \"AnimLayer::base\", \"\" {\n\t}\n\tAnimationCurveNode: 3, \"AnimCurveNode::R\", \"\" {\n\t\tProperties70:  {\n\t\t\tP: \"d|X\", \"Number\", \"\", \"A\",0\n\t\t\tP: \"d|Y\", \"Number\", \"\", \"A\",0\n\t\t\tP: \"d|Z\", \"Number\", \"\", \"A\",0\n\t\t}\n\t}\n\tAnimationCurve: 4, \"AnimCurve::\", \"\" {\n\t\tKeyTime: *2 {\n\t\t\ta: 0,46186158000\n\t\t}\n\t\tKeyValueFloat: *2 {\n\t\t\ta: 0,180\n\t\t}\n\t\tKeyAttrFlags: *1 {\n\t\t\ta: 4\n\t\t}\n\t\tKeyAttrDataFloat: *4 {\n\t\t\ta: 0,0,0,0\n\t\t}\n\t\tKeyAttrRefCount: *1 {\n\t\t\ta: 2\n\t\t}\n\t}\n";
        let mut src = transform_doc(&[(10, 0, props)], curves);
        src = src.replace("\tC: \"OO\",10,0\n", "\tC: \"OO\",10,0\n\tC: \"OO\",2,1\n\tC: \"OO\",3,2\n\tC: \"OP\",3,10, \"Lcl Rotation\"\n\tC: \"OP\",4,3, \"d|Z\"\n");
        for t in [0.25f32, 0.5, 0.75] {
            let got = world_triangle(&src, Some(t));
            // Corner 0 (the origin) orbits the pivot (0,1,0) at radius 1.
            let a = (180.0 * t as f64).to_radians();
            let want = [a.sin(), 1.0 - a.cos(), 0.0];
            for k in 0..3 {
                assert!((got[0][k] - want[k]).abs() < 2e-3, "t={t}: {:?} vs {want:?}", got[0]);
            }
        }
    }

    #[test]
    fn primitives_that_disagree_about_attributes_round_trip_as_prepared() {
        let mut s = quad_scene();
        let mut bare = s.meshes[0].primitives[0].clone();
        bare.uvs.clear();
        bare.normals = None;
        for p in bare.positions.iter_mut() {
            p[2] = 2.0;
        }
        s.meshes[0].primitives[0].colors = Some(vec![[0.2, 0.4, 0.6, 1.0]; 4]);
        s.meshes[0].primitives.push(bare);
        let prepared = prepare(&s, WriteOptions::default()).unwrap();
        let back = read_back(&write_prepared(&s)).scene;
        for (a, b) in prepared.meshes[0].primitives.iter().zip(&back.meshes[0].primitives) {
            assert_eq!(a.uvs.len(), b.uvs.len());
            assert_eq!(a.normals.is_some(), b.normals.is_some());
            assert_eq!(a.colors.is_some(), b.colors.is_some());
        }
        assert_same_geometry(&prepared, &pose::default_pose(&prepared), &back, &pose::default_pose(&back), 1e-6, "mixed");
    }

    // ── files written by an independent tool (Assimp 6.0) ────────────────────
    //
    // Generated from our own fixtures; provenance and commands in
    // `testdata/fbx/README.md`. Assimp writes FBX 7500 (64-bit record headers),
    // never compresses arrays, writes no BindPose, spells the scale property
    // "Lcl Scale", flags every key 0, and writes linear colours where three.js
    // and this module read sRGB — each a difference from our own writer that
    // these tests exercise.

    const ASSIMP_TEXTURED: &[u8] = include_bytes!("testdata/fbx/textured_assimp_7500.fbx");
    const ASSIMP_MORPH_ASCII: &[u8] = include_bytes!("testdata/fbx/morph_assimp_ascii.fbx");
    const ASSIMP_CUBE_ASCII: &[u8] = include_bytes!("testdata/fbx/cube_assimp_ascii.fbx");
    const ASSIMP_SKINNED: &[u8] = include_bytes!("testdata/fbx/skinned_small_assimp_7500.fbx");
    const ASSIMP_SKINNED_SOURCE: &[u8] = include_bytes!("testdata/fbx/skinned_small_source.glb");

    /// Assimp writes glTF's metre values unchanged but declares
    /// `UnitScaleFactor 1` (centimetres), so an honest reader — this one,
    /// Unity, Maya — sees the model at 1/100 scale. Check that the one root
    /// correction says exactly that, then remove it to compare shapes.
    fn read_assimp(bytes: &[u8]) -> Scene {
        unconverted(read_back(bytes).scene)
    }

    /// [`read_assimp`]'s check and removal of the root correction, for a
    /// scene already read.
    fn unconverted(mut s: Scene) -> Scene {
        assert_eq!(s.roots.len(), 1);
        let root = s.roots[0];
        assert_eq!(s.nodes[root].name.as_deref(), Some("FbxAxisUnitConversion"));
        let (t, q, sc) = s.nodes[root].transform.trs().unwrap();
        assert_eq!(t, [0.0; 3]);
        assert!(math::quat_dot(q, math::QUAT_IDENTITY).abs() > 1.0 - 1e-12);
        assert!(sc.iter().all(|v| (v - 0.01).abs() < 1e-12), "{sc:?}");
        s.nodes[root].transform = Transform::default();
        s
    }

    #[test]
    fn reads_assimps_binary_7500_with_hierarchy_instancing_and_an_embedded_texture() {
        let r = SceneRead { scene: read_assimp(ASSIMP_TEXTURED), warnings: Vec::new() };
        let want = fixtures::textured();
        assert_same_geometry(&want, &pose::default_pose(&want), &r.scene, &pose::default_pose(&r.scene), 1e-4, "assimp textured");
        assert_eq!(r.scene.images.len(), 1);
        assert_eq!(r.scene.images[0].data, want.images[0].data, "the embedded PNG must come back byte for byte");
        let names: Vec<_> = r.scene.materials.iter().filter_map(|m| m.name.clone()).collect();
        for n in ["box_mat", "wedge_mat", "panel_mat"] {
            assert!(names.iter().any(|x| x == n), "{names:?}");
        }
        assert!(r.scene.meshes.iter().flat_map(|m| &m.primitives).any(|p| p.colors.is_some()), "vertex colours");
        let textured = r.scene.materials.iter().find(|m| m.name.as_deref() == Some("box_mat")).unwrap();
        assert!(textured.base_color_texture.is_some());
    }

    #[test]
    fn reads_assimps_ascii_with_blend_shapes() {
        let r = SceneRead { scene: read_assimp(ASSIMP_MORPH_ASCII), warnings: Vec::new() };
        let mut want = fixtures::morph();
        let m = &r.scene.meshes[0];
        assert_eq!(m.target_names, vec!["bulge".to_string(), "twist".to_string()]);
        // Assimp writes DeformPercent 0 whatever glTF's default weights were.
        assert_eq!(m.weights, vec![0.0, 0.0]);
        want.meshes[0].weights = vec![0.0, 0.0];
        assert_same_geometry(&want, &pose::default_pose(&want), &r.scene, &pose::default_pose(&r.scene), 1e-4, "assimp morph");
        // Every target, fully on.
        let mut a = pose::default_pose(&want);
        let mut b = pose::default_pose(&r.scene);
        for w in [vec![1.0, 0.0], vec![0.0, 1.0]] {
            a.weights[0] = w.clone();
            let node = r.scene.nodes.iter().position(|n| n.mesh.is_some()).unwrap();
            b.weights[node] = w.clone();
            assert_same_geometry(&want, &a, &r.scene, &b, 1e-4, &format!("assimp morph at {w:?}"));
        }
    }

    #[test]
    fn reads_assimps_ascii_cube_exactly() {
        let got = read_assimp(ASSIMP_CUBE_ASCII);
        let want = fixtures::cube();
        assert_same_geometry(&want, &pose::default_pose(&want), &got, &pose::default_pose(&got), 1e-6, "assimp cube");
    }

    #[test]
    fn reads_assimps_skin_and_animation_without_a_bind_pose() {
        let r = SceneRead { scene: read_assimp(ASSIMP_SKINNED), warnings: Vec::new() };
        let src = super::super::read_scene(ASSIMP_SKINNED_SOURCE, MeshFormat::Glb, &NoSidecars).unwrap().scene;
        assert_eq!(r.scene.skins.len(), 1);
        assert_same_geometry(&src, &pose::default_pose(&src), &r.scene, &pose::default_pose(&r.scene), 1e-4, "assimp skin, rest");
        // The source keeps only linear single-axis rotations (see the
        // README), which Euler keys at the same times reproduce exactly; so
        // compare BETWEEN keys, where an interpolation mistake shows.
        for t in fixtures::sample_times(&src) {
            assert_same_geometry(
                &src,
                &pose::sample_pose(&src, 0, t),
                &r.scene,
                &pose::sample_pose(&r.scene, 0, t),
                1e-3,
                &format!("assimp skin @ {t}"),
            );
        }
    }

    const ASSIMP_STAGED: &[u8] = include_bytes!("testdata/fbx/staged_assimp.fbx");

    /// The committed third-party FBX with lights (Ruling R10):
    /// `fixtures::staged()` written as GLB by litegen's glTF writer and
    /// converted by Assimp 6.0.5 (`testdata/fbx/README.md`), read under
    /// Assimp's own conventions, each from its source at v6.0.5:
    /// - its glTF importer multiplies each light's colour by its intensity
    ///   (`glTF2Importer.cpp` L975-L978), and its FBX exporter writes that as
    ///   `Color`, with `Intensity` always 1000 ("Default value to 1000W",
    ///   `FBXExporter.cpp` L2271-L2276); so this reader, taking FBX's
    ///   intensity as percent and folding a colour past 1 into the
    ///   intensity (R8), reads the fixture's colour at 10 × its intensity;
    /// - its cones are glTF's half-angles (`glTF2Importer.cpp` L999-L1000),
    ///   written in degrees (`FBXExporter.cpp` L2286-L2289) where FBX's are
    ///   full cones, so they read as half the fixture's;
    /// - `DecayType` 0, no decay (`FBXExporter.cpp` L2303-L2304), which the
    ///   scene's inverse-square lights cannot hold: warned once for each
    ///   point and spot light;
    /// - each light's model is written with its glTF node's own transform
    ///   (`WriteModelNode` with "Light", `FBXExporter.cpp` L2781-L2787), no
    ///   turn added, so, FBX lights pointing along −Y, each shines along its
    ///   node's −Y where the fixture's shone along −Z;
    /// - the bulb's range goes on its model as `PBR_LightRange`
    ///   (`glTF2Importer.cpp` L1258-L1263), no FBX light property, so it
    ///   reads as none;
    /// - **Assimp writes no cameras** (`FBXExporter.cpp` L2316,
    ///   `// TODO: cameras`): the camera nodes read as plain nodes.
    ///
    /// Assimp writes its colours linear (`testdata/fbx/README.md`), so the
    /// file is read as `FbxColorSpace::Linear` (Ruling R14). It declares
    /// `UnitScaleFactor` 1 on metre values; [`unconverted`] takes off the
    /// hundredth this reader honestly applies.
    #[test]
    fn reads_assimps_lights_under_its_own_conventions() {
        let read = read_scene_with(ASSIMP_STAGED, &NoSidecars, FbxColorSpace::Linear, LightUnits::default()).unwrap_or_else(|e| panic!("{e}"));
        let decays = ["light \"bulb_node\" ", "light \"spot_node\" "];
        assert_eq!(read.warnings.len(), decays.len(), "{:#?}", read.warnings);
        for name in decays {
            assert!(read.warnings.iter().any(|w| w.starts_with(name) && w.contains("DecayType 0")), "no decay warning for {name}: {:#?}", read.warnings);
        }
        let got = unconverted(read.scene);
        let staged = fixtures::staged();
        assert!(got.cameras.is_empty(), "Assimp writes no cameras: {:?}", got.cameras);
        for name in ["persp_camera_node", "ortho_camera_node"] {
            assert_eq!(got.nodes[node_named(&got, name)].camera, None, "{name} is a plain node");
        }
        let (want, have) = (lightcam::frames(&staged, None).0, lightcam::frames(&got, None).0);
        let nodes = |f: &[lightcam::LightFrame]| f.iter().map(|f| f.node.clone()).collect::<Vec<String>>();
        assert_eq!(nodes(&have), nodes(&want), "the nodes that place lights");
        let world = staged.world_matrices();
        for (w, g) in want.iter().zip(&have) {
            let at = format!("the light on {:?}", w.node);
            assert_close3(g.position, w.position, 1e-5, &format!("{at}: position"));
            let m = world[node_named(&staged, &w.node)];
            let len = (m[4] * m[4] + m[5] * m[5] + m[6] * m[6]).sqrt();
            assert_close3(g.direction, [-m[4] / len, -m[5] / len, -m[6] / len], 1e-5, &format!("{at}: along its node's −Y"));
            assert_eq!(g.light.name.as_deref(), Some(w.node.as_str()), "{at}: Assimp names a light after its node");
            assert_close3(g.light.color.map(f64::from), w.light.color.map(f64::from), 1e-6, &format!("{at}: colour"));
            let intensity = 10.0 * f64::from(w.light.intensity);
            assert!((f64::from(g.light.intensity) - intensity).abs() <= 1e-6 * intensity, "{at}: intensity {} against {intensity}", g.light.intensity);
            assert_eq!(g.light.range, None, "{at}: no range");
            match (w.light.kind, g.light.kind) {
                (LightKind::Spot { inner_cone: wi, outer_cone: wo }, LightKind::Spot { inner_cone: gi, outer_cone: go }) => {
                    assert!((gi - wi / 2.0).abs() < 1e-6 && (go - wo / 2.0).abs() < 1e-6, "{at}: cones {gi}, {go} against half of {wi}, {wo}");
                }
                (wk, gk) => assert_eq!(gk, wk, "{at}: kind"),
            }
        }
    }

    // ── hostile input ───────────────────────────────────────────────────────

    #[test]
    fn rejects_other_containers_as_not_this_format() {
        for bytes in [&b"glTF\x02\0\0\0"[..], b"v 0 0 0\n", b"ply\nformat ascii 1.0\n", b"solid x\n", b"{}", b"", b"PK\x03\x04"] {
            let e = read_scene(bytes, &NoSidecars).unwrap_err();
            assert!(matches!(e, MeshError::NotThisFormat { .. }), "{bytes:?}: {e:?}");
        }
    }

    #[test]
    fn a_text_mode_corrupted_binary_is_named_as_such() {
        let mut bytes = write_prepared(&fixtures::cube());
        for b in bytes.iter_mut() {
            if *b == 0 {
                *b = b' ';
            }
        }
        let e = read_scene(&bytes, &NoSidecars).unwrap_err();
        assert!(matches!(&e, MeshError::Malformed(m) if m.contains("corrupted")), "{e:?}");
    }

    #[test]
    fn fbx6_is_refused_as_unsupported() {
        let mut bytes = write_prepared(&fixtures::cube());
        bytes[23..27].copy_from_slice(&6100u32.to_le_bytes());
        assert!(matches!(read_scene(&bytes, &NoSidecars), Err(MeshError::Unsupported(_))));
    }

    #[test]
    fn never_panics_on_truncation() {
        for (_, f) in fixtures::all() {
            let bytes = write_prepared(&f);
            let step = (bytes.len() / 400).max(1);
            for cut in (0..bytes.len()).step_by(step) {
                let _ = read_scene(&bytes[..cut], &NoSidecars);
            }
        }
    }

    #[test]
    fn never_panics_on_single_byte_corruption() {
        for (_, f) in fixtures::all() {
            let bytes = write_prepared(&f);
            let step = (bytes.len() / 300).max(1);
            for i in (0..bytes.len()).step_by(step) {
                for flip in [0x01u8, 0x80] {
                    let mut v = bytes.clone();
                    v[i] ^= flip;
                    let _ = read_scene(&v, &NoSidecars);
                }
            }
        }
    }

    #[test]
    fn never_panics_on_corrupted_ascii() {
        let src = super::super::fbx_write::tests_support::ascii_cube();
        let b = src.as_bytes();
        for cut in (0..b.len()).step_by((b.len() / 300).max(1)) {
            let _ = read_scene(&b[..cut], &NoSidecars);
        }
        for i in (0..b.len()).step_by((b.len() / 300).max(1)) {
            for c in *b"{},\"-9\n" {
                let mut v = b.to_vec();
                v[i] = c;
                let _ = read_scene(&v, &NoSidecars);
            }
        }
    }

    #[test]
    fn an_out_of_range_polygon_index_is_malformed() {
        let src = super::super::fbx_write::tests_support::ascii_cube().replace("a: 0,1,-3", "a: 0,7,-3");
        let e = super::super::read_scene(src.as_bytes(), MeshFormat::Fbx, &NoSidecars).unwrap_err();
        assert!(matches!(e, MeshError::Malformed(_)), "{e:?}");
    }

    #[test]
    fn an_unterminated_polygon_list_is_malformed() {
        let src = super::super::fbx_write::tests_support::ascii_cube().replace("a: 0,1,-3", "a: 0,1,2");
        let e = super::super::read_scene(src.as_bytes(), MeshFormat::Fbx, &NoSidecars).unwrap_err();
        assert!(matches!(e, MeshError::Malformed(_)), "{e:?}");
    }

    #[test]
    fn a_tiny_file_cannot_declare_a_giant_array() {
        // Vertices claiming 2^31 doubles in a few bytes of zlib.
        let mut bytes = write_prepared(&fixtures::cube());
        let at = bytes.windows(8).position(|w| w == b"Vertices").expect("Vertices record");
        // The array header follows the name: 'd', u32 count, u32 enc, u32 clen.
        let p = at + 8;
        assert_eq!(bytes[p], b'd');
        bytes[p + 1..p + 5].copy_from_slice(&0x7FFF_FFFFu32.to_le_bytes());
        let started = std::time::Instant::now();
        assert!(read_scene(&bytes, &NoSidecars).is_err());
        assert!(started.elapsed() < std::time::Duration::from_millis(500), "{:?}", started.elapsed());
    }

    #[test]
    fn deep_nesting_is_refused_without_recursion() {
        let mut src = String::from("FBXHeaderExtension:  {\n\tFBXVersion: 7400\n}\n");
        for _ in 0..100_000 {
            src.push_str("X: {\n");
        }
        let e = read_scene(src.as_bytes(), &NoSidecars).unwrap_err();
        assert!(matches!(e, MeshError::Unsupported(_) | MeshError::Malformed(_)), "{e:?}");
    }

    #[test]
    fn a_model_cycle_is_malformed_not_a_hang() {
        let src = super::super::fbx_write::tests_support::ascii_cube()
            .replace("C: \"OO\",10,0", "C: \"OO\",10,11\n\tC: \"OO\",11,10");
        let src = src.replace("Objects:  {", "Objects:  {\n\tModel: 11, \"Model::loop\", \"Null\" {\n\t}");
        let e = super::super::read_scene(src.as_bytes(), MeshFormat::Fbx, &NoSidecars);
        assert!(e.is_err(), "{e:?}");
    }

    #[test]
    fn in_between_shapes_become_targets_driven_by_the_channel_weight() {
        // One channel, a shape at 50% and one at 100%, at 75% and animated
        // from 0 to 100% over a second.
        let src = "; FBX 7.4.0 project file\nFBXHeaderExtension:  {\n\tFBXVersion: 7400\n}\nGlobalSettings:  {\n\tProperties70:  {\n\t\tP: \"UnitScaleFactor\", \"double\", \"Number\", \"\",100\n\t}\n}\nObjects:  {\n\tModel: 10, \"Model::m\", \"Mesh\" {\n\t}\n\tGeometry: 99, \"Geometry::tri\", \"Mesh\" {\n\t\tVertices: *9 {\n\t\t\ta: 0,0,0,1,0,0,0,1,0\n\t\t}\n\t\tPolygonVertexIndex: *3 {\n\t\t\ta: 0,1,-3\n\t\t}\n\t}\n\tDeformer: 20, \"Deformer::bs\", \"BlendShape\" {\n\t}\n\tDeformer: 21, \"SubDeformer::smile\", \"BlendShapeChannel\" {\n\t\tDeformPercent: 75\n\t\tFullWeights: *2 {\n\t\t\ta: 50,100\n\t\t}\n\t}\n\tGeometry: 30, \"Geometry::half\", \"Shape\" {\n\t\tIndexes: *1 {\n\t\t\ta: 2\n\t\t}\n\t\tVertices: *3 {\n\t\t\ta: 1,0,0\n\t\t}\n\t}\n\tGeometry: 31, \"Geometry::full\", \"Shape\" {\n\t\tIndexes: *1 {\n\t\t\ta: 2\n\t\t}\n\t\tVertices: *3 {\n\t\t\ta: 0,2,0\n\t\t}\n\t}\n\tAnimationStack: 40, \"AnimStack::take\", \"\" {\n\t}\n\tAnimationLayer: 41, \"AnimLayer::base\", \"\" {\n\t}\n\tAnimationCurveNode: 42, \"AnimCurveNode::DeformPercent\", \"\" {\n\t}\n\tAnimationCurve: 43, \"AnimCurve::\", \"\" {\n\t\tKeyTime: *2 {\n\t\t\ta: 0,46186158000\n\t\t}\n\t\tKeyValueFloat: *2 {\n\t\t\ta: 0,100\n\t\t}\n\t\tKeyAttrFlags: *1 {\n\t\t\ta: 4\n\t\t}\n\t\tKeyAttrDataFloat: *4 {\n\t\t\ta: 0,0,0,0\n\t\t}\n\t\tKeyAttrRefCount: *1 {\n\t\t\ta: 2\n\t\t}\n\t}\n}\nConnections:  {\n\tC: \"OO\",10,0\n\tC: \"OO\",99,10\n\tC: \"OO\",20,99\n\tC: \"OO\",21,20\n\tC: \"OO\",30,21\n\tC: \"OO\",31,21\n\tC: \"OO\",41,40\n\tC: \"OO\",42,41\n\tC: \"OP\",42,21,\"DeformPercent\"\n\tC: \"OP\",43,42,\"d|DeformPercent\"\n}\n";
        let r = super::super::read_scene(src.as_bytes(), MeshFormat::Fbx, &NoSidecars).unwrap();
        let m = &r.scene.meshes[0];
        assert_eq!(m.target_names, ["smile@50", "smile"]);
        assert_eq!(m.weights, [0.5, 0.5]);
        assert!(r.warnings.iter().any(|w| w.contains("in-between")), "{:?}", r.warnings);
        // The channel weight crosses the in-between's 50% at half a second:
        // a key there keeps each target's share piecewise linear.
        let ch = r.scene.animations[0].channels.iter().find(|c| c.path == AnimPath::Weights).unwrap();
        assert!(ch.times.iter().any(|t| (t - 0.5).abs() < 1e-6), "{:?}", ch.times);
        for (t, corner) in [(0.25, [0.5, 1.0]), (0.5, [1.0, 1.0]), (0.75, [0.5, 2.0]), (1.0, [0.0, 3.0])] {
            let got = world_triangle(src, Some(t));
            assert!((got[2][0] - corner[0]).abs() < 1e-5 && (got[2][1] - corner[1]).abs() < 1e-5, "{t}: {:?}", got[2]);
        }
    }

    #[test]
    fn morph_and_skin_index_arrays_are_bounds_checked() {
        let mut s = fixtures::morph();
        s.animations.clear();
        let bytes = write_prepared(&s);
        // Corrupt every Indexes array's first element to a huge value.
        let mut v = bytes.clone();
        let mut hits = 0;
        let mut i = 0;
        while let Some(at) = v[i..].windows(7).position(|w| w == b"Indexes") {
            let p = i + at + 7;
            if v[p] == b'i' && u32::from_le_bytes(v[p + 5..p + 9].try_into().unwrap()) == 0 {
                v[p + 13..p + 17].copy_from_slice(&0x7FFF_FFF0i32.to_le_bytes());
                hits += 1;
            }
            i = p;
        }
        assert!(hits > 0);
        let e = read_scene(&v, &NoSidecars).unwrap_err();
        assert!(matches!(e, MeshError::Malformed(_)), "{e:?}");
        let _ = (MorphTarget::default(), Skin::default());
    }

    /// An ASCII FBX whose one mesh has `channels` blend-shape channels of one
    /// shape each, the first animated by one cubic curve of `keys` keys
    /// `spacing` seconds apart, alternating 0 and 100.
    fn many_channel_morph(channels: usize, keys: usize, spacing: u64) -> String {
        let mut objects = String::from("\tModel: 10, \"Model::m\", \"Mesh\" {\n\t}\n\tGeometry: 99, \"Geometry::tri\", \"Mesh\" {\n\t\tVertices: *9 {\n\t\t\ta: 0,0,0,1,0,0,0,1,0\n\t\t}\n\t\tPolygonVertexIndex: *3 {\n\t\t\ta: 0,1,-3\n\t\t}\n\t}\n\tDeformer: 20, \"Deformer::bs\", \"BlendShape\" {\n\t}\n");
        let mut links = String::from("\tC: \"OO\",10,0\n\tC: \"OO\",99,10\n\tC: \"OO\",20,99\n");
        for i in 0..channels {
            let (ch, shape) = (1000 + 2 * i, 1001 + 2 * i);
            objects.push_str(&format!("\tDeformer: {ch}, \"SubDeformer::c{i}\", \"BlendShapeChannel\" {{\n\t\tDeformPercent: 0\n\t\tFullWeights: *1 {{\n\t\t\ta: 100\n\t\t}}\n\t}}\n\tGeometry: {shape}, \"Geometry::s{i}\", \"Shape\" {{\n\t\tIndexes: *1 {{\n\t\t\ta: 2\n\t\t}}\n\t\tVertices: *3 {{\n\t\t\ta: 0,0,1\n\t\t}}\n\t}}\n"));
            links.push_str(&format!("\tC: \"OO\",{ch},20\n\tC: \"OO\",{shape},{ch}\n"));
        }
        let times: Vec<String> = (0..keys as u64).map(|k| (k * spacing * 46_186_158_000).to_string()).collect();
        let values: Vec<&str> = (0..keys).map(|k| if k % 2 == 0 { "0" } else { "100" }).collect();
        objects.push_str(&format!("\tAnimationStack: 40, \"AnimStack::take\", \"\" {{\n\t}}\n\tAnimationLayer: 41, \"AnimLayer::base\", \"\" {{\n\t}}\n\tAnimationCurveNode: 42, \"AnimCurveNode::DeformPercent\", \"\" {{\n\t}}\n\tAnimationCurve: 43, \"AnimCurve::\", \"\" {{\n\t\tKeyTime: *{keys} {{\n\t\t\ta: {}\n\t\t}}\n\t\tKeyValueFloat: *{keys} {{\n\t\t\ta: {}\n\t\t}}\n\t\tKeyAttrFlags: *1 {{\n\t\t\ta: 1032\n\t\t}}\n\t\tKeyAttrDataFloat: *4 {{\n\t\t\ta: 0,0,0,0\n\t\t}}\n\t\tKeyAttrRefCount: *1 {{\n\t\t\ta: {keys}\n\t\t}}\n\t}}\n", times.join(","), values.join(",")));
        links.push_str("\tC: \"OO\",41,40\n\tC: \"OO\",42,41\n\tC: \"OP\",42,1000,\"DeformPercent\"\n\tC: \"OP\",43,42,\"d|DeformPercent\"\n");
        format!("; FBX 7.4.0 project file\nFBXHeaderExtension:  {{\n\tFBXVersion: 7400\n}}\nObjects:  {{\n{objects}}}\nConnections:  {{\n{links}}}\n")
    }

    /// 2026-10-12 review, finding 2: the weights of every channel at every
    /// resampled time were allocated with nothing charging them. 1,024
    /// channels and an 80-key cubic curve resampled to ~79k times are 81M
    /// values (324 MB, and a copy per node); the reproduction's 160-key curve
    /// aborted on 651 MB.
    #[test]
    fn blend_shape_weights_are_charged_before_they_are_allocated() {
        let small = read_scene(many_channel_morph(8, 2, 1).as_bytes(), &NoSidecars).unwrap().scene;
        assert_eq!(small.meshes[0].weights.len(), 8);
        assert_eq!(small.animations.len(), 1);
        let keys = small.animations[0].channels[0].times.len();
        assert!(keys > 30, "the cubic curve is sampled at 30 Hz or finer: {keys} keys over 1 s");
        let e = read_scene(many_channel_morph(1024, 80, 40).as_bytes(), &NoSidecars).unwrap_err();
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("weight")), "{e:?}");
    }

    /// A chain of `depth` models, each InheritType 2 (as Mixamo writes every
    /// joint) and each with its own translation curve, a triangle under the
    /// last.
    fn animated_inherit_chain(depth: usize) -> String {
        let mut objects = String::from("\tAnimationStack: 40, \"AnimStack::take\", \"\" {\n\t}\n\tAnimationLayer: 41, \"AnimLayer::base\", \"\" {\n\t}\n\tGeometry: 99, \"Geometry::tri\", \"Mesh\" {\n\t\tVertices: *9 {\n\t\t\ta: 0,0,0,1,0,0,0,1,0\n\t\t}\n\t\tPolygonVertexIndex: *3 {\n\t\t\ta: 0,1,-3\n\t\t}\n\t}\n");
        let mut links = String::from("\tC: \"OO\",41,40\n");
        for i in 0..depth {
            let (id, cn, cu) = (1_000_000 + i, 2_000_000 + i, 3_000_000 + i);
            let parent = if i == 0 { 0 } else { id - 1 };
            objects.push_str(&format!("\tModel: {id}, \"Model::m{i}\", \"Null\" {{\n\t\tProperties70:  {{\n\t\t\tP: \"InheritType\", \"enum\", \"\", \"\",2\n\t\t}}\n\t}}\n\tAnimationCurveNode: {cn}, \"AnimCurveNode::T\", \"\" {{\n\t}}\n\tAnimationCurve: {cu}, \"AnimCurve::\", \"\" {{\n\t\tKeyTime: *2 {{\n\t\t\ta: 0,46186158000\n\t\t}}\n\t\tKeyValueFloat: *2 {{\n\t\t\ta: 0,1\n\t\t}}\n\t\tKeyAttrFlags: *1 {{\n\t\t\ta: 4\n\t\t}}\n\t\tKeyAttrDataFloat: *4 {{\n\t\t\ta: 0,0,0,0\n\t\t}}\n\t\tKeyAttrRefCount: *1 {{\n\t\t\ta: 2\n\t\t}}\n\t}}\n"));
            links.push_str(&format!("\tC: \"OO\",{id},{parent}\n\tC: \"OO\",{cn},41\n\tC: \"OP\",{cn},{id},\"Lcl Translation\"\n\tC: \"OP\",{cu},{cn},\"d|X\"\n"));
        }
        links.push_str(&format!("\tC: \"OO\",99,{}\n", 1_000_000 + depth - 1));
        format!("; FBX 7.4.0 project file\nFBXHeaderExtension:  {{\n\tFBXVersion: 7400\n}}\nObjects:  {{\n{objects}}}\nConnections:  {{\n{links}}}\n")
    }

    /// 2026-10-12 review, finding 17: deciding whether an InheritType 0/2
    /// node sits under a scale-animated one walked to the root for every
    /// animated node — quadratic in a deep chain (a 12 MB FBX chain took 45 s).
    #[test]
    fn scale_animated_ancestors_are_found_once_per_chain() {
        let doc = animated_inherit_chain(8000);
        let parsed = std::time::Instant::now();
        let dom = parse(doc.as_bytes()).unwrap();
        let parse_time = parsed.elapsed();
        drop(dom);
        let started = std::time::Instant::now();
        let s = read_scene(doc.as_bytes(), &NoSidecars).unwrap().scene;
        assert_eq!(s.animations.len(), 1);
        let read_time = started.elapsed();
        assert!(read_time < parse_time * 4 + std::time::Duration::from_millis(500), "read {read_time:?} against a parse of {parse_time:?}");
    }

    /// The 2026-10-31 audit: the writer keeps every clip as its own
    /// AnimationStack, but the reader carried only the active one, so a
    /// delivered FBX's manifest listed one clip of two (and `mod.rs`'s table
    /// claimed FBX keeps only the first).
    #[test]
    fn every_animation_stack_reads_back_as_its_own_clip() {
        let mut s = fixtures::skinned();
        let mut second = s.animations[0].clone();
        second.name = Some("second".into());
        for ch in second.channels.iter_mut() {
            for t in ch.times.iter_mut() {
                *t *= 0.5;
            }
        }
        s.animations[0].name = Some("first".into());
        s.animations.push(second);
        let back = read_scene(&write_prepared(&s), &NoSidecars).unwrap();
        let names: Vec<Option<&str>> = back.scene.animations.iter().map(|a| a.name.as_deref()).collect();
        assert_eq!(names, [Some("first"), Some("second")], "{:?}", back.warnings);
        let (_, hi0) = back.scene.animations[0].time_range().unwrap();
        let (_, hi1) = back.scene.animations[1].time_range().unwrap();
        assert!((hi1 - hi0 * 0.5).abs() < 1e-3, "each clip keeps its own keys: {hi0} {hi1}");
    }

    // ── lights and cameras (mesh-gaps item 5) ───────────────────────────────

    /// The objects of `kind` in a parsed file, each as (record, id, name,
    /// class).
    fn fbx_objects(dom: &Dom<'_>, kind: &str) -> Vec<(RecId, i64, String, String)> {
        let objects = dom.child(super::super::fbx_dom::ROOT, "Objects").expect("an Objects section");
        dom.children_named(objects, kind)
            .map(|o| {
                let p = dom.props(o).unwrap();
                let id = p[0].as_i64().unwrap();
                let name = split_object_name(p[1].as_bytes().unwrap(), dom.encoding).0;
                let class = p.get(2).and_then(|c| c.as_str()).map(|c| c.into_owned()).unwrap_or_default();
                (o, id, name, class)
            })
            .collect()
    }

    /// The numbers of property `prop` in object `rec`'s `Properties70`, after
    /// its four header strings: what any reader of the file sees.
    fn fbx_numbers(dom: &Dom<'_>, rec: RecId, prop: &str) -> Option<Vec<f64>> {
        let p70 = dom.child(rec, "Properties70")?;
        dom.children_named(p70, "P").find_map(|p| {
            let v = dom.props(p).ok()?;
            (v.first()?.as_str()?.as_ref() == prop).then(|| v.iter().skip(4).filter_map(|x| x.as_f64()).collect())
        })
    }

    /// The model named `name`: its record, its class, and the class of the
    /// `NodeAttribute` connected to it, if any.
    fn fbx_model(dom: &Dom<'_>, name: &str) -> (RecId, String, Option<String>) {
        let (rec, id, _, class) =
            fbx_objects(dom, "Model").into_iter().find(|o| o.2 == name).unwrap_or_else(|| panic!("the file has no model {name:?}"));
        let attributes = fbx_objects(dom, "NodeAttribute");
        let conns = dom.child(super::super::fbx_dom::ROOT, "Connections").expect("a Connections section");
        let attribute = dom.children_named(conns, "C").find_map(|c| {
            let p = dom.props(c).ok()?;
            let (child, parent) = (p.get(1)?.as_i64()?, p.get(2)?.as_i64()?);
            if parent != id {
                return None;
            }
            attributes.iter().find(|a| a.1 == child).map(|a| a.3.clone())
        });
        (rec, class, attribute)
    }

    /// Where the model's own `Lcl Rotation`, as written (XYZ Euler degrees),
    /// turns `axis`.
    fn wire_axis(dom: &Dom<'_>, rec: RecId, axis: [f64; 3]) -> [f64; 3] {
        let e = fbx_numbers(dom, rec, "Lcl Rotation").unwrap_or_else(|| vec![0.0; 3]);
        let q = math::euler_to_quat([e[0].to_radians(), e[1].to_radians(), e[2].to_radians()], math::EulerOrder::Xyz);
        math::transform_point_f64(&math::quat_to_mat(q), axis)
    }

    /// A written model's local matrix: T·R·S from its own `Lcl Translation`,
    /// `Lcl Rotation` (XYZ Euler degrees) and `Lcl Scaling`, as the writer
    /// writes every model (no pivots, `InheritType` 1, plain composition).
    fn wire_local(dom: &Dom<'_>, rec: RecId) -> math::Mat4 {
        let v = |prop: &str, d: [f64; 3]| fbx_numbers(dom, rec, prop).map_or(d, |v| [v[0], v[1], v[2]]);
        let e = v("Lcl Rotation", [0.0; 3]);
        let q = math::euler_to_quat([e[0].to_radians(), e[1].to_radians(), e[2].to_radians()], math::EulerOrder::Xyz);
        math::compose(v("Lcl Translation", [0.0; 3]), q, v("Lcl Scaling", [1.0; 3]))
    }

    /// The X, Y and Z curves of a written model's property `prop`, each as
    /// its key times (ticks) and values.
    fn wire_curves(dom: &Dom<'_>, model: &str, prop: &str) -> Vec<(Vec<i64>, Vec<f32>)> {
        let id = fbx_objects(dom, "Model").into_iter().find(|o| o.2 == model).map(|o| o.1).expect("the model");
        let conns = dom.child(super::super::fbx_dom::ROOT, "Connections").expect("a Connections section");
        let links: Vec<(i64, i64, String)> = dom
            .children_named(conns, "C")
            .filter_map(|c| {
                let p = dom.props(c).ok()?;
                Some((p.get(1)?.as_i64()?, p.get(2)?.as_i64()?, p.get(3).and_then(|x| x.as_str()).map(|x| x.into_owned()).unwrap_or_default()))
            })
            .collect();
        let node = links.iter().find(|l| l.1 == id && l.2 == prop).map(|l| l.0).expect("a curve node on the property");
        let curves = fbx_objects(dom, "AnimationCurve");
        ["d|X", "d|Y", "d|Z"]
            .iter()
            .map(|axis| {
                let curve = links.iter().find(|l| l.1 == node && l.2 == *axis).map(|l| l.0).expect("a curve on each axis");
                let rec = curves.iter().find(|o| o.1 == curve).expect("the curve").0;
                let times = dom.i64s(&dom.child_array(rec, "KeyTime").unwrap().unwrap(), "KeyTime").unwrap();
                let values = dom.f32s(&dom.child_array(rec, "KeyValueFloat").unwrap().unwrap(), "KeyValueFloat").unwrap();
                (times, values)
            })
            .collect()
    }

    fn assert_close3(got: [f64; 3], want: [f64; 3], eps: f64, ctx: &str) {
        assert!((0..3).all(|k| (got[k] - want[k]).abs() <= eps), "{ctx}: {got:?} against {want:?}");
    }

    /// `got`'s numbers against `want`, each within `eps`.
    fn assert_numbers(got: Option<Vec<f64>>, want: &[f64], eps: f64, ctx: &str) {
        let ok = got.as_ref().is_some_and(|g| g.len() == want.len() && g.iter().zip(want).all(|(g, w)| (g - w).abs() <= eps));
        assert!(ok, "{ctx}: {got:?} against {want:?}");
    }

    fn node_named(s: &Scene, name: &str) -> usize {
        s.nodes.iter().position(|n| n.name.as_deref() == Some(name)).unwrap_or_else(|| panic!("no node {name:?}"))
    }

    /// `Properties70` lines of a number, an enum and a colour, as ASCII FBX
    /// spells them.
    fn pn(name: &str, v: f64) -> String {
        format!("\t\t\tP: \"{name}\", \"Number\", \"\", \"A\",{v}\n")
    }
    fn pe(name: &str, v: i64) -> String {
        format!("\t\t\tP: \"{name}\", \"enum\", \"\", \"\",{v}\n")
    }
    fn p3(name: &str, v: [f64; 3]) -> String {
        format!("\t\t\tP: \"{name}\", \"Color\", \"\", \"A\",{},{},{}\n", v[0], v[1], v[2])
    }

    /// An ASCII FBX of a triangle (model 10) beside `objects`, connected by
    /// `links`, with `creator` in its header and `unit` as its
    /// `UnitScaleFactor`.
    fn lit_doc(creator: &str, unit: f64, objects: &str, links: &str) -> String {
        format!(
            "; FBX 7.4.0 project file\nFBXHeaderExtension:  {{\n\tFBXVersion: 7400\n\tCreator: \"{creator}\"\n}}\n\
             GlobalSettings:  {{\n\tVersion: 1000\n\tProperties70:  {{\n\t\tP: \"UnitScaleFactor\", \"double\", \"Number\", \"\",{unit}\n\t}}\n}}\n\
             Objects:  {{\n\tModel: 10, \"Model::tri\", \"Mesh\" {{\n\t}}\n\
             \tGeometry: 99, \"Geometry::tri\", \"Mesh\" {{\n\t\tVertices: *9 {{\n\t\t\ta: 0,0,0,1,0,0,0,1,0\n\t\t}}\n\t\tPolygonVertexIndex: *3 {{\n\t\t\ta: 0,1,-3\n\t\t}}\n\t}}\n\
             {objects}}}\nConnections:  {{\n\tC: \"OO\",10,0\n\tC: \"OO\",99,10\n{links}}}\n"
        )
    }

    /// A `NodeAttribute` of `class` ("Light" or "Camera"), id `id`, named
    /// `name` and holding `props`, on a root model `{name}_node` of the same
    /// class, id `id + 1`: the objects and the links, as ASCII FBX.
    fn light_or_camera(id: i64, name: &str, class: &str, props: &str) -> (String, String) {
        let model = id + 1;
        let objects = format!(
            "\tNodeAttribute: {id}, \"NodeAttribute::{name}\", \"{class}\" {{\n\t\tProperties70:  {{\n{props}\t\t}}\n\t\tTypeFlags: \"{class}\"\n\t}}\n\
             \tModel: {model}, \"Model::{name}_node\", \"{class}\" {{\n\t}}\n"
        );
        (objects, format!("\tC: \"OO\",{model},0\n\tC: \"OO\",{id},{model}\n"))
    }

    /// `fixtures::staged()` through FBX in both of the codec's write modes,
    /// material and light colours sRGB and linear (each also with `self_contained`
    /// off, which changes nothing for a one-file container): nothing is
    /// reported lost, the read warns of nothing, every light and camera
    /// keeps its name, and at rest and at every sample time each stands,
    /// faces and projects as the fixture's does, within 1e-5 ([`lightcam`]).
    /// Light colours are stored in the colour space material colours are
    /// (Ruling R14): sRGB, as three.js's `FBXLoader` reads them (L1196 at
    /// r183), or linear, as Blender, ufbx and assimp read them; in both, each
    /// comes back within 1e-6. The sun's stored colour, by hand (Python,
    /// 2026-11-06): sRGB(0.875) = 0.942904884492641 and sRGB(0.75) =
    /// 0.8808250210902997, or the fixture's own numbers when linear.
    #[test]
    fn every_light_and_camera_round_trips_through_fbx() {
        let staged = fixtures::staged();
        let names = |s: &Scene| {
            let mut l: Vec<Option<String>> = s.lights.iter().map(|l| l.name.clone()).collect();
            let mut c: Vec<Option<String>> = s.cameras.iter().map(|c| c.name.clone()).collect();
            l.sort();
            c.sort();
            (l, c)
        };
        for colors in [FbxColorSpace::Srgb, FbxColorSpace::Linear] {
            for self_contained in [true, false] {
                let ctx = format!("{colors:?}{}", if self_contained { "" } else { " (package)" });
                let opts = WriteOptions { self_contained, fbx_color_space: colors, ..Default::default() };
                let w = super::super::write_scene(&staged, MeshFormat::Fbx, opts).unwrap_or_else(|e| panic!("{ctx}: {e}"));
                assert!(w.losses.is_empty(), "{ctx}: {:?}", w.losses);
                let bytes = &w.package.files[0].bytes;
                let back = read_scene_with(bytes, &NoSidecars, colors, LightUnits::default()).unwrap_or_else(|e| panic!("{ctx}: {e}"));
                assert!(back.warnings.is_empty(), "{ctx}: {:?}", back.warnings);
                assert_eq!(names(&back.scene), names(&staged), "{ctx}: every light and camera keeps its name");
                lightcam::assert_same_lights_and_cameras(&staged, &back.scene, 1e-5, &ctx);
                for l in &staged.lights {
                    let got = back.scene.lights.iter().find(|g| g.name == l.name).map(|g| g.color.map(f64::from));
                    assert_close3(got.unwrap_or([f64::NAN; 3]), l.color.map(f64::from), 1e-6, &format!("{ctx}: {:?}'s colour", l.name));
                }
                let dom = parse(bytes).unwrap();
                let sun = fbx_objects(&dom, "NodeAttribute").into_iter().find(|o| o.2 == "sun");
                assert!(sun.is_some(), "{ctx}: the sun has an attribute of its own");
                let stored = match colors {
                    FbxColorSpace::Srgb => [1.0, 0.942904884492641, 0.8808250210902997],
                    FbxColorSpace::Linear => [1.0, 0.875, 0.75],
                };
                let sun = fbx_numbers(&dom, sun.unwrap().0, "Color");
                assert_numbers(sun, &stored, 1e-6, &format!("{ctx}: the sun's colour, as the colour space stores it"));
            }
        }
    }

    /// What the writer puts on the wire, in the units FBX's readers take,
    /// each value worked out by hand from `fixtures::staged()` (Python's
    /// math, 2026-11-06):
    /// - every light: a `Light` model holding a `Light` attribute (the class
    ///   Blender's importer keys its turn on, import_fbx.py L2280,
    ///   L2465-L2468); `LightType` 0 point, 1 directional, 2 spot (ufbx.h
    ///   L1381-L1395, at v0.23.1); its `Color` in the default colour space,
    ///   sRGB (Ruling R14): sRGB(0.875) = 0.942904884492641, sRGB(0.75) =
    ///   0.8808250210902997, sRGB(0.5) = 0.7353569830524495; `Intensity` 100 × the
    ///   candela or lux (ufbx.c L23049-L23052 divides it by 100, as Blender's
    ///   importer does, import_fbx.py L2251); `DecayType` 2, the inverse
    ///   square, as Blender's exporter writes (export_fbx_bin.py L619); a
    ///   spot's `InnerAngle` and `OuterAngle` as full cones in degrees,
    ///   twice the scene's half-angles, as Blender writes `spot_size`
    ///   (export_fbx_bin.py L624-L626): 2·deg(0.2) = 22.918312 and
    ///   2·deg(0.6) = 68.754938; and the bulb's 10 m range as
    ///   `EnableFarAttenuation` 1 and `FarAttenuationEnd` 10, the SDK's far
    ///   attenuation (the names assimp's FBX document lists with the SDK's
    ///   defaults, FBXDocument.h L315-L317 at v6.0.5);
    /// - the perspective camera: `CameraProjectionType` 0; `ApertureMode` 3,
    ///   focal length, and `GateFit` 2, horizontal, as Blender writes
    ///   (export_fbx_bin.py L689-L690); a 35 mm film across its wider side,
    ///   `FilmWidth` 35/25.4 = 1.3779528 in and `FilmHeight` that over the
    ///   aspect 1.5, 0.9186352 in; the aspect as `AspectWidth` 1.5 over
    ///   `AspectHeight` 1 (`AspectRatioMode` 1, a fixed ratio); and
    ///   `FocalLength` 25.4 × 0.9186352 / (2·tan(0.4)) = 27.594261 mm, from
    ///   which ufbx computes the scene's yfov again (ufbx.c L23240-L23245);
    ///   `FieldOfView` the horizontal angle, 2·atan(1.5·tan(0.4)) =
    ///   64.764910°, as Blender writes it beside a horizontal gate fit
    ///   (export_fbx_bin.py L691); `NearPlane` 0.125 and `FarPlane` 64, in
    ///   the file's units, which are metres;
    /// - the orthographic camera: `CameraProjectionType` 1 and `OrthoZoom`
    ///   2·3/30 = 0.2, since ufbx takes 30 × OrthoZoom as the full width on
    ///   a horizontal gate fit (ufbx.c L21356, L23109, L23198-L23203), with
    ///   the aspect 3 : 2.
    #[test]
    fn lights_and_cameras_are_written_in_fbxs_own_units() {
        let bytes = write_prepared(&fixtures::staged());
        let dom = parse(&bytes).unwrap();
        let attributes = fbx_objects(&dom, "NodeAttribute");
        let classes: Vec<(&str, &str)> = attributes.iter().map(|a| (a.2.as_str(), a.3.as_str())).collect();
        for (name, class) in [("sun", "Light"), ("bulb", "Light"), ("spot", "Light"), ("persp_camera", "Camera"), ("ortho_camera", "Camera")] {
            assert!(classes.contains(&(name, class)), "no {class} attribute named {name:?} in {classes:?}");
        }
        let get = |name: &str| attributes.iter().find(|a| a.2 == name).map(|a| a.0).unwrap();
        let num = |name: &str, prop: &str| fbx_numbers(&dom, get(name), prop);
        let (s875, s75, s5) = (0.942904884492641, 0.8808250210902997, 0.7353569830524495);
        for (light, kind, color, intensity) in
            [("sun", 1.0, [1.0, s875, s75], 300.0), ("bulb", 0.0, [1.0, s75, s5], 2000.0), ("spot", 2.0, [s5, s75, 1.0], 4000.0)]
        {
            assert_numbers(num(light, "LightType"), &[kind], 0.0, &format!("{light}: LightType"));
            assert_numbers(num(light, "Color"), &color, 1e-6, &format!("{light}: Color, sRGB-encoded"));
            assert_numbers(num(light, "Intensity"), &[intensity], 0.0, &format!("{light}: Intensity, in percent"));
            assert_numbers(num(light, "DecayType"), &[2.0], 0.0, &format!("{light}: DecayType"));
        }
        assert_numbers(num("spot", "InnerAngle"), &[22.918312146742387], 1e-6, "the spot's inner cone, full, in degrees");
        assert_numbers(num("spot", "OuterAngle"), &[68.75493814777445], 1e-6, "the spot's outer cone, full, in degrees");
        assert_numbers(num("bulb", "EnableFarAttenuation"), &[1.0], 0.0, "the bulb's range is on");
        assert_numbers(num("bulb", "FarAttenuationEnd"), &[10.0], 0.0, "the bulb's range");
        assert!(num("sun", "EnableFarAttenuation").is_none_or(|v| v == [0.0]), "a directional light has no range");
        let persp = [
            ("CameraProjectionType", 0.0, 0.0),
            ("ApertureMode", 3.0, 0.0),
            ("GateFit", 2.0, 0.0),
            ("AspectRatioMode", 1.0, 0.0),
            ("AspectWidth", 1.5, 1e-12),
            ("AspectHeight", 1.0, 0.0),
            ("FilmWidth", 1.3779527559055118, 1e-12),
            ("FilmHeight", 0.9186351706036745, 1e-12),
            ("FocalLength", 27.59426110856516, 1e-9),
            ("FieldOfView", 64.76491019818114, 1e-9),
            ("NearPlane", 0.125, 0.0),
            ("FarPlane", 64.0, 0.0),
        ];
        for (prop, want, eps) in persp {
            assert_numbers(num("persp_camera", prop), &[want], eps, &format!("the perspective camera's {prop}"));
        }
        let ortho = [
            ("CameraProjectionType", 1.0, 0.0),
            ("OrthoZoom", 0.2, 1e-15),
            ("GateFit", 2.0, 0.0),
            ("AspectWidth", 1.5, 1e-12),
            ("AspectHeight", 1.0, 0.0),
            ("NearPlane", 0.0625, 0.0),
            ("FarPlane", 20.0, 0.0),
        ];
        for (prop, want, eps) in ortho {
            assert_numbers(num("ortho_camera", prop), &[want], eps, &format!("the orthographic camera's {prop}"));
        }
    }

    /// The axis test for cameras: FBX cameras look along their node's +X
    /// with +Y up (ufbx.h L1617-L1618 at v0.23.1, "FBX saves cameras with +X
    /// forward and +Y up"; Blender's `MAT_CONVERT_CAMERA`, fbx_utils.py
    /// L74), so a written camera model's own rotation, read straight off the
    /// wire before any reader turns it, takes +X to the way the scene's
    /// camera looks (its node's −Z) and +Y to its up. The axes are worked out
    /// by hand from `fixtures::staged()`, in the parent's space: the
    /// perspective camera at (0, 1.5, 4) faces (0, 0.5, 0), forward
    /// (0, −1, −4)/√17 and up (0, 4, −1)/√17; the orthographic one at
    /// (−3, 2.5, 3) faces it too, forward (3, −2, −3)/√22 and up
    /// (1, 3, −1)/√11.
    #[test]
    fn fbx_cameras_look_along_plus_x_on_the_wire() {
        let bytes = write_prepared(&fixtures::staged());
        let dom = parse(&bytes).unwrap();
        let (s17, s22, s11) = (17f64.sqrt(), 22f64.sqrt(), 11f64.sqrt());
        let cases = [
            ("persp_camera_node", [0.0, -1.0 / s17, -4.0 / s17], [0.0, 4.0 / s17, -1.0 / s17]),
            ("ortho_camera_node", [3.0 / s22, -2.0 / s22, -3.0 / s22], [1.0 / s11, 3.0 / s11, -1.0 / s11]),
        ];
        for (name, forward, up) in cases {
            let (rec, class, attribute) = fbx_model(&dom, name);
            assert_eq!((class.as_str(), attribute.as_deref()), ("Camera", Some("Camera")), "{name}: a Camera model holding a Camera attribute");
            assert_close3(wire_axis(&dom, rec, [1.0, 0.0, 0.0]), forward, 1e-9, &format!("{name}'s +X, on the wire"));
            assert_close3(wire_axis(&dom, rec, [0.0, 1.0, 0.0]), up, 1e-9, &format!("{name}'s +Y, on the wire"));
        }
    }

    /// The axis test for lights: FBX lights point along their node's −Y
    /// (ufbx.h L1439, "usually -Y", and ufbx.c L23726-L23731 sets every
    /// light's `local_direction` to (0, −1, 0); Blender's
    /// `MAT_CONVERT_LIGHT`, fbx_utils.py L73), so a written light model's
    /// own rotation takes −Y to the way the scene's light shines (its node's
    /// −Z). By hand, in the parent's space: the spot at (1.5, 2.5, 1.5) and
    /// the sun at (2, 4, 3) face (0, 0.5, 0), along (−3, −4, −3)/√34 and
    /// (−4, −7, −6)/√101. The bulb's node is unrotated, so its model's
    /// rotation is the turn alone, R_x(+90°): the Euler angles (90°, 0, 0).
    #[test]
    fn fbx_lights_point_along_minus_y_on_the_wire() {
        let bytes = write_prepared(&fixtures::staged());
        let dom = parse(&bytes).unwrap();
        let (s34, s101) = (34f64.sqrt(), 101f64.sqrt());
        for (name, along) in [("spot_node", [-3.0 / s34, -4.0 / s34, -3.0 / s34]), ("sun_node", [-4.0 / s101, -7.0 / s101, -6.0 / s101])] {
            let (rec, class, attribute) = fbx_model(&dom, name);
            assert_eq!((class.as_str(), attribute.as_deref()), ("Light", Some("Light")), "{name}: a Light model holding a Light attribute");
            assert_close3(wire_axis(&dom, rec, [0.0, -1.0, 0.0]), along, 1e-9, &format!("{name}'s −Y, on the wire"));
        }
        let (bulb, class, _) = fbx_model(&dom, "bulb_node");
        assert_eq!(class, "Light");
        assert_numbers(fbx_numbers(&dom, bulb, "Lcl Rotation"), &[90.0, 0.0, 0.0], 1e-9, "the bulb's rotation, the turn alone");
    }

    /// Review Focus 3: FBX states a camera's field of view in four ways,
    /// chosen by `ApertureMode`, and in each only one pair of properties
    /// holds it; the rest are left over from other modes. ufbx reads them so
    /// (ufbx.c L23221-L23248, v0.23.1), and so does this reader: 0,
    /// horizontal and vertical, `FieldOfViewY` is the vertical angle; 1,
    /// horizontal, `FieldOfView` is the horizontal angle, and
    /// tan(yfov/2) = tan(FieldOfView/2) / aspect; 2, vertical, `FieldOfView`
    /// is the vertical angle; 3, focal length, tan(yfov/2) = h /
    /// (2·FocalLength), h the film gate's height in mm (25.4 × inches):
    /// `FilmHeight` with no gate fit, or `FilmWidth` over the aspect with the
    /// gate fitted across, as Blender writes (`GateFit` 2,
    /// export_fbx_bin.py L690). Each case sets the other modes' properties
    /// to decoys. The angles, worked out by hand (Python's math,
    /// 2026-11-06): 40° = 0.6981317007977318; 2·atan(tan(30°)/1.5) =
    /// 0.7348438092396031; 35° = 0.6108652381980153; 2·atan(24/100) =
    /// 0.47108996144172666; 2·atan((36·9/16)/100) = 0.39959649248062956.
    #[test]
    fn fbx_aperture_modes_choose_the_right_fov() {
        let film = format!("{}{}", pn("FilmWidth", 36.0 / 25.4), pn("FilmHeight", 24.0 / 25.4));
        let ratio = |w: f64, h: f64, mode: i64| format!("{}{}{}", pe("AspectRatioMode", mode), pn("AspectWidth", w), pn("AspectHeight", h));
        let fov = |f: f64, x: f64, y: f64| format!("{}{}{}", pn("FieldOfView", f), pn("FieldOfViewX", x), pn("FieldOfViewY", y));
        let cases = [
            ("0, horizontal and vertical", format!("{}{}{}", pe("ApertureMode", 0), fov(50.0, 60.0, 40.0), ratio(1.5, 1.0, 1)), 0.6981317007977318, 1.5),
            ("1, horizontal", format!("{}{}{}", pe("ApertureMode", 1), fov(60.0, 70.0, 40.0), ratio(1.5, 1.0, 1)), 0.7348438092396031, 1.5),
            ("2, vertical", format!("{}{}{}", pe("ApertureMode", 2), fov(35.0, 60.0, 30.0), ratio(1.5, 1.0, 1)), 0.6108652381980153, 1.5),
            (
                "3, focal length",
                format!("{}{}{film}{}{}{}", pe("ApertureMode", 3), pn("FocalLength", 50.0), pe("GateFit", 0), fov(70.0, 80.0, 30.0), ratio(1.5, 1.0, 1)),
                0.47108996144172666,
                1.5,
            ),
            (
                "3, focal length, the gate fitted across (Blender's)",
                format!("{}{}{film}{}{}{}", pe("ApertureMode", 3), pn("FocalLength", 50.0), pe("GateFit", 2), fov(70.0, 80.0, 30.0), ratio(1920.0, 1080.0, 2)),
                0.39959649248062956,
                16.0 / 9.0,
            ),
        ];
        for (mode, props, yfov, aspect) in cases {
            let props = format!("{props}{}{}", pn("NearPlane", 0.1), pn("FarPlane", 100.0));
            let (objects, links) = light_or_camera(100, "eye", "Camera", &props);
            let r = read_scene(lit_doc("test", 100.0, &objects, &links).as_bytes(), &NoSidecars).unwrap_or_else(|e| panic!("{mode}: {e}"));
            assert_eq!(r.scene.cameras.len(), 1, "{mode}: {:?}", r.warnings);
            let Projection::Perspective { yfov: got, aspect: got_aspect, .. } = r.scene.cameras[0].projection else {
                panic!("{mode}: {:?}", r.scene.cameras[0])
            };
            assert!((f64::from(got) - yfov).abs() < 1e-6, "{mode}: yfov {got} against {yfov}");
            assert!(got_aspect.is_some_and(|a| (f64::from(a) - aspect).abs() < 1e-6), "{mode}: aspect {got_aspect:?} against {aspect}");
        }
    }

    /// `OrthoZoom` as ufbx reads it (v0.23.1): the view's full extent along
    /// its gate fit is 30 × OrthoZoom in the file's units (ufbx.c
    /// L21353-L21357, L23109; ufbx's own `maya_ortho_camera_size` test reads
    /// Maya's orthographic widths of 10 to 100 so), the other side following
    /// from the aspect ratio (ufbx.c L23179-L23219); except in a file
    /// Blender's exporter wrote (its `Creator`, "Blender … - …", ufbx.c
    /// L12084-L12089), where OrthoZoom is Blender's `ortho_scale`, the
    /// extent itself in metres, written unscaled (export_fbx_bin.py L702).
    /// xmag and ymag are half the width and height (glTF 2.0 §3.10.3), and
    /// every distance is in metres: a centimetre file's (`UnitScaleFactor`
    /// 1) are a hundredth of its numbers.
    #[test]
    fn an_orthographic_zoom_reads_as_ufbx_reads_it() {
        let maya = "FBX SDK/FBX Plugins version 2020.1";
        let blender = "Blender (stable FBX IO) - 5.2.3 - 5.13.0";
        // (what, creator, unit, gate fit, aspect width, height and mode, zoom,
        // near and far planes as written; xmag, ymag, znear and zfar in metres)
        let cases = [
            ("Maya's, fitted across", maya, 1.0, 2, (2.0, 1.0, 1), 0.2, (10.0, 4000.0), (0.03, 0.015, 0.1, 40.0)),
            ("Maya's, fitted to the height", maya, 1.0, 1, (2.0, 1.0, 1), 0.2, (10.0, 4000.0), (0.06, 0.03, 0.1, 40.0)),
            ("Blender's", blender, 100.0, 2, (1920.0, 1080.0, 2), 6.0, (0.1, 100.0), (3.0, 1.6875, 0.1, 100.0)),
        ];
        for (what, creator, unit, fit, (aw, ah, mode), zoom, (near, far), (xmag, ymag, znear, zfar)) in cases {
            let props = [
                pe("CameraProjectionType", 1),
                pe("GateFit", fit),
                pe("AspectRatioMode", mode),
                pn("AspectWidth", aw),
                pn("AspectHeight", ah),
                pn("OrthoZoom", zoom),
                pn("NearPlane", near),
                pn("FarPlane", far),
            ]
            .concat();
            let (objects, links) = light_or_camera(100, "plan", "Camera", &props);
            let r = read_scene(lit_doc(creator, unit, &objects, &links).as_bytes(), &NoSidecars).unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(r.scene.cameras.len(), 1, "{what}: {:?}", r.warnings);
            let got = r.scene.cameras[0].projection;
            let Projection::Orthographic { xmag: gx, ymag: gy, znear: gn, zfar: gf } = got else { panic!("{what}: {got:?}") };
            let close = |a: f32, b: f64| (f64::from(a) - b).abs() <= 1e-6 * b.abs().max(1.0);
            assert!(close(gx, xmag) && close(gy, ymag) && close(gn, znear) && close(gf, zfar), "{what}: {got:?}");
        }
    }

    /// The spec's "readers never refuse a file over a light or camera"
    /// (Rulings R5 and R8), FBX's half: a spot whose cones are out of order,
    /// a perspective camera whose `NearPlane` is 0, a light whose
    /// `Intensity` is infinite (MSVC's `1.#INF00`), an area light (FBX
    /// `LightType` 3, which the scene does not hold) and a light no model
    /// places are each dropped with one warning naming it and why. The file
    /// still reads, every model stays a node, and the good light and camera
    /// beside them are kept. A colour past 1 is not a bad light: its
    /// brightest component goes into the intensity first (R8), so, read as
    /// linear (`light_colours_follow_the_fbx_colour_space` has sRGB), (2, 1,
    /// 0.5) at intensity 1 reads as (1, 0.5, 0.25) at intensity 2, unwarned.
    #[test]
    fn a_bad_light_or_camera_is_dropped_not_refused() {
        let spot = |inner: f64, outer: f64| format!("{}{}{}", pe("LightType", 2), pn("InnerAngle", inner), pn("OuterAngle", outer));
        let camera = |near: f64| format!("{}{}{}{}", pe("ApertureMode", 2), pn("FieldOfView", 40.0), pn("NearPlane", near), pn("FarPlane", 100.0));
        let parts = [
            light_or_camera(100, "lamp", "Light", &spot(70.0, 60.0)),
            light_or_camera(110, "shot", "Camera", &camera(0.0)),
            light_or_camera(120, "flare", "Light", &format!("{}\t\t\tP: \"Intensity\", \"Number\", \"\", \"A\",1.#INF00\n", pe("LightType", 0))),
            light_or_camera(130, "panel", "Light", &pe("LightType", 3)),
            light_or_camera(140, "good", "Light", &format!("{}{}", spot(20.0, 60.0), p3("Color", [2.0, 1.0, 0.5]))),
            light_or_camera(150, "eye", "Camera", &camera(0.1)),
        ];
        let orphan = "\tNodeAttribute: 160, \"NodeAttribute::orphan\", \"Light\" {\n\t\tTypeFlags: \"Light\"\n\t}\n";
        let objects: String = parts.iter().map(|p| p.0.as_str()).chain([orphan]).collect();
        let links: String = parts.iter().map(|p| p.1.as_str()).collect();
        let doc = lit_doc("test", 100.0, &objects, &links);
        let r = read_scene_with(doc.as_bytes(), &NoSidecars, FbxColorSpace::Linear, LightUnits::default()).unwrap_or_else(|e| panic!("refused: {e}"));
        let want: [&[&str]; 5] = [
            &["light \"lamp\" is dropped: its inner cone 0.6108", "is not below its outer cone 0.5235"],
            &["camera \"shot\" is dropped: its znear 0 is not positive"],
            &["light \"flare\" is dropped: its intensity inf is not finite"],
            &["light \"panel\" is dropped:", "area", "LightType 3"],
            &["light \"orphan\" is placed by no model, so it is dropped"],
        ];
        assert_eq!(r.warnings.len(), want.len(), "{:#?}", r.warnings);
        for w in want {
            assert!(r.warnings.iter().any(|g| w.iter().all(|f| g.contains(f))), "no warning has {w:?}: {:#?}", r.warnings);
        }
        let names: Vec<Option<&str>> = r.scene.nodes.iter().map(|n| n.name.as_deref()).collect();
        let every = ["tri", "lamp_node", "shot_node", "flare_node", "panel_node", "good_node", "eye_node"].map(Some);
        assert_eq!(names, every, "every model stays a node");
        let placed: Vec<(Option<usize>, Option<usize>)> = r.scene.nodes.iter().map(|n| (n.light, n.camera)).collect();
        assert_eq!(placed, [(None, None), (None, None), (None, None), (None, None), (None, None), (Some(0), None), (None, Some(0))]);
        assert_eq!(r.scene.lights.len(), 1, "the good light is kept");
        let good = &r.scene.lights[0];
        assert_eq!((good.name.as_deref(), good.color, good.intensity), (Some("good"), [1.0, 0.5, 0.25], 2.0), "{good:?}");
        let eyes: Vec<Option<&str>> = r.scene.cameras.iter().map(|c| c.name.as_deref()).collect();
        assert_eq!(eyes, [Some("eye")], "the good camera is kept");
    }

    /// Ruling R14: a light's `Color` is stored in the caller's
    /// `FbxColorSpace`, as material colours are. Read as sRGB, the default
    /// and three.js's reading (`FBXLoader.js` L1196 at r183), it is decoded
    /// by the sRGB curve, past 1 without clamping as three.js's
    /// `SRGBToLinear` decodes it (ColorManagement.js L205-L209), so that
    /// Ruling R8 still moves a colour's brightness into the intensity. Read
    /// as linear (Blender's, ufbx's and assimp's reading) it is the scene's
    /// colour as it is. By hand (Python, 2026-11-06): sRGB 0.5 and 0.25 are
    /// 0.21404114048223255 and 0.05087608817155679 linear, and sRGB 2 is
    /// 4.953845751592042, so (2, 1, 0.5) at intensity 1 reads as
    /// (1, 0.20186337042864627, 0.04320706602813482) at 4.953845751592042;
    /// linear, it reads as (1, 0.5, 0.25) at 2.
    #[test]
    fn light_colours_follow_the_fbx_colour_space() {
        let lamp = |color: [f64; 3]| {
            let (objects, links) = light_or_camera(100, "lamp", "Light", &format!("{}{}", pe("LightType", 0), p3("Color", color)));
            lit_doc("test", 100.0, &objects, &links)
        };
        let cases = [
            ([0.5, 0.25, 1.0], FbxColorSpace::Srgb, [0.21404114048223255, 0.05087608817155679, 1.0], 1.0),
            ([0.5, 0.25, 1.0], FbxColorSpace::Linear, [0.5, 0.25, 1.0], 1.0),
            ([2.0, 1.0, 0.5], FbxColorSpace::Srgb, [1.0, 0.20186337042864627, 0.04320706602813482], 4.953845751592042),
            ([2.0, 1.0, 0.5], FbxColorSpace::Linear, [1.0, 0.5, 0.25], 2.0),
        ];
        for (stored, space, color, intensity) in cases {
            let ctx = format!("{stored:?} read as {space:?}");
            let r = read_scene_with(lamp(stored).as_bytes(), &NoSidecars, space, LightUnits::default()).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            assert!(r.warnings.is_empty(), "{ctx}: {:?}", r.warnings);
            assert_eq!(r.scene.lights.len(), 1, "{ctx}");
            let l = &r.scene.lights[0];
            assert_close3(l.color.map(f64::from), color, 1e-6, &ctx);
            assert!((f64::from(l.intensity) - intensity).abs() <= 1e-6 * intensity, "{ctx}: intensity {} against {intensity}", l.intensity);
        }
    }

    /// Ruling R15, the spec's non-goal "Ambient light: … FBX `AmbientColor`.
    /// Warned.": the scene holds no ambient light, so a file whose
    /// `GlobalSettings` gives a non-black `AmbientColor` is told so, with the
    /// colour as the file stores it. A black one, the usual default and what
    /// this writer writes, needs no word.
    #[test]
    fn a_non_black_ambient_colour_is_warned() {
        let ambient = |c: &str| {
            let doc = lit_doc("test", 100.0, "", "");
            let line = format!("\t\tP: \"AmbientColor\", \"ColorRGB\", \"Color\", \"\",{c}\n\t\tP: \"UnitScaleFactor\"");
            let doc = doc.replacen("\t\tP: \"UnitScaleFactor\"", &line, 1);
            assert!(doc.contains("AmbientColor"), "the document changed");
            doc
        };
        let r = read_scene(ambient("0.2,0.25,0.5").as_bytes(), &NoSidecars).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        let w = &r.warnings[0];
        assert!(w.contains("AmbientColor (0.2, 0.25, 0.5)") && w.contains("not carried"), "{w}");
        let r = read_scene(ambient("0,0,0").as_bytes(), &NoSidecars).unwrap_or_else(|e| panic!("{e}"));
        assert!(r.warnings.is_empty(), "black needs no warning: {:?}", r.warnings);
    }

    /// Review Focus 4 through the real codec (Ruling R5): an FBX spot whose
    /// `InnerAngle` equals its `OuterAngle`, a hard edge as FBX files carry
    /// routinely, is softened with one warning, not dropped
    /// (`Light::soften_hard_edge`: the inner cone moves to 1 − 1e-4 of the
    /// outer). Written by the FBX writer and read back by the FBX reader, the
    /// softened spot comes back as it is, with no warning: the gap outlasts
    /// FBX's full-cone degrees, so it is softened once only.
    #[test]
    fn a_hard_edged_fbx_spot_is_softened_once() {
        let props = format!("{}{}{}{}", pe("LightType", 2), pn("InnerAngle", 60.0), pn("OuterAngle", 60.0), pn("Intensity", 4000.0));
        let (objects, links) = light_or_camera(100, "stage_spot", "Light", &props);
        let r = read_scene(lit_doc("test", 100.0, &objects, &links).as_bytes(), &NoSidecars).expect("a light never refuses the file");
        assert_eq!(r.scene.lights.len(), 1, "softened, not dropped: {:?}", r.warnings);
        let LightKind::Spot { inner_cone, outer_cone } = r.scene.lights[0].kind else { panic!("{:?}", r.scene.lights[0]) };
        assert!((f64::from(outer_cone) - 30f64.to_radians()).abs() < 1e-6, "the outer cone is half of the full 60°: {outer_cone}");
        let gap = (outer_cone - inner_cone) / outer_cone;
        assert!((0.99e-4..=1.01e-4).contains(&gap), "inner cone {inner_cone}: softened by {gap} of the outer cone");
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        let w = &r.warnings[0];
        assert!(w.starts_with("light \"stage_spot\" ") && w.contains("hard edge") && w.contains("softened"), "{w}");
        let back = read_back(&write_prepared(&r.scene));
        assert_eq!(back.scene.lights, r.scene.lights, "the softened spot comes back as it is");
        assert!(back.warnings.is_empty(), "softened once only: {:?}", back.warnings);
    }

    /// Ruling R32 in FBX: a spot whose `OuterAngle`, its full cone in
    /// degrees, is past 180° is wider than a hemisphere. It lights nearly
    /// every way, so it is read as a point light, with one warning naming
    /// it, not dropped. One of exactly 180°, a hemisphere, stays a spot.
    #[test]
    fn an_fbx_spot_wider_than_a_hemisphere_reads_as_a_point_light() {
        let spot = |outer: f64| format!("{}{}{}", pe("LightType", 2), pn("InnerAngle", 30.0), pn("OuterAngle", outer));
        let parts = [light_or_camera(100, "flood", "Light", &spot(200.0)), light_or_camera(110, "edge", "Light", &spot(180.0))];
        let objects: String = parts.iter().map(|p| p.0.as_str()).collect();
        let links: String = parts.iter().map(|p| p.1.as_str()).collect();
        let r = read_scene(lit_doc("test", 100.0, &objects, &links).as_bytes(), &NoSidecars).expect("a light never refuses the file");
        let kinds: Vec<(Option<&str>, LightKind)> = r.scene.lights.iter().map(|l| (l.name.as_deref(), l.kind)).collect();
        assert_eq!(kinds.first(), Some(&(Some("flood"), LightKind::Point)), "{:?}", r.warnings);
        assert!(matches!(kinds.get(1), Some((Some("edge"), LightKind::Spot { .. }))), "180° stays a spot: {kinds:?}");
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert!(r.warnings[0].starts_with("light \"flood\" has a cone of 200° across") && r.warnings[0].contains("point light"), "{:?}", r.warnings);
    }

    /// A model may hold several light and camera attributes, of which the
    /// reader takes the first light and the first camera. Each model that
    /// does is named in a warning of its own: one model's warning does not
    /// stand for the next, since the warning is keyed by the model.
    #[test]
    fn each_model_with_extra_light_attributes_is_named() {
        let lamp = pe("LightType", 0);
        let (one, one_links) = light_or_camera(100, "one", "Light", &lamp);
        let (two, two_links) = light_or_camera(110, "two", "Light", &lamp);
        let extra = |id: i64, name: &str| {
            format!("\tNodeAttribute: {id}, \"NodeAttribute::{name}\", \"Light\" {{\n\t\tProperties70:  {{\n{lamp}\t\t}}\n\t\tTypeFlags: \"Light\"\n\t}}\n")
        };
        // A second light on each model: 120 on one_node (101), 130 on two_node (111).
        let objects = format!("{one}{two}{}{}", extra(120, "one_extra"), extra(130, "two_extra"));
        let links = format!("{one_links}{two_links}\tC: \"OO\",120,101\n\tC: \"OO\",130,111\n");
        let r = read_scene(lit_doc("test", 100.0, &objects, &links).as_bytes(), &NoSidecars).expect("a light never refuses the file");
        let named: Vec<&String> = r.warnings.iter().filter(|w| w.contains("light and camera attributes")).collect();
        assert_eq!(named.len(), 2, "one warning for each model: {:?}", r.warnings);
        for (w, model) in named.iter().zip(["one_node", "two_node"]) {
            assert!(w.starts_with(&format!("model \"{model}\" has 2 light and camera attributes")), "{w}");
        }
        assert_eq!(r.scene.lights.len(), 2, "each model's first light: {:?}", r.warnings);
    }

    /// Ruling R19 through FBX: `pose::bake_shear` moves a camera out of a
    /// sheared static run onto a new node that takes its old node's name,
    /// and renames the old node, which now holds nothing, `<name>_shear`.
    /// FBX names must be unique, so had both kept the name, the writer
    /// would have renamed the camera's node (`eye_node_1`, as GLTFLoader's
    /// `createUniqueName` renames a repeat) and a reader binding by name
    /// would miss it. Read back, the camera is on `eye_node`, at its eye and
    /// facing as before.
    #[test]
    fn a_camera_moved_out_of_a_shear_keeps_its_node_name_through_fbx() {
        let mut s = Scene::default();
        let tri = Primitive { positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], indices: vec![0, 1, 2], ..Default::default() };
        s.meshes.push(SceneMesh { primitives: vec![tri], ..Default::default() });
        s.cameras.push(Camera { name: Some("eye".into()), projection: Projection::Perspective { yfov: 0.8, aspect: Some(1.5), znear: 0.1, zfar: Some(100.0) } });
        // Its Z axis leans half a unit toward +X, which no TRS holds.
        let shear = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.5, 0.0, 1.0, 0.0, 1.0, 2.0, 3.0, 1.0];
        let placed = Transform::Trs { translation: [0.5, 1.0, 4.0], rotation: math::quat_from_axis_angle([0.0, 1.0, 0.0], 0.5), scale: [1.0; 3] };
        s.nodes = vec![
            Node { name: Some("tri".into()), mesh: Some(0), ..Default::default() },
            Node { name: Some("lean".into()), transform: Transform::Matrix(shear), children: vec![2], ..Default::default() },
            Node { name: Some("eye_node".into()), transform: placed, camera: Some(0), ..Default::default() },
        ];
        s.roots = vec![0, 1];
        s.validate().unwrap();
        let back = read_back(&write_prepared(&s));
        let names: Vec<(Option<&str>, bool)> = back.scene.nodes.iter().map(|n| (n.name.as_deref(), n.camera.is_some())).collect();
        assert!(names.contains(&(Some("eye_node"), true)), "the camera stands on eye_node: {names:?}");
        assert!(names.contains(&(Some("eye_node_shear"), false)), "the old node, renamed: {names:?}");
        let ((_, want), (_, got)) = (lightcam::frames(&s, None), lightcam::frames(&back.scene, None));
        assert_close3(got[0].eye, want[0].eye, 1e-5, "the eye");
        assert_close3(got[0].forward, want[0].forward, 1e-5, "the way it faces");
    }

    /// A camera's or light's model carries FBX's fixed turn (R_y(+90°) or
    /// R_x(+90°)) in its own rotation, and its children must not turn with
    /// it: the writer gives each child the inverse turn first, and the reader
    /// takes both off again, as Blender's importer does (the turn is the
    /// light or camera node's `post_matrix`, its inverse each child's
    /// `pre_matrix`: import_fbx.py L2372-L2380, L2462-L2478). A triangle on a
    /// child of the perspective camera's node and one on a child of the
    /// spot's, each node scaled unevenly so that the turn must swap the
    /// scale's axes too, stand where the scene puts them: on the wire, before
    /// any reader, the marker's FBX world matrix (its model's local under its
    /// holder's and the rig's) is the scene's, within 1e-9; and after an FBX
    /// round trip, at rest and at every sample time, within 1e-5, with the
    /// camera and the spot standing and facing as before.
    #[test]
    fn a_child_of_a_camera_node_keeps_its_place() {
        let mut s = fixtures::staged();
        let marker = s.meshes.len();
        let triangle = Primitive { positions: vec![[0.0; 3], [0.25, 0.0, 0.0], [0.0, 0.5, 0.0]], indices: vec![0, 1, 2], ..Default::default() };
        s.meshes.push(SceneMesh { name: Some("marker".into()), primitives: vec![triangle], ..Default::default() });
        for (holder, child) in [("persp_camera_node", "camera_marker"), ("spot_node", "spot_marker")] {
            let h = node_named(&s, holder);
            let Transform::Trs { scale, .. } = &mut s.nodes[h].transform else { panic!("{holder} has a matrix") };
            *scale = [1.0, 2.0, 0.5];
            let c = s.nodes.len();
            let rotation = math::quat_from_axis_angle([1.0, 1.0, 0.0], 0.5);
            let transform = Transform::Trs { translation: [0.25, -0.5, -1.0], rotation, scale: [1.0, 1.0, 2.0] };
            s.nodes.push(Node { name: Some(child.into()), transform, mesh: Some(marker), ..Default::default() });
            s.nodes[h].children.push(c);
        }
        s.validate().unwrap();
        let bytes = write_prepared(&s);
        let dom = parse(&bytes).unwrap();
        let world = s.world_matrices();
        for (holder, child) in [("persp_camera_node", "camera_marker"), ("spot_node", "spot_marker")] {
            let chain = ["rig", holder, child].map(|name| wire_local(&dom, fbx_model(&dom, name).0));
            let m = chain.iter().fold(math::IDENTITY, |m, local| math::mat_mul(&m, local));
            let want = world[node_named(&s, child)];
            assert!((0..16).all(|k| (m[k] - want[k]).abs() < 1e-9), "{child}'s world on the wire {m:?} against the scene's {want:?}");
        }
        let back = read_back(&bytes);
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        let got = &back.scene;
        assert_same_geometry(&s, &pose::default_pose(&s), got, &pose::default_pose(got), 1e-5, "at rest");
        for t in fixtures::sample_times(&s) {
            assert_same_geometry(&s, &pose::sample_pose(&s, 0, t), got, &pose::sample_pose(got, 0, t), 1e-5, &format!("at {t} s"));
        }
        lightcam::assert_same_lights_and_cameras(&s, got, 1e-5, "the camera and the spot that hold them");
    }

    /// An animated camera node takes FBX's turn at every key: the writer
    /// turns every sampled rotation by R_y(+90°) on the right, as Blender's
    /// exporter does each animated frame through `fbx_object_matrix`
    /// (fbx_utils.py L1786-L1789), and the reader takes it off every key.
    /// The perspective camera of `fixtures::staged()` pans from R_y(−150°)
    /// to R_y(−30°) over the clip while sliding from (0, 1.5, 4) to (1, 2, 3);
    /// in FBX's frame it turns from R_y(−60°) to R_y(60°), one Euler angle
    /// well away from gimbal lock, so FBX's per-angle interpolation follows
    /// the scene's slerp exactly. On the wire, the camera model's rotation
    /// curves hold the turned keys, the Euler angles (0, ψ + 90°, 0): −60°,
    /// 0° and 60° at the scene's keys. At rest and at every sample time it
    /// stands, faces and projects as the scene's does, within 1e-5.
    #[test]
    fn an_animated_camera_node_keeps_its_aim() {
        let mut s = fixtures::staged();
        let cam = node_named(&s, "persp_camera_node");
        let yaw = |deg: f64| math::quat_from_axis_angle([0.0, 1.0, 0.0], deg.to_radians());
        s.nodes[cam].transform = Transform::Trs { translation: [0.0, 1.5, 4.0], rotation: yaw(-150.0), scale: [1.0; 3] };
        let rotation: Vec<f32> = [-150.0, -90.0, -30.0].iter().flat_map(|d| yaw(*d)).map(|v| v as f32).collect();
        let channels = &mut s.animations[0].channels;
        channels.push(Channel { node: cam, path: AnimPath::Rotation, interpolation: Interpolation::Linear, times: vec![0.0, 1.0, 2.0], values: rotation });
        let slide = vec![0.0, 1.5, 4.0, 1.0, 2.0, 3.0];
        channels.push(Channel { node: cam, path: AnimPath::Translation, interpolation: Interpolation::Linear, times: vec![0.0, 2.0], values: slide });
        s.validate().unwrap();
        let bytes = write_prepared(&s);
        let curves = wire_curves(&parse(&bytes).unwrap(), "persp_camera_node", "Lcl Rotation");
        for (seconds, yaw) in [(0.0f64, -60.0f64), (1.0, 0.0), (2.0, 60.0)] {
            let tick = (seconds * TICKS_PER_SECOND).round() as i64;
            for (axis, want) in [(0, 0.0), (1, yaw), (2, 0.0)] {
                let (times, values) = &curves[axis];
                let at = times.iter().position(|t| *t == tick).unwrap_or_else(|| panic!("no key at {seconds} s"));
                assert!((f64::from(values[at]) - want).abs() < 1e-4, "axis {axis} at {seconds} s: {} against {want}", values[at]);
            }
        }
        let back = read_back(&bytes);
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        lightcam::assert_same_lights_and_cameras(&s, &back.scene, 1e-5, "the panning camera");
    }

    /// What FBX cannot hold of a camera is written as the nearest thing it
    /// can, named once among the losses as a cameras loss, and `prepare`
    /// says what will read back (the task 4 report's rulings): a perspective
    /// camera with no aspect ratio (glTF's "the viewport's") is written
    /// 16:9, Blender's default render size (1920 × 1080, DNA_scene_types.h
    /// L871 and L875 at v5.2.3); one with no far plane (glTF's infinite) gets
    /// 1000 m, Blender's default clip end (DNA_camera_types.h L218), or
    /// 10,000 × its near plane when that is already at or past 1000 m,
    /// Blender's default ratio of the two; and an orthographic camera with a
    /// negative xmag or ymag, which mirrors the view, is written unmirrored,
    /// FBX storing the view's size and not its sign.
    #[test]
    fn what_fbx_cannot_hold_of_a_camera_is_set_and_reported() {
        let mut s = fixtures::staged();
        s.cameras[0].projection = Projection::Perspective { yfov: 0.8, aspect: None, znear: 0.125, zfar: None };
        s.cameras[1].projection = Projection::Orthographic { xmag: -3.0, ymag: 2.0, znear: 0.0625, zfar: 20.0 };
        s.cameras.push(Camera { name: Some("far_camera".into()), projection: Projection::Perspective { yfov: 0.5, aspect: Some(2.0), znear: 2000.0, zfar: None } });
        s.add_root(Node { name: Some("far_camera_node".into()), camera: Some(2), ..Default::default() });
        s.validate().unwrap();
        let w = super::super::write_scene(&s, MeshFormat::Fbx, WriteOptions::default()).unwrap();
        let named: Vec<&Loss> = w.losses.iter().filter(|l| l.kind == LossKind::Cameras).collect();
        assert_eq!(named.len(), 3, "{:#?}", w.losses);
        for (what, cameras, why) in [
            ("aspect ratio", "[persp_camera]", "16:9"),
            ("far plane", "[persp_camera, far_camera]", "1000"),
            ("mirroring", "[ortho_camera]", "unmirrored"),
        ] {
            let found = named.iter().any(|l| l.what.contains(what) && l.what.contains(cameras) && l.why.contains(why));
            assert!(found, "no cameras loss names the {what} of {cameras} with {why:?}: {named:#?}");
        }
        let prepared = prepare(&s, WriteOptions::default()).unwrap();
        let projections: Vec<Projection> = prepared.cameras.iter().map(|c| c.projection).collect();
        let want = [
            Projection::Perspective { yfov: 0.8, aspect: Some(16.0 / 9.0), znear: 0.125, zfar: Some(1000.0) },
            Projection::Orthographic { xmag: 3.0, ymag: 2.0, znear: 0.0625, zfar: 20.0 },
            Projection::Perspective { yfov: 0.5, aspect: Some(2.0), znear: 2000.0, zfar: Some(2.0e7) },
        ];
        assert_eq!(projections, want, "what prepare says will read back");
        let back = read_back(&w.package.files[0].bytes);
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        lightcam::assert_same_lights_and_cameras(&prepared, &back.scene, 1e-5, "read back as prepared");
    }

    /// A node holding a mesh and a light or camera, or a light and a camera,
    /// or a skin joint holding either, cannot be one FBX model: a model is
    /// one thing, its class (`Mesh`, `LimbNode`, `Light`, `Camera`, the
    /// class Blender's importer keys the light and camera turns on,
    /// import_fbx.py L2280, L2465-L2468), and FBX's turn would swing its
    /// mesh, its joint or its other attribute. So `prepare` gives each light
    /// and camera that shares a node an untransformed child of its own, named
    /// for the node, and the round trip keeps where everything stands: the
    /// staged cube's node with a lamp and an eye of its own, at rest and at
    /// every sample time, and the skinned fixture's knee with a lamp, at
    /// rest (its clip turns joints on several axes, which FBX's resampling
    /// follows only to its own tolerance).
    #[test]
    fn a_light_or_camera_sharing_a_node_gets_a_child_of_its_own() {
        let mut s = fixtures::staged();
        let cube = node_named(&s, "cube_node");
        s.lights.push(Light { name: Some("cube_lamp".into()), kind: LightKind::Point, color: [1.0; 3], intensity: 5.0, range: None });
        s.cameras.push(Camera { name: Some("cube_eye".into()), projection: Projection::Perspective { yfov: 0.6, aspect: Some(1.0), znear: 0.25, zfar: Some(8.0) } });
        (s.nodes[cube].light, s.nodes[cube].camera) = (Some(3), Some(2));
        s.validate().unwrap();
        let prepared = prepare(&s, WriteOptions::default()).unwrap();
        let names = |p: &Scene| p.nodes.iter().filter_map(|n| n.name.clone()).collect::<Vec<String>>();
        let children = ["cube_node_light".to_string(), "cube_node_camera".to_string()];
        assert!(children.iter().all(|c| names(&prepared).contains(c)), "{children:?} among {:?}", names(&prepared));
        let held = |p: &Scene, name: &str| {
            let n = &p.nodes[node_named(p, name)];
            (n.mesh.is_some(), n.light, n.camera, n.transform.is_identity())
        };
        let (mesh, light, camera, _) = held(&prepared, "cube_node");
        assert!(mesh && light.is_none() && camera.is_none(), "the cube keeps its mesh and holds no light or camera");
        assert_eq!(held(&prepared, "cube_node_light"), (false, Some(3), None, true), "the lamp, on an untransformed child");
        assert_eq!(held(&prepared, "cube_node_camera"), (false, None, Some(2), true), "the eye, on another");
        let back = read_back(&write_prepared(&s));
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        assert_same_geometry(&prepared, &pose::default_pose(&prepared), &back.scene, &pose::default_pose(&back.scene), 1e-5, "the cube stays");
        lightcam::assert_same_lights_and_cameras(&prepared, &back.scene, 1e-5, "the cube's lamp and eye");

        let mut k = fixtures::skinned();
        let knee = node_named(&k, "knee");
        k.lights.push(Light { name: Some("knee_lamp".into()), kind: LightKind::Spot { inner_cone: 0.2, outer_cone: 0.6 }, color: [1.0; 3], intensity: 9.0, range: Some(4.0) });
        k.nodes[knee].light = Some(0);
        k.validate().unwrap();
        let prepared = prepare(&k, WriteOptions::default()).unwrap();
        assert!(names(&prepared).contains(&"knee_light".to_string()), "{:?}", names(&prepared));
        assert_eq!(held(&prepared, "knee").1, None, "the joint holds no light");
        assert_eq!(held(&prepared, "knee_light"), (false, Some(0), None, true), "its lamp, on an untransformed child");
        let back = read_back(&write_prepared(&k));
        let (want, got) = (lightcam::frames(&prepared, None).0, lightcam::frames(&back.scene, None).0);
        assert_eq!(got.len(), 1, "{:?}", back.warnings);
        assert_close3(got[0].position, want[0].position, 1e-5, "the knee's lamp stands");
        assert_close3(got[0].direction, want[0].direction, 1e-5, "and shines");
        assert_eq!(got[0].light, want[0].light, "as it was");
    }

    /// The reader's side of the same rule, for files that do it anyway: a
    /// model holding a mesh, a light and a camera keeps its mesh where it is,
    /// and its light and camera each go on a child node of their own that
    /// carries FBX's turn (`tri_light`, `tri_camera`); a Light model with a
    /// camera attribute too keeps its light and gives the camera a child.
    /// Each faces as the file says: a light along its model's −Y, a camera
    /// along +X with +Y up. By hand: the triangle's model turned 30° about Z
    /// shines along (sin 30°, −cos 30°, 0) and looks along (cos 30°,
    /// sin 30°, 0), up (−sin 30°, cos 30°, 0); the rig turned 45° about X
    /// shines along (0, −cos 45°, −sin 45°) and looks along +X, up
    /// (0, cos 45°, sin 45°).
    #[test]
    fn a_model_holding_a_mesh_and_a_light_keeps_both() {
        let light = format!("{}{}", pe("LightType", 0), pn("Intensity", 500.0));
        let camera = format!("{}{}{}{}", pe("ApertureMode", 2), pn("FieldOfView", 40.0), pn("NearPlane", 0.1), pn("FarPlane", 100.0));
        let attribute = |id: i64, name: &str, class: &str, props: &str| {
            format!("\tNodeAttribute: {id}, \"NodeAttribute::{name}\", \"{class}\" {{\n\t\tProperties70:  {{\n{props}\t\t}}\n\t}}\n")
        };
        let rig = "\tModel: 300, \"Model::rig\", \"Light\" {\n\t\tProperties70:  {\n\t\t\tP: \"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\",45,0,0\n\t\t}\n\t}\n";
        let objects = [attribute(200, "lamp", "Light", &light), attribute(210, "eye", "Camera", &camera), attribute(220, "rig_lamp", "Light", &light), attribute(230, "rig_eye", "Camera", &camera), rig.to_string()].concat();
        let links = "\tC: \"OO\",200,10\n\tC: \"OO\",210,10\n\tC: \"OO\",300,0\n\tC: \"OO\",220,300\n\tC: \"OO\",230,300\n";
        let doc = lit_doc("test", 100.0, &objects, links).replacen(
            "\tModel: 10, \"Model::tri\", \"Mesh\" {\n\t}",
            "\tModel: 10, \"Model::tri\", \"Mesh\" {\n\t\tProperties70:  {\n\t\t\tP: \"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\",0,0,30\n\t\t}\n\t}",
            1,
        );
        assert!(doc.contains("A\",0,0,30"), "the document changed");
        let r = read_scene(doc.as_bytes(), &NoSidecars).unwrap_or_else(|e| panic!("{e}"));
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let tri = &r.scene.nodes[node_named(&r.scene, "tri")];
        assert_eq!((tri.mesh.is_some(), tri.light, tri.camera), (true, None, None), "the triangle's node keeps its mesh alone");
        let (h, c) = (0.5f64, 0.75f64.sqrt());
        let world = pose::world_primitives(&r.scene, &pose::default_pose(&r.scene)).unwrap();
        let corners: Vec<[f64; 3]> = world[0].positions.iter().map(|p| p.map(f64::from)).collect();
        for (got, want) in corners.iter().zip([[0.0; 3], [c, h, 0.0], [-h, c, 0.0]]) {
            assert_close3(*got, want, 1e-6, "the triangle turns 30° about Z, unturned by FBX's light and camera turns");
        }
        let (lights, cameras) = lightcam::frames(&r.scene, None);
        let lit: Vec<(&str, [f64; 3])> = lights.iter().map(|f| (f.node.as_str(), f.direction)).collect();
        assert_eq!(lit.iter().map(|l| l.0).collect::<Vec<_>>(), ["rig", "tri_light"], "where the lights stand");
        let r45 = 0.5f64.sqrt();
        assert_close3(lit[0].1, [0.0, -r45, -r45], 1e-6, "the rig's light, along its −Y");
        assert_close3(lit[1].1, [h, -c, 0.0], 1e-6, "the triangle's light, along its model's −Y");
        let seen: Vec<&str> = cameras.iter().map(|f| f.node.as_str()).collect();
        assert_eq!(seen, ["rig_camera", "tri_camera"], "where the cameras stand");
        assert_close3(cameras[0].forward, [1.0, 0.0, 0.0], 1e-6, "the rig's camera, along its +X");
        assert_close3(cameras[0].up, [0.0, r45, r45], 1e-6, "its up, +Y");
        assert_close3(cameras[1].forward, [c, h, 0.0], 1e-6, "the triangle's camera, along its model's +X");
        assert_close3(cameras[1].up, [-h, c, 0.0], 1e-6, "its up, +Y");
    }

    /// The light and camera reader through the overview's hostile-input bar
    /// ([`layer_sweep`]): `fixtures::staged()` as FBX, read whole, where it
    /// must carry its three lights and two cameras, then swept with every
    /// bit from its first camera attribute to its first geometry's vertices
    /// among the single flips: the writer writes every node's model and
    /// attribute before any geometry, so that span holds every light and
    /// camera `NodeAttribute`. Every reading is `Ok` or `Err`, never a panic.
    #[test]
    fn fbx_light_and_camera_inputs_survive_truncation_and_bit_flips() {
        let name = "staged as FBX";
        let file = write_prepared(&fixtures::staged());
        let whole = read_scene(&file, &NoSidecars).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((whole.scene.lights.len(), whole.scene.cameras.len()), (3, 2), "{name} itself carries every light and camera");
        let attributes = span(name, &file, "persp_camera\u{0}\u{1}NodeAttribute", "Vertices");
        layer_sweep(name, &file, attributes, |bytes| {
            let _ = read_scene(bytes, &NoSidecars);
        });
    }

    /// FNV-1a 64, to pin a whole file's bytes.
    fn fnv1a64(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf29ce484222325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100000001b3))
    }

    /// The light-units option's default is today's output, byte for byte,
    /// pinned as far as it can hold on every platform (Ruling L7):
    /// - `fixtures::cube()` (no lights, and no rotation, so no libm trig):
    ///   its whole FBX, written with `WriteOptions::default()` and with an
    ///   explicit `LightUnits::Photometric`, hashes to the FNV-1a 64 pinned
    ///   here;
    /// - `fixtures::staged()` (three lights): its whole file goes through
    ///   libm trig (`atan2` and `asin` in the node transforms' Euler angles,
    ///   `tan` and `atan` in the cameras), which rounds the last bit
    ///   differently on macOS and on Linux (CI), so the whole file is not
    ///   pinned. Each light's record is: its `LightType`, `Color`,
    ///   `Intensity`, `DecayType`, cones and range, which only basic
    ///   arithmetic (and `powf` rounded to f32 for the sRGB colour) produces,
    ///   hashed per light. The whole files written by the default options
    ///   and by an explicit `Photometric` are compared with each other in the
    ///   same run.
    ///
    /// The constants were computed on 2026-11-08 from `Photometric` output
    /// whose whole-file hash still matched the one pinned before the option
    /// existed (the writer as item 10 left it: `staged` 0xff748b849edfc5a0).
    #[test]
    fn fbx_photometric_bytes_are_unchanged() {
        const CUBE_FNV1A: u64 = 0x32e00d4949e94114;
        const STAGED_LIGHTS_FNV1A: [(&str, u64); 3] = [("bulb", 0x310bac52d0c576d7), ("spot", 0x4680191ca939101e), ("sun", 0x5340665688080120)];
        let cube = fixtures::cube();
        let want = format!("cube: {CUBE_FNV1A:#018x}");
        assert_eq!(format!("cube: {:#018x}", fnv1a64(&write_prepared(&cube))), want, "with the default options");
        assert_eq!(format!("cube: {:#018x}", fnv1a64(&write_units(&cube, LightUnits::Photometric))), want, "with an explicit Photometric");
        let staged = fixtures::staged();
        let default = write_prepared(&staged);
        assert_eq!(default, write_units(&staged, LightUnits::Photometric), "staged: the default is Photometric, byte for byte");
        let mut records = light_record_fnv1a(&default);
        records.sort();
        let got: Vec<String> = records.iter().map(|(name, hash)| format!("{name}: {hash:#018x}")).collect();
        let want: Vec<String> = STAGED_LIGHTS_FNV1A.iter().map(|(name, hash)| format!("{name}: {hash:#018x}")).collect();
        assert_eq!(got, want, "staged's light records");
    }

    /// Each light attribute's name and the FNV-1a 64 of its property values
    /// (`LightType` to `FarAttenuationEnd`, each value's f64 bits), which
    /// [`fbx_photometric_bytes_are_unchanged`] pins.
    fn light_record_fnv1a(bytes: &[u8]) -> Vec<(String, u64)> {
        const PROPS: [&str; 10] = [
            "LightType",
            "CastLight",
            "Color",
            "Intensity",
            "DecayType",
            "InnerAngle",
            "OuterAngle",
            "EnableFarAttenuation",
            "FarAttenuationStart",
            "FarAttenuationEnd",
        ];
        let dom = parse(bytes).unwrap();
        fbx_objects(&dom, "NodeAttribute")
            .into_iter()
            .filter(|a| a.3 == "Light")
            .map(|(rec, _, name, _)| {
                let mut stream = Vec::new();
                for prop in PROPS {
                    stream.extend_from_slice(prop.as_bytes());
                    for v in fbx_numbers(&dom, rec, prop).unwrap_or_default() {
                        stream.extend_from_slice(&v.to_bits().to_le_bytes());
                    }
                    stream.push(0xff);
                }
                (name, fnv1a64(&stream))
            })
            .collect()
    }

    /// The option's two names, as a caller spells them, and its default.
    #[test]
    fn light_units_are_named_as_callers_spell_them() {
        assert_eq!((LightUnits::Photometric.as_str(), LightUnits::Blender.as_str()), ("photometric", "blender"));
        assert_eq!(LightUnits::default(), LightUnits::Photometric);
        let (write, read) = (WriteOptions::default(), super::super::ReadOptions::default());
        assert_eq!((write.fbx_light_units, write.usd_light_units), (LightUnits::Photometric, LightUnits::Photometric));
        assert_eq!((read.fbx_light_units, read.usd_light_units), (LightUnits::Photometric, LightUnits::Photometric));
    }

    /// `scene` as FBX with its lights in `units` (and the FBX colour space
    /// the default).
    fn write_units(scene: &Scene, units: LightUnits) -> Vec<u8> {
        let opts = WriteOptions { fbx_light_units: units, ..Default::default() };
        super::super::write_scene(scene, MeshFormat::Fbx, opts).unwrap_or_else(|e| panic!("write: {e}")).package.files.remove(0).bytes
    }

    /// `bytes` read with its lights in `units`.
    fn read_units(bytes: &[u8], units: LightUnits) -> SceneRead {
        read_scene_with(bytes, &NoSidecars, FbxColorSpace::default(), units).unwrap_or_else(|e| panic!("read: {e}"))
    }

    /// Each light attribute's name and `Intensity`, as the file holds them.
    fn light_intensities(bytes: &[u8]) -> Vec<(String, f64)> {
        let dom = parse(bytes).unwrap();
        fbx_objects(&dom, "NodeAttribute")
            .into_iter()
            .filter(|a| a.3 == "Light")
            .map(|a| (a.2, fbx_numbers(&dom, a.0, "Intensity").unwrap_or_else(|| panic!("no Intensity")).remove(0)))
            .collect()
    }

    /// `got` against `want`, within `rel` of `want`.
    fn assert_close_rel(got: f64, want: f64, rel: f64, ctx: &str) {
        assert!((got - want).abs() <= rel * want.abs(), "{ctx}: {got} against {want}");
    }

    /// `LightUnits::Blender` writes each light's `Intensity` as 100 × the
    /// `energy` Blender's glTF importer gives it (`scene::blender_energy`),
    /// and nothing else about the light moves. `staged`'s sun (3 lux), bulb
    /// (20 cd) and spot (40 cd): Task 1's note gives E = 0.004392386530,
    /// 0.367975713451 and 0.735951426902, so `Intensity` = 0.439238653001,
    /// 36.797571345122 and 73.595142690244 (against 300, 2000 and 4000
    /// photometric). The two files differ only in those three numbers (the
    /// last assertion makes them equal by copying the three f64s across).
    #[test]
    fn fbx_blender_units_write_blender_energy() {
        let staged = fixtures::staged();
        let photometric = write_units(&staged, LightUnits::Photometric);
        let blender = write_units(&staged, LightUnits::Blender);
        let got = light_intensities(&blender);
        assert_eq!(got.len(), 3, "{got:?}");
        let pinned = [("sun", 0.439238653001), ("bulb", 36.797571345122), ("spot", 73.595142690244)];
        for light in &staged.lights {
            let name = light.name.clone().unwrap();
            let written = got.iter().find(|g| g.0 == name).unwrap_or_else(|| panic!("no light {name}: {got:?}")).1;
            let energy = super::super::scene::blender_energy(&light.kind, f64::from(light.intensity));
            assert_close_rel(written, 100.0 * energy, 1e-12, &format!("{name}: 100 × E"));
            let pin = pinned.iter().find(|p| p.0 == name).unwrap().1;
            assert_close_rel(written, pin, 1e-10, &format!("{name}: Task 1's number"));
        }
        let before = light_intensities(&photometric);
        assert_eq!(before.iter().map(|b| b.1).collect::<Vec<_>>(), [300.0, 2000.0, 4000.0], "photometric stays 100 × the candela or lux");
        // Colour, decay, cones, range and everything else are unchanged: the
        // three Intensity f64s, copied from the Blender file over the
        // photometric one, make the two files equal.
        let at = |haystack: &[u8], value: f64| {
            let needle = value.to_le_bytes();
            let found: Vec<usize> = (0..haystack.len().saturating_sub(7)).filter(|i| haystack[*i..*i + 8] == needle).collect();
            assert_eq!(found.len(), 1, "the f64 {value} should occur once in the file: at {found:?}");
            found[0]
        };
        let mut patched = photometric.clone();
        for ((_, before), (name, after)) in light_intensities(&photometric).into_iter().zip(light_intensities(&blender)) {
            let (from, to) = (at(&photometric, before), at(&blender, after));
            assert_eq!(from, to, "{name}: the Intensity sits at the same offset in both files");
            patched[from..from + 8].copy_from_slice(&blender[to..to + 8]);
        }
        assert!(patched == blender, "the files differ in more than the three Intensity numbers");
    }

    /// Writing and reading with `Blender` returns each light's intensity to
    /// 1e-6 relative, with its kind, and the whole of every light and camera
    /// (`lightcam::assert_same_lights_and_cameras`, within 1e-5: placement,
    /// colour, range, cones); the file is read differently by the other
    /// units (the energy, not the candela).
    #[test]
    fn fbx_blender_units_round_trip() {
        let staged = fixtures::staged();
        let bytes = write_units(&staged, LightUnits::Blender);
        let back = read_units(&bytes, LightUnits::Blender);
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        assert_eq!(back.scene.lights.len(), 3);
        for want in &staged.lights {
            let got = back.scene.lights.iter().find(|l| l.name == want.name).unwrap_or_else(|| panic!("no light {:?}", want.name));
            assert_close_rel(f64::from(got.intensity), f64::from(want.intensity), 1e-6, &format!("{:?}: intensity", want.name));
            assert_eq!(std::mem::discriminant(&got.kind), std::mem::discriminant(&want.kind), "{:?}", want.name);
        }
        // Where each light stands and shines, its colour, range and cones,
        // and the cameras, at rest and at every sample time.
        lightcam::assert_same_lights_and_cameras(&staged, &back.scene, 1e-5, "staged through FBX in Blender units");
        let as_photometric = read_units(&bytes, LightUnits::Photometric);
        let sun = as_photometric.scene.lights.iter().find(|l| l.name.as_deref() == Some("sun")).unwrap();
        assert!(f64::from(sun.intensity) < 0.01, "read as photometric, the sun is Blender's 0.0044, not 3: {}", sun.intensity);
    }

    /// Review Focus 2: a Blender FBX light reads as Blender's importer reads
    /// it. `Intensity` 2000 is `energy` 20 (`Intensity` / 100, ufbx.c
    /// L22937 and import_fbx.py L2251), which is what a point light of
    /// (2000 / 100) × 683 / 4π candela gets from Blender's glTF import. The
    /// same holds for a sun (lux = 683 × `Intensity` / 100) and a spot, whose
    /// cone changes nothing. Blender's exporter bakes 2^`exposure` into
    /// `Intensity` and writes no `Exposure`, and its importer reads none: a
    /// stray `Exposure` property is not read here either. Photometric units
    /// still read 20 cd.
    #[test]
    fn fbx_blender_reads_intensity_as_blender_does() {
        let spot = format!("{}{}{}{}", pe("LightType", 2), pn("InnerAngle", 20.0), pn("OuterAngle", 50.0), pn("Intensity", 4000.0));
        let parts = [
            light_or_camera(100, "bulb", "Light", &format!("{}{}", pe("LightType", 0), pn("Intensity", 2000.0))),
            light_or_camera(110, "sun", "Light", &format!("{}{}", pe("LightType", 1), pn("Intensity", 300.0))),
            light_or_camera(120, "spot", "Light", &spot),
            light_or_camera(130, "bulb_exposed", "Light", &format!("{}{}{}", pe("LightType", 0), pn("Intensity", 2000.0), pn("Exposure", 3.0))),
        ];
        let objects: String = parts.iter().map(|p| p.0.as_str()).collect();
        let links: String = parts.iter().map(|p| p.1.as_str()).collect();
        let doc = lit_doc("test", 100.0, &objects, &links);
        let read = |units| read_units(doc.as_bytes(), units).scene;
        let blender = read(LightUnits::Blender);
        let candela = (2000.0 / 100.0) * 683.0 / (4.0 * std::f64::consts::PI);
        let by_name = |s: &Scene, name: &str| f64::from(s.lights.iter().find(|l| l.name.as_deref() == Some(name)).unwrap_or_else(|| panic!("no light {name}")).intensity);
        assert_close_rel(by_name(&blender, "bulb"), candela, 1e-6, "a point light of Intensity 2000");
        assert_close_rel(by_name(&blender, "bulb_exposed"), candela, 1e-6, "Exposure is not read");
        assert_close_rel(by_name(&blender, "sun"), 3.0 * 683.0, 1e-6, "a sun of Intensity 300, in lux");
        assert_close_rel(by_name(&blender, "spot"), 40.0 * 683.0 / (4.0 * std::f64::consts::PI), 1e-6, "a spot of Intensity 4000: its cone changes nothing");
        let photometric = read(LightUnits::Photometric);
        assert_close_rel(by_name(&photometric, "bulb"), 20.0, 1e-6, "Photometric reads Intensity / 100 as the candela");
    }

    /// Review Focus 1, with Rulings L4 and L5: no clamp, a drop. A light at
    /// the f32 edge (the largest candela and lux, 1e38 cd, zero and the
    /// smallest subnormal) writes finite `Intensity` numbers, exactly 100 × E
    /// (FBX holds `Intensity` as an f64 and E is below the candela it came
    /// from, so there is nothing to clamp on the way out), and reads back
    /// finite, with the largest candela back as itself. Read the other way,
    /// an `Intensity` whose candela or lux is past f32 is not clamped: it is
    /// dropped with the warning `Photometric` units give an `Intensity` past
    /// f32 (`Photometric` keeps 1e40, `Blender` drops it, both drop 1e42 and
    /// an infinite one), so no light is ever read as inf or NaN.
    #[test]
    fn fbx_blender_units_clamp_at_the_f32_edge() {
        for edge in [[f32::MAX, 1e38, f32::MAX], [0.0, f32::from_bits(1), 1e-30]] {
            let mut scene = fixtures::staged();
            for (light, intensity) in scene.lights.iter_mut().zip(edge) {
                light.intensity = intensity;
            }
            let bytes = write_units(&scene, LightUnits::Blender);
            for (name, written) in light_intensities(&bytes) {
                let light = scene.lights.iter().find(|l| l.name.as_deref() == Some(name.as_str())).unwrap();
                let per_energy = if light.kind == LightKind::Directional { 1.0 / 683.0 } else { 4.0 * std::f64::consts::PI / 683.0 };
                let want = 100.0 * f64::from(light.intensity) * per_energy;
                assert!(written.is_finite(), "{name} at {}: Intensity {written}", light.intensity);
                assert_close_rel(written, want, 1e-12, &format!("{name} at {}", light.intensity));
            }
            let back = read_units(&bytes, LightUnits::Blender);
            assert!(back.warnings.is_empty(), "{:?}", back.warnings);
            assert_eq!(back.scene.lights.len(), 3, "no light is dropped at the edge");
            for want in &scene.lights {
                let got = back.scene.lights.iter().find(|l| l.name == want.name).unwrap().intensity;
                assert!(got.is_finite(), "{:?} read as {got}", want.name);
                assert_close_rel(f64::from(got), f64::from(want.intensity), 1e-6, &format!("{:?} comes back", want.name));
            }
        }
        let huge = |intensity: &str, kind: i64, units: LightUnits| {
            let (objects, links) = light_or_camera(100, "far", "Light", &format!("{}\t\t\tP: \"Intensity\", \"Number\", \"\", \"A\",{intensity}\n", pe("LightType", kind)));
            read_units(lit_doc("test", 100.0, &objects, &links).as_bytes(), units)
        };
        let dropped = "light \"far\" is dropped: its intensity inf is not finite";
        for kind in [0, 1] {
            // 1e40 / 100 = 1e38 fits an f32 as it is, so `Photometric` keeps
            // it; as Blender's energy it is 5.4e39 cd (a point light) or
            // 6.8e40 lux (a sun), past f32, and the light is dropped.
            let r = huge("1e40", kind, LightUnits::Photometric);
            assert_eq!(r.scene.lights.len(), 1, "kind {kind}: Photometric keeps 1e40: {:?}", r.warnings);
            assert_close_rel(f64::from(r.scene.lights[0].intensity), 1e38, 1e-6, &format!("kind {kind}: Photometric reads 1e40 / 100"));
            let r = huge("1e40", kind, LightUnits::Blender);
            assert!(r.scene.lights.is_empty(), "kind {kind}: Blender drops what overflows f32, it does not clamp: {:?}", r.scene.lights);
            assert_eq!(r.warnings, [dropped], "kind {kind}: {:?}", r.warnings);
            // 1e42 / 100 = 1e40 is past f32 in either units: the same drop.
            for units in [LightUnits::Photometric, LightUnits::Blender] {
                let r = huge("1e42", kind, units);
                assert!(r.scene.lights.is_empty(), "kind {kind}, {}: {:?}", units.as_str(), r.scene.lights);
                assert_eq!(r.warnings, [dropped], "kind {kind}, {}: {:?}", units.as_str(), r.warnings);
            }
            // An infinite `Intensity` is never read as an infinite light.
            for units in [LightUnits::Photometric, LightUnits::Blender] {
                let r = huge("1.#INF00", kind, units);
                assert!(r.scene.lights.is_empty(), "kind {kind}, {}: an infinite Intensity is dropped", units.as_str());
                assert_eq!(r.warnings, [dropped], "kind {kind}, {}: {:?}", units.as_str(), r.warnings);
            }
        }
    }

    /// Review Focus 4: a scene with no lights writes the same bytes with
    /// either units. `staged`'s do differ, so the comparison has something
    /// to catch.
    #[test]
    fn fbx_blender_units_leave_a_lightless_scene_alone() {
        let cube = fixtures::cube();
        assert!(cube.lights.is_empty());
        assert_eq!(write_units(&cube, LightUnits::Blender), write_units(&cube, LightUnits::Photometric));
        assert_eq!(write_units(&cube, LightUnits::Blender), write_prepared(&cube), "and the default's");
        let staged = fixtures::staged();
        assert_ne!(write_units(&staged, LightUnits::Blender), write_units(&staged, LightUnits::Photometric));
    }

    /// Review Focus 5: a spot's energy ignores its cone (Blender's spot
    /// `energy` is the power of the point light of the same intensity). A
    /// point light and two spots of 40 cd, one narrow and one near a
    /// hemisphere, write the same `Intensity` (100 × E = 73.595142690244),
    /// and read back as 40 cd each, with their cones.
    #[test]
    fn fbx_spot_energy_ignores_the_cone() {
        let mut scene = fixtures::staged();
        let kinds = [LightKind::Point, LightKind::Spot { inner_cone: 0.1, outer_cone: 0.2 }, LightKind::Spot { inner_cone: 0.5, outer_cone: 1.5 }];
        for (light, kind) in scene.lights.iter_mut().zip(kinds) {
            light.kind = kind;
            light.intensity = 40.0;
        }
        let bytes = write_units(&scene, LightUnits::Blender);
        let written = light_intensities(&bytes);
        assert_eq!(written.len(), 3);
        for (name, intensity) in &written {
            assert_close_rel(*intensity, 73.595142690244, 1e-10, name);
        }
        let back = read_units(&bytes, LightUnits::Blender);
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        for want in &scene.lights {
            let got = back.scene.lights.iter().find(|l| l.name == want.name).unwrap();
            assert_close_rel(f64::from(got.intensity), 40.0, 1e-6, &format!("{:?}", want.name));
            assert_eq!(std::mem::discriminant(&got.kind), std::mem::discriminant(&want.kind));
        }
    }
}
