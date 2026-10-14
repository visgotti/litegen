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
//!   both ways.
//! - **Colours.** See [`srgb_encode`]: written sRGB, read back to linear.
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
//!
//! # What is refused
//!
//! FBX 6.x and older (a different object model: string names, `Properties60`),
//! encrypted files, and the bytes of any other container.

use super::ir::Mesh;
use super::package::Sidecars;
use super::scene::{AlphaMode, Capabilities, ImageFormat, Loss, LossKind, MaterialSupport, Scene, Wrap};
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
/// three.js's FBXLoader decodes `DiffuseColor`, `EmissiveColor`, `Specular`
/// and every vertex colour as sRGB (`ColorManagement.colorSpaceToWorking(…,
/// SRGBColorSpace)`), Blender writes and reads vertex colours as sRGB by
/// default, while Unity URP's MaterialDescription path treats material colours
/// as linear (research notes §4.2, item 5). The scene is linear, so one of them
/// will be off. three.js is the engine behind the product's own viewer, so it
/// is the one that must show the colour the customer asked for: colours are
/// encoded to sRGB here and decoded from sRGB on read ([`srgb_decode`]), which
/// makes a converted file render exactly like its glTF source in the dashboard,
/// and a Unity user sees the colour slightly lighter rather than the dashboard
/// showing it darker.
pub(super) fn srgb_encode(linear: f32) -> f64 {
    super::math::linear_to_srgb(linear) as f64
}

/// sRGB → linear for a colour read from FBX. See [`srgb_encode`].
pub(super) fn srgb_decode(encoded: f64) -> f32 {
    super::math::srgb_to_linear(encoded as f32)
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
    let dom = parse(bytes)?;
    super::fbx_read::read(&dom, sidecars)
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
pub fn write_scene(scene: &Scene) -> Result<Vec<u8>, MeshError> {
    super::fbx_write::write(scene)
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
    }
}

/// The scene reduced to exactly what [`write_scene`] encodes: the generic
/// Phong reduction, then what FBX's material and sampler model cannot say.
pub fn prepare(scene: &Scene, opts: WriteOptions) -> Result<Scene, MeshError> {
    let _ = opts;
    let mut s = super::pose::degrade(scene, &capabilities())?;
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
        for r in [&mut m.base_color_texture, &mut m.normal_texture, &mut m.emissive_texture] {
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
    out
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::fixtures;
    use super::super::package::{MemorySidecars, NoSidecars, PackageFile};
    use super::super::pose;
    use super::super::scene::{
        AnimPath, Animation, Channel, Interpolation, Material, MorphTarget, Node, Primitive, SceneMesh, Skin,
        SkinWeights, Transform,
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
            assert_eq!(got.animations.len(), prepared.animations.len().min(1), "{name}: animations");
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

    #[test]
    fn without_rotation_active_pre_rotation_and_order_are_ignored() {
        let props = "\t\t\tP: \"RotationOrder\", \"enum\", \"\", \"\",5\n\t\t\tP: \"PreRotation\", \"Vector3D\", \"Vector\", \"\",90,0,0\n\t\t\tP: \"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\",10,20,30";
        let got = world_triangle(&transform_doc(&[(10, 0, props)], ""), None);
        let m = math::quat_to_mat(math::euler_to_quat(deg([10.0, 20.0, 30.0]), math::EulerOrder::Xyz));
        assert_points(&got, &m, "RotationActive 0");
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
        let mut s = read_back(bytes).scene;
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
}
